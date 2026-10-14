//! Natural-language photo search (PRD F2.9, docs/09 P1-4): `GET /api/search`.
//!
//! The query text goes through the worker's SigLIP2 text tower (`text.embed`); the stored image
//! embeddings (`analysis.embedding`, L2-normalised f16, written by the `embed` analysis step)
//! live in the same space, so the cosine of the two ranks the photos. SigLIP is trained with a
//! sigmoid loss, so a score that does not depend on the other photos exists:
//! `sigmoid(LOGIT_SCALE * cos + LOGIT_BIAS)`; it is what the threshold applies to.
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
/// Default score threshold (probability): cosine of about 0.12, below which a photo is not
/// "about" the query. `min_score=0` returns everything ranked.
pub const DEFAULT_MIN_SCORE: f64 = 0.05;
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

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SearchOut {
    /// The query as searched (trimmed, whitespace collapsed).
    pub query: String,
    /// Text model that embedded it.
    pub model: String,
    pub min_score: f64,
    /// Best first (cosine, then photo id), at most `limit`.
    pub hits: Vec<SearchHit>,
    /// Photos at or above `min_score` before `limit` cut the list.
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

/// Ranks `(photo_id, embedding)` rows against a (normalised) query vector. Rows whose embedding
/// is empty or of another dimension count as unindexed.
pub fn rank(
    query: &[f32],
    rows: impl IntoIterator<Item = (i64, Option<Vec<f32>>)>,
    min_score: f64,
    limit: usize,
) -> (Vec<SearchHit>, usize, usize, usize) {
    let (mut hits, mut searched, mut unindexed, mut matched) = (Vec::new(), 0, 0, 0);
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
        searched += 1;
        let cos = dot(query, &e) / (qn * en);
        let score = score_of(cos);
        if score >= min_score {
            matched += 1;
            hits.push(SearchHit {
                photo_id,
                score,
                similarity: cos as f64,
            });
        }
    }
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
    (hits, matched, searched, unindexed)
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
        let min_score = req.min_score.unwrap_or(DEFAULT_MIN_SCORE);
        if !(0.0..=1.0).contains(&min_score) {
            return Err(CoreError::bad_request("min_score must be 0..1"));
        }
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
        let (hits, total_matched, searched, unindexed) = self
            .db
            .call(move |c| {
                let rows = scope_rows(c, filter.as_ref())?;
                Ok(rank(&vector, rows, min_score, limit))
            })
            .await?;
        Ok(SearchOut {
            query: q,
            model: resp.model,
            min_score,
            hits,
            total_matched,
            searched,
            unindexed,
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
        // the default threshold sits at a cosine of about 0.12
        assert!(score_of(0.11) < DEFAULT_MIN_SCORE && score_of(0.13) > DEFAULT_MIN_SCORE);
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
        let (hits, matched, searched, unindexed) = rank(&q, rows.clone(), 0.0, 10);
        assert_eq!(
            hits.iter().map(|h| h.photo_id).collect::<Vec<_>>(),
            [2, 6, 1]
        );
        assert_eq!((matched, searched, unindexed), (3, 3, 3));
        let (hits, matched, ..) = rank(&q, rows.clone(), 0.05, 10);
        assert_eq!(hits.iter().map(|h| h.photo_id).collect::<Vec<_>>(), [2, 6]);
        assert_eq!(matched, 2);
        let (hits, matched, ..) = rank(&q, rows, 0.0, 1);
        assert_eq!((hits.len(), matched), (1, 3));
        assert_eq!(hits[0].photo_id, 2);
    }
}
