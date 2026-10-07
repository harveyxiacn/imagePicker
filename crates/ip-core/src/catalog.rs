//! Synchronous catalog queries. All functions take a `&Connection` / `&mut Connection`
//! and are run through [`crate::db::Db`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ip_imaging::{ImageFormat, Metadata};
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, Row, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::model::*;

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn format_from_str(s: &str) -> Option<ImageFormat> {
    Some(match s {
        "jpeg" => ImageFormat::Jpeg,
        "png" => ImageFormat::Png,
        "webp" => ImageFormat::Webp,
        "heif" => ImageFormat::Heif,
        "avif" => ImageFormat::Avif,
        "tiff" => ImageFormat::Tiff,
        "raw" => ImageFormat::Raw,
        _ => return None,
    })
}

/// `root` + `/`-separated relative path, with native separators.
pub fn join_rel(root: &str, rel: &str) -> PathBuf {
    let mut p = PathBuf::from(root);
    for part in rel.split('/').filter(|s| !s.is_empty()) {
        p.push(part);
    }
    p
}

pub fn thumb_version(fast_key: &str) -> String {
    fast_key.chars().take(8).collect()
}

/// `thumb_version` including the edit-stack hash, so cached URLs change with every edit.
pub fn thumb_version_for(fast_key: &str, edit_hash: Option<&str>) -> String {
    match edit_hash {
        None => thumb_version(fast_key),
        Some(h) => crate::edit::edited_key(fast_key, h)
            .chars()
            .take(8)
            .collect(),
    }
}

// ---------------------------------------------------------------- roots & sessions

pub fn upsert_root(conn: &Connection, path: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO root_folder(path, added_at) VALUES(?1, ?2) ON CONFLICT(path) DO NOTHING",
        params![path, now_ms()],
    )?;
    Ok(
        conn.query_row("SELECT id FROM root_folder WHERE path=?1", [path], |r| {
            r.get(0)
        })?,
    )
}

pub fn create_session(conn: &Connection, root_id: i64, title: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO session(title, created_at, root_id, import_state) VALUES(?1, ?2, ?3, 'scanning')",
        params![title, now_ms(), root_id],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn set_import_state(conn: &Connection, session_id: i64, state: ImportState) -> Result<()> {
    conn.execute(
        "UPDATE session SET import_state=?1 WHERE id=?2",
        params![state.as_str(), session_id],
    )?;
    Ok(())
}

/// Sessions left in a transient state by a previous run can never finish; mark them ready.
pub fn settle_stale_sessions(conn: &Connection) -> Result<()> {
    conn.execute(
        "UPDATE session SET import_state='ready' WHERE import_state<>'ready'",
        [],
    )?;
    Ok(())
}

const SESSION_SELECT: &str =
    "SELECT s.id, COALESCE(s.title,''), COALESCE(r.path,''), COALESCE(s.created_at,0),
        s.cover_photo_id, s.import_state,
        COUNT(p.id), COALESCE(SUM(p.flag=1),0), COALESCE(SUM(p.flag=-1),0),
        COALESCE(SUM(p.user_rating IS NOT NULL),0)
 FROM session s
 LEFT JOIN root_folder r ON r.id=s.root_id
 LEFT JOIN session_photo sp ON sp.session_id=s.id
 LEFT JOIN photo p ON p.id=sp.photo_id";

fn session_row(r: &Row) -> rusqlite::Result<Session> {
    Ok(Session {
        id: r.get(0)?,
        title: r.get(1)?,
        root_path: r.get(2)?,
        created_at: r.get(3)?,
        cover_photo_id: r.get(4)?,
        import_state: ImportState::parse(&r.get::<_, String>(5)?),
        photo_count: r.get(6)?,
        picked_count: r.get(7)?,
        rejected_count: r.get(8)?,
        rated_count: r.get(9)?,
    })
}

pub fn get_session(conn: &Connection, id: i64) -> Result<Session> {
    let sql = format!("{SESSION_SELECT} WHERE s.id=?1 GROUP BY s.id");
    conn.query_row(&sql, [id], session_row)
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::not_found(format!("session {id} not found"))
            }
            e => e.into(),
        })
}

