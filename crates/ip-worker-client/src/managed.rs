//! The real worker: a Python child process started lazily, restarted when it crashes
//! (exponential backoff, at most 3 consecutive failures) and killed with its whole process tree.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{watch, Mutex as AsyncMutex};

use crate::client::{CancelToken, ProgressTx, RpcClient};
use crate::error::{Result, WorkerError};
use crate::launch::{
    find_on_path, resolve_launch, Launch, LaunchInputs, RuntimeLaunch, RUNTIME_MISSING_TAG,
};
use crate::process;
use crate::protocol::*;
use crate::protocol_m4::*;
use crate::protocol_m5::*;
use crate::protocol_m6::*;
use crate::{AiWorker, WorkerState, WorkerStatus};

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// Full command (see [`process::build_argv`]); default `uv run imagepicker-ai serve ...`.
    pub cmd: Option<String>,
    /// Directory of the `ai-worker` project (working directory of the process).
    pub dir: Option<PathBuf>,
    /// Passed as `--models-dir` when set.
    pub models_dir: Option<PathBuf>,
    pub start_timeout: Duration,
    /// Consecutive failed starts / crashes before the worker is declared unavailable.
    pub max_failures: u32,
    pub backoff_base: Duration,
    /// After being declared unavailable, new requests retry only after this long.
    pub cooldown: Duration,
    /// The installed AI runtime (`<data>/runtime`), used when there is no override or dev repo.
    pub runtime: Option<RuntimeLaunch>,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            cmd: None,
            dir: None,
            models_dir: None,
            start_timeout: Duration::from_secs(180),
            max_failures: 3,
            backoff_base: Duration::from_millis(500),
            cooldown: Duration::from_secs(30),
            runtime: None,
        }
    }
}

impl WorkerConfig {
    /// Honours `IMAGEPICKER_WORKER_CMD`, `IMAGEPICKER_WORKER_DIR` and `IMAGEPICKER_MODELS_DIR`.
    pub fn from_env() -> Self {
        Self {
            cmd: std::env::var(process::ENV_WORKER_CMD)
                .ok()
                .filter(|s| !s.trim().is_empty()),
            models_dir: std::env::var_os("IMAGEPICKER_MODELS_DIR")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            ..Self::default()
        }
    }
}

struct Shared {
    tx: watch::Sender<WorkerStatus>,
    generation: AtomicU64,
    inflight: AtomicUsize,
    failures: Mutex<Failures>,
}

#[derive(Default)]
struct Failures {
    count: u32,
    last_at: Option<Instant>,
    last_error: Option<String>,
}

impl Shared {
    fn set_state(&self, state: WorkerState, error: Option<String>) {
        self.tx.send_if_modified(|s| {
            if s.state == state && s.error == error {
                return false;
            }
            s.state = state;
            s.error = error;
            true
        });
    }

    fn set_info(&self, info: SystemInfo) {
        self.tx.send_modify(|s| {
            s.tier = info.tier.clone();
            s.info = Some(info);
        });
    }

    fn reset_failures(&self) {
        self.failures.lock().unwrap().count = 0;
    }
}

/// Kills the process tree when dropped.
struct ChildGuard {
    pid: u32,
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        process::kill_tree(self.pid);
    }
}

struct Conn {
    client: RpcClient,
    _guard: ChildGuard,
}

struct Busy(Arc<Shared>);

impl Busy {
    fn new(shared: &Arc<Shared>) -> Self {
        if shared.inflight.fetch_add(1, Ordering::SeqCst) == 0 {
            shared.tx.send_if_modified(|s| {
                if s.state == WorkerState::Ready {
                    s.state = WorkerState::Busy;
                    true
                } else {
                    false
                }
            });
        }
        Busy(shared.clone())
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        if self.0.inflight.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.0.tx.send_if_modified(|s| {
                if s.state == WorkerState::Busy {
                    s.state = WorkerState::Ready;
                    true
                } else {
                    false
                }
            });
        }
    }
}

pub struct ManagedWorker {
    cfg: WorkerConfig,
    options: Mutex<WorkerOptions>,
    shared: Arc<Shared>,
    conn: AsyncMutex<Option<Conn>>,
}

impl ManagedWorker {
    pub fn new(cfg: WorkerConfig) -> Self {
        let (tx, _) = watch::channel(WorkerStatus::stopped());
        Self {
            cfg,
            options: Mutex::new(WorkerOptions::default()),
            shared: Arc::new(Shared {
                tx,
                generation: AtomicU64::new(0),
                inflight: AtomicUsize::new(0),
                failures: Mutex::new(Failures::default()),
            }),
            conn: AsyncMutex::new(None),
        }
    }

