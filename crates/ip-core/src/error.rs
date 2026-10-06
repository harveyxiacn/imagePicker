//! Error type shared by core and server.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Conflict(String),
    /// Models the analysis needs are not installed (HTTP 409 `models_missing`).
    #[error("required models are not installed: {}", .0.join(", "))]
    ModelsMissing(Vec<String>),
    /// The AI worker cannot run (HTTP 503 `worker_unavailable`).
    #[error("{0}")]
    WorkerUnavailable(String),
    #[error("{0:#}")]
    Internal(anyhow::Error),
}

pub type Result<T> = std::result::Result<T, CoreError>;

impl CoreError {
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::NotFound(msg.into())
    }
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self::BadRequest(msg.into())
    }
    /// Stable machine-readable code used in the JSON error body.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "not_found",
            Self::BadRequest(_) => "bad_request",
            Self::Conflict(_) => "conflict",
            Self::ModelsMissing(_) => "models_missing",
            Self::WorkerUnavailable(_) => "worker_unavailable",
            Self::Internal(_) => "internal",
        }
    }
}

impl From<rusqlite::Error> for CoreError {
    fn from(e: rusqlite::Error) -> Self {
        match e {
            rusqlite::Error::QueryReturnedNoRows => Self::NotFound("row not found".into()),
            other => Self::Internal(other.into()),
        }
    }
}

impl From<anyhow::Error> for CoreError {
    fn from(e: anyhow::Error) -> Self {
        Self::Internal(e)
    }
}

impl From<std::io::Error> for CoreError {
    fn from(e: std::io::Error) -> Self {
        Self::Internal(e.into())
    }
}

impl From<serde_json::Error> for CoreError {
    fn from(e: serde_json::Error) -> Self {
        Self::Internal(e.into())
    }
}
