//! Crop assistance (docs/03 §3.1.1 and §6.6): "Auto straighten" and "Auto crop".
//!
//! Both only compute a [`Crop`](crate::Crop) (`rect` + `angle`); nothing here touches pixels of
//! the render pipeline, so preview and export stay on the single geometry stage (`geom.rs`).
//!
//! Conventions (same as `Geo`): the rect is normalised on a canvas the size of the source,
//! rotated about the source centre by `angle` degrees. A positive angle turns the content
//! counter-clockwise, and `analysis.tilt_deg > 0` means the content is rotated clockwise, so
//! levelling a photo sets `angle = tilt_deg`.

use ip_lite::imgops::{resize_area, rgb_to_gray};

use crate::RgbImage;

/// |tilt| below this is "already level": nothing is changed (the estimator resolves ~0.1 deg).
pub const LEVEL_DEAD_ZONE_DEG: f64 = 0.15;
/// Minimum confidence of a stored or estimated tilt (`scoring::TILTED_MIN_CONFIDENCE`).
pub const MIN_TILT_CONFIDENCE: f64 = 0.2;
/// Largest straighten angle (the estimator searches +-15 deg; the crop op allows 45).
pub const MAX_STRAIGHTEN_DEG: f64 = 15.0;
/// Long edge the tilt estimator runs at (what the `quality` step uses).
const ANALYSIS_EDGE: usize = 1024;

/// Result of [`estimate_tilt`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TiltEstimate {
    /// Degrees, positive = content rotated clockwise.
    pub deg: f64,
    /// 0..1; 0 when no long straight structure was found.
    pub confidence: f64,
}

/// CPU horizon / level estimate of an upright RGB image: the `ip-lite` orientation-gated Hough
/// on a luma copy downscaled to a ~1024 px long edge (identical to the stored `analysis.tilt_deg`).
pub fn estimate_tilt(src: &RgbImage) -> TiltEstimate {
    let (w, h) = (src.width as usize, src.height as usize);
    if w == 0 || h == 0 || src.data.len() < w * h * 3 {
        return TiltEstimate::default();
    }
    let mut gray = rgb_to_gray(&src.data[..w * h * 3], w, h);
    let long = w.max(h);
    if long > ANALYSIS_EDGE {
        let k = ANALYSIS_EDGE as f64 / long as f64;
        let dw = ((w as f64 * k).round() as usize).clamp(1, w);
        let dh = ((h as f64 * k).round() as usize).clamp(1, h);
        gray = resize_area(&gray, dw, dh);
    }
    let t = ip_lite::estimate_tilt(&gray);
    TiltEstimate {
        deg: t.deg,
        confidence: t.confidence,
    }
}

/// Largest axis-aligned rect of pixel aspect `ratio` (w/h), centred, that stays inside a
/// `w x h` image rotated by `angle_deg` about its centre. Returned normalised `[x, y, w, h]`
/// on the (unrotated-size) canvas, i.e. directly usable as `Crop.rect`.
pub fn inscribed_rect(w: u32, h: u32, angle_deg: f64, ratio: f64) -> [f64; 4] {
    let (wf, hf) = (w.max(1) as f64, h.max(1) as f64);
    let ratio = if ratio.is_finite() && ratio > 0.0 {
        ratio.clamp(1e-3, 1e3)
    } else {
        wf / hf
    };
    let (s, c) = angle_deg.to_radians().sin_cos();
    let (s, c) = (s.abs(), c.abs());
    // Half extents (a, b) = (k ratio, k); corners (+-a, +-b) rotated by the angle must stay
    // within (+-w/2, +-h/2):  a c + b s <= w/2  and  a s + b c <= h/2.
    let k = ((wf / 2.0) / (ratio * c + s)).min((hf / 2.0) / (ratio * s + c));
    let (a, b) = (k * ratio, k);
    [0.5 - a / wf, 0.5 - b / hf, 2.0 * a / wf, 2.0 * b / hf]
}

