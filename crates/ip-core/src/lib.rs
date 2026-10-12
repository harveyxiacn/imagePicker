//! Catalog (SQLite), import, thumbnail scheduling, tasks and events.

pub mod admin;
pub mod analysis;
pub mod assistant;
pub mod auth;
pub mod catalog;
pub mod collections;
pub mod db;
pub mod device;
pub mod edit;
pub mod error;
pub mod events;
pub mod export;
pub mod generate;
pub mod imaging;
pub mod import;
pub mod jpegmeta;
pub mod jsonfix;
pub mod lazy_renderer;
pub mod lite;
pub mod model;
pub mod paths;
pub mod remote;
pub mod roots;
pub mod runtime;
pub mod settings;
pub mod tasks;
pub mod taste;
pub mod thumbs;
pub mod xmp;

#[cfg(any(test, feature = "testutil"))]
pub mod fake_renderer;
#[cfg(any(test, feature = "testutil"))]
pub mod fake_worker;
#[cfg(any(test, feature = "testutil"))]
pub mod testutil;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub use analysis::types::*;
pub use edit::{
    AutoRequest, CheckedStack, EditDoc, EditPut, LutOut, PresetCreate, PresetOut, Preview,
    PreviewRequest, RenderService, SyncRequest,
};
pub use error::{CoreError, Result};
pub use events::{Event, EventBus};
pub use generate::{
    BaseChoice, BaseFrame, BestTakeAutoBody, BestTakeChoice, BestTakePlan, BestTakeRequest,
    BestTakeResult, EnhanceBody, InpaintBody, Stroke,
};
pub use imaging::{Imaging, RealImaging};
pub use ip_render;
pub use model::*;
pub use settings::Settings;

use db::Db;
use ip_worker_client::{
    AiWorker, ManagedWorker, TimeoutHandle, TimeoutWorker, UnavailableWorker, WorkerConfig,
    WorkerTimeouts,
};
use paths::DataDirs;
use settings::SettingsStore;
use thumbs::Thumbs;

pub struct CoreConfig {
    /// `None` resolves via `IMAGEPICKER_DATA_DIR` / platform default. **Android has no
    /// platform default**: the shell passes the app's private files dir here (or sets
    /// `IMAGEPICKER_DATA_DIR`), otherwise [`Core::open`] fails.
    pub data_dir: Option<PathBuf>,
    pub imaging: Arc<dyn Imaging>,
    /// Thumbnail worker threads; `None` = number of CPU cores (at most 4 on Android).
    pub thumb_workers: Option<usize>,
    /// `None` = the real Python worker (started lazily on first use).
    pub worker: Option<Arc<dyn AiWorker>>,
    /// `None` = `ip_render::create_renderer` (GPU preferred; CPU when `force_cpu` or
    /// `IMAGEPICKER_RENDER=cpu`).
    pub renderer: Option<Arc<dyn ip_render::Renderer>>,
    pub force_cpu: bool,
}

impl CoreConfig {
    pub fn new(data_dir: Option<PathBuf>) -> Self {
        Self {
            data_dir,
            imaging: Arc::new(RealImaging),
            thumb_workers: None,
            worker: None,
            renderer: None,
            force_cpu: false,
        }
    }
}

pub struct Core {
    pub db: Db,
    pub imaging: Arc<dyn Imaging>,
    pub events: EventBus,
    pub thumbs: Thumbs,
    pub dirs: DataDirs,
    /// The AI worker, behind per-call timeouts ([`Core::worker_timeouts`]).
    pub worker: Arc<dyn AiWorker>,
    /// The on-device `lite` analysis backend (always available, see [`lite::LiteWorker`]).
    pub lite: Arc<dyn AiWorker>,
    /// Adjustable per-method worker timeouts (defaults + `IMAGEPICKER_WORKER_TIMEOUT*`).
    pub worker_timeouts: TimeoutHandle,
    pub render: Arc<RenderService>,
    /// Persisted settings (`docs/api-contract-m6.md` section D).
    pub settings: SettingsStore,
    /// Assistant plans (in memory, 30 minute TTL).
    pub assistant: assistant::AssistantState,
    /// The installable AI runtime (M7).
    pub runtime: runtime::RuntimeManager,
    /// Remote AI (phone -> home PC), M8.
    pub remote: remote::RemoteAi,
    pub(crate) xmp: xmp::XmpState,
    pub(crate) evicting: std::sync::atomic::AtomicBool,
    task_seq: AtomicU64,
    /// Running recorded tasks (cancel tokens, live progress).
    pub(crate) tasks: tasks::TaskRegistry,
    pub(crate) runs: std::sync::Mutex<std::collections::HashMap<i64, analysis::RunInfo>>,
    pub(crate) analysis_gate: tokio::sync::Semaphore,
    pub(crate) taste_lock: tokio::sync::Mutex<()>,
}

