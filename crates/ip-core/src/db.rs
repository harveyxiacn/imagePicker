//! SQLite access: tiny blocking connection pool + versioned migrations.
//!
//! Async code uses [`Db::call`] (runs on tokio's blocking pool); worker threads use [`Db::with`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use rusqlite::Connection;

use crate::error::{CoreError, Result};

/// Ordered migrations; index + 1 == `PRAGMA user_version` after applying.
/// Never edit a released migration, only append.
pub const MIGRATIONS: &[&str] = &[
    // v1: M1 subset of docs/05 §1.1 (+ a few additive columns, see comments).
    r#"
CREATE TABLE root_folder (
  id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL,
  portable INTEGER DEFAULT 0, added_at INTEGER NOT NULL
);
CREATE TABLE device (
  id INTEGER PRIMARY KEY, make TEXT, model TEXT, serial TEXT,
  time_offset_ms INTEGER DEFAULT 0, offset_source TEXT
);
CREATE TABLE event (id INTEGER PRIMARY KEY, title TEXT, start_at INTEGER, end_at INTEGER, place TEXT);
CREATE TABLE scene (id INTEGER PRIMARY KEY, event_id INTEGER REFERENCES event(id), title TEXT, start_at INTEGER);
CREATE TABLE burst (id INTEGER PRIMARY KEY, scene_id INTEGER REFERENCES scene(id),
                    best_photo_id INTEGER, size INTEGER, manual INTEGER DEFAULT 0);
CREATE TABLE photo (
  id INTEGER PRIMARY KEY,
  root_id INTEGER NOT NULL REFERENCES root_folder(id),
  rel_path TEXT NOT NULL,
  file_name TEXT NOT NULL,                -- additive: basename, for sorting
  file_size INTEGER, mtime INTEGER,
  fast_key TEXT NOT NULL,
  content_key TEXT,
  format TEXT,
  pair_id INTEGER,
  width INTEGER, height INTEGER,          -- display size (orientation applied)
  orientation INTEGER DEFAULT 1,
  taken_at INTEGER,
  taken_at_raw INTEGER, device_id INTEGER REFERENCES device(id),
  camera TEXT, lens TEXT, focal REAL, aperture REAL, shutter REAL, iso INTEGER,
  gps_lat REAL, gps_lon REAL,
  burst_id INTEGER REFERENCES burst(id),
  rank_in_burst INTEGER,
  user_rating INTEGER CHECK(user_rating BETWEEN 0 AND 5),
  flag INTEGER DEFAULT 0,
  color_label TEXT,
  ai_score REAL, ai_rating REAL,
  issues INTEGER DEFAULT 0,
  scene_type TEXT,
  thumb_state INTEGER DEFAULT 0,
  analysis_version INTEGER DEFAULT 0,
  has_edits INTEGER DEFAULT 0,
  missing INTEGER DEFAULT 0,
  meta_done INTEGER NOT NULL DEFAULT 0,   -- additive: metadata phase finished
  UNIQUE(root_id, rel_path)
);
CREATE INDEX idx_photo_taken ON photo(taken_at);
CREATE INDEX idx_photo_burst ON photo(burst_id, rank_in_burst);
CREATE INDEX idx_photo_rating ON photo(user_rating, ai_rating);
CREATE TABLE session (
  id INTEGER PRIMARY KEY, title TEXT, cover_photo_id INTEGER, created_at INTEGER,
  root_id INTEGER REFERENCES root_folder(id),          -- additive: M1 session = one folder
  import_state TEXT NOT NULL DEFAULT 'ready'           -- additive
);
CREATE TABLE session_photo (
  session_id INTEGER NOT NULL REFERENCES session(id) ON DELETE CASCADE,
  photo_id INTEGER NOT NULL REFERENCES photo(id) ON DELETE CASCADE,
  PRIMARY KEY(session_id, photo_id)
);
CREATE TABLE task (
  id TEXT PRIMARY KEY, kind TEXT, status TEXT, priority INTEGER,
  params TEXT, progress REAL, error TEXT, created_at INTEGER, updated_at INTEGER
);
"#,
    // v2: lookup indexes.
    r#"
CREATE INDEX idx_session_photo_photo ON session_photo(photo_id);
CREATE INDEX idx_photo_name ON photo(file_name COLLATE NOCASE);
"#,
    // v3: capture-time UTC offset, so the UI can show the wall-clock time where the photo was taken.
    r#"
ALTER TABLE photo ADD COLUMN taken_at_offset_min INTEGER;
"#,
    // v4: M2 analysis (docs/05 §1.1 + additive columns, see docs/api-contract-m2.md §B).
    r#"
ALTER TABLE photo ADD COLUMN face_count INTEGER;
ALTER TABLE photo ADD COLUMN subject_face_count INTEGER;
ALTER TABLE burst ADD COLUMN session_id INTEGER;
ALTER TABLE burst ADD COLUMN start_at INTEGER;
ALTER TABLE burst ADD COLUMN end_at INTEGER;
ALTER TABLE scene ADD COLUMN session_id INTEGER;
ALTER TABLE scene ADD COLUMN end_at INTEGER;
CREATE INDEX idx_burst_session ON burst(session_id);
CREATE INDEX idx_scene_session ON scene(session_id);
CREATE TABLE analysis (
  photo_id INTEGER PRIMARY KEY REFERENCES photo(id) ON DELETE CASCADE,
  profile TEXT,                           -- additive: fast | standard
  steps_done INTEGER DEFAULT 0,
  model_versions TEXT,                    -- JSON {step: model id}
  phash BLOB,                             -- 8 bytes, big endian
  embedding BLOB,                         -- f16 little endian, L2-normalised
  sharpness REAL, exposure REAL, noise REAL, tilt_deg REAL,
  aesthetic REAL, iqa REAL, composition REAL,
  mean_luminance REAL, clipped_highlights REAL, crushed_shadows REAL,   -- additive: issue inputs
  scene_scores TEXT,                      -- additive: JSON
  explain TEXT,                           -- JSON {contributions, reasons}
  artifacts_dir TEXT,
  analyzed_at INTEGER
);
CREATE TABLE person (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT, cover_face_id INTEGER, hidden INTEGER NOT NULL DEFAULT 0, beauty_profile TEXT,
  center BLOB,                            -- additive: f16 mean identity embedding
  created_at INTEGER
);
CREATE TABLE face (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  photo_id INTEGER NOT NULL REFERENCES photo(id) ON DELETE CASCADE,
  idx INTEGER NOT NULL DEFAULT 0,         -- additive: position inside the photo's faces[]
  person_id INTEGER REFERENCES person(id) ON DELETE SET NULL,
  person_locked INTEGER NOT NULL DEFAULT 0,  -- additive: assigned by the user (must-link / cannot-link)
  track_id INTEGER,
  bbox_x REAL, bbox_y REAL, bbox_w REAL, bbox_h REAL,   -- normalised, display orientation
  det_score REAL, embedding BLOB,         -- 512d f16
  eyes_open REAL, smile REAL, gaze REAL, yaw REAL, pitch REAL, roll REAL,
  sharpness REAL, occlusion REAL, expression_score REAL,
  is_subject INTEGER NOT NULL DEFAULT 0, is_bystander INTEGER NOT NULL DEFAULT 0,
  blendshapes TEXT
);
CREATE INDEX idx_face_person ON face(person_id, photo_id);
CREATE INDEX idx_face_photo ON face(photo_id);
"#,
    // v5: M3 editing (docs/api-contract-m3.md). `edit_version` keeps the current stack plus a few
    // previous ones per photo; `photo.has_edits` already exists, `edit_hash` is its content hash
    // (part of `thumb_version`).
    r#"
ALTER TABLE photo ADD COLUMN edit_hash TEXT;
CREATE TABLE edit_version (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  photo_id INTEGER NOT NULL REFERENCES photo(id) ON DELETE CASCADE,
  name TEXT,
  stack TEXT NOT NULL,                    -- JSON edit stack (docs/05 section 2), stored as received
  hash TEXT NOT NULL,                     -- additive: content hash of the stack
  updated_at INTEGER,
  is_current INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX idx_edit_version_photo ON edit_version(photo_id, is_current, id);
CREATE TABLE preset (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL,
  stack TEXT NOT NULL,
  created_at INTEGER
);
"#,
    // v6: M4 (docs/api-contract-m4.md): portrait geometry cache, smart collections and the
    // personalised-scoring data (preference labels + the trained model's state).
    r#"
ALTER TABLE photo ADD COLUMN base_score REAL;  -- ai_score before personalisation
CREATE TABLE beauty_geometry (
  photo_id INTEGER PRIMARY KEY REFERENCES photo(id) ON DELETE CASCADE,
  content_key TEXT NOT NULL,              -- photo.content_key (fast_key when unknown) when prepared
  faces_sig TEXT NOT NULL,                -- fingerprint of the analysed faces the geometry was built for
  people INTEGER NOT NULL DEFAULT 0,
  prepared_at INTEGER NOT NULL
);
CREATE TABLE smart_collection (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL,
  query TEXT NOT NULL,                    -- URLSearchParams of GET /api/photos (no session_id/cursor/limit)
  created_at INTEGER
);
CREATE TABLE preference_label (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  photo_id INTEGER NOT NULL REFERENCES photo(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,                     -- rating | flag
  value REAL NOT NULL,                    -- rating 1..5, flag +1 (pick) / -1 (reject)
  source TEXT NOT NULL DEFAULT 'patch',   -- patch | accept_ai
  created_at INTEGER NOT NULL,
  UNIQUE(photo_id, kind)
);
CREATE TABLE taste_state (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  model TEXT,                             -- JSON: weights, normalisation, dimensions
  labels_at_train INTEGER NOT NULL DEFAULT 0,
  active INTEGER NOT NULL DEFAULT 0,
  alpha REAL NOT NULL DEFAULT 0,
  holdout_accuracy REAL,
  base_accuracy REAL,
  pairs INTEGER NOT NULL DEFAULT 0,
  traits TEXT,                            -- JSON [{key, params}]
  trained_at INTEGER
);
"#,
    // v7: M5 (docs/api-contract-m5.md section D): single-photo subject people, and a flag on the
    // portrait geometry cache for results that were built without some models.
    r#"
ALTER TABLE person ADD COLUMN singleton INTEGER NOT NULL DEFAULT 0;
ALTER TABLE beauty_geometry ADD COLUMN partial INTEGER NOT NULL DEFAULT 0;
"#,
    // v8: M6 (docs/api-contract-m6.md section C): keywords (XMP `dc:subject`) and what the
    // XMP sync last saw of each photo's sidecar (for conflict detection by mtime).
    r#"
CREATE TABLE photo_tag (
  photo_id INTEGER NOT NULL REFERENCES photo(id) ON DELETE CASCADE,
  tag TEXT NOT NULL,
  PRIMARY KEY(photo_id, tag)
);
CREATE INDEX idx_photo_tag_tag ON photo_tag(tag);
CREATE TABLE xmp_state (
  photo_id INTEGER PRIMARY KEY REFERENCES photo(id) ON DELETE CASCADE,
  sidecar_path TEXT,
  sidecar_mtime INTEGER NOT NULL DEFAULT 0,   -- ms, as of the last read/write by the app
  synced_at INTEGER
);
"#,
    // v9: device naming (docs/api-contract-m1.md "设备"): normalised display name and kind.
    // Existing rows are filled in by `device::backfill` when the catalog is opened.
    r#"
ALTER TABLE device ADD COLUMN name TEXT;
ALTER TABLE device ADD COLUMN kind TEXT;
CREATE INDEX idx_photo_device ON photo(device_id);
"#,
    // v10: horizon tilt (docs/03 §3.1): `analysis.tilt_deg` is now filled by the quality step of
    // every profile; this is its 0..1 confidence (the `tilted` issue needs both).
    r#"
ALTER TABLE analysis ADD COLUMN tilt_confidence REAL;
"#,
    // v11: task history (docs/api-contract-m5.md section F): progress counts and the end time of
    // a task; the list is read newest first.
    r#"
ALTER TABLE task ADD COLUMN done INTEGER;
ALTER TABLE task ADD COLUMN total INTEGER;
ALTER TABLE task ADD COLUMN finished_at INTEGER;
CREATE INDEX idx_task_created ON task(created_at);
"#,
];

/// Applies all pending migrations. Returns the resulting schema version.
pub fn migrate(conn: &mut Connection) -> Result<usize> {
    migrate_to(conn, MIGRATIONS.len())
}

/// Applies migrations up to `target` (used by tests to simulate old databases).
pub fn migrate_to(conn: &mut Connection, target: usize) -> Result<usize> {
    let current = conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? as usize;
    if current > MIGRATIONS.len() {
        return Err(CoreError::Internal(anyhow::anyhow!(
            "catalog schema v{current} is newer than this build (v{})",
            MIGRATIONS.len()
        )));
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().take(target).skip(current) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.execute_batch(&format!("PRAGMA user_version = {}", i + 1))?;
        tx.commit()?;
    }
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? as usize)
}

/// Memory-related pragmas: desktop maps up to 256 MiB; phones (flash storage, little RAM) use no
/// mmap, a small page cache and file-backed temp tables.
pub fn memory_pragmas(mobile: bool) -> &'static str {
    if mobile {
        "PRAGMA mmap_size=0;
         PRAGMA cache_size=-8192;
         PRAGMA temp_store=FILE;
         PRAGMA wal_autocheckpoint=500;"
    } else {
        "PRAGMA mmap_size=268435456;
         PRAGMA cache_size=-32768;
         PRAGMA temp_store=MEMORY;"
    }
}

