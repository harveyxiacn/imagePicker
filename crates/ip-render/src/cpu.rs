//! CPU reference renderer (rayon). The WGSL shader is a line-for-line port of the
//! per-pixel functions here.

use rayon::prelude::*;

use crate::color::*;
use crate::geom::{self, Geo};
use crate::prep::{self, r, Plan, CURVE_N, HSL_CENTERS};
use crate::*;

const HAZE_A: f32 = 0.92;
const SKIN_HUE: f32 = 55.0;
const MID_PV: f32 = 0.4614; // sRGB-encoded 18 % grey

pub struct CpuRenderer;

impl Renderer for CpuRenderer {
    fn backend(&self) -> Backend {
        Backend::Cpu
    }
    fn render(&self, req: &RenderRequest<'_>) -> Result<RgbImage> {
        let plan = prep::build(req)?;
        Ok(render_plan(&plan, req.source))
    }
}

pub type Layer = Vec<[f32; 4]>;

struct Ctx<'a> {
    plan: &'a Plan,
    layers: &'a [Option<Layer>],
}

// ---------------------------------------------------------------- per-pixel adjust

#[inline]
fn flag(rec: &[f32; prep::REC], f: u32) -> bool {
    (rec[r::FLAGS] as u32) & f != 0
}

#[inline]
fn hue_weights(hdeg: f32) -> [(usize, f32); 2] {
    let mut hh = hdeg;
    if hh < HSL_CENTERS[0] {
        hh += 360.0;
    }
    for i in 0..8 {
        let lo = HSL_CENTERS[i];
        let hi = if i < 7 { HSL_CENTERS[i + 1] } else { 389.0 };
        if hh >= lo && hh < hi {
            let t = (hh - lo) / (hi - lo);
            let s = (std::f32::consts::FRAC_PI_2 * t).sin();
            let w_hi = s * s;
            return [(i, 1.0 - w_hi), ((i + 1) % 8, w_hi)];
        }
    }
    [(7, 1.0), (0, 0.0)]
}