    fn random_token() -> String {
        let mut b = [0u8; 16];
        if getrandom::fill(&mut b).is_err() {
            // extremely unlikely; fall back to time + pid (loopback only)
            let t = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            b.copy_from_slice(&(t ^ ((std::process::id() as u128) << 64)).to_le_bytes());
        }
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// Returns a live client, (re)starting the process when needed.
    async fn client(&self) -> Result<RpcClient> {
        let mut guard = self.conn.lock().await;
        if let Some(c) = guard.as_ref() {
            if !c.client.is_closed() {
                return Ok(c.client.clone());
            }
        }
        // dead or never started: drop the old process tree
        guard.take();

        loop {
            let (count, last_at, last_err) = {
                let f = self.shared.failures.lock().unwrap();
                (f.count, f.last_at, f.last_error.clone())
            };
            if count >= self.cfg.max_failures {
                match last_at {
                    Some(t) if t.elapsed() < self.cfg.cooldown => {
                        return Err(WorkerError::Unavailable(
                            last_err
                                .unwrap_or_else(|| "the AI worker failed repeatedly".to_string()),
                        ));
                    }
                    _ => self.shared.reset_failures(),
                }
                continue;
            }
            if count > 0 {
                let delay = self.cfg.backoff_base * 2u32.pow((count - 1).min(6));
                tokio::time::sleep(delay).await;
            }
            self.shared.set_state(WorkerState::Starting, None);
            match self.start().await {
                Ok(conn) => {
                    let client = conn.client.clone();
                    *guard = Some(conn);
                    self.shared.set_state(WorkerState::Ready, None);
                    // best effort: tier / hardware for the UI
                    if let Ok(v) = client.call("system.info", json!({}), None, None).await {
                        if let Ok(info) = serde_json::from_value::<SystemInfo>(v) {
                            self.shared.set_info(info);
                        }
                    }
                    return Ok(client);
                }
                Err(e) => {
                    let msg = e.to_string();
                    tracing::warn!(error = %msg, "AI worker failed to start");
                    {
                        let mut f = self.shared.failures.lock().unwrap();
                        f.last_at = Some(Instant::now());
                        f.last_error = Some(msg.clone());
                        // a missing launcher will not fix itself between retries
                        f.count = if matches!(&e, WorkerError::Unavailable(m) if m.contains("not found"))
                        {
                            self.cfg.max_failures
                        } else {
                            f.count + 1
                        };
                        if f.count >= self.cfg.max_failures {
                            self.shared.set_state(
                                WorkerState::Unavailable,
                                Some(msg.replace(RUNTIME_MISSING_TAG, "")),
                            );
                            return Err(WorkerError::Unavailable(msg));
                        }
                    }
                }
            }
        }
    }

    async fn start(&self) -> Result<Conn> {
        let token = Self::random_token();
        let dir = self.cfg.dir.clone().or_else(process::find_worker_dir);
        let cmd = self.cfg.cmd.clone();
        let opts = self.options.lock().unwrap().clone();
        let models_dir = opts
            .models_dir
            .clone()
            .or_else(|| self.cfg.models_dir.clone());
        let launch = resolve_launch(
            &LaunchInputs {
                cmd_override: cmd.as_deref(),
                dev_dir: dir,
                uv_on_path: find_on_path("uv").is_some(),
                runtime: self.cfg.runtime.as_ref(),
            },
            &token,
            std::process::id(),
            models_dir.as_deref(),
        )
        .map_err(WorkerError::Unavailable)?;
        let Launch { argv, dir, source } = launch;
        tracing::info!(argv = %argv[0], dir = ?dir, ?source, "starting AI worker");
        let mut child = process::spawn_with_env(&argv, dir.as_deref(), &token, &opts.env).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                WorkerError::Unavailable(format!(
                    "`{}` not found (install uv, or set IMAGEPICKER_WORKER_CMD / IMAGEPICKER_WORKER_DIR): {e}",
                    argv[0]
                ))
            } else {
                WorkerError::Unavailable(format!("cannot start `{}`: {e}", argv[0]))
            }
        })?;
        let pid = child
            .id()
            .ok_or_else(|| WorkerError::Unavailable("worker exited immediately".into()))?;
        let guard = ChildGuard { pid };
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let tail: Arc<Mutex<VecDeque<String>>> = Arc::default();
        if let Some(err) = stderr {
            let tail = tail.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(err).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    tracing::debug!(target: "ai_worker", "{l}");
                    let mut t = tail.lock().unwrap();
                    if t.len() >= 30 {
                        t.pop_front();
                    }
                    t.push_back(l);
                }
            });
        }
        let tail_text = |tail: &Arc<Mutex<VecDeque<String>>>| {
            tail.lock()
                .unwrap()
                .iter()
                .rev()
                .take(8)
                .rev()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        };

        let stdout = stdout.ok_or_else(|| WorkerError::Protocol("no stdout".into()))?;
        let mut lines = BufReader::new(stdout).lines();
        let read_ready = async {
            loop {
                match lines.next_line().await {
                    Ok(Some(l)) => {
                        if let Ok(v) = serde_json::from_str::<Value>(l.trim()) {
                            if v.get("event").and_then(Value::as_str) == Some("ready") {
                                if let Some(p) = v.get("port").and_then(Value::as_u64) {
                                    return Some(p as u16);
                                }
                            }
                        }
                    }
                    Ok(None) | Err(_) => return None,
                }
            }
        };
        // Pipe EOF alone is unreliable on Windows (stray handle inheritance), so also watch the child.
        let outcome = tokio::time::timeout(self.cfg.start_timeout, async {
            tokio::select! {
                p = read_ready => p.ok_or(()),
                _ = child.wait() => Err(()),
            }
        })
        .await;
        let port = match outcome {
            Ok(Ok(p)) => p,
            Ok(Err(())) => {
                // give the stderr reader a moment to flush
                tokio::time::sleep(Duration::from_millis(200)).await;
                return Err(WorkerError::Protocol(format!(
                    "worker exited before it was ready:
{}",
                    tail_text(&tail)
                )));
            }
            Err(_) => {
                return Err(WorkerError::Timeout(format!(
                    "worker did not become ready within {:?}:
{}",
                    self.cfg.start_timeout,
                    tail_text(&tail)
                )))
            }
        };
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
        // keep draining stdout so the pipe never fills
        tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });

        let client = RpcClient::connect("127.0.0.1", port, &token).await?;
        let generation = self.shared.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let (shared, watch_client) = (self.shared.clone(), client.clone());
        tokio::spawn(async move {
            watch_client.closed().await;
            if shared.generation.load(Ordering::SeqCst) == generation {
                // Only flag a crash when nobody asked us to stop (state would be Stopped then).
                shared.tx.send_if_modified(|s| {
                    if matches!(s.state, WorkerState::Ready | WorkerState::Busy) {
                        s.state = WorkerState::Crashed;
                        s.error = Some("the AI worker process exited".into());
                        true
                    } else {
                        false
                    }
                });
            }
        });
        Ok(Conn {
            client,
            _guard: guard,
        })
    }

    /// Untyped call (diagnostics and tests); starts the worker when needed.
    pub async fn raw_call(&self, method: &str, params: Value) -> Result<Value> {
        self.rpc(method, params, None, None).await
    }

    async fn rpc(
        &self,
        method: &str,
        params: Value,
        progress: Option<ProgressTx>,
        cancel: Option<&CancelToken>,
    ) -> Result<Value> {
        let client = self.client().await?;
        let _busy = Busy::new(&self.shared);
        let r = client.call(method, params, progress, cancel).await;
        match &r {
            Err(WorkerError::Disconnected) => {
                let mut f = self.shared.failures.lock().unwrap();
                f.count += 1;
                f.last_at = Some(Instant::now());
                f.last_error = Some("the AI worker crashed".into());
            }
            _ => self.shared.reset_failures(),
        }
        r
    }
}

