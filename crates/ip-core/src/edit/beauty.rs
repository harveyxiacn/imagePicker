//! Portrait retouching support (docs/api-contract-m4.md sections B and C.1): the geometry cache
//! fed by the worker's `beauty.prepare`, the `MaskProvider::people()` mapping, the people
//! endpoint, per-person beauty profiles and `apply-profiles`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};

use ip_render::{Mask, Op, PersonGeometry};
use ip_worker_client::{
    AiWorker, BeautyFaceRef, BeautyPerson, BeautyPhoto, BeautyPrepareRequest, BeautyPrepareResponse,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Map, Value};
use tokio::runtime::Handle;

use super::service::MaskStore;
use super::{ops_of, validate_stack};
use crate::analysis::map_worker_err;
use crate::analysis::store as astore;
use crate::analysis::types::Face;
use crate::catalog::{self, now_ms, PhotoRef};
use crate::db::Db;
use crate::error::{CoreError, Result};
use crate::events::Event;
use crate::Core;

/// Long edge of the skin/body masks requested from the worker.
pub const BEAUTY_SIZE: u32 = 1536;

fn faces_sig(faces: &[Face]) -> String {
    let mut s = String::new();
    for f in faces {
        s.push_str(&format!(
            "{}:{}:{}:{}:{};",
            f.id,
            (f.bbox[0] * 1000.0).round(),
            (f.bbox[1] * 1000.0).round(),
            (f.bbox[2] * 1000.0).round(),
            (f.bbox[3] * 1000.0).round()
        ));
    }
    blake3::hash(s.as_bytes()).to_hex()[..16].to_string()
}

/// `(content key, faces signature)` the geometry of a photo must match to be valid.
fn current_keys(conn: &Connection, photo_id: i64) -> Result<(String, String, Vec<Face>)> {
    let key: String = conn
        .query_row(
            "SELECT COALESCE(content_key, fast_key) FROM photo WHERE id=?1",
            [photo_id],
            |r| r.get(0),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::not_found(format!("photo {photo_id} not found"))
            }
            e => e.into(),
        })?;
    let faces = astore::faces_of_photo(conn, photo_id)?;
    Ok((key, faces_sig(&faces), faces))
}

/// Disk + DB cache of person geometry (`<cache>/beauty/<content key>/<photo>.json` + mask PNGs).
pub struct BeautyStore {
    db: Db,
    worker: Arc<dyn AiWorker>,
    root: PathBuf,
    masks: Arc<MaskStore>,
    gen_lock: Mutex<()>,
    /// Number of `beauty.prepare` requests sent to the worker (tests, diagnostics).
    pub prepares: AtomicUsize,
}

impl BeautyStore {
    pub fn new(db: Db, worker: Arc<dyn AiWorker>, root: PathBuf, masks: Arc<MaskStore>) -> Self {
        Self {
            db,
            worker,
            root,
            masks,
            gen_lock: Mutex::new(()),
            prepares: AtomicUsize::new(0),
        }
    }

    fn json_path(&self, key: &str, photo_id: i64) -> PathBuf {
        self.root.join(key).join(format!("{photo_id}.json"))
    }

    /// The cached geometry when it exists and still matches the photo and its faces.
    pub fn peek(&self, conn: &Connection, photo_id: i64) -> Result<Option<Vec<BeautyPerson>>> {
        let (key, sig, _) = current_keys(conn, photo_id)?;
        self.peek_with(conn, photo_id, &key, &sig)
    }

