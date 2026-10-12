//! Catalog safety (docs/02 §8, PRD F10.8): `PRAGMA quick_check` after start-up, a daily
//! `VACUUM INTO` backup (the newest [`KEEP`] are kept in `<data dir>/backups/`) and restoring
//! from a backup, also when `catalog.db` cannot be opened at all.
//!
//! An unreadable catalog never stops the app: it is moved to `backups/corrupt-*.db` and an empty
//! catalog is opened in its place (state `recovery`), so the UI can offer the backups. A restore
//! swaps the file under the connection pool ([`Db::replace_file`]); the catalog it replaces is
//! kept as `backups/replaced-*.db`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use rusqlite::Connection;
use serde::Serialize;

use crate::analysis::types::RunState;
use crate::catalog::now_ms;
use crate::db::Db;
use crate::error::{CoreError, Result};
use crate::paths::DataDirs;
use crate::Core;

/// Automatic backups kept.
pub const KEEP: usize = 7;
/// Copies kept of replaced and of unreadable catalogs (each).
pub const KEEP_SPARE: usize = 3;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// The first automatic backup waits this long after start-up (start-up work goes first).
const FIRST_BACKUP_AFTER: Duration = Duration::from_secs(120);
/// How often the daily backup is looked at.
const BACKUP_CHECK_EVERY: Duration = Duration::from_secs(60 * 60);
/// How long a restore waits for catalog queries that are running.
const RESTORE_WAIT: Duration = Duration::from_secs(30);
/// `quick_check` findings kept.
const MAX_PROBLEMS: usize = 20;

/// File name prefixes in the backup directory.
const AUTO: &str = "catalog-";
const REPLACED: &str = "replaced-";
const CORRUPT: &str = "corrupt-";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogState {
    /// The start-up `quick_check` has not finished yet.
    Checking,
    Ok,
    /// `quick_check` found problems; the catalog still opens.
    Damaged,
    /// `catalog.db` could not be opened: it was moved aside and an empty catalog is in use.
    Recovery,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct BackupInfo {
    /// File name inside the backup directory.
    pub name: String,
    /// `auto` (daily or "back up now"), `replaced` (the catalog a restore replaced) or
    /// `corrupt` (an unreadable catalog moved aside at start-up).
    pub kind: String,
    /// ms since the epoch (file modification time).
    pub created_at: i64,
    pub bytes: u64,
}

/// `GET /api/catalog`.
#[derive(Debug, Clone, Serialize)]
pub struct CatalogStatus {
    pub state: CatalogState,
    /// `quick_check` findings, or the error that made the catalog unopenable.
    pub problems: Vec<String>,
    /// Where the unreadable catalog was moved to (`recovery`).
    pub moved_to: Option<String>,
    /// When the last integrity check finished (ms).
    pub checked_at: Option<i64>,
    /// The backup the catalog was restored from while the app runs.
    pub restored_from: Option<String>,
    /// Newest automatic backup (ms).
    pub last_backup_at: Option<i64>,
    pub backup_dir: String,
    /// Newest first.
    pub backups: Vec<BackupInfo>,
}

#[derive(Debug, Clone)]
struct Health {
    state: CatalogState,
    problems: Vec<String>,
    moved_to: Option<PathBuf>,
    checked_at: Option<i64>,
    restored_from: Option<String>,
}

/// Core-side state: the catalog's health and a lock that serialises backups and restores.
pub struct CatalogGuard {
    health: Mutex<Health>,
    busy: tokio::sync::Mutex<()>,
}

impl CatalogGuard {
    fn new(health: Health) -> Self {
        Self {
            health: Mutex::new(health),
            busy: tokio::sync::Mutex::new(()),
        }
    }

    pub fn state(&self) -> CatalogState {
        self.health.lock().unwrap().state
    }

    fn health(&self) -> Health {
        self.health.lock().unwrap().clone()
    }

    fn update(&self, f: impl FnOnce(&mut Health)) {
        f(&mut self.health.lock().unwrap())
    }
}

/// `YYYYMMDD-HHMMSS-mmm` (UTC) of a time in ms: file names that sort by time.
pub(crate) fn stamp(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, sod) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // civil_from_days (H. Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}-{:03}",
        sod / 3600,
        sod % 3600 / 60,
        sod % 60,
        ms.rem_euclid(1000)
    )
}

