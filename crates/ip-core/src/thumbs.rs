//! Thumbnail scheduler: priority queue + worker pool for eager 256px grid thumbnails, and
//! deduplicated on-demand generation for every size.

use std::collections::{BinaryHeap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use anyhow::Context;
use ip_imaging::ThumbCache;
use tokio::sync::{Notify, OnceCell};

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

type Cell = Arc<OnceCell<std::result::Result<PathBuf, String>>>;

struct Inner {
    db: Db,
    imaging: Arc<dyn Imaging>,
    events: EventBus,
    thumbs: ThumbCache,
    previews: ThumbCache,
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
    if size <= 512 {
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
        workers: usize,
    ) -> Self {
        let inner = Arc::new(Inner {
            db,
            imaging,
            events,
            thumbs: ThumbCache::new(thumbs_dir),
            previews: ThumbCache::new(previews_dir),
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
        self.inner.cache_for(size).path_for(fast_key, size)
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
    pub async fn ensure_ref(&self, r: PhotoRef, size: u32) -> Result<PathBuf> {
        let inner = self.inner.clone();
        let id = r.id;
        let path = inner.cache_for(size).path_for(&r.fast_key, size);
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Ok(path);
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
                    tokio::task::spawn_blocking(move || inner.generate_and_record(&r, size))
                        .await
                        .map_err(|e| format!("thumbnail task failed: {e}"))?
                        .map_err(|e| format!("{e:#}"))
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
        res.map_err(|m| CoreError::Internal(anyhow::anyhow!(m)))
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
            self.generate_and_record(&r, GRID_SIZE)?;
        }
        Ok(())
    }

    fn generate_and_record(&self, r: &PhotoRef, size: u32) -> anyhow::Result<PathBuf> {
        let path = self.cache_for(size).path_for(&r.fast_key, size);
        let img = self
            .imaging
            .generate_thumbnail(&r.path, r.format, r.orientation, size, quality_for(size))
            .with_context(|| format!("generate {size}px for {}", r.path.display()))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension(format!(
            "tmp{}",
            self.tmp_seq.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&tmp, &img.bytes)?;
        if let Err(e) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            if !path.exists() {
                return Err(e.into());
            }
        }
        self.record(r, size)?;
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
}
