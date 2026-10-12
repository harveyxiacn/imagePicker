//! Export task: copy originals (cleaned of GPS / metadata on request) or render, resize and
//! encode them as JPEG, PNG, TIFF or WebP ([`formats`]), never overwriting existing files.
//! The export presets ([`EXPORT_PRESETS`]) live here too: the dialog (`GET
//! /api/export/presets`) and the assistant's `export` tool share them.

use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use rayon::prelude::*;
use serde::Serialize;

use ip_render::{EditStack, RgbImage};
use ip_worker_client::{AiWorker, EnhanceRequest, MaskPhoto};
use tokio::runtime::Handle;

use crate::catalog::{self, now_ms, PhotoRef};
use crate::edit::service::MaskMode;
use crate::edit::{store, RenderService};
use crate::error::{CoreError, Result};
use crate::events::Event;
use crate::imaging::Imaging;
use crate::jpegmeta::{self, PreparedMeta};
use crate::model::ExportRequest;
use crate::Core;

#[path = "export_formats.rs"]
pub mod formats;

pub use formats::ExportFormat;

pub struct ExportOptions {
    /// Long edge of the result (`None` = original size).
    pub long_edge: Option<u32>,
    /// JPEG quality (PNG, TIFF and WebP are lossless).
    pub quality: u8,
    pub name_template: String,
    /// Leave the GPS position out (EXIF GPS IFD, XMP `GPS*` properties); copies of originals
    /// are cleaned rather than copied.
    pub strip_gps: bool,
    /// Leave out all EXIF / XMP / IPTC; cleaned copies keep the colour profile and the
    /// orientation the stored pixels need.
    pub strip_metadata: bool,
    pub format: ExportFormat,
    pub upscale: Option<UpscaleExport>,
    /// Centre-crop every output to this aspect ratio `(w, h)`; `long_edge` then sizes the crop
    /// (scaling down only).
    pub aspect: Option<(u32, u32)>,
    /// Values of the `{event}` / `{scene}` / `{camera}` / `{rating}` name tokens per photo.
    pub names: HashMap<i64, NameInfo>,
}

impl Default for ExportOptions {
    fn default() -> Self {
        ExportOptions {
            long_edge: None,
            quality: 90,
            name_template: "{name}".into(),
            strip_gps: false,
            strip_metadata: false,
            format: ExportFormat::Original,
            upscale: None,
            aspect: None,
            names: HashMap::new(),
        }
    }
}

// ------------------------------------------------------------------ presets

/// An export preset: the one table the export dialog (`GET /api/export/presets`) and the
/// assistant's `export` tool both use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ExportPreset {
    pub id: &'static str,
    /// Long edge of the result (`None` = original size).
    pub long_edge: Option<u32>,
    /// Centre crop to this aspect ratio `[w, h]` before sizing.
    pub aspect: Option<[u32; 2]>,
    pub quality: u8,
    pub format: ExportFormat,
    pub strip_gps: bool,
}

/// Export presets: original files, WeChat (long edge 2048), Xiaohongshu (3:4, 1440x1920),
/// Instagram (4:5, 1080x1350). The sharing presets leave the GPS position out.
pub const EXPORT_PRESETS: &[ExportPreset] = &[
    ExportPreset {
        id: "original",
        long_edge: None,
        aspect: None,
        quality: 95,
        format: ExportFormat::Original,
        strip_gps: false,
    },
    ExportPreset {
        id: "wechat",
        long_edge: Some(2048),
        aspect: None,
        quality: 85,
        format: ExportFormat::Jpeg,
        strip_gps: true,
    },
    ExportPreset {
        id: "xiaohongshu",
        long_edge: Some(1920),
        aspect: Some([3, 4]),
        quality: 90,
        format: ExportFormat::Jpeg,
        strip_gps: true,
    },
    ExportPreset {
        id: "instagram",
        long_edge: Some(1350),
        aspect: Some([4, 5]),
        quality: 90,
        format: ExportFormat::Jpeg,
        strip_gps: true,
    },
];

/// The ids of [`EXPORT_PRESETS`], in order (the enum of the assistant's `export` tool).
pub const EXPORT_PRESET_IDS: [&str; 4] = [
    EXPORT_PRESETS[0].id,
    EXPORT_PRESETS[1].id,
    EXPORT_PRESETS[2].id,
    EXPORT_PRESETS[3].id,
];

pub fn export_preset(id: &str) -> Option<&'static ExportPreset> {
    EXPORT_PRESETS.iter().find(|p| p.id == id)
}

impl ExportPreset {
    /// Sets size, crop, quality, format and the GPS option of `req` to the preset's.
    pub fn apply(&self, req: &mut ExportRequest) {
        req.long_edge = self.long_edge;
        req.aspect = self.aspect;
        req.quality = self.quality;
        req.format = self.format;
        req.strip_gps = self.strip_gps;
    }
}

// ------------------------------------------------------------------ geometry

