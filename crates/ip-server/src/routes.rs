//! REST handlers (docs/api-contract-m1.md).

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use ip_core::{
    Core, ExportRequest, FlagFilter, ImportRequest, Issue, PatchRequest, PersonMode, PersonState,
    PhotoQuery, SortKey, COLOR_LABELS,
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
    pub auth: Arc<crate::auth::AuthState>,
    pub remote: Arc<crate::remote::RemoteHost>,
}

/// Resolves a client path inside the allowed roots (403 `forbidden_path` otherwise).
pub(crate) async fn whitelisted(st: &AppState, raw: &str) -> ApiResult<String> {
    if raw.trim().is_empty() {
        return Ok(raw.to_string()); // the core rejects empty paths with its own 400
    }
    if !std::path::Path::new(raw).is_absolute() && !raw.starts_with(['\\', '/']) {
        return Err(ApiError::bad_request("path must be absolute"));
    }
    Ok(st
        .auth
        .check_path(&st.core, raw)
        .await?
        .to_string_lossy()
        .into_owned())
}

pub async fn health() -> Json<Value> {
    Json(json!({"ok": true, "version": env!("CARGO_PKG_VERSION")}))
}

pub async fn import(
    State(st): State<AppState>,
    ApiJson(mut req): ApiJson<ImportRequest>,
) -> ApiResult<Response> {
    if !req.path.trim().is_empty() {
        req.path = whitelisted(&st, &req.path).await?;
    }
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

pub async fn session_devices(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "devices": st.core.devices(id).await? })))
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
    // ---- M2 (docs/api-contract-m2.md C.4)
    ai_rating_gte: Option<f64>,
    issues_none: Option<String>,
    issues_any: Option<String>,
    burst_best_only: Option<String>,
    burst_id: Option<i64>,
    scene_type: Option<String>,
    persons: Option<String>,
    person_mode: Option<String>,
    exclude_persons: Option<String>,
    person_state: Option<String>,
    include_background: Option<String>,
    faces_min: Option<i64>,
    faces_max: Option<i64>,
    // ---- M4
    has_edits: Option<String>,
    // ---- devices
    device: Option<String>,
}

impl PhotosParams {
    /// Parameters of a smart collection's `query` string (no session/paging keys allowed).
    pub(crate) fn from_collection_query(q: &str) -> Result<Self, ApiError> {
        const KEYS: [&str; 21] = [
            "rating_gte",
            "flag",
            "color_label",
            "sort",
            "ai_rating_gte",
            "issues_none",
            "issues_any",
            "burst_best_only",
            "burst_id",
            "scene_type",
            "persons",
            "person_mode",
            "exclude_persons",
            "person_state",
            "include_background",
            "faces_min",
            "faces_max",
            "has_edits",
            "device",
            "session_id",
            "cursor",
        ];
        let q = q.trim().trim_start_matches('?');
        let pairs: Vec<(String, String)> = serde_urlencoded::from_str(q)
            .map_err(|e| ApiError::bad_request(format!("invalid query: {e}")))?;
        if let Some((k, _)) = pairs
            .iter()
            .find(|(k, _)| k != "limit" && !KEYS.contains(&k.as_str()))
        {
            return Err(ApiError::bad_request(format!(
                "unknown query parameter {k:?}"
            )));
        }
        let mut p: PhotosParams = serde_urlencoded::from_str(q)
            .map_err(|e| ApiError::bad_request(format!("invalid query: {e}")))?;
        if p.session_id.is_some() || p.cursor.is_some() || p.limit.is_some() {
            return Err(ApiError::bad_request(
                "query must not contain session_id, cursor or limit",
            ));
        }
        p.session_id = Some(0); // placeholder so the shared validation accepts it
        Ok(p)
    }
}

fn flag_of(name: &str, v: &Option<String>) -> Result<bool, ApiError> {
    match v.as_deref().filter(|s| !s.is_empty()) {
        None => Ok(false),
        Some("1") | Some("true") => Ok(true),
        Some("0") | Some("false") => Ok(false),
        Some(_) => Err(ApiError::bad_request(format!(
            "{name} must be 1/0/true/false"
        ))),
    }
}