fn parse<T: serde::de::DeserializeOwned>(v: Value, what: &str) -> Result<T> {
    serde_json::from_value(v).map_err(|e| WorkerError::Protocol(format!("bad {what} payload: {e}")))
}

#[async_trait::async_trait]
impl AiWorker for ManagedWorker {
    fn status(&self) -> WorkerStatus {
        self.shared.tx.borrow().clone()
    }

    fn subscribe(&self) -> watch::Receiver<WorkerStatus> {
        self.shared.tx.subscribe()
    }

    async fn system_info(&self) -> Result<SystemInfo> {
        let v = self.rpc("system.info", json!({}), None, None).await?;
        let info: SystemInfo = parse(v, "system.info")?;
        self.shared.set_info(info.clone());
        Ok(info)
    }

    async fn models_list(&self) -> Result<ModelsListing> {
        let v = self.rpc("models.list", json!({}), None, None).await?;
        parse(v, "models.list")
    }

    async fn models_ensure(
        &self,
        ids: &[String],
        progress: Option<ProgressTx>,
        cancel: &CancelToken,
    ) -> Result<()> {
        self.rpc("models.ensure", json!({"ids": ids}), progress, Some(cancel))
            .await
            .map(|_| ())
    }

    async fn analyze_batch(
        &self,
        req: &AnalyzeRequest,
        progress: Option<ProgressTx>,
        cancel: &CancelToken,
    ) -> Result<AnalyzeResponse> {
        let params = serde_json::to_value(req)
            .map_err(|e| WorkerError::Protocol(format!("cannot encode request: {e}")))?;
        let v = self
            .rpc("analyze.batch", params, progress, Some(cancel))
            .await?;
        parse(v, "analyze.batch")
    }

