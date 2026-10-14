//! Collage / contact-sheet export (PRD F9.5, `docs/api-contract-m5.md` section F.2): the selected
//! photos (2..=30), rendered with their saved edits, are tiled into one image.
//!
//! * [`compute_layout`] is the pure layout algorithm: integer rectangles that tile the canvas
//!   inside the outer margin with exactly `gap` pixels between neighbours. It has a mirror in
//!   `web/src/lib/collageLayout.ts` (the dialog's live preview); both are checked against the
//!   same golden rectangles, so keep them in step.
//! * [`render_collage`] renders every photo (parallel, cancellable), fits it into its cell
//!   (cover crop centred on the faces, or contain), draws captions and returns the canvas.
//! * [`Core::export_collage`] validates the request and runs it as a task of kind `export`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use ab_glyph::{point, Font, FontArc, PxScale, ScaleFont};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use ip_render::RgbImage;
use ip_worker_client::CancelToken;

use crate::catalog::{self, PhotoRef};
use crate::edit::service::MaskMode;
use crate::error::{CoreError, Result};
use crate::events::Event;
use crate::export::{self, formats, EditedExport, ExportFormat};
use crate::imaging::Imaging;
use crate::jpegmeta::PreparedMeta;
use crate::watermark::{PreparedWatermark, Watermark};
use crate::Core;

/// Fewest / most photos of one collage.
pub const MIN_PHOTOS: usize = 2;
pub const MAX_PHOTOS: usize = 30;
/// Canvas side limits (pixels) and the largest canvas area (80 megapixels).
pub const MIN_SIDE: u32 = 64;
pub const MAX_SIDE: u32 = 12_000;
pub const MAX_PIXELS: u64 = 80_000_000;
/// Largest gap / outer margin (pixels).
pub const MAX_SPACING: u32 = 2_000;
/// Longest edge a photo is rendered at for its cell.
const MAX_RENDER_EDGE: u32 = 6_000;
/// Aspect ratio (w / h) assumed for photos whose size is not known.
const DEFAULT_ASPECT: f64 = 1.5;

const DEFAULT_FONT: &[u8] = include_bytes!("../assets/fonts/DejaVuSans.ttf");

