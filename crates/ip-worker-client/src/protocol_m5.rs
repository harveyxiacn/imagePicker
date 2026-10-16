//! M5 payloads: `besttake.compose`, `inpaint.run` and `enhance.run` (`docs/api-contract-m5.md`
//! section B), plus `hair.rank` / `hair.compose` (section H). Parsed leniently like the other payloads.

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

/// One face to measure in `hair.rank`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HairRankFace {
    pub photo: MaskPhoto,
    /// Normalised `[x, y, w, h]` of the face (upright image).
    pub face: [f64; 4],
}

/// `hair.rank` params: measure the hair-quality cues of each face (at most 24).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HairRankRequest {
    pub faces: Vec<HairRankFace>,
    pub allow_download: bool,
}

/// Hair-quality cues of one face (docs/03 section 5.1), measured at a normalised face width.
/// `ok: false` (with `reason`) when the face or its hair could not be measured.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct HairMetrics {
    pub photo_id: i64,
    pub ok: bool,
    pub reason: Option<String>,
    pub yaw: Option<f64>,
    pub pitch: Option<f64>,
    pub roll: Option<f64>,
    /// Face width in the photo (px).
    pub face_px: Option<f64>,
    /// Hair area / face-oval area.
    pub hair_ratio: Option<f64>,
    /// Mean |Laplacian| inside the hair (0-255 luma scale).
    pub sharpness: Option<f64>,
    /// Share of the hair that is specular highlight (0-1).
    pub shine: Option<f64>,
    /// Share of the face skin that is shine (0-1).
    pub skin_shine: Option<f64>,
    /// Share of the hair mask made of thin stray strands (0-1).
    pub roughness: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct HairRankResponse {
    pub results: Vec<HairMetrics>,
}

/// `hair.compose` params: paste the hair of `donor_face` in `donor` onto `target_face` of
/// `target` (boxes normalised, upright images).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HairComposeRequest {
    pub target: MaskPhoto,
    pub donor: MaskPhoto,
    pub target_face: [f64; 4],
    pub donor_face: [f64; 4],
    pub out_dir: String,
    pub allow_download: bool,
}

/// The donor -> target similarity transform `hair.compose` used (shift as a fraction of the
/// target's width / height).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct HairAlign {
    pub scale: f64,
    pub rotation_deg: f64,
    pub dx: f64,
    pub dy: f64,
    /// Landmark residual after the fit, as a fraction of the face width.
    pub landmark_rms: f64,
}

/// `hair.compose` result; `patch == None` means "cannot be composed" (`reason`: e.g.
/// `pose_too_different`, `donor_too_small`, `donor_no_hair`, `face_not_found`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct HairComposeResponse {
    pub patch: Option<String>,
    pub rect: Option<[f32; 4]>,
    /// warnings: `pose_differs` | `large_fill` | `donor_small` | `seam` | `lighting_unmatched`
    pub quality: Option<ComposeQuality>,
    pub align: Option<HairAlign>,
    pub reason: Option<String>,
}
