//! An [`AiWorker`] that is permanently unavailable: used where no Python worker can run
//! (Android, or `IMAGEPICKER_NO_WORKER=1`). Every call fails with [`WorkerError::Unavailable`],
//! so the core reports "unavailable" cleanly and the on-device `lite` analysis profile remains.

use tokio::sync::watch;

use crate::{
    AiWorker, AnalyzeRequest, AnalyzeResponse, CancelToken, ModelsListing, ProgressTx, Result,
    SystemInfo, WorkerError, WorkerState, WorkerStatus,
};

/// Environment switch that forces [`UnavailableWorker`] on any platform.
pub const ENV_NO_WORKER: &str = "IMAGEPICKER_NO_WORKER";

/// `true` when the Python worker cannot be used on this platform / configuration.
pub fn worker_unsupported() -> bool {
    cfg!(target_os = "android")
        || std::env::var(ENV_NO_WORKER).is_ok_and(|v| !v.is_empty() && v != "0")
}

pub struct UnavailableWorker {
    reason: String,
    tx: watch::Sender<WorkerStatus>,
}

impl UnavailableWorker {
    pub fn new(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        let (tx, _) = watch::channel(WorkerStatus {
            state: WorkerState::Unavailable,
            tier: None,
            error: Some(reason.clone()),
            info: None,
        });
        Self { reason, tx }
    }

    fn err<T>(&self) -> Result<T> {
        Err(WorkerError::Unavailable(self.reason.clone()))
    }
}

#[async_trait::async_trait]
impl AiWorker for UnavailableWorker {
    fn status(&self) -> WorkerStatus {
        self.tx.borrow().clone()
    }
    fn subscribe(&self) -> watch::Receiver<WorkerStatus> {
        self.tx.subscribe()
    }
    async fn system_info(&self) -> Result<SystemInfo> {
        self.err()
    }
    async fn models_list(&self) -> Result<ModelsListing> {
        self.err()
    }
    async fn models_ensure(
        &self,
        _ids: &[String],
        _progress: Option<ProgressTx>,
        _cancel: &CancelToken,
    ) -> Result<()> {
        self.err()
    }
    async fn analyze_batch(
        &self,
        _req: &AnalyzeRequest,
        _progress: Option<ProgressTx>,
        _cancel: &CancelToken,
    ) -> Result<AnalyzeResponse> {
        self.err()
    }
    async fn shutdown(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reports_unavailable() {
        let w = UnavailableWorker::new("no worker here");
        assert_eq!(w.status().state, WorkerState::Unavailable);
        assert!(matches!(
            w.system_info().await,
            Err(WorkerError::Unavailable(m)) if m == "no worker here"
        ));
        assert!(w.models_list().await.is_err());
    }
}