fn kind_of(name: &str) -> Option<&'static str> {
    if !name.ends_with(".db") {
        return None;
    }
    [(AUTO, "auto"), (REPLACED, "replaced"), (CORRUPT, "corrupt")]
        .into_iter()
        .find(|(p, _)| name.starts_with(p))
        .map(|(_, k)| k)
}

/// `path` + `suffix` (`catalog.db` -> `catalog.db-wal`).
fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// A new file name `<prefix><stamp>.db` in `dir`.
fn fresh_name(dir: &Path, prefix: &str) -> PathBuf {
    let mut ms = now_ms();
    loop {
        let p = dir.join(format!("{prefix}{}.db", stamp(ms)));
        if !p.exists() {
            return p;
        }
        ms += 1;
    }
}

fn info_of(p: &Path) -> Option<BackupInfo> {
    let name = p.file_name()?.to_string_lossy().into_owned();
    let kind = kind_of(&name)?;
    let md = std::fs::metadata(p).ok().filter(|m| m.is_file())?;
    let created_at = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Some(BackupInfo {
        name,
        kind: kind.to_string(),
        created_at,
        bytes: md.len(),
    })
}

/// Backups in `dir`, newest first.
pub fn list_backups(dir: &Path) -> Vec<BackupInfo> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<BackupInfo> = rd.flatten().filter_map(|e| info_of(&e.path())).collect();
    v.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.name.cmp(&a.name))
    });
    v
}

/// Deletes all but the newest `keep` files with `prefix` (the stamp in the name orders them).
fn prune(dir: &Path, prefix: &str, keep: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<String> = rd
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(prefix) && n.ends_with(".db"))
        .collect();
    names.sort_unstable();
    names.reverse();
    for n in names.into_iter().skip(keep) {
        let p = dir.join(&n);
        if let Err(e) = std::fs::remove_file(&p) {
            tracing::warn!(path = %p.display(), error = %e, "cannot delete an old catalog backup");
        }
        let _ = std::fs::remove_file(with_suffix(&p, "-wal"));
    }
}

/// Moves a catalog file (with its `-wal`; the `-shm` index is dropped) to `dest`. Nothing may
/// have it open.
pub(crate) fn move_catalog(from: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::rename(from, dest)?;
    let wal = with_suffix(from, "-wal");
    if wal.exists() {
        std::fs::rename(&wal, with_suffix(dest, "-wal"))?;
    }
    let _ = std::fs::remove_file(with_suffix(from, "-shm"));
    Ok(())
}

/// SQLite said the file is not a database or is malformed (as opposed to busy, missing
/// permissions, a newer schema...).
pub fn is_corruption(e: &CoreError) -> bool {
    let CoreError::Internal(e) = e else {
        return false;
    };
    e.chain().any(|c| {
        matches!(
            c.downcast_ref::<rusqlite::Error>(),
            Some(rusqlite::Error::SqliteFailure(f, _)) if matches!(
                f.code,
                rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase
            )
        )
    })
}

/// `PRAGMA quick_check`: the problems found (empty = ok).
fn quick_check(c: &Connection) -> Result<Vec<String>> {
    let mut st = c.prepare("PRAGMA quick_check")?;
    let rows = st
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.len() == 1 && rows[0] == "ok" {
        return Ok(Vec::new());
    }
    Ok(rows.into_iter().take(MAX_PROBLEMS).collect())
}

/// Reads a few rows of the main tables: catches an unreadable schema or table root right away
/// (the full `quick_check` runs in the background).
fn probe(db: &Db) -> Result<()> {
    db.with(|c| {
        c.query_row(
            "SELECT (SELECT COUNT(*) FROM session) + (SELECT COUNT(*) FROM task)
                    + (SELECT COUNT(*) FROM (SELECT id FROM photo LIMIT 1))",
            [],
            |r| r.get::<_, i64>(0),
        )?;
        Ok(())
    })
}

