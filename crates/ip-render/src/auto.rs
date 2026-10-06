//! Rule-based "AI one-click" (docs/03 §6.1): outputs slider values, never pixels.
//! Deterministic and fast: works on a <= ~256x256 sub-sample of the source.

use crate::color::*;
use crate::*;

const GRID: u32 = 256;
const SKIN_TARGET_HUE: f32 = 55.0;

struct Samples {
    lin: Vec<[f32; 3]>,
    gw: usize,
    gh: usize,
}

fn subsample(src: &RgbImage) -> Samples {
    let gw = src.width.min(GRID) as usize;
    let gh = src.height.min(GRID) as usize;
    let lut = dec_lut();
    let mut lin = Vec::with_capacity(gw * gh);
    for gy in 0..gh {
        let y = (((gy as f32 + 0.5) / gh as f32) * src.height as f32) as usize;
        for gx in 0..gw {
            let x = (((gx as f32 + 0.5) / gw as f32) * src.width as f32) as usize;
            let i = (y.min(src.height as usize - 1) * src.width as usize
                + x.min(src.width as usize - 1))
                * 3;
            lin.push([
                lut[src.data[i] as usize],
                lut[src.data[i + 1] as usize],
                lut[src.data[i + 2] as usize],
            ]);
        }
    }
    Samples { lin, gw, gh }
}

struct Hist {
    bins: Vec<u32>,
    n: u32,
}

const NB: usize = 1024;

impl Hist {
    fn new() -> Hist {
        Hist {
            bins: vec![0; NB],
            n: 0,
        }
    }
    fn add(&mut self, enc: f32) {
        let i = ((enc.clamp(0.0, 1.0) * (NB - 1) as f32) + 0.5) as usize;
        self.bins[i] += 1;
        self.n += 1;
    }
    /// Percentile (0..1) of the encoded value.
    fn pct(&self, p: f32) -> f32 {
        if self.n == 0 {
            return 0.0;
        }
        let target = (p * self.n as f32).ceil().max(1.0) as u32;
        let mut acc = 0;
        for (i, &b) in self.bins.iter().enumerate() {
            acc += b;
            if acc >= target {
                return i as f32 / (NB - 1) as f32;
            }
        }
        1.0
    }
    fn frac_above(&self, e: f32) -> f32 {
        let i = (e * (NB - 1) as f32) as usize;
        self.bins[i + 1..].iter().sum::<u32>() as f32 / self.n.max(1) as f32
    }
    fn frac_below(&self, e: f32) -> f32 {
        let i = (e * (NB - 1) as f32) as usize;
        self.bins[..i].iter().sum::<u32>() as f32 / self.n.max(1) as f32
    }
}

fn dead(x: f32, zone: f32) -> f32 {
    // Soft dead zone: removes |x| < zone, continuous.
    if x.abs() <= zone {
        0.0
    } else {
        x - zone * x.signum()
    }
}

fn rnd(x: f32, step: f32) -> f32 {
    (x / step).round() * step
}

/// Colour cast of a linear RGB triple under a white-balance setting: returns the hue
/// (OKLCh degrees) of the colour after applying the WB matrix.
fn hue_after(m: &[f32; 9], c: [f32; 3]) -> (f32, f32) {
    let o = [
        (m[0] * c[0] + m[1] * c[1] + m[2] * c[2]).max(0.0),
        (m[3] * c[0] + m[4] * c[1] + m[5] * c[2]).max(0.0),
        (m[6] * c[0] + m[7] * c[1] + m[8] * c[2]).max(0.0),
    ];
    let lab = to_oklab(o);
    let mut h = lab[2].atan2(lab[1]).to_degrees();
    if h < 0.0 {
        h += 360.0;
    }
    (h, lab[1].hypot(lab[2]))
}