fn adjust(rec: &[f32; prep::REC], rgb_in: [f32; 3], lay: [f32; 4], curves: &[f32]) -> [f32; 3] {
    let mut c = rgb_in;
    if flag(rec, r::F_WB) {
        let m = &rec[r::WB..r::WB + 9];
        c = [
            (m[0] * c[0] + m[1] * c[1] + m[2] * c[2]).max(0.0),
            (m[3] * c[0] + m[4] * c[1] + m[5] * c[2]).max(0.0),
            (m[6] * c[0] + m[7] * c[1] + m[8] * c[2]).max(0.0),
        ];
    }
    if flag(rec, r::F_EV) {
        let g = rec[r::GAIN];
        c = [c[0] * g, c[1] * g, c[2] * g];
        if rec[r::EV] > 0.0 {
            let knee = rec[r::KNEE];
            let m = c[0].max(c[1]).max(c[2]);
            if m > knee {
                let m2 = knee + (1.0 - knee) * (1.0 - (-(m - knee) / (1.0 - knee)).exp());
                let k = m2 / m;
                c = [c[0] * k, c[1] * k, c[2] * k];
            }
        }
    }
    if flag(rec, r::F_HS) {
        let lb = lay[0] * rec[r::GAIN];
        let p = srgb_enc(lb);
        let ws = 1.0 - smoothstep(0.1, 0.65, p);
        let wh = smoothstep(0.35, 0.9, p);
        let g = (rec[r::SH] * ws + rec[r::HI] * wh).exp2();
        c = [c[0] * g, c[1] * g, c[2] * g];
    }
    if flag(rec, r::F_WB_BL) {
        let py = srgb_enc(lum(c));
        let g = (rec[r::WHITES] * smoothstep(0.4, 1.0, py)).exp2();
        c = [c[0] * g, c[1] * g, c[2] * g];
        let py = srgb_enc(lum(c));
        let d = rec[r::BLACKS] * (1.0 - smoothstep(0.0, 0.35, py));
        c = [
            (c[0] + d).max(0.0),
            (c[1] + d).max(0.0),
            (c[2] + d).max(0.0),
        ];
    }
    c = [
        c[0].clamp(0.0, 1.0),
        c[1].clamp(0.0, 1.0),
        c[2].clamp(0.0, 1.0),
    ];
    if flag(rec, r::F_CONTRAST) {
        let e = rec[r::CONTRAST];
        for v in c.iter_mut() {
            let p = srgb_enc(*v);
            let q = if p < MID_PV {
                MID_PV * (p / MID_PV).powf(e)
            } else {
                1.0 - (1.0 - MID_PV) * ((1.0 - p) / (1.0 - MID_PV)).powf(e)
            };
            *v = srgb_dec(q.clamp(0.0, 1.0));
        }
    }
    if flag(rec, r::F_CLARITY) {
        let yin = lum(rgb_in);
        let ratio = (yin / lay[1].max(1e-4)).clamp(0.25, 4.0);
        let pm = srgb_enc(lum(c));
        let wm = 1.0 - (2.0 * pm - 1.0) * (2.0 * pm - 1.0);
        let g = (ratio.log2() * rec[r::CLARITY] * wm).exp2();
        c = [c[0] * g, c[1] * g, c[2] * g];
    }
    if flag(rec, r::F_DEHAZE) {
        let w = rec[r::DEHAZE_W];
        if w > 0.0 {
            let t = (1.0 - w * lay[2] / HAZE_A).clamp(0.25, 1.0);
            c = [
                (c[0] - HAZE_A) / t + HAZE_A,
                (c[1] - HAZE_A) / t + HAZE_A,
                (c[2] - HAZE_A) / t + HAZE_A,
            ];
        }
        let s = rec[r::DEHAZE_S];
        if s > 0.0 {
            c = [
                c[0] * (1.0 - s) + HAZE_A * s,
                c[1] * (1.0 - s) + HAZE_A * s,
                c[2] * (1.0 - s) + HAZE_A * s,
            ];
        }
        c = [c[0].max(0.0), c[1].max(0.0), c[2].max(0.0)];
    }
    if flag(rec, r::F_CURVES) {
        let off = rec[r::CURVE_OFF] as usize;
        for (k, v) in c.iter_mut().enumerate() {
            let p = srgb_enc(*v) * (CURVE_N - 1) as f32;
            let i = (p.floor() as usize).min(CURVE_N - 2);
            let f = p - i as f32;
            let t = &curves[off + k * CURVE_N..off + (k + 1) * CURVE_N];
            *v = srgb_dec(t[i] + (t[i + 1] - t[i]) * f);
        }
    }
    if flag(rec, r::F_COLOR | r::F_HSL | r::F_GRADE) {
        let mut lab = to_oklab(c);
        let chroma = lab[1].hypot(lab[2]);
        let mut hue = if chroma > 1e-6 {
            lab[2].atan2(lab[1])
        } else {
            0.0
        };
        let mut cs = 1.0f32;
        if flag(rec, r::F_COLOR) {
            let wv = 1.0 - (chroma / 0.26).clamp(0.0, 1.0);
            let mut dh = (hue.to_degrees() - SKIN_HUE).abs() % 360.0;
            if dh > 180.0 {
                dh = 360.0 - dh;
            }
            let skin = 1.0 - 0.65 * (-(dh / 30.0) * (dh / 30.0)).exp();
            cs *= (1.0 + rec[r::VIBRANCE] * wv * skin).max(0.0);
            cs *= (1.0 + rec[r::SAT]).max(0.0);
        }
        let mut dl = 0.0f32;
        if flag(rec, r::F_HSL) {
            let hd = {
                let h = hue.to_degrees();
                if h < 0.0 {
                    h + 360.0
                } else {
                    h
                }
            };
            let cw = smoothstep(0.0, 0.03, chroma);
            let (mut sh, mut ss, mut sl) = (0.0f32, 0.0f32, 0.0f32);
            for (i, w) in hue_weights(hd) {
                sh += w * rec[r::HSL + i * 3];
                ss += w * rec[r::HSL + i * 3 + 1];
                sl += w * rec[r::HSL + i * 3 + 2];
            }
            hue += (sh * cw * 30.0).to_radians();
            cs *= (1.0 + ss * cw).max(0.0);
            dl = sl * cw * 0.25;
        }
        let nc = chroma * cs;
        lab[0] = (lab[0] + dl).clamp(0.0, 1.0);
        lab[1] = nc * hue.cos();
        lab[2] = nc * hue.sin();
        if flag(rec, r::F_GRADE) {
            let pivot = rec[r::PIVOT];
            let l = lab[0];
            let ws = 1.0 - smoothstep(0.0, pivot, l);
            let wh = smoothstep(pivot, 1.0, l);
            let wm = 1.0 - ws - wh;
            let g = &rec[r::GRADE..r::GRADE + 6];
            lab[1] += ws * g[0] + wm * g[2] + wh * g[4];
            lab[2] += ws * g[1] + wm * g[3] + wh * g[5];
        }
        c = from_oklab(lab);
        c = [c[0].max(0.0), c[1].max(0.0), c[2].max(0.0)];
    }
    [
        c[0].clamp(0.0, 1.0),
        c[1].clamp(0.0, 1.0),
        c[2].clamp(0.0, 1.0),
    ]
}