impl Core {
    pub fn open(cfg: CoreConfig) -> Result<Arc<Core>> {
        if !paths::data_dir_resolvable(cfg.data_dir.as_deref()) {
            return Err(CoreError::Internal(anyhow::anyhow!(
                "no data directory: on Android the shell must pass CoreConfig::data_dir \
                 (or set IMAGEPICKER_DATA_DIR)"
            )));
        }
        let dirs = DataDirs::new(paths::resolve_data_dir(cfg.data_dir.as_deref()));
        dirs.create()?;
        let db = Db::open(&dirs.catalog)?;
        db.with(|c| catalog::settle_stale_sessions(c))?;
        // tasks left running by a previous process: recorded as interrupted, scratch removed
        for id in db.with(|c| tasks::settle_interrupted(c))? {
            let _ = std::fs::remove_dir_all(dirs.gen.join(&id));
        }
        let task_seq = db.with(|c| tasks::max_seq(c))?;
        let settings = SettingsStore::load(&dirs.root);
        let events = EventBus::new();
        let workers = cfg.thumb_workers.unwrap_or_else(|| {
            let n = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4);
            if cfg!(target_os = "android") {
                n.clamp(1, 4)
            } else {
                n
            }
        });
        let lite: Arc<dyn AiWorker> = Arc::new(
            lite::LiteWorker::new(cfg.imaging.clone(), lite::default_threads())
                .with_models_dir(&dirs.root.join("models")),
        );
        let thumbs = Thumbs::new(
            db.clone(),
            cfg.imaging.clone(),
            events.clone(),
            &dirs.thumbs,
            &dirs.previews,
            workers,
        );
        let worker_timeouts = TimeoutHandle::new(WorkerTimeouts::from_env());
        let runtime = runtime::RuntimeManager::new(&dirs.root, dirs.logs.clone());
        runtime::configure_heif_decoding(runtime.root(), &dirs.root);
        let worker: Arc<dyn AiWorker> = match cfg.worker {
            Some(w) => w,
            None if ip_worker_client::worker_unsupported() => Arc::new(UnavailableWorker::new(
                "the AI worker is not available on this device; use the lite profile or a remote AI host",
            )),
            None => {
                let mut wc = WorkerConfig::from_env();
                wc.runtime = Some(ip_worker_client::RuntimeLaunch {
                    root: runtime.root().to_path_buf(),
                    version: runtime.version().to_string(),
                });
                if wc.models_dir.is_none() {
                    wc.models_dir = Some(dirs.root.join("models"));
                }
                Arc::new(ManagedWorker::new(wc))
            }
        };
        let remote = remote::RemoteAi::new(&dirs.root);
        let worker = remote.wrap(worker);
        let worker: Arc<dyn AiWorker> =
            Arc::new(TimeoutWorker::new(worker, worker_timeouts.clone()));
        let renderer: Arc<dyn ip_render::Renderer> = match cfg.renderer {
            Some(r) => r,
            None => {
                let cpu_env = std::env::var("IMAGEPICKER_RENDER")
                    .map(|v| v.trim().eq_ignore_ascii_case("cpu"))
                    .unwrap_or(false);
                let cpu_setting = settings.get().render.backend == "cpu";
                lazy_renderer::LazyRenderer::spawn(!(cfg.force_cpu || cpu_env || cpu_setting))
            }
        };
        let render = Arc::new(RenderService::new(edit::service::ServiceParts {
            renderer,
            imaging: cfg.imaging.clone(),
            db: db.clone(),
            worker: worker.clone(),
            luts_dir: dirs.luts.clone(),
            masks_dir: dirs.masks.clone(),
            beauty_dir: dirs.beauty.clone(),
            edited_thumbs_dir: dirs.edited_thumbs.clone(),
            edited_previews_dir: dirs.edited_previews.clone(),
            edits_dir: dirs.edits.clone(),
        }));
        let core = Arc::new(Core {
            db,
            imaging: cfg.imaging,
            events,
            thumbs,
            dirs,
            worker,
            lite,
            worker_timeouts,
            render,
            settings,
            assistant: Default::default(),
            runtime,
            remote,
            xmp: Default::default(),
            evicting: std::sync::atomic::AtomicBool::new(false),
            task_seq: AtomicU64::new(task_seq),
            tasks: Default::default(),
            runs: Default::default(),
            analysis_gate: tokio::sync::Semaphore::new(1),
            taste_lock: tokio::sync::Mutex::new(()),
        });
        Core::spawn_worker_status_forwarder(&core);
        core.apply_settings(None, &core.settings());
        Core::spawn_cache_janitor(&core);
        Ok(core)
    }

    pub(crate) fn next_task_seq(&self) -> u64 {
        self.task_seq.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn data_dir(&self) -> &Path {
        &self.dirs.root
    }

    // ------------------------------------------------------------ sessions

    pub async fn session(&self, id: i64) -> Result<Session> {
        self.db.call(move |c| catalog::get_session(c, id)).await
    }

    /// Cameras/phones that took the photos of a session (404 for an unknown session).
    pub async fn devices(&self, session_id: i64) -> Result<Vec<SessionDevice>> {
        self.db
            .call(move |c| catalog::session_devices(c, session_id))
            .await
    }

    pub async fn sessions(&self) -> Result<Vec<Session>> {
        self.db.call(|c| catalog::list_sessions(c)).await
    }

    /// Removes catalog rows and cached images; originals are never touched.
    pub async fn delete_session(&self, id: i64) -> Result<()> {
        let (keys, orphans) = self
            .db
            .call(move |c| {
                let orphans = catalog::session_orphans(c, id)?;
                let keys = catalog::delete_session(c, id)?;
                analysis::store::purge_session_groups(c, id)?;
                analysis::store::purge_orphan_people(c)?;
                Ok((keys, orphans))
            })
            .await?;
        self.runs.lock().unwrap().remove(&id);
        self.thumbs.purge(&keys);
        // rendered (edited) thumbnails/previews and generated patch assets of the deleted photos
        for (photo_id, fast_key, hashes) in &orphans {
            for h in hashes {
                self.render.purge_edited(&edit::edited_key(fast_key, h));
            }
            self.render.patches.remove_photo(*photo_id);
        }
        Ok(())
    }

    /// Polls until the session leaves the scanning/thumbnailing states.
    pub async fn wait_session_ready(&self, id: i64, timeout: Duration) -> Result<Session> {
        let start = std::time::Instant::now();
        loop {
            let s = self.session(id).await?;
            if s.import_state == ImportState::Ready {
                return Ok(s);
            }
            if start.elapsed() > timeout {
                return Err(CoreError::Internal(anyhow::anyhow!(
                    "timed out waiting for import of session {id}"
                )));
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }

    // ------------------------------------------------------------ photos

    pub async fn photos(&self, q: PhotoQuery) -> Result<PhotosPage> {
        let db = self.db.clone();
        self.db
            .call(move |c| catalog::query_photos_cached(c, &q, Some(&db)))
            .await
    }

    pub async fn photo(&self, id: i64) -> Result<Photo> {
        self.db.call(move |c| catalog::get_photo(c, id)).await
    }

    /// Applies a rating/flag/colour patch and broadcasts `photos.updated`. Returns the updated count.
    pub async fn patch_photos(self: &Arc<Self>, req: PatchRequest) -> Result<usize> {
        catalog::validate_patch(&req)?;
        let (updates, labels) = self
            .db
            .call(move |c| {
                let updates = catalog::patch_photos(c, &req)?;
                let labels = taste::record_patch(c, &req)?;
                Ok((updates, labels))
            })
            .await?;
        self.taste_after_labels(labels);
        let n = updates.len();
        if n > 0 {
            let ids: Vec<i64> = updates.iter().map(|u| u.id).collect();
            self.events.emit(Event::PhotosUpdated { items: updates });
            self.xmp_touch(&ids);
        }
        Ok(n)
    }

    pub async fn viewport(&self, ids: Vec<i64>) -> Result<()> {
        self.thumbs.viewport(ids).await
    }

    // ------------------------------------------------------------ images

    /// Grid/detail thumbnail (256 or 512), generated synchronously on a cache miss.
    pub async fn thumb_path(&self, id: i64, size: u32) -> Result<PathBuf> {
        if !thumbs::THUMB_SIZES.contains(&size) {
            return Err(CoreError::bad_request("s must be 256 or 512"));
        }
        self.image_path(id, size).await
    }

    pub async fn preview_path(&self, id: i64, size: u32) -> Result<PathBuf> {
        if !thumbs::PREVIEW_SIZES.contains(&size) {
            return Err(CoreError::bad_request("s must be 1024, 2048 or 4096"));
        }
        self.image_path(id, size).await
    }

    /// Cached thumbnail/preview: the rendered image for edited photos, the original otherwise.
    async fn image_path(&self, id: i64, size: u32) -> Result<PathBuf> {
        let r = self.db.call(move |c| catalog::photo_ref(c, id)).await?;
        if r.edit_hash.is_some() {
            if let Some(p) = self.render.edited_image(r.clone(), size).await? {
                return Ok(p);
            }
        }
        self.thumbs.ensure_ref(r, size).await
    }

    pub async fn original(&self, id: i64) -> Result<(PathBuf, ip_imaging::ImageFormat)> {
        let r = self.db.call(move |c| catalog::photo_ref(c, id)).await?;
        if !r.path.is_file() {
            return Err(CoreError::not_found(format!(
                "original file is missing: {}",
                r.path.display()
            )));
        }
        Ok((r.path, r.format))
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_lite;
#[cfg(test)]
mod tests_m2;
#[cfg(test)]
mod tests_m3;
#[cfg(test)]
mod tests_m4;
#[cfg(test)]
mod tests_m5;
#[cfg(test)]
mod tests_m6;
#[cfg(test)]
mod tests_tasks;

impl Drop for Core {
    fn drop(&mut self) {
        // Do not let the process exit while the GPU warm-up thread is inside the driver.
        lazy_renderer::wait_for_warmup();
    }
}