/// Center-crops `img` to the aspect ratio of `(w, h)`, then scales it down to `w x h` when it
/// is larger. With a huge `(w, h)` (see [`fit_target`]) it only crops.
pub fn fit_crop(img: &RgbImage, w: u32, h: u32) -> RgbImage {
    let (iw, ih) = (img.width as u64, img.height as u64);
    if iw == 0 || ih == 0 || w == 0 || h == 0 {
        return img.clone();
    }
    // largest centered region with aspect w:h
    let (cw, ch) = if iw * h as u64 > ih * w as u64 {
        ((ih * w as u64 / h as u64).max(1), ih)
    } else {
        (iw, (iw * h as u64 / w as u64).max(1))
    };
    let (x0, y0) = ((iw - cw) / 2, (ih - ch) / 2);
    let mut crop = Vec::with_capacity((cw * ch * 3) as usize);
    for y in 0..ch {
        let start = (((y0 + y) * iw + x0) * 3) as usize;
        crop.extend_from_slice(&img.data[start..start + (cw * 3) as usize]);
    }
    let (ow, oh) = if cw > w as u64 {
        (w, h)
    } else {
        (cw as u32, ch as u32)
    };
    if (ow as u64, oh as u64) == (cw, ch) {
        return RgbImage {
            width: cw as u32,
            height: ch as u32,
            data: crop,
        };
    }
    let buf = image::RgbImage::from_raw(cw as u32, ch as u32, crop).expect("size matches");
    let out = image::imageops::resize(&buf, ow, oh, image::imageops::FilterType::Triangle);
    RgbImage {
        width: ow,
        height: oh,
        data: out.into_raw(),
    }
}

/// The `(w, h)` an output cropped to `aspect` is fitted into: long edge `long_edge`, or no
/// scaling at all without one.
pub fn fit_target(aspect: (u32, u32), long_edge: Option<u32>) -> (u32, u32) {
    let (a, b) = (aspect.0.max(1) as u64, aspect.1.max(1) as u64);
    // without a long edge: a box of the right shape that no picture exceeds
    let l = long_edge.map_or(1 << 20, |l| l as u64);
    let (w, h) = if a >= b {
        (l, (l * b + a / 2) / a)
    } else {
        ((l * a + b / 2) / b, l)
    };
    (w.max(1) as u32, h.max(1) as u32)
}

/// Scales `img` down so that no side exceeds `max` (the WebP limit).
fn cap_side(img: RgbImage, max: Option<u32>) -> RgbImage {
    let Some(max) = max else { return img };
    let long = img.width.max(img.height);
    if long <= max {
        return img;
    }
    let scale = |v: u32| ((v as u64 * max as u64 / long as u64).max(1)) as u32;
    let (w, h) = (scale(img.width), scale(img.height));
    let buf = image::RgbImage::from_raw(img.width, img.height, img.data).expect("size matches");
    let out = image::imageops::resize(&buf, w, h, image::imageops::FilterType::Triangle);
    RgbImage {
        width: w,
        height: h,
        data: out.into_raw(),
    }
}

/// Super-resolution of exports: render -> temp PNG -> `enhance.run upscale` -> encode.
pub struct UpscaleExport {
    pub svc: Arc<RenderService>,
    pub worker: Arc<dyn AiWorker>,
    pub handle: Handle,
    pub scale: u32,
    /// Scratch directory for the intermediate PNGs and the worker's output.
    pub tmp: PathBuf,
}

/// Saved edits to render into the output (M3): photos listed in `stacks` are rendered at full
/// resolution and encoded in the export's format; the rest follow the original M1 behaviour.
pub struct EditedExport {
    pub svc: Arc<RenderService>,
    pub handle: Handle,
    pub stacks: HashMap<i64, EditStack>,
}

#[derive(Debug, Default)]
pub struct ExportReport {
    pub written: Vec<PathBuf>,
    pub errors: Vec<(i64, String)>,
}

/// Days since 1970-01-01 -> (year, month, day), proleptic Gregorian.
pub(crate) fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn date_string(ms: i64) -> String {
    let (y, m, d) = civil_from_days(ms.div_euclid(86_400_000));
    format!("{y:04}{m:02}{d:02}")
}

fn sanitize(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    cleaned
        .trim_matches(|c: char| c == '.' || c == ' ')
        .to_string()
}

/// Catalog values of the name tokens that [`PhotoRef`] does not carry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NameInfo {
    /// `{event}`: title of the import (session) the photo came in with.
    pub event: Option<String>,
    /// `{scene}`: scene type found by the analysis (`portrait`, `landscape`, `night`...).
    pub scene: Option<String>,
    /// `{camera}`: camera make and model.
    pub camera: Option<String>,
    /// `{rating}`: the user's star rating.
    pub rating: Option<i64>,
}