    fn peek_with(
        &self,
        conn: &Connection,
        photo_id: i64,
        key: &str,
        sig: &str,
    ) -> Result<Option<Vec<BeautyPerson>>> {
        let row: Option<(String, String)> = conn
            .query_row(
                "SELECT content_key, faces_sig FROM beauty_geometry WHERE photo_id=?1",
                [photo_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((k, s)) = row else { return Ok(None) };
        if k != key || s != sig {
            return Ok(None);
        }
        let Ok(bytes) = std::fs::read(self.json_path(key, photo_id)) else {
            return Ok(None);
        };
        let Ok(people) = serde_json::from_slice::<Vec<BeautyPerson>>(&bytes) else {
            return Ok(None);
        };
        // a deleted mask PNG invalidates the entry
        let masks_ok = people.iter().all(|p| {
            [&p.skin_mask, &p.body_mask]
                .into_iter()
                .flatten()
                .all(|m| Path::new(m).is_file())
        });
        Ok(masks_ok.then_some(people))
    }

    /// Geometry of a photo. With `generate`, a miss asks the worker (blocking, via `handle`);
    /// without it a miss yields `Ok(None)`.
    pub fn geometry(
        &self,
        r: &PhotoRef,
        handle: &Handle,
        generate: bool,
    ) -> Result<Option<Vec<BeautyPerson>>> {
        let (key, sig, faces) = self.db.with(|c| current_keys(c, r.id))?;
        if let Some(p) = self.db.with(|c| self.peek_with(c, r.id, &key, &sig))? {
            return Ok(Some(p));
        }
        if !generate {
            return Ok(None);
        }
        let _g = self.gen_lock.lock().unwrap();
        if let Some(p) = self.db.with(|c| self.peek_with(c, r.id, &key, &sig))? {
            return Ok(Some(p));
        }
        let dir = self.root.join(&key);
        std::fs::create_dir_all(&dir)?;
        let req = BeautyPrepareRequest {
            photo: BeautyPhoto {
                photo_id: r.id,
                path: r.path.to_string_lossy().into_owned(),
                orientation: r.orientation,
            },
            faces: faces
                .iter()
                .map(|f| BeautyFaceRef {
                    face_id: f.id,
                    bbox: f.bbox,
                })
                .collect(),
            size: BEAUTY_SIZE,
            out_dir: dir.to_string_lossy().into_owned(),
            allow_download: false,
        };
        self.prepares.fetch_add(1, Ordering::SeqCst);
        let resp: BeautyPrepareResponse = handle
            .block_on(self.worker.beauty_prepare(&req))
            .map_err(map_worker_err)?;
        let unavailable: Vec<String> = resp
            .skipped
            .iter()
            .filter(|(_, why)| why.as_str() == "model_unavailable")
            .map(|(k, _)| format!("beauty.{k}"))
            .collect();
        if resp.people.is_empty() && !unavailable.is_empty() {
            return Err(CoreError::ModelsMissing(unavailable));
        }
        let bytes = serde_json::to_vec(&resp.people)?;
        let path = self.json_path(&key, r.id);
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &path)?;
        let n = resp.people.len() as i64;
        let (id, k, s) = (r.id, key.clone(), sig.clone());
        self.db.with(move |c| {
            c.execute(
                "INSERT INTO beauty_geometry(photo_id, content_key, faces_sig, people, prepared_at)
                 VALUES(?1,?2,?3,?4,?5)
                 ON CONFLICT(photo_id) DO UPDATE SET content_key=?2, faces_sig=?3, people=?4, prepared_at=?5",
                params![id, k, s, n, now_ms()],
            )?;
            Ok(())
        })?;
        Ok(Some(resp.people))
    }

    fn load_mask(&self, path: &Option<String>) -> Option<Mask> {
        let p = path.as_deref()?;
        self.masks.load(Path::new(p)).ok().map(|m| (*m).clone())
    }

    /// Geometry in the renderer's shape, with `person_id` resolved through the face table.
    pub fn people(
        &self,
        r: &PhotoRef,
        handle: &Handle,
        generate: bool,
    ) -> Result<Vec<PersonGeometry>> {
        let Some(people) = self.geometry(r, handle, generate)? else {
            return Ok(Vec::new());
        };
        let rid = r.id;
        let owner: HashMap<i64, Option<i64>> = self.db.with(move |c| {
            Ok(astore::faces_of_photo(c, rid)?
                .into_iter()
                .map(|f| (f.id, f.person_id))
                .collect())
        })?;
        Ok(people
            .iter()
            .map(|p| PersonGeometry {
                person_id: p.face_id.and_then(|f| owner.get(&f).copied().flatten()),
                face_box: p.face_box,
                face_landmarks: p.face_landmarks.clone(),
                pose: p.pose.clone(),
                skin: self.load_mask(&p.skin_mask),
                body: self.load_mask(&p.body_mask),
                blemishes: p.blemishes.clone(),
            })
            .collect())
    }
}

// ------------------------------------------------------------------ wire types

#[derive(Debug, Clone, Serialize)]
pub struct PersonInfo {
    pub face_id: Option<i64>,
    pub person_id: Option<i64>,
    pub person_name: Option<String>,
    pub face_box: [f32; 4],
    pub is_subject: bool,
    pub has_pose: bool,
    pub has_profile: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PeopleOut {
    pub people: Vec<PersonInfo>,
    pub ready: bool,
}

// ------------------------------------------------------------------ profiles

const PROFILE_KEYS: [&str; 3] = ["beauty", "face", "body"];

/// Op JSON for one profile part (`beauty` | `face` | `body`) with `person_id` filled in.
fn part_op(part: &str, body: &Map<String, Value>, person_id: Option<i64>) -> Result<Value> {
    let mut m = body.clone();
    m.remove("person_id");
    m.remove("type");
    m.remove("kind");
    match part {
        "beauty" => m.insert("type".into(), json!("beauty")),
        "face" => {
            m.insert("kind".into(), json!("face"));
            m.insert("type".into(), json!("warp"))
        }
        _ => {
            m.insert("kind".into(), json!("body"));
            m.insert("type".into(), json!("warp"))
        }
    };
    if let Some(p) = person_id {
        m.insert("person_id".into(), json!(p));
    }
    // range-check through the normal validator; the typed op re-serialises with defaults filled
    let checked = validate_stack(json!({"version": 1, "ops": [Value::Object(m)]}))?;
    use ip_render::Warp;
    match (part, checked.stack.ops.first()) {
        ("beauty", Some(op @ Op::Beauty(_)))
        | ("face", Some(op @ Op::Warp(Warp::Face { .. })))
        | ("body", Some(op @ Op::Warp(Warp::Body { .. }))) => Ok(serde_json::to_value(op)?),
        _ => Err(CoreError::Unprocessable(format!(
            "profile.{part} is not a valid {part} setting"
        ))),
    }
}

/// Validates a profile and returns its canonical stored form (`None` = no settings at all).
pub fn normalize_profile(profile: &Value) -> Result<Option<Value>> {
    let obj = profile
        .as_object()
        .ok_or_else(|| CoreError::Unprocessable("profile must be an object or null".into()))?;
    if let Some(k) = obj.keys().find(|k| !PROFILE_KEYS.contains(&k.as_str())) {
        return Err(CoreError::Unprocessable(format!(
            "unknown profile key {k:?} (expected beauty, face, body)"
        )));
    }
    let mut out = Map::new();
    for part in PROFILE_KEYS {
        match obj.get(part) {
            None | Some(Value::Null) => {}
            Some(Value::Object(body)) => {
                let op = part_op(part, body, None)?;
                let mut m = op.as_object().cloned().unwrap_or_default();
                m.remove("type");
                m.remove("kind");
                m.remove("person_id");
                out.insert(part.to_string(), Value::Object(m));
            }
            Some(_) => {
                return Err(CoreError::Unprocessable(format!(
                    "profile.{part} must be an object or null"
                )))
            }
        }
    }
    Ok((!out.is_empty()).then_some(Value::Object(out)))
}

/// The ops a stored profile stands for, bound to `person_id`.
pub fn profile_ops(profile: &Value, person_id: i64) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for part in PROFILE_KEYS {
        if let Some(Value::Object(body)) = profile.get(part) {
            out.push(part_op(part, body, Some(person_id))?);
        }
    }
    Ok(out)
}

fn same_slot(a: &Value, b: &Value) -> bool {
    let key = |v: &Value| {
        (
            v.get("type").and_then(Value::as_str).map(str::to_string),
            v.get("kind").and_then(Value::as_str).map(str::to_string),
            v.get("person_id").and_then(Value::as_i64),
        )
    };
    key(a) == key(b)
}

// ------------------------------------------------------------------ Core API

impl Core {
    /// `GET /api/photos/{id}/people`.
    pub async fn photo_people(&self, photo_id: i64) -> Result<PeopleOut> {
        let store = self.render.beauty.clone();
        self.db
            .call(move |c| {
                let faces = astore::faces_of_photo(c, photo_id)?;
                catalog::photo_ref(c, photo_id)?; // 404
                let profiles: HashSet<i64> = {
                    let mut st =
                        c.prepare("SELECT id FROM person WHERE beauty_profile IS NOT NULL")?;
                    let v = st
                        .query_map([], |r| r.get::<_, i64>(0))?
                        .collect::<rusqlite::Result<HashSet<i64>>>()?;
                    v
                };
                let by_id: HashMap<i64, &Face> = faces.iter().map(|f| (f.id, f)).collect();
                let info = |face: Option<&Face>, b: Option<&BeautyPerson>| PersonInfo {
                    face_id: face.map(|f| f.id),
                    person_id: face.and_then(|f| f.person_id),
                    person_name: face.and_then(|f| f.person_name.clone()),
                    face_box: match (b, face) {
                        (Some(b), _) => b.face_box,
                        (None, Some(f)) => [
                            f.bbox[0] as f32,
                            f.bbox[1] as f32,
                            f.bbox[2] as f32,
                            f.bbox[3] as f32,
                        ],
                        _ => [0.0; 4],
                    },
                    is_subject: face.map(|f| f.is_subject).unwrap_or(false),
                    has_pose: b.map(|b| !b.pose.is_empty()).unwrap_or(false),
                    has_profile: face
                        .and_then(|f| f.person_id)
                        .map(|p| profiles.contains(&p))
                        .unwrap_or(false),
                };
                Ok(match store.peek(c, photo_id)? {
                    Some(geo) => PeopleOut {
                        people: geo
                            .iter()
                            .map(|b| info(b.face_id.and_then(|i| by_id.get(&i).copied()), Some(b)))
                            .collect(),
                        ready: true,
                    },
                    None => PeopleOut {
                        people: faces.iter().map(|f| info(Some(f), None)).collect(),
                        ready: false,
                    },
                })
            })
            .await
    }

