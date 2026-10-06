//! Client side of the Python AI worker: process lifecycle (lazy start, restart with backoff,
//! process-tree kill) and a typed JSON-RPC 2.0 / WebSocket client.
//!
//! [`AiWorker`] is the seam the core programs against; [`ManagedWorker`] is the real
//! implementation and tests use fakes.

pub mod client;
pub mod error;
pub mod managed;
pub mod process;
pub mod protocol;
mod protocol_m4;
mod protocol_m5;
mod protocol_m6;
pub mod timeout;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

pub use client::{CancelToken, ProgressTx, RpcClient};
pub use error::{Result, WorkerError};
pub use managed::{ManagedWorker, WorkerConfig};
pub use protocol::*;
pub use protocol_m4::*;
pub use protocol_m5::*;
pub use protocol_m6::*;
pub use timeout::{TimeoutHandle, TimeoutWorker, WorkerTimeouts};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkerState {
    Stopped,
    Starting,
    Ready,
    Busy,
    Crashed,
    Unavailable,
}

impl WorkerState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Busy => "busy",
            Self::Crashed => "crashed",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Snapshot of the worker for `GET /api/system/hardware` and the `worker.status` event.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorkerStatus {
    pub state: WorkerState,
    pub tier: Option<String>,
    pub error: Option<String>,
    /// Last `system.info` (kept while the worker is stopped, so the UI can still show it).
    pub info: Option<SystemInfo>,
}

impl WorkerStatus {
    pub fn stopped() -> Self {
        Self {
            state: WorkerState::Stopped,
            tier: None,
            error: None,
            info: None,
        }
    }
}

#[async_trait::async_trait]
pub trait AiWorker: Send + Sync {
    fn status(&self) -> WorkerStatus;
    fn subscribe(&self) -> watch::Receiver<WorkerStatus>;
    /// Starts the worker if needed and returns `system.info`.
    async fn system_info(&self) -> Result<SystemInfo>;
    async fn models_list(&self) -> Result<ModelsListing>;
    /// Downloads the given models. `progress` gets the raw `progress` params
    /// (`{kind:"model.download", model, done, total, ...}`).
    async fn models_ensure(
        &self,
        ids: &[String],
        progress: Option<ProgressTx>,
        cancel: &CancelToken,
    ) -> Result<()>;
    async fn analyze_batch(
        &self,
        req: &AnalyzeRequest,
        progress: Option<ProgressTx>,
        cancel: &CancelToken,
    ) -> Result<AnalyzeResponse>;
    /// Generates AI masks (`mask.generate`). Workers that predate M3 report `Unavailable`.
    async fn mask_generate(&self, _req: &MaskRequest) -> Result<MaskResponse> {
        Err(WorkerError::Unavailable(
            "this AI worker does not support mask.generate".into(),
        ))
    }
    /// Per-person geometry for portrait retouching (`beauty.prepare`). Workers that predate M4
    /// report `Unavailable`.
    async fn beauty_prepare(&self, _req: &BeautyPrepareRequest) -> Result<BeautyPrepareResponse> {
        Err(WorkerError::Unavailable(
            "this AI worker does not support beauty.prepare".into(),
        ))
    }
    /// Detects and embeds the faces of one image (`faces.embed`), for "search by face".
    async fn faces_embed(&self, _req: &FacesEmbedRequest) -> Result<FacesEmbedResponse> {
        Err(WorkerError::Unavailable(
            "this AI worker does not support faces.embed".into(),
        ))
    }
    /// Composites one person's face from another frame (`besttake.compose`). Workers that predate
    /// M5 report `Unavailable`.
    async fn besttake_compose(
        &self,
        _req: &BestTakeComposeRequest,
    ) -> Result<BestTakeComposeResponse> {
        Err(WorkerError::Unavailable(
            "this AI worker does not support besttake.compose".into(),
        ))
    }
    /// Generative object removal (`inpaint.run`).
    async fn inpaint_run(&self, _req: &InpaintRequest) -> Result<InpaintResponse> {
        Err(WorkerError::Unavailable(
            "this AI worker does not support inpaint.run".into(),
        ))
    }
    /// Denoise / face restoration / super-resolution (`enhance.run`).
    async fn enhance_run(&self, _req: &EnhanceRequest) -> Result<EnhanceResponse> {
        Err(WorkerError::Unavailable(
            "this AI worker does not support enhance.run".into(),
        ))
    }
    /// Language-model planning (`llm.plan`). Workers that predate M6 report `Unavailable`.
    async fn llm_plan(&self, _req: &LlmPlanRequest) -> Result<LlmPlanResponse> {
        Err(WorkerError::Unavailable(
            "this AI worker does not support llm.plan".into(),
        ))
    }
    /// VLM edit suggestion (`vlm.suggest`).
    async fn vlm_suggest(&self, _req: &VlmSuggestRequest) -> Result<VlmSuggestResponse> {
        Err(WorkerError::Unavailable(
            "this AI worker does not support vlm.suggest".into(),
        ))
    }
    /// VLM caption + keywords (`vlm.describe`).
    async fn vlm_describe(&self, _req: &VlmDescribeRequest) -> Result<VlmDescribeResponse> {
        Err(WorkerError::Unavailable(
            "this AI worker does not support vlm.describe".into(),
        ))
    }
    /// Deletes a downloaded model (`models.delete`). Workers without the method answer
    /// `Unavailable`; the core then removes the model directory itself.
    async fn models_delete(&self, _id: &str) -> Result<()> {
        Err(WorkerError::Unavailable(
            "this AI worker does not support models.delete".into(),
        ))
    }
    /// Sets process-level options (models dir, environment); they apply from the next start.
    fn configure(&self, _opts: &WorkerOptions) {}
    /// Stops the worker process (gracefully, then by killing the process tree).
    async fn shutdown(&self);
    /// Kills an unresponsive worker immediately (no polite shutdown request). The next request
    /// starts a fresh process. Default: [`AiWorker::shutdown`].
    async fn kill(&self) {
        self.shutdown().await;
    }
}
