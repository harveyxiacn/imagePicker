//! Thumbnail scheduler: priority queue + worker pool for eager 256px grid thumbnails, and
//! deduplicated on-demand generation for every size, including the full-resolution image the
//! loupe shows at 100% and above.

use std::collections::{BinaryHeap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::SystemTime;

use anyhow::Context;
use ip_imaging::ThumbCache;
use tokio::sync::{Notify, OnceCell, Semaphore};

use crate::catalog::{self, PhotoRef};
use crate::db::Db;
use crate::error::{CoreError, Result};
use crate::events::{Event, EventBus};
use crate::imaging::Imaging;
use crate::model::ThumbItem;

pub const GRID_SIZE: u32 = 256;
pub const THUMB_SIZES: [u32; 2] = [256, 512];
pub const PREVIEW_SIZES: [u32; 3] = [1024, 2048, 4096];
const THUMB_QUALITY: u8 = 80;
const PREVIEW_QUALITY: u8 = 88;
/// Size key of the full-resolution image (`GET /api/full/{id}`): the photo's own pixel size.
pub const FULL_SIZE: u32 = 0;
/// Long edge asked of the decoder for it: the largest a JPEG can hold, so nothing is scaled down.
pub const FULL_EDGE: u32 = u16::MAX as u32;
/// JPEG quality of full-resolution images: they are looked at pixel by pixel.
pub const FULL_QUALITY: u8 = 92;
/// Bytes of full-resolution images kept on disk (about 80 photos of 24 MP); the least recently
/// used go first. The global `cache.max_gb` limit applies on top.
pub const FULL_CACHE_BUDGET: u64 = 1 << 30;
/// Full-resolution decodes at once: each holds the whole decoded image in memory.
const FULL_CONCURRENCY: usize = 2;

/// On-disk cache of full-resolution images, `<root>/<k[0..2]>/<k[2..4]>/<key>_full.jpg`, kept
/// within a byte budget by last use.
#[derive(Debug, Clone)]
pub struct FullCache {
    root: PathBuf,
    budget: u64,
}

impl FullCache {
    pub fn new(root: impl Into<PathBuf>, budget: u64) -> Self {
        Self {
            root: root.into(),
            budget,
        }
    }

    pub fn path_for(&self, key: &str) -> PathBuf {
        self.root
            .join(&key[0..2])
            .join(&key[2..4])
            .join(format!("{key}_full.jpg"))
    }

    /// Marks a cached image as used now (file systems often do not update access times).
    pub fn touch(path: &Path) {
        if let Ok(f) = std::fs::OpenOptions::new().write(true).open(path) {
            let _ = f.set_modified(SystemTime::now());
        }
    }

    /// Deletes the least recently used images until the cache fits its budget, never `keep`.
    /// Returns the bytes freed.
    pub fn trim(&self, keep: &Path) -> u64 {
        let mut files: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
        for l1 in read_dirs(&self.root) {
            for l2 in read_dirs(&l1) {
                let Ok(rd) = std::fs::read_dir(&l2) else {
                    continue;
                };
                for e in rd.flatten() {
                    let p = e.path();
                    if !p.to_string_lossy().ends_with("_full.jpg") {
                        continue;
                    }
                    if let Ok(md) = e.metadata() {
                        let used = md.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                        files.push((used, md.len(), p));
                    }
                }
            }
        }
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        if total <= self.budget {
            return 0;
        }
        files.sort_by_key(|f| f.0);
        let mut freed = 0;
        for (_, size, p) in files {
            if total <= self.budget {
                break;
            }
            if p != keep && std::fs::remove_file(&p).is_ok() {
                total -= size;
                freed += size;
            }
        }
        freed
    }
}

fn read_dirs(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default()
}

/// Counts outstanding jobs of one import so it can await completion.
pub struct Tracker {
    pending: AtomicUsize,
    notify: Notify,
}

impl Tracker {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            pending: AtomicUsize::new(0),
            notify: Notify::new(),
        })
    }
    fn add(&self, n: usize) {
        self.pending.fetch_add(n, Ordering::SeqCst);
    }
    fn done(&self) {
        if self.pending.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.notify.notify_one();
        }
    }
    pub fn pending(&self) -> usize {
        self.pending.load(Ordering::SeqCst)
    }
    /// Resolves when no job is outstanding.
    pub async fn wait_idle(&self) {
        loop {
            if self.pending() == 0 {
                return;
            }
            self.notify.notified().await;
        }
    }
}

#[derive(PartialEq, Eq)]
struct Item {
    id: i64,
    boosted: bool,
    seq: u64,
}

