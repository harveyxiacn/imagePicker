//! Typed worker errors.

use serde_json::Value;
use thiserror::Error;

/// `error.code` of a cancelled request.
pub const CODE_CANCELLED: i64 = -32800;
/// `error.code`: a required model is not installed and downloads were not allowed.
pub const CODE_MODEL_UNAVAILABLE: i64 = -32010;

#[derive(Debug, Clone, Error)]
pub enum WorkerError {
    /// The worker cannot be started (no `uv`, spawn failure, repeated crashes, ...).
    #[error("AI worker unavailable: {0}")]
    Unavailable(String),
    /// The connection to the worker was lost (crash / exit) while a request was pending.
    #[error("connection to the AI worker was lost")]
    Disconnected,
    #[error("request cancelled")]
    Cancelled,
    #[error("timeout: {0}")]
    Timeout(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    /// A JSON-RPC error answered by the worker.
    #[error("worker error {code}: {message}")]
    Rpc {
        code: i64,
        message: String,
        /// `error.data.kind`, the stable machine-readable tag.
        kind: Option<String>,
        /// `error.data.detail`.
        detail: Option<Value>,
    },
}

impl WorkerError {
    pub fn from_rpc_error(e: &Value) -> Self {
        let code = e.get("code").and_then(Value::as_i64).unwrap_or(-32603);
        if code == CODE_CANCELLED {
            return Self::Cancelled;
        }
        Self::Rpc {
            code,
            message: e
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown worker error")
                .to_string(),
            kind: e
                .pointer("/data/kind")
                .and_then(Value::as_str)
                .map(str::to_string),
            detail: e.pointer("/data/detail").cloned(),
        }
    }

    /// Ids of the missing models when this is a `-32010 model_unavailable` error.
    pub fn model_unavailable(&self) -> Option<Vec<String>> {
        match self {
            Self::Rpc {
                code, kind, detail, ..
            } if *code == CODE_MODEL_UNAVAILABLE || kind.as_deref() == Some("model_unavailable") => {
                Some(
                    detail
                        .as_ref()
                        .and_then(|d| d.get("models"))
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default(),
                )
            }
            _ => None,
        }
    }

    /// True for failures worth retrying after a restart (the process died under us).
    pub fn is_crash(&self) -> bool {
        matches!(self, Self::Disconnected)
    }
}

pub type Result<T> = std::result::Result<T, WorkerError>;