fn csv_ids(name: &str, v: &Option<String>) -> Result<Vec<i64>, ApiError> {
    v.as_deref()
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<i64>()
                .map_err(|_| ApiError::bad_request(format!("{name} must be comma separated ids")))
        })
        .collect()
}

/// Validates the `/api/photos` parameters (also used to vet smart-collection queries).
pub(crate) fn photo_query(p: PhotosParams) -> Result<PhotoQuery, ApiError> {
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
        Some(s) => SortKey::parse(s).ok_or_else(|| {
            ApiError::bad_request("sort must be taken_at|-taken_at|name|rating|ai")
        })?,
    };
    let color_label = p.color_label.filter(|s| !s.is_empty());
    if let Some(c) = &color_label {
        if !COLOR_LABELS.contains(&c.as_str()) {
            return Err(ApiError::bad_request("unknown color_label"));
        }
    }
    if let Some(n) = p.ai_rating_gte {
        if !(0.0..=5.0).contains(&n) {
            return Err(ApiError::bad_request("ai_rating_gte must be 0..5"));
        }
    }
    let issues_any = p
        .issues_any
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            Issue::parse(s).ok_or_else(|| {
                ApiError::bad_request(
                    "issues_any must list closed_eyes|blurry|overexposed|underexposed|noisy|tilted",
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let person_mode = match p.person_mode.as_deref().filter(|s| !s.is_empty()) {
        None | Some("all") => PersonMode::All,
        Some("any") => PersonMode::Any,
        Some(_) => return Err(ApiError::bad_request("person_mode must be all|any")),
    };
    let person_state = p
        .person_state
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            PersonState::parse(s).ok_or_else(|| {
                ApiError::bad_request("person_state must list eyes_open|smiling|looking|subject")
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    for (name, v) in [("faces_min", p.faces_min), ("faces_max", p.faces_max)] {
        if v.map(|n| n < 0).unwrap_or(false) {
            return Err(ApiError::bad_request(format!("{name} must be >= 0")));
        }
    }
    let mut devices = Vec::new();
    let mut device_none = false;
    for part in p.device.as_deref().unwrap_or("").split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if part == "none" {
            device_none = true;
        } else {
            devices.push(part.parse::<i64>().map_err(|_| {
                ApiError::bad_request("device must be comma separated ids or none")
            })?);
        }
    }
    Ok(PhotoQuery {
        session_id,
        devices,
        device_none,
        rating_gte: p.rating_gte,
        flag,
        color_label,
        sort,
        cursor: p.cursor.filter(|s| !s.is_empty()),
        limit: p.limit,
        ai_rating_gte: p.ai_rating_gte,
        issues_none: flag_of("issues_none", &p.issues_none)?,
        issues_any,
        burst_best_only: flag_of("burst_best_only", &p.burst_best_only)?,
        burst_id: p.burst_id,
        scene_type: p.scene_type.filter(|s| !s.is_empty()),
        persons: csv_ids("persons", &p.persons)?,
        person_mode,
        exclude_persons: csv_ids("exclude_persons", &p.exclude_persons)?,
        person_state,
        include_background: flag_of("include_background", &p.include_background)?,
        faces_min: p.faces_min,
        faces_max: p.faces_max,
        has_edits: match p.has_edits.as_deref().filter(|s| !s.is_empty()) {
            None => None,
            Some(_) => Some(flag_of("has_edits", &p.has_edits)?),
        },
    })
}

pub async fn list_photos(
    State(st): State<AppState>,
    ApiQuery(p): ApiQuery<PhotosParams>,
) -> ApiResult<Json<PhotosOut>> {
    let page = st.core.photos(photo_query(p)?).await?;
    // Serialised straight from the rows (no intermediate `serde_json::Value` tree).
    Ok(Json(PhotosOut {
        photos: page.photos,
        total: page.total,
        next_cursor: page.next_cursor,
    }))
}

#[derive(serde::Serialize)]
pub struct PhotosOut {
    photos: Vec<ip_core::Photo>,
    total: i64,
    next_cursor: Option<String>,
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
    ApiJson(mut req): ApiJson<ExportRequest>,
) -> ApiResult<Response> {
    if !req.dest.trim().is_empty() {
        req.dest = whitelisted(&st, &req.dest).await?;
    }
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