impl Ord for Item {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.boosted.cmp(&other.boosted).then_with(|| {
            if self.boosted {
                // newest viewport report first
                self.seq.cmp(&other.seq)
            } else {
                // import order
                other.seq.cmp(&self.seq)
            }
        })
    }
}
impl PartialOrd for Item {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Default)]
struct Queue {
    heap: BinaryHeap<Item>,
    /// Ids with a live job, and the trackers waiting for them.
    pending: HashMap<i64, Vec<Arc<Tracker>>>,
    seq: u64,
}

/// Why an image could not be generated.
#[derive(Debug, Clone)]
enum GenError {
    /// The original is there but cannot be decoded here (HTTP 415).
    Decode(String),
    Other(String),
}

impl From<GenError> for CoreError {
    fn from(e: GenError) -> Self {
        match e {
            GenError::Decode(m) => CoreError::Unsupported(m),
            GenError::Other(m) => CoreError::Internal(anyhow::anyhow!(m)),
        }
    }
}

type Cell = Arc<OnceCell<std::result::Result<PathBuf, GenError>>>;

struct Inner {
    db: Db,
    imaging: Arc<dyn Imaging>,
    events: EventBus,
    thumbs: ThumbCache,
    previews: ThumbCache,
    full: FullCache,
    full_gate: Semaphore,
    queue: Mutex<Queue>,
    cv: Condvar,
    shutdown: AtomicBool,
    inflight: Mutex<HashMap<(i64, u32), Cell>>,
    tmp_seq: AtomicU64,
}

pub struct Thumbs {
    inner: Arc<Inner>,
}

impl Drop for Thumbs {
    fn drop(&mut self) {
        self.inner.shutdown.store(true, Ordering::SeqCst);
        self.inner.cv.notify_all();
    }
}

fn quality_for(size: u32) -> u8 {
    if size == FULL_SIZE {
        FULL_QUALITY
    } else if size <= 512 {
        THUMB_QUALITY
    } else {
        PREVIEW_QUALITY
    }
}

impl Thumbs {
    pub fn new(
        db: Db,
        imaging: Arc<dyn Imaging>,
        events: EventBus,
        thumbs_dir: &Path,
        previews_dir: &Path,
        full: FullCache,
        workers: usize,
    ) -> Self {
        let inner = Arc::new(Inner {
            db,
            imaging,
            events,
            thumbs: ThumbCache::new(thumbs_dir),
            previews: ThumbCache::new(previews_dir),
            full,
            full_gate: Semaphore::new(FULL_CONCURRENCY),
            queue: Mutex::new(Queue::default()),
            cv: Condvar::new(),
            shutdown: AtomicBool::new(false),
            inflight: Mutex::new(HashMap::new()),
            tmp_seq: AtomicU64::new(0),
        });
        for n in 0..workers.max(1) {
            let w = inner.clone();
            std::thread::Builder::new()
                .name(format!("ip-thumb-{n}"))
                .spawn(move || w.worker_loop())
                .expect("spawn thumbnail worker");
        }
        Thumbs { inner }
    }

    /// Queue grid thumbnails. `boost` makes them jump ahead of normal (import-order) jobs.
    pub fn enqueue(&self, ids: &[i64], tracker: Option<&Arc<Tracker>>, boost: bool) {
        let mut q = self.inner.queue.lock().unwrap();
        // Iterate in reverse for boosted jobs so ids[0] gets the highest sequence number.
        let iter: Box<dyn Iterator<Item = &i64>> = if boost {
            Box::new(ids.iter().rev())
        } else {
            Box::new(ids.iter())
        };
        for &id in iter {
            q.seq += 1;
            let seq = q.seq;
            let known = q.pending.contains_key(&id);
            if known {
                if let Some(t) = tracker {
                    t.add(1);
                    q.pending.get_mut(&id).unwrap().push(t.clone());
                }
                if boost {
                    q.heap.push(Item {
                        id,
                        boosted: true,
                        seq,
                    });
                }
            } else {
                let mut trackers = Vec::new();
                if let Some(t) = tracker {
                    t.add(1);
                    trackers.push(t.clone());
                }
                q.pending.insert(id, trackers);
                q.heap.push(Item {
                    id,
                    boosted: boost,
                    seq,
                });
            }
        }
        drop(q);
        self.inner.cv.notify_all();
    }