// ---------------------------------------------------------------- masks & layers

pub(crate) fn bilerp_idx(u: f32, n: u32) -> (usize, usize, f32) {
    let x = u * n as f32 - 0.5;
    let x0 = x.floor();
    let f = x - x0;
    let i0 = (x0 as i32).clamp(0, n as i32 - 1) as usize;
    let i1 = (x0 as i32 + 1).clamp(0, n as i32 - 1) as usize;
    (i0, i1, f)
}

fn layer_at(layer: &Layer, proxy: (u32, u32), u: f32, v: f32) -> [f32; 4] {
    layer_at_base(layer, 0, proxy, u, v)
}

/// Bilinear layer sample; `base` is the first pixel of the slot (beauty ops own two slots).
fn layer_at_base(layer: &Layer, base: usize, proxy: (u32, u32), u: f32, v: f32) -> [f32; 4] {
    let (x0, x1, fx) = bilerp_idx(u, proxy.0);
    let (y0, y1, fy) = bilerp_idx(v, proxy.1);
    let w = proxy.0 as usize;
    let layer = &layer[base..];
    let mut o = [0.0f32; 4];
    for (k, ov) in o.iter_mut().enumerate() {
        let a = layer[y0 * w + x0][k];
        let b = layer[y0 * w + x1][k];
        let c = layer[y1 * w + x0][k];
        let d = layer[y1 * w + x1][k];
        let t = a + (b - a) * fx;
        let bt = c + (d - c) * fx;
        *ov = t + (bt - t) * fy;
    }
    o
}

fn mask_value(rec: &[f32; prep::REC], planes: &[f32], u: f32, v: f32, guide: f32) -> f32 {
    let kind = rec[r::KIND];
    let m0 = &rec[r::M0..r::M0 + 5];
    let m = if kind == r::KIND_BEAUTY && m0[1] < 1.0 {
        0.0 // no skin plane: only the landmark features act
    } else if kind == r::KIND_AI || kind == r::KIND_BEAUTY {
        let (pw, ph) = (m0[1] as u32, m0[2] as u32);
        let off = m0[0] as usize;
        let (x0, x1, fx) = bilerp_idx(u, pw);
        let (y0, y1, fy) = bilerp_idx(v, ph);
        let w = pw as usize;
        let mut ab = [0.0f32; 2];
        for (k, o) in ab.iter_mut().enumerate() {
            let at = |x: usize, y: usize| planes[off + (y * w + x) * 2 + k];
            let t = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * fx;
            let b = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * fx;
            *o = t + (b - t) * fy;
        }
        (ab[0] * guide + ab[1]).clamp(0.0, 1.0)
    } else if kind == r::KIND_RADIAL {
        let dx = (u - m0[0]) / m0[2];
        let dy = (v - m0[1]) / m0[3];
        let d = (dx * dx + dy * dy).sqrt();
        1.0 - smoothstep(1.0 - m0[4], 1.0, d)
    } else {
        let (ex, ey) = (m0[2] - m0[0], m0[3] - m0[1]);
        let len2 = (ex * ex + ey * ey).max(1e-8);
        let t = ((u - m0[0]) * ex + (v - m0[1]) * ey) / len2;
        1.0 - smoothstep(0.0, 1.0, t)
    };
    let m = if rec[r::INVERT] > 0.5 { 1.0 - m } else { m };
    m * rec[r::AMOUNT]
}

impl Ctx<'_> {
    /// Run the first `nops` ops on a pre-op linear pixel at normalised position (u, v).
    fn ops_px(&self, rgb0: [f32; 3], u: f32, v: f32, nops: usize) -> [f32; 3] {
        let guide = enc_guide(rgb0);
        let mut c = rgb0;
        for k in 0..nops {
            let rec = &self.plan.ops[k];
            if rec[r::KIND] == r::KIND_BEAUTY {
                let m = mask_value(rec, &self.plan.planes, u, v, guide);
                let fl = rec[r::FLAGS] as u32;
                if m > 0.0 || fl & (r::FB_EYE | r::FB_TEETH) != 0 {
                    c = self.beauty_px(rec, c, m, u, v, self.layers[k].as_ref());
                }
                continue;
            }
            let global = rec[r::KIND] == r::KIND_GLOBAL;
            let m = if global {
                1.0
            } else {
                mask_value(rec, &self.plan.planes, u, v, guide)
            };
            if m <= 0.0 {
                continue;
            }
            let lay = match &self.layers[k] {
                Some(l) => layer_at(l, self.plan.proxy, u, v),
                None => [0.0; 4],
            };
            let a = adjust(rec, c, lay, &self.plan.curves);
            c = if global {
                a
            } else {
                [
                    c[0] + (a[0] - c[0]) * m,
                    c[1] + (a[1] - c[1]) * m,
                    c[2] + (a[2] - c[2]) * m,
                ]
            };
        }
        c
    }
}

