//! `LiteAnalyzer`: on-device analysis without Python (`docs/api-contract-m8.md` section B).
//!
//! [`LiteWorker`] implements [`AiWorker`] for the `analyze.batch` method only: it decodes each
//! photo at analysis size through [`Imaging`] and runs the pure-Rust ports of the worker's
//! `phash` and `quality` steps (`ip-lite`). No faces, no embeddings: grouping then relies on the
//! time line plus pHash, and scoring renormalises over the available components (M2).
//!
//! With the optional `infer` feature and a YuNet model on disk (`IMAGEPICKER_YUNET_MODEL` or
//! `<models>/yunet/face_detection_yunet_2023mar.onnx`) a `faces` step runs after `quality`
//! (`ip-infer`, `docs/api-contract-m8.md` section B.2): bbox, detection score and face
//! sharpness; no eyes / smile / embeddings, so scoring renormalises over what is present.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use ip_imaging::ImageFormat;
use ip_worker_client::{
    AiWorker, AnalyzeFace, AnalyzeItem, AnalyzeRequest, AnalyzeResponse, CancelToken, ItemError,
    ModelsListing, ProfileInfo, ProgressTx, QualityInfo, Result, SystemInfo, WorkerError,
    WorkerState, WorkerStatus,
};
use serde_json::json;
use tokio::sync::watch;

use crate::imaging::Imaging;

/// Steps the lite backend always runs (`faces` is appended when a detector is loaded).
pub const LITE_STEPS: [&str; 2] = ["phash", "quality"];

/// Faces kept per photo (same cap as the Python worker).
const MAX_FACES: usize = 30;

#[cfg(feature = "infer")]
use ip_infer::FaceDetector;
/// Without the `infer` feature no detector can exist.
#[cfg(not(feature = "infer"))]
pub enum FaceDetector {}

#[cfg(not(feature = "infer"))]
impl FaceDetector {
    fn detect(&self, _rgb: &[u8], _w: usize, _h: usize) -> Vec<Det> {
        match *self {}
    }
}
#[cfg(not(feature = "infer"))]
struct Det {
    bbox: [f32; 4],
    score: f32,
}

/// Item-level error code for undecodable images (same as the Python worker's decode error).
const CODE_DECODE: i64 = -32020;

pub struct LiteWorker {
    imaging: Arc<dyn Imaging>,
    pool: Arc<rayon::ThreadPool>,
    tx: watch::Sender<WorkerStatus>,
    faces: Option<Arc<FaceDetector>>,
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
            faces: None,
        }
    }

    /// Enables the `faces` step when the `infer` feature is built in and a YuNet model is found
    /// (`IMAGEPICKER_YUNET_MODEL`, then `<models_dir>/yunet/`). A model that fails to load is
    /// logged and ignored.
    #[allow(unused_mut)]
    pub fn with_models_dir(mut self, models_dir: &Path) -> Self {
        #[cfg(feature = "infer")]
        if let Some(path) = ip_infer::find_yunet(Some(models_dir)) {
            match FaceDetector::load(&path) {
                Ok(d) => self.faces = Some(Arc::new(d)),
                Err(e) => tracing::warn!("YuNet at {} unusable: {e:#}", path.display()),
            }
        }
        #[cfg(not(feature = "infer"))]
        let _ = models_dir;
        self
    }

    /// Steps this worker runs.
    pub fn steps(&self) -> Vec<String> {
        steps_for(self.faces.is_some())
    }
}

fn steps_for(faces: bool) -> Vec<String> {
    let mut v: Vec<String> = LITE_STEPS.iter().map(|s| s.to_string()).collect();
    if faces {
        v.push("faces".into());
    }
    v
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
    analyze_one_with(imaging, photo_id, path, orient, size, None)
}

fn analyze_one_with(
    imaging: &dyn Imaging,
    photo_id: i64,
    path: &str,
    orient: u8,
    size: u32,
    detector: Option<&FaceDetector>,
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
    let faces = detector.map(|d| detect_faces(d, &rgb, wu, hu));
    let face_sharp: Vec<f64> = faces.iter().flatten().filter_map(|f| f.sharpness).collect();
    // Like the worker: the headline sharpness follows the faces when there are any.
    let sharpness_face = (!face_sharp.is_empty())
        .then(|| round5(face_sharp.iter().sum::<f64>() / face_sharp.len() as f64));
    AnalyzeItem {
        photo_id,
        width: Some(w as i64),
        height: Some(h as i64),
        phash: Some(ip_lite::phash_hex(&rgb, wu, hu)),
        sharpness: Some(sharpness_face.unwrap_or(q.sharpness)),
        exposure: Some(q.exposure),
        noise: Some(q.noise),
        quality: Some(QualityInfo {
            sharpness_center: Some(q.sharpness_center),
            sharpness_face,
            mean_luminance: Some(q.mean_luminance),
            clipped_highlights: Some(q.clipped_highlights),
            crushed_shadows: Some(q.crushed_shadows),
            noise_sigma: Some(q.noise_sigma),
        }),
        faces: faces.unwrap_or_default(),
        ..Default::default()
    }
}