fn configure(conn: &Connection) -> Result<()> {
    conn.busy_timeout(Duration::from_secs(15))?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         PRAGMA foreign_keys=ON;",
    )?;
    conn.execute_batch(memory_pragmas(cfg!(target_os = "android")))?;
    Ok(())
}

struct Pool {
    conns: Mutex<Vec<Connection>>,
    cv: Condvar,
    /// Bumped whenever a pooled connection changed any row (see [`Db::generation`]).
    generation: AtomicU64,
    /// `COUNT(*)` results of list queries, valid for one generation.
    counts: Mutex<(u64, HashMap<String, i64>)>,
}

#[derive(Clone)]
pub struct Db {
    pool: Arc<Pool>,
    path: PathBuf,
}

struct Guard<'a> {
    pool: &'a Pool,
    conn: Option<Connection>,
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        if let Some(c) = self.conn.take() {
            self.pool.conns.lock().unwrap().push(c);
            self.pool.cv.notify_one();
        }
    }
}

/// Connections per catalog (fewer on phones: each one holds its own page cache).
const POOL_SIZE: usize = if cfg!(target_os = "android") { 3 } else { 6 };

impl Db {
    pub fn open(path: &Path) -> Result<Db> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut first = Connection::open(path)?;
        configure(&first)?;
        migrate(&mut first)?;
        crate::device::backfill(&first)?;
        let mut conns = vec![first];
        for _ in 1..POOL_SIZE {
            let c = Connection::open(path)?;
            configure(&c)?;
            conns.push(c);
        }
        Ok(Db {
            pool: Arc::new(Pool {
                conns: Mutex::new(conns),
                cv: Condvar::new(),
                generation: AtomicU64::new(0),
                counts: Mutex::new((0, HashMap::new())),
            }),
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn acquire(&self) -> Guard<'_> {
        let mut g = self.pool.conns.lock().unwrap();
        loop {
            if let Some(c) = g.pop() {
                return Guard {
                    pool: &self.pool,
                    conn: Some(c),
                };
            }
            g = self.pool.cv.wait(g).unwrap();
        }
    }

    /// Blocking access; do not call from async context directly.
    pub fn with<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let mut guard = self.acquire();
        let conn = guard.conn.as_mut().expect("connection present");
        let before = conn.total_changes();
        let out = f(conn);
        if conn.total_changes() != before {
            self.pool.generation.fetch_add(1, Ordering::SeqCst);
        }
        out
    }

