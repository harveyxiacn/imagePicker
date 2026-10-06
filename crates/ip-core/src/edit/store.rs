//! SQLite access for edits and presets.

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::Value;

use crate::error::{CoreError, Result};

/// Versions kept per photo (the current one plus the previous ones, for safety).
pub const KEEP_VERSIONS: i64 = 5;

#[derive(Debug, Clone)]
pub struct CurrentEdit {
    /// The stack exactly as it was saved (unknown ops included).
    pub stack: Value,
    pub hash: String,
    pub updated_at: i64,
    /// False when the saved stack renders as the original (e.g. only unknown ops).
    pub has_edits: bool,
}

fn ensure_photo(conn: &Connection, id: i64) -> Result<()> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM photo WHERE id=?1", [id], |r| r.get(0))?;
    if n == 0 {
        return Err(CoreError::not_found(format!("photo {id} not found")));
    }
    Ok(())
}

pub fn current(conn: &Connection, photo_id: i64) -> Result<Option<CurrentEdit>> {
    let row = conn
        .query_row(
            "SELECT e.stack, e.hash, COALESCE(e.updated_at,0), COALESCE(p.has_edits,0)
             FROM edit_version e JOIN photo p ON p.id=e.photo_id
             WHERE e.photo_id=?1 AND e.is_current=1 ORDER BY e.id DESC LIMIT 1",
            [photo_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()?;
    match row {
        None => Ok(None),
        Some((stack, hash, updated_at, has)) => Ok(Some(CurrentEdit {
            stack: serde_json::from_str(&stack)?,
            hash,
            updated_at,
            has_edits: has != 0,
        })),
    }
}

/// Every retained stack version of the photo (current and previous), as stored.
pub fn all_stacks(conn: &Connection, photo_id: i64) -> Result<Vec<Value>> {
    let mut st = conn.prepare("SELECT stack FROM edit_version WHERE photo_id=?1")?;
    let rows = st
        .query_map([photo_id], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|s| serde_json::from_str(&s).map_err(Into::into))
        .collect()
}

#[derive(Debug, Clone)]
pub struct SaveOutcome {
    /// `photo.edit_hash` before the call (cached images of it can be dropped).
    pub old_hash: Option<String>,
    /// False when the stack equals the current one (nothing written).
    pub changed: bool,
    pub updated_at: i64,
}

/// Makes `stack` the current version of the photo.
pub fn save(
    conn: &mut Connection,
    photo_id: i64,
    stack: &Value,
    hash: &str,
    has_edits: bool,
    now: i64,
) -> Result<SaveOutcome> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    ensure_photo(&tx, photo_id)?;
    let old_hash: Option<String> =
        tx.query_row("SELECT edit_hash FROM photo WHERE id=?1", [photo_id], |r| {
            r.get(0)
        })?;
    let cur: Option<(String, i64)> = tx
        .query_row(
            "SELECT hash, COALESCE(updated_at,0) FROM edit_version
             WHERE photo_id=?1 AND is_current=1 ORDER BY id DESC LIMIT 1",
            [photo_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((h, at)) = &cur {
        if h == hash {
            tx.commit()?;
            return Ok(SaveOutcome {
                old_hash,
                changed: false,
                updated_at: *at,
            });
        }
    }
    tx.execute(
        "UPDATE edit_version SET is_current=0 WHERE photo_id=?1",
        [photo_id],
    )?;
    tx.execute(
        "INSERT INTO edit_version(photo_id, name, stack, hash, updated_at, is_current)
         VALUES(?1, NULL, ?2, ?3, ?4, 1)",
        params![photo_id, stack.to_string(), hash, now],
    )?;
    tx.execute(
        "DELETE FROM edit_version WHERE photo_id=?1 AND id NOT IN
           (SELECT id FROM edit_version WHERE photo_id=?1 ORDER BY id DESC LIMIT ?2)",
        params![photo_id, KEEP_VERSIONS],
    )?;
    tx.execute(
        "UPDATE photo SET has_edits=?2, edit_hash=?3 WHERE id=?1",
        params![photo_id, has_edits as i64, has_edits.then_some(hash)],
    )?;
    tx.commit()?;
    Ok(SaveOutcome {
        old_hash,
        changed: true,
        updated_at: now,
    })
}

/// Resets the photo to its original. Returns the previous `edit_hash`.
pub fn clear(conn: &mut Connection, photo_id: i64) -> Result<Option<String>> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    ensure_photo(&tx, photo_id)?;
    let old: Option<String> =
        tx.query_row("SELECT edit_hash FROM photo WHERE id=?1", [photo_id], |r| {
            r.get(0)
        })?;
    tx.execute(
        "UPDATE edit_version SET is_current=0 WHERE photo_id=?1",
        [photo_id],
    )?;
    tx.execute(
        "UPDATE photo SET has_edits=0, edit_hash=NULL WHERE id=?1",
        [photo_id],
    )?;
    tx.commit()?;
    Ok(old)
}

#[derive(Debug, Clone)]
pub struct PresetRow {
    pub id: i64,
    pub name: String,
    pub stack: Value,
}

pub fn presets(conn: &Connection) -> Result<Vec<PresetRow>> {
    let mut st = conn.prepare("SELECT id, name, stack FROM preset ORDER BY id")?;
    let rows = st
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|(id, name, stack)| {
            Ok(PresetRow {
                id,
                name,
                stack: serde_json::from_str(&stack)?,
            })
        })
        .collect()
}

pub fn insert_preset(conn: &Connection, name: &str, stack: &Value, now: i64) -> Result<i64> {
    conn.execute(
        "INSERT INTO preset(name, stack, created_at) VALUES(?1, ?2, ?3)",
        params![name, stack.to_string(), now],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn delete_preset(conn: &Connection, id: i64) -> Result<()> {
    let n = conn.execute("DELETE FROM preset WHERE id=?1", [id])?;
    if n == 0 {
        return Err(CoreError::not_found(format!("preset {id} not found")));
    }
    Ok(())
}
