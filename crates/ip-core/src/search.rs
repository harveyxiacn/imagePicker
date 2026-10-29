//! Natural-language photo search (PRD F2.9, docs/09 P1-4): `GET /api/search`.
//!
//! The query text goes through the worker's SigLIP2 text tower (`text.embed`); the stored image
//! embeddings (`analysis.embedding`, L2-normalised f16, written by the `embed` analysis step)
//! live in the same space, so the cosine of the two ranks the photos. SigLIP is trained with a
//! sigmoid loss, so a score that does not depend on the other photos exists:
//! `sigmoid(LOGIT_SCALE * cos + LOGIT_BIAS)`; an explicit `min_score` applies to it.
//!
//! Without `min_score` the threshold adapts to the query ([`Threshold::Adaptive`]): the cosine
//! level of a match depends on the language and wording of the query (on the 1000-photo library a
//! Chinese query's best match sits around cos 0.10-0.13, an English one around 0.12-0.17, and a
//! Chinese query is about 0.03 closer to *every* photo), so one absolute cut-off either drops the
//! right photos of a Chinese query or lets everything through for an English one. The adaptive
//! cut-off keeps a photo whose cosine is at least `ADAPTIVE_Z` standard deviations above the mean
//! of the scope, or above `CONFIDENT_SIMILARITY` in any case; `low_confidence` says when even the
//! best photo is below `CONFIDENT_SIMILARITY` (probably nothing in scope shows the query).
//!
//! The scan is exact (no index): one pass over the embeddings of the scope, which is a few
//! hundred milliseconds for 100k photos. Photos without an embedding (not analysed with the
//! `standard` profile, or analysed before the `embed` step existed) cannot be found; they are
//! counted in [`SearchOut::unindexed`] so the UI can say so instead of showing "no results".

use ip_worker_client::TextEmbedRequest;
use rusqlite::{params_from_iter, types::Value, Connection};
use serde::Serialize;

use crate::analysis::map_worker_err;
use crate::analysis::vecs::{decode_f16, dot, norm};
use crate::catalog::{push_complete_filters, push_m2_filters};
use crate::error::{CoreError, Result};
use crate::model::{FlagFilter, PhotoQuery};
use crate::Core;

/// `exp(logit_scale)` of google/siglip2-base-patch16-224 (the same constant the worker's
/// zero-shot heads use).
pub const LOGIT_SCALE: f64 = 112.67;
/// Learned `logit_bias` of the SigLIP family (about -16.5 for the base models). It is not in the
/// exported ONNX graphs, so this is the published family value rather than a read-back; it only
/// shifts the probability scale (the ranking is the cosine's), and `bench/eval-search.py`
/// reports the score of the relevant photos so it can be re-calibrated.
pub const LOGIT_BIAS: f64 = -16.5;
/// Cosine at which a photo counts as a confident match (score about 0.003). On the 1000-photo
/// library (bench/eval-search.py, 2026-10-29) the best photo of 38 of 40 queries whose concept is
/// in the library was at 0.101 or above (the exceptions, both wedding queries, find no wedding
/// photo in their top 20 either), the best photo of 19 of 20 queries for absent things (a giraffe,
/// 长颈鹿, ...) at 0.091 or below; 0.095 sits in that gap.
pub const CONFIDENT_SIMILARITY: f32 = 0.095;
/// Adaptive cut-off: standard deviations above the mean cosine of the scope.
pub const ADAPTIVE_Z: f32 = 2.0;
/// Below this many compared photos the mean / deviation say little: only `CONFIDENT_SIMILARITY`.
pub const ADAPTIVE_MIN_PHOTOS: usize = 30;
pub const DEFAULT_LIMIT: usize = 200;
pub const MAX_LIMIT: usize = 5000;
/// Longest query, in characters.
pub const MAX_QUERY_CHARS: usize = 256;

/// `sigmoid(scale * cos + bias)`.
pub fn score_of(cosine: f32) -> f64 {
    1.0 / (1.0 + (-(LOGIT_SCALE * cosine as f64 + LOGIT_BIAS)).exp())
}

fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SearchHit {
    pub photo_id: i64,
    /// SigLIP probability, 0..1 (what `min_score` applies to).
    pub score: f64,
    /// Cosine of the query and the photo embedding.
    pub similarity: f64,
}

/// How hits are cut off.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Threshold {
    /// Relative to the scope (the default, see the module doc): cosine at least
    /// `min(CONFIDENT_SIMILARITY, mean + ADAPTIVE_Z * deviation)`, or `CONFIDENT_SIMILARITY` alone
    /// below `ADAPTIVE_MIN_PHOTOS` compared photos.
    Adaptive,
    /// An explicit `min_score` (probability): `score >= min_score`. `0` keeps every photo.
    MinScore(f64),
}

