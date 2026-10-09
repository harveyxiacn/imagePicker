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
/// Ids of the fake M5 models (`required_for` `besttake` / `inpaint` / `enhance`).
pub const BESTTAKE_MODEL: &str = "besttake-align";
pub const INPAINT_MODEL: &str = "lama";
pub const SDXL_MODEL: &str = "fake-sdxl-inpaint";
pub const ENHANCE_MODEL: &str = "enhance-pack";

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
    /// Explicit `steps` of the last `analyze.batch` request.
    pub last_steps: Mutex<Option<Vec<String>>>,
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
    /// Makes `mask.generate` honour `person_bbox` for `person` masks: 255 inside the box, 0
    /// elsewhere (on a 100x100 raster). Off = the legacy "left half" mask.
    pub person_mask_from_bbox: std::sync::atomic::AtomicBool,
    /// Makes `mask.generate` answer every `person` mask with an all-black image (what the real
    /// matting returns for small background people).
    pub empty_person_masks: std::sync::atomic::AtomicBool,
    /// Milliseconds every non-analysis call sleeps before answering (timeout tests).
    pub call_delay_ms: AtomicUsize,
    /// Number of times the core killed this worker (after a call timeout).
    pub kills: AtomicUsize,
    /// `besttake.compose` calls received and the last request.
    pub compose_calls: AtomicUsize,
    pub last_compose_request: Mutex<Option<BestTakeComposeRequest>>,
    /// Source photo ids that cannot be composed, with the reason the worker reports.
    pub compose_refusals: Mutex<HashMap<i64, String>>,
    /// Warnings attached to every successful composition.
    pub compose_warnings: Mutex<Vec<String>>,
    pub inpaint_calls: AtomicUsize,
    pub last_inpaint_request: Mutex<Option<InpaintRequest>>,
    /// The mask PNG of the last `inpaint.run`, decoded at call time (the core deletes its
    /// scratch files afterwards).
    pub last_inpaint_mask: Mutex<Option<image::GrayImage>>,
    pub enhance_calls: AtomicUsize,
    pub enhance_requests: Mutex<Vec<EnhanceRequest>>,
    /// `beauty.prepare` reports `{"pose": "model_unavailable"}` as skipped (partial geometry).
    pub beauty_partial: std::sync::atomic::AtomicBool,
    /// `models.list` fails as if the worker could not start.
    pub models_unavailable: std::sync::atomic::AtomicBool,
    /// M6: what `llm.plan` answers (`None` = no language model installed).
    llm_response: Mutex<Option<LlmPlanResponse>>,
    pub llm_calls: AtomicUsize,
    pub last_llm_request: Mutex<Option<LlmPlanRequest>>,
    /// M6: `(caption, keywords, problems, adjust, reason)` of the VLM (`None` = not installed).
    vlm: Mutex<Option<VlmScript>>,
    pub vlm_calls: AtomicUsize,
    /// Model ids deleted through `models.delete`.
    pub deleted_models: Mutex<Vec<String>>,
    /// The last `configure` call (models dir / environment).
    pub last_options: Mutex<Option<WorkerOptions>>,
    /// Makes `models.delete` answer like a worker that predates it.
    pub delete_unsupported: std::sync::atomic::AtomicBool,
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
            last_steps: Mutex::new(None),
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
            person_mask_from_bbox: std::sync::atomic::AtomicBool::new(false),
            empty_person_masks: std::sync::atomic::AtomicBool::new(false),
            call_delay_ms: AtomicUsize::new(0),
            kills: AtomicUsize::new(0),
            compose_calls: AtomicUsize::new(0),
            last_compose_request: Mutex::new(None),
            compose_refusals: Mutex::new(HashMap::new()),
            compose_warnings: Mutex::new(Vec::new()),
            inpaint_calls: AtomicUsize::new(0),
            last_inpaint_request: Mutex::new(None),
            last_inpaint_mask: Mutex::new(None),
            enhance_calls: AtomicUsize::new(0),
            enhance_requests: Mutex::new(Vec::new()),
            beauty_partial: std::sync::atomic::AtomicBool::new(false),
            models_unavailable: std::sync::atomic::AtomicBool::new(false),
            llm_response: Mutex::new(None),
            llm_calls: AtomicUsize::new(0),
            last_llm_request: Mutex::new(None),
            vlm: Mutex::new(None),
            vlm_calls: AtomicUsize::new(0),
            deleted_models: Mutex::new(Vec::new()),
            last_options: Mutex::new(None),
            delete_unsupported: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

