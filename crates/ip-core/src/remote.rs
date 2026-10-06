//! Phone side of remote AI (docs/api-contract-m8.md section C): the paired-host secret, the
//! switch between the local worker and a [`RemoteWorker`], and the connection status.
//!
//! The device token is kept in `<data dir>/remote.json` (mode 0600 on Unix), never in
//! `settings.json` and never in any API response. The token is only ever sent to the host it was
//! issued by: it is bound to the `host_url` recorded next to it, so editing `remote_ai.host_url`
//! in the settings cannot redirect it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use ip_worker_client::*;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::error::{CoreError, Result};
use crate::settings::Settings;
use crate::Core;

pub const SECRET_FILE: &str = "remote.json";
/// A status probe is reused for this long.
const PROBE_TTL: Duration = Duration::from_secs(5);

#[derive(Clone, Serialize, Deserialize)]
pub struct RemoteSecret {
    pub host_url: String,
    pub device_id: String,
    pub token: String,
}

impl std::fmt::Debug for RemoteSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteSecret")
            .field("host_url", &self.host_url)
            .field("device_id", &self.device_id)
            .finish_non_exhaustive()
    }
}

/// The secret file (atomic writes).
#[derive(Debug, Clone)]
pub struct RemoteSecretStore {
    path: PathBuf,
}

impl RemoteSecretStore {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join(SECRET_FILE),
        }
    }

    pub fn load(&self) -> Option<RemoteSecret> {
        let b = std::fs::read(&self.path).ok()?;
        serde_json::from_slice(&b).ok()
    }

    pub fn save(&self, s: &RemoteSecret) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(s)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    pub fn clear(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

type Slot = Arc<RwLock<Option<Arc<RemoteWorker>>>>;

/// Routes calls to the remote worker while one is configured, to the local worker otherwise.
pub struct SwitchWorker {
    local: Arc<dyn AiWorker>,
    slot: Slot,
}

impl SwitchWorker {
    fn pick(&self) -> Arc<dyn AiWorker> {
        match self.slot.read().unwrap().as_ref() {
            Some(r) => r.clone() as Arc<dyn AiWorker>,
            None => self.local.clone(),
        }
    }
}

#[async_trait::async_trait]
impl AiWorker for SwitchWorker {
    fn status(&self) -> WorkerStatus {
        self.pick().status()
    }
    /// The local worker's channel: remote reachability is reported by `GET /api/remote/status`.
    fn subscribe(&self) -> watch::Receiver<WorkerStatus> {
        self.local.subscribe()
    }
    async fn system_info(&self) -> ip_worker_client::Result<SystemInfo> {
        self.pick().system_info().await
    }
    async fn models_list(&self) -> ip_worker_client::Result<ModelsListing> {
        self.pick().models_list().await
    }
    async fn models_ensure(
        &self,
        ids: &[String],
        progress: Option<ProgressTx>,
        cancel: &CancelToken,
    ) -> ip_worker_client::Result<()> {
        self.pick().models_ensure(ids, progress, cancel).await
    }
    async fn analyze_batch(
        &self,
        req: &AnalyzeRequest,
        progress: Option<ProgressTx>,
        cancel: &CancelToken,
    ) -> ip_worker_client::Result<AnalyzeResponse> {
        self.pick().analyze_batch(req, progress, cancel).await
    }
    async fn mask_generate(&self, req: &MaskRequest) -> ip_worker_client::Result<MaskResponse> {
        self.pick().mask_generate(req).await
    }
    async fn beauty_prepare(
        &self,
        req: &BeautyPrepareRequest,
    ) -> ip_worker_client::Result<BeautyPrepareResponse> {
        self.pick().beauty_prepare(req).await
    }
    async fn faces_embed(
        &self,
        req: &FacesEmbedRequest,
    ) -> ip_worker_client::Result<FacesEmbedResponse> {
        self.pick().faces_embed(req).await
    }
    async fn besttake_compose(
        &self,
        req: &BestTakeComposeRequest,
    ) -> ip_worker_client::Result<BestTakeComposeResponse> {
        self.pick().besttake_compose(req).await
    }
    async fn inpaint_run(&self, req: &InpaintRequest) -> ip_worker_client::Result<InpaintResponse> {
        self.pick().inpaint_run(req).await
    }
    async fn enhance_run(&self, req: &EnhanceRequest) -> ip_worker_client::Result<EnhanceResponse> {
        self.pick().enhance_run(req).await
    }
    async fn llm_plan(&self, req: &LlmPlanRequest) -> ip_worker_client::Result<LlmPlanResponse> {
        self.pick().llm_plan(req).await
    }
    async fn vlm_suggest(
        &self,
        req: &VlmSuggestRequest,
    ) -> ip_worker_client::Result<VlmSuggestResponse> {
        self.pick().vlm_suggest(req).await
    }
    async fn vlm_describe(
        &self,
        req: &VlmDescribeRequest,
    ) -> ip_worker_client::Result<VlmDescribeResponse> {
        self.pick().vlm_describe(req).await
    }
    async fn models_delete(&self, id: &str) -> ip_worker_client::Result<()> {
        self.pick().models_delete(id).await
    }
    fn configure(&self, opts: &WorkerOptions) {
        self.local.configure(opts);
    }
    async fn shutdown(&self) {
        self.local.shutdown().await;
    }
    async fn kill(&self) {
        // a remote worker is not ours to kill
        if self.slot.read().unwrap().is_none() {
            self.local.kill().await;
        }
    }
}

/// `GET /api/remote/status`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RemoteStatus {
    pub enabled: bool,
    pub host_url: String,
    pub connected: bool,
    pub host_tier: Option<String>,
    pub last_error: Option<String>,
}

