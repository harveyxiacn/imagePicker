//! SQLite access: tiny blocking connection pool + versioned migrations.
//!
//! Async code uses [`Db::call`] (runs on tokio's blocking pool); worker threads use [`Db::with`].

use std::path::{Path, PathBuf};
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

fn configure(conn: &Connection) -> Result<()> {
    conn.busy_timeout(Duration::from_secs(15))?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         PRAGMA foreign_keys=ON;
         PRAGMA mmap_size=268435456;",
    )?;
    Ok(())
}

struct Pool {
    conns: Mutex<Vec<Connection>>,
    cv: Condvar,
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

const POOL_SIZE: usize = 6;

impl Db {
    pub fn open(path: &Path) -> Result<Db> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut first = Connection::open(path)?;
        configure(&first)?;
        migrate(&mut first)?;
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
        f(guard.conn.as_mut().expect("connection present"))
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