fn round5(x: f64) -> f64 {
    (x * 1e5).round() / 1e5
}

/// The worker's faces step on an upright RGB8 image: boxes clamped to the image (normalised
/// `[x, y, w, h]`), largest first, with the crop sharpness; eyes / smile / pose stay `None`.
fn detect_faces(det: &FaceDetector, rgb: &[u8], w: usize, h: usize) -> Vec<AnalyzeFace> {
    let mut out: Vec<AnalyzeFace> = Vec::new();
    for d in det.detect(rgb, w, h).iter().take(MAX_FACES) {
        let (fw, fh) = (w as f64, h as f64);
        let x0 = (d.bbox[0] as f64).max(0.0);
        let y0 = (d.bbox[1] as f64).max(0.0);
        let x1 = ((d.bbox[0] + d.bbox[2]) as f64).min(1.0);
        let y1 = ((d.bbox[1] + d.bbox[3]) as f64).min(1.0);
        if (x1 - x0) * fw < 4.0 || (y1 - y0) * fh < 4.0 {
            continue;
        }
        let bbox = [round5(x0), round5(y0), round5(x1 - x0), round5(y1 - y0)];
        out.push(AnalyzeFace {
            bbox,
            det_score: Some((d.score as f64 * 1e4).round() / 1e4),
            sharpness: ip_lite::face_sharpness(rgb, w, h, bbox).map(round5),
            ..Default::default()
        });
    }
    out.sort_by(|a, b| (b.bbox[2] * b.bbox[3]).total_cmp(&(a.bbox[2] * a.bbox[3])));
    out
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
                steps: self.steps(),
                models: if self.faces.is_some() {
                    vec!["yunet".into()]
                } else {
                    vec![]
                },
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
        let detector = self.faces.clone();
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
                        let r = analyze_one_with(
                            &*imaging,
                            it.photo_id,
                            &it.path,
                            it.orientation,
                            size,
                            detector.as_deref(),
                        );
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
            steps: steps_for(self.faces.is_some()),
            skipped_steps: vec![],
            models: if self.faces.is_some() {
                json!({"lite": format!("ip-lite {}", env!("CARGO_PKG_VERSION")), "faces": "yunet"})
            } else {
                json!({"lite": format!("ip-lite {}", env!("CARGO_PKG_VERSION"))})
            },
            warnings: vec![],
            timings: json!({"total_ms": started.elapsed().as_millis() as u64}),
        })
    }
    async fn shutdown(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::FakeImaging;

    #[test]
    fn steps_without_a_model_are_phash_and_quality() {
        if std::env::var_os("IMAGEPICKER_YUNET_MODEL").is_some() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let w = LiteWorker::new(Arc::new(FakeImaging::new()), 1).with_models_dir(dir.path());
        assert_eq!(w.steps(), vec!["phash", "quality"]);
    }

    /// Needs `--features infer`, `IMAGEPICKER_YUNET_MODEL` and `IMAGEPICKER_FACE_TEST_IMAGES`
    /// (an image with a face); skips otherwise.
    #[cfg(feature = "infer")]
    #[test]
    fn faces_step_reports_faces_with_sharpness() {
        let Some(img) = std::env::var_os("IMAGEPICKER_FACE_TEST_IMAGES")
            .and_then(|l| std::env::split_paths(&l).next())
        else {
            eprintln!("skip: IMAGEPICKER_FACE_TEST_IMAGES is not set");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let w = LiteWorker::new(Arc::new(FakeImaging::new()), 1).with_models_dir(dir.path());
        if w.faces.is_none() {
            eprintln!("skip: no YuNet model (IMAGEPICKER_YUNET_MODEL)");
            return;
        }
        assert_eq!(w.steps(), vec!["phash", "quality", "faces"]);
        let it = analyze_one_with(
            &FakeImaging::new(),
            1,
            img.to_str().unwrap(),
            1,
            1024,
            w.faces.as_deref(),
        );
        assert!(it.error.is_none(), "{:?}", it.error);
        assert!(!it.faces.is_empty());
        let f = &it.faces[0];
        println!("faces: {:?}", it.faces);
        assert!(f.det_score.unwrap() >= 0.7);
        assert!(f.sharpness.is_some());
        assert!(f.eyes_open.is_none() && f.smile.is_none());
        assert_eq!(it.quality.unwrap().sharpness_face, f.sharpness);
    }
}