/// Name tokens whose values come from the catalog ([`NameInfo`]).
pub const CATALOG_TOKENS: [&str; 4] = ["{event}", "{scene}", "{camera}", "{rating}"];
/// Longest text a catalog token contributes to a file name.
const TOKEN_MAX_CHARS: usize = 64;

/// Expands `{name}` `{seq}` `{date}` `{event}` `{scene}` `{camera}` `{rating}` in one pass
/// (values are never expanded again), then makes the result a safe file name.
fn render_name(template: &str, r: &PhotoRef, seq: usize, info: Option<&NameInfo>) -> String {
    let stem = Path::new(&r.file_name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("photo{}", r.id));
    let date = r
        .taken_at
        .or(r.mtime_ms)
        .map(date_string)
        .unwrap_or_else(|| "nodate".into());
    let text = |v: Option<&String>, none: &str| {
        v.map(|v| v.trim())
            .filter(|v| !v.is_empty())
            .map(|v| v.chars().take(TOKEN_MAX_CHARS).collect())
            .unwrap_or_else(|| none.to_string())
    };
    let mut out = String::with_capacity(template.len() + 32);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let tail = &rest[open..];
        let Some(close) = tail.find('}') else {
            out.push_str(tail);
            rest = "";
            break;
        };
        let value = match &tail[1..close] {
            "name" => Some(stem.clone()),
            "seq" => Some(format!("{seq:04}")),
            "date" => Some(date.clone()),
            "event" => Some(text(info.and_then(|i| i.event.as_ref()), "noevent")),
            "scene" => Some(text(info.and_then(|i| i.scene.as_ref()), "noscene")),
            "camera" => Some(text(info.and_then(|i| i.camera.as_ref()), "nocamera")),
            "rating" => Some(info.and_then(|i| i.rating).unwrap_or(0).to_string()),
            _ => None,
        };
        match value {
            Some(v) => {
                out.push_str(&v);
                rest = &tail[close + 1..];
            }
            None => {
                out.push('{');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    let out = sanitize(&out);
    if out.is_empty() {
        sanitize(&stem)
    } else {
        out
    }
}

/// The extension of a copied original (`jpg` when it has none).
fn source_ext(r: &PhotoRef) -> String {
    Path::new(&r.file_name)
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_else(|| "jpg".into())
}

/// Decide the output file name of every photo up front (collision-free, deterministic).
pub fn plan_names(refs: &[PhotoRef], template: &str, dest: &Path, resized: bool) -> Vec<PathBuf> {
    plan_names_with(
        refs,
        template,
        dest,
        &|i| {
            if resized {
                "jpg".to_string()
            } else {
                source_ext(&refs[i])
            }
        },
        &HashMap::new(),
    )
}

/// Like [`plan_names`] with the extension of the `i`-th output given and the catalog token
/// values of every photo. Names are never reused within the export (case-insensitively) and
/// never collide with existing files: `name_1`, `name_2`... instead.
pub fn plan_names_with(
    refs: &[PhotoRef],
    template: &str,
    dest: &Path,
    ext: &dyn Fn(usize) -> String,
    names: &HashMap<i64, NameInfo>,
) -> Vec<PathBuf> {
    let mut taken: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(refs.len());
    for (i, r) in refs.iter().enumerate() {
        let base = render_name(template, r, i + 1, names.get(&r.id));
        let ext = ext(i);
        let mut n = 0u32;
        let path = loop {
            let name = if n == 0 {
                format!("{base}.{ext}")
            } else {
                format!("{base}_{n}.{ext}")
            };
            let p = dest.join(&name);
            if !taken.contains(&name.to_lowercase()) && !p.exists() {
                taken.insert(name.to_lowercase());
                break p;
            }
            n += 1;
        };
        out.push(path);
    }
    out
}

/// The catalog values of the name tokens of `ids` (see [`NameInfo`]).
fn name_info(c: &rusqlite::Connection, ids: &[i64]) -> Result<HashMap<i64, NameInfo>> {
    let mut out = HashMap::new();
    for chunk in ids.chunks(500) {
        let ph = vec!["?"; chunk.len()].join(",");
        let mut st = c.prepare(&format!(
            "SELECT p.id,
                    (SELECT s.title FROM session_photo sp JOIN session s ON s.id = sp.session_id
                      WHERE sp.photo_id = p.id ORDER BY s.id LIMIT 1),
                    p.scene_type, p.camera, p.user_rating
               FROM photo p WHERE p.id IN ({ph})"
        ))?;
        let rows = st.query_map(rusqlite::params_from_iter(chunk.iter()), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                NameInfo {
                    event: row.get(1)?,
                    scene: row.get(2)?,
                    camera: row.get(3)?,
                    rating: row.get(4)?,
                },
            ))
        })?;
        for row in rows {
            let (id, info) = row?;
            out.insert(id, info);
        }
    }
    Ok(out)
}

/// Writes a new file and never touches an existing one: fails when `path` exists (even when it
/// appeared after the names were planned). Only a file this call created is removed again when
/// filling it fails.
fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    fill_new(path, |f| f.write_all(bytes))
}

