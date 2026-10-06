//! M3 routes (docs/api-contract-m3.md section B).

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use ip_core::ip_render::Backend;
use ip_core::{AutoRequest, PresetCreate, PreviewRequest, SyncRequest};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{ApiJson, ApiPath, ApiQuery, ApiResult};
use crate::routes::AppState;

pub async fn get_edit(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.core.get_edit(id).await?)))
}

#[derive(Deserialize)]
pub struct PutEditBody {
    stack: Value,
}

pub async fn put_edit(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
    ApiJson(b): ApiJson<PutEditBody>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.core.put_edit(id, b.stack).await?)))
}

pub async fn delete_edit(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<StatusCode> {
    st.core.delete_edit(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn render_preview(
    State(st): State<AppState>,
    ApiJson(req): ApiJson<PreviewRequest>,
) -> ApiResult<Response> {
    let p = st.core.render_preview(req).await?;
    let mut resp = Response::new(Body::from(p.jpeg));
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert("x-render-ms", HeaderValue::from(p.ms));
    h.insert(
        "x-render-backend",
        HeaderValue::from_static(match p.backend {
            Backend::Gpu => "gpu",
            Backend::Cpu => "cpu",
        }),
    );
    Ok(resp)
}

pub async fn auto_edit(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
    ApiJson(req): ApiJson<AutoRequest>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        json!({ "adjust": st.core.auto_adjust(id, req.mode).await? }),
    ))
}

pub async fn sync_edits(
    State(st): State<AppState>,
    ApiJson(req): ApiJson<SyncRequest>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "updated": st.core.sync_edits(req).await? })))
}

pub async fn list_presets(State(st): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "presets": st.core.presets().await? })))
}

pub async fn create_preset(
    State(st): State<AppState>,
    ApiJson(req): ApiJson<PresetCreate>,
) -> ApiResult<Response> {
    let preset = st.core.create_preset(req).await?;
    Ok((StatusCode::CREATED, Json(json!({ "preset": preset }))).into_response())
}

pub async fn delete_preset(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
) -> ApiResult<StatusCode> {
    st.core.delete_preset(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct LutBody {
    path: String,
}

pub async fn list_luts(State(st): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "luts": st.core.luts().await? })))
}

pub async fn import_lut(
    State(st): State<AppState>,
    ApiJson(b): ApiJson<LutBody>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.core.import_lut(b.path).await?)))
}

#[derive(Deserialize)]
pub struct MaskParams {
    target: Option<String>,
    person_id: Option<String>,
}

pub async fn mask(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
    ApiQuery(q): ApiQuery<MaskParams>,
) -> ApiResult<Response> {
    let target = q
        .target
        .filter(|s| !s.is_empty())
        .ok_or_else(|| crate::error::ApiError::bad_request("target is required"))?;
    let person_id = match q.person_id.as_deref().filter(|s| !s.is_empty()) {
        None => None,
        Some(s) => Some(
            s.parse::<i64>()
                .map_err(|_| crate::error::ApiError::bad_request("person_id must be an integer"))?,
        ),
    };
    let png = st.core.mask_png(id, &target, person_id).await?;
    let mut resp = Response::new(Body::from(png));
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=3600"),
    );
    Ok(resp)
}