/// Opens `catalog.db`. When SQLite reports it as corrupt, it is moved to
/// `backups/corrupt-*.db` and an empty catalog takes its place (state `recovery`); any other
/// error is returned.
pub(crate) fn open_catalog(dirs: &DataDirs) -> Result<(Db, CatalogGuard)> {
    let first = Db::open(&dirs.catalog).and_then(|db| probe(&db).map(|()| db));
    match first {
        Ok(db) => Ok((
            db,
            CatalogGuard::new(Health {
                state: CatalogState::Checking,
                problems: Vec::new(),
                moved_to: None,
                checked_at: None,
                restored_from: None,
            }),
        )),
        Err(e) if is_corruption(&e) => {
            // the failed pool is gone by now, so the file can be moved (Windows)
            std::fs::create_dir_all(&dirs.backups)?;
            let dest = fresh_name(&dirs.backups, CORRUPT);
            tracing::error!(
                error = %e,
                moved_to = %dest.display(),
                "the catalog cannot be opened; starting with an empty one (restore a backup in the app)"
            );
            move_catalog(&dirs.catalog, &dest)?;
            prune(&dirs.backups, CORRUPT, KEEP_SPARE);
            let db = Db::open(&dirs.catalog)?;
            Ok((
                db,
                CatalogGuard::new(Health {
                    state: CatalogState::Recovery,
                    problems: vec![e.to_string()],
                    moved_to: Some(dest),
                    checked_at: Some(now_ms()),
                    restored_from: None,
                }),
            ))
        }
        Err(e) => Err(e),
    }
}

/// `VACUUM INTO` a new automatic backup (via a temp name, so a backup file is always complete),
/// then drops the oldest beyond [`KEEP`].
fn backup_into(c: &Connection, dir: &Path) -> Result<BackupInfo> {
    std::fs::create_dir_all(dir)?;
    let dest = fresh_name(dir, AUTO);
    let name = dest.file_name().unwrap_or_default().to_string_lossy();
    let tmp = dir.join(format!(".{name}.tmp"));
    let _ = std::fs::remove_file(&tmp);
    let res = c
        .execute("VACUUM INTO ?1", [tmp.to_string_lossy()])
        .map_err(CoreError::from)
        .and_then(|_| std::fs::rename(&tmp, &dest).map_err(CoreError::from));
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    prune(dir, AUTO, KEEP);
    info_of(&dest).ok_or_else(|| CoreError::Internal(anyhow::anyhow!("backup vanished")))
}

/// A backup is only restored when it is a readable catalog of this or an older schema.
fn verify_backup(p: &Path) -> Result<()> {
    let damaged = |why: String| {
        CoreError::Unprocessable(format!(
            "{} cannot be restored: {why}",
            p.file_name().unwrap_or_default().to_string_lossy()
        ))
    };
    let c = Connection::open(p).map_err(|e| damaged(e.to_string()))?;
    let version: i64 = c
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| damaged(e.to_string()))?;
    if version as usize > crate::db::MIGRATIONS.len() {
        return Err(damaged(format!(
            "it was written by a newer version (schema v{version})"
        )));
    }
    let problems = quick_check(&c).map_err(|e| damaged(e.to_string()))?;
    if !problems.is_empty() {
        return Err(damaged(format!("it is damaged ({})", problems.join("; "))));
    }
    let tables: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('photo','session','root_folder')",
            [],
            |r| r.get(0),
        )
        .map_err(|e| damaged(e.to_string()))?;
    if tables != 3 {
        return Err(damaged("it is not an imagePicker catalog".into()));
    }
    Ok(())
}

/// The backup file `name` refers to (a bare file name of a listed kind).
fn backup_path(dir: &Path, name: &str) -> Result<PathBuf> {
    let plain = !name.is_empty()
        && name.len() <= 128
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !name.starts_with('.');
    if !plain || kind_of(name).is_none() {
        return Err(CoreError::bad_request(format!(
            "{name:?} is not a backup name"
        )));
    }
    let p = dir.join(name);
    if !p.is_file() {
        return Err(CoreError::not_found(format!(
            "backup {name} does not exist"
        )));
    }
    Ok(p)
}