/// [`write_new`] with the content of `src`.
fn copy_new(src: &Path, dst: &Path) -> std::io::Result<()> {
    let mut input = std::fs::File::open(src)?;
    fill_new(dst, |f| std::io::copy(&mut input, f).map(drop))
}

fn fill_new(
    path: &Path,
    fill: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let mut f = OpenOptions::new().write(true).create_new(true).open(path)?;
    let res = fill(&mut f);
    drop(f);
    if res.is_err() {
        let _ = std::fs::remove_file(path); // ours: created just above
    }
    res
}

/// The XMP sidecar of a source (`<stem>.xmp` / `<file>.xmp`), unless unreasonably large.
fn sidecar_xmp(path: &Path) -> Option<String> {
    let p = crate::xmp::find_sidecar(path)?;
    if std::fs::metadata(&p).ok()?.len() > 4 << 20 {
        return None;
    }
    std::fs::read_to_string(p).ok()
}

/// What the source contributes to the metadata of a re-encoded export: a JPEG its own EXIF,
/// XMP (else the sidecar's) and IPTC; other formats their EXIF (copied or rebuilt by the
/// imaging layer, else synthesised from the catalog metadata) and the sidecar XMP.
fn source_meta(imaging: &dyn Imaging, r: &PhotoRef) -> jpegmeta::SourceMeta {
    if r.format == ip_imaging::ImageFormat::Jpeg {
        let mut m = jpegmeta::SourceMeta::of_jpeg_file(&r.path);
        if m.xmp.is_none() {
            m.xmp = sidecar_xmp(&r.path);
        }
        return m;
    }
    jpegmeta::SourceMeta {
        exif: imaging.read_exif(&r.path, r.format).ok().flatten(),
        xmp: sidecar_xmp(&r.path),
        irb: None,
        metadata: imaging.read_metadata(&r.path, r.format).ok(),
    }
}

/// The metadata a re-encoded export of `r` with `dims` pixels carries (nothing with
/// `strip_metadata`).
fn meta_for(
    imaging: &dyn Imaging,
    r: &PhotoRef,
    opts: &ExportOptions,
    dims: (u32, u32),
) -> PreparedMeta {
    if opts.strip_metadata {
        return PreparedMeta::default();
    }
    jpegmeta::prepare_meta(&source_meta(imaging, r), dims, opts.strip_gps)
}

/// Upscaled render of one photo (see [`UpscaleExport`]).
fn render_upscaled(
    r: &PhotoRef,
    stack: Option<&EditStack>,
    up: &UpscaleExport,
    opts: &ExportOptions,
) -> std::result::Result<RgbImage, String> {
    let edge = opts
        .long_edge
        .map(|e| (e as f64 / up.scale as f64).ceil().max(16.0) as u32);
    let default_stack = EditStack::default();
    let rendered = up
        .svc
        .render_gated(
            r,
            stack.unwrap_or(&default_stack),
            edge,
            MaskMode::Strict,
            &up.handle,
        )
        .map_err(|e| e.to_string())?;
    let dir = up.tmp.join(r.id.to_string());
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let input = dir.join("render.png");
    image::RgbImage::from_raw(
        rendered.image.width,
        rendered.image.height,
        rendered.image.data.clone(),
    )
    .ok_or_else(|| "bad render buffer".to_string())?
    .save_with_format(&input, image::ImageFormat::Png)
    .map_err(|e| e.to_string())?;
    drop(rendered);
    let resp = up
        .handle
        .block_on(up.worker.enhance_run(&EnhanceRequest {
            photo: MaskPhoto {
                photo_id: r.id,
                path: input.to_string_lossy().into_owned(),
                orientation: 1,
            },
            op: "upscale".into(),
            strength: 1.0,
            scale: Some(up.scale),
            faces: None,
            out_dir: dir.join("out").to_string_lossy().into_owned(),
            allow_download: false,
        }))
        .map_err(|e| crate::analysis::map_worker_err(e).to_string());
    let result = resp.and_then(|resp| {
        let path = resp
            .image
            .ok_or_else(|| "enhance.run returned no image".to_string())?;
        let big = image::open(&path)
            .map_err(|e| format!("unreadable upscaled image: {e}"))?
            .to_rgb8();
        Ok(RgbImage {
            width: big.width(),
            height: big.height(),
            data: big.into_raw(),
        })
    });
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// How one photo is exported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    /// The original file, byte for byte.
    Copy,
    /// The original without its GPS position / metadata, image data untouched.
    Clean,
    /// Decoded or rendered, then encoded in this format.
    Encode(ExportFormat),
}

fn route(r: &PhotoRef, opts: &ExportOptions, edited: bool) -> Route {
    let changed =
        edited || opts.long_edge.is_some() || opts.upscale.is_some() || opts.aspect.is_some();
    let encode = Route::Encode(opts.format.encoded());
    if changed || !opts.format.keeps(r.format) {
        encode
    } else if !(opts.strip_gps || opts.strip_metadata) {
        Route::Copy
    } else if formats::cleanable(r.format) {
        Route::Clean
    } else {
        encode // RAW, HEIF, TIFF: cleaned by re-encoding
    }
}