// ------------------------------------------------------------------ request

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CollageLayout {
    /// Auto rows / columns, every photo the same size.
    #[default]
    Grid,
    /// The first photo large, the others small around it.
    Story,
    /// Many thumbnails with a caption (file name, rating) under each.
    Contact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CollageFit {
    /// Fill the cell, cropping the overflow (centred on the faces when known).
    #[default]
    Cover,
    /// Show the whole photo, letterboxed on the background.
    Contain,
}

/// Body of `POST /api/export/collage`.
#[derive(Debug, Clone, Deserialize)]
pub struct CollageRequest {
    /// The photos, in layout order (the first is the large one of `story`).
    pub ids: Vec<i64>,
    pub dest: String,
    #[serde(default)]
    pub layout: CollageLayout,
    /// Canvas size in pixels.
    pub width: u32,
    pub height: u32,
    /// Pixels between cells.
    #[serde(default = "d_gap")]
    pub gap: u32,
    /// Pixels between the canvas edge and the cells.
    #[serde(default = "d_margin")]
    pub margin: u32,
    /// `#rrggbb` / `#rgb`.
    #[serde(default = "d_background")]
    pub background: String,
    #[serde(default)]
    pub fit: CollageFit,
    /// `contact`: the file name under each thumbnail.
    #[serde(default = "d_true")]
    pub caption_name: bool,
    /// `contact`: the star rating under each thumbnail.
    #[serde(default = "d_true")]
    pub caption_rating: bool,
    #[serde(default)]
    pub format: ExportFormat,
    #[serde(default = "d_quality")]
    pub quality: u8,
    /// Output file name (same tokens as the export; `{name}` is the first photo).
    #[serde(default = "d_template")]
    pub name_template: String,
    /// Render the saved edits (default); off = the unedited pictures.
    #[serde(default = "d_true")]
    pub apply_edits: bool,
    /// Drawn on the finished collage.
    #[serde(default)]
    pub watermark: Option<Watermark>,
}

fn d_gap() -> u32 {
    16
}
fn d_margin() -> u32 {
    16
}
fn d_background() -> String {
    "#ffffff".into()
}
fn d_true() -> bool {
    true
}
fn d_quality() -> u8 {
    90
}
fn d_template() -> String {
    "collage".into()
}

fn parse_color(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().strip_prefix('#')?;
    if !h.is_ascii() {
        return None;
    }
    let v = |s: &str| u8::from_str_radix(s, 16).ok();
    match h.len() {
        6 => Some([v(&h[0..2])?, v(&h[2..4])?, v(&h[4..6])?]),
        3 => {
            let d = |i: usize| v(&h[i..i + 1]).map(|n| n * 17);
            Some([d(0)?, d(1)?, d(2)?])
        }
        _ => None,
    }
}

impl CollageRequest {
    /// Checks everything that does not need the catalog; the error is the client's.
    pub fn validate(&self) -> Result<()> {
        let bad = |m: String| Err(CoreError::bad_request(m));
        if !(MIN_PHOTOS..=MAX_PHOTOS).contains(&self.ids.len()) {
            return bad(format!(
                "a collage takes {MIN_PHOTOS}..{MAX_PHOTOS} photos, got {}",
                self.ids.len()
            ));
        }
        let mut seen = std::collections::HashSet::new();
        if !self.ids.iter().all(|i| seen.insert(*i)) {
            return bad("ids must not repeat".into());
        }
        if self.dest.trim().is_empty() {
            return bad("dest must not be empty".into());
        }
        for (name, v) in [("width", self.width), ("height", self.height)] {
            if !(MIN_SIDE..=MAX_SIDE).contains(&v) {
                return bad(format!("{name} must be {MIN_SIDE}..{MAX_SIDE}"));
            }
        }
        if self.width as u64 * self.height as u64 > MAX_PIXELS {
            return bad(format!(
                "the canvas is limited to {} megapixels",
                MAX_PIXELS / 1_000_000
            ));
        }
        for (name, v) in [("gap", self.gap), ("margin", self.margin)] {
            if v > MAX_SPACING {
                return bad(format!("{name} must be 0..{MAX_SPACING}"));
            }
        }
        if parse_color(&self.background).is_none() {
            return bad("background must be a colour like #1a1a1a".into());
        }
        if !(1..=100).contains(&self.quality) {
            return bad("quality must be 1..100".into());
        }
        Ok(())
    }
}

// ------------------------------------------------------------------ layout

/// A rectangle in canvas pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Where one photo goes: the picture's rectangle and, for contact sheets, the caption band
/// under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Cell {
    pub photo: Rect,
    pub caption: Option<Rect>,
}

/// Height of the caption band of a contact sheet on a `width` x `height` canvas.
pub fn caption_height(width: u32, height: u32) -> u32 {
    (width.min(height) / 40).max(14)
}

/// Splits `total` pixels into `parts` consecutive pieces separated by `gap`: `(offset, length)`
/// each, lengths differ by at most one (the first ones are the longer). `None` when a piece
/// would be empty.
fn split(total: u32, parts: u32, gap: u32) -> Option<Vec<(u32, u32)>> {
    let gaps = (parts - 1).checked_mul(gap)?;
    let avail = total.checked_sub(gaps)?;
    let (base, rem) = (avail / parts, avail % parts);
    if base == 0 {
        return None;
    }
    let mut at = 0;
    Some(
        (0..parts)
            .map(|i| {
                let len = base + u32::from(i < rem);
                let out = (at, len);
                at += len + gap;
                out
            })
            .collect(),
    )
}

