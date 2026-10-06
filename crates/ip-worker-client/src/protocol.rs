//! Typed view of the worker's JSON-RPC payloads (`docs/05` §4, `docs/api-contract-m2.md` §A).
//!
//! Everything the worker returns is parsed leniently: unknown fields are ignored and missing
//! optional fields default, so a newer or older worker never breaks the core.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One photo of an `analyze.batch` request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnalyzeRequestItem {
    pub photo_id: i64,
    pub path: String,
    pub orientation: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnalyzeRequest {
    pub items: Vec<AnalyzeRequestItem>,
    /// `fast` | `standard`; the worker expands it into its step list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Explicit steps (win over `profile`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<String>>,
    pub analysis_size: u32,
    pub out_dir: String,
    pub allow_download: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ItemError {
    pub code: i64,
    pub message: String,
    pub kind: Option<String>,
}

/// One detected face; bbox is normalised `[x, y, w, h]` in display orientation.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AnalyzeFace {
    pub bbox: [f64; 4],
    pub det_score: Option<f64>,
    pub sharpness: Option<f64>,
    pub eyes_open: Option<f64>,
    pub smile: Option<f64>,
    pub gaze: Option<f64>,
    pub yaw: Option<f64>,
    pub pitch: Option<f64>,
    pub roll: Option<f64>,
    /// Row of the photo's `identity_file` belonging to this face.
    pub identity_index: Option<usize>,
    pub blendshapes: Option<Value>,
}

/// Sub-metrics of the `quality` step.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct QualityInfo {
    pub sharpness_center: Option<f64>,
    pub sharpness_face: Option<f64>,
    pub mean_luminance: Option<f64>,
    pub clipped_highlights: Option<f64>,
    pub crushed_shadows: Option<f64>,
    pub noise_sigma: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AnalyzeItem {
    pub photo_id: i64,
    pub error: Option<ItemError>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    /// 16 hex chars (64-bit pHash).
    pub phash: Option<String>,
    pub sharpness: Option<f64>,
    pub exposure: Option<f64>,
    pub noise: Option<f64>,
    pub quality: Option<QualityInfo>,
    pub iqa: Option<f64>,
    pub iqa_model: Option<String>,
    pub aesthetic: Option<f64>,
    pub aesthetic_model: Option<String>,
    pub scene_type: Option<String>,
    pub scene_scores: Option<Value>,
    pub faces: Vec<AnalyzeFace>,
    pub embedding_file: Option<String>,
    pub embedding_model: Option<String>,
    pub identity_file: Option<String>,
    pub identity_error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AnalyzeResponse {
    pub items: Vec<AnalyzeItem>,
    pub steps: Vec<String>,
    pub skipped_steps: Vec<String>,
    pub models: Value,
    pub warnings: Vec<String>,
    pub timings: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct GpuInfo {
    pub name: String,
    pub vram_mb: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct HardwareJson {
    pub device: String,
    pub gpus: Vec<GpuInfo>,
}

/// `system.info` (subset).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SystemInfo {
    pub tier: Option<String>,
    pub providers: Vec<String>,
    pub hardware: HardwareJson,
}

/// An entry of `models.list`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct WorkerModel {
    pub id: String,
    pub task: Vec<String>,
    pub size_mb: f64,
    pub license: String,
    pub noncommercial: bool,
    pub installed: bool,
    pub recommended: bool,
    pub optional: bool,
    pub tiers: Vec<String>,
    pub required_for: Vec<String>,
}

/// `profiles[name]` of `models.list`: the steps and exactly the model ids needed on this machine.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProfileInfo {
    pub steps: Vec<String>,
    pub models: Vec<String>,
}

/// `models.list`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ModelsListing {
    pub models: Vec<WorkerModel>,
    pub profiles: std::collections::BTreeMap<String, ProfileInfo>,
}

/// Photo of a `mask.generate` request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MaskPhoto {
    pub photo_id: i64,
    pub path: String,
    pub orientation: u8,
}

/// `mask.generate` params (`docs/api-contract-m3.md` section D).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MaskRequest {
    pub photo: MaskPhoto,
    pub targets: Vec<String>,
    /// Normalised `[x, y, w, h]` face box for `target = person`.
    pub person_bbox: Option<[f64; 4]>,
    /// Long edge of the produced masks.
    pub size: u32,
    pub out_dir: String,
    pub allow_download: bool,
}

/// `mask.generate` result: target -> PNG path, plus what was skipped and why.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MaskResponse {
    pub masks: std::collections::BTreeMap<String, String>,
    pub models: std::collections::BTreeMap<String, String>,
    pub skipped: std::collections::BTreeMap<String, String>,
}