    /// `POST /api/photos/{id}/beauty/prepare`: starts the preparation, returns the task id.
    pub async fn beauty_prepare(self: &Arc<Self>, photo_id: i64) -> Result<String> {
        let r = self
            .db
            .call(move |c| catalog::photo_ref(c, photo_id))
            .await?;
        // models first, so the client gets its 409 right away instead of a failed task
        let listing = self.worker.models_list().await.map_err(map_worker_err)?;
        let missing: Vec<String> = listing
            .models
            .iter()
            .filter(|m| !m.installed && m.required_for.iter().any(|s| s == "beauty"))
            .map(|m| m.id.clone())
            .collect();
        if !missing.is_empty() {
            return Err(CoreError::ModelsMissing(missing));
        }
        let task_id = format!("beauty-{}", self.next_task_seq());
        let (core, tid) = (self.clone(), task_id.clone());
        let ev = move |done: i64, state: &str, error: Option<String>| Event::TaskProgress {
            task_id: tid.clone(),
            kind: "beauty_prepare".into(),
            done,
            total: 1,
            state: state.into(),
            error,
        };
        self.events.emit(ev(0, "running", None));
        tokio::spawn(async move {
            let svc = core.render.clone();
            let handle = Handle::current();
            let r2 = r.clone();
            let res = tokio::task::spawn_blocking(move || {
                svc.beauty.geometry(&r2, &handle, true).map(|_| ())
            })
            .await
            .map_err(|e| CoreError::Internal(anyhow::anyhow!("beauty task failed: {e}")))
            .and_then(|x| x);
            match res {
                Ok(()) => {
                    core.after_beauty_ready(&r).await;
                    core.events.emit(Event::BeautyReady { photo_id });
                    core.events.emit(ev(1, "done", None));
                }
                Err(e) => core.events.emit(ev(0, "failed", Some(e.to_string()))),
            }
        });
        Ok(task_id)
    }

