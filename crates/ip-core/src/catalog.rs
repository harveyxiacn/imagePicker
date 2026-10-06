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

fn camera_string(make: Option<&str>, model: Option<&str>) -> Option<String> {
    let make = make.map(str::trim).filter(|s| !s.is_empty());
    let model = model.map(str::trim).filter(|s| !s.is_empty());
    match (make, model) {
        (Some(a), Some(b)) => {
            if b.to_ascii_lowercase().starts_with(&a.to_ascii_lowercase()) {
                Some(b.to_string())
            } else {
                Some(format!("{a} {b}"))
            }
        }
        (None, Some(b)) => Some(b.to_string()),
        (Some(a), None) => Some(a.to_string()),
        (None, None) => None,
    }
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
        let device_id = if m.camera_make.is_some()
            || m.camera_model.is_some()
            || m.camera_serial.is_some()
        {
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
                        tx.execute(
                            "INSERT INTO device(make, model, serial, offset_source) VALUES(?1,?2,?3,'auto')",
                            params![key.0, key.1, key.2],
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
               content_key=COALESCE(?15, content_key), meta_done=1
             WHERE id=?1",
            params![
                u.id,
                w,
                h,
                orientation,
                m.taken_at_ms,
                device_id,
                camera_string(m.camera_make.as_deref(), m.camera_model.as_deref()),
                m.lens,
                m.focal_mm.map(f64::from),
                m.aperture.map(f64::from),
                m.shutter_s.map(f64::from),
                m.iso.map(i64::from),
                m.gps_lat,
                m.gps_lon,
                u.content_key,
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
    })
}

const REF_SELECT: &str =
    "SELECT p.id, r.path, p.rel_path, p.file_name, COALESCE(p.format,''), p.orientation,
       p.fast_key, p.taken_at, p.mtime FROM photo p JOIN root_folder r ON r.id=p.root_id";

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
    p.user_rating, p.ai_rating, COALESCE(p.flag,0), p.color_label, p.burst_id, COALESCE(p.thumb_state,0), p.fast_key";

/// Column 22 (after PHOTO_COLS) is the session id.
fn photo_row(r: &Row) -> rusqlite::Result<Photo> {
    let root: String = r.get(1)?;
    let rel: String = r.get(2)?;
    let fast_key: String = r.get(21)?;
    Ok(Photo {
        id: r.get(0)?,
        session_id: r.get::<_, Option<i64>>(22)?.unwrap_or(0),
        path: join_rel(&root, &rel).to_string_lossy().into_owned(),
        file_name: r.get(3)?,
        format: r.get(4)?,
        file_size: r.get(5)?,
        width: r.get(6)?,
        height: r.get(7)?,
        taken_at: r.get(8)?,
        camera: r.get(9)?,
        lens: r.get(10)?,
        focal_mm: r.get(11)?,
        aperture: r.get(12)?,
        shutter_s: r.get(13)?,
        iso: r.get(14)?,
        user_rating: r.get(15)?,
        ai_rating: r.get(16)?,
        flag: r.get(17)?,
        color_label: r.get(18)?,
        burst_id: r.get(19)?,
        thumb_ready: r.get::<_, i64>(20)? >= 2,
        thumb_version: thumb_version(&fast_key),
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
    // The session must exist.
    get_session_exists(conn, q.session_id)?;

    let (key_expr, desc, text_key) = match q.sort {
        SortKey::TakenAt => ("COALESCE(p.taken_at, 9223372036854775807)", false, false),
        SortKey::TakenAtDesc => ("COALESCE(p.taken_at, -9223372036854775807)", true, false),
        SortKey::Name => ("p.file_name COLLATE NOCASE", false, true),
        SortKey::Rating => ("COALESCE(p.user_rating, -1)", true, false),
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
    let base_where = if wheres.is_empty() {
        String::new()
    } else {
        format!(" AND {}", wheres.join(" AND "))
    };
    let from = format!(
        "FROM photo p JOIN root_folder r ON r.id=p.root_id
         JOIN session_photo sp ON sp.photo_id=p.id AND sp.session_id=?{base_where}"
    );

    let total: i64 = conn.query_row(
        &format!("SELECT COUNT(*) {from}"),
        params_from_iter(args.iter()),
        |r| r.get(0),
    )?;

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

    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let dir = if desc { "DESC" } else { "ASC" };
    let sql = format!(
        "SELECT {PHOTO_COLS}, sp.session_id, {key_expr} {from}{cursor_sql}
         ORDER BY {key_expr} {dir}, p.id ASC LIMIT {}",
        limit + 1
    );
    let mut st = conn.prepare(&sql)?;
    let mut rows: Vec<(Photo, serde_json::Value)> = st
        .query_map(params_from_iter(page_args.iter()), |r| {
            let photo = photo_row(r)?;
            let key = if text_key {
                serde_json::Value::String(r.get::<_, String>(23)?)
            } else {
                serde_json::Value::from(r.get::<_, i64>(23)?)
            };
            Ok((photo, key))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut next_cursor = None;
    if rows.len() as i64 > limit {
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
