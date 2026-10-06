//! M5 routes (docs/api-contract-m5.md section C) and the response float cleanup.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use ip_core::{BestTakeRequest, EnhanceBody, InpaintBody};
use serde_json::{json, Value};

use crate::error::{ApiJson, ApiPath, ApiResult};
use crate::routes::AppState;

fn accepted(task_id: String) -> Response {
    (StatusCode::ACCEPTED, Json(json!({ "task_id": task_id }))).into_response()
}

pub async fn besttake_plan(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.core.besttake_plan(id).await?)))
}

pub async fn besttake(
    State(st): State<AppState>,
    ApiJson(req): ApiJson<BestTakeRequest>,
) -> ApiResult<Response> {
    Ok(accepted(st.core.besttake_start(req).await?))
}

pub async fn besttake_auto(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<Response> {
    Ok(accepted(st.core.besttake_auto(id).await?))
}

pub async fn bystanders(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "faces": st.core.bystanders(id).await? })))
}

pub async fn inpaint(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
    ApiJson(body): ApiJson<InpaintBody>,
) -> ApiResult<Response> {
    Ok(accepted(st.core.inpaint_start(id, body).await?))
}

pub async fn enhance(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
    ApiJson(body): ApiJson<EnhanceBody>,
) -> ApiResult<Response> {
    Ok(accepted(st.core.enhance_start(id, body).await?))
}

pub async fn asset(
    State(st): State<AppState>,
    ApiPath((photo_id, asset)): ApiPath<(i64, String)>,
) -> ApiResult<Response> {
    let png = st.core.asset_png(photo_id, &asset).await?;
    let mut resp = Response::new(Body::from(png));
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    // asset ids are never reused for different pixels
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=31536000, immutable"),
    );
    Ok(resp)
}

/// Largest JSON body the float cleanup re-serialises.
const TIDY_LIMIT: usize = 64 * 1024 * 1024;

/// Removes `f32 -> f64` noise (`0.30000001192092896`) from every JSON response
/// (docs/api-contract-m5.md section D); see [`ip_core::jsonfix`].
pub async fn tidy_json(req: Request, next: Next) -> Response {
    let resp = next.run(req).await;
    let is_json = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if !is_json {
        return resp;
    }
    let (mut parts, body) = resp.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, TIDY_LIMIT).await else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "response too large").into_response();
    };
    let out = match serde_json::from_slice::<Value>(&bytes) {
        Ok(mut v) => {
            ip_core::jsonfix::tidy(&mut v);
            serde_json::to_vec(&v).unwrap_or_else(|_| bytes.to_vec())
        }
        Err(_) => bytes.to_vec(),
    };
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, Body::from(out))
}
