//! M6 routes (docs/api-contract-m6.md sections A, C, D): assistant, settings, cache, models,
//! onboarding, face data and XMP sync.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use ip_core::assistant::PlanRequest;
use ip_core::xmp::XmpSyncRequest;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{ApiError, ApiJson, ApiPath, ApiQuery, ApiResult};
use crate::routes::AppState;

fn accepted(task_id: String) -> Response {
    (StatusCode::ACCEPTED, Json(json!({ "task_id": task_id }))).into_response()
}

// ------------------------------------------------------------------ assistant

#[derive(Deserialize)]
pub struct StatusParams {
    probe: Option<String>,
}

pub async fn assistant_status(
    State(st): State<AppState>,
    ApiQuery(p): ApiQuery<StatusParams>,
) -> ApiResult<Json<Value>> {
    let probe = matches!(p.probe.as_deref(), Some("1") | Some("true"));
    Ok(Json(json!(st.core.assistant_status(probe).await)))
}

pub async fn assistant_plan(
    State(st): State<AppState>,
    ApiJson(req): ApiJson<PlanRequest>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.core.assistant_plan(req).await?)))
}

#[derive(Deserialize)]
pub struct ExecuteBody {
    plan_id: String,
}

pub async fn assistant_execute(
    State(st): State<AppState>,
    ApiJson(b): ApiJson<ExecuteBody>,
) -> ApiResult<Response> {
    Ok(accepted(st.core.assistant_execute(&b.plan_id).await?))
}

#[derive(Deserialize)]
pub struct PhotoBody {
    photo_id: i64,
}

pub async fn assistant_describe(
    State(st): State<AppState>,
    ApiJson(b): ApiJson<PhotoBody>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.core.assistant_describe(b.photo_id).await?)))
}

pub async fn assistant_suggest(
    State(st): State<AppState>,
    ApiJson(b): ApiJson<PhotoBody>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.core.assistant_suggest(b.photo_id).await?)))
}

// ------------------------------------------------------------------ settings

pub async fn get_settings(State(st): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(serde_json::to_value(&*st.core.settings()).map_err(
        |e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string()),
    )?))
}

pub async fn patch_settings(
    State(st): State<AppState>,
    ApiJson(patch): ApiJson<Value>,
) -> ApiResult<Json<Value>> {
    let s = st.core.patch_settings(patch).await?;
    Ok(Json(serde_json::to_value(&*s).map_err(|e| {
        ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string())
    })?))
}

// ------------------------------------------------------------------ cache

pub async fn get_cache(State(st): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.core.cache_info().await?)))
}

#[derive(Deserialize)]
pub struct ClearBody {
    #[serde(default)]
    kinds: Vec<String>,
}

pub async fn clear_cache(
    State(st): State<AppState>,
    ApiJson(b): ApiJson<ClearBody>,
) -> ApiResult<Json<Value>> {
    let freed = st.core.cache_clear(b.kinds).await?;
    let info = st.core.cache_info().await?;
    Ok(Json(json!({"freed": freed, "cache": info})))
}

// ------------------------------------------------------------------ models / onboarding / faces

pub async fn delete_model(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
) -> ApiResult<StatusCode> {
    st.core.delete_model(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn onboarding(State(st): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(st.core.onboarding().await?))
}

pub async fn onboarding_done(State(st): State<AppState>) -> ApiResult<StatusCode> {
    st.core.onboarding_done().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct ConfirmParams {
    confirm: Option<String>,
}

pub async fn clear_faces(
    State(st): State<AppState>,
    ApiQuery(p): ApiQuery<ConfirmParams>,
) -> ApiResult<Json<Value>> {
    if !matches!(p.confirm.as_deref(), Some("true") | Some("1")) {
        return Err(ApiError::bad_request(
            "this deletes all face and people data; pass confirm=true",
        ));
    }
    Ok(Json(st.core.clear_faces().await?))
}

// ------------------------------------------------------------------ xmp / keywords

pub async fn xmp_sync(
    State(st): State<AppState>,
    ApiJson(req): ApiJson<XmpSyncRequest>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.core.xmp_sync(req).await?)))
}

pub async fn photo_tags(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "tags": st.core.photo_tags(id).await? })))
}

#[derive(Deserialize)]
pub struct TagsBody {
    ids: Vec<i64>,
    #[serde(default)]
    add: Vec<String>,
    #[serde(default)]
    remove: Vec<String>,
}

pub async fn set_tags(
    State(st): State<AppState>,
    ApiJson(b): ApiJson<TagsBody>,
) -> ApiResult<Json<Value>> {
    let n = st.core.set_tags(b.ids, b.add, b.remove).await?;
    Ok(Json(json!({ "updated": n })))
}