/// Evaluate the first `nops` ops over the proxy grid (linear light).
fn proxy_base(plan: &Plan, src: &RgbImage) -> Vec<[f32; 3]> {
    let (pw, ph) = plan.proxy;
    let g = plan.proxy_geo();
    let mut out = vec![[0.0f32; 3]; (pw * ph) as usize];
    out.par_chunks_mut(pw as usize)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                let d = plan.warp_disp((x as f32 + 0.5) / pw as f32, (y as f32 + 0.5) / ph as f32);
                *o = geom::sample_off(
                    src,
                    &g,
                    x as u32,
                    y as u32,
                    d[0] * pw as f32,
                    d[1] * ph as f32,
                );
            }
        });
    out
}

fn render_proxy(
    plan: &Plan,
    base: &[[f32; 3]],
    layers: &[Option<Layer>],
    nops: usize,
) -> Vec<[f32; 3]> {
    let (pw, ph) = plan.proxy;
    let ctx = Ctx { plan, layers };
    let mut out = vec![[0.0f32; 3]; (pw * ph) as usize];
    out.par_chunks_mut(pw as usize)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                let u = (x as f32 + 0.5) / pw as f32;
                let v = (y as f32 + 0.5) / ph as f32;
                *o = ctx.ops_px(base[y * pw as usize + x], u, v, nops);
            }
        });
    out
}

pub fn layer_sigmas(proxy: (u32, u32)) -> (f32, f32, f32) {
    let pl = proxy.0.max(proxy.1) as f32;
    (
        (0.012 * pl).max(1.0),
        (0.03 * pl).max(1.5),
        (0.006 * pl).max(1.0),
    )
}

/// Blurred luminance (two scales) and a smoothed dark channel of a proxy image.
fn layers_from(img: &[[f32; 3]], proxy: (u32, u32)) -> Layer {
    let (w, h) = (proxy.0 as usize, proxy.1 as usize);
    let (s_hs, s_cl, s_d) = layer_sigmas(proxy);
    let lumv: Vec<f32> = img.iter().map(|&c| lum(c)).collect();
    let minc: Vec<f32> = img.iter().map(|c| c[0].min(c[1]).min(c[2])).collect();
    let weights = |s: f32| -> Vec<f32> {
        let r = (3.0 * s).ceil() as i32;
        (-r..=r)
            .map(|d| (-(d * d) as f32 / (2.0 * s * s)).exp())
            .collect()
    };
    let (w_hs, w_cl, w_d) = (weights(s_hs), weights(s_cl), weights(s_d));
    // Weighted mean over a clamped window: `get` is indexed by clamped position.
    let gauss = |get: &dyn Fn(usize) -> f32, i: usize, wt: &[f32], n: usize| -> f32 {
        let r = (wt.len() as i32 - 1) / 2;
        let (mut acc, mut ws) = (0.0f32, 0.0f32);
        for (k, &wgt) in wt.iter().enumerate() {
            let p = (i as i32 + k as i32 - r).clamp(0, n as i32 - 1) as usize;
            acc += wgt * get(p);
            ws += wgt;
        }
        acc / ws
    };
    // Horizontal pass (dark channel: 5-tap min first, then blur).
    let mut tmp = vec![[0.0f32; 4]; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let m5: Vec<f32> = (0..w)
            .map(|i| {
                (-2i32..=2)
                    .map(|j| minc[y * w + (i as i32 + j).clamp(0, w as i32 - 1) as usize])
                    .fold(f32::MAX, f32::min)
            })
            .collect();
        let l = |i: usize| lumv[y * w + i];
        let m = |i: usize| m5[i];
        for (x, o) in row.iter_mut().enumerate() {
            *o = [
                gauss(&l, x, &w_hs, w),
                gauss(&l, x, &w_cl, w),
                gauss(&m, x, &w_d, w),
                0.0,
            ];
        }
    });
    // Vertical pass.
    let vmin: Vec<f32> = (0..w * h)
        .map(|k| {
            let (x, y) = (k % w, k / w);
            (-2i32..=2)
                .map(|j| tmp[(y as i32 + j).clamp(0, h as i32 - 1) as usize * w + x][2])
                .fold(f32::MAX, f32::min)
        })
        .collect();
    let mut out = vec![[0.0f32; 4]; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let a = |i: usize| tmp[i * w + x][0];
            let b = |i: usize| tmp[i * w + x][1];
            let m = |i: usize| vmin[i * w + x];
            *o = [
                gauss(&a, y, &w_hs, h),
                gauss(&b, y, &w_cl, h),
                gauss(&m, y, &w_d, h),
                0.0,
            ];
        }
    });
    out
}