/// Remote-AI state of a [`Core`].
pub struct RemoteAi {
    secret: RemoteSecretStore,
    slot: Slot,
    probe: Mutex<Option<(Instant, bool)>>,
    /// `system.info` was already asked of the current host for its tier.
    tier_tried: std::sync::atomic::AtomicBool,
}

impl RemoteAi {
    pub(crate) fn new(data_dir: &Path) -> Self {
        Self {
            secret: RemoteSecretStore::new(data_dir),
            slot: Arc::new(RwLock::new(None)),
            probe: Mutex::new(None),
            tier_tried: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub(crate) fn wrap(&self, local: Arc<dyn AiWorker>) -> Arc<dyn AiWorker> {
        Arc::new(SwitchWorker {
            local,
            slot: self.slot.clone(),
        })
    }

    pub fn secret_store(&self) -> &RemoteSecretStore {
        &self.secret
    }

    /// The remote worker currently routed to, if any.
    pub fn worker(&self) -> Option<Arc<RemoteWorker>> {
        self.slot.read().unwrap().clone()
    }

    /// Re-evaluates "enabled + paired with exactly this host" after settings changed.
    pub(crate) fn refresh(&self, s: &Settings) {
        *self.probe.lock().unwrap() = None;
        self.tier_tried
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let want = if s.remote_ai.enabled && !s.remote_ai.host_url.is_empty() {
            self.secret
                .load()
                .filter(|sec| sec.host_url == s.remote_ai.host_url && !sec.token.is_empty())
        } else {
            None
        };
        let mut slot = self.slot.write().unwrap();
        match want {
            None => *slot = None,
            Some(sec) => {
                let unchanged = slot.as_ref().is_some_and(|w| w.host_url() == sec.host_url);
                if !unchanged {
                    match RemoteWorker::new(RemoteConfig::new(sec.host_url, sec.token)) {
                        Ok(w) => *slot = Some(Arc::new(w)),
                        Err(e) => {
                            tracing::warn!(error = %e, "cannot set up the remote AI client");
                            *slot = None;
                        }
                    }
                }
            }
        }
    }
}

impl Core {
    /// Stores a successful pairing and switches to the remote worker.
    pub fn remote_save_pairing(
        self: &Arc<Self>,
        host_url: &str,
        outcome: &PairOutcome,
    ) -> Result<()> {
        let host_url = normalize_host_url(host_url).map_err(CoreError::BadRequest)?;
        self.remote.secret.save(&RemoteSecret {
            host_url: host_url.clone(),
            device_id: outcome.device_id.clone(),
            token: outcome.token.clone(),
        })?;
        let patch = serde_json::json!({"remote_ai": {"enabled": true, "host_url": host_url}});
        let (old, new) = self.settings.patch(&patch)?;
        self.apply_settings(Some(&old), &new);
        Ok(())
    }

    /// `DELETE /api/remote/connect`: forgets the token and goes back to local AI.
    pub fn remote_disconnect(self: &Arc<Self>) -> Result<()> {
        self.remote.secret.clear();
        let patch = serde_json::json!({"remote_ai": {"enabled": false, "host_url": ""}});
        let (old, new) = self.settings.patch(&patch)?;
        self.apply_settings(Some(&old), &new);
        Ok(())
    }

    /// `GET /api/remote/status`. Probes the host (cached for a few seconds) when enabled.
    pub async fn remote_status(&self) -> RemoteStatus {
        let s = self.settings();
        let mut st = RemoteStatus {
            enabled: s.remote_ai.enabled,
            host_url: s.remote_ai.host_url.clone(),
            connected: false,
            host_tier: None,
            last_error: None,
        };
        if !st.enabled {
            return st;
        }
        let Some(w) = self.remote.worker() else {
            st.last_error = Some(if self.remote.secret.load().is_some() {
                "the saved pairing belongs to another host; pair again".into()
            } else {
                "not paired with a host".into()
            });
            return st;
        };
        let cached = self
            .remote
            .probe
            .lock()
            .unwrap()
            .filter(|(t, _)| t.elapsed() < PROBE_TTL)
            .map(|(_, ok)| ok);
        st.connected = match cached {
            Some(ok) => ok,
            None => {
                let ok = w.ping().await.is_ok();
                *self.remote.probe.lock().unwrap() = Some((Instant::now(), ok));
                ok
            }
        };
        // the tier is only known once the host's worker has been asked; ask it once
        if st.connected
            && w.host_tier().is_none()
            && !self
                .remote
                .tier_tried
                .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            let _ = tokio::time::timeout(Duration::from_secs(10), w.system_info()).await;
        }
        st.host_tier = w.host_tier();
        if !st.connected {
            st.last_error = w.last_error();
        }
        st
    }
}
