//! SQL for analysis results: ingest, grouping, scoring, clustering, groups/people queries.
//! Everything here is synchronous and runs through [`crate::db::Db`].

use std::collections::{BTreeMap, HashMap, HashSet};

use rusqlite::{params, params_from_iter, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value as Json};

use ip_worker_client::AnalyzeItem;

use super::cluster::{self, Assignment, TrackFace, TrackInput};
use super::grouping::{self, GroupItem, GroupParams, SceneUnit};
use super::scoring::{self, BurstMember, FaceFeat, Issue, PhotoFeat};
use super::types::*;
use super::vecs::{decode_f16, encode_f16, normalize};
use crate::catalog::now_ms;
use crate::error::{CoreError, Result};

fn placeholders(n: usize) -> String {
    vec!["?"; n].join(",")
}

fn ids_sql(ids: &[i64]) -> String {
    ids.iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

// ====================================================================== ingest

/// A worker result with its artifact files already read.
pub struct IngestItem {
    pub item: AnalyzeItem,
    /// Image embedding (L2-normalised).
    pub embedding: Option<Vec<f32>>,
    /// Identity embedding per face (same order as `item.faces`).
    pub face_embs: Vec<Option<Vec<f32>>>,
}

fn parse_phash(s: &str) -> Option<u64> {
    u64::from_str_radix(s.trim(), 16).ok()
}

/// Writes analysis rows and faces of a batch (replacing earlier results of the same photos);
/// user-assigned face -> person links survive when the same face is found again (IoU >= 0.5).
pub fn ingest(
    conn: &mut Connection,
    items: &[IngestItem],
    profile: Profile,
    model_versions: &Json,
    artifacts_dir: &str,
) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let now = now_ms();
    for it in items {
        let a = &it.item;
        if a.error.is_some() {
            continue;
        }
        let pid = a.photo_id;
        let locked: Vec<([f64; 4], i64)> = {
            let mut st = tx.prepare_cached(
                "SELECT bbox_x, bbox_y, bbox_w, bbox_h, person_id FROM face
                 WHERE photo_id=?1 AND person_locked=1 AND person_id IS NOT NULL",
            )?;
            let v = st
                .query_map([pid], |r| {
                    Ok(([r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?], r.get(4)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            v
        };
        tx.execute("DELETE FROM face WHERE photo_id=?1", [pid])?;
        let q = a.quality.clone().unwrap_or_default();
        let phash_blob = a
            .phash
            .as_deref()
            .and_then(parse_phash)
            .map(|h| h.to_be_bytes().to_vec());
        let emb_blob = it.embedding.as_ref().map(|e| encode_f16(e));
        tx.execute(
            "INSERT OR REPLACE INTO analysis(photo_id, profile, steps_done, model_versions, phash, embedding,
               sharpness, exposure, noise, aesthetic, iqa, mean_luminance, clipped_highlights, crushed_shadows,
               scene_scores, explain, artifacts_dir, analyzed_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,NULL,?16,?17)",
            params![
                pid,
                profile.as_str(),
                profile.level(),
                model_versions.to_string(),
                phash_blob,
                emb_blob,
                a.sharpness,
                a.exposure,
                a.noise,
                a.aesthetic,
                a.iqa,
                q.mean_luminance,
                q.clipped_highlights,
                q.crushed_shadows,
                a.scene_scores.as_ref().map(|v| v.to_string()),
                artifacts_dir,
                now
            ],
        )?;
        // faces
        let feats: Vec<(FaceFeat, bool)> = a
            .faces
            .iter()
            .map(|f| {
                (
                    FaceFeat {
                        bbox: f.bbox,
                        eyes_open: f.eyes_open,
                        smile: f.smile,
                        gaze: f.gaze,
                        yaw: f.yaw,
                        pitch: f.pitch,
                        sharpness: f.sharpness,
                        ..Default::default()
                    },
                    false,
                )
            })
            .collect();
        let subjects = scoring::subject_flags(&feats);
        for (i, f) in a.faces.iter().enumerate() {
            let emb = it
                .face_embs
                .get(i)
                .and_then(|e| e.as_ref())
                .map(|e| encode_f16(e));
            let expr = scoring::expression_score(&feats[i].0);
            let mut person: Option<i64> = None;
            let mut best_iou = 0.5;
            for (bb, p) in &locked {
                let o = cluster::iou(bb, &f.bbox);
                if o >= best_iou {
                    best_iou = o;
                    person = Some(*p);
                }
            }
            // the person may have been deleted meanwhile
            let person = match person {
                Some(p) => tx
                    .query_row("SELECT id FROM person WHERE id=?1", [p], |r| {
                        r.get::<_, i64>(0)
                    })
                    .optional()?,
                None => None,
            };
            tx.execute(
                "INSERT INTO face(photo_id, idx, person_id, person_locked, bbox_x, bbox_y, bbox_w, bbox_h,
                   det_score, embedding, eyes_open, smile, gaze, yaw, pitch, roll, sharpness, expression_score,
                   is_subject, is_bystander, blendshapes)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21)",
                params![
                    pid,
                    i as i64,
                    person,
                    person.is_some() as i64,
                    f.bbox[0],
                    f.bbox[1],
                    f.bbox[2],
                    f.bbox[3],
                    f.det_score,
                    emb,
                    f.eyes_open,
                    f.smile,
                    f.gaze,
                    f.yaw,
                    f.pitch,
                    f.roll,
                    f.sharpness,
                    expr,
                    subjects[i] as i64,
                    (!subjects[i]) as i64,
                    f.blendshapes.as_ref().map(|b| b.to_string()),
                ],
            )?;
        }
        tx.execute(
            "UPDATE photo SET scene_type=?2, analysis_version=?3, face_count=?4, subject_face_count=?5 WHERE id=?1",
            params![
                pid,
                a.scene_type,
                profile.level(),
                a.faces.len() as i64,
                subjects.iter().filter(|s| **s).count() as i64
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

// ====================================================================== grouping

struct GRow {
    id: i64,
    t: Option<i64>,
    device: Option<i64>,
    name: String,
    burst_id: Option<i64>,
    manual: bool,
    phash: Option<u64>,
    emb: Option<Vec<f32>>,
}

fn blob_phash(b: Option<Vec<u8>>) -> Option<u64> {
    let b = b?;
    let a: [u8; 8] = b.try_into().ok()?;
    Some(u64::from_be_bytes(a))
}

fn mean_embedding<'a>(embs: impl Iterator<Item = &'a Vec<f32>>) -> Option<Vec<f32>> {
    let mut sum: Vec<f32> = Vec::new();
    for e in embs {
        if sum.is_empty() {
            sum = vec![0.0; e.len()];
        }
        if e.len() == sum.len() {
            sum.iter_mut().zip(e).for_each(|(a, b)| *a += b);
        }
    }
    if sum.is_empty() {
        return None;
    }
    normalize(&mut sum);
    Some(sum)
}

/// Rebuilds bursts (auto, time/similarity based) and scenes of a session from the stored analysis.
/// Manual bursts (user split/merge) are kept as they are. Returns every burst id of the session.
pub fn regroup_session(
    conn: &mut Connection,
    session_id: i64,
    params: &GroupParams,
) -> Result<Vec<i64>> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let rows: Vec<GRow> = {
        let mut st = tx.prepare(
            "SELECT p.id, COALESCE(p.taken_at, p.mtime), p.file_name, p.burst_id,
                    COALESCE(b.manual,0) AND b.session_id=?1, a.phash, a.embedding, p.device_id
             FROM photo p
             JOIN session_photo sp ON sp.photo_id=p.id AND sp.session_id=?1
             JOIN analysis a ON a.photo_id=p.id
             LEFT JOIN burst b ON b.id=p.burst_id",
        )?;
        let v = st
            .query_map([session_id], |r| {
                Ok(GRow {
                    id: r.get(0)?,
                    t: r.get(1)?,
                    name: r.get(2)?,
                    burst_id: r.get(3)?,
                    manual: r.get::<_, Option<i64>>(4)?.unwrap_or(0) != 0,
                    phash: blob_phash(r.get(5)?),
                    emb: r
                        .get::<_, Option<Vec<u8>>>(6)?
                        .map(|b| decode_f16(&b))
                        .filter(|e| !e.is_empty()),
                    device: r.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        v
    };
    let (manual_rows, mut auto_rows): (Vec<GRow>, Vec<GRow>) =
        rows.into_iter().partition(|r| r.manual);
    auto_rows.sort_by(|a, b| {
        a.t.unwrap_or(i64::MAX)
            .cmp(&b.t.unwrap_or(i64::MAX))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then(a.id.cmp(&b.id))
    });
    let items: Vec<GroupItem> = auto_rows
        .iter()
        .map(|r| GroupItem {
            id: r.id,
            t: r.t,
            device: r.device,
            phash: r.phash,
            emb: r.emb.clone(),
        })
        .collect();
    let groups = grouping::segment_bursts(&items, params);

    // reset the old structure of this session
    for chunk in auto_rows.chunks(500) {
        let ids: Vec<i64> = chunk.iter().map(|r| r.id).collect();
        tx.execute(
            &format!(
                "UPDATE photo SET burst_id=NULL, rank_in_burst=NULL WHERE id IN ({})",
                placeholders(ids.len())
            ),
            params_from_iter(ids.iter()),
        )?;
    }
    tx.execute(
        "UPDATE photo SET burst_id=NULL, rank_in_burst=NULL
         WHERE burst_id IN (SELECT id FROM burst WHERE session_id=?1 AND manual=0)",
        [session_id],
    )?;
    tx.execute(
        "DELETE FROM burst WHERE session_id=?1 AND manual=0",
        [session_id],
    )?;
    tx.execute(
        "UPDATE burst SET scene_id=NULL WHERE session_id=?1",
        [session_id],
    )?;
    tx.execute("DELETE FROM scene WHERE session_id=?1", [session_id])?;

    struct Unit {
        id: i64,
        start: i64,
        end: i64,
        emb: Option<Vec<f32>>,
        phash: Option<u64>,
    }
    let mut units: Vec<Unit> = Vec::new();
    for g in &groups {
        let members: Vec<&GRow> = g.iter().map(|&i| &auto_rows[i]).collect();
        let ts: Vec<i64> = members.iter().filter_map(|m| m.t).collect();
        let (start, end) = (
            ts.iter().copied().min().unwrap_or(0),
            ts.iter().copied().max().unwrap_or(0),
        );
        tx.execute(
            "INSERT INTO burst(session_id, best_photo_id, size, manual, start_at, end_at)
             VALUES(?1,?2,?3,0,?4,?5)",
            params![session_id, members[0].id, members.len() as i64, start, end],
        )?;
        let bid = tx.last_insert_rowid();
        for m in &members {
            tx.execute(
                "UPDATE photo SET burst_id=?2 WHERE id=?1",
                params![m.id, bid],
            )?;
        }
        units.push(Unit {
            id: bid,
            start,
            end,
            emb: mean_embedding(members.iter().filter_map(|m| m.emb.as_ref())),
            phash: members[0].phash,
        });
    }
    // manual bursts: refresh their statistics
    let mut by_burst: BTreeMap<i64, Vec<&GRow>> = BTreeMap::new();
    for r in &manual_rows {
        if let Some(b) = r.burst_id {
            by_burst.entry(b).or_default().push(r);
        }
    }
    for (bid, members) in by_burst {
        let ts: Vec<i64> = members.iter().filter_map(|m| m.t).collect();
        let (start, end) = (
            ts.iter().copied().min().unwrap_or(0),
            ts.iter().copied().max().unwrap_or(0),
        );
        tx.execute(
            "UPDATE burst SET size=?2, start_at=?3, end_at=?4 WHERE id=?1",
            params![bid, members.len() as i64, start, end],
        )?;
        units.push(Unit {
            id: bid,
            start,
            end,
            emb: mean_embedding(members.iter().filter_map(|m| m.emb.as_ref())),
            phash: members.first().and_then(|m| m.phash),
        });
    }
    // bursts that lost all their photos (e.g. photos removed) are garbage
    tx.execute(
        "DELETE FROM burst WHERE session_id=?1 AND manual=1
           AND NOT EXISTS(SELECT 1 FROM photo WHERE burst_id=burst.id)",
        [session_id],
    )?;
    units.sort_by(|a, b| a.start.cmp(&b.start).then(a.id.cmp(&b.id)));
    let scene_units: Vec<SceneUnit> = units
        .iter()
        .map(|u| SceneUnit {
            start: u.start,
            end: u.end,
            emb: u.emb.clone(),
            phash: u.phash,
        })
        .collect();
    for sc in grouping::segment_scenes(&scene_units, params) {
        let (start, end) = (
            units[sc[0]].start,
            sc.iter().map(|&i| units[i].end).max().unwrap_or(0),
        );
        tx.execute(
            "INSERT INTO scene(session_id, start_at, end_at) VALUES(?1,?2,?3)",
            params![session_id, start, end],
        )?;
        let scene_id = tx.last_insert_rowid();
        for &i in &sc {
            tx.execute(
                "UPDATE burst SET scene_id=?2 WHERE id=?1",
                params![units[i].id, scene_id],
            )?;
        }
    }
    let ids = units.iter().map(|u| u.id).collect();
    tx.commit()?;
    Ok(ids)
}

// ====================================================================== scoring

struct SRow {
    burst_id: i64,
    taken_at: Option<i64>,
    feat: PhotoFeat,
    emb: Option<Vec<f32>>,
}

pub(crate) fn load_faces_feat(
    conn: &Connection,
    sql: &str,
    args: &[i64],
) -> Result<HashMap<i64, Vec<FaceFeat>>> {
    let mut st = conn.prepare(sql)?;
    let mut out: HashMap<i64, Vec<FaceFeat>> = HashMap::new();
    let rows = st.query_map(params_from_iter(args.iter()), |r| {
        Ok((
            r.get::<_, i64>(0)?,
            FaceFeat {
                id: r.get(1)?,
                person_id: r.get(2)?,
                bbox: [r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?],
                eyes_open: r.get(7)?,
                smile: r.get(8)?,
                gaze: r.get(9)?,
                yaw: r.get(10)?,
                pitch: r.get(11)?,
                sharpness: r.get(12)?,
                is_subject: r.get::<_, i64>(13)? != 0,
            },
        ))
    })?;
    for r in rows {
        let (pid, f) = r?;
        out.entry(pid).or_default().push(f);
    }
    Ok(out)
}

pub(crate) const FACE_FEAT_COLS: &str =
    "f.photo_id, f.id, f.person_id, f.bbox_x, f.bbox_y, f.bbox_w, f.bbox_h,
    f.eyes_open, f.smile, f.gaze, f.yaw, f.pitch, f.sharpness, f.is_subject";

/// Scores, ranks and stars every analysed photo of the given bursts and writes the results.
/// Returns the ids of the photos whose AI fields were written.
pub fn rescore_bursts(conn: &mut Connection, burst_ids: &[i64]) -> Result<Vec<i64>> {
    let mut touched = Vec::new();
    let taste = crate::taste::load_active(conn)?;
    for chunk in burst_ids.chunks(200) {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ph = placeholders(chunk.len());
        let mut rows: Vec<SRow> = {
            let mut st = tx.prepare(&format!(
                "SELECT p.id, p.burst_id, COALESCE(p.taken_at, p.mtime), p.scene_type, a.sharpness, a.exposure,
                        a.noise, a.iqa, a.aesthetic, a.clipped_highlights, a.crushed_shadows,
                        a.mean_luminance, a.embedding
                 FROM photo p JOIN analysis a ON a.photo_id=p.id WHERE p.burst_id IN ({ph})"
            ))?;
            let v = st
                .query_map(params_from_iter(chunk.iter()), |r| {
                    Ok(SRow {
                        burst_id: r.get(1)?,
                        taken_at: r.get(2)?,
                        feat: PhotoFeat {
                            photo_id: r.get(0)?,
                            scene_type: r.get(3)?,
                            sharpness: r.get(4)?,
                            exposure: r.get(5)?,
                            noise: r.get(6)?,
                            iqa: r.get(7)?,
                            aesthetic: r.get(8)?,
                            clipped_highlights: r.get(9)?,
                            crushed_shadows: r.get(10)?,
                            mean_luminance: r.get(11)?,
                            faces: Vec::new(),
                        },
                        emb: r
                            .get::<_, Option<Vec<u8>>>(12)?
                            .map(|b| decode_f16(&b))
                            .filter(|e| !e.is_empty()),
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            v
        };
        let mut faces = load_faces_feat(
            &tx,
            &format!(
                "SELECT {FACE_FEAT_COLS} FROM face f WHERE f.photo_id IN
                   (SELECT id FROM photo WHERE burst_id IN ({ph})) ORDER BY f.id"
            ),
            chunk,
        )?;
        for r in &mut rows {
            r.feat.faces = faces.remove(&r.feat.photo_id).unwrap_or_default();
        }
        let mut by_burst: BTreeMap<i64, Vec<SRow>> = BTreeMap::new();
        for r in rows {
            by_burst.entry(r.burst_id).or_default().push(r);
        }
        for (bid, members) in by_burst {
            let scored: Vec<BurstMember> = members
                .iter()
                .map(|m| BurstMember {
                    score: scoring::score_photo(&m.feat),
                    taken_at: m.taken_at,
                    emb: m.emb.as_deref(),
                })
                .collect();
            let mut ranked = scoring::rank_burst(scored);
            if let Some((model, alpha)) = &taste {
                let user: HashMap<i64, f64> = members
                    .iter()
                    .map(|m| (m.feat.photo_id, model.user_score(&m.feat, m.emb.as_deref())))
                    .collect();
                ranked = crate::taste::fuse(ranked, &user, *alpha);
            }
            let best = ranked.first().map(|r| r.photo_id);
            for r in &ranked {
                tx.execute(
                    "UPDATE photo SET ai_score=?2, ai_rating=?3, issues=?4, rank_in_burst=?5, base_score=?6 WHERE id=?1",
                    params![
                        r.photo_id,
                        (r.q * 10_000.0).round() / 10_000.0,
                        r.ai_rating,
                        Issue::to_mask(&r.issues),
                        r.rank as i64,
                        (r.base_q * 10_000.0).round() / 10_000.0
                    ],
                )?;
                let explain = json!({
                    "contributions": r.contributions,
                    "reasons": r.reasons,
                    "face_score": r.face_score,
                });
                tx.execute(
                    "UPDATE analysis SET explain=?2 WHERE photo_id=?1",
                    params![r.photo_id, explain.to_string()],
                )?;
                touched.push(r.photo_id);
            }
            tx.execute(
                "UPDATE burst SET best_photo_id=?2, size=?3 WHERE id=?1",
                params![bid, best, ranked.len() as i64],
            )?;
        }
        tx.commit()?;
    }
    Ok(touched)
}

pub fn session_burst_ids(conn: &Connection, session_id: i64) -> Result<Vec<i64>> {
    let mut st = conn.prepare("SELECT id FROM burst WHERE session_id=?1 ORDER BY start_at, id")?;
    let v = st
        .query_map([session_id], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<i64>>>()?;
    Ok(v)
}

// ====================================================================== subjects

/// Recomputes `is_subject` / counts for the given photos (names raise a face's weight).
/// Returns photos whose subject set changed.
pub fn recompute_subjects(conn: &mut Connection, photo_ids: &[i64]) -> Result<Vec<i64>> {
    let mut changed = Vec::new();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for chunk in photo_ids.chunks(500) {
        let ph = placeholders(chunk.len());
        let mut st = tx.prepare(&format!(
            "SELECT f.photo_id, f.id, f.bbox_x, f.bbox_y, f.bbox_w, f.bbox_h, f.sharpness, f.gaze,
                    f.is_subject, pe.name IS NOT NULL
             FROM face f LEFT JOIN person pe ON pe.id=f.person_id
             WHERE f.photo_id IN ({ph}) ORDER BY f.photo_id, f.id"
        ))?;
        struct R {
            id: i64,
            feat: FaceFeat,
            named: bool,
            was: bool,
        }
        let mut per: BTreeMap<i64, Vec<R>> = BTreeMap::new();
        let rows = st.query_map(params_from_iter(chunk.iter()), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                R {
                    id: r.get(1)?,
                    feat: FaceFeat {
                        bbox: [r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?],
                        sharpness: r.get(6)?,
                        gaze: r.get(7)?,
                        ..Default::default()
                    },
                    was: r.get::<_, i64>(8)? != 0,
                    named: r.get::<_, i64>(9)? != 0,
                },
            ))
        })?;
        for r in rows {
            let (p, r) = r?;
            per.entry(p).or_default().push(r);
        }
        drop(st);
        for (pid, faces) in per {
            let flags = scoring::subject_flags(
                &faces
                    .iter()
                    .map(|f| (f.feat.clone(), f.named))
                    .collect::<Vec<_>>(),
            );
            let mut diff = false;
            for (f, s) in faces.iter().zip(&flags) {
                if f.was != *s {
                    diff = true;
                    tx.execute(
                        "UPDATE face SET is_subject=?2, is_bystander=?3 WHERE id=?1",
                        params![f.id, *s as i64, (!*s) as i64],
                    )?;
                }
            }
            tx.execute(
                "UPDATE photo SET face_count=?2, subject_face_count=?3 WHERE id=?1",
                params![
                    pid,
                    faces.len() as i64,
                    flags.iter().filter(|s| **s).count() as i64
                ],
            )?;
            if diff {
                changed.push(pid);
            }
        }
    }
    tx.commit()?;
    Ok(changed)
}

// ====================================================================== tracks & people clustering

struct CFace {
    id: i64,
    photo_id: i64,
    person_id: Option<i64>,
    locked: bool,
    bbox: [f64; 4],
    emb: Option<Vec<f32>>,
    is_subject: bool,
    /// Burst id, or `-photo_id` for photos outside any burst.
    group: i64,
    t: i64,
}

fn load_cfaces(conn: &Connection, sql: &str, args: &[i64]) -> Result<Vec<CFace>> {
    let mut st = conn.prepare(sql)?;
    let v = st
        .query_map(params_from_iter(args.iter()), |r| {
            let photo_id: i64 = r.get(1)?;
            Ok(CFace {
                id: r.get(0)?,
                photo_id,
                person_id: r.get(2)?,
                locked: r.get::<_, i64>(3)? != 0,
                bbox: [r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?],
                emb: r
                    .get::<_, Option<Vec<u8>>>(8)?
                    .map(|b| decode_f16(&b))
                    .filter(|e| !e.is_empty()),
                group: r.get::<_, Option<i64>>(9)?.unwrap_or(-photo_id),
                t: r.get::<_, Option<i64>>(10)?.unwrap_or(0),
                is_subject: r.get::<_, i64>(11)? != 0,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

const CFACE_COLS: &str =
    "f.id, f.photo_id, f.person_id, f.person_locked, f.bbox_x, f.bbox_y, f.bbox_w, f.bbox_h,
    f.embedding, p.burst_id, COALESCE(p.taken_at, p.mtime), f.is_subject";

/// Track index (within its group) per face; `groups` are the face indices of each burst.
fn track_groups(faces: &[CFace]) -> (Vec<usize>, BTreeMap<i64, Vec<usize>>) {
    let mut groups: BTreeMap<i64, Vec<usize>> = BTreeMap::new();
    for (i, f) in faces.iter().enumerate() {
        groups.entry(f.group).or_default().push(i);
    }
    let mut track_of = vec![0usize; faces.len()];
    for idx in groups.values() {
        // frame = rank of the photo in time order
        let mut photos: Vec<(i64, i64)> = idx
            .iter()
            .map(|&i| (faces[i].t, faces[i].photo_id))
            .collect();
        photos.sort_unstable();
        photos.dedup();
        let frame_of: HashMap<i64, usize> = photos
            .iter()
            .enumerate()
            .map(|(k, (_, p))| (*p, k))
            .collect();
        let tf: Vec<TrackFace> = idx
            .iter()
            .map(|&i| TrackFace {
                frame: frame_of[&faces[i].photo_id],
                bbox: faces[i].bbox,
                emb: faces[i].emb.clone(),
            })
            .collect();
        for (k, t) in cluster::assign_tracks(&tf).into_iter().enumerate() {
            track_of[idx[k]] = t;
        }
    }
    (track_of, groups)
}

/// Re-derives `track_id` of the faces in one burst (after a manual split/merge).
pub fn retrack_burst(conn: &mut Connection, burst_id: i64) -> Result<()> {
    let faces = load_cfaces(
        conn,
        &format!(
            "SELECT {CFACE_COLS} FROM face f JOIN photo p ON p.id=f.photo_id WHERE p.burst_id=?1 ORDER BY f.id"
        ),
        &[burst_id],
    )?;
    let (track_of, _) = track_groups(&faces);
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for (f, t) in faces.iter().zip(track_of) {
        tx.execute(
            "UPDATE face SET track_id=?2 WHERE id=?1",
            params![f.id, t as i64],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Result of [`cluster_session`].
#[derive(Debug, Default)]
pub struct ClusterOutcome {
    pub changed_photos: Vec<i64>,
}

/// Tracks inside bursts, then people across the whole library (seeded by known people).
pub fn cluster_session(conn: &mut Connection, session_id: i64) -> Result<ClusterOutcome> {
    let faces = load_cfaces(
        conn,
        &format!(
            "SELECT {CFACE_COLS} FROM face f JOIN photo p ON p.id=f.photo_id
             JOIN session_photo sp ON sp.photo_id=p.id AND sp.session_id=?1 ORDER BY f.id"
        ),
        &[session_id],
    )?;
    if faces.is_empty() {
        return Ok(ClusterOutcome::default());
    }
    let (track_of, groups) = track_groups(&faces);
    // global track list
    let mut key_to_track: HashMap<(i64, usize), usize> = HashMap::new();
    let mut tracks: Vec<TrackInput> = Vec::new();
    let mut face_track: Vec<usize> = Vec::with_capacity(faces.len());
    let mut locked_votes: Vec<HashMap<i64, usize>> = Vec::new();
    let mut sums: Vec<Option<Vec<f32>>> = Vec::new();
    let _ = &groups;
    for (i, f) in faces.iter().enumerate() {
        // a user-assigned face is its own unit: the user's word beats "same track = same person"
        let k = if f.locked {
            (f.group, usize::MAX - i)
        } else {
            (f.group, track_of[i])
        };
        let ti = *key_to_track.entry(k).or_insert_with(|| {
            tracks.push(TrackInput {
                emb: None,
                photos: HashSet::new(),
                n_faces: 0,
                locked_person: None,
            });
            locked_votes.push(HashMap::new());
            sums.push(None);
            tracks.len() - 1
        });
        face_track.push(ti);
        tracks[ti].photos.insert(f.photo_id);
        tracks[ti].n_faces += 1;
        if f.locked {
            if let Some(p) = f.person_id {
                *locked_votes[ti].entry(p).or_insert(0) += 1;
            }
        }
        if let Some(e) = &f.emb {
            match &mut sums[ti] {
                Some(s) if s.len() == e.len() => s.iter_mut().zip(e).for_each(|(a, b)| *a += b),
                None => sums[ti] = Some(e.clone()),
                _ => {}
            }
        }
    }
    for (ti, t) in tracks.iter_mut().enumerate() {
        t.emb = sums[ti].take().map(|mut v| {
            normalize(&mut v);
            v
        });
        t.locked_person = locked_votes[ti]
            .iter()
            .max_by_key(|(p, n)| (**n, std::cmp::Reverse(**p)))
            .map(|(p, _)| *p);
    }
    let seeds: Vec<(i64, Vec<f32>)> = {
        let mut st = conn.prepare("SELECT id, center FROM person WHERE center IS NOT NULL")?;
        let v = st
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        v.into_iter()
            .map(|(id, b)| (id, decode_f16(&b)))
            .filter(|(_, c)| !c.is_empty())
            .collect()
    };
    let assignment = cluster::cluster_tracks(&tracks, &seeds, cluster::PERSON_COS);

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut new_ids: HashMap<usize, i64> = HashMap::new();
    // subjects that stayed unassigned for lack of a second photo still get a (singleton) person
    // so they can be picked in the portrait panel; they join a real person later if one matches
    let mut track_subject = vec![false; tracks.len()];
    for (i, f) in faces.iter().enumerate() {
        track_subject[face_track[i]] |= f.is_subject;
    }
    let mut singles: HashMap<usize, i64> = HashMap::new();
    let mut touched: HashSet<i64> = faces.iter().filter_map(|f| f.person_id).collect();
    let mut changed_photos: HashSet<i64> = HashSet::new();
    for (i, f) in faces.iter().enumerate() {
        let new_person = match &assignment[face_track[i]] {
            Assignment::Person(p) => Some(*p),
            Assignment::New(k) => Some(match new_ids.get(k) {
                Some(id) => *id,
                None => {
                    tx.execute("INSERT INTO person(created_at) VALUES(?1)", [now_ms()])?;
                    let id = tx.last_insert_rowid();
                    new_ids.insert(*k, id);
                    id
                }
            }),
            Assignment::None => {
                let ti = face_track[i];
                if tracks[ti].emb.is_some() && track_subject[ti] {
                    Some(match singles.get(&ti) {
                        Some(id) => *id,
                        None => {
                            tx.execute(
                                "INSERT INTO person(created_at, singleton) VALUES(?1, 1)",
                                [now_ms()],
                            )?;
                            let id = tx.last_insert_rowid();
                            singles.insert(ti, id);
                            id
                        }
                    })
                } else {
                    None
                }
            }
        };
        let final_person = if f.locked { f.person_id } else { new_person };
        if let Some(p) = final_person {
            touched.insert(p);
        }
        if final_person != f.person_id {
            changed_photos.insert(f.photo_id);
        }
        tx.execute(
            "UPDATE face SET track_id=?2, person_id=?3 WHERE id=?1",
            params![f.id, track_of[i] as i64, final_person],
        )?;
    }
    for p in touched {
        recompute_person(&tx, p)?;
    }
    tx.commit()?;
    let photo_ids: Vec<i64> = faces
        .iter()
        .map(|f| f.photo_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let mut changed = recompute_subjects(conn, &photo_ids)?;
    changed.extend(changed_photos);
    changed.sort_unstable();
    changed.dedup();
    Ok(ClusterOutcome {
        changed_photos: changed,
    })
}

/// Refreshes centre and cover of a person; deletes it when it has no faces left and no name.
pub fn recompute_person(conn: &Connection, person_id: i64) -> Result<()> {
    let mut st = conn.prepare(
        "SELECT id, embedding, bbox_w * bbox_h * (0.3 + COALESCE(sharpness, 0.5)) FROM face WHERE person_id=?1",
    )?;
    let faces = st
        .query_map([person_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<Vec<u8>>>(1)?,
                r.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(st);
    if faces.is_empty() {
        let named: Option<i64> = conn
            .query_row(
                "SELECT name IS NOT NULL FROM person WHERE id=?1",
                [person_id],
                |r| r.get(0),
            )
            .optional()?;
        if named == Some(0) {
            conn.execute("DELETE FROM person WHERE id=?1", [person_id])?;
        } else {
            conn.execute(
                "UPDATE person SET cover_face_id=NULL WHERE id=?1",
                [person_id],
            )?;
        }
        return Ok(());
    }
    let embs: Vec<Vec<f32>> = faces
        .iter()
        .filter_map(|(_, e, _)| e.as_ref().map(|b| decode_f16(b)))
        .filter(|e| !e.is_empty())
        .collect();
    let center = mean_embedding(embs.iter()).map(|c| encode_f16(&c));
    let cover = faces
        .iter()
        .max_by(|a, b| a.2.partial_cmp(&b.2).unwrap().then(b.0.cmp(&a.0)))
        .map(|f| f.0);
    conn.execute(
        "UPDATE person SET cover_face_id=?2, center=COALESCE(?3, center) WHERE id=?1",
        params![person_id, cover, center],
    )?;
    // a singleton that gained a second photo (or a name) is a real person now
    conn.execute(
        "UPDATE person SET singleton=0 WHERE id=?1 AND singleton=1 AND
           (name IS NOT NULL OR (SELECT COUNT(DISTINCT photo_id) FROM face WHERE person_id=?1) >= 2)",
        [person_id],
    )?;
    Ok(())
}

// ====================================================================== reads: faces, analysis, groups

const FACE_SELECT: &str = "SELECT f.id, f.photo_id, f.person_id, pe.name, f.bbox_x, f.bbox_y, f.bbox_w, f.bbox_h,
    f.eyes_open, f.smile, f.gaze, f.yaw, f.pitch, f.roll, f.sharpness, f.expression_score, f.is_subject, f.track_id
    FROM face f LEFT JOIN person pe ON pe.id=f.person_id";

fn face_row(r: &rusqlite::Row) -> rusqlite::Result<(Face, Option<i64>)> {
    Ok((
        Face {
            id: r.get(0)?,
            photo_id: r.get(1)?,
            person_id: r.get(2)?,
            person_name: r.get(3)?,
            bbox: [r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?],
            eyes_open: r.get(8)?,
            smile: r.get(9)?,
            gaze: r.get(10)?,
            yaw: r.get(11)?,
            pitch: r.get(12)?,
            roll: r.get(13)?,
            sharpness: r.get(14)?,
            expression_score: r.get(15)?,
            is_subject: r.get::<_, i64>(16)? != 0,
        },
        r.get(17)?,
    ))
}

pub fn get_face(conn: &Connection, id: i64) -> Result<Face> {
    conn.query_row(&format!("{FACE_SELECT} WHERE f.id=?1"), [id], face_row)
        .map(|r| r.0)
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::not_found(format!("face {id} not found"))
            }
            e => e.into(),
        })
}

pub fn faces_of_photo(conn: &Connection, photo_id: i64) -> Result<Vec<Face>> {
    let mut st = conn.prepare(&format!(
        "{FACE_SELECT} WHERE f.photo_id=?1 ORDER BY f.idx, f.id"
    ))?;
    let v = st
        .query_map([photo_id], face_row)?
        .map(|r| r.map(|x| x.0))
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

/// Face geometry + photo reference for the crop endpoint.
pub fn face_crop_ref(conn: &Connection, id: i64) -> Result<(i64, [f64; 4])> {
    conn.query_row(
        "SELECT photo_id, bbox_x, bbox_y, bbox_w, bbox_h FROM face WHERE id=?1",
        [id],
        |r| Ok((r.get(0)?, [r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?])),
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => {
            CoreError::not_found(format!("face {id} not found"))
        }
        e => e.into(),
    })
}

pub fn analysis_detail(conn: &Connection, photo_id: i64) -> Result<AnalysisDetail> {
    let exists: i64 =
        conn.query_row("SELECT COUNT(*) FROM photo WHERE id=?1", [photo_id], |r| {
            r.get(0)
        })?;
    if exists == 0 {
        return Err(CoreError::not_found(format!("photo {photo_id} not found")));
    }
    type Row = (
        Option<String>,
        [Option<f64>; 5],
        Option<String>,
        Option<f64>,
        Option<f64>,
        Option<String>,
    );
    let row: Option<Row> = conn
        .query_row(
            "SELECT a.profile, a.sharpness, a.exposure, a.noise, a.iqa, a.aesthetic, a.explain,
                    p.ai_score, p.ai_rating, p.scene_type
             FROM analysis a JOIN photo p ON p.id=a.photo_id WHERE a.photo_id=?1",
            [photo_id],
            |r| {
                Ok((
                    r.get(0)?,
                    [r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?],
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                ))
            },
        )
        .optional()?;
    let Some((profile, s, explain, ai_score, ai_rating, scene)) = row else {
        return Ok(AnalysisDetail {
            photo_id,
            analyzed: false,
            profile: None,
            scores: None,
            ai_score: None,
            ai_rating: None,
            contributions: vec![],
            reasons: vec![],
            scene_type: None,
            faces: vec![],
        });
    };
    let faces = faces_of_photo(conn, photo_id)?;
    let explain: Json = explain
        .and_then(|e| serde_json::from_str(&e).ok())
        .unwrap_or(Json::Null);
    let contributions: Vec<scoring::Contribution> = explain
        .get("contributions")
        .and_then(|c| serde_json::from_value(c.clone()).ok())
        .unwrap_or_default();
    let raw_reasons: Vec<scoring::Reason> = explain
        .get("reasons")
        .and_then(|c| serde_json::from_value(c.clone()).ok())
        .unwrap_or_default();
    let reasons = raw_reasons
        .into_iter()
        .map(|r| {
            let mut params = serde_json::Map::new();
            if let Json::Object(o) = &r.params {
                for (k, v) in o {
                    if k == "person_id" {
                        if let Some(pid) = v.as_i64() {
                            let name: Option<String> = conn
                                .query_row("SELECT name FROM person WHERE id=?1", [pid], |r| {
                                    r.get(0)
                                })
                                .optional()
                                .ok()
                                .flatten()
                                .flatten();
                            if let Some(n) = name {
                                params.insert("person".into(), Json::String(n));
                            }
                        }
                    } else {
                        params.insert(k.clone(), v.clone());
                    }
                }
            }
            ReasonOut {
                key: r.key,
                params: Json::Object(params),
            }
        })
        .collect();
    Ok(AnalysisDetail {
        photo_id,
        analyzed: true,
        profile,
        scores: Some(Scores {
            sharpness: s[0],
            exposure: s[1],
            noise: s[2],
            iqa: s[3],
            aesthetic: s[4],
            face: explain.get("face_score").and_then(Json::as_f64),
            composition: None,
        }),
        ai_score,
        ai_rating,
        contributions,
        reasons,
        scene_type: scene,
        faces,
    })
}

pub fn groups_of_session(conn: &Connection, session_id: i64) -> Result<GroupsOut> {
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM session WHERE id=?1",
        [session_id],
        |r| r.get(0),
    )?;
    if exists == 0 {
        return Err(CoreError::not_found(format!(
            "session {session_id} not found"
        )));
    }
    // photos of the session grouped by burst, best first
    let mut st = conn.prepare(
        "SELECT b.id, b.scene_id, b.best_photo_id, COALESCE(b.start_at,0), p.id
         FROM photo p JOIN session_photo sp ON sp.photo_id=p.id AND sp.session_id=?1
         JOIN burst b ON b.id=p.burst_id
         ORDER BY b.id, COALESCE(p.rank_in_burst, 1000000), p.id",
    )?;
    struct B {
        id: i64,
        scene: Option<i64>,
        best: Option<i64>,
        start: i64,
        photos: Vec<i64>,
    }
    let mut bursts: BTreeMap<i64, B> = BTreeMap::new();
    let rows = st.query_map([session_id], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, Option<i64>>(1)?,
            r.get::<_, Option<i64>>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, i64>(4)?,
        ))
    })?;
    for r in rows {
        let (id, scene, best, start, pid) = r?;
        bursts
            .entry(id)
            .or_insert(B {
                id,
                scene,
                best,
                start,
                photos: Vec::new(),
            })
            .photos
            .push(pid);
    }
    let mut scene_info: HashMap<i64, (i64, i64)> = HashMap::new();
    let mut st = conn.prepare(
        "SELECT id, COALESCE(start_at,0), COALESCE(end_at,0) FROM scene WHERE session_id=?1",
    )?;
    for r in st.query_map([session_id], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })? {
        let (id, s, e) = r?;
        scene_info.insert(id, (s, e));
    }
    let mut scenes: BTreeMap<(i64, i64), SceneOut> = BTreeMap::new();
    for b in bursts.into_values() {
        let sid = b.scene.unwrap_or(-b.id);
        let (s, e) = scene_info.get(&sid).copied().unwrap_or((b.start, b.start));
        scenes
            .entry((s, sid))
            .or_insert(SceneOut {
                id: sid,
                start_at: s,
                end_at: e,
                bursts: Vec::new(),
            })
            .bursts
            .push(BurstOut {
                id: b.id,
                best_photo_id: b.best,
                size: b.photos.len() as i64,
                photo_ids: b.photos,
                start_at: b.start,
            });
    }
    let mut out: Vec<SceneOut> = scenes.into_values().collect();
    for s in &mut out {
        s.bursts.sort_by_key(|b| (b.start_at, b.id));
    }
    Ok(GroupsOut { scenes: out })
}

pub fn burst_faces(conn: &Connection, burst_id: i64) -> Result<BurstFacesOut> {
    let exists: i64 =
        conn.query_row("SELECT COUNT(*) FROM burst WHERE id=?1", [burst_id], |r| {
            r.get(0)
        })?;
    if exists == 0 {
        return Err(CoreError::not_found(format!("burst {burst_id} not found")));
    }
    let photo_ids: Vec<i64> = {
        let mut st = conn.prepare(
            "SELECT id FROM photo WHERE burst_id=?1 ORDER BY COALESCE(rank_in_burst,1000000), id",
        )?;
        let v = st
            .query_map([burst_id], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        v
    };
    let mut st = conn.prepare(&format!(
        "{FACE_SELECT} JOIN photo p ON p.id=f.photo_id WHERE p.burst_id=?1 ORDER BY f.track_id, f.id"
    ))?;
    let faces = st
        .query_map([burst_id], face_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut by_track: BTreeMap<i64, Vec<Face>> = BTreeMap::new();
    for (f, t) in faces {
        by_track
            .entry(t.unwrap_or(1_000_000 + f.id))
            .or_default()
            .push(f);
    }
    let mut tracks = Vec::new();
    for (tid, faces) in by_track {
        let mut votes: HashMap<(i64, Option<String>), usize> = HashMap::new();
        for f in &faces {
            if let Some(p) = f.person_id {
                *votes.entry((p, f.person_name.clone())).or_insert(0) += 1;
            }
        }
        let top = votes
            .into_iter()
            .max_by_key(|((p, _), n)| (*n, std::cmp::Reverse(*p)));
        let mut cells: BTreeMap<String, Option<Face>> =
            photo_ids.iter().map(|p| (p.to_string(), None)).collect();
        let mut ranked: Vec<&Face> = faces.iter().collect();
        ranked.sort_by(|a, b| {
            b.expression_score
                .unwrap_or(-1.0)
                .partial_cmp(&a.expression_score.unwrap_or(-1.0))
                .unwrap()
                .then(a.id.cmp(&b.id))
        });
        let best: Vec<i64> = ranked.iter().take(3).map(|f| f.photo_id).collect();
        for f in &faces {
            cells.insert(f.photo_id.to_string(), Some(f.clone()));
        }
        tracks.push(TrackOut {
            track_id: tid,
            person_id: top.as_ref().map(|((p, _), _)| *p),
            person_name: top.and_then(|((_, n), _)| n),
            cells,
            best_photo_ids: best,
        });
    }
    Ok(BurstFacesOut { photo_ids, tracks })
}

// ====================================================================== manual group edits

fn burst_photos_by_time(conn: &Connection, burst_id: i64) -> Result<Vec<(i64, i64)>> {
    let mut st = conn.prepare(
        "SELECT id, COALESCE(taken_at, mtime, 0) FROM photo WHERE burst_id=?1
         ORDER BY COALESCE(taken_at, mtime, 0), lower(file_name), id",
    )?;
    let v = st
        .query_map([burst_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

fn refresh_burst_stats(conn: &Connection, burst_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE burst SET
           size=(SELECT COUNT(*) FROM photo WHERE burst_id=?1),
           start_at=(SELECT MIN(COALESCE(taken_at, mtime, 0)) FROM photo WHERE burst_id=?1),
           end_at=(SELECT MAX(COALESCE(taken_at, mtime, 0)) FROM photo WHERE burst_id=?1)
         WHERE id=?1",
        [burst_id],
    )?;
    Ok(())
}

fn refresh_scene_bounds(conn: &Connection, scene_id: i64) -> Result<()> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM burst WHERE scene_id=?1",
        [scene_id],
        |r| r.get(0),
    )?;
    if n == 0 {
        conn.execute("DELETE FROM scene WHERE id=?1", [scene_id])?;
    } else {
        conn.execute(
            "UPDATE scene SET start_at=(SELECT MIN(start_at) FROM burst WHERE scene_id=?1),
                              end_at=(SELECT MAX(end_at) FROM burst WHERE scene_id=?1) WHERE id=?1",
            [scene_id],
        )?;
    }
    Ok(())
}

pub struct SplitOutcome {
    pub burst_ids: [i64; 2],
    pub session_id: i64,
}

/// `at_photo_id` and everything after it (in time order) moves to a new burst; both become manual.
pub fn split_burst(conn: &mut Connection, burst_id: i64, at_photo_id: i64) -> Result<SplitOutcome> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (session_id, scene_id): (Option<i64>, Option<i64>) = tx
        .query_row(
            "SELECT session_id, scene_id FROM burst WHERE id=?1",
            [burst_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::not_found(format!("burst {burst_id} not found"))
            }
            e => e.into(),
        })?;
    let photos = burst_photos_by_time(&tx, burst_id)?;
    let idx = photos
        .iter()
        .position(|(id, _)| *id == at_photo_id)
        .ok_or_else(|| CoreError::bad_request("at_photo_id is not in this burst"))?;
    if idx == 0 {
        return Err(CoreError::bad_request(
            "cannot split before the first photo of the burst",
        ));
    }
    tx.execute(
        "INSERT INTO burst(session_id, scene_id, size, manual) VALUES(?1,?2,0,1)",
        params![session_id, scene_id],
    )?;
    let new_id = tx.last_insert_rowid();
    for (pid, _) in &photos[idx..] {
        tx.execute(
            "UPDATE photo SET burst_id=?2 WHERE id=?1",
            params![pid, new_id],
        )?;
    }
    tx.execute("UPDATE burst SET manual=1 WHERE id=?1", [burst_id])?;
    refresh_burst_stats(&tx, burst_id)?;
    refresh_burst_stats(&tx, new_id)?;
    if let Some(s) = scene_id {
        refresh_scene_bounds(&tx, s)?;
    }
    tx.commit()?;
    Ok(SplitOutcome {
        burst_ids: [burst_id, new_id],
        session_id: session_id.unwrap_or(0),
    })
}

pub struct MergeOutcome {
    pub burst_id: i64,
    pub session_id: i64,
}

/// Merges bursts of one session into the earliest one (which becomes manual).
pub fn merge_bursts(conn: &mut Connection, burst_ids: &[i64]) -> Result<MergeOutcome> {
    let mut ids: Vec<i64> = burst_ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    if ids.len() < 2 {
        return Err(CoreError::bad_request(
            "give at least two distinct burst_ids",
        ));
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    struct B {
        id: i64,
        session: Option<i64>,
        scene: Option<i64>,
        start: i64,
    }
    let mut bursts = Vec::new();
    for id in &ids {
        let b = tx
            .query_row(
                "SELECT id, session_id, scene_id, COALESCE(start_at,0) FROM burst WHERE id=?1",
                [id],
                |r| {
                    Ok(B {
                        id: r.get(0)?,
                        session: r.get(1)?,
                        scene: r.get(2)?,
                        start: r.get(3)?,
                    })
                },
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    CoreError::not_found(format!("burst {id} not found"))
                }
                e => e.into(),
            })?;
        bursts.push(b);
    }
    if bursts.iter().any(|b| b.session != bursts[0].session) {
        return Err(CoreError::bad_request(
            "bursts belong to different sessions",
        ));
    }
    bursts.sort_by_key(|b| (b.start, b.id));
    let target = &bursts[0];
    let mut scenes: Vec<i64> = bursts.iter().filter_map(|b| b.scene).collect();
    scenes.sort_unstable();
    scenes.dedup();
    let target_scene = target.scene.or_else(|| scenes.first().copied());
    for b in &bursts[1..] {
        tx.execute(
            "UPDATE photo SET burst_id=?2 WHERE burst_id=?1",
            params![b.id, target.id],
        )?;
        tx.execute("DELETE FROM burst WHERE id=?1", [b.id])?;
    }
    if let Some(ts) = target_scene {
        // everything of the other scenes involved joins the target scene
        for s in scenes.iter().filter(|s| **s != ts) {
            tx.execute(
                "UPDATE burst SET scene_id=?2 WHERE scene_id=?1",
                params![s, ts],
            )?;
            tx.execute("DELETE FROM scene WHERE id=?1", [s])?;
        }
        tx.execute(
            "UPDATE burst SET scene_id=?2 WHERE id=?1",
            params![target.id, ts],
        )?;
    }
    tx.execute("UPDATE burst SET manual=1 WHERE id=?1", [target.id])?;
    refresh_burst_stats(&tx, target.id)?;
    if let Some(ts) = target_scene {
        refresh_scene_bounds(&tx, ts)?;
    }
    let out = MergeOutcome {
        burst_id: target.id,
        session_id: target.session.unwrap_or(0),
    };
    tx.commit()?;
    Ok(out)
}

pub fn burst_photo_ids(conn: &Connection, burst_id: i64) -> Result<Vec<i64>> {
    let mut st = conn.prepare("SELECT id FROM photo WHERE burst_id=?1")?;
    let v = st
        .query_map([burst_id], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<i64>>>()?;
    Ok(v)
}

// ====================================================================== people

fn person_row(r: &rusqlite::Row) -> rusqlite::Result<Person> {
    Ok(Person {
        id: r.get(0)?,
        name: r.get(1)?,
        cover_face_id: r.get(2)?,
        photo_count: r.get(3)?,
        hidden: r.get::<_, i64>(4)? != 0,
        singleton: r.get::<_, i64>(5)? != 0,
    })
}

/// People of a session (or all). Single-photo subjects (`singleton`) are left out unless
/// `include_singletons`.
pub fn list_people(
    conn: &Connection,
    session_id: Option<i64>,
    include_singletons: bool,
) -> Result<Vec<Person>> {
    let sql = match session_id {
        Some(_) => {
            "SELECT pe.id, pe.name, pe.cover_face_id, COUNT(DISTINCT f.photo_id), pe.hidden, pe.singleton
             FROM person pe JOIN face f ON f.person_id=pe.id
             JOIN session_photo sp ON sp.photo_id=f.photo_id AND sp.session_id=?1
             WHERE (?2 OR pe.singleton=0)
             GROUP BY pe.id ORDER BY 4 DESC, pe.id"
        }
        None => {
            "SELECT pe.id, pe.name, pe.cover_face_id, COUNT(DISTINCT f.photo_id), pe.hidden, pe.singleton
             FROM person pe JOIN face f ON f.person_id=pe.id
             WHERE ?1 IS NULL AND (?2 OR pe.singleton=0)
             GROUP BY pe.id ORDER BY 4 DESC, pe.id"
        }
    };
    let mut st = conn.prepare(sql)?;
    let v = st
        .query_map(params![session_id, include_singletons], person_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

pub fn get_person(conn: &Connection, id: i64) -> Result<Person> {
    conn.query_row(
        "SELECT pe.id, pe.name, pe.cover_face_id,
                (SELECT COUNT(DISTINCT photo_id) FROM face WHERE person_id=pe.id), pe.hidden, pe.singleton
         FROM person pe WHERE pe.id=?1",
        [id],
        person_row,
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => {
            CoreError::not_found(format!("person {id} not found"))
        }
        e => e.into(),
    })
}

pub fn sessions_of_people(conn: &Connection, person_ids: &[i64]) -> Result<Vec<i64>> {
    if person_ids.is_empty() {
        return Ok(vec![]);
    }
    let mut st = conn.prepare(&format!(
        "SELECT DISTINCT sp.session_id FROM face f JOIN session_photo sp ON sp.photo_id=f.photo_id
         WHERE f.person_id IN ({})",
        ids_sql(person_ids)
    ))?;
    let v = st
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<i64>>>()?;
    Ok(v)
}

pub fn sessions_of_photos(conn: &Connection, photo_ids: &[i64]) -> Result<Vec<i64>> {
    if photo_ids.is_empty() {
        return Ok(vec![]);
    }
    let mut out = HashSet::new();
    for chunk in photo_ids.chunks(500) {
        let mut st = conn.prepare(&format!(
            "SELECT DISTINCT session_id FROM session_photo WHERE photo_id IN ({})",
            placeholders(chunk.len())
        ))?;
        for r in st.query_map(params_from_iter(chunk.iter()), |r| r.get::<_, i64>(0))? {
            out.insert(r?);
        }
    }
    let mut v: Vec<i64> = out.into_iter().collect();
    v.sort_unstable();
    Ok(v)
}

pub fn photos_of_person(conn: &Connection, person_id: i64) -> Result<Vec<i64>> {
    let mut st = conn.prepare("SELECT DISTINCT photo_id FROM face WHERE person_id=?1")?;
    let v = st
        .query_map([person_id], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<i64>>>()?;
    Ok(v)
}

pub fn patch_person(conn: &mut Connection, id: i64, patch: &PersonPatch) -> Result<Person> {
    if patch.name.is_none() && patch.hidden.is_none() {
        return Err(CoreError::bad_request(
            "nothing to update: give name or hidden",
        ));
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    get_person(&tx, id)?;
    if let Some(name) = &patch.name {
        let name = name
            .as_ref()
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty());
        tx.execute("UPDATE person SET name=?2 WHERE id=?1", params![id, name])?;
    }
    if let Some(h) = patch.hidden {
        tx.execute(
            "UPDATE person SET hidden=?2 WHERE id=?1",
            params![id, h as i64],
        )?;
    }
    let p = get_person(&tx, id)?;
    tx.commit()?;
    Ok(p)
}

/// Faces of `ids` move to `into` (locked there); merged people disappear.
pub fn merge_people(conn: &mut Connection, ids: &[i64], into: i64) -> Result<Person> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    get_person(&tx, into)?;
    let others: Vec<i64> = ids.iter().copied().filter(|i| *i != into).collect();
    if others.is_empty() {
        return Err(CoreError::bad_request(
            "ids must contain at least one person other than `into`",
        ));
    }
    let mut inherit_name: Option<String> = None;
    for o in &others {
        let (name,): (Option<String>,) = tx
            .query_row("SELECT name FROM person WHERE id=?1", [o], |r| {
                Ok((r.get(0)?,))
            })
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    CoreError::not_found(format!("person {o} not found"))
                }
                e => e.into(),
            })?;
        if inherit_name.is_none() {
            inherit_name = name;
        }
    }
    tx.execute(
        &format!(
            "UPDATE face SET person_id=?1, person_locked=1 WHERE person_id IN ({})",
            ids_sql(&others)
        ),
        [into],
    )?;
    tx.execute(
        "UPDATE person SET name=COALESCE(name, ?2) WHERE id=?1",
        params![into, inherit_name],
    )?;
    tx.execute(
        &format!("DELETE FROM person WHERE id IN ({})", ids_sql(&others)),
        [],
    )?;
    recompute_person(&tx, into)?;
    let p = get_person(&tx, into)?;
    tx.commit()?;
    Ok(p)
}

/// Assigns a face to a person (`None` = "not this person": a new single-face person).
pub fn set_face_person(
    conn: &mut Connection,
    face_id: i64,
    person_id: Option<i64>,
) -> Result<(Face, i64, Vec<i64>)> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (photo_id, old): (i64, Option<i64>) = tx
        .query_row(
            "SELECT photo_id, person_id FROM face WHERE id=?1",
            [face_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::not_found(format!("face {face_id} not found"))
            }
            e => e.into(),
        })?;
    let target = match person_id {
        Some(p) => {
            get_person(&tx, p)?;
            p
        }
        None => {
            tx.execute("INSERT INTO person(created_at) VALUES(?1)", [now_ms()])?;
            tx.last_insert_rowid()
        }
    };
    // a second face of the same person in this photo would break the cannot-link rule
    let clash: i64 = tx.query_row(
        "SELECT COUNT(*) FROM face WHERE photo_id=?1 AND person_id=?2 AND id<>?3",
        params![photo_id, target, face_id],
        |r| r.get(0),
    )?;
    if clash > 0 {
        return Err(CoreError::bad_request(
            "another face of this photo already belongs to that person",
        ));
    }
    tx.execute(
        "UPDATE face SET person_id=?2, person_locked=1 WHERE id=?1",
        params![face_id, target],
    )?;
    recompute_person(&tx, target)?;
    if let Some(o) = old {
        if o != target {
            recompute_person(&tx, o)?;
        }
    }
    let face = get_face(&tx, face_id)?;
    tx.commit()?;
    let mut touched = vec![target];
    touched.extend(old);
    let _ = photo_id;
    Ok((face, photo_id, touched))
}

/// Result of [`create_person_from_faces`].
#[derive(Debug)]
pub struct NewPersonOutcome {
    pub person: Person,
    /// Photos of the moved faces.
    pub photo_ids: Vec<i64>,
    /// People the faces were taken from (they may be gone now).
    pub left_people: Vec<i64>,
}

/// "This is a new person": the faces move to a fresh person (named when `name` is not blank),
/// locked there like any user assignment. Old people get their cover and centre recomputed
/// and disappear when emptied (unless named).
pub fn create_person_from_faces(
    conn: &mut Connection,
    face_ids: &[i64],
    name: Option<&str>,
) -> Result<NewPersonOutcome> {
    let mut ids = face_ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Err(CoreError::bad_request("face_ids must not be empty"));
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // face -> (photo, current person)
    let mut found: HashMap<i64, (i64, Option<i64>)> = HashMap::with_capacity(ids.len());
    for chunk in ids.chunks(500) {
        let mut st = tx.prepare(&format!(
            "SELECT id, photo_id, person_id FROM face WHERE id IN ({})",
            placeholders(chunk.len())
        ))?;
        let rows = st.query_map(params_from_iter(chunk.iter()), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, Option<i64>>(2)?,
            ))
        })?;
        for row in rows {
            let (id, photo_id, person_id) = row?;
            found.insert(id, (photo_id, person_id));
        }
    }
    if let Some(missing) = ids.iter().find(|i| !found.contains_key(*i)) {
        return Err(CoreError::not_found(format!("face {missing} not found")));
    }
    // a person appears at most once per photo (the cannot-link rule of the clustering)
    let mut photo_ids: Vec<i64> = found.values().map(|(p, _)| *p).collect();
    photo_ids.sort_unstable();
    if photo_ids.windows(2).any(|w| w[0] == w[1]) {
        return Err(CoreError::bad_request(
            "two of the faces are in the same photo; a person appears at most once per photo",
        ));
    }
    let mut left_people: Vec<i64> = found.values().filter_map(|(_, p)| *p).collect();
    left_people.sort_unstable();
    left_people.dedup();
    let name = name.map(str::trim).filter(|n| !n.is_empty());
    tx.execute(
        "INSERT INTO person(created_at, name) VALUES(?1, ?2)",
        params![now_ms(), name],
    )?;
    let id = tx.last_insert_rowid();
    for chunk in ids.chunks(500) {
        tx.execute(
            &format!(
                "UPDATE face SET person_id=?1, person_locked=1 WHERE id IN ({})",
                ids_sql(chunk)
            ),
            [id],
        )?;
    }
    recompute_person(&tx, id)?;
    for p in &left_people {
        recompute_person(&tx, *p)?;
    }
    let person = get_person(&tx, id)?;
    tx.commit()?;
    Ok(NewPersonOutcome {
        person,
        photo_ids,
        left_people,
    })
}

/// Writes user ratings from the AI stars of analysed photos; returns the updates.
pub fn accept_ai(conn: &mut Connection, ids: &[i64]) -> Result<Vec<crate::model::PhotoUpdate>> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut out = Vec::new();
    for chunk in ids.chunks(500) {
        let ph = placeholders(chunk.len());
        tx.execute(
            &format!(
                "UPDATE photo SET user_rating = CAST(ROUND(ai_rating) AS INTEGER)
                 WHERE ai_rating IS NOT NULL AND id IN ({ph})"
            ),
            params_from_iter(chunk.iter()),
        )?;
        let mut st = tx.prepare(&format!(
            "SELECT id, user_rating, COALESCE(flag,0), color_label FROM photo
             WHERE ai_rating IS NOT NULL AND id IN ({ph})"
        ))?;
        let rows = st
            .query_map(params_from_iter(chunk.iter()), |r| {
                Ok(crate::model::PhotoUpdate {
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

/// Clears the grouping of a session (used when its photos are deleted).
pub fn purge_session_groups(conn: &Connection, session_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE photo SET burst_id=NULL, rank_in_burst=NULL
         WHERE burst_id IN (SELECT id FROM burst WHERE session_id=?1)",
        [session_id],
    )?;
    conn.execute(
        "UPDATE burst SET scene_id=NULL WHERE session_id=?1",
        [session_id],
    )?;
    conn.execute("DELETE FROM burst WHERE session_id=?1", [session_id])?;
    conn.execute("DELETE FROM scene WHERE session_id=?1", [session_id])?;
    Ok(())
}

/// Removes unnamed people that no face references any more.
pub fn purge_orphan_people(conn: &Connection) -> Result<()> {
    conn.execute(
        "DELETE FROM person WHERE name IS NULL AND NOT EXISTS(SELECT 1 FROM face WHERE person_id=person.id)",
        [],
    )?;
    Ok(())
}
