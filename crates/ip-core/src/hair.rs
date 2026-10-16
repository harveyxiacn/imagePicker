//! Hair transfer, "换成我最好的发型" (docs/03 section 5.1, docs/api-contract-m5.md section H):
//! replace a person's hair in one photo with their own hair from another photo, typically an
//! earlier frame of the same day before heat and sweat flattened it.
//!
//! - `GET /api/photos/{id}/hair/donors?face_id=` lists the same person's faces in other photos
//!   (same session first), each with the pose gate's verdict and, when the worker can measure
//!   them, a hair-quality score and the cues behind it ([`score_donors`]).
//! - `POST /api/photos/{id}/hair` composes one donor's hair onto the face as a `patch` op of kind
//!   `hair` (one per face; `amount` = strength), like a best take: asset + rect + the donor photo
//!   / face ids and the alignment the worker used. Undo, re-render and export go through the
//!   normal edit stack.
//!
//! The donor must be the same person ([`check_hair_identity`], enforced here, not only in the
//! UI) and in a similar pose ([`pose_reason`]).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use ip_worker_client::{HairComposeRequest, HairMetrics, HairRankFace, HairRankRequest};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::analysis::map_worker_err;
use crate::analysis::store::{self as astore, FaceIdentity};
use crate::analysis::types::Face;
use crate::analysis::vecs::{cosine, decode_f16};
use crate::catalog::{self, PhotoRef};
use crate::error::{CoreError, Result};
use crate::events::Event;
use crate::generate::{mkphoto, patch_op, reason_of, MIN_IDENTITY_COS, PATCH_FEATHER};
use crate::tasks::{TaskHandle, CANCELLED, DONE, FAILED};
use crate::Core;

/// Pose gates (degrees, absolute difference donor vs target). Hair hangs with gravity and is seen
/// from one side, so a 2-D similarity warp cannot fix a turned or tilted head. Same values as the
/// worker's `HAIR_MAX_*` (`ai-worker/imagepicker_ai/besttake/hair.py`).
pub const HAIR_MAX_YAW: f64 = 20.0;
pub const HAIR_MAX_PITCH: f64 = 15.0;
pub const HAIR_MAX_ROLL: f64 = 20.0;
/// Donor face width / target face width below this: the donor hair would be upscaled more than
/// 2x (`donor_too_small`).
pub const MIN_DONOR_SCALE: f64 = 0.5;
/// Least identity cosine for a donor that is linked to the target by neither person nor face
/// track (an unclustered face). Stricter than [`MIN_IDENTITY_COS`], which only guards linked
/// faces against clustering mistakes: AuraFace scores the same person 0.43-0.66.
pub const HAIR_MIN_COS: f32 = 0.40;
/// Most candidates one donor listing returns, and how many of them the worker measures.
pub const MAX_DONORS: usize = 40;
pub const MAX_RANKED: usize = 16;

// ------------------------------------------------------------------ wire types

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct PoseDelta {
    pub yaw: f64,
    pub pitch: f64,
    pub roll: f64,
}

/// One candidate donor face.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HairDonor {
    pub photo_id: i64,
    pub face_id: i64,
    pub file_name: String,
    pub taken_at: Option<i64>,
    /// Minutes between the donor and the target photo (negative = earlier), when both are dated.
    pub minutes_from_target: Option<i64>,
    pub same_session: bool,
    /// False when the pose gate refuses it (`reason`).
    pub composable: bool,
    /// `pose_too_different` | `donor_too_small` | `hair_not_found` (measured: no hair)
    pub reason: Option<String>,
    pub pose_delta: PoseDelta,
    /// Donor face width / target face width (photo pixels), when both are known.
    pub scale: Option<f64>,
    /// Hair-quality score 0-1 ([`score_donors`]); `None` when not measured.
    pub score: Option<f64>,
    /// Why it ranks where it does: `fuller`, `less_shine`, `tidier`, `sharper`, `similar_pose`,
    /// `earlier`, `same_session`.
    pub cues: Vec<String>,
    pub metrics: Option<HairMetrics>,
}

/// `GET /api/photos/{id}/hair/donors` response.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HairDonors {
    pub photo_id: i64,
    pub face_id: i64,
    pub person_id: Option<i64>,
    /// Whether the worker measured the hair (scores and cues present).
    pub ranked: bool,
    /// Why not: `models_missing` | `worker_unavailable` | an error message.
    pub rank_error: Option<String>,
    /// The target face's own hair cues (for "your hair here" in the UI).
    pub target: Option<HairMetrics>,
    pub candidates: Vec<HairDonor>,
}

