//! Clock offsets of the devices of a session (docs/api-contract-m2.md "多设备时间对齐",
//! docs/09 P1-2).

use axum::extract::State;
use axum::Json;
use ip_core::analysis::time_align::OffsetsRequest;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{ApiJson, ApiPath, ApiQuery, ApiResult};
use crate::routes::AppState;

#[derive(Debug, Default, Deserialize)]
pub struct OffsetsParams {
    /// `1` / `true`: estimate again even if there is a stored estimate.
    #[serde(default)]
    pub refresh: Option<String>,
}

/// `GET /api/sessions/{id}/devices/offsets`: `{"devices":[{device_id,name,kind,photo_count,
/// offset_ms,source,estimate:{offset_ms,confidence,evidence,source}|null}],"estimated_at",
/// "reference_device_id"}`.
pub async fn get_offsets(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
    ApiQuery(p): ApiQuery<OffsetsParams>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.core.device_offsets(id, matches!(p.refresh.as_deref(), Some("1" | "true"))).await?)))
}

/// `PUT /api/sessions/{id}/devices/offsets`: `{"action":"accept"|"set"|"reset",device_id?,
/// device_ids?,offset_ms?}`; answers like the GET. The session is grouped again when an applied
/// offset changed.
pub async fn put_offsets(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
    ApiJson(req): ApiJson<OffsetsRequest>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.core.update_device_offsets(id, req).await?)))
}
