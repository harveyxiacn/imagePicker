//! Person-centred queries of M4 (docs/03 §4.4): the best photos of each person and
//! "search by face".

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use ip_worker_client::FacesEmbedRequest;
use rusqlite::{params_from_iter, Connection};
use serde::Serialize;

use super::map_worker_err;
use super::scoring::{expression_score, FaceFeat};
use super::vecs::{cosine, decode_f16};
use crate::error::{CoreError, Result};
use crate::Core;

/// Weight of the person's own expression in the "best of person" score (docs/03 §4.4).
pub const BEST_EXPRESSION_WEIGHT: f64 = 0.6;
/// Weight of the photo's overall `ai_score`.
pub const BEST_PHOTO_WEIGHT: f64 = 0.4;
pub const MAX_BEST_N: usize = 50;
/// Results of a face search below this cosine similarity are noise and are dropped.
pub const MIN_SEARCH_SIMILARITY: f32 = 0.25;
pub const SEARCH_RESULTS: usize = 10;
/// Largest accepted uploaded image.
pub const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct BestPhoto {
    pub photo_id: i64,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PersonBest {
    pub person_id: i64,
    pub photos: Vec<BestPhoto>,
}

fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

/// Best photos per person: `0.6 * that person's expression + 0.4 * the photo's ai_score`, one
/// photo per burst, rejected photos left out, top `n` each. `ids = None` means every visible person.
pub fn best_of_people(
    conn: &Connection,
    session_id: Option<i64>,
    ids: Option<&[i64]>,
    n: usize,
) -> Result<Vec<PersonBest>> {
    let mut sql = String::from(
        "SELECT f.person_id, f.photo_id, f.expression_score, f.eyes_open, f.smile, f.gaze,
                f.yaw, f.pitch, f.sharpness, p.ai_score, p.burst_id
         FROM face f JOIN photo p ON p.id=f.photo_id JOIN person pe ON pe.id=f.person_id",
    );
    let mut args: Vec<i64> = Vec::new();
    if let Some(s) = session_id {
        sql.push_str(" JOIN session_photo sp ON sp.photo_id=p.id AND sp.session_id=?");
        args.push(s);
    }
    sql.push_str(" WHERE COALESCE(p.flag,0) >= 0");
    match ids {
        Some(ids) => {
            if ids.is_empty() {
                return Ok(vec![]);
            }
            sql.push_str(&format!(
                " AND f.person_id IN ({})",
                vec!["?"; ids.len()].join(",")
            ));
            args.extend(ids.iter().copied());
        }
        None => sql.push_str(" AND pe.hidden=0 AND pe.singleton=0"),
    }
    let mut st = conn.prepare(&sql)?;
    // person -> photo -> (best expression, ai_score, burst)
    type PhotoBest = (f64, f64, Option<i64>);
    let mut by_person: BTreeMap<i64, HashMap<i64, PhotoBest>> = BTreeMap::new();
    let rows = st.query_map(params_from_iter(args.iter()), |r| {
        let feat = FaceFeat {
            eyes_open: r.get(3)?,
            smile: r.get(4)?,
            gaze: r.get(5)?,
            yaw: r.get(6)?,
            pitch: r.get(7)?,
            sharpness: r.get(8)?,
            ..Default::default()
        };
        let expr: Option<f64> = r.get(2)?;
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, i64>(1)?,
            expr.or_else(|| expression_score(&feat)).unwrap_or(0.5),
            r.get::<_, Option<f64>>(9)?.unwrap_or(0.5),
            r.get::<_, Option<i64>>(10)?,
        ))
    })?;
    for row in rows {
        let (person, photo, expr, ai, burst) = row?;
        let e = by_person
            .entry(person)
            .or_default()
            .entry(photo)
            .or_insert((expr, ai, burst));
        if expr > e.0 {
            e.0 = expr; // several faces of one person in a photo: the best one counts
        }
    }
    let mut out: Vec<PersonBest> = Vec::new();
    for (person_id, photos) in by_person {
        let mut scored: Vec<(i64, f64, Option<i64>)> = photos
            .into_iter()
            .map(|(pid, (expr, ai, burst))| {
                (
                    pid,
                    BEST_EXPRESSION_WEIGHT * expr + BEST_PHOTO_WEIGHT * ai,
                    burst,
                )
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then(a.0.cmp(&b.0)));
        let mut seen_bursts = std::collections::HashSet::new();
        let photos = scored
            .into_iter()
            .filter(|(_, _, burst)| burst.is_none_or(|b| seen_bursts.insert(b)))
            .take(n)
            .map(|(photo_id, score, _)| BestPhoto {
                photo_id,
                score: round4(score),
            })
            .collect();
        out.push(PersonBest { person_id, photos });
    }
    if let Some(ids) = ids {
        // keep the order asked for and include people without photos
        let mut by_id: HashMap<i64, PersonBest> =
            out.into_iter().map(|p| (p.person_id, p)).collect();
        let mut seen = std::collections::HashSet::new();
        return Ok(ids
            .iter()
            .filter(|i| seen.insert(**i))
            .map(|i| {
                by_id.remove(i).unwrap_or(PersonBest {
                    person_id: *i,
                    photos: vec![],
                })
            })
            .collect());
    }
    Ok(out)
}