/// Solve (temp, tint) so that a grey card lit with the observed cast is neutralised:
/// `gains` are the per-channel multipliers (relative) that would neutralise the cast.
fn solve_wb(gains: [f32; 3]) -> (f32, f32) {
    let apply = |t: f32, ti: f32| {
        let m = wb_matrix(t, ti);
        [m[0] + m[1] + m[2], m[3] + m[4] + m[5], m[6] + m[7] + m[8]]
    };
    // Temperature: match the R/B ratio.
    let want_rb = (gains[0] / gains[2]).clamp(0.3, 3.3);
    let (mut lo, mut hi) = (-3000.0f32, 3000.0f32);
    for _ in 0..22 {
        let mid = 0.5 * (lo + hi);
        let g = apply(mid, 0.0);
        if g[0] / g[2] < want_rb {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let temp = 0.5 * (lo + hi);
    // Tint: match green vs sqrt(R*B).
    let want_g = (gains[1] * gains[1] / (gains[0] * gains[2]))
        .sqrt()
        .clamp(0.5, 2.0);
    let (mut lo, mut hi) = (-100.0f32, 100.0f32);
    for _ in 0..22 {
        let mid = 0.5 * (lo + hi);
        let g = apply(temp, mid);
        let have = g[1] / (g[0] * g[2]).sqrt();
        // Positive tint (magenta) lowers green.
        if have > want_g {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (temp, 0.5 * (lo + hi))
}

pub fn auto_adjust(source: &RgbImage, ctx: &AutoContext, mode: AutoMode) -> Adjust {
    let mut out = Adjust {
        source: Some("ai_auto@1".into()),
        ..Adjust::default()
    };
    if source.width == 0
        || source.height == 0
        || source.data.len() < (source.width * source.height * 3) as usize
    {
        return out;
    }
    let s = subsample(source);
    let scene = ctx.scene_type.as_deref().unwrap_or("other");
    let night = scene == "night";
    let portrait = match mode {
        AutoMode::Portrait => true,
        AutoMode::Landscape => false,
        AutoMode::Auto => {
            matches!(scene, "portrait" | "group" | "pet")
                || (!ctx.faces.is_empty() && scene != "landscape")
        }
    };
    let landscape = match mode {
        AutoMode::Landscape => true,
        AutoMode::Portrait => false,
        AutoMode::Auto => matches!(scene, "landscape"),
    };

    // ---- histogram of encoded luminance
    let mut hist = Hist::new();
    for c in &s.lin {
        hist.add(srgb_enc(lum(*c)));
    }
    let p_lo = hist.pct(0.005);
    let p05 = hist.pct(0.05);
    let p50 = hist.pct(0.5);
    let p95 = hist.pct(0.95);
    let p_hi = hist.pct(0.995);

    // ---- face statistics
    let mut face_lin: Vec<[f32; 3]> = Vec::new();
    for f in &ctx.faces {
        let (x0, y0) = (f[0] + f[2] * 0.25, f[1] + f[3] * 0.25);
        let (x1, y1) = (f[0] + f[2] * 0.75, f[1] + f[3] * 0.75);
        let (gx0, gx1) = (
            (x0.clamp(0.0, 1.0) * s.gw as f32) as usize,
            ((x1.clamp(0.0, 1.0) * s.gw as f32).ceil() as usize).min(s.gw),
        );
        let (gy0, gy1) = (
            (y0.clamp(0.0, 1.0) * s.gh as f32) as usize,
            ((y1.clamp(0.0, 1.0) * s.gh as f32).ceil() as usize).min(s.gh),
        );
        for gy in gy0..gy1 {
            for gx in gx0..gx1 {
                face_lin.push(s.lin[gy * s.gw + gx]);
            }
        }
    }
    face_lin.retain(|c| {
        let e = srgb_enc(lum(*c));
        e > 0.04 && e < 0.97
    });
    let face_enc_median = if face_lin.len() >= 8 {
        let mut h = Hist::new();
        for c in &face_lin {
            h.add(srgb_enc(lum(*c)));
        }
        Some(h.pct(0.5))
    } else {
        None
    };

    // ---- exposure
    let target_mid = if night { 0.20 } else { 0.46 };
    let ev_for = |from: f32, to: f32| (srgb_dec(to).max(1e-4) / srgb_dec(from).max(1e-4)).log2();
    let ev_global = ev_for(p50.max(0.02), target_mid);
    let mut ev = ev_global * 0.7;
    if let Some(fm) = face_enc_median {
        let ev_face = ev_for(fm.max(0.03), 0.52);
        ev = 0.65 * ev_face * 0.8 + 0.35 * ev;
    }
    if night {
        ev = ev.clamp(-0.6, 0.5);
    } else {
        ev = ev.clamp(-1.5, 2.0);
    }
    // Keep the 99.5th percentile from running far past white.
    let p_hi_lin = srgb_dec(p_hi).max(1e-3);
    if ev > 0.0 {
        ev = ev.min((1.6 / p_hi_lin).log2().max(0.0));
    }
    ev = dead(ev, 0.06);
    out.exposure = rnd(ev, 0.01);

    // ---- distribution after exposure
    let gain = ev.exp2();
    let after = |e: f32| srgb_enc(srgb_dec(e) * gain);
    let (a_lo, a_05, a_95, a_hi) = (after(p_lo), after(p05), after(p95), after(p_hi));
    let spread = a_95 - a_05;

    // whites / blacks
    let mut whites = dead((0.975 - a_hi) * 150.0, 4.0).clamp(-25.0, 25.0);
    let mut blacks = dead((0.02 - a_lo) * 250.0, 4.0).clamp(-25.0, 20.0);
    if night {
        blacks = blacks.min(0.0);
        whites *= 0.5;
    }
    out.whites = rnd(whites, 1.0);
    out.blacks = rnd(blacks, 1.0);

    // highlights / shadows
    let frac_hi = hist.frac_above(after_inv(0.92, gain));
    let frac_lo = hist.frac_below(after_inv(0.12, gain));
    let mut highlights = -(frac_hi * 160.0).clamp(0.0, 45.0);
    let mut shadows = (frac_lo * 120.0).clamp(0.0, 45.0);
    if let Some(fm) = face_enc_median {
        let frame = after(p50);
        let face = after(fm);
        if face < frame - 0.03 {
            // Backlit subject: lift shadows towards the faces.
            shadows += ((frame - face) * 220.0).clamp(0.0, 40.0);
        }
    }
    if night {
        shadows = shadows.min(12.0);
    }
    out.highlights = rnd(dead(highlights, 3.0), 1.0);
    out.shadows = rnd(dead(shadows.clamp(0.0, 70.0), 3.0), 1.0);
    highlights = out.highlights;
    let _ = highlights;

    // contrast
    let mut contrast = if spread < 0.55 {
        ((0.55 - spread) * 60.0).min(20.0)
    } else if spread > 0.9 {
        -((spread - 0.9) * 80.0).min(12.0)
    } else {
        0.0
    };
    if portrait {
        contrast = contrast * 0.5 - 6.0;
    }
    if night {
        contrast *= 0.5;
    }
    out.contrast = rnd(contrast, 1.0);

    // ---- white balance: grey-world + white-patch, skin anchored
    let (temp, tint) = estimate_wb(&s, &face_lin, night);
    out.temp = rnd(temp, 10.0);
    out.tint = rnd(tint, 1.0);

    // ---- colour and clarity rules
    let mut chroma_sum = 0.0f32;
    for c in &s.lin {
        let lab = to_oklab(*c);
        chroma_sum += lab[1].hypot(lab[2]);
    }
    let mean_chroma = chroma_sum / s.lin.len() as f32;
    let mut vibrance: f32 = if mean_chroma < 0.04 {
        10.0
    } else if mean_chroma > 0.12 {
        0.0
    } else {
        4.0
    };
    let mut clarity: f32 = 4.0;
    let mut dehaze = 0.0;
    let haze = haze_level(&s);
    if landscape {
        vibrance += 14.0;
        clarity += 8.0;
        dehaze = ((haze - 0.30) * 120.0).clamp(0.0, 35.0);
        out.saturation = 3.0;
    } else if portrait {
        vibrance = vibrance.min(6.0);
        clarity = -8.0;
        dehaze = 0.0;
    } else if scene == "food" {
        vibrance += 8.0;
        clarity += 4.0;
    } else if scene == "architecture" {
        clarity += 6.0;
        dehaze = ((haze - 0.32) * 100.0).clamp(0.0, 20.0);
    } else {
        dehaze = ((haze - 0.34) * 100.0).clamp(0.0, 25.0);
    }
    if night {
        vibrance = vibrance.min(6.0);
        clarity = clarity.min(2.0);
        dehaze = 0.0;
    }
    out.vibrance = rnd(vibrance, 1.0);
    out.clarity = rnd(clarity, 1.0);
    out.dehaze = rnd(dehaze, 1.0);
    out
}

/// Inverse of "encode after exposure" for a fixed encoded threshold.
fn after_inv(enc_after: f32, gain: f32) -> f32 {
    srgb_enc(srgb_dec(enc_after) / gain)
}

/// Mean dark-channel (encoded) over 5x5 patches of the sub-sample: high = hazy.
fn haze_level(s: &Samples) -> f32 {
    let (w, h) = (s.gw, s.gh);
    let minc: Vec<f32> = s.lin.iter().map(|c| c[0].min(c[1]).min(c[2])).collect();
    let mut acc = 0.0f64;
    let mut n = 0u32;
    let step = 4usize;
    let mut y = 0;
    while y + 5 <= h {
        let mut x = 0;
        while x + 5 <= w {
            let mut m = f32::MAX;
            for j in 0..5 {
                for i in 0..5 {
                    m = m.min(minc[(y + j) * w + x + i]);
                }
            }
            acc += m as f64;
            n += 1;
            x += step;
        }
        y += step;
    }
    if n == 0 {
        return 0.0;
    }
    srgb_enc((acc / n as f64) as f32)
}

fn estimate_wb(s: &Samples, face_lin: &[[f32; 3]], night: bool) -> (f32, f32) {
    // Grey-world over unclipped, not-too-saturated pixels.
    let mut sum = [0.0f64; 3];
    let mut n = 0u32;
    let mut bright: Vec<(f32, [f32; 3])> = Vec::new();
    for c in &s.lin {
        let y = lum(*c);
        let e = srgb_enc(y);
        if !(0.05..=0.97).contains(&e) {
            continue;
        }
        let mx = c[0].max(c[1]).max(c[2]);
        let mn = c[0].min(c[1]).min(c[2]);
        if (mx - mn) / mx.max(1e-6) > 0.65 {
            continue;
        }
        for k in 0..3 {
            sum[k] += c[k] as f64;
        }
        n += 1;
        bright.push((y, *c));
    }
    if n < 32 {
        return (0.0, 0.0);
    }
    let mean = [
        (sum[0] / n as f64) as f32,
        (sum[1] / n as f64) as f32,
        (sum[2] / n as f64) as f32,
    ];
    let gw_gains = [
        mean[1] / mean[0].max(1e-4),
        1.0,
        mean[1] / mean[2].max(1e-4),
    ];
    // White-patch: brightest 3 % of the usable pixels.
    bright.sort_by(|a, b| b.0.total_cmp(&a.0));
    let top = (bright.len() / 33).max(8).min(bright.len());
    let mut wp = [0.0f64; 3];
    for (_, c) in &bright[..top] {
        for k in 0..3 {
            wp[k] += c[k] as f64;
        }
    }
    let wp = [wp[0] as f32, wp[1] as f32, wp[2] as f32];
    let wp_gains = [wp[1] / wp[0].max(1e-4), 1.0, wp[1] / wp[2].max(1e-4)];
    let wp_sat = (wp[0].max(wp[1]).max(wp[2]) - wp[0].min(wp[1]).min(wp[2]))
        / wp[0].max(wp[1]).max(wp[2]).max(1e-6);
    let wwp = if wp_sat < 0.25 { 0.4 } else { 0.0 };
    let mut gains = [0.0f32; 3];
    for k in 0..3 {
        gains[k] =
            (gw_gains[k].max(1e-3).ln() * (1.0 - wwp) + wp_gains[k].max(1e-3).ln() * wwp).exp();
    }
    // Damp the correction: grey-world is wrong on dominant-colour scenes.
    let strength = if night { 0.25 } else { 0.5 };
    for g in gains.iter_mut() {
        *g = g.powf(strength);
    }
    let (mut temp, mut tint) = solve_wb(gains);
    temp = dead(temp, 120.0).clamp(-900.0, 900.0);
    tint = dead(tint, 4.0).clamp(-15.0, 15.0);

    // Skin anchoring.
    if face_lin.len() >= 8 {
        let mut skin = [0.0f64; 3];
        for c in face_lin {
            for k in 0..3 {
                skin[k] += c[k] as f64;
            }
        }
        let skin = [
            (skin[0] / face_lin.len() as f64) as f32,
            (skin[1] / face_lin.len() as f64) as f32,
            (skin[2] / face_lin.len() as f64) as f32,
        ];
        let (t0, ti0) = (temp, tint);
        let mut best = (f32::MAX, t0, ti0);
        let mut dt = -600.0f32;
        while dt <= 600.0 {
            let mut dti = -20.0f32;
            while dti <= 20.0 {
                let (t, ti) = (
                    (t0 + dt).clamp(-3000.0, 3000.0),
                    (ti0 + dti).clamp(-100.0, 100.0),
                );
                let m = wb_matrix(t, ti);
                let (h, c) = hue_after(&m, skin);
                let mut dh = (h - SKIN_TARGET_HUE).abs();
                if dh > 180.0 {
                    dh = 360.0 - dh;
                }
                let cost = (dh / 8.0).powi(2)
                    + (((0.045 - c).max(0.0)) / 0.02).powi(2)
                    + (dt / 450.0).powi(2)
                    + (dti / 12.0).powi(2);
                if cost < best.0 {
                    best = (cost, t, ti);
                }
                dti += 4.0;
            }
            dt += 100.0;
        }
        temp = best.1;
        tint = best.2;
    }
    (temp.clamp(-1500.0, 1500.0), tint.clamp(-30.0, 30.0))
}