    /// Photos the user is looking at: queue the ones lacking a grid thumbnail with top priority.
    pub async fn viewport(&self, ids: Vec<i64>) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let need = self
            .inner
            .db
            .call(move |c| catalog::ids_needing_grid(c, &ids))
            .await?;
        if !need.is_empty() {
            self.enqueue(&need, None, true);
        }
        Ok(())
    }

    pub fn queue_len(&self) -> usize {
        self.inner.queue.lock().unwrap().pending.len()
    }

    pub fn cache_path(&self, fast_key: &str, size: u32) -> PathBuf {
        self.inner.path_for(fast_key, size)
    }

    /// Returns the cached file, generating it first if needed. Concurrent calls for the same
    /// `(photo, size)` share one generation.
    pub async fn ensure(&self, id: i64, size: u32) -> Result<PathBuf> {
        let r = self
            .inner
            .db
            .call(move |c| catalog::photo_ref(c, id))
            .await?;
        self.ensure_ref(r, size).await
    }

    /// Like [`Thumbs::ensure`] for the *original* image of an already loaded row.
    /// `size` is a thumbnail or preview size, or [`FULL_SIZE`].
    pub async fn ensure_ref(&self, r: PhotoRef, size: u32) -> Result<PathBuf> {
        let inner = self.inner.clone();
        let id = r.id;
        let path = inner.path_for(&r.fast_key, size);
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            if size == FULL_SIZE {
                let p = path.clone();
                drop(tokio::task::spawn_blocking(move || FullCache::touch(&p)));
            }
            return Ok(path);
        }
        if !tokio::fs::try_exists(&r.path).await.unwrap_or(false) {
            return Err(CoreError::not_found(format!(
                "original file is missing: {}",
                r.path.display()
            )));
        }
        let key = (id, size);
        let cell: Cell = inner
            .inflight
            .lock()
            .unwrap()
            .entry(key)
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone();
        let res = cell
            .get_or_init(|| {
                let inner = inner.clone();
                async move {
                    // full-resolution decodes are big: only a few at once
                    let _permit = if size == FULL_SIZE {
                        Some(inner.full_gate.acquire().await.map_err(|_| {
                            GenError::Other("thumbnail service is shutting down".into())
                        })?)
                    } else {
                        None
                    };
                    let task = inner.clone();
                    tokio::task::spawn_blocking(move || task.generate_and_record(&r, size))
                        .await
                        .map_err(|e| GenError::Other(format!("thumbnail task failed: {e}")))?
                }
            })
            .await
            .clone();
        {
            let mut m = inner.inflight.lock().unwrap();
            if m.get(&key).is_some_and(|c| Arc::ptr_eq(c, &cell)) {
                m.remove(&key);
            }
        }
        res.map_err(CoreError::from)
    }

    /// Deletes cached images of the given photos (best effort).
    pub fn purge(&self, fast_keys: &[String]) {
        for k in fast_keys {
            if k.len() < 4 {
                continue;
            }
            for s in THUMB_SIZES {
                let _ = std::fs::remove_file(self.inner.thumbs.path_for(k, s));
            }
            for s in PREVIEW_SIZES {
                let _ = std::fs::remove_file(self.inner.previews.path_for(k, s));
            }
            let _ = std::fs::remove_file(self.inner.full.path_for(k));
        }
    }
}

impl Inner {
    fn cache_for(&self, size: u32) -> &ThumbCache {
        if size <= 512 {
            &self.thumbs
        } else {
            &self.previews
        }
    }

    fn path_for(&self, key: &str, size: u32) -> PathBuf {
        if size == FULL_SIZE {
            self.full.path_for(key)
        } else {
            self.cache_for(size).path_for(key, size)
        }
    }

    fn worker_loop(self: Arc<Self>) {
        loop {
            let (id, trackers) = {
                let mut q = self.queue.lock().unwrap();
                loop {
                    if self.shutdown.load(Ordering::SeqCst) {
                        return;
                    }
                    match q.heap.pop() {
                        Some(item) => {
                            // Stale duplicates (from boosts) have no pending entry any more.
                            if let Some(t) = q.pending.remove(&item.id) {
                                break (item.id, t);
                            }
                        }
                        None => q = self.cv.wait(q).unwrap(),
                    }
                }
            };
            if let Err(e) = self.run_grid(id) {
                tracing::warn!(photo = id, error = %format!("{e:#}"), "grid thumbnail failed");
            }
            for t in trackers {
                t.done();
            }
        }
    }

