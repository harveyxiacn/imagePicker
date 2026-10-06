//! Catalog <-> XMP synchronisation: reading on import, debounced writing after user edits,
//! conflict detection by sidecar mtime and the manual `POST /api/xmp/sync`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use ip_imaging::ImageFormat;
use rusqlite::{params, params_from_iter, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    apply_catalog, atomic_write, find_sidecar, jpeg, normalize_tags, sidecar_candidates,
    values_of, CatalogValues, Xmp, XmpValues,
};
use crate::catalog::{self, now_ms};
use crate::error::{CoreError, Result};
use crate::events::Event;
use crate::model::PhotoUpdate;
use crate::Core;

#[derive(Debug, Clone, Deserialize)]
pub struct XmpSyncRequest {
    pub session_id: i64,
    /// `read` | `write`
    pub direction: String,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct XmpSyncReport {
    pub direction: String,
    /// Photos looked at.
    pub photos: usize,
    /// Catalog rows changed from XMP.
    pub updated: usize,
    /// Sidecars (or embedded packets) written.
    pub written: usize,
    /// Photos whose sidecar differed from the catalog (the sidecar won).
    pub conflicts: usize,
    pub errors: Vec<String>,
}

struct Loaded {
    id: i64,
    path: PathBuf,
    format: ImageFormat,
    cat: CatalogValues,
    recorded_mtime: Option<i64>,
}

fn mtime_ms(p: &Path) -> Option<i64> {
    std::fs::metadata(p)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as i64)
}

fn load(conn: &Connection, ids: &[i64]) -> Result<Vec<Loaded>> {
    let mut out = Vec::new();
    for chunk in ids.chunks(500) {
        let ph = vec!["?"; chunk.len()].join(",");
        let mut st = conn.prepare(&format!(
            "SELECT p.id, r.path, p.rel_path, COALESCE(p.format,''), p.user_rating, COALESCE(p.flag,0),
                    p.color_label, s.sidecar_mtime
             FROM photo p JOIN root_folder r ON r.id=p.root_id
             LEFT JOIN xmp_state s ON s.photo_id=p.id
             WHERE p.id IN ({ph}) ORDER BY p.id"
        ))?;
        let rows = st
            .query_map(params_from_iter(chunk.iter()), |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, Option<i64>>(7)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, root, rel, fmt, rating, flag, label, mtime) in rows {
            let tags = tags_of(conn, id)?;
            out.push(Loaded {
                id,
                path: catalog::join_rel(&root, &rel),
                format: catalog::format_from_str(&fmt).unwrap_or(ImageFormat::Jpeg),
                cat: CatalogValues {
                    user_rating: rating,
                    flag,
                    color_label: label,
                    tags,
                },
                recorded_mtime: mtime,
            });
        }
    }
    Ok(out)
}

