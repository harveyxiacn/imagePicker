//! `text.embed` payloads (natural-language photo search, `docs/api-contract-m2.md` "语义搜索").

use serde::{Deserialize, Serialize};

/// `text.embed` params: queries to embed with the SigLIP2 text tower.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TextEmbedRequest {
    pub texts: Vec<String>,
    /// Fetch the text model first when it is missing (the core never asks for this).
    #[serde(default)]
    pub allow_download: bool,
}

/// `text.embed` result: `vectors[i]` (L2-normalised, same space as the stored image embeddings)
/// belongs to `texts[i]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TextEmbedResponse {
    /// Model that produced the vectors (e.g. `siglip2-base-text`).
    pub model: String,
    pub dim: usize,
    /// `exp(logit_scale)` of the SigLIP2 checkpoint, for `sigmoid(scale * cos + bias)`.
    pub logit_scale: f64,
    pub vectors: Vec<Vec<f32>>,
    /// Per text: answered from the worker's query cache.
    pub cached: Vec<bool>,
}
