//! Non-destructive render engine: edit stack (JSON, docs/05 §2) -> pixels.
//!
//! CONTRACT: the public types and signatures in this file are consumed by `ip-core`
//! (and mirrored in `docs/api-contract-m3.md` for the frontend). Implementations live
//! in private modules and may change freely; public items change only in coordination.
//!
//! Pipeline order is fixed (docs/02 §5.3): decode -> geometry (crop/rotate) ->
//! [warp/patch: M4/M5] -> global -> local (masks) -> [beauty: M4] -> effects -> LUT ->
//! detail (whole-image) -> output sharpen.
//!
//! Everything up to and including the LUT is per-pixel and runs on the GPU; `detail`
//! (M9) and `output_sharpen` are whole-image stages that both backends run in `detail.rs`
//! on the encoded result, which is what keeps the two backends bit-identical there.

#![allow(clippy::chunks_exact_to_as_chunks)]

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub type Result<T> = anyhow::Result<T>;

// ------------------------------------------------------------------ edit stack

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EditStack {
    #[serde(default = "one")]
    pub version: u32,
    #[serde(default)]
    pub ops: Vec<Op>,
}

fn one() -> u32 {
    1
}

impl EditStack {
    pub fn is_identity(&self) -> bool {
        self.ops.iter().all(|o| {
            matches!(o, Op::Unknown | Op::Warp(Warp::Unknown))
                || matches!(o, Op::Patch(p) if !p.enabled)
                || matches!(o, Op::Effects(e) if e.is_neutral())
                || matches!(o, Op::Detail(d) if d.is_neutral())
        })
    }
}

/// One operation. Unknown `type`s (e.g. M4 `warp`/`beauty`, M5 `patch`) deserialize to
/// `Unknown` and are ignored by the M3 renderer, so newer stacks still load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Op {
    Crop(Crop),
    Global(Adjust),
    Local(LocalAdjust),
    Lut(Lut),
    OutputSharpen(OutputSharpen),
    /// Post-crop vignette and film grain. Applied after the local adjustments, in
    /// post-crop coordinates, before the LUT.
    Effects(Effects),
    /// Sharpening and noise reduction. A whole-image stage — see [`Detail`].
    Detail(Detail),
    /// Portrait retouching on skin (M4). Applied after local adjustments.
    Beauty(Beauty),
    /// Parametric geometric reshaping of a face or body (M4). Applied right after crop.
    Warp(Warp),
    /// A generated raster layer (M5: best take, inpainting, enhancement) composited onto
    /// the upright source before crop.
    Patch(Patch),
    #[serde(other)]
    Unknown,
}

/// Geometry on the upright (EXIF-oriented) image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Crop {
    /// Normalised `[x, y, w, h]` of the kept region, measured after `angle` rotation.
    pub rect: [f32; 4],
    /// Straighten angle in degrees, positive = counter-clockwise, |angle| <= 45.
    #[serde(default)]
    pub angle: f32,
    /// UI hint only (e.g. "4:5"); the renderer uses `rect`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aspect: Option<String>,
}

/// Slider-style adjustments. Every field defaults to 0 / neutral.
/// Ranges follow the conventions photographers already expect, so intuition transfers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Adjust {
    /// Exposure in EV stops, -5..5.
    pub exposure: f32,
    /// -100..100 for all of the following.
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    /// White-balance shift in Kelvin relative to as-shot, -3000..3000 (positive = warmer).
    pub temp: f32,
    /// Green(-)..magenta(+), -100..100.
    pub tint: f32,
    pub vibrance: f32,
    pub saturation: f32,
    /// Local contrast (midtone), -100..100.
    pub clarity: f32,
    /// -100..100 (negative adds haze).
    pub dehaze: f32,
    /// Mid-scale texture, -100..100. Positive emphasises the
    /// detail band that sits between "flat" and "edge" (fabric, hair, foliage); negative
    /// evens it out, which is the skin-friendly direction. Distinct from `clarity`, which
    /// works on the midtone-weighted *ratio* of a much coarser band.
    pub texture: f32,
    /// Dodge (brighten) the masked pixels, 0..100. Luminance
    /// moves with the hue kept, so dodging skin does not wash the colour out. Usable on
    /// the global op and — where it matters — inside a local adjustment's mask.
    pub dodge: f32,
    /// Burn (darken) the masked pixels, 0..100. See `dodge`.
    pub burn: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub curve: Option<Curves>,
    /// Per-band hue/saturation/luminance shifts, each -100..100.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub hsl: BTreeMap<HslBand, Hsl>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grading: Option<Grading>,
    /// Provenance, e.g. "ai_auto@1" or "user". Not used for rendering.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Tone curves as control points in 0..1 (x ascending; monotone cubic interpolation).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Curves {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rgb: Vec<[f32; 2]>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub r: Vec<[f32; 2]>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub g: Vec<[f32; 2]>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub b: Vec<[f32; 2]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HslBand {
    Red,
    Orange,
    Yellow,
    Green,
    Aqua,
    Blue,
    Purple,
    Magenta,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hsl {
    pub h: f32,
    pub s: f32,
    pub l: f32,
}

