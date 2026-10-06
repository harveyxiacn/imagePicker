//! M2 routes (docs/api-contract-m2.md section C.2).

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use ip_core::{
    AnalysisRunRequest, MergeBurstsRequest, MergePeopleRequest, PersonPatch, SetFacePersonRequest,
    SplitRequest,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{ApiError, ApiJson, ApiPath, ApiQuery, ApiResult};
use crate::routes::AppState;

#[derive(Deserialize)]
pub struct HardwareParams {
    /// `1` starts the worker (if needed) to report tier and hardware.
    probe: Option<String>,
}

pub async fn hardware(
    State(st): State<AppState>,
    ApiQuery(p): ApiQuery<HardwareParams>,
) -> ApiResult<Json<Value>> {
    let probe = matches!(p.probe.as_deref(), Some("1") | Some("true"));
    Ok(Json(json!({ "worker": st.core.hardware(probe).await? })))
}

pub async fn models(State(st): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "models": st.core.models().await? })))
}

#[derive(Deserialize)]
pub struct EnsureBody {
    ids: Vec<String>,
}

pub async fn models_ensure(
    State(st): State<AppState>,
    ApiJson(b): ApiJson<EnsureBody>,
) -> ApiResult<Response> {
    let task_id = st.core.models_ensure(b.ids).await?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "task_id": task_id }))).into_response())
}

pub async fn analysis_run(
    State(st): State<AppState>,
    ApiJson(req): ApiJson<AnalysisRunRequest>,
) -> ApiResult<Response> {
    // the HTTP path never downloads models by itself (user consent goes through /models/ensure)
    let req = AnalysisRunRequest {
        allow_download: false,
        ..req
    };
    let task_id = st.core.analysis_run(req).await?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "task_id": task_id }))).into_response())
}

#[derive(Deserialize)]
pub struct SessionBody {
    session_id: i64,
}

pub async fn analysis_cancel(
    State(st): State<AppState>,
    ApiJson(b): ApiJson<SessionBody>,
) -> ApiResult<StatusCode> {
    st.core.session(b.session_id).await?;
    st.core.analysis_cancel(b.session_id);
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct SessionParams {
    session_id: Option<i64>,
}

fn require_session(p: &SessionParams) -> Result<i64, ApiError> {
    p.session_id
        .ok_or_else(|| ApiError::bad_request("session_id is required"))
}

pub async fn analysis_status(
    State(st): State<AppState>,
    ApiQuery(p): ApiQuery<SessionParams>,
) -> ApiResult<Json<Value>> {
    let sid = require_session(&p)?;
    st.core.session(sid).await?;
    Ok(Json(
        serde_json::to_value(st.core.analysis_status(sid)).map_err(|e| {
            ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string())
        })?,
    ))
}

pub async fn photo_analysis(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.core.photo_analysis(id).await?)))
}

pub async fn groups(
    State(st): State<AppState>,
    ApiQuery(p): ApiQuery<SessionParams>,
) -> ApiResult<Json<Value>> {
    let sid = require_session(&p)?;
    Ok(Json(json!(st.core.groups(sid).await?)))
}

pub async fn groups_split(
    State(st): State<AppState>,
    ApiJson(req): ApiJson<SplitRequest>,
) -> ApiResult<Json<Value>> {
    let ids = st.core.split_burst(req).await?;
    Ok(Json(json!({ "burst_ids": ids })))
}

pub async fn groups_merge(
    State(st): State<AppState>,
    ApiJson(req): ApiJson<MergeBurstsRequest>,
) -> ApiResult<Json<Value>> {
    let id = st.core.merge_bursts(req).await?;
    Ok(Json(json!({ "burst_id": id })))
}

pub async fn burst_faces(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.core.burst_faces(id).await?)))
}

#[derive(Deserialize)]
pub struct CropParams {
    s: Option<u32>,
}

pub async fn face_crop(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
    ApiQuery(q): ApiQuery<CropParams>,
) -> ApiResult<Response> {
    let path = st.core.face_crop(id, q.s.unwrap_or(128)).await?;
    let bytes = tokio::fs::read(&path).await.map_err(|e| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            format!("cache read failed: {e}"),
        )
    })?;
    let mut resp = Response::new(Body::from(bytes));
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=86400"),
    );
    Ok(resp)
}

#[derive(Deserialize)]
pub struct IdsBody {
    ids: Vec<i64>,
}

pub async fn accept_ai(
    State(st): State<AppState>,
    ApiJson(b): ApiJson<IdsBody>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "updated": st.core.accept_ai(b.ids).await? })))
}

pub async fn people(
    State(st): State<AppState>,
    ApiQuery(p): ApiQuery<SessionParams>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        json!({ "people": st.core.people(p.session_id).await? }),
    ))
}

pub async fn patch_person(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
    ApiJson(patch): ApiJson<PersonPatch>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        json!({ "person": st.core.patch_person(id, patch).await? }),
    ))
}

pub async fn merge_people(
    State(st): State<AppState>,
    ApiJson(req): ApiJson<MergePeopleRequest>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "person": st.core.merge_people(req).await? })))
}

pub async fn set_face_person(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
    ApiJson(req): ApiJson<SetFacePersonRequest>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        json!({ "face": st.core.set_face_person(id, req.person_id).await? }),
    ))
}