/// What [`rank`] found.
#[derive(Debug, Clone, PartialEq)]
pub struct Ranked {
    pub hits: Vec<SearchHit>,
    pub total_matched: usize,
    pub searched: usize,
    pub unindexed: usize,
    /// Cosine cut-off of an adaptive threshold.
    pub min_similarity: Option<f64>,
    /// Score cut-off that was applied (the explicit one, or the score of `min_similarity`).
    pub min_score: f64,
    /// Photos were compared but even the best is below `CONFIDENT_SIMILARITY`.
    pub low_confidence: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SearchOut {
    /// The query as searched (trimmed, whitespace collapsed).
    pub query: String,
    /// Text model that embedded it.
    pub model: String,
    /// `"adaptive"` (no `min_score` in the request) or `"min_score"`.
    pub threshold: &'static str,
    /// Score cut-off that was applied: the request's `min_score`, or the score of
    /// `min_similarity` for an adaptive threshold.
    pub min_score: f64,
    /// Cosine cut-off of the adaptive threshold (`null` with an explicit `min_score`).
    pub min_similarity: Option<f64>,
    /// Even the best photo is a weak match (cosine below `CONFIDENT_SIMILARITY`): the scope
    /// probably holds nothing like the query, the hits are only the nearest photos.
    pub low_confidence: bool,
    /// Best first (cosine, then photo id), at most `limit`.
    pub hits: Vec<SearchHit>,
    /// Photos at or above the cut-off before `limit` cut the list.
    pub total_matched: usize,
    /// Photos of the scope that have an embedding and were compared.
    pub searched: usize,
    /// Photos of the scope without a usable embedding (never matched).
    pub unindexed: usize,
}

/// A search request. `filter` carries the scope (a session) and any photo filter of
/// `GET /api/photos`; `None` searches the whole library.
#[derive(Debug, Clone, Default)]
pub struct SearchRequest {
    pub q: String,
    pub filter: Option<PhotoQuery>,
    pub limit: Option<usize>,
    pub min_score: Option<f64>,
}

/// Trims and collapses whitespace; 400 for an empty or over-long query.
pub fn clean_query(q: &str) -> Result<String> {
    let q = q.split_whitespace().collect::<Vec<_>>().join(" ");
    if q.is_empty() {
        return Err(CoreError::bad_request("q is required"));
    }
    if q.chars().count() > MAX_QUERY_CHARS {
        return Err(CoreError::bad_request(format!(
            "q is limited to {MAX_QUERY_CHARS} characters"
        )));
    }
    Ok(q)
}

/// Cosine cut-off of the adaptive threshold for these cosines (see [`Threshold::Adaptive`]).
pub fn adaptive_cutoff(cosines: &[f32]) -> f64 {
    let confident = CONFIDENT_SIMILARITY as f64;
    let n = cosines.len();
    if n < ADAPTIVE_MIN_PHOTOS {
        return confident;
    }
    let mean = cosines.iter().map(|c| *c as f64).sum::<f64>() / n as f64;
    let var = cosines
        .iter()
        .map(|c| (*c as f64 - mean).powi(2))
        .sum::<f64>()
        / n as f64;
    confident.min(mean + ADAPTIVE_Z as f64 * var.sqrt())
}

/// Ranks `(photo_id, embedding)` rows against a (normalised) query vector. Rows whose embedding
/// is empty or of another dimension count as unindexed.
pub fn rank(
    query: &[f32],
    rows: impl IntoIterator<Item = (i64, Option<Vec<f32>>)>,
    threshold: Threshold,
    limit: usize,
) -> Ranked {
    let (mut scored, mut unindexed) = (Vec::new(), 0);
    let qn = norm(query).max(1e-12);
    for (photo_id, emb) in rows {
        let Some(e) = emb.filter(|e| e.len() == query.len() && !e.is_empty()) else {
            unindexed += 1;
            continue;
        };
        let en = norm(&e);
        if en < 1e-12 {
            unindexed += 1;
            continue;
        }
        scored.push((photo_id, dot(query, &e) / (qn * en)));
    }
    let searched = scored.len();
    let best = scored
        .iter()
        .map(|(_, c)| *c)
        .fold(f32::NEG_INFINITY, f32::max);
    let low_confidence = searched > 0 && best < CONFIDENT_SIMILARITY;
    let (min_score, min_similarity) = match threshold {
        Threshold::MinScore(s) => (s, None),
        Threshold::Adaptive => {
            let cosines: Vec<f32> = scored.iter().map(|(_, c)| *c).collect();
            let cut = adaptive_cutoff(&cosines);
            (score_of(cut as f32), Some(cut))
        }
    };
    let mut hits: Vec<SearchHit> = scored
        .into_iter()
        .filter(|(_, cos)| match min_similarity {
            Some(cut) => *cos as f64 >= cut,
            None => score_of(*cos) >= min_score,
        })
        .map(|(photo_id, cos)| SearchHit {
            photo_id,
            score: score_of(cos),
            similarity: cos as f64,
        })
        .collect();
    let total_matched = hits.len();
    // The cosine orders the hits: the sigmoid saturates to exactly 1.0 for strong matches, which
    // would turn their order into photo-id order.
    hits.sort_by(|a, b| {
        b.similarity
            .partial_cmp(&a.similarity)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.photo_id.cmp(&b.photo_id))
    });
    hits.truncate(limit);
    for h in &mut hits {
        h.score = round4(h.score);
        h.similarity = round4(h.similarity);
    }
    Ranked {
        hits,
        total_matched,
        searched,
        unindexed,
        min_similarity: min_similarity.map(round4),
        min_score: (min_score * 1e6).round() / 1e6,
        low_confidence,
    }
}

