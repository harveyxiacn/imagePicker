//! M6 housekeeping (`docs/api-contract-m6.md` section D): cache accounting / clearing / LRU
//! eviction, model deletion, clearing face data and the onboarding state.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ip_worker_client::WorkerError;
use serde::Serialize;
use serde_json::{json, Value};

use crate::analysis::map_worker_err;
use crate::analysis::types::WorkerOut;
use crate::error::{CoreError, Result};
use crate::events::Event;
use crate::Core;

pub const CACHE_KINDS: [&str; 5] = ["thumbs", "previews", "masks", "edits", "gen"];
const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
/// How often the background task enforces `cache.max_gb`.
const EVICT_EVERY: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct CacheKinds {
    pub thumbs: u64,
    pub previews: u64,
    pub masks: u64,
    pub edits: u64,
    pub gen: u64,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct CacheInfo {
    /// Total size in bytes.
    pub bytes: u64,
    /// Number of cached files per kind.
    pub items: CacheKinds,
    /// Bytes per kind (not part of the contract).
    pub sizes: CacheKinds,
    /// The configured limit in bytes.
    pub max_bytes: u64,
}

struct Entry {
    path: PathBuf,
    size: u64,
    used: SystemTime,
}

fn walk(dir: &Path, out: &mut Vec<Entry>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(md) = e.metadata() else { continue };
        if md.is_dir() {
            walk(&p, out);
        } else {
            let used = [md.accessed().ok(), md.modified().ok()]
                .into_iter()
                .flatten()
                .max()
                .unwrap_or(SystemTime::UNIX_EPOCH);
            out.push(Entry {
                path: p,
                size: md.len(),
                used,
            });
        }
    }
}

fn remove_empty_dirs(dir: &Path, keep_root: bool) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        if e.path().is_dir() {
            remove_empty_dirs(&e.path(), false);
        }
    }
    if !keep_root {
        let _ = std::fs::remove_dir(dir);
    }
}

impl Core {
    fn cache_dirs(&self, kind: &str) -> Vec<PathBuf> {
        let d = &self.dirs;
        match kind {
            "thumbs" => vec![d.thumbs.clone()],
            "previews" => vec![d.previews.clone()],
            "masks" => vec![d.masks.clone(), d.beauty.clone()],
            // rendered thumbnails/previews of edited photos (never the saved patch assets)
            "edits" => vec![d.edited_thumbs.clone(), d.edited_previews.clone()],
            "gen" => vec![d.gen.clone()],
            _ => Vec::new(),
        }
    }

    fn scan_kind(&self, kind: &str) -> Vec<Entry> {
        let mut v = Vec::new();
        for d in self.cache_dirs(kind) {
            walk(&d, &mut v);
        }
        v
    }

