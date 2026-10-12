//! Horizon / level estimation, a port of `ai-worker/imagepicker_ai/steps/tilt.py` (same
//! constants and formulas, cross-checked by `tests/parity.rs`): how far the dominant straight
//! structures (horizon, building edges, door frames, poles) deviate from level.
//!
//! An orientation-gated Hough transform restricted to near-axis lines: thinned strong edges whose
//! direction is within [`TILT_MAX_DEG`] (+ gate) of horizontal or vertical vote, per family, for
//! the candidate angles near their own gradient direction at their line offset `rho`. Only long
//! collinear runs (rho peaks of at least `LINE_MIN_FRAC` x the long edge) count, so texture,
//! faces and foliage add nothing. The tilt is the centroid of the angle profile's peak; the
//! confidence combines the line length behind the peak, its dominance over the best other angle
//! and, per family, how strongly that family alone prefers another angle (converging verticals
//! against a level horizon, a slanted table edge against upright door frames).
//!
//! Convention: `deg > 0` means the content is rotated clockwise (the horizon falls to the right);
//! rotating the photo counter-clockwise by `deg` levels it.

use crate::imgops::{filter3x3, gaussian_blur, Gray8, SOBEL_X, SOBEL_Y};

/// Largest deviation from level that is searched, in degrees (both signs).
pub const TILT_MAX_DEG: f64 = 15.0;
const GATE_DEG: f64 = 2.0;
const BIN_DEG: f64 = 0.25;
const BLUR_SIGMA: f64 = 1.0;
const EDGE_LO: f64 = 20.0;
const EDGE_HI: f64 = 60.0;
const MAG_STEPS: f64 = 16.0;
const MARGIN: usize = 2;
const LINE_MIN_FRAC: f64 = 0.04;
const PEAK_HALF_BINS: usize = 3;
const RIVAL_BINS: usize = 4;
const SUPPORT_SCALE: f64 = 0.5;
const FAMILY_SCALE: f64 = 0.5;

/// Result of [`estimate_tilt`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Tilt {
    /// Degrees, positive = rotated clockwise; 0 when nothing straight was found.
    pub deg: f64,
    /// 0..1: how much straight, agreeing structure backs `deg` (0 = none).
    pub confidence: f64,
}

fn n_bins() -> usize {
    (2.0 * TILT_MAX_DEG / BIN_DEG).round() as usize + 1
}

fn theta(k: usize) -> f64 {
    -TILT_MAX_DEG + BIN_DEG * k as f64
}

/// Python `round(x, n)` for the reported values.
fn round_to(x: f64, scale: f64) -> f64 {
    (x * scale).round_ties_even() / scale
}

/// `p[k-1] + p[k] + p[k+1]` (zero outside).
fn smooth3(p: &[f64]) -> Vec<f64> {
    let n = p.len();
    (0..n)
        .map(|k| {
            let left = if k > 0 { p[k - 1] } else { 0.0 };
            let right = if k + 1 < n { p[k + 1] } else { 0.0 };
            p[k] + left + right
        })
        .collect()
}

/// First index of the maximum (numpy `argmax`).
fn argmax(v: &[f64]) -> usize {
    let mut k = 0;
    for (i, &x) in v.iter().enumerate() {
        if x > v[k] {
            k = i;
        }
    }
    k
}

/// Soft-thresholded strength of the rho peaks of every angle row of one family's accumulator.
fn line_profile(acc: &[f64], n_rho: usize, line_min: f64) -> Vec<f64> {
    let mut a3 = vec![0f64; n_rho];
    acc.chunks(n_rho)
        .map(|row| {
            for r in 0..n_rho {
                let left = if r > 0 { row[r - 1] } else { 0.0 };
                let right = if r + 1 < n_rho { row[r + 1] } else { 0.0 };
                a3[r] = row[r] + left + right;
            }
            let at = |r: usize, d: isize| {
                let i = r as isize + d;
                if i < 0 || i >= n_rho as isize {
                    0.0
                } else {
                    a3[i as usize]
                }
            };
            let mut s = 0f64;
            for (r, &c) in a3.iter().enumerate() {
                if c > at(r, -1) && c > at(r, -2) && c >= at(r, 1) && c >= at(r, 2) {
                    s += c * ((c - line_min) / line_min).clamp(0.0, 1.0);
                }
            }
            s
        })
        .collect()
}

