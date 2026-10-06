//! A deterministic in-process stand-in for the Python worker (tests only; feature `testutil`).
//! It writes real `.npy` artifacts so the production ingest path is exercised.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use ip_worker_client::*;
use serde_json::json;
use tokio::sync::watch;

use crate::analysis::vecs::write_npy_f16;

/// Id of the fake model portrait retouching needs (`required_for: ["beauty"]`).
pub const BEAUTY_MODEL: &str = "mediapipe-face";

/// One synthetic face.
#[derive(Clone, Debug)]
pub struct FakeFace {
    pub bbox: [f64; 4],
    /// Identity embedding (normalised like AuraFace output).
    pub identity: Vec<f32>,
    pub eyes_open: Option<f64>,
    pub smile: Option<f64>,
    pub gaze: Option<f64>,
    pub yaw: Option<f64>,
    pub pitch: Option<f64>,
    pub sharpness: Option<f64>,
}

impl FakeFace {
    pub fn new(bbox: [f64; 4], identity: Vec<f32>) -> Self {
        Self {
            bbox,
            identity,
            eyes_open: Some(0.95),
            smile: Some(0.7),
            gaze: Some(0.9),
            yaw: Some(2.0),
            pitch: Some(1.0),
            sharpness: Some(0.85),
        }
    }
    pub fn eyes(mut self, v: f64) -> Self {
        self.eyes_open = Some(v);
        self
    }
    pub fn smile(mut self, v: f64) -> Self {
        self.smile = Some(v);
        self
    }
}

/// Synthetic analysis result for a file (looked up by file name).
#[derive(Clone, Debug)]
pub struct FakeSpec {
    pub emb: Vec<f32>,
    pub phash: u64,
    pub sharpness: f64,
    pub exposure: f64,
    pub noise: f64,
    pub iqa: Option<f64>,
    pub aesthetic: Option<f64>,
    pub scene: Option<String>,
    pub mean_luminance: f64,
    pub clipped_highlights: f64,
    pub crushed_shadows: f64,
    pub faces: Vec<FakeFace>,
    pub fail: bool,
}

impl Default for FakeSpec {
    fn default() -> Self {
        Self {
            emb: unit(0.0),
            phash: 0xF0F0_F0F0_F0F0_F0F0,
            sharpness: 0.8,
            exposure: 0.8,
            noise: 0.1,
            iqa: Some(0.7),
            aesthetic: Some(0.6),
            scene: None,
            mean_luminance: 0.45,
            clipped_highlights: 0.0,
            crushed_shadows: 0.0,
            faces: vec![],
            fail: false,
        }
    }
}

impl FakeSpec {
    pub fn at(angle_deg: f32) -> Self {
        Self {
            emb: unit(angle_deg),
            ..Default::default()
        }
    }
    pub fn sharp(mut self, v: f64) -> Self {
        self.sharpness = v;
        self
    }
    pub fn with_faces(mut self, faces: Vec<FakeFace>) -> Self {
        self.faces = faces;
        self
    }
    pub fn scene(mut self, s: &str) -> Self {
        self.scene = Some(s.into());
        self
    }
}

/// 4-d unit vector at an angle: the cosine of two of them is cos(angle difference).
pub fn unit(angle_deg: f32) -> Vec<f32> {
    let a = angle_deg.to_radians();
    vec![a.cos(), a.sin(), 0.0, 0.0]
}

/// Identity embedding of "person #k" (distinct 8-d directions; `jitter_deg` varies it slightly).
pub fn person(k: usize, jitter_deg: f32) -> Vec<f32> {
    let mut v = vec![0.0f32; 8];
    v[k % 8] = jitter_deg.to_radians().cos();
    v[(k + 1) % 8] = jitter_deg.to_radians().sin();
    v
}