// ------------------------------------------------------------------ face search

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Candidate {
    pub person_id: i64,
    pub person_name: Option<String>,
    pub similarity: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SimilarFace {
    pub face_id: i64,
    pub photo_id: i64,
    pub similarity: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FaceSearchOut {
    /// Normalised `[x, y, w, h]` of every face found in the query image.
    pub faces_detected: Vec<[f64; 4]>,
    /// Index into `faces_detected` of the face that was searched (`None` = no face found).
    pub query_face: Option<usize>,
    pub candidates: Vec<Candidate>,
    pub similar_faces: Vec<SimilarFace>,
}

fn sim4(x: f32) -> f64 {
    ((x as f64) * 10_000.0).round() / 10_000.0
}

/// Ranks people (by centroid) and individual faces against one embedding.
pub fn search_embedding(
    conn: &Connection,
    q: &[f32],
    session_id: Option<i64>,
    exclude_face: Option<i64>,
) -> Result<(Vec<Candidate>, Vec<SimilarFace>)> {
    let scope_join = if session_id.is_some() {
        " JOIN session_photo sp ON sp.photo_id=f.photo_id AND sp.session_id=?1"
    } else {
        ""
    };
    let args: Vec<i64> = session_id.into_iter().collect();

    let mut people: Vec<Candidate> = Vec::new();
    {
        let sql = if session_id.is_some() {
            "SELECT pe.id, pe.name, pe.center FROM person pe WHERE pe.center IS NOT NULL
             AND EXISTS (SELECT 1 FROM face f JOIN session_photo sp ON sp.photo_id=f.photo_id
                         AND sp.session_id=?1 WHERE f.person_id=pe.id)"
        } else {
            "SELECT pe.id, pe.name, pe.center FROM person pe WHERE pe.center IS NOT NULL"
        };
        let mut st = conn.prepare(sql)?;
        let rows = st.query_map(params_from_iter(args.iter()), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Vec<u8>>(2)?,
            ))
        })?;
        for row in rows {
            let (id, name, center) = row?;
            let s = cosine(q, &decode_f16(&center));
            if s >= MIN_SEARCH_SIMILARITY {
                people.push(Candidate {
                    person_id: id,
                    person_name: name,
                    similarity: sim4(s),
                });
            }
        }
    }
    people.sort_by(|a, b| {
        b.similarity
            .partial_cmp(&a.similarity)
            .unwrap()
            .then(a.person_id.cmp(&b.person_id))
    });
    people.truncate(SEARCH_RESULTS);

    let mut faces: Vec<SimilarFace> = Vec::new();
    {
        let sql = format!(
            "SELECT f.id, f.photo_id, f.embedding FROM face f{scope_join}
             WHERE f.embedding IS NOT NULL"
        );
        let mut st = conn.prepare(&sql)?;
        let rows = st.query_map(params_from_iter(args.iter()), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, Vec<u8>>(2)?,
            ))
        })?;
        for row in rows {
            let (id, photo_id, emb) = row?;
            if Some(id) == exclude_face {
                continue;
            }
            let s = cosine(q, &decode_f16(&emb));
            if s >= MIN_SEARCH_SIMILARITY {
                faces.push(SimilarFace {
                    face_id: id,
                    photo_id,
                    similarity: sim4(s),
                });
            }
        }
    }
    faces.sort_by(|a, b| {
        b.similarity
            .partial_cmp(&a.similarity)
            .unwrap()
            .then(a.face_id.cmp(&b.face_id))
    });
    faces.truncate(SEARCH_RESULTS);
    Ok((people, faces))
}

/// Recognised upload formats (by magic bytes) and the file extension to store them under.
fn sniff_extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("png")
    } else if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

impl Core {
    /// `GET /api/people/best`.
    pub async fn best_of_people(
        &self,
        session_id: Option<i64>,
        ids: Option<Vec<i64>>,
        n: usize,
    ) -> Result<Vec<PersonBest>> {
        if n == 0 || n > MAX_BEST_N {
            return Err(CoreError::bad_request(format!(
                "n must be within 1..{MAX_BEST_N}"
            )));
        }
        if let Some(s) = session_id {
            self.session(s).await?;
        }
        self.db
            .call(move |c| best_of_people(c, session_id, ids.as_deref(), n))
            .await
    }