/// Angle profiles `[P_H, P_V]` (one value per `BIN_DEG` step over +-[`TILT_MAX_DEG`]) of the
/// near-horizontal and near-vertical lines of a gray image.
pub fn line_profiles(gray: &Gray8) -> [Vec<f64>; 2] {
    let (w, h) = (gray.w, gray.h);
    let nk = n_bins();
    if w.min(h) < 4 * MARGIN + 16 {
        return [vec![0.0; nk], vec![0.0; nk]];
    }
    let g = gaussian_blur(&gray.to_f32(), BLUR_SIGMA);
    let gx = filter3x3(&g, &SOBEL_X);
    let gy = filter3x3(&g, &SOBEL_Y);
    // 1/16 steps: equal magnitudes on both sides of a pixel-aligned edge stay equal whatever the
    // float rounding, so the thinning keeps the same pixel here and in the worker
    let mag: Vec<f64> = gx
        .data
        .iter()
        .zip(&gy.data)
        .map(|(&a, &b)| {
            let (a, b) = (a as f64, b as f64);
            ((a * a + b * b).sqrt() * MAG_STEPS).round_ties_even() / MAG_STEPS
        })
        .collect();
    let t = (TILT_MAX_DEG + GATE_DEG).to_radians().tan();
    let (cos, sin): (Vec<f64>, Vec<f64>) = (0..nk)
        .map(|k| {
            let a = theta(k).to_radians();
            (a.cos(), a.sin())
        })
        .unzip();
    let gate_bins = (2.0 * GATE_DEG / BIN_DEG).floor() as i64 + 1;
    let line_min = LINE_MIN_FRAC * w.max(h) as f64;
    let r0 = ((w as f64).hypot(h as f64) / 2.0).ceil() + 2.0;
    let n_rho = 2 * r0 as usize + 2;
    let (cx, cy) = ((w as f64 - 1.0) / 2.0, (h as f64 - 1.0) / 2.0);
    // [horizontal family, vertical family], each `nk` rows of `n_rho` offsets
    let mut accs = [vec![0f64; nk * n_rho], vec![0f64; nk * n_rho]];
    for y in MARGIN..h - MARGIN {
        for x in MARGIN..w - MARGIN {
            let i = y * w + x;
            let m = mag[i];
            if m <= EDGE_LO {
                continue;
            }
            let (gxv, gyv) = (gx.data[i] as f64, gy.data[i] as f64);
            let (ax, ay) = (gxv.abs(), gyv.abs());
            // non-maximum suppression across the edge: up/down for H, left/right for V
            let (vertical, delta) = if ax <= t * ay && m >= mag[i - w] && m > mag[i + w] {
                (false, (-gxv / gyv).atan().to_degrees())
            } else if ay <= t * ax && m >= mag[i - 1] && m > mag[i + 1] {
                (true, (gyv / gxv).atan().to_degrees())
            } else {
                continue;
            };
            let wgt = ((m - EDGE_LO) / (EDGE_HI - EDGE_LO)).clamp(0.0, 1.0);
            let (xc, yc) = (x as f64 - cx, y as f64 - cy);
            let acc = &mut accs[vertical as usize];
            let k0 = ((delta - GATE_DEG + TILT_MAX_DEG) / BIN_DEG).ceil() as i64;
            for k in (k0..=k0 + gate_bins).filter(|&k| k >= 0 && k < nk as i64) {
                let k = k as usize;
                let dist = (theta(k) - delta).abs();
                if dist >= GATE_DEG {
                    continue;
                }
                let wv = wgt * (1.0 - dist / GATE_DEG);
                let rho = if vertical {
                    xc * cos[k] + yc * sin[k]
                } else {
                    yc * cos[k] - xc * sin[k]
                } + r0;
                let r = rho.floor();
                let f = rho - r;
                let base = k * n_rho + r as usize;
                acc[base] += wv * (1.0 - f);
                acc[base + 1] += wv * f;
            }
        }
    }
    let [ah, av] = accs;
    [
        line_profile(&ah, n_rho, line_min),
        line_profile(&av, n_rho, line_min),
    ]
}

/// Tilt and confidence from the two families' angle profiles (`long_edge` of the image they
/// came from).
pub fn combine(prof_h: &[f64], prof_v: &[f64], long_edge: usize) -> Tilt {
    let long = long_edge as f64;
    let prof: Vec<f64> = prof_h.iter().zip(prof_v).map(|(a, b)| a + b).collect();
    let nk = prof.len();
    if prof.iter().sum::<f64>() <= 0.0 {
        return Tilt::default();
    }
    let sm = smooth3(&prof);
    let k = argmax(&sm);
    let (lo, hi) = (
        k.saturating_sub(PEAK_HALF_BINS),
        (k + PEAK_HALF_BINS + 1).min(nk),
    );
    let win = &prof[lo..hi];
    let top = win.iter().copied().fold(f64::MIN, f64::max);
    let (mut num, mut den) = (0f64, 0f64);
    for (j, &v) in win.iter().enumerate() {
        let wt = (v - 0.5 * top).max(0.0);
        num += wt * theta(lo + j);
        den += wt;
    }
    let rival = sm
        .iter()
        .enumerate()
        .filter(|&(i, _)| i.abs_diff(k) > RIVAL_BINS)
        .map(|(_, &v)| v)
        .fold(f64::MIN, f64::max);
    let dominance = (1.0 - rival / sm[k]).max(0.0);
    let mut conf = (1.0 - (-top / (SUPPORT_SCALE * long)).exp()) * dominance;
    for p in [prof_h, prof_v] {
        let smf = smooth3(p);
        let kf = argmax(&smf);
        let own = smf[kf];
        if kf.abs_diff(k) > RIVAL_BINS && own > 0.0 {
            let strength = 1.0 - (-own / (FAMILY_SCALE * long)).exp();
            conf *= 1.0 - strength * (1.0 - smf[k] / own);
        }
    }
    Tilt {
        deg: round_to(num / den, 1e2),
        confidence: round_to(conf, 1e3),
    }
}

