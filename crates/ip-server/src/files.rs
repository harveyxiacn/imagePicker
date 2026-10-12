//! Folders and files (docs/api-contract-m1.md "RAW+JPEG 与文件"): rescan, relocate a moved
//! folder, safe reject.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use ip_core::{RejectFilesRequest, RelocateRequest};
use serde_json::{json, Value};

use crate::error::{ApiJson, ApiPath, ApiResult};
use crate::routes::{whitelisted, AppState};

/// `POST /api/sessions/{id}/rescan`: look at the folder again in the background (what file
/// watching does by itself); answers 202 at once.
pub async fn rescan(State(st): State<AppState>, ApiPath(id): ApiPath<i64>) -> ApiResult<Response> {
    st.core.session(id).await?;
    let core = st.core.clone();
    tokio::spawn(async move {
        if let Err(e) = core.rescan_session(id).await {
            tracing::warn!(session = id, error = %e, "rescan failed");
        }
    });
    Ok((StatusCode::ACCEPTED, Json(json!({ "started": true }))).into_response())
}

/// `POST /api/sessions/{id}/relocate`: `{path, dry_run?}`.
pub async fn relocate(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
    ApiJson(mut req): ApiJson<RelocateRequest>,
) -> ApiResult<Json<Value>> {
    if !req.path.trim().is_empty() {
        req.path = whitelisted(&st, &req.path).await?;
    }
    let report = st.core.relocate_session(id, req).await?;
    Ok(Json(json!({ "relocate": report })))
}

/// `POST /api/photos/reject-files`: `{session_id, ids?, mode: "trash"|"folder", confirm: true}`.
pub async fn reject_files(
    State(st): State<AppState>,
    ApiJson(req): ApiJson<RejectFilesRequest>,
) -> ApiResult<Json<Value>> {
    let report = st.core.reject_files(req).await?;
    Ok(Json(json!({ "result": report })))
}