/// `(photo id, embedding)` of every photo in scope. In a session the photo filters of
/// `GET /api/photos` apply (same SQL); without one, every photo that is not marked missing.
fn scope_rows(
    conn: &Connection,
    filter: Option<&PhotoQuery>,
) -> Result<Vec<(i64, Option<Vec<f32>>)>> {
    let (sql, args): (String, Vec<Value>) = match filter {
        None => (
            "SELECT p.id, a.embedding FROM photo p
             LEFT JOIN analysis a ON a.photo_id = p.id
             WHERE COALESCE(p.missing,0) = 0"
                .into(),
            Vec::new(),
        ),
        Some(q) => {
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
            push_complete_filters(conn, q, &mut wheres, &mut args)?;
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
            let extra = if wheres.is_empty() {
                String::new()
            } else {
                format!(" AND {}", wheres.join(" AND "))
            };
            (
                format!(
                    "SELECT p.id, a.embedding FROM session_photo sp
                     CROSS JOIN photo p ON p.id = sp.photo_id
                     LEFT JOIN analysis a ON a.photo_id = p.id
                     WHERE sp.session_id = ?{extra}"
                ),
                args,
            )
        }
    };
    let mut st = conn.prepare(&sql)?;
    let rows = st.query_map(params_from_iter(args.iter()), |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, Option<Vec<u8>>>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, blob) = row?;
        out.push((id, blob.map(|b| decode_f16(&b))));
    }
    Ok(out)
}