    /// Edited thumbnails rendered before the geometry existed lack the portrait ops: redo them.
    async fn after_beauty_ready(self: &Arc<Self>, r: &PhotoRef) {
        let id = r.id;
        let cur = self
            .db
            .call(move |c| crate::edit::store::current(c, id))
            .await
            .ok()
            .flatten();
        let Some(cur) = cur else { return };
        let portrait = ops_of(&cur.stack).iter().any(|o| {
            matches!(
                super::sync::op_type(o),
                Some("beauty") | Some("warp")
            )
        });
        if portrait && cur.has_edits {
            self.render
                .purge_edited(&crate::edit::edited_key(&r.fast_key, &cur.hash));
            self.spawn_edit_refresh(vec![id]);
        }
    }

    // -------------------------------------------------------------- profiles

    pub async fn beauty_profile(&self, person_id: i64) -> Result<Option<Value>> {
        self.db
            .call(move |c| {
                astore::get_person(c, person_id)?; // 404
                let raw: Option<String> = c.query_row(
                    "SELECT beauty_profile FROM person WHERE id=?1",
                    [person_id],
                    |r| r.get(0),
                )?;
                Ok(raw.and_then(|s| serde_json::from_str(&s).ok()))
            })
            .await
    }

    /// `None` (or an empty profile) deletes the profile. Returns what is stored afterwards.
    pub async fn set_beauty_profile(
        &self,
        person_id: i64,
        profile: Option<Value>,
    ) -> Result<Option<Value>> {
        let norm = match &profile {
            None | Some(Value::Null) => None,
            Some(p) => normalize_profile(p)?,
        };
        let n2 = norm.clone();
        self.db
            .call(move |c| {
                astore::get_person(c, person_id)?; // 404
                c.execute(
                    "UPDATE person SET beauty_profile=?2 WHERE id=?1",
                    params![person_id, n2.map(|v| v.to_string())],
                )?;
                Ok(())
            })
            .await?;
        Ok(norm)
    }

