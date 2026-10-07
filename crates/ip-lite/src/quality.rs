//! Classic technical-quality metrics, a port of `ai-worker/imagepicker_ai/steps/quality.py`
//! without the face-box branch (the on-device profile has no face detector): sharpness of the
//! central region, exposure statistics and Immerkaer noise. Same constants, same 5-digit
//! rounding of the reported values.

use crate::imgops::{
    filter3x3, gaussian_blur, resize_area, resize_area_emulated, resize_linear, rgb_to_gray,
    Gray32, Gray8, IMMERKAER, LAPLACIAN1, SOBEL_X, SOBEL_Y,
};

pub const REF_LONG_EDGE: usize = 1024;
const K_LAP: f64 = 120.0;
const K_TEN: f64 = 1500.0;
const CENTER_MARGIN: f64 = 0.2;
const NOISE_FULL_SCALE_SIGMA: f64 = 12.0;
const K_LAP_FACE: f64 = 80.0;
const K_TEN_FACE: f64 = 6000.0;
const FACE_LAP_WEIGHT: f64 = 0.7;
const FACE_CROP: usize = 128;

/// Result of [`analyze_quality`]; field meanings follow the worker's `quality` step output.
#[derive(Debug, Clone, PartialEq)]
pub struct Quality {
    pub sharpness: f64,
    pub sharpness_center: f64,
    pub laplacian_var: f64,
    pub tenengrad: f64,
    pub exposure: f64,
    pub mean_luminance: f64,
    pub clipped_highlights: f64,
    pub crushed_shadows: f64,
    pub noise: f64,
    pub noise_sigma: f64,
}