/// Whether the crop `rect` (normalised, on the rotated canvas) lies inside both the canvas and
/// the source image rotated by `angle_deg`; `eps` is a tolerance in pixels.
pub fn rect_fits(w: u32, h: u32, angle_deg: f64, rect: [f64; 4], eps: f64) -> bool {
    let (wf, hf) = (w as f64, h as f64);
    let tol = eps / wf.min(hf).max(1.0);
    if rect[0] < -tol
        || rect[1] < -tol
        || rect[0] + rect[2] > 1.0 + tol
        || rect[1] + rect[3] > 1.0 + tol
    {
        return false;
    }
    let (s, c) = angle_deg.to_radians().sin_cos();
    for (fx, fy) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        let dx = (rect[0] + fx * rect[2] - 0.5) * wf;
        let dy = (rect[1] + fy * rect[3] - 0.5) * hf;
        let sx = dx * c - dy * s;
        let sy = dx * s + dy * c;
        if sx.abs() > wf / 2.0 + eps || sy.abs() > hf / 2.0 + eps {
            return false;
        }
    }
    true
}

fn rect_ratio(w: u32, h: u32, rect: [f64; 4]) -> f64 {
    let (pw, ph) = (rect[2] * w as f64, rect[3] * h as f64);
    if pw > 0.0 && ph > 0.0 {
        pw / ph
    } else {
        w as f64 / h.max(1) as f64
    }
}

/// Where a tilt came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TiltSource {
    /// `analysis.tilt_deg` from the quality step.
    Stored,
    /// Computed now by [`estimate_tilt`] (photo not analysed, or no confident stored value).
    Estimated,
}

/// Outcome of [`auto_straighten`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Straighten {
    /// Crop angle (degrees, rounded to 0.1 like the slider); 0 when the photo is level.
    pub angle: f64,
    /// Largest crop of the requested aspect inside the rotated image.
    pub rect: [f64; 4],
    pub tilt_deg: f64,
    pub confidence: f64,
    pub source: TiltSource,
    /// |tilt| is inside the dead zone: the photo counts as level.
    pub level: bool,
}

/// The tilt to correct: the stored `(deg, confidence)` when confident, else a fresh estimate;
/// `None` when neither is confident (no long straight structure, or beyond +-15 deg).
pub fn pick_tilt(src: &RgbImage, stored: Option<(f64, f64)>) -> Option<(f64, f64, TiltSource)> {
    let ok = |deg: f64, conf: f64| {
        deg.is_finite()
            && conf.is_finite()
            && conf >= MIN_TILT_CONFIDENCE
            && deg.abs() <= MAX_STRAIGHTEN_DEG
    };
    if let Some((deg, conf)) = stored {
        if ok(deg, conf) {
            return Some((deg, conf, TiltSource::Stored));
        }
    }
    let e = estimate_tilt(src);
    ok(e.deg, e.confidence).then_some((e.deg, e.confidence, TiltSource::Estimated))
}

/// One-click straighten: rotate by the photo's tilt and take the maximal axis-aligned crop of
/// aspect `ratio` (pixel w/h; `None` keeps the aspect of `cur_rect`) inside the rotated image.
pub fn auto_straighten(
    src: &RgbImage,
    stored: Option<(f64, f64)>,
    ratio: Option<f64>,
    cur_rect: [f64; 4],
) -> Option<Straighten> {
    let (deg, conf, source) = pick_tilt(src, stored)?;
    let level = deg.abs() < LEVEL_DEAD_ZONE_DEG;
    let angle = if level {
        0.0
    } else {
        (deg * 10.0).round() / 10.0 + 0.0
    };
    let ratio = ratio.unwrap_or_else(|| rect_ratio(src.width, src.height, cur_rect));
    Some(Straighten {
        angle,
        rect: inscribed_rect(src.width, src.height, angle, ratio),
        tilt_deg: deg,
        confidence: conf,
        source,
        level,
    })
}

// ------------------------------------------------------------------ smart crop

/// What the composition is built around.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubjectKind {
    Faces,
    /// No face boxes: colour / luminance contrast proxy.
    Saliency,
}

