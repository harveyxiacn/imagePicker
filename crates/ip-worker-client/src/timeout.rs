//! Per-call timeouts for any [`AiWorker`]: a request that does not answer in time fails with
//! [`WorkerError::CallTimeout`] and the (presumably hung) worker is killed, so the next request
//! starts a fresh process instead of queueing behind a stuck one.
//!
//! Long-running batch methods (`analyze.batch`, `models.ensure`) are not limited here: they
//! stream progress and are cancelled through their own [`CancelToken`].

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;

use crate::error::{Result, WorkerError};
use crate::*;

/// Timeout table: per method, with a default for methods not listed. `None` = unlimited.
#[derive(Debug, Clone)]
pub struct WorkerTimeouts {
    per_method: HashMap<String, Option<Duration>>,
    default: Option<Duration>,
}

impl Default for WorkerTimeouts {
    fn default() -> Self {
        let secs = |s: u64| Some(Duration::from_secs(s));
        let mut per_method = HashMap::new();
        for (m, t) in [
            ("system.info", secs(60)),
            ("models.list", secs(60)),
            ("mask.generate", secs(120)),
            ("beauty.prepare", secs(120)),
            ("faces.embed", secs(60)),
            ("besttake.compose", secs(120)),
            ("inpaint.run", secs(300)),
            ("enhance.run", secs(300)),
            // batch / download calls: governed by their cancel tokens, not by a wall clock
            ("analyze.batch", None),
            ("models.ensure", None),
        ] {
            per_method.insert(m.to_string(), t);
        }
        Self {
            per_method,
            default: secs(120),
        }
    }
}

impl WorkerTimeouts {
    /// Defaults, overridden by `IMAGEPICKER_WORKER_TIMEOUT` (all methods without an entry) and
    /// `IMAGEPICKER_WORKER_TIMEOUT_<METHOD>` (method name upper-cased, `.` -> `_`, e.g.
    /// `IMAGEPICKER_WORKER_TIMEOUT_MASK_GENERATE`). Values are seconds; `0` disables the limit.
    pub fn from_env() -> Self {
        let mut t = Self::default();
        let parse = |v: String| v.trim().parse::<u64>().ok();
        let secs = |n: u64| (n > 0).then(|| Duration::from_secs(n));
        if let Some(n) = std::env::var("IMAGEPICKER_WORKER_TIMEOUT")
            .ok()
            .and_then(parse)
        {
            t.default = secs(n);
        }
        let methods: Vec<String> = t.per_method.keys().cloned().collect();
        for m in methods {
            let key = format!(
                "IMAGEPICKER_WORKER_TIMEOUT_{}",
                m.to_uppercase().replace('.', "_")
            );
            if let Some(n) = std::env::var(key).ok().and_then(parse) {
                t.per_method.insert(m, secs(n));
            }
        }
        t
    }

    pub fn get(&self, method: &str) -> Option<Duration> {
        self.per_method.get(method).copied().unwrap_or(self.default)
    }

    pub fn set(&mut self, method: &str, limit: Option<Duration>) {
        self.per_method.insert(method.to_string(), limit);
    }

    pub fn set_default(&mut self, limit: Option<Duration>) {
        self.default = limit;
    }
}

/// Shared, adjustable handle on the timeouts of a [`TimeoutWorker`] (also usable from tests).
#[derive(Clone)]
pub struct TimeoutHandle(Arc<Mutex<WorkerTimeouts>>);

impl TimeoutHandle {
    pub fn new(t: WorkerTimeouts) -> Self {
        Self(Arc::new(Mutex::new(t)))
    }
    pub fn get(&self, method: &str) -> Option<Duration> {
        self.0.lock().unwrap().get(method)
    }
    pub fn set(&self, method: &str, limit: Option<Duration>) {
        self.0.lock().unwrap().set(method, limit);
    }
    pub fn set_default(&self, limit: Option<Duration>) {
        self.0.lock().unwrap().set_default(limit);
    }
    pub fn snapshot(&self) -> WorkerTimeouts {
        self.0.lock().unwrap().clone()
    }
}

/// Wraps a worker with per-call timeouts.
pub struct TimeoutWorker {
    inner: Arc<dyn AiWorker>,
    timeouts: TimeoutHandle,
}

impl TimeoutWorker {
    pub fn new(inner: Arc<dyn AiWorker>, timeouts: TimeoutHandle) -> Self {
        Self { inner, timeouts }
    }

    async fn limited<T>(&self, method: &str, fut: impl Future<Output = Result<T>>) -> Result<T> {
        let Some(limit) = self.timeouts.get(method) else {
            return fut.await;
        };
        match tokio::time::timeout(limit, fut).await {
            Ok(r) => r,
            Err(_) => {
                tracing::warn!(
                    method,
                    secs = limit.as_secs(),
                    "AI worker call timed out; killing the worker"
                );
                self.inner.kill().await;
                Err(WorkerError::CallTimeout {
                    method: method.to_string(),
                    secs: limit.as_secs().max(1),
                })
            }
        }
    }
}