fn compute_layers(plan: &Plan, src: &RgbImage) -> Vec<Option<Layer>> {
    let mut layers: Vec<Option<Layer>> = vec![None; plan.ops.len()];
    if plan.layer_ops.is_empty() {
        return layers;
    }
    let base = proxy_base(plan, src);
    for &k in &plan.layer_ops {
        let img = render_proxy(plan, &base, &layers, k);
        layers[k] = Some(if plan.ops[k][r::KIND] == r::KIND_BEAUTY {
            beauty_layers(&img, plan.proxy, &plan.ops[k])
        } else {
            layers_from(&img, plan.proxy)
        });
    }
    layers
}

// ---------------------------------------------------------------- beauty

#[inline]
fn enc3(c: [f32; 3]) -> [f32; 3] {
    [srgb_enc(c[0]), srgb_enc(c[1]), srgb_enc(c[2])]
}

#[inline]
fn dec3(e: [f32; 3]) -> [f32; 3] {
    [srgb_dec(e[0]), srgb_dec(e[1]), srgb_dec(e[2])]
}

/// Bilinear 4-channel sample of a feature map stored in the planes.
fn feat_at(planes: &[f32], off: usize, fw: u32, fh: u32, u: f32, v: f32) -> [f32; 4] {
    let (x0, x1, fx) = bilerp_idx(u, fw);
    let (y0, y1, fy) = bilerp_idx(v, fh);
    let w = fw as usize;
    let mut o = [0.0f32; 4];
    for (k, ov) in o.iter_mut().enumerate() {
        let at = |x: usize, y: usize| planes[off + (y * w + x) * 4 + k];
        let t = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * fx;
        let b = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * fx;
        *ov = t + (b - t) * fy;
    }
    o
}

const S: f32 = std::f32::consts::FRAC_1_SQRT_2;
const RING: [[f32; 2]; 8] = [
    [1.0, 0.0],
    [S, S],
    [0.0, 1.0],
    [-S, S],
    [-1.0, 0.0],
    [-S, -S],
    [0.0, -1.0],
    [S, -S],
];
const TAU: f32 = std::f32::consts::TAU;
/// OKLab hue (radians) of a natural skin tone (50 degrees).
const SKIN_TARGET_HUE: f32 = 0.872_664_6;

