//! Whole-image detail stage: noise reduction then sharpening (`Op::Detail`), plus the
//! output sharpening that has to follow it.
//!
//! Everything else in this crate is per-pixel, which is what lets the GPU run the whole
//! pipeline in one pass. Sharpening and noise reduction cannot work that way — both are
//! defined by a radius in output pixels and need the whole image. So this stage runs after
//! the backend's pixel loop, on the encoded 8-bit sRGB result, and **both** backends call
//! it. That is what keeps CPU and GPU bit-identical here rather than merely close: the GPU
//! contributes nothing to this stage, so there is nothing to diverge.
//!
//! Working on 8 bits is a deliberate trade: it matches the precision of the usual 8-bit
//! editing workflow, and it costs a `RgbImage` round-trip instead of a second float
//! readback path in the GPU renderer.
//!
//! ## Why these algorithms
//!
//! Noise reduction is a **noise-aware guided filter**. The frame's own noise level is
//! measured once (the median absolute high-frequency deviation, which edges do not move),
//! and that measurement becomes the guided filter's regularisation: where the local
//! variance is below the noise floor the filter falls back to the local mean, and where it
//! is above, it passes the pixel through. A clean photo therefore stays sharp instead of
//! being flattened to remove noise it never had — the failure mode of a fixed-strength
//! denoiser.
//!
//! Sharpening is an **edge-masked, halo-limited multi-scale unsharp mask**. A single
//! unsharp mask trades detail against halos, so the halo limiter compresses the
//! high-pass response against the local edge amplitude (a soft limiter, not a clamp, so
//! it never introduces a kink), the *Detail* slider cross-fades a second coarser scale in,
//! and *Masking* gates the whole thing on gradient magnitude.

use rayon::prelude::*;

use crate::cpu::sharpen_px;
use crate::prep::{box_mean, Plan};
use crate::RgbImage;

/// Radii in output pixels. Sharpening radius is user-controlled; these are the fixed
/// scales of the noise reduction.
const NR_RADIUS: usize = 3;
const NR_WIDE: f32 = 6.0;
/// Robust scale factor turning a median absolute deviation into a Gaussian sigma.
const MAD_TO_SIGMA: f32 = 1.4826;

/// Apply the plan's detail stage. Returns `img` untouched when there is none, so a stack
/// without a `detail` op renders exactly as it did before this module existed.
pub fn apply(plan: &Plan, img: RgbImage) -> RgbImage {
    let Some(stage) = &plan.detail else {
        return img;
    };
    let (w, h) = (img.width as usize, img.height as usize);
    let n = w * h;
    debug_assert_eq!(img.data.len(), n * 3);

    // Encoded (gamma) RGB as floats; the stage works in display space.
    let mut e = vec![[0.0f32; 3]; n];
    e.par_iter_mut().enumerate().for_each(|(i, o)| {
        *o = [
            img.data[i * 3] as f32 / 255.0,
            img.data[i * 3 + 1] as f32 / 255.0,
            img.data[i * 3 + 2] as f32 / 255.0,
        ];
    });

    // YCoCg: luma gets the detail treatment, chroma gets its own smoothing.
    let mut y = vec![0.0f32; n];
    let mut co = vec![0.0f32; n];
    let mut cg = vec![0.0f32; n];
    y.par_iter_mut()
        .zip(co.par_iter_mut())
        .zip(cg.par_iter_mut())
        .enumerate()
        .for_each(|(i, ((yv, cov), cgv))| {
            let c = e[i];
            let t = c[2] + (c[0] - c[2]) * 0.5;
            *yv = t + (c[1] - t) * 0.5;
            *cov = c[0] - c[2];
            *cgv = c[1] - t;
        });

    let noise = estimate_noise(&y, w, h);
    let nr = &stage.detail.noise;

    if !nr.is_neutral() {
        if nr.luminance > 0.0 {
            y = reduce_luma(&y, w, h, nr, noise);
        }
        if nr.color > 0.0 {
            let (nco, ncg) = reduce_chroma(&co, &cg, w, h, nr, noise);
            co = nco;
            cg = ncg;
        }
    }

    let sh = &stage.detail.sharpen;
    if sh.amount > 0.0 {
        y = sharpen(&y, w, h, sh);
    }

    // Back to RGB, then the output sharpening that was moved here (see `prep::build`).
    let mut out = RgbImage {
        width: img.width,
        height: img.height,
        data: vec![0u8; n * 3],
    };
    out.data
        .par_chunks_mut(w * 3)
        .enumerate()
        .for_each(|(row, buf)| {
            for x in 0..w {
                let i = row * w + x;
                let t = y[i] - cg[i] * 0.5;
                let r = co[i] + t - co[i] * 0.5;
                let g = cg[i] + t;
                let b = t - co[i] * 0.5;
                for (k, v) in [r, g, b].iter().enumerate() {
                    buf[x * 3 + k] = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                }
            }
        });

    if stage.sharpen > 0.0 {
        out = output_sharpen(out, stage.sharpen);
    }
    out
}