/// Tiles `area` with `aspects.len()` cells: `r` rows of `n / r` or one more cells (the first
/// rows have the extra one), each row stretched over the full width, the `r` that fits the
/// photos' aspect ratios best (least total stretch, fewer rows on a tie). `caption` pixels of
/// every cell are reserved for a caption.
fn grid_in(area: Rect, aspects: &[f64], gap: u32, caption: u32) -> Option<Vec<Rect>> {
    let n = aspects.len() as u32;
    let mut best: Option<(f64, Vec<Rect>)> = None;
    'rows: for r in 1..=n {
        let Some(heights) = split(area.h, r, gap) else {
            break; // more rows only make them thinner
        };
        let (base, extra) = (n / r, n % r);
        let mut cells = Vec::with_capacity(n as usize);
        let mut cost = 0.0f64;
        let mut idx = 0usize;
        for (k, (y, h)) in heights.iter().enumerate() {
            if *h <= caption {
                continue 'rows;
            }
            let count = base + u32::from((k as u32) < extra);
            let Some(widths) = split(area.w, count, gap) else {
                continue 'rows;
            };
            for (x, w) in widths {
                let q = (w as f64 / (*h - caption) as f64) / aspects[idx].max(1e-6);
                cost += q.max(1.0 / q);
                idx += 1;
                cells.push(Rect {
                    x: area.x + x,
                    y: area.y + y,
                    w,
                    h: *h,
                });
            }
        }
        if best.as_ref().is_none_or(|(c, _)| cost < *c) {
            best = Some((cost, cells));
        }
    }
    best.map(|(_, c)| c)
}

/// The cells of a collage of `aspects.len()` photos (width / height of each, in order) on a
/// `width` x `height` canvas. All cells lie inside the `margin`; neighbours are exactly `gap`
/// apart; the cells and gaps cover the area inside the margin completely. Deterministic.
///
/// * `Grid`: [`grid_in`].
/// * `Story`: the first photo takes 2/3 (1/2 when there are more than four small ones) of the
///   long side of the area, the others tile the rest as a grid; one photo fills everything.
/// * `Contact`: a grid whose cells are `caption` pixels taller than their picture; the
///   caption band is the bottom `caption` pixels of the cell.
pub fn compute_layout(
    layout: CollageLayout,
    width: u32,
    height: u32,
    gap: u32,
    margin: u32,
    caption: u32,
    aspects: &[f64],
) -> std::result::Result<Vec<Cell>, String> {
    let n = aspects.len();
    let too_small = || format!("the canvas is too small for {n} photos with this gap and margin");
    if n == 0 {
        return Ok(Vec::new());
    }
    let inner = |v: u32| v.checked_sub(margin.saturating_mul(2)).filter(|v| *v > 0);
    let (Some(iw), Some(ih)) = (inner(width), inner(height)) else {
        return Err(too_small());
    };
    let area = Rect {
        x: margin,
        y: margin,
        w: iw,
        h: ih,
    };
    let plain = |r: Rect| Cell {
        photo: r,
        caption: None,
    };
    match layout {
        CollageLayout::Grid => grid_in(area, aspects, gap, 0)
            .map(|c| c.into_iter().map(plain).collect())
            .ok_or_else(too_small),
        CollageLayout::Contact => grid_in(area, aspects, gap, caption)
            .map(|c| {
                c.into_iter()
                    .map(|r| {
                        if caption == 0 {
                            return plain(r);
                        }
                        Cell {
                            photo: Rect {
                                h: r.h - caption,
                                ..r
                            },
                            caption: Some(Rect {
                                y: r.y + r.h - caption,
                                h: caption,
                                ..r
                            }),
                        }
                    })
                    .collect()
            })
            .ok_or_else(too_small),
        CollageLayout::Story => {
            if n == 1 {
                return Ok(vec![plain(area)]);
            }
            let (num, den) = if n - 1 <= 4 { (2, 3) } else { (1, 2) };
            let landscape = iw >= ih;
            let long = if landscape { iw } else { ih };
            let avail = long.checked_sub(gap).ok_or_else(too_small)?;
            let hero_len = (avail as u64 * num / den) as u32;
            let rest_len = avail - hero_len;
            if hero_len == 0 || rest_len == 0 {
                return Err(too_small());
            }
            let (hero, rest) = if landscape {
                (
                    Rect {
                        w: hero_len,
                        ..area
                    },
                    Rect {
                        x: area.x + hero_len + gap,
                        w: rest_len,
                        ..area
                    },
                )
            } else {
                (
                    Rect {
                        h: hero_len,
                        ..area
                    },
                    Rect {
                        y: area.y + hero_len + gap,
                        h: rest_len,
                        ..area
                    },
                )
            };
            let small = grid_in(rest, &aspects[1..], gap, 0).ok_or_else(too_small)?;
            Ok(std::iter::once(hero).chain(small).map(plain).collect())
        }
    }
}