/// Three-way colour grading. Each wheel is `[hue_degrees 0..360, amount 0..1]`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Grading {
    pub shadows: [f32; 2],
    pub midtones: [f32; 2],
    pub highlights: [f32; 2],
    /// -100 (favour shadows) .. 100 (favour highlights).
    pub balance: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalAdjust {
    pub mask: MaskRef,
    /// Mask opacity 0..1.
    #[serde(default = "one_f")]
    pub amount: f32,
    #[serde(default)]
    pub invert: bool,
    pub adjust: Adjust,
    /// Extra gate multiplied into the mask, derived from each pixel's own colour.
    /// Composes with every mask kind, including AI masks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<MaskRange>,
}

fn one_f() -> f32 {
    1.0
}

/// A per-pixel weight computed from the pixel under the mask, used to narrow a local
/// adjustment to part of the masked area.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MaskRange {
    /// Keep pixels whose sRGB-encoded luminance lies inside `min..max` (each 0..100),
    /// with `softness` (0..100) feathering both ends.
    Luminance {
        min: f32,
        max: f32,
        #[serde(default = "half_100")]
        softness: f32,
    },
    /// Keep pixels whose hue is within `width` degrees of `hue` (0..360) and whose chroma
    /// lies inside `sat_min..sat_max` (each 0..100). The weight fades over the outer
    /// `softness` (0..100) of both ranges, so the selection is never a hard cut.
    Color {
        hue: f32,
        width: f32,
        #[serde(default)]
        sat_min: f32,
        #[serde(default = "hundred")]
        sat_max: f32,
        #[serde(default = "half_100")]
        softness: f32,
    },
    /// Unrecognised `kind` from a newer version: kept loadable, not applied.
    #[serde(other)]
    Unknown,
}

fn half_100() -> f32 {
    50.0
}

fn hundred() -> f32 {
    100.0
}

/// Where a local adjustment's mask comes from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MaskRef {
    /// AI mask generated by the worker; resolved through [`MaskProvider`].
    Ai {
        target: MaskTarget,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        person_id: Option<i64>,
    },
    /// Elliptical gradient in normalised (post-crop) coordinates.
    Radial {
        center: [f32; 2],
        radius: [f32; 2],
        #[serde(default = "half")]
        feather: f32,
    },
    /// Linear gradient from `start` (full effect) to `end` (no effect), normalised coords.
    Linear { start: [f32; 2], end: [f32; 2] },
}