#[async_trait::async_trait]
impl AiWorker for TimeoutWorker {
    fn status(&self) -> WorkerStatus {
        self.inner.status()
    }
    fn subscribe(&self) -> watch::Receiver<WorkerStatus> {
        self.inner.subscribe()
    }
    async fn system_info(&self) -> Result<SystemInfo> {
        self.limited("system.info", self.inner.system_info()).await
    }
    async fn models_list(&self) -> Result<ModelsListing> {
        self.limited("models.list", self.inner.models_list()).await
    }
    async fn models_ensure(
        &self,
        ids: &[String],
        progress: Option<ProgressTx>,
        cancel: &CancelToken,
    ) -> Result<()> {
        self.limited(
            "models.ensure",
            self.inner.models_ensure(ids, progress, cancel),
        )
        .await
    }
    async fn analyze_batch(
        &self,
        req: &AnalyzeRequest,
        progress: Option<ProgressTx>,
        cancel: &CancelToken,
    ) -> Result<AnalyzeResponse> {
        self.limited(
            "analyze.batch",
            self.inner.analyze_batch(req, progress, cancel),
        )
        .await
    }
    async fn mask_generate(&self, req: &MaskRequest) -> Result<MaskResponse> {
        self.limited("mask.generate", self.inner.mask_generate(req))
            .await
    }
    async fn beauty_prepare(&self, req: &BeautyPrepareRequest) -> Result<BeautyPrepareResponse> {
        self.limited("beauty.prepare", self.inner.beauty_prepare(req))
            .await
    }
    async fn faces_embed(&self, req: &FacesEmbedRequest) -> Result<FacesEmbedResponse> {
        self.limited("faces.embed", self.inner.faces_embed(req))
            .await
    }
    async fn besttake_compose(
        &self,
        req: &BestTakeComposeRequest,
    ) -> Result<BestTakeComposeResponse> {
        self.limited("besttake.compose", self.inner.besttake_compose(req))
            .await
    }
    async fn inpaint_run(&self, req: &InpaintRequest) -> Result<InpaintResponse> {
        self.limited("inpaint.run", self.inner.inpaint_run(req))
            .await
    }
    async fn enhance_run(&self, req: &EnhanceRequest) -> Result<EnhanceResponse> {
        self.limited("enhance.run", self.inner.enhance_run(req))
            .await
    }
    async fn shutdown(&self) {
        self.inner.shutdown().await;
    }
    async fn kill(&self) {
        self.inner.kill().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct Slow {
        kills: AtomicUsize,
        sleep_ms: u64,
    }

    #[async_trait::async_trait]
    impl AiWorker for Slow {
        fn status(&self) -> WorkerStatus {
            WorkerStatus::stopped()
        }
        fn subscribe(&self) -> watch::Receiver<WorkerStatus> {
            watch::channel(WorkerStatus::stopped()).1
        }
        async fn system_info(&self) -> Result<SystemInfo> {
            tokio::time::sleep(Duration::from_millis(self.sleep_ms)).await;
            Ok(SystemInfo::default())
        }
        async fn models_list(&self) -> Result<ModelsListing> {
            Ok(ModelsListing::default())
        }
        async fn models_ensure(
            &self,
            _: &[String],
            _: Option<ProgressTx>,
            _: &CancelToken,
        ) -> Result<()> {
            Ok(())
        }
        async fn analyze_batch(
            &self,
            _: &AnalyzeRequest,
            _: Option<ProgressTx>,
            _: &CancelToken,
        ) -> Result<AnalyzeResponse> {
            Ok(AnalyzeResponse::default())
        }
        async fn shutdown(&self) {}
        async fn kill(&self) {
            self.kills.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn slow_calls_time_out_and_kill_the_worker() {
        let slow = Arc::new(Slow {
            sleep_ms: 500,
            ..Default::default()
        });
        let h = TimeoutHandle::new(WorkerTimeouts::default());
        h.set("system.info", Some(Duration::from_millis(40)));
        let w = TimeoutWorker::new(slow.clone(), h.clone());
        let r = w.system_info().await;
        assert!(matches!(r, Err(WorkerError::CallTimeout { .. })), "{r:?}");
        assert_eq!(slow.kills.load(Ordering::SeqCst), 1);
        // limits can be lifted per method
        h.set("system.info", None);
        let fast = Arc::new(Slow {
            sleep_ms: 1,
            ..Default::default()
        });
        let w = TimeoutWorker::new(fast.clone(), h);
        assert!(w.system_info().await.is_ok());
        assert_eq!(fast.kills.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn defaults_per_method() {
        let t = WorkerTimeouts::default();
        assert_eq!(t.get("mask.generate"), Some(Duration::from_secs(120)));
        assert_eq!(t.get("inpaint.run"), Some(Duration::from_secs(300)));
        assert_eq!(t.get("analyze.batch"), None);
    }
}