/// The region a crop should keep.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Subject {
    /// Normalised `[x, y, w, h]` on the unrotated source.
    pub bbox: [f64; 4],
    pub kind: SubjectKind,
}

impl Subject {
    fn centre(&self) -> (f64, f64) {
        (
            self.bbox[0] + self.bbox[2] / 2.0,
            self.bbox[1] + self.bbox[3] / 2.0,
        )
    }
}

const SAL_GRID: usize = 48;

/// Union of the (normalised) face boxes, else a saliency centroid box. Deterministic.
pub fn find_subject(src: &RgbImage, faces: &[[f32; 4]]) -> Subject {
    let mut u: Option<[f64; 4]> = None;
    for f in faces {
        let b = [f[0] as f64, f[1] as f64, f[2] as f64, f[3] as f64];
        if !b.iter().all(|v| v.is_finite()) || b[2] <= 0.0 || b[3] <= 0.0 {
            continue;
        }
        let (x0, y0) = (b[0].clamp(0.0, 1.0), b[1].clamp(0.0, 1.0));
        let (x1, y1) = ((b[0] + b[2]).clamp(0.0, 1.0), (b[1] + b[3]).clamp(0.0, 1.0));
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        u = Some(match u {
            None => [x0, y0, x1, y1],
            Some(a) => [a[0].min(x0), a[1].min(y0), a[2].max(x1), a[3].max(y1)],
        });
    }
    if let Some(a) = u {
        return Subject {
            bbox: [a[0], a[1], a[2] - a[0], a[3] - a[1]],
            kind: SubjectKind::Faces,
        };
    }
    Subject {
        bbox: saliency_box(src),
        kind: SubjectKind::Saliency,
    }
}

/// Centre-weighted global-contrast saliency on a 48-cell grid -> `[x, y, w, h]`.
fn saliency_box(src: &RgbImage) -> [f64; 4] {
    const CENTRE: [f64; 4] = [0.25, 0.25, 0.5, 0.5];
    let (w, h) = (src.width as usize, src.height as usize);
    if w == 0 || h == 0 || src.data.len() < w * h * 3 {
        return CENTRE;
    }
    let (gw, gh) = (SAL_GRID.min(w), SAL_GRID.min(h));
    // box-average into the grid (opponent colour space: luma, r-g, (r+g)/2-b)
    let mut sum = vec![[0.0f64; 3]; gw * gh];
    let mut cnt = vec![0.0f64; gw * gh];
    for y in 0..h {
        let gy = y * gh / h;
        for x in 0..w {
            let gx = x * gw / w;
            let i = (y * w + x) * 3;
            let (r, g, b) = (
                src.data[i] as f64,
                src.data[i + 1] as f64,
                src.data[i + 2] as f64,
            );
            let s = &mut sum[gy * gw + gx];
            s[0] += (r + g + b) / 3.0;
            s[1] += r - g;
            s[2] += (r + g) / 2.0 - b;
            cnt[gy * gw + gx] += 1.0;
        }
    }
    for (s, n) in sum.iter_mut().zip(&cnt) {
        for v in s.iter_mut() {
            *v /= n.max(1.0);
        }
    }
    let mut mean = [0.0f64; 3];
    for s in &sum {
        for k in 0..3 {
            mean[k] += s[k] / (gw * gh) as f64;
        }
    }
    let mut sal = Vec::with_capacity(gw * gh);
    let mut peak = 0.0f64;
    for s in &sum {
        let d =
            ((s[0] - mean[0]).powi(2) + (s[1] - mean[1]).powi(2) + (s[2] - mean[2]).powi(2)).sqrt();
        peak = peak.max(d);
        sal.push(d);
    }
    if peak < 6.0 {
        return CENTRE; // flat image: no subject to find
    }
    let (mut tw, mut mx, mut my) = (0.0, 0.0, 0.0);
    let mut pts = Vec::with_capacity(gw * gh);
    for gy in 0..gh {
        for gx in 0..gw {
            let (u, v) = ((gx as f64 + 0.5) / gw as f64, (gy as f64 + 0.5) / gh as f64);
            let centre =
                1.0 - 0.5 * (((u - 0.5) * 2.0).powi(2) + ((v - 0.5) * 2.0).powi(2)).min(1.0);
            let wt = (sal[gy * gw + gx] / peak).powi(2) * centre;
            tw += wt;
            mx += wt * u;
            my += wt * v;
            pts.push((u, v, wt));
        }
    }
    if tw <= 0.0 {
        return CENTRE;
    }
    let (cx, cy) = (mx / tw, my / tw);
    let (mut vx, mut vy) = (0.0, 0.0);
    for (u, v, wt) in pts {
        vx += wt * (u - cx).powi(2);
        vy += wt * (v - cy).powi(2);
    }
    let (sx, sy) = ((vx / tw).sqrt(), (vy / tw).sqrt());
    let (hw, hh) = ((1.5 * sx).clamp(0.06, 0.5), (1.5 * sy).clamp(0.06, 0.5));
    let (x0, y0) = ((cx - hw).max(0.0), (cy - hh).max(0.0));
    let (x1, y1) = ((cx + hw).min(1.0), (cy + hh).min(1.0));
    [x0, y0, x1 - x0, y1 - y0]
}

