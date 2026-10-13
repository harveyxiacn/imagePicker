//! Export watermark (PRD F9.4, `docs/api-contract-m5.md` section F): a text or a PNG logo
//! blended into the finished pixels of an export, after resizing and colour conversion and
//! before the encoder, so every container carries it the same way.
//!
//! Text is rasterised in pure Rust with `ab_glyph`. The bundled font is DejaVu Sans (Bitstream
//! Vera licence, `assets/fonts/LICENSE-DejaVu.txt`); it covers Latin, Greek, Cyrillic and `(c)`
//! but not CJK. A request whose literal text has characters the font lacks is refused with a
//! hint to pick a font file (`font_path`, TTF / OTF) instead of printing empty boxes.

use ab_glyph::{point, Font, FontArc, GlyphId, PxScale, ScaleFont};
use ip_render::RgbImage;
use serde::{Deserialize, Serialize};

/// The font used when a text watermark names none.
const DEFAULT_FONT: &[u8] = include_bytes!("../assets/fonts/DejaVuSans.ttf");

/// Longest watermark text (characters, before token expansion).
pub const MAX_TEXT_CHARS: usize = 200;
/// Largest logo / font file read.
const MAX_LOGO_BYTES: u64 = 32 << 20;
const MAX_FONT_BYTES: u64 = 32 << 20;
/// Logos with a side beyond this are refused.
const MAX_LOGO_SIDE: u32 = 8192;
/// Tokens a text watermark expands (`{camera}` ...).
pub const TOKENS: [&str; 7] = [
    "{name}", "{date}", "{year}", "{camera}", "{event}", "{scene}", "{rating}",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WatermarkKind {
    #[default]
    Text,
    Image,
}

/// One cell of the 3x3 grid the mark is anchored in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WatermarkPosition {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    #[default]
    BottomRight,
}

impl WatermarkPosition {
    /// `(column, row)`, each 0..=2.
    fn cell(self) -> (u8, u8) {
        match self {
            Self::TopLeft => (0, 0),
            Self::Top => (1, 0),
            Self::TopRight => (2, 0),
            Self::Left => (0, 1),
            Self::Center => (1, 1),
            Self::Right => (2, 1),
            Self::BottomLeft => (0, 2),
            Self::Bottom => (1, 2),
            Self::BottomRight => (2, 2),
        }
    }
}

/// The `watermark` object of an export request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Watermark {
    pub kind: WatermarkKind,
    /// `text`: the mark, with the tokens of [`TOKENS`]; one line.
    #[serde(default)]
    pub text: String,
    /// `image`: path of the logo (PNG with alpha, any format `image` decodes).
    #[serde(default)]
    pub image_path: Option<String>,
    /// `text`: a TTF / OTF file instead of the bundled font (needed for CJK).
    #[serde(default)]
    pub font_path: Option<String>,
    #[serde(default)]
    pub position: WatermarkPosition,
    /// Distance from the edges, percent of the long edge (0..=40).
    #[serde(default = "d_margin")]
    pub margin_pct: f32,
    /// `image`: width of the logo, percent of the long edge (1..=100).
    #[serde(default = "d_size")]
    pub size_pct: f32,
    /// `text`: font size, percent of the long edge (0.5..=50).
    #[serde(default = "d_font_size")]
    pub font_size_pct: f32,
    /// 0 (invisible) ..= 1 (solid).
    #[serde(default = "d_opacity")]
    pub opacity: f32,
    /// `text`: `#rrggbb` or `#rgb`.
    #[serde(default = "d_color")]
    pub color: String,
    /// `text`: a dark drop shadow behind the letters.
    #[serde(default = "d_shadow")]
    pub shadow: bool,
}

fn d_margin() -> f32 {
    2.0
}
fn d_size() -> f32 {
    20.0
}
fn d_font_size() -> f32 {
    3.0
}
fn d_opacity() -> f32 {
    0.6
}
fn d_color() -> String {
    "#ffffff".into()
}
fn d_shadow() -> bool {
    true
}