// ------------------------------------------------------------------ fitting

/// The part of an `iw` x `ih` picture shown in a `cw` x `ch` cell by a cover fit: the largest
/// window with the cell's aspect ratio, centred on `focus` (fractions of the picture, e.g. the
/// middle of the faces; the picture's centre without) and kept inside the picture.
/// Returns `(x, y, w, h)` in picture pixels.
pub fn cover_window(
    iw: u32,
    ih: u32,
    cw: u32,
    ch: u32,
    focus: Option<(f64, f64)>,
) -> (u32, u32, u32, u32) {
    let (iw64, ih64) = (iw as u64, ih as u64);
    let (w, h) = if iw64 * ch as u64 > ih64 * cw as u64 {
        ((ih64 * cw as u64 / ch as u64).clamp(1, iw64), ih64)
    } else {
        (iw64, (iw64 * ch as u64 / cw as u64).clamp(1, ih64))
    };
    let (fx, fy) = focus.unwrap_or((0.5, 0.5));
    let place = |f: f64, full: u64, win: u64| -> u32 {
        let f = if f.is_finite() {
            f.clamp(0.0, 1.0)
        } else {
            0.5
        };
        let start = (f * full as f64 - win as f64 / 2.0).round();
        start.clamp(0.0, (full - win) as f64) as u32
    };
    (place(fx, iw64, w), place(fy, ih64, h), w as u32, h as u32)
}

/// `img` fitted into a `cw` x `ch` cell: `(picture, dx, dy)` with the offset inside the cell
/// (cover fills it; contain centres a smaller picture).
fn fit_into(
    img: &RgbImage,
    cw: u32,
    ch: u32,
    fit: CollageFit,
    focus: Option<(f64, f64)>,
) -> (RgbImage, u32, u32) {
    let src =
        image::RgbImage::from_raw(img.width, img.height, img.data.clone()).expect("size matches");
    let done = |buf: image::RgbImage, dx: u32, dy: u32| {
        (
            RgbImage {
                width: buf.width(),
                height: buf.height(),
                data: buf.into_raw(),
            },
            dx,
            dy,
        )
    };
    match fit {
        CollageFit::Cover => {
            let (x, y, w, h) = cover_window(img.width, img.height, cw, ch, focus);
            let crop = image::imageops::crop_imm(&src, x, y, w, h).to_image();
            let out = if (w, h) == (cw, ch) {
                crop
            } else {
                image::imageops::resize(&crop, cw, ch, image::imageops::FilterType::Triangle)
            };
            done(out, 0, 0)
        }
        CollageFit::Contain => {
            let (iw, ih) = (img.width as u64, img.height as u64);
            let (w, h) = if iw * ch as u64 > ih * cw as u64 {
                (
                    cw,
                    ((ih * cw as u64 + iw / 2) / iw).clamp(1, ch as u64) as u32,
                )
            } else {
                (
                    ((iw * ch as u64 + ih / 2) / ih).clamp(1, cw as u64) as u32,
                    ch,
                )
            };
            let out = if (w, h) == (img.width, img.height) {
                src
            } else {
                image::imageops::resize(&src, w, h, image::imageops::FilterType::Triangle)
            };
            done(out, (cw - w) / 2, (ch - h) / 2)
        }
    }
}