    /// Changes whenever any statement run through this pool inserted, updated or deleted rows.
    /// Derived data (e.g. list totals) computed under one generation is valid until it moves.
    pub fn generation(&self) -> u64 {
        self.pool.generation.load(Ordering::SeqCst)
    }

    pub fn cached_count(&self, generation: u64, key: &str) -> Option<i64> {
        let c = self.pool.counts.lock().unwrap();
        (c.0 == generation).then(|| c.1.get(key).copied()).flatten()
    }

    pub fn store_count(&self, generation: u64, key: String, n: i64) {
        let mut c = self.pool.counts.lock().unwrap();
        if c.0 != generation {
            if generation < c.0 {
                return;
            }
            *c = (generation, HashMap::new());
        }
        if c.1.len() >= 256 {
            c.1.clear();
        }
        c.1.insert(key, n);
    }

    /// Runs `f` on the blocking pool so the tokio runtime is never blocked.
    pub async fn call<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    {
        let db = self.clone();
        tokio::task::spawn_blocking(move || db.with(f))
            .await
            .map_err(|e| CoreError::Internal(anyhow::anyhow!("db task failed: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_apply_and_are_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.db");
        let db = Db::open(&path).unwrap();
        let v: i64 = db
            .with(|c| Ok(c.query_row("PRAGMA user_version", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(v as usize, MIGRATIONS.len());
        let mode: String = db
            .with(|c| Ok(c.query_row("PRAGMA journal_mode", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(mode, "wal");
        let fk: i64 = db
            .with(|c| Ok(c.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(fk, 1);
        drop(db);
        // reopen: nothing to do
        let db = Db::open(&path).unwrap();
        let n: i64 = db
            .with(|c| {
                Ok(c.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name IN ('photo','session','session_photo','root_folder','device','task')",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(n, 6);
    }

    #[test]
    fn upgrades_from_v1() {
        let mut conn = Connection::open_in_memory().unwrap();
        assert_eq!(migrate_to(&mut conn, 1).unwrap(), 1);
        assert_eq!(migrate(&mut conn).unwrap(), MIGRATIONS.len());
        let idx: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='idx_photo_name'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(idx, 1);
    }

    #[test]
    fn rejects_newer_schema() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA user_version = 99").unwrap();
        assert!(migrate(&mut conn).is_err());
    }
}
