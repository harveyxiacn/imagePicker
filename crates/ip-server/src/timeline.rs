//! Event layer and duplicates (docs/api-contract-m2.md section "事件与重复", docs/09 P1-1 / P1-3).

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::error::{ApiPath, ApiResult};
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
