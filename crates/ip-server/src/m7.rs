//! M7 routes: the installable AI runtime (`/api/runtime`), owner only.

use axum::extract::State;
use axum::http::StatusCode;
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