impl Default for Watermark {
    fn default() -> Self {
        Watermark {
            kind: WatermarkKind::Text,
            text: String::new(),
            image_path: None,
            font_path: None,
            position: WatermarkPosition::default(),
            margin_pct: d_margin(),
            size_pct: d_size(),
            font_size_pct: d_font_size(),
            opacity: d_opacity(),
            color: d_color(),
            shadow: d_shadow(),
        }
    }
}

/// Per-photo values of the text tokens (empty when unknown).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WatermarkTokens {
    pub name: String,
    /// `YYYY-MM-DD`
    pub date: String,
    pub year: String,
    pub camera: String,
    pub event: String,
    pub scene: String,
    pub rating: String,
}

/// Expands the tokens of `text` in one pass (values are never expanded again); unknown
/// `{words}` stay as typed.
pub fn expand_tokens(text: &str, t: &WatermarkTokens) -> String {
    let mut out = String::with_capacity(text.len() + 32);
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let tail = &rest[open..];
        let Some(close) = tail.find('}') else {
            out.push_str(tail);
            return out;
        };
        let value = match &tail[1..close] {
            "name" => Some(&t.name),
            "date" => Some(&t.date),
            "year" => Some(&t.year),
            "camera" => Some(&t.camera),
            "event" => Some(&t.event),
            "scene" => Some(&t.scene),
            "rating" => Some(&t.rating),
            _ => None,
        };
        match value {
            Some(v) => {
                out.push_str(v);
                rest = &tail[close + 1..];
            }
            None => {
                out.push('{');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Whether `text` uses any catalog-backed token (`{camera}` `{event}` `{scene}` `{rating}`).
pub fn uses_catalog_tokens(text: &str) -> bool {
    ["{camera}", "{event}", "{scene}", "{rating}"]
        .iter()
        .any(|t| text.contains(t))
}

/// The pixel rectangle a mark was drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// A validated watermark with its font / logo loaded once per export.
pub struct PreparedWatermark {
    spec: Watermark,
    color: [u8; 3],
    font: Option<FontArc>,
    logo: Option<image::RgbaImage>,
}

impl std::fmt::Debug for PreparedWatermark {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedWatermark")
            .field("spec", &self.spec)
            .finish()
    }
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

impl PreparedWatermark {
    /// Checks the settings and loads the logo or font; the error is a message for the client
    /// (HTTP 400).
    pub fn new(spec: Watermark) -> Result<Self, String> {
        let in_range = |v: f32, lo: f32, hi: f32| v.is_finite() && (lo..=hi).contains(&v);
        if !in_range(spec.opacity, 0.0, 1.0) {
            return Err("watermark.opacity must be 0..1".into());
        }
        if !in_range(spec.margin_pct, 0.0, 40.0) {
            return Err("watermark.margin_pct must be 0..40".into());
        }
        let color = parse_color(&spec.color)
            .ok_or_else(|| "watermark.color must be #rrggbb or #rgb".to_string())?;
        let (mut font, mut logo) = (None, None);
        match spec.kind {
            WatermarkKind::Text => {
                if !in_range(spec.font_size_pct, 0.5, 50.0) {
                    return Err("watermark.font_size_pct must be 0.5..50".into());
                }
                let text = spec.text.trim();
                if text.is_empty() {
                    return Err("watermark.text must not be empty".into());
                }
                if text.chars().count() > MAX_TEXT_CHARS {
                    return Err(format!(
                        "watermark.text is longer than {MAX_TEXT_CHARS} characters"
                    ));
                }
                let f = match spec.font_path.as_deref().filter(|p| !p.trim().is_empty()) {
                    Some(p) => load_font(p)?,
                    None => FontArc::try_from_slice(DEFAULT_FONT)
                        .map_err(|e| format!("bundled font: {e}"))?,
                };
                // the literal part of the text (tokens removed) must be drawable
                let literal = expand_tokens(text, &WatermarkTokens::default());
                let missing: String = literal
                    .chars()
                    .filter(|c| !c.is_whitespace() && !c.is_control())
                    .filter(|c| f.glyph_id(*c) == GlyphId(0))
                    .collect();
                if !missing.is_empty() {
                    let mut shown: Vec<char> = Vec::new();
                    for c in missing.chars() {
                        if !shown.contains(&c) && shown.len() < 8 {
                            shown.push(c);
                        }
                    }
                    return Err(format!(
                        "the watermark font has no glyphs for {}; choose a font file (watermark.font_path) that has them",
                        shown.into_iter().collect::<String>()
                    ));
                }
                font = Some(f);
            }
            WatermarkKind::Image => {
                if !in_range(spec.size_pct, 1.0, 100.0) {
                    return Err("watermark.size_pct must be 1..100".into());
                }
                let p = spec
                    .image_path
                    .as_deref()
                    .filter(|p| !p.trim().is_empty())
                    .ok_or_else(|| "watermark.image_path is required for an image".to_string())?;
                logo = Some(load_logo(p)?);
            }
        }
        Ok(PreparedWatermark {
            spec,
            color,
            font,
            logo,
        })
    }

    pub fn spec(&self) -> &Watermark {
        &self.spec
    }

    /// Blends the mark into `img`; returns the rectangle it was drawn in (`None`: nothing to
    /// draw, e.g. the text expanded to nothing).
    pub fn apply(&self, img: &mut RgbImage, tokens: &WatermarkTokens) -> Option<Rect> {
        if img.width == 0 || img.height == 0 || self.spec.opacity <= 0.0 {
            return None;
        }
        match (&self.logo, &self.font) {
            (Some(logo), _) => self.apply_logo(img, logo),
            (_, Some(font)) => self.apply_text(img, font, tokens),
            _ => None,
        }
    }

    /// Top-left corner of a `w x h` box in the grid cell, margin from the edges, kept inside.
    fn place(&self, iw: u32, ih: u32, w: u32, h: u32) -> (u32, u32) {
        let margin = (iw.max(ih) as f32 * self.spec.margin_pct / 100.0).round() as i64;
        let (col, row) = self.spec.position.cell();
        let at = |cell: u8, size: i64, boxed: i64| match cell {
            0 => margin,
            1 => (size - boxed) / 2,
            _ => size - boxed - margin,
        };
        let x = at(col, iw as i64, w as i64).clamp(0, (iw.saturating_sub(w)) as i64);
        let y = at(row, ih as i64, h as i64).clamp(0, (ih.saturating_sub(h)) as i64);
        (x as u32, y as u32)
    }

    fn apply_logo(&self, img: &mut RgbImage, logo: &image::RgbaImage) -> Option<Rect> {
        let long = img.width.max(img.height) as f32;
        let mut w = (long * self.spec.size_pct / 100.0).round().max(1.0) as u32;
        let mut h = ((w as f32) * logo.height() as f32 / logo.width() as f32)
            .round()
            .max(1.0) as u32;
        if w > img.width || h > img.height {
            let k = (img.width as f32 / w as f32).min(img.height as f32 / h as f32);
            w = ((w as f32 * k) as u32).clamp(1, img.width);
            h = ((h as f32 * k) as u32).clamp(1, img.height);
        }
        let scaled;
        let logo = if (w, h) == logo.dimensions() {
            logo
        } else {
            scaled = image::imageops::resize(logo, w, h, image::imageops::FilterType::Lanczos3);
            &scaled
        };
        let (x0, y0) = self.place(img.width, img.height, w, h);
        for (lx, ly, px) in logo.enumerate_pixels() {
            let a = px[3] as f32 / 255.0 * self.spec.opacity;
            blend(img, x0 + lx, y0 + ly, [px[0], px[1], px[2]], a);
        }
        Some(Rect { x: x0, y: y0, w, h })
    }

    fn apply_text(
        &self,
        img: &mut RgbImage,
        font: &FontArc,
        tokens: &WatermarkTokens,
    ) -> Option<Rect> {
        let text: String = expand_tokens(&self.spec.text, tokens)
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let long = img.width.max(img.height) as f32;
        let mut px = (long * self.spec.font_size_pct / 100.0).max(8.0);
        let mut lay = layout(font, text, px, self.spec.shadow);
        if lay.width > img.width || lay.height > img.height {
            // too big for the picture: shrink to fit
            let k = (img.width as f32 / lay.width as f32).min(img.height as f32 / lay.height as f32);
            px = (px * k * 0.98).max(4.0);
            lay = layout(font, text, px, self.spec.shadow);
        }
        let (w, h) = (lay.width.min(img.width), lay.height.min(img.height));
        let (x0, y0) = self.place(img.width, img.height, w, h);
        let rect = Rect { x: x0, y: y0, w, h };
        let scale = PxScale::from(px);
        let mut pass = |dx: f32, dy: f32, rgb: [u8; 3], alpha: f32| {
            for (id, gx) in &lay.glyphs {
                let g = id.with_scale_and_position(
                    scale,
                    point(x0 as f32 + gx + dx, y0 as f32 + lay.ascent + dy),
                );
                let Some(o) = font.outline_glyph(g) else {
                    continue;
                };
                let b = o.px_bounds();
                o.draw(|ix, iy, cov| {
                    let (x, y) = (b.min.x as i64 + ix as i64, b.min.y as i64 + iy as i64);
                    // stay inside the box the mark was placed in
                    if x < rect.x as i64
                        || y < rect.y as i64
                        || x >= (rect.x + rect.w) as i64
                        || y >= (rect.y + rect.h) as i64
                    {
                        return;
                    }
                    blend(img, x as u32, y as u32, rgb, cov * alpha);
                });
            }
        };
        if self.spec.shadow {
            pass(lay.shadow, lay.shadow, [0, 0, 0], self.spec.opacity * 0.7);
        }
        pass(0.0, 0.0, self.color, self.spec.opacity);
        Some(rect)
    }
}

struct Layout {
    glyphs: Vec<(GlyphId, f32)>,
    ascent: f32,
    /// Shadow offset in pixels (0 without shadow).
    shadow: f32,
    width: u32,
    height: u32,
}

fn layout(font: &FontArc, text: &str, px: f32, shadow: bool) -> Layout {
    let sf = font.as_scaled(PxScale::from(px));
    let mut glyphs = Vec::with_capacity(text.len());
    let (mut x, mut prev) = (0.0f32, None::<GlyphId>);
    for ch in text.chars() {
        let id = sf.glyph_id(ch);
        if let Some(p) = prev {
            x += sf.kern(p, id);
        }
        glyphs.push((id, x));
        x += sf.h_advance(id);
        prev = Some(id);
    }
    let off = if shadow { (px / 16.0).round().max(1.0) } else { 0.0 };
    Layout {
        glyphs,
        ascent: sf.ascent(),
        shadow: off,
        width: (x + off).ceil().max(1.0) as u32,
        height: (sf.height() + off).ceil().max(1.0) as u32,
    }
}

/// `dst += (rgb - dst) * a`
fn blend(img: &mut RgbImage, x: u32, y: u32, rgb: [u8; 3], a: f32) {
    if a <= 0.0 || x >= img.width || y >= img.height {
        return;
    }
    let a = a.min(1.0);
    let i = ((y as usize * img.width as usize) + x as usize) * 3;
    for (c, v) in rgb.iter().enumerate() {
        let d = img.data[i + c] as f32;
        img.data[i + c] = (d + (*v as f32 - d) * a).round().clamp(0.0, 255.0) as u8;
    }
}

fn load_font(path: &str) -> Result<FontArc, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("watermark font {path}: {e}"))?;
    if !meta.is_file() || meta.len() > MAX_FONT_BYTES {
        return Err(format!("watermark font {path} is not a usable font file"));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("watermark font {path}: {e}"))?;
    FontArc::try_from_vec(bytes)
        .map_err(|_| format!("watermark font {path} is not a TTF / OTF font (collections are not supported)"))
}

fn load_logo(path: &str) -> Result<image::RgbaImage, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("watermark image {path}: {e}"))?;
    if !meta.is_file() || meta.len() > MAX_LOGO_BYTES {
        return Err(format!("watermark image {path} is not a usable image file"));
    }
    let img = image::open(path)
        .map_err(|e| format!("watermark image {path} cannot be read: {e}"))?
        .to_rgba8();
    if img.width() > MAX_LOGO_SIDE || img.height() > MAX_LOGO_SIDE {
        return Err(format!(
            "watermark image {path} is larger than {MAX_LOGO_SIDE} px"
        ));
    }
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: u32, h: u32, v: u8) -> RgbImage {
        RgbImage {
            width: w,
            height: h,
            data: vec![v; (w * h * 3) as usize],
        }
    }

    fn text_mark(text: &str) -> Watermark {
        Watermark {
            text: text.into(),
            opacity: 1.0,
            shadow: false,
            ..Default::default()
        }
    }

    /// Pixels that differ between `a` and `b`, as `(x, y)`.
    fn changed(a: &RgbImage, b: &RgbImage) -> Vec<(u32, u32)> {
        let mut v = Vec::new();
        for y in 0..a.height {
            for x in 0..a.width {
                let i = ((y * a.width + x) * 3) as usize;
                if a.data[i..i + 3] != b.data[i..i + 3] {
                    v.push((x, y));
                }
            }
        }
        v
    }

    fn px(img: &RgbImage, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * img.width + x) * 3) as usize;
        [img.data[i], img.data[i + 1], img.data[i + 2]]
    }

    fn logo_file(dir: &std::path::Path, w: u32, h: u32, rgba: [u8; 4]) -> String {
        let p = dir.join("logo.png");
        image::RgbaImage::from_pixel(w, h, image::Rgba(rgba))
            .save(&p)
            .unwrap();
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn text_changes_pixels_only_inside_its_box_in_the_corner() {
        let wm = PreparedWatermark::new(text_mark("(c) Test 2024")).unwrap();
        let orig = flat(600, 400, 90);
        let mut img = orig.clone();
        let r = wm.apply(&mut img, &WatermarkTokens::default()).unwrap();
        let diff = changed(&orig, &img);
        assert!(diff.len() > 50, "text was drawn: {}", diff.len());
        for (x, y) in &diff {
            assert!(
                *x >= r.x && *x < r.x + r.w && *y >= r.y && *y < r.y + r.h,
                "{x},{y} outside {r:?}"
            );
        }
        // bottom-right with a 2 % margin of the long edge (12 px)
        assert_eq!((r.x + r.w, r.y + r.h), (600 - 12, 400 - 12));
        assert!(r.x > 300 && r.y > 200, "{r:?}");
    }

    #[test]
    fn opacity_scales_the_blend() {
        let mut spec = text_mark("MMMM");
        spec.font_size_pct = 20.0;
        let full = |opacity: f32| {
            let mut s = spec.clone();
            s.opacity = opacity;
            let wm = PreparedWatermark::new(s).unwrap();
            let mut img = flat(300, 200, 0);
            wm.apply(&mut img, &WatermarkTokens::default());
            img.data.iter().copied().max().unwrap()
        };
        assert_eq!(full(1.0), 255, "solid white");
        let half = full(0.5);
        assert!((126..=129).contains(&half), "half: {half}");
        assert_eq!(full(0.0), 0, "invisible");
    }

    #[test]
    fn shadow_adds_dark_pixels_beside_the_letters() {
        let mut spec = text_mark("MMM");
        spec.font_size_pct = 20.0;
        let draw = |shadow: bool| {
            let mut s = spec.clone();
            s.shadow = shadow;
            let wm = PreparedWatermark::new(s).unwrap();
            let mut img = flat(300, 200, 200);
            wm.apply(&mut img, &WatermarkTokens::default());
            img
        };
        let (plain, shadowed) = (draw(false), draw(true));
        let darker = |img: &RgbImage| img.data.iter().filter(|v| **v < 200).count();
        assert_eq!(darker(&plain), 0, "white text on light grey has no dark pixel");
        assert!(darker(&shadowed) > 0);
    }

    #[test]
    fn text_colour_is_used() {
        let mut spec = text_mark("MMMM");
        spec.font_size_pct = 20.0;
        spec.color = "#f00".into();
        let wm = PreparedWatermark::new(spec).unwrap();
        let mut img = flat(300, 200, 0);
        wm.apply(&mut img, &WatermarkTokens::default());
        assert!(img.data.chunks(3).any(|p| p == [255, 0, 0]));
        assert!(img.data.chunks(3).all(|p| p[1] == 0 && p[2] == 0));
    }

    #[test]
    fn tokens_expand_once_and_unknown_ones_stay() {
        let t = WatermarkTokens {
            camera: "Canon {date}".into(),
            date: "2024-02-29".into(),
            year: "2024".into(),
            ..Default::default()
        };
        assert_eq!(
            expand_tokens("(c) {year} {camera} {date} {foo} {", &t),
            "(c) 2024 Canon {date} 2024-02-29 {foo} {"
        );
        assert!(uses_catalog_tokens("x {camera}"));
        assert!(!uses_catalog_tokens("{date} {name}"));
    }

    #[test]
    fn text_with_empty_tokens_draws_nothing() {
        let wm = PreparedWatermark::new(text_mark("{camera}")).unwrap();
        let orig = flat(100, 100, 5);
        let mut img = orig.clone();
        assert_eq!(wm.apply(&mut img, &WatermarkTokens::default()), None);
        assert_eq!(img.data, orig.data);
    }

    #[test]
    fn logo_lands_exactly_in_its_grid_cell() {
        let dir = tempfile::tempdir().unwrap();
        let path = logo_file(dir.path(), 20, 10, [255, 0, 0, 255]);
        let base = Watermark {
            kind: WatermarkKind::Image,
            image_path: Some(path),
            opacity: 1.0,
            size_pct: 10.0,
            margin_pct: 5.0,
            ..Default::default()
        };
        // 400x300: logo 40x20, margin 20
        let cases = [
            (WatermarkPosition::TopLeft, (20, 20)),
            (WatermarkPosition::Top, (180, 20)),
            (WatermarkPosition::TopRight, (340, 20)),
            (WatermarkPosition::Left, (20, 140)),
            (WatermarkPosition::Center, (180, 140)),
            (WatermarkPosition::Right, (340, 140)),
            (WatermarkPosition::BottomLeft, (20, 260)),
            (WatermarkPosition::Bottom, (180, 260)),
            (WatermarkPosition::BottomRight, (340, 260)),
        ];
        for (pos, (x, y)) in cases {
            let wm = PreparedWatermark::new(Watermark {
                position: pos,
                ..base.clone()
            })
            .unwrap();
            let orig = flat(400, 300, 10);
            let mut img = orig.clone();
            let r = wm.apply(&mut img, &WatermarkTokens::default()).unwrap();
            assert_eq!(r, Rect { x, y, w: 40, h: 20 }, "{pos:?}");
            let diff = changed(&orig, &img);
            assert_eq!(diff.len(), 40 * 20, "{pos:?}: only the logo's pixels");
            assert!(diff
                .iter()
                .all(|(px_, py)| (x..x + 40).contains(px_) && (y..y + 20).contains(py)));
            let c = px(&img, x, y);
            assert!(c[0] >= 250 && c[1] <= 5 && c[2] <= 5, "{c:?}");
        }
    }

    #[test]
    fn logo_alpha_and_opacity_multiply() {
        let dir = tempfile::tempdir().unwrap();
        let path = logo_file(dir.path(), 10, 10, [200, 200, 200, 128]);
        let wm = PreparedWatermark::new(Watermark {
            kind: WatermarkKind::Image,
            image_path: Some(path),
            opacity: 0.5,
            size_pct: 10.0,
            position: WatermarkPosition::TopLeft,
            margin_pct: 0.0,
            ..Default::default()
        })
        .unwrap();
        let mut img = flat(100, 100, 0);
        let r = wm.apply(&mut img, &WatermarkTokens::default()).unwrap();
        assert_eq!(r, Rect { x: 0, y: 0, w: 10, h: 10 });
        // 200 * (128/255 * 0.5) = 50.2
        let v = px(&img, 3, 3)[0];
        assert!((49..=51).contains(&v), "{v}");
        assert_eq!(px(&img, 50, 50), [0, 0, 0]);
    }

    #[test]
    fn oversized_logo_is_shrunk_into_the_picture() {
        let dir = tempfile::tempdir().unwrap();
        let path = logo_file(dir.path(), 400, 100, [0, 0, 255, 255]);
        let wm = PreparedWatermark::new(Watermark {
            kind: WatermarkKind::Image,
            image_path: Some(path),
            opacity: 1.0,
            size_pct: 100.0,
            ..Default::default()
        })
        .unwrap();
        let mut img = flat(80, 200, 0);
        let r = wm.apply(&mut img, &WatermarkTokens::default()).unwrap();
        assert!(r.x + r.w <= 80 && r.y + r.h <= 200, "{r:?}");
    }

    #[test]
    fn invalid_settings_are_refused() {
        let ok = text_mark("x");
        assert!(PreparedWatermark::new(ok.clone()).is_ok());
        let bad = |f: &dyn Fn(&mut Watermark)| {
            let mut w = ok.clone();
            f(&mut w);
            PreparedWatermark::new(w).unwrap_err()
        };
        assert!(bad(&|w| w.opacity = 1.5).contains("opacity"));
        assert!(bad(&|w| w.opacity = f32::NAN).contains("opacity"));
        assert!(bad(&|w| w.margin_pct = 41.0).contains("margin"));
        assert!(bad(&|w| w.color = "red".into()).contains("color"));
        assert!(bad(&|w| w.font_size_pct = 0.0).contains("font_size"));
        assert!(bad(&|w| w.text = "  ".into()).contains("empty"));
        assert!(bad(&|w| w.text = "x".repeat(MAX_TEXT_CHARS + 1)).contains("longer"));
        assert!(bad(&|w| w.font_path = Some("/no/such/font.ttf".into())).contains("font"));
        let img = |w: &mut Watermark| {
            w.kind = WatermarkKind::Image;
        };
        assert!(bad(&img).contains("image_path"));
        assert!(bad(&|w| {
            img(w);
            w.image_path = Some("/no/such/logo.png".into());
        })
        .contains("logo.png"));
        assert!(bad(&|w| {
            img(w);
            w.image_path = Some("/x".into());
            w.size_pct = 0.0;
        })
        .contains("size_pct"));
    }

    #[test]
    fn cjk_text_needs_a_font_file() {
        let err = PreparedWatermark::new(text_mark("摄影 水印")).unwrap_err();
        assert!(err.contains("font_path"), "{err}");
        // Latin, the copyright sign and tokens (even next to CJK values later) are fine
        PreparedWatermark::new(text_mark("© Müller {camera}")).unwrap();
    }

    #[test]
    fn json_shape() {
        let w: Watermark = serde_json::from_value(serde_json::json!({
            "kind": "text", "text": "x", "position": "top-left"
        }))
        .unwrap();
        assert_eq!(w.position, WatermarkPosition::TopLeft);
        assert_eq!((w.opacity, w.margin_pct, w.shadow), (0.6, 2.0, true));
        assert_eq!(
            serde_json::to_value(WatermarkPosition::BottomRight).unwrap(),
            "bottom-right"
        );
        assert!(serde_json::from_value::<Watermark>(serde_json::json!({"kind": "x"})).is_err());
    }
}