// ------------------------------------------------------------------ captions

/// Draws `text` (left aligned, vertically centred, shortened with `…` to fit) into `rect`.
fn draw_caption(img: &mut RgbImage, font: &FontArc, text: &str, rect: Rect, rgb: [u8; 3]) {
    let px = (rect.h as f32 * 0.62).max(6.0);
    let sf = font.as_scaled(PxScale::from(px));
    let pad = (px * 0.25).round();
    let room = rect.w as f32 - pad * 2.0;
    // glyphs the font lacks (CJK file names) print as '?', not as empty boxes
    let chars: Vec<char> = text
        .chars()
        .map(|c| {
            if c.is_control() {
                ' '
            } else if sf.glyph_id(c).0 == 0 {
                '?'
            } else {
                c
            }
        })
        .collect();
    let adv = |c: char| sf.h_advance(sf.glyph_id(c));
    let mut kept = chars.len();
    let mut total: f32 = chars.iter().map(|c| adv(*c)).sum();
    if total > room {
        let dots = adv('…');
        total = dots;
        kept = 0;
        for c in &chars {
            if total - dots + adv(*c) + dots > room {
                break;
            }
            total += adv(*c);
            kept += 1;
        }
    }
    let mut shown: String = chars[..kept].iter().collect();
    if kept < chars.len() {
        shown.push('…');
    }
    let base = rect.y as f32 + (rect.h as f32 - sf.height()) / 2.0 + sf.ascent();
    let mut x = rect.x as f32 + pad;
    for c in shown.chars() {
        let id = sf.glyph_id(c);
        let g = id.with_scale_and_position(PxScale::from(px), point(x, base));
        x += sf.h_advance(id);
        let Some(o) = font.outline_glyph(g) else {
            continue;
        };
        let b = o.px_bounds();
        o.draw(|ix, iy, cov| {
            let (gx, gy) = (b.min.x as i64 + ix as i64, b.min.y as i64 + iy as i64);
            if gx < rect.x as i64
                || gy < rect.y as i64
                || gx >= (rect.x + rect.w) as i64
                || gy >= (rect.y + rect.h) as i64
                || gx >= img.width as i64
                || gy >= img.height as i64
            {
                return;
            }
            let i = ((gy as usize * img.width as usize) + gx as usize) * 3;
            for (c, v) in rgb.iter().enumerate() {
                let d = img.data[i + c] as f32;
                img.data[i + c] = (d + (*v as f32 - d) * cov.min(1.0)).round() as u8;
            }
        });
    }
}

// ------------------------------------------------------------------ rendering

/// Everything [`render_collage`] draws, resolved.
pub struct CollageJob<'a> {
    pub cells: &'a [Cell],
    pub width: u32,
    pub height: u32,
    pub background: [u8; 3],
    pub fit: CollageFit,
    /// Middle of the faces of a photo (fractions of the picture) for the cover crop.
    pub focus: &'a HashMap<i64, (f64, f64)>,
    /// One caption per cell (empty strings draw nothing); only used where a cell has a band.
    pub captions: &'a [String],
    pub cancel: Option<&'a CancelToken>,
}

/// Renders a photo for a `cw` x `ch` cell (edits applied when `edits` has a stack for it).
fn render_for_cell(
    imaging: &dyn Imaging,
    r: &PhotoRef,
    edits: Option<&EditedExport>,
    cw: u32,
    ch: u32,
) -> std::result::Result<(RgbImage, bool), String> {
    let aspect = match (r.width, r.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => w as f64 / h as f64,
        _ => DEFAULT_ASPECT,
    };
    let edge = (cw.max(ch) as f64 * aspect.max(1.0 / aspect))
        .ceil()
        .clamp(64.0, MAX_RENDER_EDGE as f64) as u32;
    if let Some((e, stack)) = edits.and_then(|e| e.stacks.get(&r.id).map(|s| (e, s))) {
        let img = e
            .svc
            .render_gated(r, stack, Some(edge), MaskMode::Strict, &e.handle)
            .map_err(|e| e.to_string())?
            .image;
        return Ok((img, true));
    }
    let (width, height, data) = imaging
        .decode_rgb8(&r.path, r.format, r.orientation, edge)
        .map_err(|e| format!("{e:#}"))?;
    Ok((
        RgbImage {
            width,
            height,
            data,
        },
        false,
    ))
}

