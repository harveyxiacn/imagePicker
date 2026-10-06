//! M4 payloads: `beauty.prepare` and `faces.embed` (`docs/api-contract-m4.md` section B).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Photo of a `beauty.prepare` request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BeautyPhoto {
    pub photo_id: i64,
    pub path: String,
    pub orientation: u8,
}

/// A face already found by the analysis (normalised `[x, y, w, h]`, upright image).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BeautyFaceRef {
    pub face_id: i64,
    pub bbox: [f64; 4],
}

/// `beauty.prepare` params.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BeautyPrepareRequest {
    pub photo: BeautyPhoto,
    /// Empty = the worker detects faces itself.
    pub faces: Vec<BeautyFaceRef>,
    /// Long edge of the produced masks.
    pub size: u32,
    pub out_dir: String,
    pub allow_download: bool,
}

/// One person of a `beauty.prepare` result.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BeautyPerson {
    /// `None` for faces the worker found on its own.
    pub face_id: Option<i64>,
    pub face_box: [f32; 4],
    pub face_landmarks: Vec<[f32; 2]>,
    pub pose: Vec<[f32; 3]>,
    pub skin_mask: Option<String>,
    pub body_mask: Option<String>,
    pub blemishes: Vec<[f32; 3]>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BeautyPrepareResponse {
    pub people: Vec<BeautyPerson>,
    pub models: Value,
    /// What could not be produced and why (e.g. `{"pose": "model_unavailable"}`).
    pub skipped: std::collections::BTreeMap<String, String>,
}

/// `faces.embed` params: detect the faces of one image and embed each of them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FacesEmbedRequest {
    pub path: String,
    pub orientation: u8,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct EmbeddedFace {
    pub bbox: [f64; 4],
    pub det_score: f64,
}

/// `faces.embed` result: `embeddings[i]` (512 floats, L2-normalised) belongs to `faces[i]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct FacesEmbedResponse {
    pub faces: Vec<EmbeddedFace>,
    pub embeddings: Vec<Vec<f32>>,
}
