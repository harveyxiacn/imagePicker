//! M7 routes: the installable AI runtime (`/api/runtime`), owner only.

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use ip_core::runtime::InstallRequest;
use serde_json::{json, Value};

use crate::error::ApiResult;
use crate::routes::AppState;

pub async fn get_runtime(State(st): State<AppState>) -> Json<Value> {
    Json(st.core.runtime_status().await)
}

/// The body is optional: `{}` / no body = the extras recommended for this machine.
pub async fn install_runtime(
    State(st): State<AppState>,
    body: Option<Json<InstallRequest>>,
) -> ApiResult<Response> {
    let req = body.map(|b| b.0).unwrap_or_default();
    let task_id = st.core.runtime_install(req).await?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "task_id": task_id }))).into_response())
}

pub async fn cancel_runtime(State(st): State<AppState>) -> StatusCode {
    st.core.runtime_cancel();
    StatusCode::NO_CONTENT
}

pub async fn delete_runtime(State(st): State<AppState>) -> ApiResult<StatusCode> {
    st.core.runtime_remove().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/system/diagnostics`: the diagnostics bundle (zip) as a download. Owner only: the
/// guest and device roles are refused by the auth middleware (`system` is denied to guests).
pub async fn diagnostics(State(st): State<AppState>) -> ApiResult<Response> {
    let settings = crate::m6::full_settings(&st)?;
    let bundle = st.core.diagnostics_bundle(settings).await?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/zip".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", bundle.file_name),
            ),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        bundle.bytes,
    )
        .into_response())
}
