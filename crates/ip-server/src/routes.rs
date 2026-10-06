//! REST handlers (docs/api-contract-m1.md).

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use ip_core::{
    Core, ExportRequest, FlagFilter, ImportRequest, PatchRequest, PhotoQuery, SortKey, COLOR_LABELS,
};
use ip_imaging::ImageFormat;
use serde::Deserialize;
use serde_json::{json, Value};
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};

use crate::error::{ApiError, ApiJson, ApiPath, ApiQuery, ApiResult};

#[derive(Clone)]
pub struct AppState {
    pub core: Arc<Core>,
    pub web_dir: Option<PathBuf>,
}

pub async fn health() -> Json<Value> {
    Json(json!({"ok": true, "version": env!("CARGO_PKG_VERSION")}))
}

pub async fn import(
    State(st): State<AppState>,
    ApiJson(req): ApiJson<ImportRequest>,
) -> ApiResult<Response> {
    let session = st.core.import(req).await?;
    Ok((StatusCode::CREATED, Json(json!({ "session": session }))).into_response())
}

pub async fn list_sessions(State(st): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "sessions": st.core.sessions().await? })))
}

pub async fn get_session(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "session": st.core.session(id).await? })))
}

pub async fn delete_session(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<StatusCode> {
    st.core.delete_session(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct PhotosParams {
    session_id: Option<i64>,
    rating_gte: Option<i64>,
    flag: Option<String>,
    color_label: Option<String>,
    sort: Option<String>,
    cursor: Option<String>,
    limit: Option<i64>,
}

pub async fn list_photos(
    State(st): State<AppState>,
    ApiQuery(p): ApiQuery<PhotosParams>,
) -> ApiResult<Json<Value>> {
    let session_id = p
        .session_id
        .ok_or_else(|| ApiError::bad_request("session_id is required"))?;
    if let Some(n) = p.rating_gte {
        if !(0..=5).contains(&n) {
            return Err(ApiError::bad_request("rating_gte must be 0..5"));
        }
    }
    let flag = match p.flag.as_deref().filter(|s| !s.is_empty()) {
        None => FlagFilter::Any,
        Some(s) => FlagFilter::parse(s).ok_or_else(|| {
            ApiError::bad_request("flag must be picked|rejected|unflagged|not_rejected")
        })?,
    };
    let sort = match p.sort.as_deref().filter(|s| !s.is_empty()) {
        None => SortKey::TakenAt,
        Some(s) => SortKey::parse(s)
            .ok_or_else(|| ApiError::bad_request("sort must be taken_at|-taken_at|name|rating"))?,
    };
    let color_label = p.color_label.filter(|s| !s.is_empty());
    if let Some(c) = &color_label {
        if !COLOR_LABELS.contains(&c.as_str()) {
            return Err(ApiError::bad_request("unknown color_label"));
        }
    }
    let page = st
        .core
        .photos(PhotoQuery {
            session_id,
            rating_gte: p.rating_gte,
            flag,
            color_label,
            sort,
            cursor: p.cursor.filter(|s| !s.is_empty()),
            limit: p.limit,
        })
        .await?;
    Ok(Json(json!({
        "photos": page.photos, "total": page.total, "next_cursor": page.next_cursor
    })))
}

pub async fn get_photo(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "photo": st.core.photo(id).await? })))
}

pub async fn patch_photos(
    State(st): State<AppState>,
    ApiJson(req): ApiJson<PatchRequest>,
) -> ApiResult<Json<Value>> {
    let updated = st.core.patch_photos(req).await?;
    Ok(Json(json!({ "updated": updated })))
}

#[derive(Deserialize)]
pub struct SizeParams {
    s: Option<u32>,
    #[allow(dead_code)]
    v: Option<String>,
}

async fn jpeg_file(path: PathBuf, cache_control: &'static str) -> ApiResult<Response> {
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
        HeaderValue::from_static(cache_control),
    );
    Ok(resp)
}

pub async fn thumb(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
    ApiQuery(q): ApiQuery<SizeParams>,
) -> ApiResult<Response> {
    let path = st.core.thumb_path(id, q.s.unwrap_or(256)).await?;
    jpeg_file(path, "public, max-age=31536000, immutable").await
}

pub async fn preview(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
    ApiQuery(q): ApiQuery<SizeParams>,
) -> ApiResult<Response> {
    let path = st.core.preview_path(id, q.s.unwrap_or(2048)).await?;
    jpeg_file(path, "public, max-age=3600").await
}

fn content_type(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::Png => "image/png",
        ImageFormat::Webp => "image/webp",
        ImageFormat::Heif => "image/heif",
        ImageFormat::Avif => "image/avif",
        ImageFormat::Tiff => "image/tiff",
        ImageFormat::Raw => "application/octet-stream",
    }
}

/// Streams the original with HTTP Range support (tower-http `ServeFile`).
pub async fn original(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
    req: Request,
) -> ApiResult<Response> {
    let (path, format) = st.core.original(id).await?;
    let mut resp = ServeFile::new(path)
        .oneshot(req)
        .await
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string()))?
        .into_response();
    if resp.status().is_success() {
        resp.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(content_type(format)),
        );
    }
    Ok(resp)
}

#[derive(Deserialize)]
pub struct ViewportBody {
    ids: Vec<i64>,
}

pub async fn viewport(
    State(st): State<AppState>,
    ApiJson(b): ApiJson<ViewportBody>,
) -> ApiResult<StatusCode> {
    st.core.viewport(b.ids).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn export(
    State(st): State<AppState>,
    ApiJson(req): ApiJson<ExportRequest>,
) -> ApiResult<Response> {
    let task_id = st.core.export(req).await?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "task_id": task_id }))).into_response())
}

/// Unknown `/api/*` -> JSON 404; everything else -> SPA from `web_dir` with `index.html` fallback.
pub async fn fallback(State(st): State<AppState>, req: Request) -> Response {
    let path = req.uri().path();
    if path == "/api" || path.starts_with("/api/") {
        return ApiError::not_found(format!("no such route: {path}")).into_response();
    }
    match &st.web_dir {
        Some(dir) => {
            let index = dir.join("index.html");
            let svc = ServeDir::new(dir).fallback(ServeFile::new(index));
            match svc.oneshot(req).await {
                Ok(resp) => resp.into_response(),
                Err(e) => {
                    ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string())
                        .into_response()
                }
            }
        }
        None => ApiError::not_found("no web UI is configured (use --web-dir)").into_response(),
    }
}

/// Fallback of the nested `/api` router (the URI there has the `/api` prefix stripped).
pub async fn api_not_found(req: Request) -> Response {
    ApiError::not_found(format!("no such route: /api{}", req.uri().path())).into_response()
}

pub async fn method_not_allowed() -> Response {
    ApiError::new(
        StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
        "method not allowed",
    )
    .into_response()
}