/// The original without its GPS position / all metadata ([`formats::clean_original`]); a file
/// that cannot be parsed is re-encoded in its own format instead.
fn clean_copy(
    imaging: &dyn Imaging,
    r: &PhotoRef,
    opts: &ExportOptions,
) -> std::result::Result<Vec<u8>, String> {
    let bytes = std::fs::read(&r.path).map_err(|e| e.to_string())?;
    match formats::clean_original(r.format, &bytes, opts.strip_metadata, r.orientation) {
        Some(clean) => Ok(clean),
        None => encode_photo(
            imaging,
            r,
            None,
            None,
            opts,
            ExportFormat::of_source(r.format),
        ),
    }
}

/// Renders (edits, crop, upscale) or decodes one photo and encodes it as `format`.
fn encode_photo(
    imaging: &dyn Imaging,
    r: &PhotoRef,
    stack: Option<&EditStack>,
    edits: Option<&EditedExport>,
    opts: &ExportOptions,
    format: ExportFormat,
) -> std::result::Result<Vec<u8>, String> {
    let max_side = format.max_side();
    if let (ExportFormat::Jpeg, None, None, None, Some(edge)) = (
        format,
        stack,
        opts.upscale.as_ref(),
        opts.aspect,
        opts.long_edge,
    ) {
        // the imaging layer's fast path (embedded previews, DCT-domain scaling)
        let img = imaging
            .generate_thumbnail(&r.path, r.format, r.orientation, edge, opts.quality)
            .map_err(|e| format!("{e:#}"))?;
        let meta = meta_for(imaging, r, opts, (img.width, img.height));
        return Ok(jpegmeta::with_meta(&img.bytes, &meta));
    }
    // a crop is rendered at twice the long edge so that it keeps its resolution
    let edge = match (opts.aspect, opts.long_edge) {
        (Some(_), Some(l)) => Some(l.saturating_mul(2)),
        (None, Some(l)) => Some(max_side.map_or(l, |m| l.min(m))),
        (_, None) => None,
    };
    let img = match (stack, &opts.upscale) {
        (stack, Some(up)) => render_upscaled(r, stack, up, opts)?,
        (Some(stack), None) => {
            let e = edits.expect("edited stacks come with a render context");
            e.svc
                .render_gated(r, stack, edge, MaskMode::Strict, &e.handle)
                .map_err(|e| e.to_string())?
                .image
        }
        (None, None) => {
            let (width, height, data) = imaging
                .decode_rgb8(&r.path, r.format, r.orientation, edge.unwrap_or(u32::MAX))
                .map_err(|e| format!("{e:#}"))?;
            RgbImage {
                width,
                height,
                data,
            }
        }
    };
    let img = match opts.aspect {
        Some(aspect) => {
            let (w, h) = fit_target(aspect, opts.long_edge);
            fit_crop(&img, w, h)
        }
        None => img,
    };
    let img = cap_side(img, max_side);
    let meta = meta_for(imaging, r, opts, (img.width, img.height));
    formats::encode(&img, format, opts.quality, &meta)
}

