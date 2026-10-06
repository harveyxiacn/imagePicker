//! M4 routes (docs/api-contract-m4.md section C).

use axum::extract::{FromRequest, Multipart, Request, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use ip_core::analysis::faces::MAX_UPLOAD_BYTES;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{ApiError, ApiJson, ApiPath, ApiQuery, ApiResult};
use crate::routes::{photo_query, AppState, PhotosParams};

// ------------------------------------------------------------------ portrait

pub async fn photo_people(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.core.photo_people(id).await?)))
}

pub async fn beauty_prepare(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<Response> {
    let task_id = st.core.beauty_prepare(id).await?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "task_id": task_id }))).into_response())
}

pub async fn get_beauty_profile(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        json!({ "profile": st.core.beauty_profile(id).await? }),
    ))
}

#[derive(Deserialize)]
pub struct ProfileBody {
    #[serde(default)]
    profile: Option<Value>,
}

pub async fn put_beauty_profile(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<i64>,
    ApiJson(b): ApiJson<ProfileBody>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        json!({ "profile": st.core.set_beauty_profile(id, b.profile).await? }),
    ))
}

#[derive(Deserialize)]
pub struct ApplyBody {
    photo_ids: Vec<i64>,
}

pub async fn apply_profiles(
    State(st): State<AppState>,
    ApiJson(b): ApiJson<ApplyBody>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        json!({ "updated": st.core.apply_profiles(b.photo_ids).await? }),
    ))
}

// ------------------------------------------------------------------ people / faces

#[derive(Deserialize)]
pub struct BestParams {
    session_id: Option<i64>,
    ids: Option<String>,
    n: Option<usize>,
}

pub async fn people_best(
    State(st): State<AppState>,
    ApiQuery(p): ApiQuery<BestParams>,
) -> ApiResult<Json<Value>> {
    let ids = match p.ids.as_deref().filter(|s| !s.trim().is_empty()) {
        None => None,
        Some(s) => Some(
            s.split(',')
                .map(|x| {
                    x.trim()
                        .parse::<i64>()
                        .map_err(|_| ApiError::bad_request("ids must be comma separated ids"))
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
    };
    let people = st
        .core
        .best_of_people(p.session_id, ids, p.n.unwrap_or(3))
        .await?;
    Ok(Json(json!({ "people": people })))
}

#[derive(Deserialize)]
struct SearchJson {
    face_id: i64,
    #[serde(default)]
    session_id: Option<i64>,
}

fn multipart_err(e: axum::extract::multipart::MultipartError) -> ApiError {
    ApiError::new(e.status(), "bad_request", e.body_text())
}

/// `multipart/form-data` (`image`, optional `session_id`, `face_index`) or JSON `{face_id}`.
pub async fn faces_search(State(st): State<AppState>, req: Request) -> ApiResult<Json<Value>> {
    let is_multipart = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.to_ascii_lowercase().starts_with("multipart/form-data"));
    if !is_multipart {
        let Json(b): Json<SearchJson> = Json::from_request(req, &()).await.map_err(|e| {
            let status = e.status();
            ApiError::new(status, "bad_request", e.body_text())
        })?;
        return Ok(Json(json!(
            st.core.faces_search_face(b.face_id, b.session_id).await?
        )));
    }
    let mut mp = Multipart::from_request(req, &())
        .await
        .map_err(|e| ApiError::new(e.status(), "bad_request", e.body_text()))?;
    let (mut image, mut session_id, mut face_index) = (None, None, None);
    while let Some(field) = mp.next_field().await.map_err(multipart_err)? {
        match field.name().unwrap_or("") {
            "image" => image = Some(field.bytes().await.map_err(multipart_err)?),
            "session_id" => {
                let t = field.text().await.map_err(multipart_err)?;
                if !t.trim().is_empty() {
                    session_id = Some(
                        t.trim()
                            .parse::<i64>()
                            .map_err(|_| ApiError::bad_request("session_id must be an integer"))?,
                    );
                }
            }
            "face_index" => {
                let t = field.text().await.map_err(multipart_err)?;
                if !t.trim().is_empty() {
                    face_index = Some(t.trim().parse::<usize>().map_err(|_| {
                        ApiError::bad_request("face_index must be a non-negative integer")
                    })?);
                }
            }
            _ => {}
        }
    }
    let image =
        image.ok_or_else(|| ApiError::bad_request("multipart field `image` is required"))?;
    Ok(Json(json!(
        st.core
            .faces_search_image(image.to_vec(), session_id, face_index)
            .await?
    )))
}

/// Request body limit of the face-search route.
pub const SEARCH_BODY_LIMIT: usize = MAX_UPLOAD_BYTES + 1024 * 1024;

// ------------------------------------------------------------------ collections

pub async fn list_collections(State(st): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "collections": st.core.collections().await? })))
}

#[derive(Deserialize)]
pub struct CollectionBody {
    name: String,
    query: String,
}

fn check_query(q: &str) -> Result<(), ApiError> {
    photo_query(PhotosParams::from_collection_query(q)?).map(|_| ())
}

pub async fn create_collection(
    State(st): State<AppState>,
    ApiJson(b): ApiJson<CollectionBody>,
) -> ApiResult<Response> {
    check_query(&b.query)?;
    let c = st.core.create_collection(&b.name, &b.query).await?;
    Ok((StatusCode::CREATED, Json(json!({ "collection": c }))).into_response())
}

#[derive(Deserialize)]
pub struct CollectionPatch {
    name: Option<String>,
    query: Option<String>,
}

pub async fn patch_collection(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiJson(b): ApiJson<CollectionPatch>,
) -> ApiResult<Json<Value>> {
    if let Some(q) = &b.query {
        check_query(q)?;
    }
    Ok(Json(
        json!({ "collection": st.core.update_collection(&id, b.name, b.query).await? }),
    ))
}

pub async fn delete_collection(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
) -> ApiResult<StatusCode> {
    st.core.delete_collection(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ------------------------------------------------------------------ taste

pub async fn taste(State(st): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.core.taste().await?)))
}

pub async fn taste_reset(State(st): State<AppState>) -> ApiResult<StatusCode> {
    st.core.reset_taste().await?;
    Ok(StatusCode::NO_CONTENT)
}