pub fn list_sessions(conn: &Connection) -> Result<Vec<Session>> {
    let sql = format!("{SESSION_SELECT} GROUP BY s.id ORDER BY s.created_at DESC, s.id DESC");
    let mut st = conn.prepare(&sql)?;
    let rows = st
        .query_map([], session_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn set_cover_if_missing(conn: &Connection, session_id: i64, photo_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE session SET cover_photo_id=?2 WHERE id=?1 AND cover_photo_id IS NULL",
        params![session_id, photo_id],
    )?;
    Ok(())
}

/// The photos only this session references: `(photo id, fast_key, every edit hash they ever had)`.
/// Read before [`delete_session`] so the edited-image caches and patch assets can be purged.
pub fn session_orphans(conn: &Connection, id: i64) -> Result<Vec<(i64, String, Vec<String>)>> {
    let mut st = conn.prepare(
        "SELECT p.id, p.fast_key, p.edit_hash FROM photo p JOIN session_photo sp ON sp.photo_id=p.id
         WHERE sp.session_id=?1
           AND NOT EXISTS(SELECT 1 FROM session_photo o WHERE o.photo_id=p.id AND o.session_id<>?1)",
    )?;
    let rows = st
        .query_map([id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(st);
    let mut out = Vec::with_capacity(rows.len());
    for (pid, key, current) in rows {
        let mut hashes: Vec<String> = current.into_iter().collect();
        let mut st = conn.prepare("SELECT hash FROM edit_version WHERE photo_id=?1")?;
        for h in st.query_map([pid], |r| r.get::<_, String>(0))? {
            let h = h?;
            if !hashes.contains(&h) {
                hashes.push(h);
            }
        }
        out.push((pid, key, hashes));
    }
    Ok(out)
}

/// Removes the session, its links and the photo rows no other session references.
/// Returns `(fast_key)` of removed photos so the caller can purge caches. Never touches originals.
pub fn delete_session(conn: &mut Connection, id: i64) -> Result<Vec<String>> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let exists: i64 = tx.query_row("SELECT COUNT(*) FROM session WHERE id=?1", [id], |r| {
        r.get(0)
    })?;
    if exists == 0 {
        return Err(CoreError::not_found(format!("session {id} not found")));
    }
    let orphans: Vec<(i64, String)> = {
        let mut st = tx.prepare(
            "SELECT p.id, p.fast_key FROM photo p JOIN session_photo sp ON sp.photo_id=p.id
             WHERE sp.session_id=?1
               AND NOT EXISTS(SELECT 1 FROM session_photo o WHERE o.photo_id=p.id AND o.session_id<>?1)",
        )?;
        let v = st
            .query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        v
    };
    let root_id: Option<i64> =
        tx.query_row("SELECT root_id FROM session WHERE id=?1", [id], |r| {
            r.get(0)
        })?;
    tx.execute("DELETE FROM session WHERE id=?1", [id])?; // cascades session_photo
    for chunk in orphans.chunks(500) {
        let ph = vec!["?"; chunk.len()].join(",");
        tx.execute(
            &format!("DELETE FROM photo WHERE id IN ({ph})"),
            params_from_iter(chunk.iter().map(|(i, _)| *i)),
        )?;
    }
    if let Some(root) = root_id {
        tx.execute(
            "DELETE FROM root_folder WHERE id=?1
               AND NOT EXISTS(SELECT 1 FROM session WHERE root_id=?1)
               AND NOT EXISTS(SELECT 1 FROM photo WHERE root_id=?1)",
            [root],
        )?;
    }
    tx.commit()?;
    Ok(orphans.into_iter().map(|(_, k)| k).collect())
}

// ---------------------------------------------------------------- import writes

pub struct NewPhoto {
    pub rel_path: String,
    pub file_name: String,
    pub size: i64,
    pub mtime_ms: i64,
    pub fast_key: String,
    pub format: ImageFormat,
}

#[derive(Debug, Clone)]
pub struct Inserted {
    pub id: i64,
    pub meta_done: bool,
    pub thumb_state: i64,
}

/// One transaction: upsert photos (keyed by root + relative path) and link them to the session.
pub fn insert_batch(
    conn: &mut Connection,
    root_id: i64,
    session_id: i64,
    batch: &[NewPhoto],
) -> Result<Vec<Inserted>> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut out = Vec::with_capacity(batch.len());
    {
        let mut ins = tx.prepare_cached(
            "INSERT INTO photo(root_id, rel_path, file_name, file_size, mtime, fast_key, format)
             VALUES(?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(root_id, rel_path) DO UPDATE SET
               file_size=excluded.file_size, mtime=excluded.mtime, format=excluded.format, missing=0,
               thumb_state = CASE WHEN photo.fast_key=excluded.fast_key THEN photo.thumb_state ELSE 0 END,
               meta_done   = CASE WHEN photo.fast_key=excluded.fast_key THEN photo.meta_done ELSE 0 END,
               fast_key=excluded.fast_key
             RETURNING id, meta_done, thumb_state",
        )?;
        let mut link = tx.prepare_cached(
            "INSERT OR IGNORE INTO session_photo(session_id, photo_id) VALUES(?1, ?2)",
        )?;
        for p in batch {
            let ins_row = ins.query_row(
                params![
                    root_id,
                    p.rel_path,
                    p.file_name,
                    p.size,
                    p.mtime_ms,
                    p.fast_key,
                    p.format.as_str()
                ],
                |r| {
                    Ok(Inserted {
                        id: r.get(0)?,
                        meta_done: r.get::<_, i64>(1)? != 0,
                        thumb_state: r.get(2)?,
                    })
                },
            )?;
            link.execute(params![session_id, ins_row.id])?;
            out.push(ins_row);
        }
    }
    if let Some(first) = out.first() {
        set_cover_if_missing(&tx, session_id, first.id)?;
    }
    tx.commit()?;
    Ok(out)
}

pub struct MetaUpdate {
    pub id: i64,
    pub meta: Option<Metadata>,
    pub content_key: Option<String>,
}

/// Writes metadata (or just marks `meta_done` when reading failed) for a batch in one transaction.
pub fn apply_metadata(conn: &mut Connection, updates: &[MetaUpdate]) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    type DeviceKey = (Option<String>, Option<String>, Option<String>);
    let mut devices: HashMap<DeviceKey, i64> = HashMap::new();
    for u in updates {
        let Some(m) = &u.meta else {
            tx.execute(
                "UPDATE photo SET meta_done=1, content_key=COALESCE(?2, content_key) WHERE id=?1",
                params![u.id, u.content_key],
            )?;
            continue;
        };
        let device_id =
            if m.camera_make.is_some() || m.camera_model.is_some() || m.camera_serial.is_some() {
                let key = (
                    m.camera_make.clone(),
                    m.camera_model.clone(),
                    m.camera_serial.clone(),
                );
                if let Some(id) = devices.get(&key) {
                    Some(*id)
                } else {
                    let found: Option<i64> = tx
                    .query_row(
                        "SELECT id FROM device WHERE make IS ?1 AND model IS ?2 AND serial IS ?3",
                        params![key.0, key.1, key.2],
                        |r| r.get(0),
                    )
                    .ok();
                    let id = match found {
                        Some(id) => id,
                        None => {
                            let (name, kind) =
                                crate::device::describe(key.0.as_deref(), key.1.as_deref());
                            tx.execute(
                                "INSERT INTO device(make, model, serial, offset_source, name, kind)
                             VALUES(?1,?2,?3,'auto',?4,?5)",
                                params![key.0, key.1, key.2, name, kind.as_str()],
                            )?;
                            tx.last_insert_rowid()
                        }
                    };
                    devices.insert(key, id);
                    Some(id)
                }
            } else {
                None
            };
        let (mut w, mut h) = (m.width.map(i64::from), m.height.map(i64::from));
        if (5..=8).contains(&m.orientation) {
            std::mem::swap(&mut w, &mut h);
        }
        let orientation = if (1..=8).contains(&m.orientation) {
            m.orientation
        } else {
            1
        };
        tx.execute(
            "UPDATE photo SET width=?2, height=?3, orientation=?4, taken_at=?5, taken_at_raw=?5, device_id=?6,
               camera=?7, lens=?8, focal=?9, aperture=?10, shutter=?11, iso=?12, gps_lat=?13, gps_lon=?14,
               content_key=COALESCE(?15, content_key), taken_at_offset_min=?16, meta_done=1
             WHERE id=?1",
            params![
                u.id,
                w,
                h,
                orientation,
                m.taken_at_ms,
                device_id,
                crate::device::display_from_raw(m.camera_make.as_deref(), m.camera_model.as_deref()),
                m.lens,
                m.focal_mm.map(clean_f64),
                m.aperture.map(clean_f64),
                m.shutter_s.map(clean_f64),
                m.iso.map(i64::from),
                m.gps_lat,
                m.gps_lon,
                u.content_key,
                m.taken_at_offset_min,
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

// ---------------------------------------------------------------- photo refs (for imaging)

/// Everything needed to generate an image from a photo row.
#[derive(Debug, Clone)]
pub struct PhotoRef {
    pub id: i64,
    pub path: PathBuf,
    pub file_name: String,
    pub format: ImageFormat,
    pub orientation: u8,
    pub fast_key: String,
    pub taken_at: Option<i64>,
    pub mtime_ms: Option<i64>,
    /// Hash of the saved edit stack (`None` = unedited).
    pub edit_hash: Option<String>,
    /// Display size (orientation applied), when known.
    pub width: Option<u32>,
    pub height: Option<u32>,
}

impl PhotoRef {
    pub fn thumb_version(&self) -> String {
        thumb_version_for(&self.fast_key, self.edit_hash.as_deref())
    }
}

fn photo_ref_row(r: &Row) -> rusqlite::Result<PhotoRef> {
    let root: String = r.get(1)?;
    let rel: String = r.get(2)?;
    let fmt: String = r.get(4)?;
    Ok(PhotoRef {
        id: r.get(0)?,
        path: join_rel(&root, &rel),
        file_name: r.get(3)?,
        format: format_from_str(&fmt).unwrap_or(ImageFormat::Jpeg),
        orientation: r.get::<_, Option<i64>>(5)?.unwrap_or(1).clamp(1, 8) as u8,
        fast_key: r.get(6)?,
        taken_at: r.get(7)?,
        mtime_ms: r.get(8)?,
        edit_hash: r.get(9)?,
        width: r.get::<_, Option<i64>>(10)?.map(|v| v.max(0) as u32),
        height: r.get::<_, Option<i64>>(11)?.map(|v| v.max(0) as u32),
    })
}

const REF_SELECT: &str =
    "SELECT p.id, r.path, p.rel_path, p.file_name, COALESCE(p.format,''), p.orientation,
       p.fast_key, p.taken_at, p.mtime, p.edit_hash, p.width, p.height FROM photo p JOIN root_folder r ON r.id=p.root_id";

pub fn photo_ref(conn: &Connection, id: i64) -> Result<PhotoRef> {
    conn.query_row(&format!("{REF_SELECT} WHERE p.id=?1"), [id], photo_ref_row)
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::not_found(format!("photo {id} not found"))
            }
            e => e.into(),
        })
}

/// Refs for the ids that exist, in the order given (missing ids are skipped).
pub fn photo_refs(conn: &Connection, ids: &[i64]) -> Result<Vec<PhotoRef>> {
    let mut found: HashMap<i64, PhotoRef> = HashMap::new();
    for chunk in ids.chunks(500) {
        let ph = vec!["?"; chunk.len()].join(",");
        let mut st = conn.prepare(&format!("{REF_SELECT} WHERE p.id IN ({ph})"))?;
        let rows = st
            .query_map(params_from_iter(chunk.iter()), photo_ref_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for r in rows {
            found.insert(r.id, r);
        }
    }
    Ok(ids.iter().filter_map(|i| found.remove(i)).collect())
}

pub fn mark_thumb_state(conn: &Connection, id: i64, state: i64) -> Result<()> {
    conn.execute(
        "UPDATE photo SET thumb_state=MAX(thumb_state, ?2) WHERE id=?1",
        params![id, state],
    )?;
    Ok(())
}

/// Ids of photos in `ids` whose grid thumbnail is not generated yet.
pub fn ids_needing_grid(conn: &Connection, ids: &[i64]) -> Result<Vec<i64>> {
    let mut need = std::collections::HashSet::new();
    for chunk in ids.chunks(500) {
        let ph = vec!["?"; chunk.len()].join(",");
        let mut st = conn.prepare(&format!(
            "SELECT id FROM photo WHERE thumb_state<2 AND id IN ({ph})"
        ))?;
        for id in st.query_map(params_from_iter(chunk.iter()), |r| r.get::<_, i64>(0))? {
            need.insert(id?);
        }
    }
    Ok(ids.iter().copied().filter(|i| need.contains(i)).collect())
}

// ---------------------------------------------------------------- photo queries

const PHOTO_COLS: &str = "p.id, r.path, p.rel_path, p.file_name, COALESCE(p.format,''), COALESCE(p.file_size,0),
    p.width, p.height, p.taken_at, p.camera, p.lens, p.focal, p.aperture, p.shutter, p.iso,
    p.user_rating, p.ai_rating, COALESCE(p.flag,0), p.color_label, p.burst_id, COALESCE(p.thumb_state,0), p.fast_key, p.taken_at_offset_min,
    p.ai_score, COALESCE(p.issues,0), p.rank_in_burst, (SELECT b.size FROM burst b WHERE b.id=p.burst_id), p.scene_type,
    p.face_count, p.subject_face_count, COALESCE(p.analysis_version,0),
    COALESCE(p.has_edits,0), p.edit_hash,
    p.device_id, (SELECT d.kind FROM device d WHERE d.id=p.device_id)";

/// Number of columns in `PHOTO_COLS`; queries append the session id at this index and the
/// sort key (for cursors) right after it. Update when adding columns.
const PHOTO_COL_COUNT: usize = 35;

/// Column `PHOTO_COL_COUNT` (after PHOTO_COLS) is the session id.
fn photo_row(r: &Row) -> rusqlite::Result<Photo> {
    let root: String = r.get(1)?;
    let rel: String = r.get(2)?;
    let fast_key: String = r.get(21)?;
    Ok(Photo {
        id: r.get(0)?,
        session_id: r.get::<_, Option<i64>>(PHOTO_COL_COUNT)?.unwrap_or(0),
        path: join_rel(&root, &rel).to_string_lossy().into_owned(),
        file_name: r.get(3)?,
        format: r.get(4)?,
        file_size: r.get(5)?,
        width: r.get(6)?,
        height: r.get(7)?,
        taken_at: r.get(8)?,
        taken_at_offset_min: r.get(22)?,
        camera: r.get(9)?,
        lens: r.get(10)?,
        focal_mm: r.get(11)?,
        aperture: r.get(12)?,
        shutter_s: r.get(13)?,
        iso: r.get(14)?,
        user_rating: r.get(15)?,
        ai_rating: r.get::<_, Option<f64>>(16)?.map(crate::jsonfix::tidy_f64),
        flag: r.get(17)?,
        color_label: r.get(18)?,
        burst_id: r.get(19)?,
        thumb_ready: r.get::<_, i64>(20)? >= 2,
        thumb_version: thumb_version_for(&fast_key, r.get::<_, Option<String>>(32)?.as_deref()),
        ai_score: r.get::<_, Option<f64>>(23)?.map(crate::jsonfix::tidy_f64),
        issues: crate::analysis::scoring::Issue::from_mask(r.get::<_, i64>(24)?),
        rank_in_burst: r.get(25)?,
        burst_size: r.get(26)?,
        scene_type: r.get(27)?,
        face_count: r.get(28)?,
        subject_face_count: r.get(29)?,
        analyzed: r.get::<_, i64>(30)? > 0,
        has_edits: r.get::<_, i64>(31)? != 0,
        device_id: r.get(33)?,
        device_kind: r.get::<_, Option<String>>(34)?.map(|k| {
            crate::device::DeviceKind::parse(&k).unwrap_or(crate::device::DeviceKind::Unknown)
        }),
    })
}

pub fn get_photo(conn: &Connection, id: i64) -> Result<Photo> {
    let sql = format!(
        "SELECT {PHOTO_COLS}, (SELECT MIN(session_id) FROM session_photo WHERE photo_id=p.id)
         FROM photo p JOIN root_folder r ON r.id=p.root_id WHERE p.id=?1"
    );
    conn.query_row(&sql, [id], photo_row).map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => {
            CoreError::not_found(format!("photo {id} not found"))
        }
        e => e.into(),
    })
}

#[derive(Serialize, Deserialize)]
struct Cursor {
    k: serde_json::Value,
    id: i64,
}

fn hex_encode(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

pub const DEFAULT_LIMIT: i64 = 500;
pub const MAX_LIMIT: i64 = 5000;

pub fn query_photos(conn: &Connection, q: &PhotoQuery) -> Result<PhotosPage> {
    query_photos_cached(conn, q, None)
}

/// [`query_photos`] that can reuse a `total` computed earlier for the same filter, as long as
/// nothing was written to the catalog in between (`Db::generation`).
pub fn query_photos_cached(
    conn: &Connection,
    q: &PhotoQuery,
    db: Option<&crate::db::Db>,
) -> Result<PhotosPage> {
    // The session must exist.
    get_session_exists(conn, q.session_id)?;

    let (key_expr, desc, text_key) = match q.sort {
        SortKey::TakenAt => ("COALESCE(p.taken_at, 9223372036854775807)", false, false),
        SortKey::TakenAtDesc => ("COALESCE(p.taken_at, -9223372036854775807)", true, false),
        SortKey::Name => ("p.file_name COLLATE NOCASE", false, true),
        SortKey::Rating => ("COALESCE(p.user_rating, -1)", true, false),
        SortKey::Ai => (
            "CAST(COALESCE(p.ai_score, -0.001) * 1000000 AS INTEGER)",
            true,
            false,
        ),
    };

    let mut wheres: Vec<String> = Vec::new();
    let mut args: Vec<Value> = vec![Value::Integer(q.session_id)];
    if let Some(n) = q.rating_gte.filter(|n| *n > 0) {
        wheres.push("p.user_rating >= ?".into());
        args.push(Value::Integer(n));
    }
    match q.flag {
        FlagFilter::Any => {}
        FlagFilter::Picked => wheres.push("p.flag = 1".into()),
        FlagFilter::Rejected => wheres.push("p.flag = -1".into()),
        FlagFilter::Unflagged => wheres.push("p.flag = 0".into()),
        FlagFilter::NotRejected => wheres.push("p.flag >= 0".into()),
    }
    if let Some(c) = &q.color_label {
        wheres.push("p.color_label = ?".into());
        args.push(Value::Text(c.clone()));
    }
    push_m2_filters(q, &mut wheres, &mut args);
    if !q.devices.is_empty() || q.device_none {
        let mut parts = Vec::new();
        if !q.devices.is_empty() {
            let ids: Vec<String> = q.devices.iter().map(i64::to_string).collect();
            parts.push(format!("p.device_id IN ({})", ids.join(",")));
        }
        if q.device_none {
            parts.push("p.device_id IS NULL".into());
        }
        wheres.push(format!("({})", parts.join(" OR ")));
    }
    let base_where = if wheres.is_empty() {
        String::new()
    } else {
        format!(" AND {}", wheres.join(" AND "))
    };
    // Join order is pinned (CROSS JOIN): session members first, photo rows by primary key.
    // Without it the planner loops over `root_folder` first and scans the members once per root.
    // A burst filter is far more selective than the session: let the planner start from it.
    let join = if q.burst_id.is_some() {
        "JOIN"
    } else {
        "CROSS JOIN"
    };
    let members = format!(
        "FROM session_photo sp {join} photo p ON p.id=sp.photo_id
         WHERE sp.session_id=?{base_where}"
    );

    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let mut page_args = args.clone();
    let mut cursor_sql = String::new();
    if let Some(c) = &q.cursor {
        let cur: Cursor = hex_decode(c)
            .and_then(|b| serde_json::from_slice(&b).ok())
            .ok_or_else(|| CoreError::bad_request("invalid cursor"))?;
        let kval = match (&cur.k, text_key) {
            (serde_json::Value::String(s), true) => Value::Text(s.clone()),
            (serde_json::Value::Number(n), false) if n.is_i64() => {
                Value::Integer(n.as_i64().unwrap())
            }
            _ => return Err(CoreError::bad_request("invalid cursor")),
        };
        let cmp = if desc { "<" } else { ">" };
        cursor_sql = format!(" AND ({key_expr} {cmp} ? OR ({key_expr} = ? AND p.id > ?))");
        page_args.push(kval.clone());
        page_args.push(kval);
        page_args.push(Value::Integer(cur.id));
    }

    let dir = if desc { "DESC" } else { "ASC" };
    let collate = if text_key { " COLLATE NOCASE" } else { "" };
    // Two steps: sort only (id, key) pairs and keep the first `limit + 1`, then read the wide
    // rows for those ids. Sorting complete rows made every page cost O(session) row copies.
    let sql = format!(
        "SELECT {PHOTO_COLS}, pg.sid, pg.k
         FROM (SELECT p.id AS id, sp.session_id AS sid, {key_expr} AS k {members}{cursor_sql}
               ORDER BY {key_expr} {dir}, p.id ASC LIMIT {limit1}) pg
         CROSS JOIN photo p ON p.id=pg.id
         CROSS JOIN root_folder r ON r.id=p.root_id
         ORDER BY pg.k{collate} {dir}, pg.id ASC",
        limit1 = limit + 1
    );
    let mut st = conn.prepare_cached(&sql)?;
    let mut rows: Vec<(Photo, serde_json::Value)> = st
        .query_map(params_from_iter(page_args.iter()), |r| {
            let photo = photo_row(r)?;
            let key = if text_key {
                serde_json::Value::String(r.get::<_, String>(PHOTO_COL_COUNT + 1)?)
            } else {
                serde_json::Value::from(r.get::<_, i64>(PHOTO_COL_COUNT + 1)?)
            };
            Ok((photo, key))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let more = rows.len() as i64 > limit;
    let total: i64 = if q.cursor.is_none() && !more {
        // the whole result is on this page
        rows.len() as i64
    } else {
        let count_sql = if wheres.is_empty() {
            "SELECT COUNT(*) FROM session_photo WHERE session_id=?".to_string()
        } else {
            format!("SELECT COUNT(*) {members}")
        };
        let compute = || -> Result<i64> {
            let mut st = conn.prepare_cached(&count_sql)?;
            Ok(st.query_row(params_from_iter(args.iter()), |r| r.get(0))?)
        };
        match db {
            Some(db) => {
                let generation = db.generation();
                let key = format!("{count_sql}|{args:?}");
                match db.cached_count(generation, &key) {
                    Some(n) => n,
                    None => {
                        let n = compute()?;
                        db.store_count(generation, key, n);
                        n
                    }
                }
            }
            None => compute()?,
        }
    };

    let mut next_cursor = None;
    if more {
        rows.truncate(limit as usize);
        if let Some((p, k)) = rows.last() {
            let cur = Cursor {
                k: k.clone(),
                id: p.id,
            };
            next_cursor = Some(hex_encode(&serde_json::to_vec(&cur)?));
        }
    }
    Ok(PhotosPage {
        photos: rows.into_iter().map(|(p, _)| p).collect(),
        total,
        next_cursor,
    })
}

/// M2 filters (docs/api-contract-m2.md C.4, docs/03 section 4.4). Ids and thresholds are inlined
/// (all numeric), everything else binds parameters in order.
fn push_m2_filters(q: &PhotoQuery, wheres: &mut Vec<String>, args: &mut Vec<Value>) {
    if let Some(n) = q.ai_rating_gte {
        wheres.push("p.ai_rating >= ?".into());
        args.push(Value::Real(n));
    }
    if q.issues_none {
        wheres.push("p.analysis_version > 0 AND COALESCE(p.issues,0) = 0".into());
    }
    if !q.issues_any.is_empty() {
        let mask = crate::analysis::scoring::Issue::to_mask(&q.issues_any);
        wheres.push(format!("(COALESCE(p.issues,0) & {mask}) <> 0"));
    }
    if q.burst_best_only {
        wheres.push("(p.burst_id IS NULL OR p.rank_in_burst = 0)".into());
    }
    if let Some(b) = q.burst_id {
        wheres.push("p.burst_id = ?".into());
        args.push(Value::Integer(b));
    }
    if let Some(s) = &q.scene_type {
        wheres.push("p.scene_type = ?".into());
        args.push(Value::Text(s.clone()));
    }
    let id_list = |ids: &[i64]| {
        ids.iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",")
    };
    let mut state_conds: Vec<String> = q
        .person_state
        .iter()
        .map(|s| match s {
            PersonState::EyesOpen => format!("f.eyes_open >= {EYES_OPEN_MIN}"),
            PersonState::Smiling => format!("f.smile >= {SMILE_MIN}"),
            PersonState::Looking => format!("f.gaze >= {GAZE_MIN}"),
            PersonState::Subject => "f.is_subject = 1".to_string(),
        })
        .collect();
    if !q.include_background && !q.person_state.contains(&PersonState::Subject) {
        state_conds.push("f.is_subject = 1".into());
    }
    if !q.persons.is_empty() {
        let mut ids = q.persons.clone();
        ids.sort_unstable();
        ids.dedup();
        let mut conds = vec![format!("f.person_id IN ({})", id_list(&ids))];
        conds.extend(state_conds.iter().cloned());
        let having = match q.person_mode {
            PersonMode::All => format!(
                " GROUP BY f.photo_id HAVING COUNT(DISTINCT f.person_id) = {}",
                ids.len()
            ),
            PersonMode::Any => String::new(),
        };
        wheres.push(format!(
            "p.id IN (SELECT f.photo_id FROM face f WHERE {}{having})",
            conds.join(" AND ")
        ));
    } else if !q.person_state.is_empty() {
        wheres.push(format!(
            "EXISTS (SELECT 1 FROM face f WHERE f.photo_id = p.id AND {})",
            state_conds.join(" AND ")
        ));
    }
    if !q.exclude_persons.is_empty() {
        let mut conds = vec![
            "f.photo_id = p.id".to_string(),
            format!("f.person_id IN ({})", id_list(&q.exclude_persons)),
        ];
        if !q.include_background {
            conds.push("f.is_subject = 1".into());
        }
        wheres.push(format!(
            "NOT EXISTS (SELECT 1 FROM face f WHERE {})",
            conds.join(" AND ")
        ));
    }
    match q.has_edits {
        Some(true) => wheres.push("COALESCE(p.has_edits,0) = 1".into()),
        Some(false) => wheres.push("COALESCE(p.has_edits,0) = 0".into()),
        None => {}
    }
    if let Some(n) = q.faces_min {
        wheres.push(format!(
            "p.subject_face_count IS NOT NULL AND p.subject_face_count >= {n}"
        ));
    }
    if let Some(n) = q.faces_max {
        wheres.push(format!(
            "p.subject_face_count IS NOT NULL AND p.subject_face_count <= {n}"
        ));
    }
}

/// Devices with photos in a session (not missing), most photos first.
pub fn session_devices(conn: &Connection, session_id: i64) -> Result<Vec<SessionDevice>> {
    get_session_exists(conn, session_id)?;
    let mut st = conn.prepare_cached(
        "SELECT d.id, d.make, d.model, d.name, d.kind, COUNT(*) AS n
         FROM session_photo sp
         CROSS JOIN photo p ON p.id=sp.photo_id AND COALESCE(p.missing,0)=0
         CROSS JOIN device d ON d.id=p.device_id
         WHERE sp.session_id=?1
         GROUP BY d.id
         ORDER BY n DESC, COALESCE(d.name,'') COLLATE NOCASE, d.id",
    )?;
    let rows = st
        .query_map([session_id], |r| {
            let kind: Option<String> = r.get(4)?;
            Ok(SessionDevice {
                id: r.get(0)?,
                make: r.get(1)?,
                model: r.get(2)?,
                name: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                kind: kind
                    .as_deref()
                    .and_then(crate::device::DeviceKind::parse)
                    .unwrap_or(crate::device::DeviceKind::Unknown),
                photo_count: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn get_session_exists(conn: &Connection, id: i64) -> Result<()> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM session WHERE id=?1", [id], |r| {
        r.get(0)
    })?;
    if n == 0 {
        return Err(CoreError::not_found(format!("session {id} not found")));
    }
    Ok(())
}

// ---------------------------------------------------------------- patch

pub fn validate_patch(req: &PatchRequest) -> Result<()> {
    if let Some(Some(r)) = req.user_rating {
        if !(0..=5).contains(&r) {
            return Err(CoreError::bad_request("user_rating must be 0..5 or null"));
        }
    }
    if let Some(f) = req.flag {
        if !(-1..=1).contains(&f) {
            return Err(CoreError::bad_request("flag must be -1, 0 or 1"));
        }
    }
    if let Some(Some(c)) = &req.color_label {
        if !COLOR_LABELS.contains(&c.as_str()) {
            return Err(CoreError::bad_request(
                "color_label must be red|yellow|green|blue|purple or null",
            ));
        }
    }
    if req.user_rating.is_none() && req.flag.is_none() && req.color_label.is_none() {
        return Err(CoreError::bad_request(
            "nothing to update: give user_rating, flag or color_label",
        ));
    }
    Ok(())
}

/// Applies the patch; returns the resulting state of every id that exists.
pub fn patch_photos(conn: &mut Connection, req: &PatchRequest) -> Result<Vec<PhotoUpdate>> {
    validate_patch(req)?;
    let mut sets: Vec<&str> = Vec::new();
    let mut set_args: Vec<Value> = Vec::new();
    if let Some(r) = &req.user_rating {
        sets.push("user_rating=?");
        set_args.push(r.map(Value::Integer).unwrap_or(Value::Null));
    }
    if let Some(f) = req.flag {
        sets.push("flag=?");
        set_args.push(Value::Integer(f));
    }
    if let Some(c) = &req.color_label {
        sets.push("color_label=?");
        set_args.push(c.clone().map(Value::Text).unwrap_or(Value::Null));
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut out = Vec::new();
    for chunk in req.ids.chunks(500) {
        let ph = vec!["?"; chunk.len()].join(",");
        let mut args = set_args.clone();
        args.extend(chunk.iter().map(|i| Value::Integer(*i)));
        tx.execute(
            &format!("UPDATE photo SET {} WHERE id IN ({ph})", sets.join(",")),
            params_from_iter(args.iter()),
        )?;
        let mut st = tx.prepare(&format!(
            "SELECT id, user_rating, COALESCE(flag,0), color_label FROM photo WHERE id IN ({ph})"
        ))?;
        let rows = st
            .query_map(params_from_iter(chunk.iter()), |r| {
                Ok(PhotoUpdate {
                    id: r.get(0)?,
                    user_rating: r.get(1)?,
                    flag: r.get(2)?,
                    color_label: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        out.extend(rows);
    }
    tx.commit()?;
    Ok(out)
}

pub fn root_path(conn: &Connection, root_id: i64) -> Result<String> {
    Ok(
        conn.query_row("SELECT path FROM root_folder WHERE id=?1", [root_id], |r| {
            r.get(0)
        })?,
    )
}

pub fn path_of(root: &str, rel: &str) -> PathBuf {
    join_rel(root, rel)
}

/// Relative `/`-separated path of `file` under `root`.
pub fn rel_path(root: &Path, file: &Path) -> Option<String> {
    let rel = file.strip_prefix(root).ok()?;
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

/// Widen an EXIF `f32` without exposing binary noise (6.3f32 -> 6.3, not 6.300000190734863).
fn clean_f64(x: f32) -> f64 {
    x.to_string().parse().unwrap_or(f64::from(x))
}

#[cfg(test)]
mod col_count_tests {
    #[test]
    fn photo_col_count_matches_select_list() {
        let mut depth = 0i32;
        let mut cols = 1;
        for ch in super::PHOTO_COLS.chars() {
            match ch {
                '(' => depth += 1,
                ')' => depth -= 1,
                ',' if depth == 0 => cols += 1,
                _ => {}
            }
        }
        assert_eq!(cols, super::PHOTO_COL_COUNT);
    }
}

#[cfg(test)]
mod clean_f64_tests {
    #[test]
    fn widens_without_noise() {
        assert_eq!(super::clean_f64(6.3), 6.3);
        assert_eq!(super::clean_f64(1.7), 1.7);
        assert_eq!(super::clean_f64(1.0 / 30.0), 0.033333335);
    }
}