    fn run_grid(&self, id: i64) -> anyhow::Result<()> {
        let r = match self.db.with(|c| catalog::photo_ref(c, id)) {
            Ok(r) => r,
            Err(CoreError::NotFound(_)) => return Ok(()), // deleted meanwhile
            Err(e) => return Err(anyhow::anyhow!("{e}")),
        };
        let path = self.thumbs.path_for(&r.fast_key, GRID_SIZE);
        if path.exists() {
            self.record(&r, GRID_SIZE)?;
        } else {
            self.generate_and_record(&r, GRID_SIZE)
                .map_err(|e| match e {
                    GenError::Decode(m) | GenError::Other(m) => anyhow::anyhow!(m),
                })?;
        }
        Ok(())
    }

    fn generate_and_record(
        &self,
        r: &PhotoRef,
        size: u32,
    ) -> std::result::Result<PathBuf, GenError> {
        let path = self.path_for(&r.fast_key, size);
        let edge = if size == FULL_SIZE { FULL_EDGE } else { size };
        let img = self
            .imaging
            .generate_thumbnail(&r.path, r.format, r.orientation, edge, quality_for(size))
            .with_context(|| match size {
                FULL_SIZE => format!("decode {} at full resolution", r.path.display()),
                _ => format!("generate {size}px for {}", r.path.display()),
            })
            .map_err(|e| GenError::Decode(format!("{e:#}")))?;
        let other = |e: std::io::Error| GenError::Other(format!("cache write failed: {e}"));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(other)?;
        }
        let tmp = path.with_extension(format!(
            "tmp{}",
            self.tmp_seq.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&tmp, &img.bytes).map_err(other)?;
        if let Err(e) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            if !path.exists() {
                return Err(other(e));
            }
        }
        if size == FULL_SIZE {
            // not a thumbnail: no `thumb_state`; keep the cache within its budget instead
            let freed = self.full.trim(&path);
            if freed > 0 {
                tracing::debug!(freed, "full-resolution cache trimmed");
            }
        } else {
            self.record(r, size)
                .map_err(|e| GenError::Other(format!("{e:#}")))?;
        }
        Ok(path)
    }

    /// Persist `thumb_state` and announce grid thumbnails.
    fn record(&self, r: &PhotoRef, size: u32) -> anyhow::Result<()> {
        let state = if size >= 1024 { 3 } else { 2 };
        self.db
            .with(|c| catalog::mark_thumb_state(c, r.id, state))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if size == GRID_SIZE {
            self.events.emit(Event::ThumbsReady {
                items: vec![ThumbItem {
                    id: r.id,
                    v: r.thumb_version(),
                }],
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_jobs_jump_the_queue() {
        let mut h = BinaryHeap::new();
        for (id, seq) in [(1, 1), (2, 2), (3, 3)] {
            h.push(Item {
                id,
                boosted: false,
                seq,
            });
        }
        // two viewport reports; the newer one wins, then import order resumes
        h.push(Item {
            id: 3,
            boosted: true,
            seq: 4,
        });
        h.push(Item {
            id: 2,
            boosted: true,
            seq: 6,
        });
        h.push(Item {
            id: 1,
            boosted: true,
            seq: 5,
        });
        let order: Vec<i64> = std::iter::from_fn(|| h.pop().map(|i| i.id)).collect();
        assert_eq!(order, vec![2, 1, 3, 1, 2, 3]);
    }

    #[test]
    fn full_cache_trims_least_recently_used() {
        let dir = tempfile::tempdir().unwrap();
        let cache = FullCache::new(dir.path(), 250);
        let t0 = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let keys = ["aa11", "aa22", "bb33", "cc44"];
        let paths: Vec<PathBuf> = keys
            .iter()
            .enumerate()
            .map(|(i, k)| {
                let p = cache.path_for(k);
                assert!(p.ends_with(format!("{}/{}/{k}_full.jpg", &k[0..2], &k[2..4])));
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, [0u8; 100]).unwrap();
                let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
                f.set_modified(t0 + std::time::Duration::from_secs(i as u64 * 60))
                    .unwrap();
                p
            })
            .collect();
        // a stray temporary file is not an image of the cache
        std::fs::write(paths[0].with_extension("tmp7"), [0u8; 100]).unwrap();
        // the oldest one was just looked at again
        FullCache::touch(&paths[0]);
        assert_eq!(cache.trim(&paths[3]), 200);
        let left: Vec<bool> = paths.iter().map(|p| p.exists()).collect();
        assert_eq!(left, vec![true, false, false, true]);
        assert_eq!(cache.trim(&paths[3]), 0, "within the budget");
        // `keep` survives even when it alone is over the budget
        let tiny = FullCache::new(dir.path(), 10);
        assert_eq!(tiny.trim(&paths[3]), 100);
        assert!(paths[3].exists() && !paths[0].exists());
    }
}