/// Smallest crop (fraction of the largest one) a composition may shrink to.
const MIN_SCALE: f64 = 0.7;
/// The subject should fill about this share of the crop's long relative side.
const SUBJECT_FILL: f64 = 0.55;
/// A subject wider / taller than this share of the crop is centred on that axis instead.
const CENTRE_SHARE: f64 = 0.45;
/// Margin kept around the subject box (normalised).
const SUBJECT_MARGIN: f64 = 0.01;

fn contains(rect: [f64; 4], b: [f64; 4], margin: f64) -> bool {
    b[0] - margin >= rect[0] - 1e-9
        && b[1] - margin >= rect[1] - 1e-9
        && b[0] + b[2] + margin <= rect[0] + rect[2] + 1e-9
        && b[1] + b[3] + margin <= rect[1] + rect[3] + 1e-9
}

/// Rule-of-thirds crop of aspect `ratio` (pixel w/h; `None` = the image's own aspect) at the
/// given straighten `angle`: the largest valid crop, tightened so the subject fills about half of
/// it (never below 70 % of the largest), slid so the subject sits on the nearest third-line
/// intersection (centred on an axis the subject spans), always inside the rotated image.
pub fn smart_crop(
    w: u32,
    h: u32,
    angle_deg: f64,
    ratio: Option<f64>,
    subject: &Subject,
) -> [f64; 4] {
    let ratio = ratio.unwrap_or(w as f64 / h.max(1) as f64);
    let full = inscribed_rect(w, h, angle_deg, ratio);
    let sb = subject.bbox;
    let fill = (sb[2] / full[2]).max(sb[3] / full[3]);
    let need =
        ((sb[2] + 2.0 * SUBJECT_MARGIN) / full[2]).max((sb[3] + 2.0 * SUBJECT_MARGIN) / full[3]);
    let f = (fill / SUBJECT_FILL)
        .max(MIN_SCALE)
        .max(need.min(1.0))
        .min(1.0);
    let (cw, ch) = (full[2] * f, full[3] * f);
    let centre_pos = (0.5 - cw / 2.0, 0.5 - ch / 2.0);
    let (sx, sy) = subject.centre();

    // Position nearest to `want` on the segment centre_pos -> want that is still a valid crop
    // (the valid set is convex and contains the centred crop).
    let place = |want: (f64, f64)| -> [f64; 4] {
        let at = |t: f64| {
            [
                centre_pos.0 + t * (want.0 - centre_pos.0),
                centre_pos.1 + t * (want.1 - centre_pos.1),
                cw,
                ch,
            ]
        };
        if rect_fits(w, h, angle_deg, at(1.0), 1e-6) {
            return at(1.0);
        }
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..30 {
            let mid = 0.5 * (lo + hi);
            if rect_fits(w, h, angle_deg, at(mid), 1e-6) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        at(lo)
    };

    let xs: &[f64] = if sb[2] / cw > CENTRE_SHARE {
        &[0.5]
    } else {
        &[1.0 / 3.0, 2.0 / 3.0]
    };
    let ys: &[f64] = if sb[3] / ch > CENTRE_SHARE {
        &[0.5]
    } else {
        &[1.0 / 3.0, 2.0 / 3.0]
    };
    let mut best: Option<(f64, [f64; 4])> = None;
    for &px in xs {
        for &py in ys {
            let r = place((sx - px * cw, sy - py * ch));
            if !contains(r, sb, SUBJECT_MARGIN) {
                continue;
            }
            // how far the subject landed from the wanted spot, plus a pull towards the
            // intersection nearest to where it already is (smallest change of the framing)
            let miss = (((sx - r[0]) / cw - px).powi(2) + ((sy - r[1]) / ch - py).powi(2)).sqrt();
            let score = miss + 0.1 * ((px - sx).abs() + (py - sy).abs());
            if best.is_none_or(|(b, _)| score < b - 1e-12) {
                best = Some((score, r));
            }
        }
    }
    best.map(|(_, r)| r)
        .unwrap_or_else(|| place((sx - cw / 2.0, sy - ch / 2.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inscribed_rect_is_the_known_closed_form() {
        // angle 0: the whole image at its own aspect
        let r = inscribed_rect(300, 200, 0.0, 1.5);
        assert!(
            r.iter()
                .zip([0.0, 0.0, 1.0, 1.0])
                .all(|(a, b)| (a - b).abs() < 1e-12),
            "{r:?}"
        );
        // square image rotated 45 deg, square crop: side = w / sqrt(2)
        let r = inscribed_rect(1000, 1000, 45.0, 1.0);
        assert!(
            (r[2] - 1.0 / 2f64.sqrt()).abs() < 1e-9 && (r[3] - r[2]).abs() < 1e-9,
            "{r:?}"
        );
        // 90 deg-ish limit never panics and stays valid
        let r = inscribed_rect(1000, 500, 44.0, 2.0);
        assert!(r[2] > 0.0 && r[3] > 0.0 && rect_fits(1000, 500, 44.0, r, 1e-6));
    }

    #[test]
    fn inscribed_rect_is_maximal_and_fits() {
        for (w, h) in [(1200u32, 800u32), (800, 1200), (1000, 1000)] {
            for deg in [-12.0, -3.5, -0.4, 0.0, 0.7, 2.0, 5.0, 10.0, 25.0] {
                for ratio in [1.0, 4.0 / 5.0, 3.0 / 2.0, 16.0 / 9.0, w as f64 / h as f64] {
                    let r = inscribed_rect(w, h, deg, ratio);
                    assert!(
                        rect_fits(w, h, deg, r, 1e-6),
                        "{w}x{h} {deg} {ratio}: {r:?}"
                    );
                    let px = (r[2] * w as f64) / (r[3] * h as f64);
                    assert!((px - ratio).abs() < 1e-9, "aspect kept");
                    // 0.5 % larger no longer fits (maximality)
                    let k = 1.005;
                    let big = [
                        0.5 - r[2] * k / 2.0,
                        0.5 - r[3] * k / 2.0,
                        r[2] * k,
                        r[3] * k,
                    ];
                    assert!(
                        !rect_fits(w, h, deg, big, 1e-6),
                        "{w}x{h} {deg} {ratio} not maximal"
                    );
                }
            }
        }
    }

    #[test]
    fn rect_fits_detects_outside_corners() {
        assert!(rect_fits(400, 300, 0.0, [0.0, 0.0, 1.0, 1.0], 1e-6));
        // the full canvas does not fit once the image is rotated
        assert!(!rect_fits(400, 300, 5.0, [0.0, 0.0, 1.0, 1.0], 1e-6));
        assert!(!rect_fits(400, 300, 0.0, [0.5, 0.5, 0.6, 0.6], 1e-6));
    }

    /// Level scene sampled in a frame rotated clockwise by `tilt` degrees (the convention of
    /// `analysis.tilt_deg`): sky above a horizon, dark ground, upright posts.
    pub(crate) fn scene(w: u32, h: u32, tilt: f64) -> RgbImage {
        let (s, c) = tilt.to_radians().sin_cos();
        let (cx, cy) = ((w as f64 - 1.0) / 2.0, (h as f64 - 1.0) / 2.0);
        let soft = |d: f64| 1.0 / (1.0 + (-d / 0.9).exp());
        let mut seed = 0x9e37_79b9_u32;
        let mut data = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = (x as f64 - cx, y as f64 - cy);
                let u = dx * c + dy * s;
                let v = -dx * s + dy * c;
                let g = soft(v - h as f64 * 0.08); // ground below the horizon
                let mut lum =
                    205.0 - 0.06 * (v + h as f64 / 2.0) * (1.0 - g) + g * (70.0 - 0.02 * v);
                let mut tint = [0.0, 8.0, 30.0 * (1.0 - g)];
                // posts: four vertical dark bars standing on the horizon
                for p in 0..4 {
                    let pu = (p as f64 - 1.5) * w as f64 * 0.22;
                    let bar = soft(u - pu + 6.0) * (1.0 - soft(u - pu - 6.0));
                    let up = soft(v + h as f64 * 0.25) * (1.0 - g);
                    lum -= 120.0 * bar * up;
                    tint[2] -= 20.0 * bar * up;
                }
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                lum += (seed % 9) as f64 - 4.0;
                data.push((lum + tint[0]).clamp(0.0, 255.0) as u8);
                data.push((lum + tint[1]).clamp(0.0, 255.0) as u8);
                data.push((lum + tint[2]).clamp(0.0, 255.0) as u8);
            }
        }
        RgbImage {
            width: w,
            height: h,
            data,
        }
    }

    #[test]
    fn tilt_estimator_reads_known_angles_and_the_sign() {
        for tilt in [0.0, 1.0, -1.0, 2.5, -4.0, 7.0, -10.0] {
            let e = estimate_tilt(&scene(960, 640, tilt));
            assert!((e.deg - tilt).abs() < 0.3, "true {tilt}: {e:?}");
            assert!(e.confidence >= MIN_TILT_CONFIDENCE, "true {tilt}: {e:?}");
        }
    }

    #[test]
    fn flat_images_have_no_confident_tilt() {
        let flat = RgbImage {
            width: 320,
            height: 240,
            data: vec![120; 320 * 240 * 3],
        };
        assert!(pick_tilt(&flat, None).is_none());
        assert!(auto_straighten(&flat, None, None, [0.0, 0.0, 1.0, 1.0]).is_none());
    }

    #[test]
    fn straighten_prefers_a_confident_stored_tilt() {
        let img = scene(480, 320, 3.0);
        let st = auto_straighten(&img, Some((-5.0, 0.8)), Some(1.5), [0.0, 0.0, 1.0, 1.0]).unwrap();
        assert_eq!(
            (st.source, st.angle, st.level),
            (TiltSource::Stored, -5.0, false)
        );
        assert!(rect_fits(480, 320, st.angle, st.rect, 1e-6));
        // a weak stored value falls back to the estimate
        let st =
            auto_straighten(&img, Some((-5.0, 0.05)), Some(1.5), [0.0, 0.0, 1.0, 1.0]).unwrap();
        assert_eq!(st.source, TiltSource::Estimated);
        assert!((st.angle - 3.0).abs() < 0.4, "{st:?}");
    }

    #[test]
    fn level_photos_stay_level() {
        let img = scene(480, 320, 0.0);
        for stored in [None, Some((0.08, 0.9)), Some((-0.1, 0.9))] {
            let st = auto_straighten(&img, stored, Some(1.5), [0.0, 0.0, 1.0, 1.0]).unwrap();
            assert!(st.level && st.angle == 0.0, "{st:?}");
            assert!(st
                .rect
                .iter()
                .zip([0.0, 0.0, 1.0, 1.0])
                .all(|(a, b)| (a - b).abs() < 1e-9));
        }
    }

    #[test]
    fn straighten_keeps_the_current_aspect_when_free() {
        let img = scene(600, 400, 4.0);
        let cur = [0.1, 0.1, 0.5, 0.8]; // pixel aspect 300:320
        let st = auto_straighten(&img, Some((4.0, 0.9)), None, cur).unwrap();
        let px = (st.rect[2] * 600.0) / (st.rect[3] * 400.0);
        assert!((px - 300.0 / 320.0).abs() < 1e-9, "{px}");
    }

    /// Renders `img` through the real pipeline with the straighten crop and returns the tilt
    /// still measured in the result.
    fn residual_after(img: &RgbImage, st: &Straighten) -> f64 {
        use crate::{Crop, EditStack, NoAssets, Op, RenderRequest};
        let stack = EditStack {
            version: 1,
            ops: vec![Op::Crop(Crop {
                rect: st.rect.map(|v| v as f32),
                angle: st.angle as f32,
                aspect: None,
            })],
        };
        let out = crate::create_renderer(false)
            .render(&RenderRequest {
                source: img,
                stack: &stack,
                masks: &NoAssets,
                luts: &NoAssets,
                max_long_edge: None,
            })
            .unwrap();
        let e = estimate_tilt(&out);
        assert!(
            e.confidence >= MIN_TILT_CONFIDENCE,
            "no structure left? {e:?}"
        );
        e.deg
    }

    #[test]
    fn one_click_straighten_leaves_at_most_half_a_degree() {
        let full = [0.0, 0.0, 1.0, 1.0];
        let mut worst = 0.0f64;
        for tilt in [
            -10.0, -8.0, -5.0, -3.0, -2.0, -1.0, 1.0, 1.5, 2.0, 3.0, 4.0, 5.0, 8.0, 10.0,
        ] {
            let img = scene(900, 600, tilt);
            for ratio in [Some(1.5), Some(1.0), None] {
                // analysis-time tilt (stored path) and the CPU fallback (estimate path)
                let est = estimate_tilt(&img);
                for stored in [Some((est.deg, est.confidence)), None] {
                    let st = auto_straighten(&img, stored, ratio, full).unwrap();
                    assert!(
                        !st.level && rect_fits(900, 600, st.angle, st.rect, 1e-6),
                        "{tilt}: {st:?}"
                    );
                    let res = residual_after(&img, &st);
                    println!("tilt {tilt:+5.1} ratio {ratio:?} source {:?}: angle {:+.1} residual {res:+.2}", st.source, st.angle);
                    worst = worst.max(res.abs());
                    assert!(res.abs() <= 0.5, "tilt {tilt}: residual {res}");
                }
            }
        }
        println!("worst residual {worst:.2} deg");
    }

    #[test]
    fn level_image_is_left_alone_end_to_end() {
        for tilt in [0.0, 0.05, -0.1] {
            let img = scene(600, 400, tilt);
            let st = auto_straighten(&img, None, Some(1.5), [0.0, 0.0, 1.0, 1.0]).unwrap();
            assert!(st.level && st.angle == 0.0, "{tilt}: {st:?}");
        }
    }

    fn rgb_with_blob(w: u32, h: u32, cx: f64, cy: f64, rad: f64) -> RgbImage {
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let d = ((x as f64 / w as f64 - cx).powi(2) + (y as f64 / h as f64 - cy).powi(2))
                    .sqrt();
                let v = if d < rad {
                    [230, 60, 40]
                } else {
                    [90, 100, 110]
                };
                data.extend_from_slice(&v);
            }
        }
        RgbImage {
            width: w,
            height: h,
            data,
        }
    }

    #[test]
    fn subject_comes_from_faces_then_saliency() {
        let img = rgb_with_blob(240, 160, 0.7, 0.4, 0.1);
        let s = find_subject(
            &img,
            &[
                [0.1, 0.1, 0.1, 0.2],
                [0.3, 0.2, 0.1, 0.1],
                [0.0, 0.0, 0.0, 0.0],
            ],
        );
        assert_eq!(s.kind, SubjectKind::Faces);
        let want = [0.1, 0.1, 0.3, 0.2];
        assert!(
            s.bbox.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-6),
            "{:?}",
            s.bbox
        );
        let s = find_subject(&img, &[]);
        assert_eq!(s.kind, SubjectKind::Saliency);
        let (cx, cy) = (s.bbox[0] + s.bbox[2] / 2.0, s.bbox[1] + s.bbox[3] / 2.0);
        assert!(
            (cx - 0.7).abs() < 0.08 && (cy - 0.4).abs() < 0.08,
            "{:?}",
            s.bbox
        );
        // a flat image has no subject: centred box
        let flat = RgbImage {
            width: 64,
            height: 48,
            data: vec![100; 64 * 48 * 3],
        };
        assert_eq!(find_subject(&flat, &[]).bbox, [0.25, 0.25, 0.5, 0.5]);
    }

    #[test]
    fn smart_crop_puts_a_small_face_on_a_third_and_keeps_it_inside() {
        let face = Subject {
            bbox: [0.62, 0.30, 0.10, 0.14],
            kind: SubjectKind::Faces,
        };
        for ratio in [
            None,
            Some(1.0),
            Some(4.0 / 5.0),
            Some(3.0 / 4.0),
            Some(16.0 / 9.0),
        ] {
            let r = smart_crop(1500, 1000, 0.0, ratio, &face);
            assert!(rect_fits(1500, 1000, 0.0, r, 1e-6), "{ratio:?} {r:?}");
            assert!(contains(r, face.bbox, 0.0), "{ratio:?} {r:?}");
            let want = ratio.unwrap_or(1.5);
            assert!(
                ((r[2] * 1500.0) / (r[3] * 1000.0) - want).abs() < 1e-6,
                "{ratio:?}"
            );
            // tighter than the full frame but never below 70 % of the largest crop
            let full = inscribed_rect(1500, 1000, 0.0, want);
            assert!(
                r[2] <= full[2] + 1e-9 && r[2] >= 0.7 * full[2] - 1e-9,
                "{ratio:?} {r:?}"
            );
            if ratio.is_none() {
                // face centre at ~2/3 of the width and ~1/3 of the height of the crop
                let (fx, fy) = ((0.67 - r[0]) / r[2], (0.37 - r[1]) / r[3]);
                assert!(
                    (fx - 2.0 / 3.0).abs() < 0.05 && (fy - 1.0 / 3.0).abs() < 0.05,
                    "{fx} {fy}"
                );
            }
        }
    }

    #[test]
    fn smart_crop_centres_wide_subjects_and_respects_rotation() {
        let group = Subject {
            bbox: [0.2, 0.3, 0.6, 0.3],
            kind: SubjectKind::Faces,
        };
        let r = smart_crop(1200, 800, 0.0, None, &group);
        assert!(contains(r, group.bbox, 0.0));
        assert!(
            ((r[0] + r[2] / 2.0) - 0.5).abs() < 0.02,
            "centred horizontally {r:?}"
        );
        // rotated: still inside the rotated image, subject still kept
        let face = Subject {
            bbox: [0.05, 0.05, 0.1, 0.12],
            kind: SubjectKind::Faces,
        };
        let r = smart_crop(1200, 800, 6.0, Some(1.0), &face);
        assert!(rect_fits(1200, 800, 6.0, r, 1e-6), "{r:?}");
    }

    #[test]
    fn smart_crop_is_deterministic() {
        let img = rgb_with_blob(300, 200, 0.3, 0.6, 0.12);
        let s = find_subject(&img, &[]);
        assert_eq!(
            smart_crop(300, 200, 0.0, Some(0.8), &s),
            smart_crop(300, 200, 0.0, Some(0.8), &s)
        );
    }
}