fn half() -> f32 {
    0.5
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskTarget {
    Subject,
    Background,
    Sky,
    Person,
    Skin,
    Hair,
    Clothes,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lut {
    /// Preset LUT id (built-in) or absolute path to a `.cube` file; resolved via [`LutProvider`].
    pub file: String,
    #[serde(default = "one_f")]
    pub amount: f32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputSharpen {
    /// 0..100.
    pub amount: f32,
}

/// A post-crop vignette plus film grain. Both are pure
/// functions of the output pixel position and colour, so they need no neighbourhood and
/// cost the same as any other per-pixel adjustment.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Effects {
    pub vignette: Vignette,
    pub grain: Grain,
}

impl Effects {
    /// True when the op would not change a single pixel.
    pub fn is_neutral(&self) -> bool {
        self.vignette.amount == 0.0 && self.grain.amount == 0.0
    }
}

/// Post-crop vignette. The falloff is an analytic super-ellipse centred on the cropped
/// frame, so it follows the crop instead of the original sensor frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Vignette {
    /// -100 (darken the corners) .. 100 (brighten them). 0 = off.
    pub amount: f32,
    /// Where the falloff starts, 0..100; 50 is the default.
    pub midpoint: f32,
    /// -100 (the clear area is a diamond, so the dark frame squares off toward the
    /// corners) .. 100 (the clear area is an oval, reaching further into the diagonals).
    pub roundness: f32,
    /// 0 (hard edge) .. 100 (very gradual); 50 is the default.
    pub feather: f32,
    /// 0..100: hold the effect back where the pixels are already bright, so a darkened
    /// corner does not crush into the highlights.
    pub highlight_contrast: f32,
    /// How the vignette is mixed in.
    pub style: VignetteStyle,
}

/// The three styles differ in what happens to the pixels underneath, not in the shape of
/// the falloff.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VignetteStyle {
    /// Multiply linear light, easing off where the pixels are bright. Keeps colour.
    #[default]
    HighlightPriority,
    /// Multiply, then restore most of the original chroma — darker corners stay coloured
    /// instead of drifting grey.
    ColorPriority,
    /// Blend toward black or white in display space, as if painted over the edge.
    PaintOverlay,
}

impl Default for Vignette {
    fn default() -> Self {
        // The neutral defaults, so an op created from the UI starts from the same look.
        Vignette {
            amount: 0.0,
            midpoint: 50.0,
            roundness: 0.0,
            feather: 50.0,
            highlight_contrast: 0.0,
            style: VignetteStyle::HighlightPriority,
        }
    }
}

/// Film grain. Deterministic: the same stack always renders the same grain, and the cell
/// size is anchored to the *source* image, so a preview and a full-resolution export show
/// grain at the same physical scale.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Grain {
    /// 0..100.
    pub amount: f32,
    /// 0..100 — larger cells; 25 is the default.
    pub size: f32,
    /// 0..100 — cross-fades a second, finer octave in for a less regular, more clumped
    /// look. 50 is the default.
    pub frequency: f32,
}

impl Default for Grain {
    fn default() -> Self {
        Grain {
            amount: 0.0,
            size: 25.0,
            frequency: 50.0,
        }
    }
}

/// Capture sharpening plus luminance and colour noise reduction.
///
/// Unlike every other op this one is not per-pixel: sharpening and noise reduction are
/// defined by a blur radius in *output* pixels, so they need the whole image. The
/// renderers therefore run it as a final stage over the encoded 8-bit sRGB result, after
/// the per-pixel pipeline and before `output_sharpen` (which moves into this stage when a
/// `detail` op is present, so that noise reduction always precedes sharpening). Both
/// backends call the same code, so CPU and GPU stay bit-identical.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Detail {
    pub sharpen: SharpenDetail,
    pub noise: NoiseReduction,
}

impl Detail {
    pub fn is_neutral(&self) -> bool {
        self.sharpen.amount == 0.0 && self.noise.is_neutral()
    }
}

/// Unsharp masking with a radius, halo limiting and an edge mask: the radius sets the
/// scale, *Detail* picks how much coarse structure joins in, and *Masking* confines the
/// effect to edges.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SharpenDetail {
    /// 0..150; 0 = off.
    pub amount: f32,
    /// 0.5..3.0 output pixels; 1.0 is the default.
    pub radius: f32,
    /// 0..100 — how much coarse structure is sharpened instead of the finest detail, and
    /// how hard the halo limiter bites. 25 is the default.
    pub detail: f32,
    /// 0..100 — restrict sharpening to edges. 0 sharpens everything (including noise),
    /// 100 keeps only strong edges.
    pub masking: f32,
}

impl Default for SharpenDetail {
    fn default() -> Self {
        SharpenDetail {
            amount: 0.0,
            radius: 1.0,
            detail: 25.0,
            masking: 0.0,
        }
    }
}