impl Ctx<'_> {
    /// Portrait retouching of one pixel. `c` is linear; `m` the skin weight (0 when the
    /// op has no skin plane). Port of `beauty_px` in render.wgsl.
    fn beauty_px(
        &self,
        rec: &[f32; prep::REC],
        c: [f32; 3],
        m: f32,
        u: f32,
        v: f32,
        layer: Option<&Layer>,
    ) -> [f32; 3] {
        let fl = rec[r::FLAGS] as u32;
        let planes = &self.plan.planes;
        let proxy = self.plan.proxy;
        let px = self.plan.layer_len();
        let mut out = c;
        let skin_f = r::FB_SMOOTH | r::FB_WHITEN | r::FB_BLEM | r::FB_DARK;
        let f = if (fl & r::FB_DARK != 0 && m > 0.0) || fl & (r::FB_EYE | r::FB_TEETH) != 0 {
            feat_at(
                planes,
                rec[r::B_FEAT_OFF] as usize,
                rec[r::B_FEAT_W] as u32,
                rec[r::B_FEAT_H] as u32,
                u,
                v,
            )
        } else {
            [0.0; 4]
        };
        if fl & skin_f != 0 && m > 0.0 {
            let mut e = enc3(c);
            let mut lin = c;
            if fl & (r::FB_SMOOTH | r::FB_BLEM) != 0 {
                let l = layer.expect("beauty layer");
                let a = layer_at_base(l, 0, proxy, u, v);
                let b = layer_at_base(l, px, proxy, u, v);
                let lo = [a[0], a[1], a[2]];
                let ls = [b[0], b[1], b[2]];
                if fl & r::FB_BLEM != 0 {
                    let n = rec[r::B_BL_N] as usize;
                    let off = rec[r::B_BL_OFF] as usize;
                    let (asx, asy) = (rec[r::B_ASX], rec[r::B_ASY]);
                    for j in 0..n {
                        let bl = &planes[off + j * 4..off + j * 4 + 3];
                        let (dx, dy) = ((u - bl[0]) * asx, (v - bl[1]) * asy);
                        let d2 = dx * dx + dy * dy;
                        let rmax = bl[2] * 1.15;
                        if d2 >= rmax * rmax {
                            continue;
                        }
                        let w = 1.0 - smoothstep(0.8 * bl[2], rmax, d2.sqrt());
                        let rr = bl[2] * 1.3 + rec[r::B_RING];
                        let mut acc = [0.0f32; 3];
                        for rg in &RING {
                            let s = layer_at_base(
                                l,
                                0,
                                proxy,
                                bl[0] + rg[0] * rr / asx,
                                bl[1] + rg[1] * rr / asy,
                            );
                            acc[0] += s[0];
                            acc[1] += s[1];
                            acc[2] += s[2];
                        }
                        for k in 0..3 {
                            let fill = acc[k] * 0.125;
                            let h = (e[k] - lo[k]).clamp(-0.015, 0.015);
                            e[k] += (fill + 0.25 * h - e[k]) * w;
                        }
                    }
                }
                if fl & r::FB_SMOOTH != 0 {
                    let s = rec[r::B_SMOOTH];
                    let tex = 1.0 - 0.5 * s;
                    for k in 0..3 {
                        e[k] = lo[k] + s * (ls[k] - lo[k]) + (e[k] - lo[k]) * tex;
                    }
                }
                lin = dec3(e);
            }
            if fl & (r::FB_WHITEN | r::FB_DARK) != 0 {
                let mut lab = to_oklab(lin);
                if fl & r::FB_WHITEN != 0 {
                    let a = rec[r::B_WHITEN];
                    let hp = 1.0 - smoothstep(0.80, 0.97, lab[0]);
                    lab[0] += 0.09 * a * hp;
                    let mut chroma = lab[1].hypot(lab[2]);
                    if chroma > 1e-5 {
                        let mut hue = lab[2].atan2(lab[1]);
                        let dh = SKIN_TARGET_HUE - hue;
                        let dh = dh - TAU * ((dh + std::f32::consts::PI) / TAU).floor();
                        hue += dh.clamp(-0.12 * a, 0.12 * a);
                        chroma *= 1.0 - 0.12 * a;
                        lab[1] = chroma * hue.cos();
                        lab[2] = chroma * hue.sin();
                    }
                }
                if fl & r::FB_DARK != 0 {
                    let w = rec[r::B_DARK] * f[3];
                    let hp = 1.0 - smoothstep(0.80, 0.97, lab[0]);
                    lab[0] += 0.07 * w * hp;
                    lab[1] *= 1.0 - 0.35 * w;
                    lab[2] = lab[2] * (1.0 - 0.35 * w) + 0.018 * w;
                }
                lin = from_oklab(lab);
            }
            let lin = [
                lin[0].clamp(0.0, 1.0),
                lin[1].clamp(0.0, 1.0),
                lin[2].clamp(0.0, 1.0),
            ];
            out = [
                c[0] + (lin[0] - c[0]) * m,
                c[1] + (lin[1] - c[1]) * m,
                c[2] + (lin[2] - c[2]) * m,
            ];
        }
        if fl & (r::FB_EYE | r::FB_TEETH) != 0 && (f[0] > 0.0 || f[1] > 0.0 || f[2] > 0.0) {
            let mut lab = to_oklab(out);
            let hp = 1.0 - smoothstep(0.90, 1.02, lab[0]);
            if fl & r::FB_EYE != 0 {
                let a = rec[r::B_EYE];
                let (wx, wy) = (f[0] * a, f[1] * a);
                lab[0] += (0.10 * wx + 0.05 * wy) * hp;
                let cs = 1.0 - 0.3 * wx + 0.15 * wy;
                lab[1] *= cs;
                lab[2] *= cs;
            }
            if fl & r::FB_TEETH != 0 {
                let w = f[2] * rec[r::B_TEETH];
                lab[0] += 0.08 * w * hp;
                lab[1] *= 1.0 - 0.25 * w;
                lab[2] *= 1.0 - 0.6 * w;
            }
            let o = from_oklab(lab);
            out = [
                o[0].clamp(0.0, 1.0),
                o[1].clamp(0.0, 1.0),
                o[2].clamp(0.0, 1.0),
            ];
        }
        out
    }
}