/// `(caption, keywords, problems, adjust, reason)` of the fake VLM.
type VlmScript = (String, Vec<String>, Vec<String>, serde_json::Value, String);

/// Ids of the fake M6 models (`task: ["llm_plan"]` / `["vlm_suggest", "vlm_describe"]`, as in the
/// worker's registry).
pub const LLM_MODEL: &str = "fake-qwen-llm";
pub const VLM_MODEL: &str = "fake-qwen-vl";

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
    /// Installs a language model that answers every `llm.plan` with `resp`.
    pub fn enable_llm(&self, resp: LlmPlanResponse) {
        *self.llm_response.lock().unwrap() = Some(resp);
    }
    /// Installs a vision-language model.
    pub fn enable_vlm(
        &self,
        caption: &str,
        keywords: &[&str],
        problems: &[&str],
        adjust: serde_json::Value,
        reason: &str,
    ) {
        *self.vlm.lock().unwrap() = Some((
            caption.to_string(),
            keywords.iter().map(|s| s.to_string()).collect(),
            problems.iter().map(|s| s.to_string()).collect(),
            adjust,
            reason.to_string(),
        ));
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
    /// Makes the given source photo unusable for best take (`besttake.compose` answers
    /// `{"patch": null, "reason": ...}`).
    pub fn refuse_compose(&self, source_photo_id: i64, reason: &str) {
        self.compose_refusals
            .lock()
            .unwrap()
            .insert(source_photo_id, reason.to_string());
    }
    async fn nap(&self) {
        let ms = self.call_delay_ms.load(Ordering::SeqCst);
        if ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(ms as u64)).await;
        }
    }
    /// A `-32010` error when any of `ids` is flagged missing and downloads are not allowed.
    fn gen_models(&self, ids: &[&str], allow_download: bool) -> Result<()> {
        let missing = self.missing.lock().unwrap().clone();
        let need: Vec<String> = ids
            .iter()
            .filter(|i| missing.iter().any(|m| m == *i))
            .map(|s| s.to_string())
            .collect();
        if need.is_empty() || allow_download {
            return Ok(());
        }
        Err(WorkerError::Rpc {
            code: ip_worker_client::error::CODE_MODEL_UNAVAILABLE,
            message: "model unavailable".into(),
            kind: Some("model_unavailable".into()),
            detail: Some(json!({ "models": need })),
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
        self.nap().await;
        if self.models_unavailable.load(Ordering::SeqCst) {
            return Err(WorkerError::Unavailable("fake worker is down".into()));
        }
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
        // like the real worker: an optional "pro" SDXL pack that must never block LaMa
        let mut extra_models = Vec::new();
        if self.llm_response.lock().unwrap().is_some() {
            let mut llm = m(LLM_MODEL, 1800.0, &[]);
            llm.task = vec!["llm_plan".into()];
            llm.recommended = false;
            llm.optional = true;
            extra_models.push(llm);
        }
        if self.vlm.lock().unwrap().is_some() {
            let mut vlm = m(VLM_MODEL, 3200.0, &[]);
            vlm.task = vec!["vlm_suggest".into(), "vlm_describe".into()];
            vlm.recommended = false;
            vlm.optional = true;
            extra_models.push(vlm);
        }
        let mut sdxl = m(SDXL_MODEL, 6700.0, &["inpaint", "pro"]);
        sdxl.recommended = false;
        sdxl.optional = true;
        let mut beauty = m(BEAUTY_MODEL, 12.0, &["beauty"]);
        beauty.recommended = false;
        beauty.optional = true;
        let mut models = vec![
            m("yunet", 0.3, &["faces"]),
            m("siglip2-base", 178.0, &["embed"]),
            beauty,
            m(BESTTAKE_MODEL, 20.0, &["besttake"]),
            m(INPAINT_MODEL, 200.0, &["inpaint"]),
            m(ENHANCE_MODEL, 300.0, &["enhance"]),
            sdxl,
        ];
        models.extend(extra_models);
        Ok(ModelsListing {
            models,
            profiles,
            beauty_models: json!([BEAUTY_MODEL]),
            extra: [(
                "inpaint_models".to_string(),
                json!({"lama": [INPAINT_MODEL], "sdxl": [SDXL_MODEL]}),
            )]
            .into_iter()
            .collect(),
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
        *self.last_steps.lock().unwrap() = req.steps.clone();
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
        self.nap().await;
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
            let img = match (t.as_str(), req.person_bbox) {
                ("person", _) if self.empty_person_masks.load(Ordering::SeqCst) => {
                    image::GrayImage::new(100, 100)
                }
                ("person", Some(b)) if self.person_mask_from_bbox.load(Ordering::SeqCst) => {
                    image::GrayImage::from_fn(100, 100, |x, y| {
                        let (fx, fy) = (x as f64 / 100.0, y as f64 / 100.0);
                        let inside =
                            fx >= b[0] && fx < b[0] + b[2] && fy >= b[1] && fy < b[1] + b[3];
                        image::Luma([if inside { 255 } else { 0 }])
                    })
                }
                _ => image::GrayImage::from_fn(32, 24, |x, _| {
                    image::Luma([if x < 16 { 255 } else { 0 }])
                }),
            };
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
            req.faces
                .iter()
                .map(|f| (Some(f.face_id), f.bbox))
                .collect()
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
        let mut skipped = std::collections::BTreeMap::new();
        if self.beauty_partial.load(Ordering::SeqCst) {
            skipped.insert("pose".to_string(), "model_unavailable".to_string());
        }
        Ok(BeautyPrepareResponse {
            people,
            models: json!({"face_landmarks": "fake-mesh"}),
            skipped,
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
    async fn besttake_compose(
        &self,
        req: &BestTakeComposeRequest,
    ) -> Result<BestTakeComposeResponse> {
        self.compose_calls.fetch_add(1, Ordering::SeqCst);
        *self.last_compose_request.lock().unwrap() = Some(req.clone());
        self.nap().await;
        self.gen_models(&[BESTTAKE_MODEL], req.allow_download)?;
        if let Some(reason) = self
            .compose_refusals
            .lock()
            .unwrap()
            .get(&req.source.photo_id)
        {
            return Ok(BestTakeComposeResponse {
                patch: None,
                rect: None,
                quality: None,
                reason: Some(reason.clone()),
            });
        }
        let out = std::path::PathBuf::from(&req.out_dir);
        std::fs::create_dir_all(&out).map_err(|e| WorkerError::Protocol(e.to_string()))?;
        let shade = (req.source.photo_id % 200) as u8 + 20;
        let img = image::RgbaImage::from_fn(24, 16, |x, y| {
            let edge = x == 0 || y == 0 || x == 23 || y == 15;
            image::Rgba([shade, 255 - shade, 128, if edge { 80 } else { 255 }])
        });
        let p = out.join(format!(
            "bt_{}_{}.png",
            req.base.photo_id, req.source.photo_id
        ));
        img.save_with_format(&p, image::ImageFormat::Png)
            .map_err(|e| WorkerError::Protocol(e.to_string()))?;
        let b = req.base_face;
        let (gx, gy) = (b[2] * 0.25, b[3] * 0.25);
        let x = (b[0] - gx).max(0.0);
        let y = (b[1] - gy).max(0.0);
        let w = (b[0] + b[2] + gx).min(1.0) - x;
        let h = (b[1] + b[3] + gy).min(1.0) - y;
        Ok(BestTakeComposeResponse {
            patch: Some(p.to_string_lossy().into_owned()),
            rect: Some([x as f32, y as f32, w as f32, h as f32]),
            quality: Some(ComposeQuality {
                score: 0.9,
                aligned: true,
                warnings: self.compose_warnings.lock().unwrap().clone(),
            }),
            reason: None,
        })
    }
    async fn inpaint_run(&self, req: &InpaintRequest) -> Result<InpaintResponse> {
        self.inpaint_calls.fetch_add(1, Ordering::SeqCst);
        *self.last_inpaint_request.lock().unwrap() = Some(req.clone());
        self.nap().await;
        self.gen_models(&[INPAINT_MODEL], req.allow_download)?;
        let mask = image::open(&req.mask)
            .map_err(|e| WorkerError::Protocol(format!("bad mask: {e}")))?
            .to_luma8();
        // bounding box of the removed region, plus a margin
        let (mw, mh) = (mask.width() as f32, mask.height() as f32);
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
        for (x, y, p) in mask.enumerate_pixels() {
            if p.0[0] > 127 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
        *self.last_inpaint_mask.lock().unwrap() = Some(mask);
        if x0 == u32::MAX {
            return Err(WorkerError::Protocol("the mask removes nothing".into()));
        }
        let rx = (x0 as f32 / mw - 0.02).max(0.0);
        let ry = (y0 as f32 / mh - 0.02).max(0.0);
        let rect = [
            rx,
            ry,
            ((x1 + 1) as f32 / mw + 0.02).min(1.0) - rx,
            ((y1 + 1) as f32 / mh + 0.02).min(1.0) - ry,
        ];
        let out = std::path::PathBuf::from(&req.out_dir);
        std::fs::create_dir_all(&out).map_err(|e| WorkerError::Protocol(e.to_string()))?;
        let img = image::RgbaImage::from_pixel(16, 16, image::Rgba([90, 120, 150, 255]));
        let p = out.join(format!("inpaint_{}.png", req.photo.photo_id));
        img.save_with_format(&p, image::ImageFormat::Png)
            .map_err(|e| WorkerError::Protocol(e.to_string()))?;
        Ok(InpaintResponse {
            patch: Some(p.to_string_lossy().into_owned()),
            rect: Some(rect),
            model: Some(req.model.clone()),
        })
    }
    async fn enhance_run(&self, req: &EnhanceRequest) -> Result<EnhanceResponse> {
        self.enhance_calls.fetch_add(1, Ordering::SeqCst);
        self.enhance_requests.lock().unwrap().push(req.clone());
        self.nap().await;
        self.gen_models(&[ENHANCE_MODEL], req.allow_download)?;
        let out = std::path::PathBuf::from(&req.out_dir);
        std::fs::create_dir_all(&out).map_err(|e| WorkerError::Protocol(e.to_string()))?;
        let png = |name: String, px: [u8; 4]| -> Result<String> {
            let p = out.join(name);
            image::RgbaImage::from_pixel(8, 8, image::Rgba(px))
                .save_with_format(&p, image::ImageFormat::Png)
                .map_err(|e| WorkerError::Protocol(e.to_string()))?;
            Ok(p.to_string_lossy().into_owned())
        };
        let pid = req.photo.photo_id;
        match req.op.as_str() {
            "denoise" => Ok(EnhanceResponse {
                patch: Some(png(format!("denoise_{pid}.png"), [200, 200, 200, 255])?),
                rect: Some([0.0, 0.0, 1.0, 1.0]),
                ..Default::default()
            }),
            "face_restore" => {
                let mut patches = Vec::new();
                for (i, f) in req.faces.clone().unwrap_or_default().iter().enumerate() {
                    patches.push(EnhancePatch {
                        patch: png(format!("face_{pid}_{i}.png"), [220, 180, 170, 255])?,
                        rect: [f[0] as f32, f[1] as f32, f[2] as f32, f[3] as f32],
                    });
                }
                Ok(EnhanceResponse {
                    patches,
                    ..Default::default()
                })
            }
            "upscale" => {
                let scale = req.scale.unwrap_or(2).max(1);
                let src = image::open(&req.photo.path)
                    .map_err(|e| WorkerError::Protocol(format!("bad image: {e}")))?
                    .to_rgb8();
                let big = image::imageops::resize(
                    &src,
                    src.width() * scale,
                    src.height() * scale,
                    image::imageops::FilterType::Nearest,
                );
                let p = out.join(format!("upscaled_{pid}.png"));
                big.save_with_format(&p, image::ImageFormat::Png)
                    .map_err(|e| WorkerError::Protocol(e.to_string()))?;
                Ok(EnhanceResponse {
                    image: Some(p.to_string_lossy().into_owned()),
                    ..Default::default()
                })
            }
            other => Err(WorkerError::Rpc {
                code: -32602,
                message: format!("unknown enhance op {other:?}"),
                kind: Some("bad_params".into()),
                detail: None,
            }),
        }
    }
    async fn kill(&self) {
        self.kills.fetch_add(1, Ordering::SeqCst);
        self.set_state(WorkerState::Stopped);
    }
    async fn llm_plan(&self, req: &LlmPlanRequest) -> Result<LlmPlanResponse> {
        self.llm_calls.fetch_add(1, Ordering::SeqCst);
        *self.last_llm_request.lock().unwrap() = Some(req.clone());
        self.nap().await;
        self.set_state(WorkerState::Ready);
        match self.llm_response.lock().unwrap().clone() {
            Some(r) => Ok(r),
            None => Err(WorkerError::Unavailable(
                "this AI worker has no language model".into(),
            )),
        }
    }
    async fn vlm_describe(&self, _req: &VlmDescribeRequest) -> Result<VlmDescribeResponse> {
        self.vlm_calls.fetch_add(1, Ordering::SeqCst);
        self.nap().await;
        match self.vlm.lock().unwrap().clone() {
            Some((caption, keywords, ..)) => Ok(VlmDescribeResponse { caption, keywords }),
            None => Err(WorkerError::Unavailable("no VLM".into())),
        }
    }
    async fn vlm_suggest(&self, _req: &VlmSuggestRequest) -> Result<VlmSuggestResponse> {
        self.vlm_calls.fetch_add(1, Ordering::SeqCst);
        self.nap().await;
        match self.vlm.lock().unwrap().clone() {
            Some((_, _, problems, adjust, reason)) => Ok(VlmSuggestResponse {
                problems,
                adjust,
                reason,
            }),
            None => Err(WorkerError::Unavailable("no VLM".into())),
        }
    }
    async fn models_delete(&self, id: &str) -> Result<()> {
        if self.delete_unsupported.load(Ordering::SeqCst) {
            return Err(WorkerError::Unavailable(
                "this AI worker does not support models.delete".into(),
            ));
        }
        let known = self.models_list().await?.models.iter().any(|m| m.id == id);
        if !known {
            return Err(WorkerError::Rpc {
                code: -32602,
                message: format!("unknown model {id}"),
                kind: None,
                detail: None,
            });
        }
        self.deleted_models.lock().unwrap().push(id.to_string());
        let mut missing = self.missing.lock().unwrap();
        if !missing.iter().any(|m| m == id) {
            missing.push(id.to_string());
        }
        Ok(())
    }
    fn configure(&self, opts: &WorkerOptions) {
        *self.last_options.lock().unwrap() = Some(opts.clone());
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