/// Luminance and colour noise reduction. The strength of the luminance pass adapts to the
/// noise actually present in the frame (estimated once per render from the high-frequency
/// band), so a clean photo is not flattened to remove noise it does not have.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NoiseReduction {
    /// 0..100 — how hard the luminance pass smooths.
    pub luminance: f32,
    /// 0..100 — how much of the removed detail band is added back; 50 is the default.
    pub luminance_detail: f32,
    /// 0..100 — re-adds low-frequency contrast that the smoothing removes.
    pub luminance_contrast: f32,
    /// 0..100 — how hard the chroma pass smooths.
    pub color: f32,
    /// 0..100 — how much colour detail survives the chroma smoothing; 50 is the default.
    pub color_detail: f32,
}

impl Default for NoiseReduction {
    fn default() -> Self {
        NoiseReduction {
            luminance: 0.0,
            luminance_detail: 50.0,
            luminance_contrast: 0.0,
            color: 0.0,
            color_detail: 50.0,
        }
    }
}

impl NoiseReduction {
    pub fn is_neutral(&self) -> bool {
        self.luminance == 0.0 && self.color == 0.0
    }
}

/// Who a portrait op applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Strength caps for a natural look (docs/03 §7.2: face warp <= 3% of face width).
    Natural,
    Standard,
    /// Highest caps (face warp <= 6% of face width).
    Refined,
}

/// Skin retouching for one person (`person_id`) or, with `None`, every detected face.
/// All amounts 0..100 (0 = off). The level scales and caps every amount.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Beauty {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person_id: Option<i64>,
    #[serde(default = "standard_level")]
    pub level: Level,
    /// Texture-preserving smoothing (frequency separation on the skin mask).
    #[serde(default)]
    pub smooth: f32,
    /// Brighten and even skin tone (LAB L up, a/b toward a natural skin target).
    #[serde(default)]
    pub whiten: f32,
    /// Remove small blemishes found by the worker (`PersonGeometry::blemishes`).
    #[serde(default)]
    pub blemish: bool,
    /// Brighten the eye whites/iris region from face landmarks.
    #[serde(default)]
    pub eye_brighten: f32,
    /// Whiten teeth inside the inner-lip polygon from face landmarks.
    #[serde(default)]
    pub teeth_whiten: f32,
    /// Reduce dark circles under the eyes.
    #[serde(default)]
    pub dark_circles: f32,
}

fn standard_level() -> Level {
    Level::Standard
}

/// Parametric liquify. The renderer derives a smooth displacement field from the
/// person's geometry (face landmarks / body keypoints) and these amounts, so sliders
/// stay interactive without a worker round-trip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Warp {
    /// Amounts -100..100 (positive = slimmer / smaller / larger eyes).
    Face {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        person_id: Option<i64>,
        #[serde(default = "standard_level")]
        level: Level,
        #[serde(default)]
        slim: f32,
        #[serde(default)]
        chin: f32,
        #[serde(default)]
        eyes: f32,
        #[serde(default)]
        nose: f32,
    },
    /// Amounts 0..100 (positive = slimmer / longer).
    Body {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        person_id: Option<i64>,
        #[serde(default = "standard_level")]
        level: Level,
        #[serde(default)]
        arms: f32,
        #[serde(default)]
        legs: f32,
        #[serde(default)]
        waist: f32,
        #[serde(default)]
        lengthen_legs: f32,
        /// Reduce the warp where it would bend straight background lines (docs/03 §7.3).
        #[serde(default = "yes")]
        protect_background: bool,
    },
    /// Unrecognised `kind` (e.g. from a newer version): kept loadable, not rendered.
    #[serde(other)]
    Unknown,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatchKind {
    BestTake,
    /// Hair from another photo of the same person (docs/03 section 5.1).
    Hair,
    Inpaint,
    Denoise,
    FaceRestore,
    /// Sweat cleanup: shine compressed on skin and hair (docs/03 section 7.5).
    Deshine,
    /// Sweat cleanup: stray strands outside the hair silhouette filled (docs/03 section 7.5).
    Flyaway,
    #[serde(other)]
    Other,
}

