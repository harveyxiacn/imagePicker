//! Consistent JSON errors: `{"error": {"code": "...", "message": "..."}}`.

use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use ip_core::CoreError;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    /// Extra top-level fields of the body (e.g. `models` of a 409 `models_missing`).
    pub extra: Option<serde_json::Value>,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            extra: None,
        }
    }
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut body = serde_json::json!({"error": {"code": self.code, "message": self.message}});
        if let (Some(serde_json::Value::Object(extra)), Some(obj)) =
            (self.extra, body.as_object_mut())
        {
            obj.extend(extra);
        }
        (self.status, Json(body)).into_response()
    }
}

/// A missing original is reported without its absolute path (the photo row already names the
/// file); the full text goes to the log.
fn path_free_not_found(m: &str) -> String {
    const MISSING: &str = "original file is missing";
    if m.starts_with(MISSING) {
        tracing::warn!(detail = %m, "{MISSING}");
        return MISSING.to_string();
    }
    m.to_string()
}

impl From<CoreError> for ApiError {
    fn from(e: CoreError) -> Self {
        match &e {
            CoreError::NotFound(m) => Self::not_found(path_free_not_found(m)),
            CoreError::BadRequest(m) => Self::bad_request(m.clone()),
            CoreError::Unprocessable(m) => {
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, "unprocessable", m.clone())
            }
            CoreError::UnprocessableCode(code, m) => {
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, code, m.clone())
            }
            CoreError::Unsupported(m) => Self::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported_media",
                m.clone(),
            ),
            CoreError::Conflict(m) => Self::new(StatusCode::CONFLICT, "conflict", m.clone()),
            CoreError::ModelsMissing(models) => {
                let mut e = Self::new(StatusCode::CONFLICT, "models_missing", e.to_string());
                e.extra = Some(serde_json::json!({ "models": models }));
                e
            }
            CoreError::WorkerUnavailable(m) => {
                let missing = m.contains(ip_worker_client::RUNTIME_MISSING_TAG);
                let msg = m.replace(ip_worker_client::RUNTIME_MISSING_TAG, "");
                let mut e = Self::new(StatusCode::SERVICE_UNAVAILABLE, "worker_unavailable", msg);
                if missing {
                    // the UI offers "install the AI components"
                    e.extra = Some(serde_json::json!({ "runtime": "missing" }));
                }
                e
            }
            CoreError::WorkerTimeout(m) => {
                Self::new(StatusCode::GATEWAY_TIMEOUT, "worker_timeout", m.clone())
            }
            CoreError::ConfirmationRequired(m) => {
                Self::new(StatusCode::CONFLICT, "confirmation_required", m.clone())
            }
            CoreError::Internal(_) => {
                // The whole anyhow chain stays in the log. The body keeps the reason (the UI
                // shows it next to its generic line) with folders and photo names masked.
                tracing::error!(error = ?e, "internal error");
                Self::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    ip_core::logging::scrub(&e.to_string()),
                )
            }
        }
    }
}

impl From<JsonRejection> for ApiError {
    fn from(r: JsonRejection) -> Self {
        let status = r.status();
        let code = if status == StatusCode::UNSUPPORTED_MEDIA_TYPE {
            "unsupported_media_type"
        } else {
            "bad_request"
        };
        Self::new(status, code, r.body_text())
    }
}

impl From<QueryRejection> for ApiError {
    fn from(r: QueryRejection) -> Self {
        Self::bad_request(r.body_text())
    }
}

impl From<PathRejection> for ApiError {
    fn from(r: PathRejection) -> Self {
        Self::bad_request(r.body_text())
    }
}

#[derive(FromRequest)]
#[from_request(via(axum::Json), rejection(ApiError))]
pub struct ApiJson<T>(pub T);

#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Query), rejection(ApiError))]
pub struct ApiQuery<T>(pub T);

#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Path), rejection(ApiError))]
pub struct ApiPath<T>(pub T);

pub type ApiResult<T> = Result<T, ApiError>;
