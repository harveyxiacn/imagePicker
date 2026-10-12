//! Folders and files: RAW+JPEG pairing, missing files and "relocate folder", safe reject
//! (recycle bin or `_rejected`), and the per-session scan options file watching reuses.
//! See `docs/api-contract-m1.md` "RAW+JPEG 与文件".

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use rusqlite::{params, params_from_iter, Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::catalog;
use crate::error::{CoreError, Result};
use crate::events::Event;
use crate::Core;

/// Folder (below the directory of the photo) that receives rejected files in `folder` mode.
/// Scans skip it, so moved files do not come back as new photos.
pub const REJECTED_DIR: &str = "_rejected";

// ---------------------------------------------------------------- state

/// Where "move to the recycle bin" goes. Behind a trait so tests (and platforms without a
/// recycle bin) can substitute it.
pub trait Trasher: Send + Sync {
    fn trash(&self, path: &Path) -> anyhow::Result<()>;
}

/// The operating system's recycle bin (desktop builds only).
pub struct OsTrash;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
impl Trasher for OsTrash {
    fn trash(&self, path: &Path) -> anyhow::Result<()> {
        trash::delete(path).map_err(|e| anyhow::anyhow!("{e}"))
    }
}

#[cfg(any(target_os = "android", target_os = "ios"))]
impl Trasher for OsTrash {
    fn trash(&self, _path: &Path) -> anyhow::Result<()> {
        anyhow::bail!("this device has no recycle bin; use the _rejected folder instead")
    }
}

pub struct FilesState {
    pub(crate) trasher: RwLock<Arc<dyn Trasher>>,
    /// Rescans (import, file watching, relocate) of one catalog run one at a time.
    pub(crate) rescan_lock: tokio::sync::Mutex<()>,
    pub(crate) watch: crate::watch::WatchState,
}

impl Default for FilesState {
    fn default() -> Self {
        Self {
            trasher: RwLock::new(Arc::new(OsTrash)),
            rescan_lock: tokio::sync::Mutex::new(()),
            watch: Default::default(),
        }
    }
}

// ---------------------------------------------------------------- pairing

fn stem_key(rel: &str) -> String {
    let (dir, name) = match rel.rsplit_once('/') {
        Some((d, n)) => (d, n),
        None => ("", rel),
    };
    let stem = match name.rfind('.') {
        Some(i) if i > 0 => &name[..i],
        _ => name,
    };
    format!("{}/{}", dir.to_lowercase(), stem.to_lowercase())
}

/// JPEG wins over HEIF as the member that is shown.
fn primary_rank(format: &str) -> Option<u8> {
    match format {
        "jpeg" => Some(0),
        "heif" => Some(1),
        _ => None,
    }
}

/// The scanned RAW files that have a JPEG/HEIF with the same base name in the same folder.
pub fn companion_paths(root: &Path, files: &[ip_imaging::ScannedFile]) -> HashSet<PathBuf> {
    let key = |p: &Path| catalog::rel_path(root, p).map(|r| stem_key(&r));
    let primaries: HashSet<String> = files
        .iter()
        .filter(|f| {
            matches!(
                f.format,
                ip_imaging::ImageFormat::Jpeg | ip_imaging::ImageFormat::Heif
            )
        })
        .filter_map(|f| key(&f.path))
        .collect();
    files
        .iter()
        .filter(|f| f.format == ip_imaging::ImageFormat::Raw)
        .filter(|f| key(&f.path).is_some_and(|k| primaries.contains(&k)))
        .map(|f| f.path.clone())
        .collect()
}

/// Pairs every RAW file of the root with the JPEG/HEIF of the same folder and base name
/// (case-insensitive): the RAW row gets `pair_id = <the JPEG>` and leaves every session, so the
/// grid, groups and analysis see one photo. Idempotent. Returns the ids of all companions and
/// of those that were paired by this call.
pub fn pair_root(conn: &mut Connection, root_id: i64) -> Result<(Vec<i64>, Vec<i64>)> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let rows: Vec<(i64, String, String, Option<i64>)> = {
        let mut st = tx.prepare(
            "SELECT id, rel_path, COALESCE(format,''), pair_id FROM photo
             WHERE root_id=?1 AND format IN ('raw','jpeg','heif')",
        )?;
        let v = st
            .query_map([root_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        v
    };
    type Group = (Vec<(u8, i64)>, Vec<(i64, Option<i64>)>);
    let mut groups: HashMap<String, Group> = HashMap::new();
    for (id, rel, format, pair) in rows {
        let g = groups.entry(stem_key(&rel)).or_default();
        if format == "raw" {
            g.1.push((id, pair));
        } else if let Some(rank) = primary_rank(&format) {
            g.0.push((rank, id));
        }
    }
    let mut companions = Vec::new();
    let mut fresh = Vec::new();
    for (_, (primaries, raws)) in groups {
        let Some(&(_, primary)) = primaries.iter().min() else {
            continue;
        };
        for (raw, old) in raws {
            companions.push(raw);
            if old != Some(primary) {
                fresh.push(raw);
                let burst: Option<i64> =
                    tx.query_row("SELECT burst_id FROM photo WHERE id=?1", [raw], |r| {
                        r.get(0)
                    })?;
                tx.execute(
                    "UPDATE photo SET pair_id=?2, burst_id=NULL, rank_in_burst=NULL WHERE id=?1",
                    params![raw, primary],
                )?;
                if let Some(b) = burst {
                    tx.execute(
                        "UPDATE burst SET size=(SELECT COUNT(*) FROM photo WHERE burst_id=?1) WHERE id=?1",
                        [b],
                    )?;
                }
            }
            // a companion is not a grid item; no metadata or thumbnail work for it
            tx.execute("UPDATE photo SET meta_done=1 WHERE id=?1", [raw])?;
            tx.execute("DELETE FROM session_photo WHERE photo_id=?1", [raw])?;
            tx.execute(
                "UPDATE session SET cover_photo_id=?2 WHERE cover_photo_id=?1",
                params![raw, primary],
            )?;
        }
    }
    tx.commit()?;
    companions.sort_unstable();
    fresh.sort_unstable();
    Ok((companions, fresh))
}

/// Pairs RAW files in every root (catalogs that predate pairing, and every start).
pub fn pair_all(conn: &mut Connection) -> Result<usize> {
    let roots: Vec<i64> = {
        let mut st = conn.prepare("SELECT DISTINCT root_id FROM photo WHERE format='raw'")?;
        let v = st
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        v
    };
    let mut fresh = Vec::new();
    for root in roots {
        fresh.extend(pair_root(conn, root)?.1);
    }
    reconcile_pairs(conn, Some(&fresh))?;
    Ok(fresh.len())
}

/// Rating, flag and colour label of a pair are one decision: the companion mirrors the JPEG.
/// A JPEG that has none of them yet adopts the RAW's (a rating made in another tool on the RAW
/// before the pair was imported). `companions = None` looks at every companion.
pub fn reconcile_pairs(
    conn: &mut Connection,
    companions: Option<&[i64]>,
) -> Result<Vec<crate::model::PhotoUpdate>> {
    type Row = (
        i64,
        i64,
        Option<i64>,
        i64,
        Option<String>,
        Option<i64>,
        i64,
        Option<String>,
    );
    let rows: Vec<Row> = {
        let mut sql = "SELECT c.id, p.id, p.user_rating, COALESCE(p.flag,0), p.color_label,
                      c.user_rating, COALESCE(c.flag,0), c.color_label
               FROM photo c JOIN photo p ON p.id=c.pair_id"
            .to_string();
        if let Some(ids) = companions {
            if ids.is_empty() {
                return Ok(Vec::new());
            }
            sql.push_str(&format!(
                " WHERE c.id IN ({})",
                ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
            ));
        }
        let mut st = conn.prepare(&sql)?;
        let v = st
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        v
    };
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut changed = Vec::new();
    for (cid, pid, pr, pf, pl, cr, cf, cl) in rows {
        let (mut r, mut f, mut l) = (pr, pf, pl.clone());
        if pr.is_none() && pf == 0 && pl.is_none() && (cr.is_some() || cf != 0 || cl.is_some()) {
            (r, f, l) = (cr, cf, cl.clone());
            tx.execute(
                "UPDATE photo SET user_rating=?2, flag=?3, color_label=?4 WHERE id=?1",
                params![pid, r, f, l],
            )?;
            changed.push(crate::model::PhotoUpdate {
                id: pid,
                user_rating: r,
                flag: f,
                color_label: l.clone(),
            });
        }
        if (cr, cf, &cl) != (r, f, &l) {
            tx.execute(
                "UPDATE photo SET user_rating=?2, flag=?3, color_label=?4 WHERE id=?1",
                params![cid, r, f, l],
            )?;
        }
    }
    tx.commit()?;
    Ok(changed)
}

/// Ids of the companions of `ids`.
pub fn companion_ids(conn: &Connection, ids: &[i64]) -> Result<Vec<i64>> {
    let mut out = Vec::new();
    for chunk in ids.chunks(500) {
        let ph = vec!["?"; chunk.len()].join(",");
        let mut st = conn.prepare(&format!(
            "SELECT id FROM photo WHERE pair_id IN ({ph}) ORDER BY id"
        ))?;
        for id in st.query_map(params_from_iter(chunk.iter()), |r| r.get::<_, i64>(0))? {
            out.push(id?);
        }
    }
    Ok(out)
}

/// Replaces the ids by what an export should write for them (`jpeg`: the photos as they are,
/// `raw`: their companions, `both`: both). Unpaired photos are never dropped.
pub fn export_members(
    conn: &Connection,
    ids: &[i64],
    member: crate::model::PairMember,
) -> Result<Vec<i64>> {
    use crate::model::PairMember;
    if member == PairMember::Jpeg {
        return Ok(ids.to_vec());
    }
    let mut out = Vec::with_capacity(ids.len());
    for &id in ids {
        let comp = companion_ids(conn, &[id])?;
        match member {
            PairMember::Raw if !comp.is_empty() => out.extend(comp),
            PairMember::Both => {
                out.push(id);
                out.extend(comp);
            }
            _ => out.push(id),
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- scan options

pub fn set_scan_opts(
    conn: &Connection,
    session_id: i64,
    recursive: bool,
    exclude: &[String],
) -> Result<()> {
    let json = if exclude.is_empty() {
        None
    } else {
        Some(serde_json::to_string(exclude)?)
    };
    conn.execute(
        "UPDATE session SET recursive=?2, exclude=?3 WHERE id=?1",
        params![session_id, recursive, json],
    )?;
    Ok(())
}

pub struct ScanOpts {
    pub root_id: i64,
    pub root: String,
    pub recursive: bool,
    pub exclude: Vec<String>,
}

pub fn scan_opts(conn: &Connection, session_id: i64) -> Result<ScanOpts> {
    let row: (Option<i64>, Option<String>, i64, Option<String>) = conn
        .query_row(
            "SELECT s.root_id, r.path, s.recursive, s.exclude FROM session s
             LEFT JOIN root_folder r ON r.id=s.root_id WHERE s.id=?1",
            [session_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::not_found(format!("session {session_id} not found"))
            }
            e => e.into(),
        })?;
    let (Some(root_id), Some(root)) = (row.0, row.1) else {
        return Err(CoreError::Conflict(format!(
            "session {session_id} has no folder"
        )));
    };
    Ok(ScanOpts {
        root_id,
        root,
        recursive: row.2 != 0,
        exclude: row
            .3
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default(),
    })
}

// ---------------------------------------------------------------- missing files

pub struct FileRow {
    pub id: i64,
    pub path: PathBuf,
    pub size: i64,
    pub missing: bool,
}

/// The files of a session: its photos and their companions.
pub fn session_files(conn: &Connection, session_id: i64) -> Result<Vec<FileRow>> {
    let mut st = conn.prepare(
        "SELECT p.id, r.path, p.rel_path, COALESCE(p.file_size,0), COALESCE(p.missing,0)
         FROM photo p JOIN root_folder r ON r.id=p.root_id
         WHERE p.id IN (SELECT photo_id FROM session_photo WHERE session_id=?1)
            OR p.pair_id IN (SELECT photo_id FROM session_photo WHERE session_id=?1)",
    )?;
    let rows = st
        .query_map([session_id], |r| {
            let root: String = r.get(1)?;
            let rel: String = r.get(2)?;
            Ok(FileRow {
                id: r.get(0)?,
                path: catalog::join_rel(&root, &rel),
                size: r.get(3)?,
                missing: r.get::<_, i64>(4)? != 0,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn set_missing(conn: &mut Connection, updates: &[(i64, bool)]) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for (id, m) in updates {
        tx.execute("UPDATE photo SET missing=?2 WHERE id=?1", params![id, m])?;
    }
    tx.commit()?;
    Ok(())
}

// ---------------------------------------------------------------- relocate

#[derive(Debug, Clone, Deserialize)]
pub struct RelocateRequest {
    pub path: String,
    /// Only report what would match.
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct RelocateReport {
    /// Files of the folder's photos looked at.
    pub total: usize,
    /// Present at the same relative path with the same size.
    pub matched: usize,
    /// Present at the same relative path, but with another size (edited elsewhere).
    pub changed: usize,
    /// Not found below the new folder.
    pub absent: usize,
    pub applied: bool,
}

// ---------------------------------------------------------------- safe reject

#[derive(Debug, Clone, Deserialize)]
pub struct RejectFilesRequest {
    pub session_id: i64,
    /// Only these photos (they must be rejected); default: every rejected photo of the session.
    #[serde(default)]
    pub ids: Option<Vec<i64>>,
    /// `trash` (the recycle bin) or `folder` (a `_rejected` folder next to each file).
    pub mode: String,
    /// The client asked the user first. Without it nothing is moved.
    #[serde(default)]
    pub confirm: bool,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct RejectFilesReport {
    pub mode: String,
    /// Photos handled (each with its RAW companions).
    pub photos: usize,
    /// Files moved (originals, companions and sidecars not counted separately).
    pub files: usize,
    /// Rejected photos whose file was already gone.
    pub skipped: usize,
    pub errors: Vec<String>,
}

struct RejectFile {
    id: i64,
    root_rel: String,
    path: PathBuf,
}

fn unique_target(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (1..)
        .map(|n| dir.join(format!("{stem}-{n}{ext}")))
        .find(|p| !p.exists())
        .expect("unbounded")
}

/// Existing sidecars of `file` with the name they get next to `new_file`.
fn sidecar_moves(file: &Path, new_file: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut out = Vec::new();
    let [stem_sc, full_sc] = crate::xmp::sidecar_candidates(file);
    let [new_stem_sc, new_full_sc] = crate::xmp::sidecar_candidates(new_file);
    if stem_sc.is_file() {
        out.push((stem_sc, new_stem_sc));
    }
    if full_sc.is_file() {
        out.push((full_sc, new_full_sc));
    }
    out
}

impl Core {
    /// Replaces the recycle-bin implementation (tests).
    pub fn set_trasher(&self, t: Arc<dyn Trasher>) {
        *self.files.trasher.write().unwrap() = t;
    }

    /// Pairs RAW files of a root. Returns all companion ids and the newly paired ones.
    pub(crate) async fn pair_files(self: &Arc<Self>, root_id: i64) -> Result<(Vec<i64>, Vec<i64>)> {
        self.db.call(move |c| pair_root(c, root_id)).await
    }

    /// After the XMP of the companions was read: mirror rating/flag/label onto them.
    pub(crate) async fn reconcile_companions(self: &Arc<Self>, comps: Vec<i64>) {
        if comps.is_empty() {
            return;
        }
        match self
            .db
            .call(move |c| reconcile_pairs(c, Some(&comps)))
            .await
        {
            Ok(updates) if !updates.is_empty() => {
                self.xmp_touch(&updates.iter().map(|u| u.id).collect::<Vec<_>>());
                self.events.emit(Event::PhotosUpdated { items: updates });
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "pairing: reconciling rating failed"),
        }
    }

    /// Checks which files of the session exist and updates the `missing` flags. Returns how
    /// many photos changed state.
    pub async fn refresh_missing(self: &Arc<Self>, session_id: i64) -> Result<usize> {
        let files = self.db.call(move |c| session_files(c, session_id)).await?;
        let updates = tokio::task::spawn_blocking(move || {
            files
                .into_iter()
                .filter_map(|f| {
                    let gone = !f.path.is_file();
                    (gone != f.missing).then_some((f.id, gone))
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| CoreError::Internal(anyhow::anyhow!("missing check failed: {e}")))?;
        if updates.is_empty() {
            return Ok(0);
        }
        let n = updates.len();
        self.db.call(move |c| set_missing(c, &updates)).await?;
        self.events.emit(Event::PhotosAdded {
            session_id,
            count: n as i64,
        });
        self.emit_session(session_id).await;
        Ok(n)
    }

    /// Looks at the folder again: new files are imported, vanished ones are flagged missing,
    /// reappeared ones are cleared. Used by file watching and by the "rescan" endpoint.
    pub async fn rescan_session(self: &Arc<Self>, session_id: i64) -> Result<()> {
        let _guard = self.files.rescan_lock.lock().await;
        let opts = self.db.call(move |c| scan_opts(c, session_id)).await?;
        let root = PathBuf::from(&opts.root);
        if root.is_dir() {
            self.run_import(
                root,
                opts.root_id,
                session_id,
                opts.recursive,
                opts.exclude,
                true,
            )
            .await
            .map_err(CoreError::Internal)?;
        }
        self.refresh_missing(session_id).await?;
        Ok(())
    }

    /// Rescans every session of a root.
    pub(crate) async fn rescan_root(self: &Arc<Self>, root_id: i64) {
        let sessions: Vec<i64> = match self
            .db
            .call(move |c| {
                let mut st = c.prepare("SELECT id FROM session WHERE root_id=?1")?;
                let v = st
                    .query_map([root_id], |r| r.get(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(v)
            })
            .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "rescan: listing sessions failed");
                return;
            }
        };
        for sid in sessions {
            if let Err(e) = self.rescan_session(sid).await {
                tracing::warn!(session = sid, error = %e, "rescan failed");
            }
        }
    }

    /// `POST /api/sessions/{id}/relocate`: the folder of the session was moved or renamed.
    /// Files are re-linked by relative path (and size); the catalog keeps ratings, edits and
    /// analysis. Everything that shares the folder follows.
    pub async fn relocate_session(
        self: &Arc<Self>,
        session_id: i64,
        req: RelocateRequest,
    ) -> Result<RelocateReport> {
        let new_root = crate::import::normalize_path(Path::new(&req.path))
            .map_err(|e| CoreError::bad_request(format!("invalid path: {e}")))?;
        if !new_root.is_dir() {
            return Err(CoreError::bad_request(format!(
                "not a directory: {}",
                new_root.display()
            )));
        }
        let new_str = new_root.to_string_lossy().into_owned();
        let opts = self.db.call(move |c| scan_opts(c, session_id)).await?;
        let root_id = opts.root_id;
        let ns = new_str.clone();
        let (rows, clash) = self
            .db
            .call(move |c| {
                let mut st = c.prepare(
                    "SELECT rel_path, COALESCE(file_size,0) FROM photo WHERE root_id=?1",
                )?;
                let rows = st
                    .query_map([root_id], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let clash: Option<i64> = c
                    .query_row(
                        "SELECT id FROM root_folder WHERE path=?1 AND id<>?2",
                        params![ns, root_id],
                        |r| r.get(0),
                    )
                    .ok();
                Ok((rows, clash))
            })
            .await?;
        if clash.is_some() {
            return Err(CoreError::Conflict(
                "that folder is already imported as another folder; remove that import first"
                    .into(),
            ));
        }
        let nr = new_root.clone();
        let mut report = tokio::task::spawn_blocking(move || {
            let mut rep = RelocateReport {
                total: rows.len(),
                ..Default::default()
            };
            for (rel, size) in rows {
                match std::fs::metadata(catalog::join_rel(&nr.to_string_lossy(), &rel)) {
                    Ok(md) if md.is_file() && md.len() as i64 == size => rep.matched += 1,
                    Ok(md) if md.is_file() => rep.changed += 1,
                    _ => rep.absent += 1,
                }
            }
            rep
        })
        .await
        .map_err(|e| CoreError::Internal(anyhow::anyhow!("relocate check failed: {e}")))?;
        if report.matched == 0 {
            return Err(CoreError::Unprocessable(
                "none of the photos was found at the same relative path with the same size in that folder"
                    .into(),
            ));
        }
        if req.dry_run {
            return Ok(report);
        }
        let ns = new_str.clone();
        let sessions: Vec<i64> = self
            .db
            .call(move |c| {
                c.execute(
                    "UPDATE root_folder SET path=?2 WHERE id=?1",
                    params![root_id, ns],
                )?;
                let mut st = c.prepare("SELECT id FROM session WHERE root_id=?1")?;
                let v = st
                    .query_map([root_id], |r| r.get(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(v)
            })
            .await?;
        report.applied = true;
        self.watch_root(root_id, &new_root);
        for sid in sessions {
            // flags first (cheap), then pick up changed and new files
            self.refresh_missing(sid).await?;
            self.emit_session(sid).await;
            let core = self.clone();
            tokio::spawn(async move {
                if let Err(e) = core.rescan_session(sid).await {
                    tracing::warn!(session = sid, error = %e, "rescan after relocate failed");
                }
            });
        }
        Ok(report)
    }

    /// Moves the rejected photos of a session (with their RAW companions and XMP sidecars) to
    /// the recycle bin or into `_rejected` folders. Never deletes. The catalog follows: moved
    /// files get their new path, trashed ones show up as missing (restoring them from the bin
    /// and rescanning brings them back with their ratings).
    pub async fn reject_files(
        self: &Arc<Self>,
        req: RejectFilesRequest,
    ) -> Result<RejectFilesReport> {
        if !req.confirm {
            return Err(CoreError::bad_request(
                "confirmation required: ask the user, then send confirm=true",
            ));
        }
        let to_trash = match req.mode.as_str() {
            "trash" => true,
            "folder" => false,
            _ => return Err(CoreError::bad_request("mode must be trash or folder")),
        };
        let (sid, only) = (req.session_id, req.ids.clone());
        // (photo id, its files: the photo first, then companions)
        let groups: Vec<Vec<RejectFile>> = self
            .db
            .call(move |c| {
                catalog::get_session(c, sid)?;
                let mut st = c.prepare(
                    "SELECT p.id FROM session_photo sp JOIN photo p ON p.id=sp.photo_id
                     WHERE sp.session_id=?1 AND p.flag=-1 ORDER BY p.id",
                )?;
                let ids = st
                    .query_map([sid], |r| r.get::<_, i64>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                drop(st);
                let only: Option<HashSet<i64>> = only.map(|v| v.into_iter().collect());
                let mut out = Vec::new();
                for id in ids.into_iter().filter(|i| only.as_ref().is_none_or(|o| o.contains(i))) {
                    let mut files = Vec::new();
                    let mut st = c.prepare(
                        "SELECT p.id, r.path, p.rel_path FROM photo p JOIN root_folder r ON r.id=p.root_id
                         WHERE p.id=?1 OR p.pair_id=?1 ORDER BY (p.id<>?1), p.id",
                    )?;
                    for row in st.query_map([id], |r| {
                        let root: String = r.get(1)?;
                        let rel: String = r.get(2)?;
                        Ok(RejectFile {
                            id: r.get(0)?,
                            path: catalog::join_rel(&root, &rel),
                            root_rel: rel,
                        })
                    })? {
                        files.push(row?);
                    }
                    out.push(files);
                }
                Ok(out)
            })
            .await?;
        let trasher = self.files.trasher.read().unwrap().clone();
        let mode = req.mode.clone();
        // (id, new rel path + name when moved, trashed)
        type Moved = (i64, Option<(String, String)>, bool);
        let (moved, mut report) = tokio::task::spawn_blocking(move || {
            let mut report = RejectFilesReport {
                mode,
                ..Default::default()
            };
            let mut moved: Vec<Moved> = Vec::new();
            for files in groups {
                if !files.iter().any(|f| f.path.is_file()) {
                    report.skipped += 1;
                    continue;
                }
                let mut ok = true;
                for f in files {
                    if !f.path.is_file() {
                        continue;
                    }
                    let (Some(dir), Some(name)) = (
                        f.path.parent(),
                        f.path.file_name().map(|n| n.to_string_lossy().into_owned()),
                    ) else {
                        continue;
                    };
                    if to_trash {
                        let sidecars: Vec<PathBuf> = crate::xmp::sidecar_candidates(&f.path)
                            .into_iter()
                            .filter(|p| p.is_file())
                            .collect();
                        match trasher.trash(&f.path) {
                            Ok(()) => {
                                report.files += 1;
                                moved.push((f.id, None, true));
                                for sc in sidecars {
                                    if let Err(e) = trasher.trash(&sc) {
                                        report.errors.push(format!("{}: {e:#}", sc.display()));
                                    }
                                }
                            }
                            Err(e) => {
                                ok = false;
                                report.errors.push(format!("{}: {e:#}", f.path.display()));
                            }
                        }
                    } else {
                        let rdir = dir.join(REJECTED_DIR);
                        if let Err(e) = std::fs::create_dir_all(&rdir) {
                            ok = false;
                            report.errors.push(format!("{}: {e}", rdir.display()));
                            continue;
                        }
                        let target = unique_target(&rdir, &name);
                        match std::fs::rename(&f.path, &target) {
                            Ok(()) => {
                                report.files += 1;
                                for (from, to) in sidecar_moves(&f.path, &target) {
                                    if to.exists() {
                                        continue;
                                    }
                                    if let Err(e) = std::fs::rename(&from, &to) {
                                        report.errors.push(format!("{}: {e}", from.display()));
                                    }
                                }
                                let new_name = target
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or(name.clone());
                                let rel = match f.root_rel.rsplit_once('/') {
                                    Some((d, _)) => format!("{d}/{REJECTED_DIR}/{new_name}"),
                                    None => format!("{REJECTED_DIR}/{new_name}"),
                                };
                                moved.push((f.id, Some((rel, new_name)), false));
                            }
                            Err(e) => {
                                ok = false;
                                report.errors.push(format!("{}: {e}", f.path.display()));
                            }
                        }
                    }
                }
                if ok {
                    report.photos += 1;
                }
            }
            (moved, report)
        })
        .await
        .map_err(|e| CoreError::Internal(anyhow::anyhow!("reject task failed: {e}")))?;
        if !moved.is_empty() {
            let applied = self
                .db
                .call(move |c| {
                    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
                    for (id, new, trashed) in &moved {
                        if let Some((rel, name)) = new {
                            tx.execute(
                                "UPDATE photo SET rel_path=?2, file_name=?3 WHERE id=?1",
                                params![id, rel, name],
                            )?;
                            tx.execute("DELETE FROM xmp_state WHERE photo_id=?1", [id])?;
                        } else if *trashed {
                            tx.execute("UPDATE photo SET missing=1 WHERE id=?1", [id])?;
                        }
                    }
                    tx.commit()?;
                    Ok(())
                })
                .await;
            if let Err(e) = applied {
                report.errors.push(format!(
                    "files were moved but the catalog update failed: {e}"
                ));
            }
            self.events.emit(Event::PhotosAdded {
                session_id: sid,
                count: report.files as i64,
            });
            self.emit_session(sid).await;
        }
        Ok(report)
    }
}