    /// `POST /api/edits/apply-profiles`: for every photo, replaces/inserts the beauty and warp
    /// ops of each person that has a profile. Returns the number of photos a profile applied to.
    pub async fn apply_profiles(self: &Arc<Self>, photo_ids: Vec<i64>) -> Result<usize> {
        if photo_ids.len() > 20_000 {
            return Err(CoreError::bad_request("too many photo_ids"));
        }
        let mut seen = HashSet::new();
        let ids: Vec<i64> = photo_ids.into_iter().filter(|i| seen.insert(*i)).collect();
        let plan = self
            .db
            .call(move |c| {
                let profiles: BTreeMap<i64, Value> = {
                    let mut st = c
                        .prepare("SELECT id, beauty_profile FROM person WHERE beauty_profile IS NOT NULL")?;
                    let v = st
                        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    v.into_iter()
                        .filter_map(|(id, s)| serde_json::from_str(&s).ok().map(|p| (id, p)))
                        .collect()
                };
                let mut out = Vec::new();
                if profiles.is_empty() {
                    return Ok(out);
                }
                for r in catalog::photo_refs(c, &ids)? {
                    let people: Vec<i64> = {
                        let mut v: Vec<i64> = astore::faces_of_photo(c, r.id)?
                            .iter()
                            .filter_map(|f| f.person_id)
                            .filter(|p| profiles.contains_key(p))
                            .collect();
                        v.sort_unstable();
                        v.dedup();
                        v
                    };
                    if people.is_empty() {
                        continue;
                    }
                    let cur = crate::edit::store::current(c, r.id)?;
                    let mut new_ops = Vec::new();
                    for p in &people {
                        new_ops.extend(profile_ops(&profiles[p], *p)?);
                    }
                    out.push((r, cur.map(|c| ops_of(&c.stack)).unwrap_or_default(), new_ops));
                }
                Ok(out)
            })
            .await?;
        let applied = plan.len();
        let mut writes = Vec::new();
        for (r, old, new_ops) in plan {
            let mut ops: Vec<Value> = old
                .iter()
                .filter(|o| !new_ops.iter().any(|n| same_slot(n, o)))
                .cloned()
                .collect();
            ops.extend(new_ops);
            ops.sort_by_key(super::sync::rank);
            if ops != old {
                writes.push((r, ops));
            }
        }
        self.persist_stacks(writes).await?;
        Ok(applied)
    }
}