impl PatchKind {
    /// Compositing order of the patch kinds (lower first; equal kinds keep their stack order).
    /// Every patch is computed from the original pixels, so a later layer replaces an earlier
    /// one where both cover the same pixels: whole-frame / per-face cleanups first, then object
    /// removal, then face replacement, and the hair transfer last (its alpha only covers pixels it
    /// actually changes, so the sweat cleanup below it survives everywhere else).
    pub fn layer_order(self) -> u8 {
        match self {
            Self::Denoise => 0,
            Self::FaceRestore => 1,
            Self::Deshine => 2,
            Self::Flyaway => 3,
            Self::Inpaint => 4,
            Self::Other => 5,
            Self::BestTake => 6,
            Self::Hair => 7,
        }
    }
}

/// A generated layer. The asset is an RGBA image whose alpha is the blend mask, placed
/// over `rect` (normalised upright pre-crop coordinates) and resampled to fit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Patch {
    pub kind: PatchKind,
    /// Opaque asset id resolved through [`MaskProvider::patch`].
    pub asset: String,
    /// `[x, y, w, h]` where the asset is placed.
    pub rect: [f32; 4],
    /// Extra edge feather as a fraction of the rect's short side, 0..0.5.
    #[serde(default)]
    pub feather: f32,
    /// Blend opacity 0..1 (e.g. denoise strength).
    #[serde(default = "one_f")]
    pub amount: f32,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Best take / hair: whose face (hair) was replaced, and from which photo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_photo_id: Option<i64>,
}

// ------------------------------------------------------------------ images & providers

/// Upright 8-bit sRGB image, tightly packed RGB.
#[derive(Debug, Clone)]
pub struct RgbImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// 8-bit single-channel mask, any resolution (the renderer resamples to the image,
/// edge-aware where it matters). 255 = full effect.
#[derive(Debug, Clone)]
pub struct Mask {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Geometry of one person for portrait ops, in normalised upright-image coordinates
/// (before crop). Produced by the worker's `beauty.prepare` and cached by the core.
#[derive(Debug, Clone, Default)]
pub struct PersonGeometry {
    /// `None` for faces not assigned to a person.
    pub person_id: Option<i64>,
    /// `[x, y, w, h]` face box.
    pub face_box: [f32; 4],
    /// MediaPipe Face Mesh 478 landmarks `[x, y]` (empty if unavailable).
    pub face_landmarks: Vec<[f32; 2]>,
    /// MediaPipe Pose 33 keypoints `[x, y, visibility]` (empty if unavailable).
    pub pose: Vec<[f32; 3]>,
    /// Skin of this person only, excluding eyes/brows/lips (255 = skin).
    pub skin: Option<Mask>,
    /// Whole-body matte of this person (255 = person).
    pub body: Option<Mask>,
    /// Blemishes `[x, y, radius]` (radius normalised to the image long edge).
    pub blemishes: Vec<[f32; 3]>,
}

/// Supplies AI masks for `MaskRef::Ai`, and person geometry for `beauty`/`warp` ops.
/// Returning `Ok(None)` / an empty list skips the op.
pub trait MaskProvider: Send + Sync {
    fn mask(&self, target: MaskTarget, person_id: Option<i64>) -> Result<Option<Mask>>;

    /// Every person/face in the photo. Default: none (portrait ops are skipped).
    fn people(&self) -> Result<Vec<PersonGeometry>> {
        Ok(Vec::new())
    }

    /// RGBA image for a `patch` op's asset (alpha = blend mask). Default: none (skipped).
    fn patch(&self, _asset: &str) -> Result<Option<RgbaImage>> {
        Ok(None)
    }
}

/// 8-bit RGBA image, tightly packed (straight alpha).
#[derive(Debug, Clone)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// A 3D LUT: `size`^3 entries of RGB in 0..1, red index fastest (`.cube` order).
#[derive(Debug, Clone)]
pub struct Lut3d {
    pub size: u32,
    pub data: Vec<[f32; 3]>,
}

pub trait LutProvider: Send + Sync {
    fn lut(&self, file: &str) -> Result<Option<Lut3d>>;
}

/// Provider that has nothing (masks/LUT ops are skipped).
pub struct NoAssets;

impl MaskProvider for NoAssets {
    fn mask(&self, _: MaskTarget, _: Option<i64>) -> Result<Option<Mask>> {
        Ok(None)
    }
}

impl LutProvider for NoAssets {
    fn lut(&self, _: &str) -> Result<Option<Lut3d>> {
        Ok(None)
    }
}

// ------------------------------------------------------------------ rendering

pub struct RenderRequest<'a> {
    /// Source pixels (already decoded at the resolution to render from).
    pub source: &'a RgbImage,
    pub stack: &'a EditStack,
    pub masks: &'a dyn MaskProvider,
    pub luts: &'a dyn LutProvider,
    /// Resize the result so its long edge is <= this (after crop). `None` = native.
    pub max_long_edge: Option<u32>,
}