/// Error text of a collage that was cancelled.
pub const CANCELLED: &str = "cancelled";

/// Renders the collage canvas. Photos are rendered in parallel; `progress(done)` is called
/// after each one. Once the token is cancelled the remaining photos are skipped and the result
/// is `Err("cancelled")`. A photo that cannot be read fails the collage (`file: reason`).
pub fn render_collage(
    imaging: &dyn Imaging,
    refs: &[PhotoRef],
    edits: Option<&EditedExport>,
    job: &CollageJob,
    progress: &(dyn Fn(usize) + Sync),
) -> std::result::Result<RgbImage, String> {
    assert_eq!(refs.len(), job.cells.len());
    let cancelled = || job.cancel.is_some_and(CancelToken::is_cancelled);
    let done = AtomicUsize::new(0);
    let tiles: Vec<std::result::Result<(RgbImage, u32, u32), String>> = refs
        .par_iter()
        .zip(job.cells.par_iter())
        .map(|(r, cell)| {
            if cancelled() {
                return Err(CANCELLED.to_string());
            }
            let (cw, ch) = (cell.photo.w, cell.photo.h);
            let (img, edited) = render_for_cell(imaging, r, edits, cw, ch)
                .map_err(|e| format!("{}: {e}", r.file_name))?;
            // face boxes belong to the stored picture: an edit may have cropped or rotated it
            let focus = if edited {
                None
            } else {
                job.focus.get(&r.id).copied()
            };
            let tile = fit_into(&img, cw, ch, job.fit, focus);
            progress(done.fetch_add(1, Ordering::SeqCst) + 1);
            Ok(tile)
        })
        .collect();
    if cancelled() {
        return Err(CANCELLED.to_string());
    }
    let mut canvas = RgbImage {
        width: job.width,
        height: job.height,
        data: job
            .background
            .repeat(job.width as usize * job.height as usize),
    };
    for (cell, tile) in job.cells.iter().zip(tiles) {
        let (img, dx, dy) = tile?;
        let (x0, y0) = (cell.photo.x + dx, cell.photo.y + dy);
        for row in 0..img.height {
            let src = (row * img.width * 3) as usize;
            let dst = (((y0 + row) as usize * job.width as usize) + x0 as usize) * 3;
            let len = img.width as usize * 3;
            canvas.data[dst..dst + len].copy_from_slice(&img.data[src..src + len]);
        }
    }
    if job.cells.iter().any(|c| c.caption.is_some()) {
        let font = FontArc::try_from_slice(DEFAULT_FONT).map_err(|e| e.to_string())?;
        let lum = 0.299 * job.background[0] as f32
            + 0.587 * job.background[1] as f32
            + 0.114 * job.background[2] as f32;
        let ink = if lum > 140.0 {
            [30, 30, 30]
        } else {
            [235, 235, 235]
        };
        for (cell, text) in job.cells.iter().zip(job.captions) {
            if let (Some(rect), false) = (cell.caption, text.is_empty()) {
                draw_caption(&mut canvas, &font, text, rect, ink);
            }
        }
    }
    Ok(canvas)
}

/// The caption of a contact-sheet cell: file name and/or stars.
pub fn caption_text(file_name: &str, rating: Option<i64>, name: bool, stars: bool) -> String {
    let mut parts = Vec::new();
    if name {
        parts.push(file_name.to_string());
    }
    if let (true, Some(r @ 1..=5)) = (stars, rating) {
        parts.push("★".repeat(r as usize));
    }
    parts.join("  ")
}

