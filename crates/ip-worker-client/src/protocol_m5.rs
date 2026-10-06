//! M5 payloads: `besttake.compose`, `inpaint.run` and `enhance.run` (`docs/api-contract-m5.md`
//! section B). Parsed leniently like the other payloads.

use serde::{Deserialize, Serialize};

use crate::protocol::MaskPhoto;

/// `besttake.compose` params: paste the face `source_face` of `source` over `base_face` of
/// `base` (boxes normalised `[x, y, w, h]`, upright images).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BestTakeComposeRequest {
    pub base: MaskPhoto,
    pub source: MaskPhoto,
    pub base_face: [f64; 4],
    pub source_face: [f64; 4],
    pub out_dir: String,
    pub allow_download: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ComposeQuality {
    pub score: f64,
    pub aligned: bool,
    /// `large_pose_change` | `camera_moved` | `occlusion` | `seam`
    pub warnings: Vec<String>,
}

/// `besttake.compose` result. `patch == None` means "cannot be composed" (`reason` says why).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BestTakeComposeResponse {
    /// RGBA PNG, alpha = blend mask.
    pub patch: Option<String>,
    pub rect: Option<[f32; 4]>,
    pub quality: Option<ComposeQuality>,
    pub reason: Option<String>,
}

/// `inpaint.run` params; `mask` is an 8-bit PNG where 255 = remove.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InpaintRequest {
    pub photo: MaskPhoto,
    pub mask: String,
    /// `lama` | `sdxl`
    pub model: String,
    pub out_dir: String,
    pub allow_download: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct InpaintResponse {
    pub patch: Option<String>,
    pub rect: Option<[f32; 4]>,
    pub model: Option<String>,
}

/// `enhance.run` params. `op` is `denoise` | `face_restore` | `upscale`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EnhanceRequest {
    pub photo: MaskPhoto,
    pub op: String,
    pub strength: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub faces: Option<Vec<[f64; 4]>>,
    pub out_dir: String,
    pub allow_download: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct EnhancePatch {
    pub patch: String,
    pub rect: [f32; 4],
}

/// `enhance.run` result: `patch`+`rect` (denoise), `patches` (face_restore) or `image` (upscale).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct EnhanceResponse {
    pub patch: Option<String>,
    pub rect: Option<[f32; 4]>,
    pub patches: Vec<EnhancePatch>,
    pub image: Option<String>,
}
