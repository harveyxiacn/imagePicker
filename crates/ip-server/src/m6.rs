//! M6 routes (docs/api-contract-m6.md sections A, C, D, F): assistant, settings, cache, models,
//! onboarding, face data, XMP sync and catalog backups.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use ip_core::assistant::PlanRequest;
use ip_core::xmp::XmpSyncRequest;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{ApiError, ApiJson, ApiPath, ApiQuery, ApiResult};
use crate::routes::{whitelisted, AppState};

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

/// The core settings plus `lan` / `roots` from the security store (never its password hashes).
pub(crate) fn full_settings(st: &AppState) -> ApiResult<Value> {
    let mut v = serde_json::to_value(&*st.core.settings())
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string()))?;
    let store = st.auth.store();
    if let Value::Object(m) = &mut v {
        m.insert("lan".into(), json!(store.lan()));
        m.insert("roots".into(), json!(store.roots()));
    }
    Ok(v)
}

fn unprocessable(msg: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "unprocessable", msg)
}

pub async fn get_settings(State(st): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(full_settings(&st)?))
}

pub async fn patch_settings(
    State(st): State<AppState>,
    ApiJson(mut patch): ApiJson<Value>,
) -> ApiResult<Json<Value>> {
    let Value::Object(obj) = &mut patch else {
        return Err(ApiError::bad_request("the body must be a JSON object"));
    };
    let store = st.auth.store();
    // `lan` and `roots` live in the security store
    let lan_patch = obj.remove("lan");
    let roots_patch = obj.remove("roots");
    let mut new_lan = None;
    if let Some(l) = lan_patch.filter(|v| !v.is_null()) {
        let Value::Object(l) = l else {
            return Err(ApiError::bad_request("lan must be an object"));
        };
        let mut lan = store.lan();
        for (k, v) in &l {
            match (k.as_str(), v) {
                ("enabled", Value::Bool(b)) => lan.enabled = *b,
                ("guest_enabled", Value::Bool(b)) => lan.guest_enabled = *b,
                ("port", Value::Number(n)) => {
                    let p = n
                        .as_u64()
                        .filter(|p| (1..=65535).contains(p))
                        .ok_or_else(|| unprocessable("lan.port must be within 1..65535"))?;
                    lan.port = p as u16;
                }
                (other, _) => {
                    return Err(ApiError::bad_request(format!(
                        "invalid lan setting {other:?}"
                    )))
                }
            }
        }
        if lan.enabled && !store.has_owner_password() {
            return Err(unprocessable(
                "set a password (POST /api/auth/password) before enabling LAN access",
            ));
        }
        new_lan = Some(lan);
    }
    let mut new_roots = None;
    if let Some(r) = roots_patch.filter(|v| !v.is_null()) {
        let list: Vec<String> = serde_json::from_value(r)
            .map_err(|_| ApiError::bad_request("roots must be a list of paths"))?;
        if list.len() > 256 || list.iter().any(|p| p.trim().is_empty() || p.contains('\0')) {
            return Err(unprocessable("roots must be at most 256 non-empty paths"));
        }
        new_roots = Some(list);
    }
    // a models directory chosen by the client must lie inside the allowed folders
    if let Some(dir) = obj
        .get("models")
        .and_then(|m| m.get("dir"))
        .and_then(Value::as_str)
        .filter(|d| !d.trim().is_empty())
    {
        if dir != st.core.settings().models.dir {
            let checked = whitelisted(&st, dir).await?;
            obj.get_mut("models")
                .and_then(Value::as_object_mut)
                .map(|m| m.insert("dir".into(), json!(checked)));
        }
    }
    st.core.patch_settings(patch).await?;
    if let Some(lan) = new_lan {
        store
            .set_lan(lan)
            .map_err(|e| unprocessable(e.to_string()))?;
    }
    if let Some(roots) = new_roots {
        store.set_roots(roots)?;
    }
    let full = full_settings(&st)?;
    st.core.emit_settings_updated(full.clone());
    Ok(Json(full))
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

// ------------------------------------------------------------------ catalog integrity / backups

pub async fn catalog_status(State(st): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.core.catalog_status().await)))
}

pub async fn catalog_backup(State(st): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.core.catalog_backup().await?)))
}

#[derive(Deserialize)]
pub struct RestoreBody {
    name: String,
}

pub async fn catalog_restore(
    State(st): State<AppState>,
    ApiJson(b): ApiJson<RestoreBody>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.core.catalog_restore(&b.name).await?)))
}