/// Aspect ratio (display orientation) of a photo from the catalog.
fn aspect_of(r: &PhotoRef) -> f64 {
    match (r.width, r.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => w as f64 / h as f64,
        _ => DEFAULT_ASPECT,
    }
}

/// Middle of the union of the face boxes of each photo (stored pictures; boxes are fractions
/// of the displayed picture).
fn face_focus(c: &rusqlite::Connection, ids: &[i64]) -> Result<HashMap<i64, (f64, f64)>> {
    let ph = vec!["?"; ids.len()].join(",");
    let mut st = c.prepare(&format!(
        "SELECT photo_id, bbox_x, bbox_y, bbox_w, bbox_h FROM face
          WHERE bbox_x IS NOT NULL AND bbox_w > 0 AND bbox_h > 0 AND photo_id IN ({ph})"
    ))?;
    let rows = st.query_map(rusqlite::params_from_iter(ids.iter()), |r| {
        Ok((
            r.get::<_, i64>(0)?,
            [r.get::<_, f64>(1)?, r.get(2)?, r.get(3)?, r.get(4)?],
        ))
    })?;
    // union box per photo: (x0, y0, x1, y1)
    let mut boxes: HashMap<i64, (f64, f64, f64, f64)> = HashMap::new();
    for row in rows {
        let (id, [x, y, w, h]) = row?;
        let e = boxes.entry(id).or_insert((x, y, x + w, y + h));
        *e = (e.0.min(x), e.1.min(y), e.2.max(x + w), e.3.max(y + h));
    }
    Ok(boxes
        .into_iter()
        .map(|(id, b)| (id, ((b.0 + b.2) / 2.0, (b.1 + b.3) / 2.0)))
        .collect())
}

/// Picks a file name for the collage (never an existing one) inside `dest`.
fn plan_output(
    first: &PhotoRef,
    template: &str,
    dest: &Path,
    format: ExportFormat,
) -> std::path::PathBuf {
    export::plan_names_with(
        std::slice::from_ref(first),
        template,
        dest,
        &|_| format.extension().to_string(),
        &HashMap::new(),
    )
    .remove(0)
}