pub(crate) fn tags_of(conn: &Connection, id: i64) -> Result<Vec<String>> {
    let mut st = conn.prepare("SELECT tag FROM photo_tag WHERE photo_id=?1 ORDER BY rowid")?;
    let v = st
        .query_map([id], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

fn record_state(conn: &Connection, id: i64, path: &Path, mtime: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO xmp_state(photo_id, sidecar_path, sidecar_mtime, synced_at) VALUES(?1,?2,?3,?4)
         ON CONFLICT(photo_id) DO UPDATE SET sidecar_path=?2, sidecar_mtime=?3, synced_at=?4",
        params![id, path.to_string_lossy(), mtime, now_ms()],
    )?;
    Ok(())
}

fn norm_rating(r: i64) -> i64 {
    if r < 0 {
        -1
    } else {
        r
    }
}

/// The differences between a sidecar and the catalog, as the `xmp.conflict` payload parts.
/// Only fields the sidecar actually carries can conflict.
fn diff(sc: &XmpValues, cat: &CatalogValues) -> Option<(Value, Value)> {
    let (mut s, mut c) = (serde_json::Map::new(), serde_json::Map::new());
    if let Some(r) = sc.rating {
        let cr = if cat.flag == -1 {
            -1
        } else {
            cat.user_rating.unwrap_or(0)
        };
        if norm_rating(r) != cr {
            s.insert("rating".into(), json!(norm_rating(r)));
            c.insert("rating".into(), json!(cr));
        }
    }
    if let Some(l) = sc.label {
        if Some(l) != cat.color_label.as_deref() {
            s.insert("label".into(), json!(l));
            c.insert("label".into(), json!(cat.color_label));
        }
    }
    if !sc.keywords.is_empty() {
        let a: BTreeSet<&String> = sc.keywords.iter().collect();
        let b: BTreeSet<&String> = cat.tags.iter().collect();
        if a != b {
            s.insert("keywords".into(), json!(sc.keywords));
            c.insert("keywords".into(), json!(cat.tags));
        }
    }
    (!s.is_empty()).then(|| (Value::Object(s), Value::Object(c)))
}

/// Makes the catalog row agree with the sidecar (the sidecar wins). Returns the update when the
/// row changed.
fn apply_sidecar(conn: &mut Connection, id: i64, v: &XmpValues) -> Result<Option<PhotoUpdate>> {
    let tx = conn.transaction()?;
    let (rating, flag, label): (Option<i64>, i64, Option<String>) = tx.query_row(
        "SELECT user_rating, COALESCE(flag,0), color_label FROM photo WHERE id=?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let (mut nrating, mut nflag, mut nlabel) = (rating, flag, label.clone());
    if let Some(r) = v.rating {
        if r < 0 {
            nflag = -1;
        } else {
            nrating = (r > 0).then_some(r);
            if nflag == -1 {
                nflag = 0;
            }
        }
    }
    if let Some(p) = v.pick {
        nflag = p;
    }
    if let Some(l) = v.label {
        nlabel = Some(l.to_string());
    }
    let changed = (nrating, nflag, &nlabel) != (rating, flag, &label);
    if changed {
        tx.execute(
            "UPDATE photo SET user_rating=?2, flag=?3, color_label=?4 WHERE id=?1",
            params![id, nrating, nflag, nlabel],
        )?;
    }
    if !v.keywords.is_empty() {
        let cur = tags_of(&tx, id)?;
        let a: BTreeSet<&String> = cur.iter().collect();
        let b: BTreeSet<&String> = v.keywords.iter().collect();
        if a != b {
            tx.execute("DELETE FROM photo_tag WHERE photo_id=?1", [id])?;
            for t in &v.keywords {
                tx.execute(
                    "INSERT OR IGNORE INTO photo_tag(photo_id, tag) VALUES(?1,?2)",
                    params![id, t],
                )?;
            }
        }
    }
    tx.commit()?;
    Ok(changed.then_some(PhotoUpdate {
        id,
        user_rating: nrating,
        flag: nflag,
        color_label: nlabel,
    }))
}

/// What XMP says about a photo: the sidecar if there is one, else the embedded JPEG packet.
/// Returns the values and the sidecar (path, mtime) if one was used.
fn read_values(path: &Path, format: ImageFormat) -> Option<(XmpValues, Option<(PathBuf, i64)>)> {
    if let Some(sc) = find_sidecar(path) {
        let text = std::fs::read_to_string(&sc)
            .or_else(|_| std::fs::read(&sc).map(|b| String::from_utf8_lossy(&b).into_owned()))
            .ok()?;
        let x = match Xmp::parse(&text) {
            Ok(x) => x,
            Err(e) => {
                tracing::warn!(path = %sc.display(), error = %e, "unreadable XMP sidecar");
                return None;
            }
        };
        let m = mtime_ms(&sc).unwrap_or(0);
        return Some((values_of(&x), Some((sc, m))));
    }
    if format == ImageFormat::Jpeg {
        let text = jpeg::read_file(path)?;
        let x = Xmp::parse(text.trim_end_matches('\0')).ok()?;
        return Some((values_of(&x), None));
    }
    None
}

enum Outcome {
    Unchanged { mtime: Option<i64>, path: Option<PathBuf> },
    Written { mtime: Option<i64>, path: PathBuf },
    Conflict {
        sidecar: XmpValues,
        mtime: i64,
        path: PathBuf,
        payload: (Value, Value),
    },
    Error(String),
}

fn parse_or_empty(text: Option<String>) -> std::result::Result<Xmp, String> {
    match text {
        None => Ok(Xmp::empty()),
        Some(t) => Xmp::parse(t.trim_end_matches('\0')).map_err(|e| e.to_string()),
    }
}

/// Rewrites the embedded XMP of a JPEG (temp file + rename; pixel data and the other
/// segments are byte-identical; mtime is restored).
fn write_embedded(path: &Path, cat: &CatalogValues) -> std::result::Result<bool, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let existing = jpeg::extract(&bytes);
    let mut x = parse_or_empty(existing.clone())
        .map_err(|e| format!("embedded XMP of {} left untouched: {e}", path.display()))?;
    let changed = apply_catalog(&mut x, cat).map_err(|e| e.to_string())?;
    if !changed && existing.is_some() {
        return Ok(false);
    }
    let out = jpeg::replace(&bytes, &x.serialize()).ok_or_else(|| {
        format!(
            "cannot embed XMP into {} (not a plain JPEG or the packet is too large)",
            path.display()
        )
    })?;
    // never write something we cannot read back, nor anything that touched other segments
    if jpeg::extract(&out).as_deref() != Some(x.serialize().as_str()) {
        return Err("embedded XMP did not round-trip; file left untouched".into());
    }
    let (old_segs, new_segs) = (
        jpeg::segments(&bytes).ok_or("unreadable JPEG")?,
        jpeg::segments(&out).ok_or("rewrite produced an unreadable JPEG")?,
    );
    let scan_old = old_segs.last().map(|s| s.end).unwrap_or(2);
    let scan_new = new_segs.last().map(|s| s.end).unwrap_or(2);
    if bytes[scan_old..] != out[scan_new..] {
        return Err("rewrite would change image data; file left untouched".into());
    }
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = dir.join(format!(
        ".{}.ip-tmp-{}",
        path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(),
        std::process::id()
    ));
    let res = (|| -> std::io::Result<()> {
        std::fs::write(&tmp, &out)?;
        if let Some(t) = mtime {
            std::fs::OpenOptions::new()
                .write(true)
                .open(&tmp)?
                .set_modified(t)?;
        }
        if let Ok(md) = std::fs::metadata(path) {
            let _ = std::fs::set_permissions(&tmp, md.permissions());
        }
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("write {}: {e}", path.display()));
    }
    Ok(true)
}

fn write_one(l: &Loaded, embedded: bool, force: bool) -> Outcome {
    let existing = find_sidecar(&l.path);
    let target = existing
        .clone()
        .unwrap_or_else(|| sidecar_candidates(&l.path)[0].clone());
    let mut text = None;
    if let Some(sc) = &existing {
        let m = mtime_ms(sc).unwrap_or(0);
        let raw = match std::fs::read(sc) {
            Ok(b) => String::from_utf8_lossy(&b).into_owned(),
            Err(e) => return Outcome::Error(format!("read {}: {e}", sc.display())),
        };
        let x = match Xmp::parse(raw.trim_end_matches('\0')) {
            Ok(x) => x,
            Err(e) => {
                return Outcome::Error(format!("sidecar {} left untouched: {e}", sc.display()))
            }
        };
        if !force && l.recorded_mtime.is_none_or(|r| m > r) {
            let sv = values_of(&x);
            if let Some(payload) = diff(&sv, &l.cat) {
                return Outcome::Conflict {
                    sidecar: sv,
                    mtime: m,
                    path: sc.clone(),
                    payload,
                };
            }
        }
        text = Some(raw);
    }
    let mut x = match parse_or_empty(text.clone()) {
        Ok(x) => x,
        Err(e) => return Outcome::Error(e),
    };
    let mut wrote = false;
    let changed = match apply_catalog(&mut x, &l.cat) {
        Ok(c) => c,
        Err(e) => return Outcome::Error(e.to_string()),
    };
    if changed || existing.is_none() {
        if let Err(e) = atomic_write(&target, x.serialize().as_bytes()) {
            return Outcome::Error(format!("write {}: {e}", target.display()));
        }
        wrote = true;
    }
    let mut err = None;
    if embedded && l.format == ImageFormat::Jpeg {
        match write_embedded(&l.path, &l.cat) {
            Ok(w) => wrote |= w,
            Err(e) => err = Some(e),
        }
    }
    if let Some(e) = err {
        return Outcome::Error(e);
    }
    let mtime = mtime_ms(&target);
    if wrote {
        Outcome::Written { mtime, path: target }
    } else {
        Outcome::Unchanged {
            mtime,
            path: existing,
        }
    }
}

impl Core {
    fn xmp_mode(&self) -> String {
        self.settings().xmp_mode.clone()
    }

    pub(crate) fn xmp_mode_changed(&self, mode: &str) {
        if mode == "off" {
            self.xmp.pending.lock().unwrap().clear();
        }
    }

    /// Sets the quiet period of the debounced writer (tests use a short one).
    pub fn set_xmp_debounce(&self, d: Duration) {
        self.xmp
            .debounce_ms
            .store(d.as_millis() as u64, Ordering::SeqCst);
    }

    /// Reads the XMP of freshly imported photos into the catalog (no write back).
    pub(crate) async fn xmp_import(self: &Arc<Self>, ids: Vec<i64>) {
        if self.xmp_mode() == "off" || ids.is_empty() {
            return;
        }
        let core = self.clone();
        let res = tokio::task::spawn_blocking(move || -> Result<Vec<PhotoUpdate>> {
            let mut updates = Vec::new();
            for chunk in ids.chunks(200) {
                let loaded = core.db.with(|c| load(c, chunk))?;
                for l in loaded {
                    let path = l.path.clone();
                    let format = l.format;
                    let Some((v, sc)) = read_values(&path, format) else {
                        continue;
                    };
                    core.db.with(|c| {
                        if let Some(u) = apply_sidecar(c, l.id, &v)? {
                            updates.push(u);
                        }
                        if let Some((p, m)) = &sc {
                            record_state(c, l.id, p, *m)?;
                        }
                        Ok(())
                    })?;
                }
            }
            Ok(updates)
        })
        .await;
        match res {
            Ok(Ok(updates)) if !updates.is_empty() => {
                self.events.emit(Event::PhotosUpdated { items: updates });
            }
            Ok(Ok(_)) => {}
            Ok(Err(e)) => tracing::warn!(error = %e, "reading XMP on import failed"),
            Err(e) => tracing::warn!(error = %e, "reading XMP on import crashed"),
        }
    }

    /// Queues photos whose rating/flag/label/keywords changed; sidecars are written after a
    /// quiet period (`xmp_mode` != off).
    pub(crate) fn xmp_touch(self: &Arc<Self>, ids: &[i64]) {
        if self.xmp_mode() == "off" || ids.is_empty() {
            return;
        }
        self.xmp.pending.lock().unwrap().extend(ids.iter().copied());
        if self.xmp.scheduled.swap(true, Ordering::SeqCst) {
            return;
        }
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                let Some(core) = weak.upgrade() else { return };
                let ms = core.xmp.debounce_ms.load(Ordering::SeqCst);
                drop(core);
                tokio::time::sleep(Duration::from_millis(ms)).await;
                let Some(core) = weak.upgrade() else { return };
                if let Err(e) = core.xmp_flush().await {
                    tracing::warn!(error = %e, "writing XMP failed");
                }
                core.xmp.scheduled.store(false, Ordering::SeqCst);
                // changes that arrived during the flush
                if core.xmp.pending.lock().unwrap().is_empty()
                    || core.xmp.scheduled.swap(true, Ordering::SeqCst)
                {
                    return;
                }
            }
        });
    }

    /// Writes everything queued by [`Core::xmp_touch`] now.
    pub async fn xmp_flush(self: &Arc<Self>) -> Result<XmpSyncReport> {
        let ids: Vec<i64> = std::mem::take(&mut *self.xmp.pending.lock().unwrap())
            .into_iter()
            .collect();
        let mode = self.xmp_mode();
        if ids.is_empty() || mode == "off" {
            return Ok(XmpSyncReport {
                direction: "write".into(),
                ..Default::default()
            });
        }
        self.xmp_write(ids, false, &mode).await
    }

    async fn xmp_write(
        self: &Arc<Self>,
        ids: Vec<i64>,
        force: bool,
        mode: &str,
    ) -> Result<XmpSyncReport> {
        let embedded = mode == "sidecar_and_embedded";
        let mut report = XmpSyncReport {
            direction: "write".into(),
            photos: ids.len(),
            ..Default::default()
        };
        let loaded = self.db.call(move |c| load(c, &ids)).await?;
        let outcomes = tokio::task::spawn_blocking(move || {
            loaded
                .into_iter()
                .map(|l| {
                    let o = write_one(&l, embedded, force);
                    (l.id, o)
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| CoreError::Internal(anyhow::anyhow!("xmp write task failed: {e}")))?;
        let mut updates = Vec::new();
        let mut conflicts: Vec<(i64, Value, Value)> = Vec::new();
        let mut db_work: Vec<(i64, Outcome)> = Vec::new();
        for (id, o) in outcomes {
            match &o {
                Outcome::Written { .. } => report.written += 1,
                Outcome::Conflict { .. } => report.conflicts += 1,
                Outcome::Error(e) => report.errors.push(format!("photo {id}: {e}")),
                Outcome::Unchanged { .. } => {}
            }
            db_work.push((id, o));
        }
        let applied = self
            .db
            .call(move |c| {
                let mut updates = Vec::new();
                let mut conflicts = Vec::new();
                for (id, o) in db_work {
                    match o {
                        Outcome::Written { mtime, path } | Outcome::Unchanged { mtime, path: Some(path) } => {
                            if let Some(m) = mtime {
                                record_state(c, id, &path, m)?;
                            }
                        }
                        Outcome::Conflict {
                            sidecar,
                            mtime,
                            path,
                            payload,
                            ..
                        } => {
                            if let Some(u) = apply_sidecar(c, id, &sidecar)? {
                                updates.push(u);
                            }
                            record_state(c, id, &path, mtime)?;
                            conflicts.push((id, payload.0, payload.1));
                        }
                        _ => {}
                    }
                }
                Ok((updates, conflicts))
            })
            .await?;
        updates.extend(applied.0);
        conflicts.extend(applied.1);
        if !updates.is_empty() {
            self.events.emit(Event::PhotosUpdated { items: updates });
        }
        for (photo_id, sidecar, catalog) in conflicts {
            self.events.emit(Event::XmpConflict {
                photo_id,
                sidecar,
                catalog,
            });
        }
        Ok(report)
    }

    /// `POST /api/xmp/sync`.
    pub async fn xmp_sync(self: &Arc<Self>, req: XmpSyncRequest) -> Result<XmpSyncReport> {
        let mode = self.xmp_mode();
        if mode == "off" {
            return Err(CoreError::Conflict(
                "XMP sync is off (settings: xmp_mode = \"off\")".into(),
            ));
        }
        if req.direction != "read" && req.direction != "write" {
            return Err(CoreError::bad_request("direction must be read or write"));
        }
        let sid = req.session_id;
        let ids: Vec<i64> = self
            .db
            .call(move |c| {
                catalog::get_session(c, sid)?;
                let mut st = c.prepare(
                    "SELECT photo_id FROM session_photo WHERE session_id=?1 ORDER BY photo_id",
                )?;
                let v = st
                    .query_map([sid], |r| r.get::<_, i64>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(v)
            })
            .await?;
        if req.direction == "write" {
            return self.xmp_write(ids, true, &mode).await;
        }
        let mut report = XmpSyncReport {
            direction: "read".into(),
            photos: ids.len(),
            ..Default::default()
        };
        let core = self.clone();
        let (updates, conflicts, read) = tokio::task::spawn_blocking(move || -> Result<_> {
            let mut updates = Vec::new();
            let mut conflicts = Vec::new();
            let mut read = 0usize;
            for chunk in ids.chunks(200) {
                let loaded = core.db.with(|c| load(c, chunk))?;
                for l in loaded {
                    let Some((v, sc)) = read_values(&l.path, l.format) else {
                        continue;
                    };
                    read += 1;
                    let payload = diff(&v, &l.cat);
                    core.db.with(|c| {
                        if let Some(u) = apply_sidecar(c, l.id, &v)? {
                            updates.push(u);
                        }
                        if let Some((p, m)) = &sc {
                            record_state(c, l.id, p, *m)?;
                        }
                        Ok(())
                    })?;
                    // a difference only counts as a conflict when the catalog had its own value
                    if let Some((s, c)) = payload {
                        let had_own = c.as_object().is_some_and(|o| {
                            o.values().any(|v| match v {
                                Value::Null => false,
                                Value::Number(n) => n.as_i64() != Some(0),
                                Value::Array(a) => !a.is_empty(),
                                _ => true,
                            })
                        });
                        if had_own {
                            conflicts.push((l.id, s, c));
                        }
                    }
                }
            }
            Ok((updates, conflicts, read))
        })
        .await
        .map_err(|e| CoreError::Internal(anyhow::anyhow!("xmp read task failed: {e}")))??;
        let _ = read;
        report.updated = updates.len();
        report.conflicts = conflicts.len();
        if !updates.is_empty() {
            self.events.emit(Event::PhotosUpdated { items: updates });
        }
        for (photo_id, sidecar, catalog) in conflicts {
            self.events.emit(Event::XmpConflict {
                photo_id,
                sidecar,
                catalog,
            });
        }
        Ok(report)
    }

    // ------------------------------------------------------------ keywords

    pub async fn photo_tags(&self, id: i64) -> Result<Vec<String>> {
        self.db
            .call(move |c| {
                catalog::photo_ref(c, id)?;
                tags_of(c, id)
            })
            .await
    }

    /// Adds/removes keywords on photos; written to XMP like ratings. Returns the number of
    /// photos that changed.
    pub async fn set_tags(
        self: &Arc<Self>,
        ids: Vec<i64>,
        add: Vec<String>,
        remove: Vec<String>,
    ) -> Result<usize> {
        if ids.is_empty() {
            return Err(CoreError::bad_request("ids must not be empty"));
        }
        let add = normalize_tags(add);
        let remove = normalize_tags(remove);
        if add.is_empty() && remove.is_empty() {
            return Err(CoreError::bad_request("give add and/or remove keywords"));
        }
        let ids2 = ids.clone();
        let changed = self
            .db
            .call(move |c| {
                let tx = c.transaction()?;
                let mut changed = Vec::new();
                for id in ids2 {
                    let exists: i64 =
                        tx.query_row("SELECT COUNT(*) FROM photo WHERE id=?1", [id], |r| r.get(0))?;
                    if exists == 0 {
                        continue;
                    }
                    let mut n = 0;
                    for t in &remove {
                        n += tx.execute(
                            "DELETE FROM photo_tag WHERE photo_id=?1 AND tag=?2",
                            params![id, t],
                        )?;
                    }
                    for t in &add {
                        n += tx.execute(
                            "INSERT OR IGNORE INTO photo_tag(photo_id, tag) VALUES(?1,?2)",
                            params![id, t],
                        )?;
                    }
                    if n > 0 {
                        changed.push(id);
                    }
                }
                tx.commit()?;
                Ok(changed)
            })
            .await?;
        self.xmp_touch(&changed);
        Ok(changed.len())
    }
}
