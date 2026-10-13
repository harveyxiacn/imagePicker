//! Catalog (SQLite), import, thumbnail scheduling, tasks and events.

pub mod admin;
pub mod analysis;
pub mod assistant;
pub mod auth;
pub mod backup;
pub mod catalog;
pub mod collections;
pub mod db;
pub mod device;
pub mod diagnostics;
pub mod edit;
pub mod error;
pub mod events;
pub mod export;
pub mod files;
pub mod generate;
pub mod imaging;
pub mod import;
pub mod jpegmeta;
pub mod jsonfix;
pub mod lazy_renderer;
pub mod lite;
pub mod logging;
pub mod model;
pub mod paths;
pub mod remote;
pub mod roots;
pub mod runtime;
pub mod settings;
pub mod tasks;
pub mod taste;
pub mod thumbs;
pub mod watch;
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
    AutoCropKind, AutoCropOut, AutoCropRequest, AutoRequest, CheckedStack, EditDoc, EditPut,
    LutOut, PresetCreate, PresetOut, Preview, PreviewRequest, RenderService, SyncRequest,
};
pub use error::{CoreError, Result};
pub use events::{Event, EventBus};
pub use files::{RejectFilesReport, RejectFilesRequest, RelocateReport, RelocateRequest, Trasher};
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
    /// RAW+JPEG pairing, rescans, file watching and safe reject (`files`, `watch`).
    pub(crate) files: files::FilesState,
    /// Catalog integrity and backups (`backup`).
    pub catalog_guard: backup::CatalogGuard,
    pub(crate) evicting: std::sync::atomic::AtomicBool,
    task_seq: AtomicU64,
    /// Running recorded tasks (cancel tokens, live progress).
    pub(crate) tasks: tasks::TaskRegistry,
    pub(crate) runs: std::sync::Mutex<std::collections::HashMap<i64, analysis::RunInfo>>,
    /// Analysis runs of a previous process, waiting for [`Core::resume_analyses`].
    pub(crate) open_analyses: std::sync::Mutex<Vec<tasks::OpenAnalysis>>,
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
        // an unreadable catalog is moved aside (state "recovery"), see `backup`
        let (db, catalog_guard) = backup::open_catalog(&dirs)?;
        db.with(|c| catalog::settle_stale_sessions(c))?;
        // catalogs from before RAW+JPEG pairing: pair what is already imported
        db.with(files::pair_all)?;
        // analysis runs a previous process left open: `resume_analyses` picks them up
        let open_analyses = db.with(|c| tasks::open_analyses(c))?;
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
        let full_cache = thumbs::FullCache::new(&dirs.full, thumbs::FULL_CACHE_BUDGET);
        let thumbs = Thumbs::new(
            db.clone(),
            cfg.imaging.clone(),
            events.clone(),
            &dirs.thumbs,
            &dirs.previews,
            full_cache.clone(),
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
                wc.crash_dir = Some(dirs.crashes.clone());
                let crashes = dirs.crashes.clone();
                wc.crash_sink = Some(ip_worker_client::CrashSink(Arc::new(
                    move |kind, message, stderr_tail| {
                        let _ = logging::write_crash(
                            &crashes,
                            &logging::CrashReport {
                                kind: kind.to_string(),
                                message: message.to_string(),
                                detail: format!("worker stderr (last lines):\n{stderr_tail}"),
                                ..Default::default()
                            },
                        );
                    },
                )));
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
            full_cache,
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
            files: Default::default(),
            catalog_guard,
            evicting: std::sync::atomic::AtomicBool::new(false),
            task_seq: AtomicU64::new(task_seq),
            tasks: Default::default(),
            runs: Default::default(),
            open_analyses: std::sync::Mutex::new(open_analyses),
            analysis_gate: tokio::sync::Semaphore::new(1),
            taste_lock: tokio::sync::Mutex::new(()),
        });
        Core::spawn_worker_status_forwarder(&core);
        core.apply_settings(None, &core.settings());
        Core::spawn_cache_janitor(&core);
        Core::spawn_catalog_guard(&core);
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
        let removed = self.runs.lock().unwrap().remove(&id);
        if let Some(r) = removed.filter(|r| r.status.state == RunState::Paused) {
            // a paused run has no task left to end its history row
            let (tid, d, t) = (r.task_id, r.status.done, r.status.total);
            let _ = self
                .db
                .call(move |c| tasks::finish(c, &tid, tasks::CANCELLED, d, t, None))
                .await;
        }
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
            let mut ids: Vec<i64> = updates.iter().map(|u| u.id).collect();
            // the RAW companions got the same values: their sidecars follow too
            let own = ids.clone();
            ids.extend(
                self.db
                    .call(move |c| files::companion_ids(c, &own))
                    .await
                    .unwrap_or_default(),
            );
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

    /// Full-resolution image for the loupe at 100% and above (`GET /api/full/{id}`): upright, sRGB,
    /// the photo's own pixel size (the rendered edit for edited photos), cached on disk. Errors:
    /// `NotFound` (unknown photo, original missing), `Unsupported` (the original cannot be decoded).
    pub async fn full_path(&self, id: i64) -> Result<PathBuf> {
        self.image_path(id, thumbs::FULL_SIZE).await
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
mod tests_crop;
#[cfg(test)]
mod tests_diag;
#[cfg(test)]
mod tests_files;
#[cfg(test)]
mod tests_filters;
#[cfg(test)]
mod tests_full;
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
mod tests_p1c;
#[cfg(test)]
mod tests_safety;
#[cfg(test)]
mod tests_tasks;

impl Drop for Core {
    fn drop(&mut self) {
        // Do not let the process exit while the GPU warm-up thread is inside the driver.
        lazy_renderer::wait_for_warmup();
    }
}