/// Median absolute deviation of the finest high-frequency band, in 0..1 encoded units.
/// A median (via a coarse histogram) rather than a mean, because a mean is dominated by
/// the edges we are trying to keep.
fn estimate_noise(y: &[f32], w: usize, h: usize) -> f32 {
    const BINS: usize = 512;
    const SPAN: f32 = 0.5; // |high-pass| beyond this is an edge, not noise
    let hp = blur(y, w, h, 1.0);
    let counts = y
        .par_chunks(1 << 14)
        .enumerate()
        .fold(
            || vec![0u32; BINS],
            |mut acc, (k, chunk)| {
                let base = k << 14;
                for (i, &v) in chunk.iter().enumerate() {
                    let d = (v - hp[base + i]).abs();
                    let b = ((d / SPAN * BINS as f32) as usize).min(BINS - 1);
                    acc[b] += 1;
                }
                acc
            },
        )
        .reduce(
            || vec![0u32; BINS],
            |mut a, b| {
                for (x, y) in a.iter_mut().zip(b.iter()) {
                    *x += *y;
                }
                a
            },
        );
    let total: u32 = counts.iter().sum();
    if total == 0 {
        return 0.0;
    }
    let half = total / 2;
    let mut acc = 0u32;
    for (b, &c) in counts.iter().enumerate() {
        acc += c;
        if acc >= half {
            return (b as f32 + 0.5) / BINS as f32 * SPAN * MAD_TO_SIGMA;
        }
    }
    0.0
}

/// Edge-preserving luminance noise reduction: a guided filter whose regularisation is the
/// measured noise level, blended in by the *Noise Reduction* amount, with *Detail* adding
/// back part of the removed band and *Contrast* restoring the low-frequency contrast the
/// smoothing costs.
fn reduce_luma(y: &[f32], w: usize, h: usize, nr: &crate::NoiseReduction, noise: f32) -> Vec<f32> {
    let strength = nr.luminance / 100.0;
    // eps is the variance below which the filter decides "this is noise". Tying it to the
    // measurement is what makes the control adaptive instead of absolute.
    let eps = (noise.max(0.002) * 1.4).powi(2);
    let base = guided(y, w, h, NR_RADIUS, eps);
    let keep = 0.15 + 0.7 * (nr.luminance_detail / 100.0);
    let mut out: Vec<f32> = (0..y.len())
        .into_par_iter()
        .map(|i| y[i] - strength * (y[i] - base[i]) * (1.0 - keep))
        .collect();
    if nr.luminance_contrast > 0.0 {
        let low = blur(&out, w, h, NR_WIDE);
        let k = nr.luminance_contrast / 100.0 * 0.5;
        out.par_iter_mut()
            .zip(low.par_iter())
            .for_each(|(v, l)| *v += k * (*v - *l));
    }
    out
}