    /// `GET /api/cache`.
    pub async fn cache_info(self: &Arc<Self>) -> Result<CacheInfo> {
        let core = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut info = CacheInfo::default();
            for kind in CACHE_KINDS {
                let entries = core.scan_kind(kind);
                let (n, bytes) = (entries.len() as u64, entries.iter().map(|e| e.size).sum());
                let (i, s) = match kind {
                    "thumbs" => (&mut info.items.thumbs, &mut info.sizes.thumbs),
                    "previews" => (&mut info.items.previews, &mut info.sizes.previews),
                    "masks" => (&mut info.items.masks, &mut info.sizes.masks),
                    "edits" => (&mut info.items.edits, &mut info.sizes.edits),
                    _ => (&mut info.items.gen, &mut info.sizes.gen),
                };
                *i = n;
                *s = bytes;
                info.bytes += bytes;
            }
            info.max_bytes = (core.settings().cache.max_gb * GIB) as u64;
            info
        })
        .await
        .map_err(|e| CoreError::Internal(anyhow::anyhow!("cache scan failed: {e}")))
    }

    /// `POST /api/cache/clear`: deletes cached files of the given kinds (`all` = every kind).
    /// Returns the bytes freed.
    pub async fn cache_clear(self: &Arc<Self>, kinds: Vec<String>) -> Result<u64> {
        let mut want: Vec<&'static str> = Vec::new();
        for k in &kinds {
            if k == "all" {
                want = CACHE_KINDS.to_vec();
                break;
            }
            match CACHE_KINDS.iter().find(|c| **c == k) {
                Some(c) => {
                    if !want.contains(c) {
                        want.push(c)
                    }
                }
                None => {
                    return Err(CoreError::bad_request(format!(
                        "unknown cache kind {k:?}; use {} or all",
                        CACHE_KINDS.join(", ")
                    )))
                }
            }
        }
        if want.is_empty() {
            return Err(CoreError::bad_request("kinds must not be empty"));
        }
        let core = self.clone();
        let want2 = want.clone();
        let freed = tokio::task::spawn_blocking(move || {
            let mut freed = 0u64;
            for kind in want2 {
                for e in core.scan_kind(kind) {
                    if std::fs::remove_file(&e.path).is_ok() {
                        freed += e.size;
                    }
                }
                for d in core.cache_dirs(kind) {
                    remove_empty_dirs(&d, true);
                }
            }
            freed
        })
        .await
        .map_err(|e| CoreError::Internal(anyhow::anyhow!("cache clear failed: {e}")))?;
        // the catalog must not claim that cleared files exist
        if want.contains(&"thumbs") {
            self.db
                .call(|c| {
                    c.execute("UPDATE photo SET thumb_state=0 WHERE thumb_state<>0", [])?;
                    Ok(())
                })
                .await?;
        }
        if want.contains(&"masks") {
            self.db
                .call(|c| {
                    c.execute("DELETE FROM beauty_geometry", [])?;
                    Ok(())
                })
                .await?;
        }
        Ok(freed)
    }

    /// Deletes least recently used cache files until the cache fits `cache.max_gb`. Returns the
    /// bytes freed. Scratch files (`gen`) go first, then rendered edits and previews, then
    /// thumbnails, masks last (they are the costliest to rebuild).
    pub fn evict_cache_blocking(&self) -> u64 {
        self.evict_cache_to((self.settings().cache.max_gb * GIB) as u64)
    }

    /// [`Core::evict_cache_blocking`] with an explicit budget in bytes.
    pub fn evict_cache_to(&self, budget: u64) -> u64 {
        let mut all: Vec<(u8, Entry)> = Vec::new();
        for (class, kinds) in [
            (0u8, &["gen"][..]),
            (1, &["previews", "edits"][..]),
            (2, &["thumbs"][..]),
            (3, &["masks"][..]),
        ] {
            for k in kinds {
                all.extend(self.scan_kind(k).into_iter().map(|e| (class, e)));
            }
        }
        let mut total: u64 = all.iter().map(|(_, e)| e.size).sum();
        if total <= budget {
            return 0;
        }
        all.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.used.cmp(&b.1.used)));
        let mut freed = 0;
        for (_, e) in all {
            if total <= budget {
                break;
            }
            if std::fs::remove_file(&e.path).is_ok() {
                total -= e.size;
                freed += e.size;
            }
        }
        freed
    }

    /// Runs one eviction pass in the background (no-op while one is running).
    pub(crate) fn spawn_cache_eviction(self: &Arc<Self>) {
        if tokio::runtime::Handle::try_current().is_err()
            || self.evicting.swap(true, Ordering::SeqCst)
        {
            return;
        }
        let core = self.clone();
        tokio::task::spawn_blocking(move || {
            let freed = core.evict_cache_blocking();
            if freed > 0 {
                tracing::info!(freed, "cache evicted");
            }
            core.evicting.store(false, Ordering::SeqCst);
        });
    }

    /// Enforces the cache limit now and then every few minutes while the core lives.
    pub(crate) fn spawn_cache_janitor(self_: &Arc<Self>) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let weak = Arc::downgrade(self_);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(EVICT_EVERY).await;
                let Some(core) = weak.upgrade() else { return };
                core.spawn_cache_eviction();
            }
        });
    }

    // ------------------------------------------------------------ models

    /// `DELETE /api/models/{id}`.
    pub async fn delete_model(&self, id: &str) -> Result<()> {
        let ok_id = !id.is_empty()
            && id.len() <= 128
            && !id.starts_with(['.', '_'])
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
        if !ok_id {
            return Err(CoreError::bad_request("invalid model id"));
        }
        match self.worker.models_delete(id).await {
            Ok(()) => {}
            Err(WorkerError::Unavailable(_)) => {
                // a worker without `models.delete` (or none running): remove the directory
                let dir = PathBuf::from(&self.settings().models.dir).join(id);
                if !dir.is_dir() {
                    return Err(CoreError::not_found(format!("model {id} is not installed")));
                }
                std::fs::remove_dir_all(&dir).map_err(|e| {
                    CoreError::Internal(anyhow::anyhow!("cannot delete {}: {e}", dir.display()))
                })?;
            }
            Err(WorkerError::Rpc {
                code: -32602,
                message,
                ..
            }) => return Err(CoreError::not_found(message)),
            Err(e) => return Err(map_worker_err(e)),
        }
        self.assistant.invalidate_info();
        Ok(())
    }

    // ------------------------------------------------------------ faces

    /// `DELETE /api/faces?confirm=true`: removes every detected face, person and the portrait
    /// geometry derived from them. Photos, ratings and edits are untouched.
    pub async fn clear_faces(&self) -> Result<Value> {
        let (faces, people, sessions) = self
            .db
            .call(|c| {
                let tx = c.transaction()?;
                let faces: i64 = tx.query_row("SELECT COUNT(*) FROM face", [], |r| r.get(0))?;
                let people: i64 = tx.query_row("SELECT COUNT(*) FROM person", [], |r| r.get(0))?;
                let mut sessions: Vec<i64> = Vec::new();
                {
                    let mut st = tx.prepare("SELECT id FROM session")?;
                    for r in st.query_map([], |r| r.get::<_, i64>(0))? {
                        sessions.push(r?);
                    }
                }
                tx.execute("DELETE FROM face", [])?;
                tx.execute("DELETE FROM person", [])?;
                tx.execute("DELETE FROM beauty_geometry", [])?;
                // issues that come from faces: closed eyes (1) and bystanders (32)
                tx.execute(
                    "UPDATE photo SET face_count=NULL, subject_face_count=NULL, issues = COALESCE(issues,0) & ~33",
                    [],
                )?;
                tx.commit()?;
                Ok((faces, people, sessions))
            })
            .await?;
        for s in sessions {
            self.events.emit(Event::PeopleUpdated { session_id: s });
        }
        Ok(json!({"faces": faces, "people": people}))
    }

    // ------------------------------------------------------------ onboarding

    fn onboarding_marker(&self) -> PathBuf {
        self.dirs.root.join("onboarding.done")
    }

    /// `GET /api/onboarding`.
    pub async fn onboarding(&self) -> Result<Value> {
        let first_run = !self.onboarding_marker().exists();
        let hw: WorkerOut =
            match tokio::time::timeout(Duration::from_secs(20), self.hardware(true)).await {
                Ok(Ok(h)) => h,
                _ => self.hardware(false).await?,
            };
        let tier = hw.tier.clone().unwrap_or_else(|| "T0".to_string());
        // what the standard analysis would still have to download
        let mut mb: Option<f64> = None;
        let mut missing: Vec<String> = Vec::new();
        if let Ok(Ok(l)) =
            tokio::time::timeout(Duration::from_secs(20), self.worker.models_list()).await
        {
            let wanted: Vec<String> = match l.profiles.get("standard") {
                Some(p) => p.models.clone(),
                None => l
                    .models
                    .iter()
                    .filter(|m| m.recommended && !m.optional)
                    .map(|m| m.id.clone())
                    .collect(),
            };
            let todo: Vec<&ip_worker_client::WorkerModel> = l
                .models
                .iter()
                .filter(|m| !m.installed && wanted.contains(&m.id))
                .collect();
            missing = todo.iter().map(|m| m.id.clone()).collect();
            mb = Some(todo.iter().map(|m| m.size_mb).sum());
        }
        Ok(json!({
            "first_run": first_run,
            "hardware": hw,
            "recommended_tier": tier,
            "recommended_download_mb": mb.map(|m| m.round() as i64),
            "recommended_models": missing,
        }))
    }

    /// `POST /api/onboarding/done`.
    pub async fn onboarding_done(&self) -> Result<()> {
        std::fs::write(self.onboarding_marker(), b"done")?;
        Ok(())
    }
}
