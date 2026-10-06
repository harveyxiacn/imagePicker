//! Catalog (SQLite), import, thumbnail scheduling, tasks and events.

pub mod analysis;
pub mod catalog;
pub mod db;
pub mod error;
pub mod events;
pub mod export;
pub mod imaging;
pub mod import;
pub mod model;
pub mod paths;
pub mod thumbs;

#[cfg(any(test, feature = "testutil"))]
pub mod fake_worker;
#[cfg(any(test, feature = "testutil"))]
pub mod testutil;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub use analysis::types::*;
pub use error::{CoreError, Result};
pub use events::{Event, EventBus};
pub use imaging::{Imaging, RealImaging};
pub use model::*;

use db::Db;
use ip_worker_client::{AiWorker, ManagedWorker, WorkerConfig};
use paths::DataDirs;
use thumbs::Thumbs;

pub struct CoreConfig {
    /// `None` resolves via `IMAGEPICKER_DATA_DIR` / platform default.
    pub data_dir: Option<PathBuf>,
    pub imaging: Arc<dyn Imaging>,
    /// Thumbnail worker threads; `None` = number of CPU cores.
    pub thumb_workers: Option<usize>,
    /// `None` = the real Python worker (started lazily on first use).
    pub worker: Option<Arc<dyn AiWorker>>,
}

impl CoreConfig {
    pub fn new(data_dir: Option<PathBuf>) -> Self {
        Self {
            data_dir,
            imaging: Arc::new(RealImaging),
            thumb_workers: None,
            worker: None,
        }
    }
}

pub struct Core {
    pub db: Db,
    pub imaging: Arc<dyn Imaging>,
    pub events: EventBus,
    pub thumbs: Thumbs,
    pub dirs: DataDirs,
    pub worker: Arc<dyn AiWorker>,
    task_seq: AtomicU64,
    pub(crate) runs: std::sync::Mutex<std::collections::HashMap<i64, analysis::RunInfo>>,
    pub(crate) analysis_gate: tokio::sync::Semaphore,
}

impl Core {
    pub fn open(cfg: CoreConfig) -> Result<Arc<Core>> {
        let dirs = DataDirs::new(paths::resolve_data_dir(cfg.data_dir.as_deref()));
        dirs.create()?;
        let db = Db::open(&dirs.catalog)?;
        db.with(|c| catalog::settle_stale_sessions(c))?;
        let tasks: i64 =
            db.with(|c| Ok(c.query_row("SELECT COUNT(*) FROM task", [], |r| r.get(0))?))?;
        let events = EventBus::new();
        let workers = cfg.thumb_workers.unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
        });
        let thumbs = Thumbs::new(
            db.clone(),
            cfg.imaging.clone(),
            events.clone(),
            &dirs.thumbs,
            &dirs.previews,
            workers,
        );
        let worker: Arc<dyn AiWorker> = match cfg.worker {
            Some(w) => w,
            None => {
                let mut wc = WorkerConfig::from_env();
                if wc.models_dir.is_none() {
                    wc.models_dir = Some(dirs.root.join("models"));
                }
                Arc::new(ManagedWorker::new(wc))
            }
        };
        let core = Arc::new(Core {
            db,
            imaging: cfg.imaging,
            events,
            thumbs,
            dirs,
            worker,
            task_seq: AtomicU64::new(tasks as u64),
            runs: Default::default(),
            analysis_gate: tokio::sync::Semaphore::new(1),
        });
        Core::spawn_worker_status_forwarder(&core);
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

    pub async fn sessions(&self) -> Result<Vec<Session>> {
        self.db.call(|c| catalog::list_sessions(c)).await
    }

    /// Removes catalog rows and cached images; originals are never touched.
    pub async fn delete_session(&self, id: i64) -> Result<()> {
        let keys = self
            .db
            .call(move |c| {
                let keys = catalog::delete_session(c, id)?;
                analysis::store::purge_session_groups(c, id)?;
                analysis::store::purge_orphan_people(c)?;
                Ok(keys)
            })
            .await?;
        self.runs.lock().unwrap().remove(&id);
        self.thumbs.purge(&keys);
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
        self.db.call(move |c| catalog::query_photos(c, &q)).await
    }

    pub async fn photo(&self, id: i64) -> Result<Photo> {
        self.db.call(move |c| catalog::get_photo(c, id)).await
    }

    /// Applies a rating/flag/colour patch and broadcasts `photos.updated`. Returns the updated count.
    pub async fn patch_photos(&self, req: PatchRequest) -> Result<usize> {
        catalog::validate_patch(&req)?;
        let updates = self
            .db
            .call(move |c| catalog::patch_photos(c, &req))
            .await?;
        let n = updates.len();
        if n > 0 {
            self.events.emit(Event::PhotosUpdated { items: updates });
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
        self.thumbs.ensure(id, size).await
    }

    pub async fn preview_path(&self, id: i64, size: u32) -> Result<PathBuf> {
        if !thumbs::PREVIEW_SIZES.contains(&size) {
            return Err(CoreError::bad_request("s must be 1024, 2048 or 4096"));
        }
        self.thumbs.ensure(id, size).await
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
mod tests_m2;
