//! `GET /api/search`: natural-language photo search (docs/api-contract-m2.md "语义搜索").

use axum::extract::{RawQuery, State};
use axum::Json;
use ip_core::search::SearchRequest;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{ApiError, ApiResult};
use crate::routes::{photo_query, AppState, PhotosParams};

#[derive(Deserialize)]
struct SearchParams {
    q: Option<String>,
    min_score: Option<f64>,
    limit: Option<usize>,
}

/// Query keys that are not photo filters (everything else needs `session_id`).
const OWN_KEYS: [&str; 4] = ["q", "min_score", "limit", "session_id"];

/// `q` (required), `session_id` (omit to search the whole library), `limit` (default 200, max
/// 5000), `min_score` (0..1, default 0.05) and, with `session_id`, every filter of
/// `GET /api/photos` (`sort` and `cursor` do not apply: the result is ranked).
pub async fn search(State(st): State<AppState>, RawQuery(raw): RawQuery) -> ApiResult<Json<Value>> {
    let raw = raw.unwrap_or_default();
    let own: SearchParams = serde_urlencoded::from_str(&raw)
        .map_err(|e| ApiError::bad_request(format!("invalid query: {e}")))?;
    let pairs: Vec<(String, String)> = serde_urlencoded::from_str(&raw)
        .map_err(|e| ApiError::bad_request(format!("invalid query: {e}")))?;
    let mut params: PhotosParams = serde_urlencoded::from_str(&raw)
        .map_err(|e| ApiError::bad_request(format!("invalid query: {e}")))?;
    params.clear_paging();
    let has_session = pairs
        .iter()
        .any(|(k, v)| k == "session_id" && !v.is_empty());
    let filter = if has_session {
        Some(photo_query(params)?)
    } else {
        if let Some((k, _)) = pairs
            .iter()
            .find(|(k, v)| !OWN_KEYS.contains(&k.as_str()) && !v.is_empty())
        {
            return Err(ApiError::bad_request(format!(
                "{k} is a photo filter: it needs session_id"
            )));
        }
        None
    };
    let out = st
        .core
        .search_photos(SearchRequest {
            q: own.q.unwrap_or_default(),
            filter,
            limit: own.limit,
            min_score: own.min_score,
        })
        .await?;
    Ok(Json(json!(out)))
}
