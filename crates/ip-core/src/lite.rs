//! `LiteAnalyzer`: on-device analysis without Python (`docs/api-contract-m8.md` section B).
//!
//! [`LiteWorker`] implements [`AiWorker`] for the `analyze.batch` method only: it decodes each
//! photo at analysis size through [`Imaging`] and runs the pure-Rust ports of the worker's
//! `phash` and `quality` steps (`ip-lite`). No faces, no embeddings: grouping then relies on the
//! time line plus pHash, and scoring renormalises over the available components (M2).

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use ip_imaging::ImageFormat;
use ip_worker_client::{
    AiWorker, AnalyzeItem, AnalyzeRequest, AnalyzeResponse, CancelToken, ItemError, ModelsListing,
    ProfileInfo, ProgressTx, QualityInfo, Result, SystemInfo, WorkerError, WorkerState,
    WorkerStatus,
};
use serde_json::json;
use tokio::sync::watch;

use crate::imaging::Imaging;

/// Steps the lite backend runs.
pub const LITE_STEPS: [&str; 2] = ["phash", "quality"];

/// Item-level error code for undecodable images (same as the Python worker's decode error).
const CODE_DECODE: i64 = -32020;

pub struct LiteWorker {
    imaging: Arc<dyn Imaging>,
    pool: Arc<rayon::ThreadPool>,
    tx: watch::Sender<WorkerStatus>,
}

/// Analysis threads: all cores on desktop, at most 4 on phones (thermals, memory).
pub fn default_threads() -> usize {
    let n = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2);
    if cfg!(target_os = "android") {
        n.clamp(1, 4)
    } else {
        n.max(1)
    }
}

impl LiteWorker {
    pub fn new(imaging: Arc<dyn Imaging>, threads: usize) -> Self {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads.max(1))
            .thread_name(|i| format!("ip-lite-{i}"))
            .build()
            .expect("lite thread pool");
        let (tx, _) = watch::channel(WorkerStatus {
            state: WorkerState::Ready,
            tier: Some("lite".into()),
            error: None,
            info: Some(lite_info()),
        });
        Self {
            imaging,
            pool: Arc::new(pool),
            tx,
        }
    }
}

fn lite_info() -> SystemInfo {
    SystemInfo {
        tier: Some("lite".into()),
        providers: vec!["cpu".into()],
        hardware: ip_worker_client::HardwareJson {
            device: "cpu".into(),
            gpus: vec![],
        },
    }
}

/// Analyses one decoded-on-demand photo.
pub fn analyze_one(
    imaging: &dyn Imaging,
    photo_id: i64,
    path: &str,
    orient: u8,
    size: u32,
) -> AnalyzeItem {
    let fail = |msg: String| AnalyzeItem {
        photo_id,
        error: Some(ItemError {
            code: CODE_DECODE,
            message: msg,
            kind: Some("decode_failed".into()),
        }),
        ..Default::default()
    };
    let p = Path::new(path);
    let Some(format) = ImageFormat::from_path(p) else {
        return fail(format!("unsupported image format: {path}"));
    };
    let (w, h, rgb) = match imaging.decode_rgb8(p, format, orient, size) {
        Ok(d) => d,
        Err(e) => return fail(format!("cannot decode {path}: {e:#}")),
    };
    let (wu, hu) = (w as usize, h as usize);
    let q = ip_lite::analyze_quality(&rgb, wu, hu);
    AnalyzeItem {
        photo_id,
        width: Some(w as i64),
        height: Some(h as i64),
        phash: Some(ip_lite::phash_hex(&rgb, wu, hu)),
        sharpness: Some(q.sharpness),
        exposure: Some(q.exposure),
        noise: Some(q.noise),
        quality: Some(QualityInfo {
            sharpness_center: Some(q.sharpness_center),
            sharpness_face: None,
            mean_luminance: Some(q.mean_luminance),
            clipped_highlights: Some(q.clipped_highlights),
            crushed_shadows: Some(q.crushed_shadows),
            noise_sigma: Some(q.noise_sigma),
        }),
        ..Default::default()
    }
}

#[async_trait::async_trait]
impl AiWorker for LiteWorker {
    fn status(&self) -> WorkerStatus {
        self.tx.borrow().clone()
    }
    fn subscribe(&self) -> watch::Receiver<WorkerStatus> {
        self.tx.subscribe()
    }
    async fn system_info(&self) -> Result<SystemInfo> {
        Ok(lite_info())
    }
    async fn models_list(&self) -> Result<ModelsListing> {
        let mut l = ModelsListing::default();
        l.profiles.insert(
            "lite".into(),
            ProfileInfo {
                steps: LITE_STEPS.iter().map(|s| s.to_string()).collect(),
                models: vec![],
            },
        );
        Ok(l)
    }
    async fn models_ensure(
        &self,
        _ids: &[String],
        _progress: Option<ProgressTx>,
        _cancel: &CancelToken,
    ) -> Result<()> {
        Ok(())
    }
    async fn analyze_batch(
        &self,
        req: &AnalyzeRequest,
        progress: Option<ProgressTx>,
        cancel: &CancelToken,
    ) -> Result<AnalyzeResponse> {
        let started = std::time::Instant::now();
        let (imaging, pool) = (self.imaging.clone(), self.pool.clone());
        let (items, cancel2) = (req.items.clone(), cancel.clone());
        let (size, total) = (req.analysis_size.max(64), req.items.len());
        let done = Arc::new(AtomicUsize::new(0));
        let out: Vec<Option<AnalyzeItem>> = tokio::task::spawn_blocking(move || {
            use rayon::prelude::*;
            pool.install(|| {
                items
                    .par_iter()
                    .map(|it| {
                        if cancel2.is_cancelled() {
                            return None;
                        }
                        let r = analyze_one(&*imaging, it.photo_id, &it.path, it.orientation, size);
                        let n = done.fetch_add(1, Ordering::SeqCst) + 1;
                        if let Some(tx) = &progress {
                            let _ = tx.send(json!({"kind": "analyze", "done": n, "total": total}));
                        }
                        Some(r)
                    })
                    .collect()
            })
        })
        .await
        .map_err(|e| WorkerError::Protocol(format!("lite analysis crashed: {e}")))?;
        if cancel.is_cancelled() {
            return Err(WorkerError::Cancelled);
        }
        Ok(AnalyzeResponse {
            items: out.into_iter().flatten().collect(),
            steps: LITE_STEPS.iter().map(|s| s.to_string()).collect(),
            skipped_steps: vec![],
            models: json!({"lite": format!("ip-lite {}", env!("CARGO_PKG_VERSION"))}),
            warnings: vec![],
            timings: json!({"total_ms": started.elapsed().as_millis() as u64}),
        })
    }
    async fn shutdown(&self) {}
}