pub struct FakeWorker {
    script: Mutex<HashMap<String, FakeSpec>>,
    tx: watch::Sender<WorkerStatus>,
    missing: Mutex<Vec<String>>,
    skipped: Mutex<Vec<String>>,
    pub analyze_calls: AtomicUsize,
    pub batch_sizes: Mutex<Vec<usize>>,
    /// Milliseconds each batch takes (to test cancellation / progress).
    pub delay_ms: AtomicUsize,
    pub last_profile: Mutex<Option<String>>,
    pub last_allow_download: Mutex<Option<bool>>,
    /// Number of upcoming batches that fail with `Disconnected` (simulated crashes).
    pub crash_batches: AtomicUsize,
    /// `mask.generate` calls received.
    pub mask_calls: AtomicUsize,
    pub last_mask_request: Mutex<Option<MaskRequest>>,
    /// Model ids reported by a `-32010 model_unavailable` error (non-empty = masks unavailable
    /// unless the request allows downloads).
    mask_missing: Mutex<Vec<String>>,
    /// Targets answered in `skipped` instead of `masks`.
    mask_skipped: Mutex<Vec<String>>,
    /// Makes `mask.generate` fail as if the worker could not start.
    pub mask_unavailable: std::sync::atomic::AtomicBool,
    /// `beauty.prepare` calls received.
    pub beauty_calls: AtomicUsize,
    pub last_beauty_request: Mutex<Option<BeautyPrepareRequest>>,
    /// Makes `beauty.prepare` fail as if the worker could not start.
    pub beauty_unavailable: std::sync::atomic::AtomicBool,
    /// What `faces.embed` returns for every image: `(bbox, embedding)` per face.
    embed_faces: Mutex<Vec<([f64; 4], Vec<f32>)>>,
    pub embed_calls: AtomicUsize,
    pub last_embed_request: Mutex<Option<FacesEmbedRequest>>,
}

impl Default for FakeWorker {
    fn default() -> Self {
        let (tx, _) = watch::channel(WorkerStatus::stopped());
        Self {
            script: Mutex::new(HashMap::new()),
            tx,
            missing: Mutex::new(vec![]),
            skipped: Mutex::new(vec![]),
            analyze_calls: AtomicUsize::new(0),
            batch_sizes: Mutex::new(vec![]),
            delay_ms: AtomicUsize::new(0),
            last_profile: Mutex::new(None),
            last_allow_download: Mutex::new(None),
            crash_batches: AtomicUsize::new(0),
            mask_calls: AtomicUsize::new(0),
            last_mask_request: Mutex::new(None),
            mask_missing: Mutex::new(vec![]),
            mask_skipped: Mutex::new(vec![]),
            mask_unavailable: std::sync::atomic::AtomicBool::new(false),
            beauty_calls: AtomicUsize::new(0),
            last_beauty_request: Mutex::new(None),
            beauty_unavailable: std::sync::atomic::AtomicBool::new(false),
            embed_faces: Mutex::new(vec![]),
            embed_calls: AtomicUsize::new(0),
            last_embed_request: Mutex::new(None),
        }
    }
}

fn hash(s: &str) -> u64 {
    let h = blake3::hash(s.as_bytes());
    u64::from_le_bytes(h.as_bytes()[..8].try_into().unwrap())
}

impl FakeWorker {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn set(&self, file_name: &str, spec: FakeSpec) {
        self.script
            .lock()
            .unwrap()
            .insert(file_name.to_string(), spec);
    }
    pub fn set_missing(&self, ids: &[&str]) {
        *self.missing.lock().unwrap() = ids.iter().map(|s| s.to_string()).collect();
    }
    pub fn set_mask_missing(&self, ids: &[&str]) {
        *self.mask_missing.lock().unwrap() = ids.iter().map(|s| s.to_string()).collect();
    }
    pub fn set_mask_skipped(&self, targets: &[&str]) {
        *self.mask_skipped.lock().unwrap() = targets.iter().map(|s| s.to_string()).collect();
    }
    /// Faces `faces.embed` reports for any image (embeddings are L2-normalised on the way out).
    pub fn set_embed_faces(&self, faces: Vec<([f64; 4], Vec<f32>)>) {
        *self.embed_faces.lock().unwrap() = faces;
    }
    pub fn set_skipped(&self, steps: &[&str]) {
        *self.skipped.lock().unwrap() = steps.iter().map(|s| s.to_string()).collect();
    }
    fn spec_for(&self, name: &str) -> FakeSpec {
        self.script
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .unwrap_or_else(|| {
                // unscripted files: a deterministic, mutually dissimilar picture
                let h = hash(name);
                FakeSpec {
                    emb: unit((h % 360) as f32),
                    phash: h,
                    ..Default::default()
                }
            })
    }
    fn set_state(&self, state: WorkerState) {
        self.tx.send_modify(|s| {
            s.state = state;
            if state == WorkerState::Ready {
                s.tier = Some("T3".into());
            }
        });
    }
}