impl Core {
    /// `GET /api/catalog`: health of the catalog and the backups.
    pub async fn catalog_status(&self) -> CatalogStatus {
        let h = self.catalog_guard.health();
        let dir = self.dirs.backups.clone();
        let backups = tokio::task::spawn_blocking(move || list_backups(&dir))
            .await
            .unwrap_or_default();
        CatalogStatus {
            state: h.state,
            problems: h.problems,
            moved_to: h.moved_to.map(|p| p.to_string_lossy().into_owned()),
            checked_at: h.checked_at,
            restored_from: h.restored_from,
            last_backup_at: backups
                .iter()
                .filter(|b| b.kind == "auto")
                .map(|b| b.created_at)
                .max(),
            backup_dir: self.dirs.backups.to_string_lossy().into_owned(),
            backups,
        }
    }

    /// Runs `PRAGMA quick_check` and records the result (start-up runs it in the background).
    /// A catalog in `recovery` stays there until a backup is restored.
    pub async fn catalog_check(&self) -> Result<CatalogState> {
        let res = self.db.call(|c| quick_check(c)).await;
        let problems = match res {
            Ok(p) => p,
            Err(e) if is_corruption(&e) => vec![e.to_string()],
            Err(e) => return Err(e),
        };
        if !problems.is_empty() {
            tracing::error!(?problems, "catalog integrity check failed");
        }
        let mut state = CatalogState::Ok;
        self.catalog_guard.update(|h| {
            h.checked_at = Some(now_ms());
            if h.state != CatalogState::Recovery {
                h.state = if problems.is_empty() {
                    CatalogState::Ok
                } else {
                    CatalogState::Damaged
                };
                h.problems = problems;
            }
            state = h.state;
        });
        Ok(state)
    }

    /// `POST /api/catalog/backup`: a backup now (`VACUUM INTO`, the newest [`KEEP`] are kept).
    pub async fn catalog_backup(&self) -> Result<BackupInfo> {
        let _busy = self.catalog_guard.busy.lock().await;
        if self.catalog_guard.state() == CatalogState::Recovery {
            return Err(CoreError::Conflict(
                "the catalog could not be opened at start-up; restore a backup first (a backup of \
                 the empty replacement could push out a good one)"
                    .into(),
            ));
        }
        let dir = self.dirs.backups.clone();
        let info = self.db.call(move |c| backup_into(c, &dir)).await?;
        tracing::info!(name = %info.name, bytes = info.bytes, "catalog backed up");
        Ok(info)
    }