    async fn mask_generate(&self, req: &MaskRequest) -> Result<MaskResponse> {
        let params = serde_json::to_value(req)
            .map_err(|e| WorkerError::Protocol(format!("cannot encode request: {e}")))?;
        let v = self.rpc("mask.generate", params, None, None).await?;
        parse(v, "mask.generate")
    }

    async fn beauty_prepare(&self, req: &BeautyPrepareRequest) -> Result<BeautyPrepareResponse> {
        let params = serde_json::to_value(req)
            .map_err(|e| WorkerError::Protocol(format!("cannot encode request: {e}")))?;
        let v = self.rpc("beauty.prepare", params, None, None).await?;
        parse(v, "beauty.prepare")
    }

    async fn faces_embed(&self, req: &FacesEmbedRequest) -> Result<FacesEmbedResponse> {
        let params = serde_json::to_value(req)
            .map_err(|e| WorkerError::Protocol(format!("cannot encode request: {e}")))?;
        let v = self.rpc("faces.embed", params, None, None).await?;
        parse(v, "faces.embed")
    }

    async fn besttake_compose(
        &self,
        req: &BestTakeComposeRequest,
    ) -> Result<BestTakeComposeResponse> {
        let params = serde_json::to_value(req)
            .map_err(|e| WorkerError::Protocol(format!("cannot encode request: {e}")))?;
        let v = self.rpc("besttake.compose", params, None, None).await?;
        parse(v, "besttake.compose")
    }

    async fn inpaint_run(&self, req: &InpaintRequest) -> Result<InpaintResponse> {
        let params = serde_json::to_value(req)
            .map_err(|e| WorkerError::Protocol(format!("cannot encode request: {e}")))?;
        let v = self.rpc("inpaint.run", params, None, None).await?;
        parse(v, "inpaint.run")
    }

    async fn enhance_run(&self, req: &EnhanceRequest) -> Result<EnhanceResponse> {
        let params = serde_json::to_value(req)
            .map_err(|e| WorkerError::Protocol(format!("cannot encode request: {e}")))?;
        let v = self.rpc("enhance.run", params, None, None).await?;
        parse(v, "enhance.run")
    }

    async fn llm_plan(&self, req: &LlmPlanRequest) -> Result<LlmPlanResponse> {
        let params = serde_json::to_value(req)
            .map_err(|e| WorkerError::Protocol(format!("cannot encode request: {e}")))?;
        let v = self.rpc("llm.plan", params, None, None).await?;
        parse(v, "llm.plan")
    }

    async fn vlm_suggest(&self, req: &VlmSuggestRequest) -> Result<VlmSuggestResponse> {
        let params = serde_json::to_value(req)
            .map_err(|e| WorkerError::Protocol(format!("cannot encode request: {e}")))?;
        let v = self.rpc("vlm.suggest", params, None, None).await?;
        parse(v, "vlm.suggest")
    }

    async fn vlm_describe(&self, req: &VlmDescribeRequest) -> Result<VlmDescribeResponse> {
        let params = serde_json::to_value(req)
            .map_err(|e| WorkerError::Protocol(format!("cannot encode request: {e}")))?;
        let v = self.rpc("vlm.describe", params, None, None).await?;
        parse(v, "vlm.describe")
    }

    async fn models_delete(&self, id: &str) -> Result<()> {
        match self
            .rpc("models.delete", json!({"id": id}), None, None)
            .await
        {
            // a worker that predates the method: let the core remove the directory itself
            Err(WorkerError::Rpc { code, .. }) if code == CODE_METHOD_NOT_FOUND => Err(
                WorkerError::Unavailable("this AI worker does not support models.delete".into()),
            ),
            other => other.map(|_| ()),
        }
    }

    fn configure(&self, opts: &WorkerOptions) {
        *self.options.lock().unwrap() = opts.clone();
    }

    async fn kill(&self) {
        let conn = self.conn.lock().await.take();
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
        if let Some(c) = conn {
            c.client.close();
            // dropping `c` kills the process tree
        }
        self.shared.reset_failures();
        self.shared.set_state(WorkerState::Stopped, None);
    }

    async fn shutdown(&self) {
        let conn = self.conn.lock().await.take();
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
        if let Some(c) = conn {
            let _ = tokio::time::timeout(
                Duration::from_secs(2),
                c.client.call("system.shutdown", json!({}), None, None),
            )
            .await;
            let _ = tokio::time::timeout(Duration::from_secs(3), c.client.closed()).await;
            c.client.close();
            // dropping `c` kills whatever is left of the process tree
        }
        self.shared.set_state(WorkerState::Stopped, None);
    }
}
