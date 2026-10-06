//! Non-destructive render engine: edit stack (JSON, docs/05 §2) -> pixels.
//!
//! CONTRACT: the public types and signatures in this file are consumed by `ip-core`
//! (and mirrored in `docs/api-contract-m3.md` for the frontend). Implementations live
//! in private modules and may change freely; public items change only in coordination.
//!
//! Pipeline order is fixed (docs/02 §5.3): decode -> geometry (crop/rotate) ->
//! [warp/patch: M4/M5] -> global -> local (masks) -> [beauty: M4] -> LUT -> output sharpen.

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
        self.ops
            .iter()
            .all(|o| matches!(o, Op::Unknown | Op::Warp(Warp::Unknown)))
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
    /// Portrait retouching on skin (M4). Applied after local adjustments.
    Beauty(Beauty),
    /// Parametric geometric reshaping of a face or body (M4). Applied right after crop.
    Warp(Warp),
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
/// Ranges follow Lightroom conventions so users' intuition transfers.
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
}

fn one_f() -> f32 {
    1.0
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

/// Built-in presets (id, display name key, stack). Stacks contain only global/LUT ops.
pub fn builtin_presets() -> Vec<(String, String, EditStack)> {
    imp::builtin_presets()
}

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

mod auto;
mod color;
mod cpu;
mod cube;
mod geom;
mod gpu;
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