#[async_trait::async_trait]
impl AiWorker for FakeWorker {
    fn status(&self) -> WorkerStatus {
        self.tx.borrow().clone()
    }
    fn subscribe(&self) -> watch::Receiver<WorkerStatus> {
        self.tx.subscribe()
    }
    async fn system_info(&self) -> Result<SystemInfo> {
        self.set_state(WorkerState::Ready);
        let info = SystemInfo {
            tier: Some("T3".into()),
            providers: vec![
                "CUDAExecutionProvider".into(),
                "CPUExecutionProvider".into(),
            ],
            hardware: HardwareJson {
                device: "cuda".into(),
                gpus: vec![GpuInfo {
                    name: "Fake RTX".into(),
                    vram_mb: Some(24564),
                }],
            },
        };
        self.tx.send_modify(|s| s.info = Some(info.clone()));
        Ok(info)
    }
    async fn models_list(&self) -> Result<ModelsListing> {
        self.set_state(WorkerState::Ready);
        let missing = self.missing.lock().unwrap().clone();
        let m = |id: &str, size: f64, req: &[&str]| WorkerModel {
            id: id.into(),
            task: vec!["fake".into()],
            size_mb: size,
            license: "Apache-2.0".into(),
            noncommercial: false,
            installed: !missing.iter().any(|x| x == id),
            recommended: true,
            optional: false,
            tiers: vec!["T3".into()],
            required_for: req.iter().map(|s| s.to_string()).collect(),
        };
        let mut profiles = std::collections::BTreeMap::new();
        profiles.insert(
            "fast".to_string(),
            ProfileInfo {
                steps: vec!["phash".into(), "quality".into(), "faces".into()],
                models: vec!["yunet".into()],
            },
        );
        profiles.insert(
            "standard".to_string(),
            ProfileInfo {
                steps: vec![
                    "phash".into(),
                    "quality".into(),
                    "faces".into(),
                    "embed".into(),
                ],
                models: vec!["yunet".into(), "siglip2-base".into()],
            },
        );
        let mut beauty = m(BEAUTY_MODEL, 12.0, &["beauty"]);
        beauty.recommended = false;
        beauty.optional = true;
        Ok(ModelsListing {
            models: vec![
                m("yunet", 0.3, &["faces"]),
                m("siglip2-base", 178.0, &["embed"]),
                beauty,
            ],
            profiles,
        })
    }
    async fn models_ensure(
        &self,
        ids: &[String],
        progress: Option<ProgressTx>,
        _cancel: &CancelToken,
    ) -> Result<()> {
        for id in ids {
            if let Some(tx) = &progress {
                let _ = tx.send(json!({"kind":"model.download","model":id,"done":50,"total":100,"phase":"download"}));
                let _ = tx.send(json!({"kind":"model.download","model":id,"done":100,"total":100,"phase":"done"}));
            }
            self.missing.lock().unwrap().retain(|m| m != id);
        }
        Ok(())
    }
    async fn analyze_batch(
        &self,
        req: &AnalyzeRequest,
        progress: Option<ProgressTx>,
        cancel: &CancelToken,
    ) -> Result<AnalyzeResponse> {
        self.set_state(WorkerState::Busy);
        self.analyze_calls.fetch_add(1, Ordering::SeqCst);
        self.batch_sizes.lock().unwrap().push(req.items.len());
        *self.last_profile.lock().unwrap() = req.profile.clone();
        *self.last_allow_download.lock().unwrap() = Some(req.allow_download);
        if take_one(&self.crash_batches) {
            self.set_state(WorkerState::Crashed);
            return Err(WorkerError::Disconnected);
        }
        let delay = self.delay_ms.load(Ordering::SeqCst);
        if delay > 0 {
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_millis(delay as u64)) => {}
                _ = cancel.cancelled() => {
                    self.set_state(WorkerState::Ready);
                    return Err(WorkerError::Cancelled);
                }
            }
        }
        let standard = req.profile.as_deref() != Some("fast");
        let out = std::path::PathBuf::from(&req.out_dir);
        std::fs::create_dir_all(&out).map_err(|e| WorkerError::Protocol(e.to_string()))?;
        let mut items = Vec::new();
        for (n, it) in req.items.iter().enumerate() {
            let name = std::path::Path::new(&it.path)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let spec = self.spec_for(&name);
            if spec.fail {
                items.push(AnalyzeItem {
                    photo_id: it.photo_id,
                    error: Some(ItemError {
                        code: -32020,
                        message: "decode failed".into(),
                        kind: Some("decode_failed".into()),
                    }),
                    ..Default::default()
                });
                continue;
            }
            let mut item = AnalyzeItem {
                photo_id: it.photo_id,
                phash: Some(format!("{:016x}", spec.phash)),
                sharpness: Some(spec.sharpness),
                exposure: Some(spec.exposure),
                noise: Some(spec.noise),
                quality: Some(QualityInfo {
                    mean_luminance: Some(spec.mean_luminance),
                    clipped_highlights: Some(spec.clipped_highlights),
                    crushed_shadows: Some(spec.crushed_shadows),
                    ..Default::default()
                }),
                faces: spec
                    .faces
                    .iter()
                    .enumerate()
                    .map(|(i, f)| AnalyzeFace {
                        bbox: f.bbox,
                        det_score: Some(0.99),
                        sharpness: f.sharpness,
                        eyes_open: f.eyes_open,
                        smile: f.smile,
                        gaze: f.gaze,
                        yaw: f.yaw,
                        pitch: f.pitch,
                        roll: Some(0.0),
                        identity_index: standard.then_some(i),
                        blendshapes: None,
                    })
                    .collect(),
                ..Default::default()
            };
            if standard {
                let p = out.join(format!("{}.emb.npy", it.photo_id));
                std::fs::write(&p, write_npy_f16(&[spec.emb.len()], &spec.emb))
                    .map_err(|e| WorkerError::Protocol(e.to_string()))?;
                item.embedding_file = Some(p.to_string_lossy().into_owned());
                item.embedding_model = Some("fake-siglip".into());
                item.iqa = spec.iqa;
                item.aesthetic = spec.aesthetic;
                item.scene_type = spec.scene.clone();
                if !spec.faces.is_empty() {
                    let dim = spec.faces[0].identity.len();
                    let mut data = Vec::new();
                    for f in &spec.faces {
                        let mut v = f.identity.clone();
                        crate::analysis::vecs::normalize(&mut v);
                        data.extend(v);
                    }
                    let p = out.join(format!("{}.faces.npy", it.photo_id));
                    std::fs::write(&p, write_npy_f16(&[spec.faces.len(), dim], &data))
                        .map_err(|e| WorkerError::Protocol(e.to_string()))?;
                    item.identity_file = Some(p.to_string_lossy().into_owned());
                }
            }
            if let Some(tx) = &progress {
                let _ = tx.send(json!({"kind":"analyze","done":n + 1,"total":req.items.len()}));
            }
            items.push(item);
        }
        self.set_state(WorkerState::Ready);
        Ok(AnalyzeResponse {
            items,
            steps: vec![],
            skipped_steps: self.skipped.lock().unwrap().clone(),
            models: json!({"embed": "fake-siglip"}),
            warnings: vec![],
            timings: json!({}),
        })
    }
    async fn mask_generate(&self, req: &MaskRequest) -> Result<MaskResponse> {
        self.mask_calls.fetch_add(1, Ordering::SeqCst);
        *self.last_mask_request.lock().unwrap() = Some(req.clone());
        if self.mask_unavailable.load(Ordering::SeqCst) {
            return Err(WorkerError::Unavailable("fake worker is down".into()));
        }
        let missing = self.mask_missing.lock().unwrap().clone();
        if !missing.is_empty() && !req.allow_download {
            return Err(WorkerError::Rpc {
                code: ip_worker_client::error::CODE_MODEL_UNAVAILABLE,
                message: "model unavailable".into(),
                kind: Some("model_unavailable".into()),
                detail: Some(json!({ "models": missing })),
            });
        }
        let skipped = self.mask_skipped.lock().unwrap().clone();
        let out = std::path::PathBuf::from(&req.out_dir);
        std::fs::create_dir_all(&out).map_err(|e| WorkerError::Protocol(e.to_string()))?;
        let mut resp = MaskResponse::default();
        for t in &req.targets {
            if skipped.contains(t) {
                resp.skipped.insert(t.clone(), "model_unavailable".into());
                continue;
            }
            // A 32x24 mask: left half (x < 16) belongs to the target.
            let img = image::GrayImage::from_fn(32, 24, |x, _| {
                image::Luma([if x < 16 { 255 } else { 0 }])
            });
            let p = out.join(format!("{}_{t}.png", req.photo.photo_id));
            img.save_with_format(&p, image::ImageFormat::Png)
                .map_err(|e| WorkerError::Protocol(e.to_string()))?;
            resp.masks
                .insert(t.clone(), p.to_string_lossy().into_owned());
            resp.models.insert(t.clone(), "fake-seg".into());
        }
        Ok(resp)
    }
    async fn beauty_prepare(&self, req: &BeautyPrepareRequest) -> Result<BeautyPrepareResponse> {
        self.beauty_calls.fetch_add(1, Ordering::SeqCst);
        *self.last_beauty_request.lock().unwrap() = Some(req.clone());
        if self.beauty_unavailable.load(Ordering::SeqCst) {
            return Err(WorkerError::Unavailable("fake worker is down".into()));
        }
        let missing = self.missing.lock().unwrap().clone();
        if missing.iter().any(|m| m == BEAUTY_MODEL) && !req.allow_download {
            return Err(WorkerError::Rpc {
                code: ip_worker_client::error::CODE_MODEL_UNAVAILABLE,
                message: "model unavailable".into(),
                kind: Some("model_unavailable".into()),
                detail: Some(json!({ "models": [BEAUTY_MODEL] })),
            });
        }
        let out = std::path::PathBuf::from(&req.out_dir);
        std::fs::create_dir_all(&out).map_err(|e| WorkerError::Protocol(e.to_string()))?;
        // Faces the analysis knows about, or one the worker "finds" itself.
        let found: Vec<(Option<i64>, [f64; 4])> = if req.faces.is_empty() {
            vec![(None, [0.3, 0.2, 0.4, 0.5])]
        } else {
            req.faces.iter().map(|f| (Some(f.face_id), f.bbox)).collect()
        };
        let pid = req.photo.photo_id;
        let mut people = Vec::new();
        for (i, (face_id, b)) in found.into_iter().enumerate() {
            let mask = |name: &str, left_half: bool| -> Result<String> {
                let img = image::GrayImage::from_fn(32, 24, |x, _| {
                    image::Luma([if (x < 16) == left_half { 255 } else { 0 }])
                });
                let p = out.join(format!("{pid}_{i}_{name}.png"));
                img.save_with_format(&p, image::ImageFormat::Png)
                    .map_err(|e| WorkerError::Protocol(e.to_string()))?;
                Ok(p.to_string_lossy().into_owned())
            };
            let (x, y, w, h) = (b[0] as f32, b[1] as f32, b[2] as f32, b[3] as f32);
            // 478 landmarks on a ring inside the face box; 33 pose points down the frame
            let face_landmarks = (0..478)
                .map(|k| {
                    let a = k as f32 / 478.0 * std::f32::consts::TAU;
                    [x + w * (0.5 + 0.4 * a.cos()), y + h * (0.5 + 0.4 * a.sin())]
                })
                .collect();
            let pose = (0..33)
                .map(|k| [x + w * 0.5, y + h * (k as f32 / 8.0), 0.9])
                .collect();
            people.push(BeautyPerson {
                face_id,
                face_box: [x, y, w, h],
                face_landmarks,
                pose,
                skin_mask: Some(mask("skin", true)?),
                body_mask: Some(mask("body", false)?),
                blemishes: vec![[x + w * 0.5, y + h * 0.4, 0.01]],
            });
        }
        Ok(BeautyPrepareResponse {
            people,
            models: json!({"face_landmarks": "fake-mesh"}),
            skipped: Default::default(),
        })
    }
    async fn faces_embed(&self, req: &FacesEmbedRequest) -> Result<FacesEmbedResponse> {
        self.embed_calls.fetch_add(1, Ordering::SeqCst);
        *self.last_embed_request.lock().unwrap() = Some(req.clone());
        let faces = self.embed_faces.lock().unwrap().clone();
        let mut out = FacesEmbedResponse::default();
        for (bbox, mut emb) in faces {
            crate::analysis::vecs::normalize(&mut emb);
            out.faces.push(EmbeddedFace {
                bbox,
                det_score: 0.99,
            });
            out.embeddings.push(emb);
        }
        Ok(out)
    }
    async fn shutdown(&self) {
        self.set_state(WorkerState::Stopped);
    }
}

/// Atomically decrements `n` if it is non-zero; returns whether it did.
/// (Spelled out instead of `fetch_update`, which newer toolchains deprecate.)
fn take_one(n: &AtomicUsize) -> bool {
    let mut cur = n.load(Ordering::SeqCst);
    while cur > 0 {
        match n.compare_exchange_weak(cur, cur - 1, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return true,
            Err(actual) => cur = actual,
        }
    }
    false
}