/// Chroma noise reduction. Chroma noise is low-amplitude but very visible, so this smooths
/// hard; *Detail* shrinks the radius so more real colour edges survive.
fn reduce_chroma(
    co: &[f32],
    cg: &[f32],
    w: usize,
    h: usize,
    nr: &crate::NoiseReduction,
    noise: f32,
) -> (Vec<f32>, Vec<f32>) {
    let strength = nr.color / 100.0;
    let rad = (2.0 + 6.0 * (1.0 - nr.color_detail / 100.0)).round() as usize;
    let eps = (noise.max(0.002) * 0.8).powi(2);
    let blend = |src: &[f32]| -> Vec<f32> {
        let sm = guided(src, w, h, rad.max(1), eps);
        src.par_iter()
            .zip(sm.par_iter())
            .map(|(a, b)| a + strength * (b - a))
            .collect()
    };
    (blend(co), blend(cg))
}

/// Multi-scale unsharp mask with a soft halo limiter and an edge mask.
fn sharpen(y: &[f32], w: usize, h: usize, sh: &crate::SharpenDetail) -> Vec<f32> {
    let radius = sh.radius.clamp(0.5, 3.0);
    let b1 = blur(y, w, h, radius);
    let b2 = blur(y, w, h, radius * 2.5);
    let mix = sh.detail / 100.0;
    // Higher *Detail* = less halo, so the limiter is tightest at 100.
    let limit = 0.14 - 0.11 * mix;
    let gain = sh.amount / 100.0 * 0.9;
    let (ma, mb) = (
        0.001 + 0.05 * (sh.masking / 100.0),
        0.021 + 0.13 * (sh.masking / 100.0),
    );

    // Edge mask. The gradient is taken from the *blurred* luma so it spans the same few
    // pixels the unsharp halo does, then dilated over 3x3 so the whole halo band is
    // sharpened. Reading the raw gradient instead would leave the mask non-zero on exactly
    // one column of a vertical edge, so a high *Masking* setting would silently kill the
    // effect rather than confine it to edges.
    let edge: Vec<f32> = if sh.masking > 0.0 {
        let mut g = vec![0.0f32; y.len()];
        g.par_chunks_mut(w).enumerate().for_each(|(row, buf)| {
            let up = row.saturating_sub(1) * w;
            let dn = (row + 1).min(h - 1) * w;
            for (x, v) in buf.iter_mut().enumerate() {
                let xm = x.saturating_sub(1);
                let xp = (x + 1).min(w - 1);
                let gx = b1[row * w + xp] - b1[row * w + xm];
                let gy = b1[dn + x] - b1[up + x];
                *v = gx.abs() + gy.abs();
            }
        });
        let mut e = vec![0.0f32; y.len()];
        e.par_chunks_mut(w).enumerate().for_each(|(row, buf)| {
            let up = row.saturating_sub(1) * w;
            let dn = (row + 1).min(h - 1) * w;
            for (x, v) in buf.iter_mut().enumerate() {
                let xm = x.saturating_sub(1);
                let xp = (x + 1).min(w - 1);
                let m = g[up + xm]
                    .max(g[up + x])
                    .max(g[up + xp])
                    .max(g[row * w + xm])
                    .max(g[row * w + x])
                    .max(g[row * w + xp])
                    .max(g[dn + xm])
                    .max(g[dn + x])
                    .max(g[dn + xp]);
                *v = smoothstep(ma, mb, m);
            }
        });
        e
    } else {
        Vec::new()
    };

    let mut out = vec![0.0f32; y.len()];
    out.par_chunks_mut(w).enumerate().for_each(|(row, buf)| {
        for (x, v) in buf.iter_mut().enumerate() {
            let i = row * w + x;
            let hp = y[i] - b1[i];
            let mp = b1[i] - b2[i];
            let d = hp * (1.0 - 0.5 * mix) + mp * (0.5 * mix);
            // Soft limiter: linear for small responses, compressing the big ones, so a
            // strong edge cannot ring. tanh is C1 and never clips hard.
            let lim = limit * (1.0 + (b1[i] - b2[i]).abs() * 4.0);
            let d = lim * (d / lim).tanh();
            let mask = if sh.masking > 0.0 { edge[i] } else { 1.0 };
            *v = y[i] + gain * mask * d;
        }
    });
    out
}