/// Runs the export synchronously (parallel over photos). `progress(done, total)` is called after each photo.
pub fn export_photos(
    imaging: &dyn Imaging,
    refs: &[PhotoRef],
    dest: &Path,
    opts: &ExportOptions,
    edits: Option<&EditedExport>,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> ExportReport {
    let edited = |r: &PhotoRef| edits.and_then(|e| e.stacks.get(&r.id));
    let routes: Vec<Route> = refs
        .iter()
        .map(|r| route(r, opts, edited(r).is_some()))
        .collect();
    let names = plan_names_with(
        refs,
        &opts.name_template,
        dest,
        &|i| match routes[i] {
            Route::Encode(f) => f.extension().to_string(),
            Route::Copy | Route::Clean => source_ext(&refs[i]),
        },
        &opts.names,
    );
    let done = AtomicUsize::new(0);
    let total = refs.len();
    let results: Vec<std::result::Result<PathBuf, (i64, String)>> = refs
        .par_iter()
        .zip(names.par_iter())
        .zip(routes.par_iter())
        .map(|((r, out), route)| {
            let res = match *route {
                Route::Copy => copy_new(&r.path, out).map_err(|e| e.to_string()),
                Route::Clean => clean_copy(imaging, r, opts)
                    .and_then(|bytes| write_new(out, &bytes).map_err(|e| e.to_string())),
                Route::Encode(format) => encode_photo(imaging, r, edited(r), edits, opts, format)
                    .and_then(|bytes| write_new(out, &bytes).map_err(|e| e.to_string())),
            };
            let d = done.fetch_add(1, Ordering::SeqCst) + 1;
            progress(d, total);
            // a failed photo leaves nothing behind: `write_new` / `copy_new` remove only what
            // they created, a file someone else put there in the meantime stays untouched
            res.map(|()| out.clone())
                .map_err(|e| (r.id, format!("{}: {e}", r.file_name)))
        })
        .collect();
    let mut report = ExportReport::default();
    for r in results {
        match r {
            Ok(p) => report.written.push(p),
            Err(e) => report.errors.push(e),
        }
    }
    report
}

impl Core {
    /// Validates and starts an export task; progress arrives as `task.progress` events.
    pub async fn export(self: &Arc<Self>, req: ExportRequest) -> Result<String> {
        let folders = req.folders.clone().filter(|f| !f.is_empty());
        match (&folders, req.ids.is_empty()) {
            (Some(_), false) => {
                return Err(CoreError::bad_request(
                    "give either ids or folders, not both",
                ))
            }
            (None, true) => return Err(CoreError::bad_request("ids must not be empty")),
            _ => {}
        }
        if req.dest.trim().is_empty() {
            return Err(CoreError::bad_request("dest must not be empty"));
        }
        if req.long_edge == Some(0) {
            return Err(CoreError::bad_request("long_edge must be positive or null"));
        }
        if !(1..=100).contains(&req.quality) {
            return Err(CoreError::bad_request("quality must be 1..100"));
        }
        if req.upscale.is_some_and(|s| s != 2 && s != 4) {
            return Err(CoreError::bad_request("upscale must be 2 or 4"));
        }
        if req
            .aspect
            .is_some_and(|a| a.iter().any(|v| !(1..=10_000).contains(v)))
        {
            return Err(CoreError::bad_request(
                "aspect must be [width, height] with values 1..10000",
            ));
        }
        if let Some(scale) = req.upscale {
            let scale = scale.to_string();
            self.preflight_models("enhance", &["upscale", scale.as_str()])
                .await?;
        }
        let dest = PathBuf::from(&req.dest);
        std::fs::create_dir_all(&dest)
            .map_err(|e| CoreError::bad_request(format!("cannot create destination: {e}")))?;
        // (output directory, photos); one group unless `folders` was given
        let mut groups: Vec<(PathBuf, Vec<PhotoRef>)> = Vec::new();
        match folders {
            None => {
                let ids = req.ids.clone();
                let refs = self.db.call(move |c| catalog::photo_refs(c, &ids)).await?;
                groups.push((dest.clone(), refs));
            }
            Some(folders) => {
                for (name, ids) in folders {
                    let clean = sanitize(&name);
                    if clean.is_empty() {
                        return Err(CoreError::bad_request(format!(
                            "folder name {name:?} is not usable"
                        )));
                    }
                    let dir = dest.join(&clean);
                    let refs = self.db.call(move |c| catalog::photo_refs(c, &ids)).await?;
                    if refs.is_empty() {
                        continue;
                    }
                    match groups.iter_mut().find(|(d, _)| *d == dir) {
                        Some((_, existing)) => existing.extend(refs),
                        None => groups.push((dir, refs)),
                    }
                }
            }
        }
        let refs: Vec<PhotoRef> = groups.iter().flat_map(|(_, r)| r.iter().cloned()).collect();
        if refs.is_empty() {
            return Err(CoreError::not_found("none of the given photos exist"));
        }
        for (dir, _) in &groups {
            std::fs::create_dir_all(dir).map_err(|e| {
                CoreError::bad_request(format!("cannot create {}: {e}", dir.display()))
            })?;
        }
        let task_id = format!("export-{}", self.next_task_seq());
        let total = refs.len();
        {
            let (tid, params) = (
                task_id.clone(),
                serde_json::json!({"dest": req.dest, "long_edge": req.long_edge, "count": total})
                    .to_string(),
            );
            self.db
                .call(move |c| {
                    c.execute(
                        "INSERT INTO task(id, kind, status, priority, params, progress, created_at, updated_at)
                         VALUES(?1,'export','running',0,?2,0,?3,?3)",
                        rusqlite::params![tid, params, now_ms()],
                    )?;
                    Ok(())
                })
                .await?;
        }
        let names = if CATALOG_TOKENS.iter().any(|t| req.name_template.contains(t)) {
            let ids: Vec<i64> = refs.iter().map(|r| r.id).collect();
            self.db.call(move |c| name_info(c, &ids)).await?
        } else {
            HashMap::new()
        };
        let core = self.clone();
        let tid = task_id.clone();
        let opts = ExportOptions {
            long_edge: req.long_edge,
            quality: req.quality,
            name_template: req.name_template.clone(),
            strip_gps: req.strip_gps || req.strip_metadata,
            strip_metadata: req.strip_metadata,
            format: req.format,
            aspect: req.aspect.map(|[w, h]| (w, h)),
            names,
            upscale: req.upscale.map(|scale| UpscaleExport {
                svc: self.render.clone(),
                worker: self.worker.clone(),
                handle: Handle::current(),
                scale,
                tmp: self.dirs.gen.join(&task_id),
            }),
        };
        let edits = if req.apply_edits {
            let with_edits: Vec<i64> = refs
                .iter()
                .filter(|r| r.edit_hash.is_some())
                .map(|r| r.id)
                .collect();
            let stacks = self
                .db
                .call(move |c| {
                    let mut m = HashMap::new();
                    for id in with_edits {
                        if let Some(cur) = store::current(c, id)? {
                            if cur.has_edits {
                                let st: EditStack =
                                    serde_json::from_value(cur.stack).map_err(|e| {
                                        CoreError::Internal(anyhow::anyhow!(
                                            "stored edit stack of photo {id} is invalid: {e}"
                                        ))
                                    })?;
                                m.insert(id, st);
                            }
                        }
                    }
                    Ok(m)
                })
                .await?;
            Some(EditedExport {
                svc: self.render.clone(),
                handle: Handle::current(),
                stacks,
            })
        } else {
            None
        };
        tokio::spawn(async move {
            let ev = |done: usize, state: &str, error: Option<String>| Event::TaskProgress {
                task_id: tid.clone(),
                kind: "export".into(),
                done: done as i64,
                total: total as i64,
                state: state.into(),
                error,
            };
            core.events.emit(ev(0, "running", None));
            let (imaging, events, tid2) = (core.imaging.clone(), core.events.clone(), tid.clone());
            let res = tokio::task::spawn_blocking(move || {
                let mut report = ExportReport::default();
                let mut offset = 0usize;
                for (dir, refs) in &groups {
                    let rep =
                        export_photos(&*imaging, refs, dir, &opts, edits.as_ref(), &|done, _| {
                            events.emit(Event::TaskProgress {
                                task_id: tid2.clone(),
                                kind: "export".into(),
                                done: (offset + done) as i64,
                                total: total as i64,
                                state: "running".into(),
                                error: None,
                            });
                        });
                    offset += refs.len();
                    report.written.extend(rep.written);
                    report.errors.extend(rep.errors);
                }
                report
            })
            .await;
            let (state, error, done) = match res {
                Ok(rep) if rep.errors.is_empty() => ("done", None, rep.written.len()),
                Ok(rep) if rep.written.is_empty() => (
                    "failed",
                    Some(rep.errors.first().map(|e| e.1.clone()).unwrap_or_default()),
                    0,
                ),
                Ok(rep) => (
                    "done",
                    Some(format!(
                        "{} of {} failed; first: {}",
                        rep.errors.len(),
                        total,
                        rep.errors[0].1
                    )),
                    rep.written.len(),
                ),
                Err(e) => ("failed", Some(format!("export task crashed: {e}")), 0),
            };
            let (tid3, st, er) = (tid.clone(), state.to_string(), error.clone());
            let _ = core
                .db
                .call(move |c| {
                    c.execute(
                        "UPDATE task SET status=?2, error=?3, progress=1, updated_at=?4 WHERE id=?1",
                        rusqlite::params![tid3, st, er, now_ms()],
                    )?;
                    Ok(())
                })
                .await;
            core.events.emit(ev(done, state, error));
        });
        Ok(task_id)
    }
}

#[cfg(test)]
#[path = "export_tests.rs"]
mod format_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use ip_imaging::ImageFormat;

    fn r(id: i64, name: &str, taken: Option<i64>) -> PhotoRef {
        PhotoRef {
            id,
            path: PathBuf::from(name),
            file_name: name.to_string(),
            format: ImageFormat::Jpeg,
            orientation: 1,
            fast_key: "0".repeat(32),
            taken_at: taken,
            mtime_ms: None,
            edit_hash: None,
            width: None,
            height: None,
        }
    }

    #[test]
    fn civil_dates() {
        assert_eq!(date_string(0), "19700101");
        assert_eq!(date_string(1_709_164_800_000), "20240229");
    }

    #[test]
    fn collisions_get_suffixes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.jpg"), b"x").unwrap();
        let refs = vec![
            r(1, "a.jpg", None),
            r(2, "a.jpg", None),
            r(3, "b.jpg", None),
        ];
        let names = plan_names(&refs, "{name}", dir.path(), false);
        let n: Vec<String> = names
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(n, vec!["a_1.jpg", "a_2.jpg", "b.jpg"]);
    }

    #[test]
    fn template_placeholders() {
        let dir = tempfile::tempdir().unwrap();
        let refs = vec![r(1, "IMG_1.CR2", Some(1_709_164_800_000))];
        let names = plan_names(&refs, "{date}_{seq}_{name}", dir.path(), true);
        assert_eq!(
            names[0].file_name().unwrap().to_string_lossy(),
            "20240229_0001_IMG_1.jpg"
        );
        let names = plan_names(&refs, "{date}/{name}", dir.path(), false);
        assert_eq!(
            names[0].file_name().unwrap().to_string_lossy(),
            "20240229_IMG_1.CR2"
        );
    }

    #[test]
    fn catalog_tokens_expand_once_with_fallbacks() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("4.jpg"), b"x").unwrap();
        let refs = vec![
            r(1, "IMG_1.CR2", Some(1_709_164_800_000)),
            r(2, "IMG_2.jpg", None),
        ];
        let mut names = HashMap::new();
        names.insert(
            1,
            NameInfo {
                event: Some(" Kyoto: day 1 ".into()),
                scene: Some("portrait".into()),
                camera: Some("Canon EOS R5".into()),
                rating: Some(4),
            },
        );
        names.insert(
            2,
            NameInfo {
                camera: Some("{seq}/x".into()),
                ..Default::default()
            },
        );
        let plan = |tpl: &str| -> Vec<String> {
            plan_names_with(&refs, tpl, dir.path(), &|_| "jpg".into(), &names)
                .iter()
                .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(
            plan("{date}_{event}_{scene}_{camera}_{rating}_{seq}"),
            [
                "20240229_Kyoto_ day 1_portrait_Canon EOS R5_4_0001.jpg",
                "nodate_noevent_noscene_{seq}_x_0_0002.jpg",
            ]
        );
        // unknown tokens and stray braces stay, collisions (with files too) get suffixes
        assert_eq!(plan("{foo}{name}{"), ["{foo}IMG_1{.jpg", "{foo}IMG_2{.jpg"]);
        assert_eq!(plan("{rating}"), ["4_1.jpg", "0.jpg"]);
    }

    #[test]
    fn one_preset_table() {
        let ids: Vec<&str> = EXPORT_PRESETS.iter().map(|p| p.id).collect();
        assert_eq!(ids, EXPORT_PRESET_IDS);
        let crop = |id: &str| {
            let p = export_preset(id).unwrap();
            let [a, b] = p.aspect.unwrap();
            fit_target((a, b), p.long_edge)
        };
        assert_eq!(crop("xiaohongshu"), (1440, 1920), "3:4");
        assert_eq!(crop("instagram"), (1080, 1350), "4:5");
        assert!(export_preset("print").is_none());
        let mut req = ExportRequest::default();
        export_preset("wechat").unwrap().apply(&mut req);
        assert_eq!(
            (
                req.long_edge,
                req.aspect,
                req.quality,
                req.format,
                req.strip_gps
            ),
            (Some(2048), None, 85, ExportFormat::Jpeg, true)
        );
        export_preset("original").unwrap().apply(&mut req);
        assert_eq!(
            (req.long_edge, req.quality, req.format, req.strip_gps),
            (None, 95, ExportFormat::Original, false)
        );
        let json = serde_json::to_value(EXPORT_PRESETS[2]).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"id": "xiaohongshu", "long_edge": 1920, "aspect": [3, 4],
                "quality": 90, "format": "jpeg", "strip_gps": true})
        );
    }

    #[test]
    fn crop_targets_and_crop_only() {
        assert_eq!(fit_target((16, 9), Some(1920)), (1920, 1080));
        assert_eq!(fit_target((1, 1), Some(500)), (500, 500));
        let (w, h) = fit_target((3, 4), None);
        assert_eq!(w as u64 * 4, h as u64 * 3);
        let img = RgbImage {
            width: 400,
            height: 300,
            data: vec![7; 400 * 300 * 3],
        };
        let c = fit_crop(&img, w, h);
        assert_eq!((c.width, c.height), (225, 300), "cropped, not scaled");
        let c = cap_side(img, Some(100));
        assert_eq!((c.width, c.height), (100, 75));
    }

    #[test]
    fn routes_by_format_and_strip_options() {
        let jpg = r(1, "a.jpg", None);
        let mut raw = r(2, "a.cr2", None);
        raw.format = ImageFormat::Raw;
        let mut opts = ExportOptions::default();
        assert_eq!(route(&jpg, &opts, false), Route::Copy);
        assert_eq!(route(&raw, &opts, false), Route::Copy);
        assert_eq!(route(&jpg, &opts, true), Route::Encode(ExportFormat::Jpeg));
        opts.strip_gps = true;
        assert_eq!(route(&jpg, &opts, false), Route::Clean);
        assert_eq!(route(&raw, &opts, false), Route::Encode(ExportFormat::Jpeg));
        opts.format = ExportFormat::Png;
        assert_eq!(route(&jpg, &opts, false), Route::Encode(ExportFormat::Png));
        opts.format = ExportFormat::Jpeg;
        opts.strip_gps = false;
        assert_eq!(route(&jpg, &opts, false), Route::Copy);
        assert_eq!(route(&raw, &opts, false), Route::Encode(ExportFormat::Jpeg));
        opts.aspect = Some((3, 4));
        assert_eq!(route(&jpg, &opts, false), Route::Encode(ExportFormat::Jpeg));
    }
}