impl Core {
    /// `GET /api/search`: photos ranked by how well they match a natural-language query.
    ///
    /// 409 `models_missing` (with the model id) when the text model is not installed, 503 when
    /// the worker cannot run, 400 for a bad query / threshold / limit, 404 for an unknown session.
    pub async fn search_photos(&self, req: SearchRequest) -> Result<SearchOut> {
        let q = clean_query(&req.q)?;
        let threshold = match req.min_score {
            Some(s) if !(0.0..=1.0).contains(&s) => {
                return Err(CoreError::bad_request("min_score must be 0..1"))
            }
            Some(s) => Threshold::MinScore(s),
            None => Threshold::Adaptive,
        };
        let limit = req.limit.unwrap_or(DEFAULT_LIMIT);
        if !(1..=MAX_LIMIT).contains(&limit) {
            return Err(CoreError::bad_request(format!(
                "limit must be within 1..{MAX_LIMIT}"
            )));
        }
        if let Some(f) = &req.filter {
            self.session(f.session_id).await?;
        }
        let resp = self
            .worker
            .text_embed(&TextEmbedRequest {
                texts: vec![q.clone()],
                allow_download: false,
            })
            .await
            .map_err(map_worker_err)?;
        let vector = match resp.vectors.into_iter().next() {
            Some(v) if !v.is_empty() => v,
            _ => {
                return Err(CoreError::Internal(anyhow::anyhow!(
                    "text.embed returned no vector"
                )))
            }
        };
        let filter = req.filter;
        let r = self
            .db
            .call(move |c| {
                let rows = scope_rows(c, filter.as_ref())?;
                Ok(rank(&vector, rows, threshold, limit))
            })
            .await?;
        Ok(SearchOut {
            query: q,
            model: resp.model,
            threshold: match threshold {
                Threshold::Adaptive => "adaptive",
                Threshold::MinScore(_) => "min_score",
            },
            min_score: r.min_score,
            min_similarity: r.min_similarity,
            low_confidence: r.low_confidence,
            hits: r.hits,
            total_matched: r.total_matched,
            searched: r.searched,
            unindexed: r.unindexed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_is_the_sigmoid_of_the_scaled_cosine() {
        assert!(score_of(0.0) < 1e-6);
        assert!((score_of(0.1) - 0.0053).abs() < 0.0005, "{}", score_of(0.1));
        assert!(score_of(0.3) > 0.99);
        // a confident match is a score of about 0.003
        let c = score_of(CONFIDENT_SIMILARITY);
        assert!(c > 0.002 && c < 0.004, "{c}");
    }

    #[test]
    fn query_is_cleaned_and_bounded() {
        assert_eq!(clean_query("  海边 \t 日落\n").unwrap(), "海边 日落");
        assert!(clean_query("   ").is_err());
        assert!(clean_query(&"x".repeat(MAX_QUERY_CHARS + 1)).is_err());
        assert!(clean_query(&"猫".repeat(MAX_QUERY_CHARS)).is_ok());
    }

    #[test]
    fn rank_orders_filters_and_counts() {
        let q = [1.0, 0.0];
        let rows = vec![
            (1, Some(vec![0.1, 0.995])),
            (2, Some(vec![1.0, 0.0])),
            (3, None),
            (4, Some(vec![1.0, 0.0, 0.0])), // other dimension
            (5, Some(vec![0.0, 0.0])),      // zero vector
            (6, Some(vec![0.99, 0.141])),
        ];
        let ids = |r: &Ranked| r.hits.iter().map(|h| h.photo_id).collect::<Vec<_>>();
        let r = rank(&q, rows.clone(), Threshold::MinScore(0.0), 10);
        assert_eq!(ids(&r), [2, 6, 1]);
        assert_eq!((r.total_matched, r.searched, r.unindexed), (3, 3, 3));
        assert_eq!(
            (r.min_score, r.min_similarity, r.low_confidence),
            (0.0, None, false)
        );
        let r = rank(&q, rows.clone(), Threshold::MinScore(0.05), 10);
        assert_eq!(ids(&r), [2, 6]);
        assert_eq!(r.total_matched, 2);
        let r = rank(&q, rows.clone(), Threshold::MinScore(0.0), 1);
        assert_eq!((r.hits.len(), r.total_matched), (1, 3));
        assert_eq!(r.hits[0].photo_id, 2);
        // few photos: the adaptive threshold is the confident similarity (photo 1 is at 0.1)
        let r = rank(&q, rows, Threshold::Adaptive, 10);
        assert_eq!(ids(&r), [2, 6, 1]);
        assert_eq!(r.min_similarity, Some(round4(CONFIDENT_SIMILARITY as f64)));
        assert!((r.min_score - score_of(CONFIDENT_SIMILARITY)).abs() < 1e-6);
        assert!(!r.low_confidence);
    }

    /// `n` unit vectors at cosine `cos` from the query `[1, 0]`, ids from `first`.
    fn at(first: i64, n: usize, cos: f32) -> Vec<(i64, Option<Vec<f32>>)> {
        (0..n as i64)
            .map(|i| (first + i, Some(vec![cos, (1.0 - cos * cos).sqrt()])))
            .collect()
    }

    #[test]
    fn adaptive_threshold_follows_the_scope() {
        let q = [1.0, 0.0];
        // a "Chinese-like" query: everything sits near 0.05, three photos stand out at 0.09 -
        // below the confident similarity, but far above the rest of the scope
        let mut rows = at(1, 3, 0.09);
        rows.extend(at(100, 50, 0.05));
        rows.extend(at(200, 50, 0.04));
        let r = rank(&q, rows, Threshold::Adaptive, 10);
        assert_eq!(
            r.hits.iter().map(|h| h.photo_id).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert!(
            r.low_confidence,
            "the best photo is below CONFIDENT_SIMILARITY"
        );
        let cut = r.min_similarity.unwrap();
        assert!(cut > 0.05 && cut < 0.09, "{cut}");
        // the same photos with an absolute score threshold of 0.05 (the old default): nothing
        let mut rows = at(1, 3, 0.09);
        rows.extend(at(100, 50, 0.05));
        assert_eq!(
            rank(&q, rows, Threshold::MinScore(0.05), 10).total_matched,
            0
        );
        // a scope of near-identical strong matches: the deviation is tiny, the confident
        // similarity still keeps all of them
        let mut rows = at(1, 40, 0.20);
        rows.extend(at(100, 2, 0.21));
        let r = rank(&q, rows, Threshold::Adaptive, 100);
        assert_eq!(r.total_matched, 42);
        assert!(!r.low_confidence);
        assert_eq!(r.min_similarity, Some(round4(CONFIDENT_SIMILARITY as f64)));
    }

    #[test]
    fn adaptive_cutoff_needs_enough_photos() {
        assert_eq!(adaptive_cutoff(&[0.01; 5]), CONFIDENT_SIMILARITY as f64);
        let mut c = vec![0.0f32; 40];
        c[0] = 0.2;
        let cut = adaptive_cutoff(&c);
        assert!(cut > 0.0 && cut < CONFIDENT_SIMILARITY as f64, "{cut}");
    }
}
