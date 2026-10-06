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

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

pub use client::{CancelToken, ProgressTx, RpcClient};
pub use error::{Result, WorkerError};
pub use managed::{ManagedWorker, WorkerConfig};
pub use protocol::*;

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
    /// Stops the worker process (gracefully, then by killing the process tree).
    async fn shutdown(&self);
}