impl Core {
    /// Validates and starts a collage export; progress arrives as `task.progress` events of
    /// kind `export` (one step per photo). Returns the task id.
    pub async fn export_collage(self: &Arc<Self>, req: CollageRequest) -> Result<String> {
        req.validate()?;
        let background = parse_color(&req.background).expect("validated");
        let watermark = match req.watermark.clone() {
            Some(spec) => Some(Arc::new(
                tokio::task::spawn_blocking(move || PreparedWatermark::new(spec))
                    .await
                    .map_err(|e| CoreError::Internal(anyhow::anyhow!("watermark loader: {e}")))?
                    .map_err(CoreError::bad_request)?,
            )),
            None => None,
        };
        let ids = req.ids.clone();
        let want_faces = req.fit == CollageFit::Cover;
        let (refs, focus, info) = self
            .db
            .call(move |c| {
                let members =
                    crate::files::export_members(c, &ids, crate::model::PairMember::Jpeg)?;
                let refs = catalog::photo_refs(c, &members)?;
                let found: Vec<i64> = refs.iter().map(|r| r.id).collect();
                let focus = if want_faces && !found.is_empty() {
                    face_focus(c, &found)?
                } else {
                    HashMap::new()
                };
                let info = export::name_info(c, &found)?;
                Ok((refs, focus, info))
            })
            .await?;
        if refs.len() < MIN_PHOTOS {
            return Err(CoreError::not_found(
                "fewer than two of the given photos exist",
            ));
        }
        let n = refs.len();
        let captions_on =
            req.layout == CollageLayout::Contact && (req.caption_name || req.caption_rating);
        let caption_h = if captions_on {
            caption_height(req.width, req.height)
        } else {
            0
        };
        let aspects: Vec<f64> = refs.iter().map(aspect_of).collect();
        let cells = compute_layout(
            req.layout, req.width, req.height, req.gap, req.margin, caption_h, &aspects,
        )
        .map_err(CoreError::bad_request)?;
        let dest = std::path::PathBuf::from(&req.dest);
        std::fs::create_dir_all(&dest)
            .map_err(|e| CoreError::bad_request(format!("cannot create destination: {e}")))?;
        let out = plan_output(&refs[0], &req.name_template, &dest, req.format);
        let edits = if req.apply_edits {
            self.edited_export(&refs).await?
        } else {
            None
        };
        let captions: Vec<String> = if captions_on {
            refs.iter()
                .map(|r| {
                    caption_text(
                        &r.file_name,
                        info.get(&r.id).and_then(|i| i.rating),
                        req.caption_name,
                        req.caption_rating,
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        let wm_tokens = export::watermark_tokens(&refs[0], info.get(&refs[0].id));

        let task_id = format!("export-{}", self.next_task_seq());
        let cancel = CancelToken::new();
        {
            let tid = task_id.clone();
            let spec = serde_json::json!({
                "dest": req.dest,
                "count": n,
                "collage": req.layout,
                "width": req.width,
                "height": req.height,
            });
            self.db
                .call(move |c| crate::tasks::insert(c, &tid, "export", &spec, n as i64))
                .await?;
            self.tasks.register(&task_id, cancel.clone(), n as i64);
        }
        let core = self.clone();
        let tid = task_id.clone();
        tokio::spawn(async move {
            let ev = |done: usize, state: &str, error: Option<String>| Event::TaskProgress {
                task_id: tid.clone(),
                kind: "export".into(),
                done: done as i64,
                total: n as i64,
                state: state.into(),
                error,
            };
            core.events.emit(ev(0, "running", None));
            let (imaging, core2, tid2, cancel2) = (
                core.imaging.clone(),
                core.clone(),
                tid.clone(),
                cancel.clone(),
            );
            let res = tokio::task::spawn_blocking(move || -> std::result::Result<(), String> {
                let job = CollageJob {
                    cells: &cells,
                    width: req.width,
                    height: req.height,
                    background,
                    fit: req.fit,
                    focus: &focus,
                    captions: &captions,
                    cancel: Some(&cancel2),
                };
                let mut canvas = render_collage(&*imaging, &refs, edits.as_ref(), &job, &|done| {
                    core2.tasks.progress(&tid2, done as i64, n as i64);
                    core2.events.emit(Event::TaskProgress {
                        task_id: tid2.clone(),
                        kind: "export".into(),
                        done: done as i64,
                        total: n as i64,
                        state: "running".into(),
                        error: None,
                    });
                })?;
                if let Some(wm) = &watermark {
                    wm.apply(&mut canvas, &wm_tokens);
                }
                if cancel2.is_cancelled() {
                    return Err(CANCELLED.into());
                }
                let bytes =
                    formats::encode(&canvas, req.format, req.quality, &PreparedMeta::default())?;
                export::write_new(&out, &bytes).map_err(|e| e.to_string())
            })
            .await;
            let (state, error, done) = match res {
                Ok(Ok(())) => ("done", None, n),
                Ok(Err(e)) if e == CANCELLED => (crate::tasks::CANCELLED, None, 0),
                Ok(Err(e)) => ("failed", Some(e), 0),
                Err(e) => ("failed", Some(format!("collage task crashed: {e}")), 0),
            };
            let (tid3, st, er) = (tid.clone(), state.to_string(), error.clone());
            let _ = core
                .db
                .call(move |c| {
                    crate::tasks::finish(c, &tid3, &st, done as i64, n as i64, er.as_deref())
                })
                .await;
            core.tasks.remove(&tid);
            core.events.emit(ev(done, state, error));
        });
        Ok(task_id)
    }
}

#[cfg(test)]
#[path = "collage_tests.rs"]
mod tests;