    /// Face search with the stored embedding of a face already in the catalog.
    pub async fn faces_search_face(
        &self,
        face_id: i64,
        session_id: Option<i64>,
    ) -> Result<FaceSearchOut> {
        if let Some(s) = session_id {
            self.session(s).await?;
        }
        self.db
            .call(move |c| {
                let (_, bbox) = super::store::face_crop_ref(c, face_id)?;
                let emb: Option<Vec<u8>> =
                    c.query_row("SELECT embedding FROM face WHERE id=?1", [face_id], |r| {
                        r.get(0)
                    })?;
                let q =
                    emb.map(|b| decode_f16(&b))
                        .filter(|v| !v.is_empty())
                        .ok_or_else(|| {
                            CoreError::Conflict(
                        "this face has no identity embedding; run a standard analysis first".into(),
                    )
                        })?;
                let (candidates, similar_faces) =
                    search_embedding(c, &q, session_id, Some(face_id))?;
                Ok(FaceSearchOut {
                    faces_detected: vec![bbox],
                    query_face: Some(0),
                    candidates,
                    similar_faces,
                })
            })
            .await
    }

    /// Face search with an uploaded image (`faces.embed` on the worker). With several faces in
    /// the image the largest is searched unless `face_index` picks one.
    pub async fn faces_search_image(
        &self,
        bytes: Vec<u8>,
        session_id: Option<i64>,
        face_index: Option<usize>,
    ) -> Result<FaceSearchOut> {
        if let Some(s) = session_id {
            self.session(s).await?;
        }
        if bytes.len() > MAX_UPLOAD_BYTES {
            return Err(CoreError::bad_request("the image is too large"));
        }
        let ext = sniff_extension(&bytes).ok_or_else(|| {
            CoreError::bad_request("unsupported image: upload a JPEG, PNG or WebP file")
        })?;
        let dir = self.dirs.root.join("cache").join("uploads");
        std::fs::create_dir_all(&dir)?;
        let path: PathBuf = dir.join(format!(
            "search-{}-{}.{ext}",
            std::process::id(),
            self.next_task_seq()
        ));
        std::fs::write(&path, &bytes)?;
        let result = self.embed_and_rank(&path, session_id, face_index).await;
        let _ = std::fs::remove_file(&path);
        result
    }

    async fn embed_and_rank(
        &self,
        path: &std::path::Path,
        session_id: Option<i64>,
        face_index: Option<usize>,
    ) -> Result<FaceSearchOut> {
        let orientation = ip_imaging::ImageFormat::from_path(path)
            .and_then(|f| self.imaging.read_metadata(path, f).ok())
            .map(|m| m.orientation.clamp(1, 8))
            .unwrap_or(1);
        let resp = self
            .worker
            .faces_embed(&FacesEmbedRequest {
                path: path.to_string_lossy().into_owned(),
                orientation,
            })
            .await
            .map_err(map_worker_err)?;
        if resp.faces.len() != resp.embeddings.len() {
            return Err(CoreError::Internal(anyhow::anyhow!(
                "faces.embed returned {} faces but {} embeddings",
                resp.faces.len(),
                resp.embeddings.len()
            )));
        }
        let faces_detected: Vec<[f64; 4]> = resp.faces.iter().map(|f| f.bbox).collect();
        if faces_detected.is_empty() {
            return Ok(FaceSearchOut {
                faces_detected,
                query_face: None,
                candidates: vec![],
                similar_faces: vec![],
            });
        }
        let idx = match face_index {
            Some(i) if i < faces_detected.len() => i,
            Some(i) => {
                return Err(CoreError::bad_request(format!(
                    "face_index {i} is out of range (the image has {} faces)",
                    faces_detected.len()
                )))
            }
            None => faces_detected
                .iter()
                .enumerate()
                .max_by(|a, b| {
                    (a.1[2] * a.1[3])
                        .partial_cmp(&(b.1[2] * b.1[3]))
                        .unwrap()
                        .then(b.0.cmp(&a.0))
                })
                .map(|(i, _)| i)
                .unwrap_or(0),
        };
        let q = resp.embeddings[idx].clone();
        let (candidates, similar_faces) = self
            .db
            .call(move |c| search_embedding(c, &q, session_id, None))
            .await?;
        Ok(FaceSearchOut {
            faces_detected,
            query_face: Some(idx),
            candidates,
            similar_faces,
        })
    }
}