/// Body of `POST /api/photos/{id}/hair`.
#[derive(Debug, Clone, Deserialize)]
pub struct HairBody {
    pub face_id: i64,
    pub donor_photo_id: i64,
    pub donor_face_id: i64,
    /// Patch opacity 0..1 (default 1); can be changed later in the edit stack (`amount`).
    #[serde(default)]
    pub strength: Option<f32>,
}

// ------------------------------------------------------------------ pure parts

fn wrap180(d: f64) -> f64 {
    (d + 180.0).rem_euclid(360.0) - 180.0
}

/// Absolute yaw / pitch / roll differences (unknown angles count as 0; pitch and roll wrap).
pub fn pose_delta(a: [Option<f64>; 3], b: [Option<f64>; 3]) -> PoseDelta {
    let d = |x: Option<f64>, y: Option<f64>, wrap: f64| match (x, y) {
        (Some(x), Some(y)) => {
            let v = x - y;
            if wrap > 0.0 {
                ((v + wrap / 2.0).rem_euclid(wrap) - wrap / 2.0).abs()
            } else {
                v.abs()
            }
        }
        _ => 0.0,
    };
    PoseDelta {
        yaw: d(a[0], b[0], 0.0),
        pitch: d(a[1], b[1], 180.0),
        roll: wrap180(d(a[2], b[2], 0.0)).abs(),
    }
}

fn angles(f: &Face) -> [Option<f64>; 3] {
    [f.yaw, f.pitch, f.roll]
}

fn metric_angles(m: &HairMetrics) -> [Option<f64>; 3] {
    [m.yaw, m.pitch, m.roll]
}

/// Why a donor cannot be used (`None` = composable).
pub fn pose_reason(d: &PoseDelta, scale: Option<f64>) -> Option<&'static str> {
    if d.yaw > HAIR_MAX_YAW || d.pitch > HAIR_MAX_PITCH || d.roll > HAIR_MAX_ROLL {
        Some("pose_too_different")
    } else if scale.is_some_and(|s| s < MIN_DONOR_SCALE) {
        Some("donor_too_small")
    } else {
        None
    }
}