/// Frequency-separation layers of a beauty op on the proxy grid (two slots, rgb in
/// sRGB-encoded space): slot 0 = Gaussian low-pass `Lo` (sigma ~ face size), slot 1 = the
/// edge-aware (bilateral) smoothed low band `Ls`, computed inside the op's rect only.
fn beauty_layers(img: &[[f32; 3]], proxy: (u32, u32), rec: &[f32; prep::REC]) -> Layer {
    let (w, h) = (proxy.0 as usize, proxy.1 as usize);
    let sig = rec[r::B_SIGLO];
    let rad = rec[r::B_RAD] as i32;
    let sr = rec[r::B_RANGE];
    let ss_ = (rad as f32 * 0.5).max(1.0);
    let rect = &rec[r::B_RECT..r::B_RECT + 4];
    let (x0, x1) = (
        ((rect[0] * w as f32).floor() as usize).min(w),
        ((rect[2] * w as f32).ceil() as usize).min(w),
    );
    let (y0, y1) = (
        ((rect[1] * h as f32).floor() as usize).min(h),
        ((rect[3] * h as f32).ceil() as usize).min(h),
    );
    let e: Vec<[f32; 3]> = img.iter().map(|&c| enc3(c)).collect();
    let r_g = (3.0 * sig).ceil() as i32;
    let wt: Vec<f32> = (-r_g..=r_g)
        .map(|d| (-(d * d) as f32 / (2.0 * sig * sig)).exp())
        .collect();
    let mut tmp = vec![[0.0f32; 3]; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let (mut acc, mut ws) = ([0.0f32; 3], 0.0f32);
            for (k, &g) in wt.iter().enumerate() {
                let xi = (x as i32 + k as i32 - r_g).clamp(0, w as i32 - 1) as usize;
                let c = e[y * w + xi];
                acc[0] += g * c[0];
                acc[1] += g * c[1];
                acc[2] += g * c[2];
                ws += g;
            }
            *o = [acc[0] / ws, acc[1] / ws, acc[2] / ws];
        }
    });
    let mut lo = vec![[0.0f32; 3]; w * h];
    lo.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let (mut acc, mut ws) = ([0.0f32; 3], 0.0f32);
            for (k, &g) in wt.iter().enumerate() {
                let yi = (y as i32 + k as i32 - r_g).clamp(0, h as i32 - 1) as usize;
                let c = tmp[yi * w + x];
                acc[0] += g * c[0];
                acc[1] += g * c[1];
                acc[2] += g * c[2];
                ws += g;
            }
            *o = [acc[0] / ws, acc[1] / ws, acc[2] / ws];
        }
    });
    let mut out = vec![[0.0f32; 4]; 2 * w * h];
    let (a, b) = out.split_at_mut(w * h);
    b.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let c = lo[y * w + x];
            if x < x0 || x >= x1 || y < y0 || y >= y1 {
                *o = [c[0], c[1], c[2], 0.0];
                continue;
            }
            let (mut acc, mut ws) = ([0.0f32; 3], 0.0f32);
            for dy in -rad..=rad {
                let yi = y as i32 + dy;
                if yi < 0 || yi >= h as i32 {
                    continue;
                }
                for dx in -rad..=rad {
                    let xi = x as i32 + dx;
                    if xi < 0 || xi >= w as i32 {
                        continue;
                    }
                    let n = lo[yi as usize * w + xi as usize];
                    let d2 = (n[0] - c[0]).powi(2) + (n[1] - c[1]).powi(2) + (n[2] - c[2]).powi(2);
                    let wgt = (-((dx * dx + dy * dy) as f32) / (2.0 * ss_ * ss_)).exp()
                        * (-d2 / (2.0 * sr * sr)).exp();
                    acc[0] += wgt * n[0];
                    acc[1] += wgt * n[1];
                    acc[2] += wgt * n[2];
                    ws += wgt;
                }
            }
            *o = [acc[0] / ws, acc[1] / ws, acc[2] / ws, 0.0];
        }
    });
    a.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let c = lo[y * w + x];
            *o = [c[0], c[1], c[2], 0.0];
        }
    });
    out
}