/// Tilt of a gray image at analysis size (long edge about 1024, as the quality step uses).
pub fn estimate_tilt(gray: &Gray8) -> Tilt {
    let [ph, pv] = line_profiles(gray);
    combine(&ph, &pv, gray.w.max(gray.h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn soft(d: f64) -> f64 {
        1.0 / (1.0 + (-d / 0.8).exp())
    }

    /// Mid-gray image with soft-edged dark stripes, each `(y through the centre column as a
    /// fraction of the height, clockwise angle)`, plus optional upright posts rotated by the
    /// first stripe's angle and +-`noise` uniform noise.
    fn render(w: usize, h: usize, stripes: &[(f64, f64)], posts: usize, noise: u32) -> Gray8 {
        let cx = (w as f64 - 1.0) / 2.0;
        let mut seed = 0x2545_f491_u32;
        let mut data = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let mut v = 170.0;
                for &(fy, deg) in stripes {
                    let (s, c) = deg.to_radians().sin_cos();
                    let d = (y as f64 - fy * h as f64) * c - (x as f64 - cx) * s;
                    v -= 110.0 * soft(d + 3.0) * (1.0 - soft(d - 3.0));
                }
                if let Some(&(_, deg)) = stripes.first() {
                    let (s, c) = deg.to_radians().sin_cos();
                    for p in 0..posts {
                        let px = (p as f64 + 1.0) / (posts as f64 + 1.0) * w as f64 - cx;
                        let u = (x as f64 - cx) * c + (y as f64 - h as f64 / 2.0) * s - px;
                        v -= 90.0 * soft(u + 5.0) * (1.0 - soft(u - 5.0));
                    }
                }
                if noise > 0 {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    v += (seed % (2 * noise + 1)) as f64 - noise as f64;
                }
                data.push(v.round().clamp(0.0, 255.0) as u8);
            }
        }
        Gray8 { w, h, data }
    }

    #[test]
    fn level_and_tilted_lines() {
        for deg in [0.0, 1.0, -2.5, 4.0, -8.0, 12.0] {
            let t = estimate_tilt(&render(1024, 768, &[(0.5, deg)], 0, 0));
            assert!((t.deg - deg).abs() < 0.2, "{deg}: {t:?}");
            assert!(t.confidence > 0.5, "{deg}: {t:?}");
        }
    }

    #[test]
    fn verticals_agree_with_the_horizon() {
        let t = estimate_tilt(&render(1024, 768, &[(0.6, -3.0)], 4, 6));
        assert!((t.deg + 3.0).abs() < 0.2, "{t:?}");
        assert!(t.confidence > 0.5, "{t:?}");
        // posts alone (portrait orientation) carry the tilt too
        let t = estimate_tilt(&render(768, 1024, &[(2.0, 2.0)], 3, 0));
        assert!((t.deg - 2.0).abs() < 0.2, "{t:?}");
    }

    #[test]
    fn rival_angles_lower_the_confidence() {
        let one = estimate_tilt(&render(1024, 768, &[(0.3, 1.0)], 0, 0));
        let two = estimate_tilt(&render(1024, 768, &[(0.3, 1.0), (0.7, 7.0)], 0, 0));
        assert!(two.confidence < 0.5 * one.confidence, "{two:?} vs {one:?}");
    }

    #[test]
    fn no_straight_structure_means_no_confidence() {
        let flat = Gray8 {
            w: 640,
            h: 480,
            data: vec![120; 640 * 480],
        };
        assert_eq!(estimate_tilt(&flat), Tilt::default());
        // blobs with steep curved outlines: plenty of edges, none of them long and straight
        let (w, h) = (800usize, 600usize);
        let mut data = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f64 / 37.0, y as f64 / 29.0);
                let f = fx.sin() * fy.cos() + (0.7 * fx + 0.4 * fy).sin() - 0.3;
                data.push((130.0 + 70.0 * (4.0 * f).tanh()) as u8);
            }
        }
        let t = estimate_tilt(&Gray8 { w, h, data });
        assert!(t.confidence < 0.1, "{t:?}");
        let tiny = Gray8 {
            w: 20,
            h: 20,
            data: vec![0; 400],
        };
        assert_eq!(estimate_tilt(&tiny), Tilt::default());
    }
}