/// The 5-tap output sharpen the per-pixel loop applies when there is no detail op.
fn output_sharpen(img: RgbImage, amount: f32) -> RgbImage {
    let (w, h) = (img.width as usize, img.height as usize);
    let mut enc = vec![[0.0f32; 3]; w * h];
    enc.par_iter_mut().enumerate().for_each(|(i, o)| {
        *o = [
            img.data[i * 3] as f32 / 255.0,
            img.data[i * 3 + 1] as f32 / 255.0,
            img.data[i * 3 + 2] as f32 / 255.0,
        ];
    });
    let mut data = vec![0u8; w * h * 3];
    data.par_chunks_mut(w * 3).enumerate().for_each(|(y, buf)| {
        for x in 0..w {
            let c = enc[y * w + x];
            let o = sharpen_px(
                c,
                enc[y * w + x.saturating_sub(1)],
                enc[y * w + (x + 1).min(w - 1)],
                enc[y.saturating_sub(1) * w + x],
                enc[(y + 1).min(h - 1) * w + x],
                amount,
            );
            for k in 0..3 {
                buf[x * 3 + k] = (o[k].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
        }
    });
    RgbImage {
        width: img.width,
        height: img.height,
        data,
    }
}

// ---------------------------------------------------------------- primitives

/// Separable Gaussian blur with a reflect-free clamped edge.
fn blur(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    let r = (3.0 * sigma).ceil().max(1.0) as i32;
    let wt: Vec<f32> = (-r..=r)
        .map(|d| (-(d * d) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let mut tmp = vec![0.0f32; src.len()];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let (mut acc, mut ws) = (0.0f32, 0.0f32);
            for (k, &g) in wt.iter().enumerate() {
                let xi = (x as i32 + k as i32 - r).clamp(0, w as i32 - 1) as usize;
                acc += g * src[y * w + xi];
                ws += g;
            }
            *o = acc / ws;
        }
    });
    let mut out = vec![0.0f32; src.len()];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let (mut acc, mut ws) = (0.0f32, 0.0f32);
            for (k, &g) in wt.iter().enumerate() {
                let yi = (y as i32 + k as i32 - r).clamp(0, h as i32 - 1) as usize;
                acc += g * tmp[yi * w + x];
                ws += g;
            }
            *o = acc / ws;
        }
    });
    out
}

/// Self-guided filter (He et al.) built from box means: output `q = a·I + b` averaged over
/// the window, where `a = var / (var + eps)`. With `I = p` this is an edge-preserving
/// smoother whose `a` is exactly a per-pixel soft-shrinkage coefficient.
fn guided(src: &[f32], w: usize, h: usize, rad: usize, eps: f32) -> Vec<f32> {
    let sq: Vec<f32> = src.par_iter().map(|v| v * v).collect();
    let mi = box_mean(src, w, h, rad);
    let mii = box_mean(&sq, w, h, rad);
    let mut a = vec![0.0f32; src.len()];
    let mut b = vec![0.0f32; src.len()];
    a.par_iter_mut()
        .zip(b.par_iter_mut())
        .enumerate()
        .for_each(|(i, (av, bv))| {
            let var = (mii[i] - mi[i] * mi[i]).max(0.0);
            *av = var / (var + eps);
            *bv = mi[i] * (1.0 - *av);
        });
    let ma = box_mean(&a, w, h, rad);
    let mb = box_mean(&b, w, h, rad);
    ma.par_iter()
        .zip(mb.par_iter())
        .enumerate()
        .map(|(i, (av, bv))| av * src[i] + bv)
        .collect()
}

#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
