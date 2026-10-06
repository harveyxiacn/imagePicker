//! M6 payloads: `llm.plan`, `vlm.suggest`, `vlm.describe` and `models.delete`
//! (`docs/api-contract-m6.md` section A.3). Parsed leniently like the other payloads.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::protocol::MaskPhoto;

/// `llm.plan` params: turn a request into tool calls (the tools are JSON schemas).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LlmPlanRequest {
    pub message: String,
    pub tools: Vec<Value>,
    pub context: Value,
    pub locale: String,
    pub allow_download: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LlmCall {
    pub tool: String,
    pub args: Value,
}

/// `llm.plan` result. Never executed directly: the core validates every call.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LlmPlanResponse {
    pub reply: String,
    pub calls: Vec<LlmCall>,
}

/// `vlm.suggest` params; `context` carries `histogram`, `scores`, `scene_type`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VlmSuggestRequest {
    pub photo: MaskPhoto,
    pub context: Value,
    pub allow_download: bool,
}

/// `vlm.suggest` result; `adjust` is an `Adjust` object (validated by the core).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct VlmSuggestResponse {
    pub problems: Vec<String>,
    pub adjust: Value,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VlmDescribeRequest {
    pub photo: MaskPhoto,
    pub locale: String,
    pub allow_download: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct VlmDescribeResponse {
    pub caption: String,
    pub keywords: Vec<String>,
}

/// Process-level options of the worker that only take effect when it (re)starts: the models
/// directory and extra environment variables (model download source, offline mode).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkerOptions {
    pub models_dir: Option<PathBuf>,
    /// `(name, value)`; names the core manages (`HF_ENDPOINT`, ...) are replaced as a set.
    pub env: Vec<(String, String)>,
}

/// JSON-RPC code of "method not found": a worker that predates the method.
pub const CODE_METHOD_NOT_FOUND: i64 = -32601;