/// The guard of `POST /api/photos/{id}/hair`: the donor face must be the target's person. 422s:
/// - `hair_person_mismatch`: the faces belong to different people (person ids differ and they are
///   not in one face track), or nothing links them and there is no identity embedding to compare;
/// - `hair_low_similarity`: both have identity embeddings and the cosine is below
///   [`MIN_IDENTITY_COS`] (linked faces) or [`HAIR_MIN_COS`] (unlinked faces).
pub fn check_hair_identity(t: &FaceIdentity, d: &FaceIdentity) -> Result<()> {
    let (tf, df) = (t.face_id, d.face_id);
    let same_track = t.track_id.is_some() && t.track_id == d.track_id;
    let same_person = t.person_id.is_some() && t.person_id == d.person_id;
    let mismatch = || {
        CoreError::UnprocessableCode(
            "hair_person_mismatch",
            format!("face {df} is not the same person as face {tf}"),
        )
    };
    if !same_track && t.person_id.is_some() && d.person_id.is_some() && !same_person {
        return Err(mismatch());
    }
    let linked = same_track || same_person;
    match (&t.embedding, &d.embedding) {
        (Some(x), Some(y)) => {
            let cos = cosine(x, y);
            let need = if linked {
                MIN_IDENTITY_COS
            } else {
                HAIR_MIN_COS
            };
            if cos < need {
                return Err(CoreError::UnprocessableCode(
                    "hair_low_similarity",
                    format!(
                        "face {df} does not look like face {tf} (identity similarity {cos:.2}, at least {need:.2} needed)"
                    ),
                ));
            }
            Ok(())
        }
        _ if linked => Ok(()),
        _ => Err(mismatch()),
    }
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    v.retain(|x| x.is_finite());
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// Scores and cues of the measured donors (docs/03 section 5.1). Per donor with usable metrics:
///
/// `score = (0.35 volume + 0.25 dryness + 0.20 tidiness + 0.20 definition) * pose`, where
/// volume = hair area relative to the person's median (target + donors), capped at 1.3x;
/// dryness = 1 - shine / 5 %; tidiness = 1 - roughness / 30 %; definition = sharpness inside the
/// hair relative to the median, capped at 1.5x; pose = 1 - 0.3 x (largest angle / its gate).
///
/// Cues compare with the target's own hair when it was measured: `fuller` (area +8 %),
/// `less_shine` (shine below 70 % of the target's and at least 0.3 points lower), `tidier`
/// (roughness 3 points lower), `sharper` (+15 %); plus `similar_pose` (every angle within 8
/// degrees), `earlier` and `same_session`.
pub fn score_donors(target: Option<&HairMetrics>, donors: &mut [HairDonor]) {
    let ok = |m: &&HairMetrics| m.ok;
    let all: Vec<&HairMetrics> = donors
        .iter()
        .filter_map(|d| d.metrics.as_ref())
        .chain(target)
        .filter(ok)
        .collect();
    let med_r = median(all.iter().filter_map(|m| m.hair_ratio).collect());
    let med_s = median(all.iter().filter_map(|m| m.sharpness).collect());
    let t = target.filter(|m| m.ok);
    for d in donors.iter_mut() {
        let mut cues = Vec::new();
        if let Some(m) = d.metrics.as_ref().filter(|m| m.ok) {
            let vol = match (m.hair_ratio, med_r) {
                (Some(r), Some(med)) if med > 1e-6 => (r / med).clamp(0.0, 1.3) / 1.3,
                _ => 0.5,
            };
            let dry = m.shine.map_or(0.5, |s| 1.0 - (s / 0.05).clamp(0.0, 1.0));
            let tidy = m.roughness.map_or(0.5, |r| 1.0 - (r / 0.3).clamp(0.0, 1.0));
            let sharp = match (m.sharpness, med_s) {
                (Some(s), Some(med)) if med > 1e-6 => (s / med).clamp(0.0, 1.5) / 1.5,
                _ => 0.5,
            };
            let p = &d.pose_delta;
            let worst = (p.yaw / HAIR_MAX_YAW)
                .max(p.pitch / HAIR_MAX_PITCH)
                .max(p.roll / HAIR_MAX_ROLL)
                .min(1.0);
            let pose = 1.0 - 0.3 * worst;
            d.score = Some(round3(
                (0.35 * vol + 0.25 * dry + 0.2 * tidy + 0.2 * sharp) * pose,
            ));
            let gt = |a: Option<f64>, b: Option<f64>, f: &dyn Fn(f64, f64) -> bool| match (a, b) {
                (Some(a), Some(b)) => f(a, b),
                _ => false,
            };
            let (tr, ts, tro, tsh) = match t {
                Some(t) => (t.hair_ratio, t.shine, t.roughness, t.sharpness),
                None => (med_r, None, None, med_s),
            };
            if gt(m.hair_ratio, tr, &|a, b| a >= b * 1.08) {
                cues.push("fuller");
            }
            if gt(m.shine, ts, &|a, b| a < b * 0.7 && b - a >= 0.003) {
                cues.push("less_shine");
            }
            if gt(m.roughness, tro, &|a, b| a < b - 0.03) {
                cues.push("tidier");
            }
            if gt(m.sharpness, tsh, &|a, b| a > b * 1.15) {
                cues.push("sharper");
            }
        } else if let Some(m) = d.metrics.as_ref() {
            // measured, but no usable hair (no face found, no hair mask)
            if m.reason.as_deref() == Some("hair_not_found") && d.composable {
                d.composable = false;
                d.reason = Some("hair_not_found".into());
            }
        }
        let p = &d.pose_delta;
        if p.yaw.max(p.pitch).max(p.roll) <= 8.0 {
            cues.push("similar_pose");
        }
        if d.minutes_from_target.is_some_and(|m| m < 0) {
            cues.push("earlier");
        }
        if d.same_session {
            cues.push("same_session");
        }
        d.cues = cues.into_iter().map(String::from).collect();
    }
}

/// Final order: same session first, composable first, then score (unmeasured last), then the
/// closest in time.
pub fn sort_donors(donors: &mut [HairDonor]) {
    donors.sort_by(|a, b| {
        b.same_session
            .cmp(&a.same_session)
            .then(b.composable.cmp(&a.composable))
            .then(
                b.score
                    .unwrap_or(-1.0)
                    .partial_cmp(&a.score.unwrap_or(-1.0))
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(
                a.minutes_from_target
                    .map_or(i64::MAX, i64::abs)
                    .cmp(&b.minutes_from_target.map_or(i64::MAX, i64::abs)),
            )
            .then(a.photo_id.cmp(&b.photo_id))
    });
}

// ------------------------------------------------------------------ catalog reads

struct Candidate {
    face: Face,
    file_name: String,
    taken_at: Option<i64>,
    width: Option<i64>,
}

struct Gathered {
    target: Face,
    target_ref: PhotoRef,
    target_identity: FaceIdentity,
    candidates: Vec<Candidate>,
    session_photos: HashSet<i64>,
}

fn gather(c: &rusqlite::Connection, photo_id: i64, face_id: i64) -> Result<Gathered> {
    let target_ref = catalog::photo_ref(c, photo_id)?;
    let target = astore::get_face(c, face_id)?;
    if target.photo_id != photo_id {
        return Err(CoreError::Unprocessable(format!(
            "face {face_id} is not in photo {photo_id}"
        )));
    }
    let tid = astore::face_identity(c, face_id)?;
    let mut st = c.prepare(
        "SELECT DISTINCT b.photo_id FROM session_photo a JOIN session_photo b ON b.session_id=a.session_id
         WHERE a.photo_id=?1",
    )?;
    let session_photos = st
        .query_map([photo_id], |r| r.get::<_, i64>(0))?
        .collect::<rusqlite::Result<HashSet<i64>>>()?;
    // same person / same track; an unclustered face: same-session faces with an embedding
    let linked = tid.person_id.is_some() || tid.track_id.is_some();
    let mut st = c.prepare(
        "SELECT f.id, f.embedding, p.file_name, p.taken_at, p.width
         FROM face f JOIN photo p ON p.id=f.photo_id
         WHERE f.photo_id<>?1 AND COALESCE(p.missing,0)=0 AND (
           (?2 IS NOT NULL AND f.person_id=?2) OR (?3 IS NOT NULL AND f.track_id=?3)
           OR (?4=0 AND f.embedding IS NOT NULL AND f.photo_id IN
               (SELECT b.photo_id FROM session_photo a JOIN session_photo b ON b.session_id=a.session_id WHERE a.photo_id=?1)))",
    )?;
    let rows = st
        .query_map(
            rusqlite::params![photo_id, tid.person_id, tid.track_id, i64::from(linked)],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Option<Vec<u8>>>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                ))
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut candidates = Vec::new();
    let mut seen_photos = HashSet::new();
    for (fid, emb, file_name, taken_at, width) in rows {
        let emb = emb.filter(|b| !b.is_empty()).map(|b| decode_f16(&b));
        if let (Some(x), Some(y)) = (&tid.embedding, &emb) {
            let need = if linked {
                MIN_IDENTITY_COS
            } else {
                HAIR_MIN_COS
            };
            if cosine(x, y) < need {
                continue; // a clustering mistake, or somebody else
            }
        } else if !linked {
            continue;
        }
        let face = astore::get_face(c, fid)?;
        if !seen_photos.insert(face.photo_id) {
            continue; // one face per photo (the person can only be in a photo once)
        }
        candidates.push(Candidate {
            face,
            file_name,
            taken_at,
            width,
        });
    }
    Ok(Gathered {
        target,
        target_ref,
        target_identity: tid,
        candidates,
        session_photos,
    })
}

fn face_px(f: &Face, width: Option<i64>) -> Option<f64> {
    width.filter(|w| *w > 0).map(|w| f.bbox[2] * w as f64)
}

fn rank_error(e: &CoreError) -> String {
    match e {
        CoreError::ModelsMissing(_) => "models_missing".into(),
        CoreError::WorkerUnavailable(_) | CoreError::WorkerTimeout(_) => {
            "worker_unavailable".into()
        }
        e => e.to_string(),
    }
}

// ------------------------------------------------------------------ Core API

impl Core {
    /// `GET /api/photos/{id}/hair/donors?face_id=&rank=`: the same person's faces in other
    /// photos, gated by pose and (with `rank`, the default) scored by the worker's hair cues.
    pub async fn hair_donors(&self, photo_id: i64, face_id: i64, rank: bool) -> Result<HairDonors> {
        let g = self.db.call(move |c| gather(c, photo_id, face_id)).await?;
        let t_px = face_px(&g.target, g.target_ref.width.map(i64::from));
        let t_time = g.target_ref.taken_at;
        let mut donors: Vec<HairDonor> = g
            .candidates
            .iter()
            .map(|c| {
                let pd = pose_delta(angles(&g.target), angles(&c.face));
                let scale = match (face_px(&c.face, c.width), t_px) {
                    (Some(d), Some(t)) if t > 0.0 => Some(round3(d / t)),
                    _ => None,
                };
                let reason = pose_reason(&pd, scale);
                HairDonor {
                    photo_id: c.face.photo_id,
                    face_id: c.face.id,
                    file_name: c.file_name.clone(),
                    taken_at: c.taken_at,
                    minutes_from_target: match (c.taken_at, t_time) {
                        (Some(a), Some(b)) => Some((a - b).div_euclid(60_000)),
                        _ => None,
                    },
                    same_session: g.session_photos.contains(&c.face.photo_id),
                    composable: reason.is_none(),
                    reason: reason.map(String::from),
                    pose_delta: pd,
                    scale,
                    score: None,
                    cues: Vec::new(),
                    metrics: None,
                }
            })
            .collect();
        sort_donors(&mut donors);
        donors.truncate(MAX_DONORS);

        let (mut ranked, mut rank_err, mut target_m) = (false, None, None);
        if rank && !donors.is_empty() {
            let pick: Vec<usize> = (0..donors.len())
                .filter(|&i| donors[i].composable)
                .take(MAX_RANKED)
                .collect();
            let refs: HashMap<i64, PhotoRef> = {
                let ids: Vec<i64> = pick.iter().map(|&i| donors[i].photo_id).collect();
                self.db
                    .call(move |c| catalog::photo_refs(c, &ids))
                    .await?
                    .into_iter()
                    .map(|r| (r.id, r))
                    .collect()
            };
            let mut faces = vec![HairRankFace {
                photo: mkphoto(&g.target_ref),
                face: g.target.bbox,
            }];
            let mut order = Vec::new();
            for &i in &pick {
                if let Some(r) = refs.get(&donors[i].photo_id) {
                    let bbox = g
                        .candidates
                        .iter()
                        .find(|c| c.face.id == donors[i].face_id)
                        .map(|c| c.face.bbox)
                        .unwrap_or_default();
                    faces.push(HairRankFace {
                        photo: mkphoto(r),
                        face: bbox,
                    });
                    order.push(i);
                }
            }
            if !order.is_empty() {
                let req = HairRankRequest {
                    faces,
                    allow_download: false,
                };
                match self.worker.hair_rank(&req).await.map_err(map_worker_err) {
                    Ok(resp) if resp.results.len() == req.faces.len() => {
                        ranked = true;
                        let mut it = resp.results.into_iter();
                        target_m = it.next();
                        for (i, m) in order.into_iter().zip(it) {
                            donors[i].metrics = Some(m);
                        }
                    }
                    Ok(_) => rank_err = Some("hair.rank returned a wrong count".to_string()),
                    Err(e) => rank_err = Some(rank_error(&e)),
                }
            }
        }
        // the worker's landmark pose fills in angles the catalog does not have (lite profile)
        if let Some(tm) = target_m.as_ref().filter(|m| m.ok) {
            let ta = angles(&g.target);
            for d in donors.iter_mut() {
                let Some(m) = d.metrics.as_ref().filter(|m| m.ok) else {
                    continue;
                };
                let face = g.candidates.iter().find(|c| c.face.id == d.face_id);
                let da = face.map(|c| angles(&c.face)).unwrap_or([None; 3]);
                let (mt, md) = (metric_angles(tm), metric_angles(m));
                // per angle: the catalog's pair when both faces have it, else the worker's pair
                let pick = |i: usize| {
                    if ta[i].is_some() && da[i].is_some() {
                        (ta[i], da[i])
                    } else {
                        (mt[i], md[i])
                    }
                };
                let (y, p, r) = (pick(0), pick(1), pick(2));
                d.pose_delta = pose_delta([y.0, p.0, r.0], [y.1, p.1, r.1]);
                let reason = pose_reason(&d.pose_delta, d.scale);
                d.composable = reason.is_none();
                d.reason = reason.map(String::from);
            }
        }
        score_donors(target_m.as_ref(), &mut donors);
        sort_donors(&mut donors);
        Ok(HairDonors {
            photo_id,
            face_id,
            person_id: g.target_identity.person_id,
            ranked,
            rank_error: rank_err,
            target: target_m,
            candidates: donors,
        })
    }

    /// `POST /api/photos/{id}/hair`: validates the pair (same person, similar pose) and starts
    /// the task.
    pub async fn hair_start(self: &Arc<Self>, photo_id: i64, body: HairBody) -> Result<String> {
        let strength = body.strength.unwrap_or(1.0);
        if !strength.is_finite() || !(0.0..=1.0).contains(&strength) {
            return Err(CoreError::bad_request("strength must be within 0..1"));
        }
        if body.donor_photo_id == photo_id {
            return Err(CoreError::Unprocessable(
                "the donor photo must differ from the photo".into(),
            ));
        }
        let b = body.clone();
        let (target_ref, donor_ref, tf, df) = self
            .db
            .call(move |c| {
                let tr = catalog::photo_ref(c, photo_id)?;
                let dr = catalog::photo_ref(c, b.donor_photo_id)?;
                let tf = astore::get_face(c, b.face_id)?;
                let df = astore::get_face(c, b.donor_face_id)?;
                if tf.photo_id != photo_id {
                    return Err(CoreError::Unprocessable(format!(
                        "face {} is not in photo {photo_id}",
                        b.face_id
                    )));
                }
                if df.photo_id != b.donor_photo_id {
                    return Err(CoreError::Unprocessable(format!(
                        "face {} is not in photo {}",
                        b.donor_face_id, b.donor_photo_id
                    )));
                }
                check_hair_identity(
                    &astore::face_identity(c, b.face_id)?,
                    &astore::face_identity(c, b.donor_face_id)?,
                )?;
                Ok((tr, dr, tf, df))
            })
            .await?;
        let pd = pose_delta(angles(&tf), angles(&df));
        let scale = match (
            face_px(&tf, target_ref.width.map(i64::from)),
            face_px(&df, donor_ref.width.map(i64::from)),
        ) {
            (Some(t), Some(d)) if t > 0.0 => Some(d / t),
            _ => None,
        };
        if let Some(why) = pose_reason(&pd, scale) {
            return Err(CoreError::UnprocessableCode(
                if why == "pose_too_different" {
                    "hair_pose_too_different"
                } else {
                    "hair_donor_too_small"
                },
                format!(
                    "face {} cannot give its hair to face {} ({why}: yaw {:.0}, pitch {:.0}, roll {:.0} degrees apart)",
                    df.id, tf.id, pd.yaw, pd.pitch, pd.roll
                ),
            ));
        }
        self.preflight_models("hair", &[]).await?;
        let spec = json!({
            "photo_id": photo_id,
            "face_id": tf.id,
            "donor_photo_id": donor_ref.id,
            "strength": strength,
        });
        let task = self.task_begin("hair", spec, 1).await?;
        let (core, task_id) = (self.clone(), task.id.clone());
        tokio::spawn(async move {
            let out = task
                .cancel
                .scope(core.run_hair(&task, &target_ref, &donor_ref, &tf, &df, strength))
                .await;
            let (state, ok, warnings, reason) = match out {
                Ok(Some(warnings)) => (DONE, true, warnings, None),
                Ok(None) => (CANCELLED, false, Vec::new(), Some(CANCELLED.to_string())),
                Err(_) if task.cancelled() => {
                    (CANCELLED, false, Vec::new(), Some(CANCELLED.to_string()))
                }
                Err(CoreError::UnprocessableCode("hair_refused", why)) => {
                    (FAILED, false, Vec::new(), Some(why))
                }
                Err(e) => (FAILED, false, Vec::new(), Some(reason_of(&e))),
            };
            let _ = std::fs::remove_dir_all(core.gen_dir(&task.id));
            let result = Event::HairDone {
                photo_id: target_ref.id,
                face_id: tf.id,
                ok,
                warnings,
                reason: reason.clone(),
            };
            let done = i64::from(state == DONE);
            let error = reason.filter(|_| state == FAILED);
            core.task_end(&task, (done, 1), state, error, Some(result))
                .await;
        });
        Ok(task_id)
    }

    /// `Ok(Some(warnings))` committed, `Ok(None)` cancelled before committing; a worker refusal
    /// is `UnprocessableCode("hair_refused", reason)`.
    async fn run_hair(
        self: &Arc<Self>,
        task: &TaskHandle,
        target: &PhotoRef,
        donor: &PhotoRef,
        tf: &Face,
        df: &Face,
        strength: f32,
    ) -> Result<Option<Vec<String>>> {
        let dir = self.gen_dir(&task.id);
        std::fs::create_dir_all(&dir)?;
        let resp = self
            .worker
            .hair_compose(&HairComposeRequest {
                target: mkphoto(target),
                donor: mkphoto(donor),
                target_face: tf.bbox,
                donor_face: df.bbox,
                out_dir: dir.join("out").to_string_lossy().into_owned(),
                allow_download: false,
            })
            .await
            .map_err(map_worker_err)?;
        if task.cancelled() {
            return Ok(None);
        }
        let (Some(patch), Some(rect)) = (resp.patch, resp.rect) else {
            return Err(CoreError::UnprocessableCode(
                "hair_refused",
                resp.reason.unwrap_or_else(|| "reason_unknown".to_string()),
            ));
        };
        let (svc, id, face) = (self.render.clone(), target.id, tf.id);
        let (asset, _) = tokio::task::spawn_blocking(move || {
            svc.patches
                .import(id, "hair", &face.to_string(), Path::new(&patch))
        })
        .await
        .map_err(|e| CoreError::Internal(anyhow::anyhow!("patch import failed: {e}")))??;
        let warnings = resp.quality.map(|q| q.warnings).unwrap_or_default();
        let mut op = patch_op("hair", &asset, rect, PATCH_FEATHER, strength);
        if let Some(p) = tf.person_id {
            op.insert("person_id".into(), json!(p));
        }
        op.insert("source_photo_id".into(), json!(donor.id));
        // additive (not in the typed op): which faces, and how the donor was aligned
        op.insert("base_face_id".into(), json!(tf.id));
        op.insert("source_face_id".into(), json!(df.id));
        if let Some(a) = resp.align {
            op.insert(
                "align".into(),
                serde_json::to_value(a).unwrap_or(Value::Null),
            );
        }
        if !warnings.is_empty() {
            op.insert("warnings".into(), json!(warnings));
        }
        self.apply_patches(target.id, vec![Value::Object(op)])
            .await?;
        Ok(Some(warnings))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ident(
        face: i64,
        person: Option<i64>,
        track: Option<i64>,
        e: Option<Vec<f32>>,
    ) -> FaceIdentity {
        FaceIdentity {
            face_id: face,
            photo_id: face * 10,
            burst_id: None,
            track_id: track,
            person_id: person,
            embedding: e,
        }
    }

    fn code(r: Result<()>) -> &'static str {
        match r {
            Ok(()) => "ok",
            Err(CoreError::UnprocessableCode(c, _)) => c,
            Err(e) => panic!("{e}"),
        }
    }

    #[test]
    fn identity_guard() {
        let a = Some(vec![1.0, 0.0, 0.0]);
        let a2 = Some(vec![0.8, 0.6, 0.0]); // cos 0.8
        let mid = Some(vec![0.35, 0.94, 0.0]); // cos ~0.35
        let b = Some(vec![0.0, 1.0, 0.0]); // cos 0
                                           // same person (clustered), across sessions / bursts: fine
        assert_eq!(
            code(check_hair_identity(
                &ident(1, Some(7), None, a.clone()),
                &ident(2, Some(7), None, a2.clone())
            )),
            "ok"
        );
        assert_eq!(
            code(check_hair_identity(
                &ident(1, Some(7), None, None),
                &ident(2, Some(7), None, None)
            )),
            "ok"
        );
        // another person
        assert_eq!(
            code(check_hair_identity(
                &ident(1, Some(7), None, a.clone()),
                &ident(2, Some(8), None, a2.clone())
            )),
            "hair_person_mismatch"
        );
        // same person id, but the faces do not look alike: a clustering mistake
        assert_eq!(
            code(check_hair_identity(
                &ident(1, Some(7), None, a.clone()),
                &ident(2, Some(7), None, b.clone())
            )),
            "hair_low_similarity"
        );
        // unclustered: needs the stricter similarity
        assert_eq!(
            code(check_hair_identity(
                &ident(1, None, None, a.clone()),
                &ident(2, None, None, a2.clone())
            )),
            "ok"
        );
        assert_eq!(
            code(check_hair_identity(
                &ident(1, None, None, a.clone()),
                &ident(2, None, None, mid.clone())
            )),
            "hair_low_similarity"
        );
        assert_eq!(
            code(check_hair_identity(
                &ident(1, Some(7), None, a.clone()),
                &ident(2, Some(7), None, mid)
            )),
            "ok"
        );
        // nothing links them, nothing to compare
        assert_eq!(
            code(check_hair_identity(
                &ident(1, None, None, None),
                &ident(2, Some(3), None, None)
            )),
            "hair_person_mismatch"
        );
        // same track wins over (stale) differing person ids
        assert_eq!(
            code(check_hair_identity(
                &ident(1, Some(7), Some(4), a.clone()),
                &ident(2, Some(8), Some(4), a2)
            )),
            "ok"
        );
    }

    #[test]
    fn pose_gate_and_wrap() {
        let d = pose_delta(
            [Some(10.0), Some(85.0), Some(175.0)],
            [Some(-5.0), Some(-85.0), Some(-175.0)],
        );
        assert!(
            (d.yaw - 15.0).abs() < 1e-9
                && (d.pitch - 10.0).abs() < 1e-9
                && (d.roll - 10.0).abs() < 1e-9
        );
        assert_eq!(pose_reason(&d, Some(1.0)), None);
        let unknown = pose_delta([None, Some(3.0), None], [Some(40.0), Some(1.0), Some(2.0)]);
        assert_eq!(
            unknown,
            PoseDelta {
                yaw: 0.0,
                pitch: 2.0,
                roll: 0.0
            }
        );
        for (y, p, r) in [(25.0, 0.0, 0.0), (0.0, 16.0, 0.0), (0.0, 0.0, 21.0)] {
            assert_eq!(
                pose_reason(
                    &PoseDelta {
                        yaw: y,
                        pitch: p,
                        roll: r
                    },
                    None
                ),
                Some("pose_too_different")
            );
        }
        assert_eq!(
            pose_reason(&PoseDelta::default(), Some(0.4)),
            Some("donor_too_small")
        );
    }

    fn donor(id: i64, same: bool, minutes: Option<i64>, m: Option<HairMetrics>) -> HairDonor {
        HairDonor {
            photo_id: id,
            face_id: id * 10,
            file_name: format!("{id}.jpg"),
            taken_at: None,
            minutes_from_target: minutes,
            same_session: same,
            composable: true,
            reason: None,
            pose_delta: PoseDelta::default(),
            scale: Some(1.0),
            score: None,
            cues: vec![],
            metrics: m,
        }
    }

    fn hm(id: i64, ratio: f64, shine: f64, rough: f64, sharp: f64) -> HairMetrics {
        HairMetrics {
            photo_id: id,
            ok: true,
            hair_ratio: Some(ratio),
            shine: Some(shine),
            roughness: Some(rough),
            sharpness: Some(sharp),
            ..Default::default()
        }
    }

    #[test]
    fn clean_full_hair_ranks_first_with_reasons() {
        // target: flat, shiny, stringy (late in the day)
        let target = hm(1, 0.5, 0.04, 0.25, 10.0);
        let mut ds = vec![
            donor(2, true, Some(-30), Some(hm(2, 0.52, 0.035, 0.24, 10.0))), // as bad as the target
            donor(3, true, Some(-180), Some(hm(3, 0.7, 0.005, 0.05, 13.0))), // morning: good hair
            donor(
                4,
                false,
                Some(-60 * 24 * 30),
                Some(hm(4, 0.75, 0.0, 0.03, 14.0)),
            ), // other session
            donor(5, true, Some(10), None),                                  // not measured
        ];
        score_donors(Some(&target), &mut ds);
        sort_donors(&mut ds);
        let order: Vec<i64> = ds.iter().map(|d| d.photo_id).collect();
        assert_eq!(order, vec![3, 2, 5, 4]); // same session first; measured before unmeasured
        let best = &ds[0];
        for c in [
            "fuller",
            "less_shine",
            "tidier",
            "sharper",
            "similar_pose",
            "earlier",
            "same_session",
        ] {
            assert!(
                best.cues.iter().any(|x| x == c),
                "{c} missing in {:?}",
                best.cues
            );
        }
        assert!(best.score.unwrap() > ds[1].score.unwrap() + 0.15);
        assert!(!ds[1]
            .cues
            .iter()
            .any(|c| c == "less_shine" || c == "fuller"));
        assert_eq!(ds[2].score, None);
        // a turned head costs score; a refused pose sorts after the composable ones
        let mut ds = vec![
            donor(3, true, None, Some(hm(3, 0.7, 0.005, 0.05, 13.0))),
            donor(6, true, None, Some(hm(6, 0.7, 0.005, 0.05, 13.0))),
            donor(7, true, None, Some(hm(7, 0.8, 0.0, 0.0, 20.0))),
        ];
        ds[1].pose_delta.yaw = 18.0;
        ds[2].composable = false;
        ds[2].reason = Some("pose_too_different".into());
        score_donors(Some(&target), &mut ds);
        sort_donors(&mut ds);
        assert_eq!(
            ds.iter().map(|d| d.photo_id).collect::<Vec<_>>(),
            vec![3, 6, 7]
        );
        assert!(ds[0].score > ds[1].score);
    }

    #[test]
    fn unmeasurable_donor_hair_is_not_composable() {
        let mut ds = vec![donor(
            2,
            true,
            None,
            Some(HairMetrics {
                photo_id: 2,
                ok: false,
                reason: Some("hair_not_found".into()),
                ..Default::default()
            }),
        )];
        score_donors(None, &mut ds);
        assert!(
            !ds[0].composable
                && ds[0].reason.as_deref() == Some("hair_not_found")
                && ds[0].score.is_none()
        );
    }
}
