//! File watching: files added to, removed from or renamed in an imported folder show up in the
//! grid without a manual re-import (`Core::start_watching`, desktop builds; a no-op on mobile).
//!
//! Change notifications are debounced per folder and answered with a rescan of its sessions
//! ([`Core::rescan_session`]): new files are imported, vanished ones flagged missing.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::Core;

/// Quiet period after the last change in a folder before it is rescanned.
pub const DEFAULT_DEBOUNCE_MS: u64 = 800;
/// A folder that keeps changing (a long copy) is rescanned at least this often.
const MAX_WAIT: Duration = Duration::from_secs(3);

/// Shared with the watcher callbacks (which run on notify's thread).
#[derive(Default)]
pub(crate) struct Pending {
    /// root id -> (first change, last change)
    roots: Mutex<HashMap<i64, (Instant, Instant)>>,
    wake: tokio::sync::Notify,
}

pub struct WatchState {
    enabled: AtomicBool,
    debounce_ms: AtomicU64,
    pub(crate) pending: Arc<Pending>,
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    watchers: Mutex<HashMap<i64, notify::RecommendedWatcher>>,
}

impl Default for WatchState {
    fn default() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            debounce_ms: AtomicU64::new(DEFAULT_DEBOUNCE_MS),
            pending: Default::default(),
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            watchers: Default::default(),
        }
    }
}

/// Does a change of `path` possibly change the set of photos? (image files and directories;
/// not sidecars, temp files or our own `_rejected` folder).
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn relevant(path: &Path) -> bool {
    if path
        .components()
        .any(|c| c.as_os_str() == crate::files::REJECTED_DIR)
    {
        return false;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    if name.starts_with('.') {
        return false;
    }
    match path.extension() {
        // directories and extension-less names (a folder that was deleted can no longer be asked)
        None => true,
        Some(_) => ip_imaging::ImageFormat::from_path(path).is_some(),
    }
}

impl Core {
    /// Sets the quiet period before a changed folder is rescanned (tests use a short one).
    pub fn set_watch_debounce(&self, d: Duration) {
        self.files
            .watch
            .debounce_ms
            .store(d.as_millis() as u64, Ordering::SeqCst);
    }

    /// Starts watching every imported folder, and every folder imported from now on. Needs a
    /// running tokio runtime. Calling it again does nothing.
    pub fn start_watching(self: &Arc<Self>) {
        if self.files.watch.enabled.swap(true, Ordering::SeqCst) {
            return;
        }
        let weak = Arc::downgrade(self);
        let pending = self.files.watch.pending.clone();
        tokio::spawn(async move {
            loop {
                pending.wake.notified().await;
                loop {
                    let Some(core) = weak.upgrade() else { return };
                    let quiet =
                        Duration::from_millis(core.files.watch.debounce_ms.load(Ordering::SeqCst));
                    drop(core);
                    tokio::time::sleep(
                        (quiet / 4).clamp(Duration::from_millis(20), Duration::from_millis(250)),
                    )
                    .await;
                    let now = Instant::now();
                    let (due, left) = {
                        let mut roots = pending.roots.lock().unwrap();
                        let due: Vec<i64> = roots
                            .iter()
                            .filter(|(_, (first, last))| {
                                now.duration_since(*last) >= quiet
                                    || now.duration_since(*first) >= MAX_WAIT
                            })
                            .map(|(r, _)| *r)
                            .collect();
                        for r in &due {
                            roots.remove(r);
                        }
                        (due, !roots.is_empty())
                    };
                    let Some(core) = weak.upgrade() else { return };
                    for root in due {
                        core.rescan_root(root).await;
                    }
                    if !left {
                        break;
                    }
                }
            }
        });
        let core = self.clone();
        tokio::spawn(async move {
            let roots = core
                .db
                .call(|c| {
                    let mut st = c.prepare(
                        "SELECT DISTINCT r.id, r.path FROM root_folder r
                         JOIN session s ON s.root_id=r.id",
                    )?;
                    let v = st
                        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    Ok(v)
                })
                .await
                .unwrap_or_default();
            for (id, path) in roots {
                core.watch_root(id, Path::new(&path));
            }
        });
    }

    /// Watches one folder when watching is on (replaces an earlier watcher of the same root).
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub(crate) fn watch_root(&self, root_id: i64, path: &Path) {
        use notify::{RecursiveMode, Watcher};
        if !self.files.watch.enabled.load(Ordering::SeqCst) || !path.is_dir() {
            return;
        }
        let pending = self.files.watch.pending.clone();
        let handler = move |res: notify::Result<notify::Event>| {
            let Ok(ev) = res else { return };
            if matches!(ev.kind, notify::EventKind::Access(_)) {
                return;
            }
            if !ev.paths.iter().any(|p| relevant(p)) {
                return;
            }
            let now = Instant::now();
            pending
                .roots
                .lock()
                .unwrap()
                .entry(root_id)
                .and_modify(|e| e.1 = now)
                .or_insert((now, now));
            pending.wake.notify_one();
        };
        match notify::recommended_watcher(handler) {
            Ok(mut w) => match w.watch(path, RecursiveMode::Recursive) {
                Ok(()) => {
                    self.files.watch.watchers.lock().unwrap().insert(root_id, w);
                }
                Err(e) => tracing::warn!(path = %path.display(), error = %e, "cannot watch folder"),
            },
            Err(e) => tracing::warn!(error = %e, "cannot create a file watcher"),
        }
    }

    #[cfg(any(target_os = "android", target_os = "ios"))]
    pub(crate) fn watch_root(&self, _root_id: i64, _path: &Path) {}
}
