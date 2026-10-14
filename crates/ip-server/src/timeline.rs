//! Event layer and duplicates (docs/api-contract-m2.md section "事件与重复", docs/09 P1-1 / P1-3).

use axum::extract::{RawQuery, State};
use axum::Json;
use serde_json::{json, Value};

use crate::error::{ApiError, ApiPath, ApiResult};
use crate::routes::AppState;

/// `GET /api/sessions/{id}/timeline`: `{"events":[{id,name,...,"scenes":[{...,"bursts":[...]}]}]}`.
pub async fn session_timeline(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.core.timeline(id).await?)))
}

/// `GET /api/photos/{id}/duplicates`: `{"group_id","photo_ids":[],"exact_ids":[]}`.
pub async fn photo_duplicates(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.core.duplicates_of(id).await?)))
}

#[derive(serde::Deserialize)]
struct TimelineParams {
    granularity: Option<String>,
    from: Option<String>,
    to: Option<String>,
    session_id: Option<i64>,
}

fn local_date(name: &str, v: &Option<String>) -> Result<Option<i64>, ApiError> {
    match v.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(s) => ip_core::library_timeline::parse_ymd(s)
            .map(Some)
            .ok_or_else(|| ApiError::bad_request(format!("{name} must be YYYY-MM-DD"))),
    }
}

/// `GET /api/timeline?granularity=year|month|day&from&to&session_id&<photo filters>`:
/// `{"granularity","from","to","total","buckets":[{"key","from","to","count","photo_ids"}],
/// "undated":{"count","photo_ids"},"events":[...]}` (docs/api-contract-m2.md C.6).
pub async fn library_timeline(
    State(st): State<AppState>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<Value>> {
    let raw = raw.unwrap_or_default();
    let p: TimelineParams = serde_urlencoded::from_str(&raw)
        .map_err(|e| ApiError::bad_request(format!("invalid query: {e}")))?;
    let granularity = match p.granularity.as_deref() {
        None => ip_core::library_timeline::Granularity::Month,
        Some(g) => ip_core::library_timeline::Granularity::parse(g)
            .ok_or_else(|| ApiError::bad_request("granularity must be year|month|day"))?,
    };
    let from_day = local_date("from", &p.from)?;
    let to_day = local_date("to", &p.to)?;
    if let (Some(a), Some(b)) = (from_day, to_day) {
        if a > b {
            return Err(ApiError::bad_request("from is after to"));
        }
    }
    let filters = crate::routes::timeline_filters(&raw)?;
    let out = st
        .core
        .library_timeline(ip_core::library_timeline::TimelineQuery {
            granularity,
            session_id: p.session_id,
            from_day,
            to_day,
            filters,
        })
        .await?;
    Ok(Json(json!(out)))
}