impl RenderRequest<'_> {
    /// The same request rendering from `source` instead (e.g. a patched copy); `None`
    /// keeps the original source.
    pub(crate) fn with_source<'b>(&'b self, source: Option<&'b RgbImage>) -> RenderRequest<'b> {
        RenderRequest {
            source: source.unwrap_or(self.source),
            stack: self.stack,
            masks: self.masks,
            luts: self.luts,
            max_long_edge: self.max_long_edge,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Gpu,
    Cpu,
}

/// A renderer instance (owns GPU device/pipelines when GPU-backed). Thread-safe.
pub trait Renderer: Send + Sync {
    fn backend(&self) -> Backend;
    /// Render to upright 8-bit sRGB.
    fn render(&self, req: &RenderRequest<'_>) -> Result<RgbImage>;
}

/// Create the best available renderer: GPU (wgpu: DX12/Vulkan/Metal) unless
/// `prefer_gpu` is false or no adapter is available, then the CPU (rayon) fallback.
/// GPU and CPU results must match within ΔE < 1 (golden tests).
pub fn create_renderer(prefer_gpu: bool) -> Box<dyn Renderer> {
    imp::create_renderer(prefer_gpu)
}

/// Parse a `.cube` 3D LUT file.
pub fn parse_cube(text: &str) -> Result<Lut3d> {
    imp::parse_cube(text)
}

// ------------------------------------------------------------------ auto adjust

/// Context for the rule-based "AI one-click" (docs/03 §6.1): scene type and subject
/// regions from analysis, when available.
#[derive(Debug, Clone, Default)]
pub struct AutoContext {
    /// "portrait" | "group" | "landscape" | "food" | "architecture" | "night" | "pet" | "other".
    pub scene_type: Option<String>,
    /// Normalised `[x, y, w, h]` face boxes (upright image).
    pub faces: Vec<[f32; 4]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutoMode {
    Auto,
    Portrait,
    Landscape,
}

/// Suggest global slider values (never modifies pixels). Must be fast (< 20 ms on a
/// 1024 px proxy) and deterministic; `source` should be a downscaled proxy.
pub fn auto_adjust(source: &RgbImage, ctx: &AutoContext, mode: AutoMode) -> Adjust {
    imp::auto_adjust(source, ctx, mode)
}

/// Built-in presets (id, display name key, nominal stack). Stacks contain only global / LUT /
/// output-sharpen ops. The nominal stack is the look at 100 % with exposure 0; apply a preset
/// to a photo through [`resolve_preset`], which fits its tone to that photo.
pub fn builtin_presets() -> Vec<(String, String, EditStack)> {
    imp::builtin_presets()
}

// ------------------------------------------------------------------ adaptive presets

/// Largest preset strength (150 %).
pub const PRESET_AMOUNT_MAX: f32 = 1.5;

/// What the adaptive preset solver knows about the photo besides its pixels.
#[derive(Debug, Clone, Default)]
pub struct PresetContext {
    /// Normalised `[x, y, w, h]` face boxes (upright image); their level is weighted 60 %
    /// and kept out of clipping.
    pub faces: Vec<[f32; 4]>,
}

/// Tone statistics of an image: sRGB-encoded luminance, 0..1.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct ToneStats {
    pub median: f32,
    pub mean: f32,
    pub p01: f32,
    pub p99: f32,
    /// Share of pixels at or above 250/255 luminance.
    pub clipped: f32,
    /// Median / clipped share of the central half of the face boxes, when there are faces.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub face_median: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub face_clipped: Option<f32>,
}

/// A preset fitted to one photo.
#[derive(Debug, Clone, Serialize)]
pub struct PresetResolution {
    /// Concrete stack (global op `source: "preset:<id>;amount=<percent>"`).
    pub stack: EditStack,
    /// Strength used, 0..=[`PRESET_AMOUNT_MAX`].
    pub amount: f32,
    /// Whether the tone was fitted to the photo (built-in presets) or only scaled (user presets).
    pub adaptive: bool,
    /// Solved exposure (EV) in the stack.
    pub exposure: f32,
    /// How far highlight protection pulled the look back, 0 (not needed) ..1 (fully).
    pub protection: f32,
    /// The highlight / face budget holds for the result.
    pub satisfied: bool,
    /// Measured on the photo before and after (on the <= 256 px measuring proxy).
    pub before: ToneStats,
    pub after: ToneStats,
}

/// Fit built-in preset `id` to a photo at `amount` (0..=1.5, 1 = 100 %): exposure is solved so
/// the subject level moves toward the preset's target, then highlight protection keeps the
/// share of newly clipped pixels (and face clipping) within the preset's budget. Deterministic.
/// `source` is an upright proxy of the photo (e.g. 1024 px); `None` for an unknown id.
pub fn resolve_preset(
    id: &str,
    source: &RgbImage,
    ctx: &PresetContext,
    amount: f32,
) -> Option<PresetResolution> {
    adapt::resolve_preset(id, source, ctx, amount)
}

/// Scale a preset stack's deltas by `amount` (the strength slider; used as-is for user presets).
pub fn scale_preset_stack(stack: &EditStack, amount: f32) -> EditStack {
    adapt::scale_preset_stack(stack, amount)
}

/// Luminance statistics of an image (`faces` as in [`PresetContext`]).
pub fn measure_tone(img: &RgbImage, faces: &[[f32; 4]]) -> ToneStats {
    adapt::measure_tone(img, faces)
}

/// `preset:<id>;amount=<percent>`, the `source` of a resolved preset's global op.
pub fn preset_source(id: &str, amount: f32) -> String {
    adapt::preset_source(id, amount)
}

/// Parse a `source` written by [`preset_source`]: `(preset id, amount)`.
pub fn parse_preset_source(source: &str) -> Option<(String, f32)> {
    adapt::parse_preset_source(source)
}

pub mod crop_assist;
mod imp;

/// Procedurally generated built-in LUT for a preset `Lut.file` id (`film_warm`, `film_cool`,
/// `cinematic`, `bw_classic`). The renderer falls back to this when the [`LutProvider`]
/// has nothing for an id, so providers only need to resolve imported `.cube` files.
pub fn builtin_lut(id: &str) -> Option<Lut3d> {
    imp::builtin_lut(id)
}

/// Serialise a LUT to `.cube` text.
pub fn write_cube(lut: &Lut3d, title: &str) -> String {
    cube::write_cube(lut, title)
}

/// Identity 3D LUT of the given size (2..=256).
pub fn identity_lut(size: u32) -> Lut3d {
    cube::identity_lut(size)
}

/// Ids of all built-in LUTs resolvable through [`builtin_lut`]: `film_warm`, `film_cool`,
/// `bw_classic`, `teal_orange`, `fade`, `vivid`, `cinematic`.
pub fn builtin_lut_ids() -> &'static [&'static str] {
    presets::builtin_lut_ids()
}

/// Name of the GPU adapter `create_renderer(true)` would use, if any.
pub fn gpu_adapter_name() -> Option<String> {
    imp::gpu_adapter_name()
}

mod adapt;
mod auto;
mod color;
mod cpu;
mod cube;
mod detail;
mod geom;
mod gpu;
mod patch;
mod portrait;
mod prep;
mod presets;
/// Synthetic images and helpers for tests/benchmarks.
#[doc(hidden)]
pub mod testutil;

/// A GPU renderer that returns an error on any GPU failure instead of silently using the
/// CPU. `None` when no adapter is available. Intended for parity tests and diagnostics.
pub fn create_strict_gpu_renderer() -> Option<Box<dyn Renderer>> {
    gpu::GpuRenderer::probe().map(|g| Box::new(gpu::StrictGpu(g)) as Box<dyn Renderer>)
}