    /// The daily backup: when the newest automatic one is a day old, the catalog is healthy and
    /// not empty (an empty catalog must not push out good backups).
    pub(crate) async fn daily_backup(&self) {
        if self.catalog_guard.state() != CatalogState::Ok {
            return;
        }
        let dir = self.dirs.backups.clone();
        let newest = tokio::task::spawn_blocking(move || {
            list_backups(&dir)
                .into_iter()
                .filter(|b| b.kind == "auto")
                .map(|b| b.created_at)
                .max()
        })
        .await
        .ok()
        .flatten();
        if newest.is_some_and(|t| now_ms() - t < DAY_MS) {
            return;
        }
        let has_photos = self
            .db
            .call(|c| {
                Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM photo)", [], |r| {
                    r.get::<_, bool>(0)
                })?)
            })
            .await
            .unwrap_or(false);
        if !has_photos {
            return;
        }
        if let Err(e) = self.catalog_backup().await {
            tracing::warn!(error = %e, "daily catalog backup failed");
        }
    }

    /// `POST /api/catalog/restore`: replaces the catalog with backup `name` (kept as
    /// `backups/replaced-*.db`). Refused while an analysis runs.
    pub async fn catalog_restore(self: &Arc<Self>, name: &str) -> Result<CatalogStatus> {
        let src = backup_path(&self.dirs.backups, name)?;
        let _busy = self.catalog_guard.busy.lock().await;
        let running = self
            .runs
            .lock()
            .unwrap()
            .values()
            .any(|r| r.status.state == RunState::Running);
        if running {
            return Err(CoreError::Conflict(
                "an analysis is running; cancel it before restoring the catalog".into(),
            ));
        }
        let (db, dir) = (self.db.clone(), self.dirs.backups.clone());
        tokio::task::spawn_blocking(move || -> Result<()> {
            verify_backup(&src)?;
            let keep = fresh_name(&dir, REPLACED);
            db.replace_file(&src, &keep, RESTORE_WAIT)?;
            prune(&dir, REPLACED, KEEP_SPARE);
            Ok(())
        })
        .await
        .map_err(|e| CoreError::Internal(anyhow::anyhow!("restore task failed: {e}")))??;
        tracing::warn!(backup = name, "catalog restored from a backup");
        // in-memory state that referred to the old catalog
        self.runs.lock().unwrap().clear();
        self.xmp.pending.lock().unwrap().clear();
        let name = name.to_string();
        self.catalog_guard.update(|h| {
            *h = Health {
                state: CatalogState::Ok,
                problems: Vec::new(),
                moved_to: None,
                checked_at: Some(now_ms()),
                restored_from: Some(name),
            }
        });
        Ok(self.catalog_status().await)
    }

    /// Integrity check right after start-up, then the daily backup while the core lives.
    pub(crate) fn spawn_catalog_guard(self_: &Arc<Self>) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let weak = Arc::downgrade(self_);
        tokio::spawn(async move {
            if let Some(core) = weak.upgrade() {
                if let Err(e) = core.catalog_check().await {
                    tracing::warn!(error = %e, "catalog integrity check could not run");
                }
            }
            tokio::time::sleep(FIRST_BACKUP_AFTER).await;
            loop {
                let Some(core) = weak.upgrade() else { return };
                core.daily_backup().await;
                drop(core);
                tokio::time::sleep(BACKUP_CHECK_EVERY).await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_sort_and_read_as_utc() {
        assert_eq!(stamp(0), "19700101-000000-000");
        assert_eq!(stamp(1_760_000_000_123), "20251009-085320-123");
        assert_eq!(stamp(951_782_400_000), "20000229-000000-000");
        assert!(stamp(1_760_000_000_000) < stamp(1_760_000_000_001));
    }

    #[test]
    fn names_and_retention() {
        assert_eq!(kind_of("catalog-20251009-085320-123.db"), Some("auto"));
        assert_eq!(kind_of("replaced-20251009-085320-123.db"), Some("replaced"));
        assert_eq!(kind_of("corrupt-x.db"), Some("corrupt"));
        assert_eq!(kind_of("catalog.db"), None);
        assert_eq!(kind_of(".catalog-1.db.tmp"), None);
        let d = tempfile::tempdir().unwrap();
        for i in 0..10 {
            std::fs::write(d.path().join(format!("{AUTO}{}.db", stamp(1000 * i))), b"x").unwrap();
        }
        std::fs::write(d.path().join("replaced-1.db"), b"x").unwrap();
        prune(d.path(), AUTO, KEEP);
        let left = list_backups(d.path());
        assert_eq!(left.iter().filter(|b| b.kind == "auto").count(), KEEP);
        assert!(
            left.iter().any(|b| b.name == "replaced-1.db"),
            "other kinds untouched"
        );
        assert!(
            !d.path().join(format!("{AUTO}{}.db", stamp(0))).exists(),
            "oldest go first"
        );
        assert!(d.path().join(format!("{AUTO}{}.db", stamp(9000))).exists());
        for bad in [
            "../catalog.db",
            "catalog.db",
            "x.db",
            "",
            ".catalog-1.db",
            "catalog-1.db/..",
        ] {
            assert!(backup_path(d.path(), bad).is_err(), "{bad}");
        }
        assert!(matches!(
            backup_path(d.path(), "catalog-29990101-000000-000.db"),
            Err(CoreError::NotFound(_))
        ));
        assert!(backup_path(d.path(), "replaced-1.db").is_ok());
    }
}