fn clamp01(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

/// Python `round(x, 5)`.
fn r5(x: f64) -> f64 {
    (x * 1e5).round_ties_even() / 1e5
}

/// Python `round(x)` (banker's rounding).
fn pyround(x: f64) -> usize {
    x.round_ties_even() as usize
}

/// Gray image resampled so that its long edge is about [`REF_LONG_EDGE`].
fn norm_gray(rgb: &[u8], w: usize, h: usize) -> Gray8 {
    let gray = rgb_to_gray(rgb, w, h);
    let long = w.max(h);
    if (long as f64 - REF_LONG_EDGE as f64).abs() > 0.1 * REF_LONG_EDGE as f64 {
        let s = REF_LONG_EDGE as f64 / long as f64;
        let (dw, dh) = (pyround(w as f64 * s).max(1), pyround(h as f64 * s).max(1));
        return if s < 1.0 {
            resize_area(&gray, dw, dh)
        } else {
            resize_linear(&gray, dw, dh)
        };
    }
    gray
}

/// `(variance of Laplacian, Tenengrad)` of a gray image (float32 math, lightly smoothed).
pub fn focus_measures(gray: &Gray8) -> (f64, f64) {
    let g = gaussian_blur(&gray.to_f32(), 0.7);
    let lap = filter3x3(&g, &LAPLACIAN1);
    let n = lap.data.len() as f64;
    let mean = lap.data.iter().map(|&v| v as f64).sum::<f64>() / n;
    let var = lap
        .data
        .iter()
        .map(|&v| {
            let d = v as f64 - mean;
            d * d
        })
        .sum::<f64>()
        / n;
    let gx = filter3x3(&g, &SOBEL_X);
    let gy = filter3x3(&g, &SOBEL_Y);
    let ten = gx
        .data
        .iter()
        .zip(&gy.data)
        .map(|(&a, &b)| (a * a + b * b) as f64)
        .sum::<f64>()
        / n;
    (var, ten)
}

/// `0.5 (1 - exp(-lap/K_lap)) + 0.5 (1 - exp(-ten/K_ten))`, clamped to 0..1.
pub fn sharpness_score(lap: f64, ten: f64) -> f64 {
    clamp01(0.5 * (1.0 - (-lap / K_LAP).exp()) + 0.5 * (1.0 - (-ten / K_TEN).exp()))
}

/// Face variant of [`sharpness_score`] (face constants, Laplacian-heavy weighting).
pub fn sharpness_score_face(lap: f64, ten: f64) -> f64 {
    clamp01(
        FACE_LAP_WEIGHT * (1.0 - (-lap / K_LAP_FACE).exp())
            + (1.0 - FACE_LAP_WEIGHT) * (1.0 - (-ten / K_TEN_FACE).exp()),
    )
}

/// Port of the worker's `face_sharpness`: sharpness of a face given its normalised `[x, y, w, h]`
/// box, measured on the inner 60% of a 128x128 gray crop. `None` for crops under 8 px.
pub fn face_sharpness(rgb: &[u8], w: usize, h: usize, bbox: [f64; 4]) -> Option<f64> {
    assert_eq!(rgb.len(), w * h * 3, "rgb buffer size");
    let (x0, y0) = (
        ((bbox[0] * w as f64) as i64).max(0) as usize,
        ((bbox[1] * h as f64) as i64).max(0) as usize,
    );
    let x1 = (((bbox[0] + bbox[2]) * w as f64) as i64).clamp(0, w as i64) as usize;
    let y1 = (((bbox[1] + bbox[3]) * h as f64) as i64).clamp(0, h as i64) as usize;
    if x1 < x0 + 8 || y1 < y0 + 8 {
        return None;
    }
    let gray = rgb_to_gray(rgb, w, h).crop(x0, y0, x1, y1);
    let (cw, ch) = (gray.w, gray.h);
    let crop = if cw.max(ch) > FACE_CROP {
        if cw >= FACE_CROP && ch >= FACE_CROP {
            resize_area(&gray, FACE_CROP, FACE_CROP)
        } else {
            resize_area_emulated(&gray, FACE_CROP, FACE_CROP)
        }
    } else {
        resize_linear(&gray, FACE_CROP, FACE_CROP)
    };
    let (lap, ten) = focus_measures(&center_region(&crop));
    Some(sharpness_score_face(lap, ten))
}

fn center_region(gray: &Gray8) -> Gray8 {
    let (h, w) = (gray.h, gray.w);
    let my = (h as f64 * CENTER_MARGIN) as usize;
    let mx = (w as f64 * CENTER_MARGIN) as usize;
    gray.crop(mx, my, w - mx, h - my)
}

/// `(exposure score, mean luminance, clipped highlights, crushed shadows)` from a 1/2 subsample.
fn exposure_metrics(rgb: &[u8], w: usize, h: usize) -> (f64, f64, f64, f64) {
    let (hi_t, lo_t) = ((250.0f64 / 255.0) as f32, (5.0f64 / 255.0) as f32);
    let (mut sum, mut hi, mut lo, mut n) = (0f64, 0u64, 0u64, 0u64);
    for y in (0..h).step_by(2) {
        for x in (0..w).step_by(2) {
            let p = &rgb[(y * w + x) * 3..][..3];
            let (r, g, b) = (
                p[0] as f32 / 255.0,
                p[1] as f32 / 255.0,
                p[2] as f32 / 255.0,
            );
            let luma = 0.2126f32 * r + 0.7152f32 * g + 0.0722f32 * b;
            sum += luma as f64;
            hi += (luma >= hi_t) as u64;
            lo += (luma <= lo_t) as u64;
            n += 1;
        }
    }
    let n = n.max(1) as f64;
    let (mean, hi, lo) = (sum / n, hi as f64 / n, lo as f64 / n);
    let score = clamp01(
        1.0 - (mean - 0.46).abs() / 0.5 - 3.0 * (hi - 0.02).max(0.0) - 2.0 * (lo - 0.05).max(0.0),
    );
    (score, mean, hi, lo)
}

/// numpy `median` of a slice (mean of the two middle values for even lengths).
fn median(v: &mut [f32]) -> f32 {
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        // numpy: mean of the two middle elements in float32
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// Immerkaer noise sigma from the flattest 25 % of the 16x16 blocks.
pub fn noise_sigma(gray: &Gray8) -> f64 {
    const BLOCK: usize = 16;
    let (h, w) = (gray.h, gray.w);
    let (h2, w2) = (h / BLOCK * BLOCK, w / BLOCK * BLOCK);
    if h2 < BLOCK * 2 || w2 < BLOCK * 2 {
        return 0.0;
    }
    let g: Gray32 = gray.crop(0, 0, w2, h2).to_f32();
    let resp = filter3x3(&g, &IMMERKAER);
    let sm = gaussian_blur(&g, 1.0);
    let gx = filter3x3(&sm, &SOBEL_X);
    let gy = filter3x3(&sm, &SOBEL_Y);
    let (bx, by) = (w2 / BLOCK, h2 / BLOCK);
    let nb = bx * by;
    let mut grad_mean = vec![0f32; nb];
    let mut bright = vec![0f32; nb];
    for j in 0..by {
        for i in 0..bx {
            let (mut gs, mut bs) = (0f32, 0f32);
            for y in 0..BLOCK {
                let o = (j * BLOCK + y) * w2 + i * BLOCK;
                for x in 0..BLOCK {
                    gs +=
                        (gx.data[o + x] * gx.data[o + x] + gy.data[o + x] * gy.data[o + x]).sqrt();
                    bs += g.data[o + x];
                }
            }
            grad_mean[j * bx + i] = gs / (BLOCK * BLOCK) as f32;
            bright[j * bx + i] = bs / (BLOCK * BLOCK) as f32;
        }
    }
    let mut idx: Vec<usize> = (0..nb)
        .filter(|&k| bright[k] > 8.0 && bright[k] < 247.0)
        .collect();
    if idx.len() < 4 {
        idx = (0..nb).collect();
    }
    let n_pick = 4.max((idx.len() as f64 * 0.25) as usize);
    idx.sort_by(|&a, &b| grad_mean[a].total_cmp(&grad_mean[b])); // stable
    idx.truncate(n_pick);
    let mut sigmas: Vec<f32> = idx
        .iter()
        .map(|&k| {
            let (i, j) = (k % bx, k / bx);
            let mut vals = Vec::with_capacity(BLOCK * BLOCK);
            for y in 0..BLOCK {
                let o = (j * BLOCK + y) * w2 + i * BLOCK;
                vals.extend_from_slice(&resp.data[o..o + BLOCK]);
            }
            let med = median(&mut vals.clone());
            let mut dev: Vec<f32> = vals.iter().map(|v| (v - med).abs()).collect();
            let mad = median(&mut dev);
            1.4826f32 * mad / 6.0
        })
        .collect();
    median(&mut sigmas) as f64
}

/// Sharpness / exposure / noise of an upright RGB8 image (analysis size).
pub fn analyze_quality(rgb: &[u8], w: usize, h: usize) -> Quality {
    assert_eq!(rgb.len(), w * h * 3, "rgb buffer size");
    let gray = norm_gray(rgb, w, h);
    let (lap, ten) = focus_measures(&center_region(&gray));
    let s_center = sharpness_score(lap, ten);
    let (exposure, mean, hi, lo) = exposure_metrics(rgb, w, h);
    let sigma = noise_sigma(&gray);
    Quality {
        sharpness: r5(s_center),
        sharpness_center: r5(s_center),
        laplacian_var: r5(lap),
        tenengrad: r5(ten),
        exposure: r5(exposure),
        mean_luminance: r5(mean),
        clipped_highlights: r5(hi),
        crushed_shadows: r5(lo),
        noise: r5(clamp01(sigma / NOISE_FULL_SCALE_SIGMA)),
        noise_sigma: r5(sigma),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: usize, h: usize, v: u8) -> Vec<u8> {
        vec![v; w * h * 3]
    }

    #[test]
    fn flat_image_has_no_detail() {
        let q = analyze_quality(&flat(256, 192, 117), 256, 192);
        assert_eq!(q.sharpness, 0.0);
        assert_eq!(q.noise, 0.0);
        assert!(q.exposure > 0.85);
    }

    #[test]
    fn blown_out_and_black_images_score_low() {
        let white = analyze_quality(&flat(128, 128, 255), 128, 128);
        let black = analyze_quality(&flat(128, 128, 0), 128, 128);
        assert!(white.exposure < 0.1 && white.clipped_highlights > 0.99);
        assert!(black.exposure < 0.1 && black.crushed_shadows > 0.99);
    }

    #[test]
    fn edges_raise_sharpness() {
        let (w, h) = (256usize, 192usize);
        let mut img = flat(w, h, 100);
        for y in 0..h {
            for x in 0..w {
                if (x / 8 + y / 8) % 2 == 0 {
                    img[(y * w + x) * 3..][..3].copy_from_slice(&[200, 200, 200]);
                }
            }
        }
        let q = analyze_quality(&img, w, h);
        assert!(q.sharpness > 0.5, "{q:?}");
    }
}