// ---------------------------------------------------------------- LUT, sharpen, output

pub(crate) fn lut_lookup(lut: &Lut3d, e: [f32; 3]) -> [f32; 3] {
    let n = lut.size as usize;
    let nm = (n - 1) as f32;
    let mut idx = [0usize; 3];
    let mut fr = [0.0f32; 3];
    for k in 0..3 {
        let p = e[k].clamp(0.0, 1.0) * nm;
        let i = (p.floor() as usize).min(n - 2);
        idx[k] = i;
        fr[k] = p - i as f32;
    }
    let at = |dr: usize, dg: usize, db: usize| {
        lut.data[(idx[0] + dr) + (idx[1] + dg) * n + (idx[2] + db) * n * n]
    };
    let mut o = [0.0f32; 3];
    for (k, ov) in o.iter_mut().enumerate() {
        let l = |dr, dg, db| at(dr, dg, db)[k];
        let c00 = l(0, 0, 0) + (l(1, 0, 0) - l(0, 0, 0)) * fr[0];
        let c10 = l(0, 1, 0) + (l(1, 1, 0) - l(0, 1, 0)) * fr[0];
        let c01 = l(0, 0, 1) + (l(1, 0, 1) - l(0, 0, 1)) * fr[0];
        let c11 = l(0, 1, 1) + (l(1, 1, 1) - l(0, 1, 1)) * fr[0];
        let c0 = c00 + (c10 - c00) * fr[1];
        let c1 = c01 + (c11 - c01) * fr[1];
        *ov = c0 + (c1 - c0) * fr[2];
    }
    o
}

fn finalize(plan: &Plan, c: [f32; 3]) -> [f32; 3] {
    let mut e = [srgb_enc(c[0]), srgb_enc(c[1]), srgb_enc(c[2])];
    for l in &plan.luts {
        let o = lut_lookup(&l.lut, e);
        for k in 0..3 {
            e[k] += (o[k] - e[k]) * l.amount;
        }
    }
    e
}

#[inline]
fn sharpen_px(
    c: [f32; 3],
    l: [f32; 3],
    r_: [f32; 3],
    u: [f32; 3],
    d: [f32; 3],
    amt: f32,
) -> [f32; 3] {
    let yc = lum(c);
    let blur = (4.0 * yc + lum(l) + lum(r_) + lum(u) + lum(d)) / 8.0;
    let delta = amt / 100.0 * 1.6 * (yc - blur);
    [
        (c[0] + delta).clamp(0.0, 1.0),
        (c[1] + delta).clamp(0.0, 1.0),
        (c[2] + delta).clamp(0.0, 1.0),
    ]
}

#[inline]
fn to8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

pub fn render_plan(plan: &Plan, src: &RgbImage) -> RgbImage {
    let (ow, oh) = plan.out_size();
    let layers = compute_layers(plan, src);
    let ctx = Ctx {
        plan,
        layers: &layers,
    };
    let nops = plan.ops.len();
    let g = plan.geo;
    let (w, h) = (ow as usize, oh as usize);
    let mut enc = vec![[0.0f32; 3]; w * h];
    enc.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let v = (y as f32 + 0.5) / oh as f32;
        for (x, o) in row.iter_mut().enumerate() {
            let u = (x as f32 + 0.5) / ow as f32;
            let d = plan.warp_disp(u, v);
            let c0 = geom::sample_off(
                src,
                &g,
                x as u32,
                y as u32,
                d[0] * ow as f32,
                d[1] * oh as f32,
            );
            let c = ctx.ops_px(c0, u, v, nops);
            *o = finalize(plan, c);
        }
    });
    let mut data = vec![0u8; w * h * 3];
    let amt = plan.sharpen;
    data.par_chunks_mut(w * 3).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let c = enc[y * w + x];
            let o = if amt > 0.0 {
                let l = enc[y * w + x.saturating_sub(1)];
                let r_ = enc[y * w + (x + 1).min(w - 1)];
                let u = enc[y.saturating_sub(1) * w + x];
                let d = enc[(y + 1).min(h - 1) * w + x];
                sharpen_px(c, l, r_, u, d, amt)
            } else {
                c
            };
            row[x * 3] = to8(o[0]);
            row[x * 3 + 1] = to8(o[1]);
            row[x * 3 + 2] = to8(o[2]);
        }
    });
    RgbImage {
        width: ow,
        height: oh,
        data,
    }
}

#[allow(dead_code)]
pub(crate) fn geo_of(plan: &Plan) -> Geo {
    plan.geo
}
