//! Adaptive presets (docs/03 §6.1.1): a built-in preset states its tonal intent as targets
//! (subject level, allowed exposure range, highlight budget) instead of fixed offsets, and is
//! resolved against the photo when it is applied. The result is an ordinary edit stack with
//! concrete slider values, so renders and exports stay deterministic and the user can keep
//! tweaking the sliders.
//!
//! Resolution is closed-loop: the CPU reference renderer renders candidate stacks on a small
//! (<= 256 px) proxy and the solver measures the result, so every op of the look (curves,
//! HSL luminance, LUT, clarity) is accounted for without a separate tone model.

use crate::color::{dec_lut, lum, srgb_enc};
use crate::presets::{builtin_specs, PresetSpec, ToneIntent};
use crate::*;

/// Long edge of the measuring proxy.
const MEASURE_EDGE: u32 = 256;
/// sRGB-encoded luminance at or above which a pixel counts as clipped (250/255).
pub(crate) const CLIP_LEVEL: f32 = 250.0 / 255.0;
/// Share of face pixels a preset may newly clip.
const FACE_NEW_CLIP: f32 = 0.002;
/// Subject level differences below this (encoded) leave exposure at 0.
const EV_DEAD_ZONE: f32 = 0.015;
const NB: usize = 1024;

// ------------------------------------------------------------------ measuring

/// Box-downscale so the long edge is <= `edge` (integer factor; identity when already small).
pub(crate) fn downscale(src: &RgbImage, edge: u32) -> RgbImage {
    let long = src.width.max(src.height);
    if long <= edge || src.width == 0 || src.height == 0 {
        return src.clone();
    }
    let f = long.div_ceil(edge);
    let (ow, oh) = ((src.width / f).max(1), (src.height / f).max(1));
    let mut data = Vec::with_capacity((ow * oh * 3) as usize);
    for oy in 0..oh {
        for ox in 0..ow {
            let mut acc = [0u32; 3];
            for y in oy * f..((oy + 1) * f).min(src.height) {
                for x in ox * f..((ox + 1) * f).min(src.width) {
                    let i = ((y * src.width + x) * 3) as usize;
                    for (a, v) in acc.iter_mut().zip(&src.data[i..i + 3]) {
                        *a += *v as u32;
                    }
                }
            }
            let n = (((oy + 1) * f).min(src.height) - oy * f)
                * (((ox + 1) * f).min(src.width) - ox * f);
            for a in acc {
                data.push(((a + n / 2) / n.max(1)) as u8);
            }
        }
    }
    RgbImage {
        width: ow,
        height: oh,
        data,
    }
}

struct Hist {
    bins: Vec<u32>,
    n: u32,
    sum: f64,
    clipped: u32,
}

impl Hist {
    fn new() -> Hist {
        Hist {
            bins: vec![0; NB],
            n: 0,
            sum: 0.0,
            clipped: 0,
        }
    }
    fn add(&mut self, e: f32) {
        let e = e.clamp(0.0, 1.0);
        self.bins[((e * (NB - 1) as f32) + 0.5) as usize] += 1;
        self.n += 1;
        self.sum += e as f64;
        if e >= CLIP_LEVEL - 1e-4 {
            self.clipped += 1;
        }
    }
    fn pct(&self, p: f32) -> f32 {
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
    fn clipped_share(&self) -> f32 {
        self.clipped as f32 / self.n.max(1) as f32
    }
}

/// Tone statistics of `img` (sRGB-encoded luminance). `faces` are normalised `[x, y, w, h]`
/// boxes; their central half is measured separately.
pub fn measure_tone(img: &RgbImage, faces: &[[f32; 4]]) -> ToneStats {
    let lut = dec_lut();
    let (w, h) = (img.width as usize, img.height as usize);
    let mut all = Hist::new();
    let enc = |i: usize| {
        srgb_enc(lum([
            lut[img.data[i] as usize],
            lut[img.data[i + 1] as usize],
            lut[img.data[i + 2] as usize],
        ]))
    };
    if w == 0 || h == 0 || img.data.len() < w * h * 3 {
        return ToneStats::default();
    }
    for p in 0..w * h {
        all.add(enc(p * 3));
    }
    let mut face = Hist::new();
    for f in faces {
        let x0 = ((f[0] + f[2] * 0.25).clamp(0.0, 1.0) * w as f32) as usize;
        let x1 = (((f[0] + f[2] * 0.75).clamp(0.0, 1.0) * w as f32).ceil() as usize).min(w);
        let y0 = ((f[1] + f[3] * 0.25).clamp(0.0, 1.0) * h as f32) as usize;
        let y1 = (((f[1] + f[3] * 0.75).clamp(0.0, 1.0) * h as f32).ceil() as usize).min(h);
        for y in y0..y1 {
            for x in x0..x1 {
                face.add(enc((y * w + x) * 3));
            }
        }
    }
    let has_face = face.n >= 16;
    ToneStats {
        median: all.pct(0.5),
        mean: (all.sum / all.n.max(1) as f64) as f32,
        p01: all.pct(0.01),
        p99: all.pct(0.99),
        clipped: all.clipped_share(),
        face_median: has_face.then(|| face.pct(0.5)),
        face_clipped: has_face.then(|| face.clipped_share()),
    }
}

/// The level the exposure target applies to: faces weigh 60 % when present.
fn subject_level(s: &ToneStats) -> f32 {
    match s.face_median {
        Some(f) => 0.6 * f + 0.4 * s.median,
        None => s.median,
    }
}

// ------------------------------------------------------------------ strength

fn clamp100(v: f32) -> f32 {
    v.clamp(-100.0, 100.0)
}

fn scale_curve(pts: &[[f32; 2]], k: f32) -> Vec<[f32; 2]> {
    pts.iter()
        .map(|p| [p[0], (p[0] + (p[1] - p[0]) * k).clamp(0.0, 1.0)])
        .collect()
}

fn scale_adjust(a: &Adjust, k: f32) -> Adjust {
    let mut o = Adjust {
        exposure: (a.exposure * k).clamp(-5.0, 5.0),
        contrast: clamp100(a.contrast * k),
        highlights: clamp100(a.highlights * k),
        shadows: clamp100(a.shadows * k),
        whites: clamp100(a.whites * k),
        blacks: clamp100(a.blacks * k),
        temp: (a.temp * k).clamp(-3000.0, 3000.0),
        tint: clamp100(a.tint * k),
        vibrance: clamp100(a.vibrance * k),
        saturation: clamp100(a.saturation * k),
        clarity: clamp100(a.clarity * k),
        dehaze: clamp100(a.dehaze * k),
        curve: a.curve.as_ref().map(|c| Curves {
            rgb: scale_curve(&c.rgb, k),
            r: scale_curve(&c.r, k),
            g: scale_curve(&c.g, k),
            b: scale_curve(&c.b, k),
        }),
        hsl: a
            .hsl
            .iter()
            .map(|(b, h)| {
                (
                    *b,
                    Hsl {
                        h: clamp100(h.h * k),
                        s: clamp100(h.s * k),
                        l: clamp100(h.l * k),
                    },
                )
            })
            .collect(),
        grading: a.grading.as_ref().map(|g| Grading {
            shadows: [g.shadows[0], (g.shadows[1] * k).clamp(0.0, 1.0)],
            midtones: [g.midtones[0], (g.midtones[1] * k).clamp(0.0, 1.0)],
            highlights: [g.highlights[0], (g.highlights[1] * k).clamp(0.0, 1.0)],
            balance: clamp100(g.balance * k),
        }),
        source: a.source.clone(),
    };
    if k == 0.0 {
        o.curve = None;
        o.hsl.clear();
        o.grading = None;
    }
    o
}

fn adjust_is_neutral(a: &Adjust) -> bool {
    let nums = [
        a.exposure,
        a.contrast,
        a.highlights,
        a.shadows,
        a.whites,
        a.blacks,
        a.temp,
        a.tint,
        a.vibrance,
        a.saturation,
        a.clarity,
        a.dehaze,
    ];
    let curve_id = a.curve.as_ref().is_none_or(|c| {
        [&c.rgb, &c.r, &c.g, &c.b]
            .iter()
            .all(|pts| pts.iter().all(|p| (p[0] - p[1]).abs() < 1e-6))
    });
    nums.iter().all(|v| *v == 0.0)
        && curve_id
        && a.hsl
            .values()
            .all(|h| h.h == 0.0 && h.s == 0.0 && h.l == 0.0)
        && a.grading.as_ref().is_none_or(|g| {
            g.shadows[1] == 0.0
                && g.midtones[1] == 0.0
                && g.highlights[1] == 0.0
                && g.balance == 0.0
        })
}

/// Scale a preset's deltas by `amount` (0 ..= [`PRESET_AMOUNT_MAX`]): every slider, curve
/// offset, HSL shift and grading amount is multiplied, LUT / local / sharpen amounts too
/// (clamped to their valid ranges). Ops that become neutral are dropped; other op types are
/// kept as they are. `amount = 1` returns the stack unchanged.
pub fn scale_preset_stack(stack: &EditStack, amount: f32) -> EditStack {
    let k = if amount.is_finite() {
        amount.clamp(0.0, PRESET_AMOUNT_MAX)
    } else {
        1.0
    };
    if k == 1.0 {
        return stack.clone();
    }
    let ops = stack
        .ops
        .iter()
        .filter_map(|op| match op {
            Op::Global(a) => {
                let s = scale_adjust(a, k);
                (!adjust_is_neutral(&s)).then_some(Op::Global(s))
            }
            Op::Local(l) => {
                let amount = (l.amount * k).clamp(0.0, 1.0);
                (amount > 0.0).then(|| {
                    Op::Local(LocalAdjust {
                        amount,
                        ..l.clone()
                    })
                })
            }
            Op::Lut(l) => {
                let amount = (l.amount * k).clamp(0.0, 1.0);
                (amount > 0.0).then(|| {
                    Op::Lut(Lut {
                        amount,
                        ..l.clone()
                    })
                })
            }
            Op::OutputSharpen(s) => {
                let amount = (s.amount * k).clamp(0.0, 100.0);
                (amount > 0.0).then_some(Op::OutputSharpen(OutputSharpen { amount }))
            }
            other => Some(other.clone()),
        })
        .collect();
    EditStack {
        version: stack.version,
        ops,
    }
}

/// `preset:<id>;amount=<percent>`: the provenance a resolved preset writes into its global op.
pub fn preset_source(id: &str, amount: f32) -> String {
    format!("preset:{id};amount={}", (amount * 100.0).round() as i32)
}

/// Inverse of [`preset_source`]: `(id, amount)`; a bare `preset` / `preset:<id>` reads as 100 %.
pub fn parse_preset_source(s: &str) -> Option<(String, f32)> {
    let rest = s.strip_prefix("preset:")?;
    let (id, amount) = match rest.split_once(";amount=") {
        Some((id, a)) => (id, a.parse::<f32>().ok()? / 100.0),
        None => (rest, 1.0),
    };
    (!id.is_empty()).then(|| (id.to_string(), amount))
}

// ------------------------------------------------------------------ resolving

fn render_small(img: &RgbImage, stack: &EditStack) -> Option<RgbImage> {
    let req = RenderRequest {
        source: img,
        stack,
        masks: &NoAssets,
        luts: &NoAssets,
        max_long_edge: None,
    };
    cpu::CpuRenderer.render(&req).ok()
}

/// The look with the solved tone: `ev` exposure, and highlight protection `p` (0..1), which
/// pulls whites, highlights, positive contrast, the upper half of the curves and exposure back.
fn with_tone(look: &EditStack, ev: f32, p: f32) -> EditStack {
    let mut s = look.clone();
    let mut found = false;
    for op in s.ops.iter_mut() {
        if let Op::Global(a) = op {
            found = true;
            a.exposure = (ev - p * (ev.max(0.0) + 0.4)).clamp(-5.0, 5.0);
            if p > 0.0 {
                a.whites = clamp100(a.whites - 40.0 * p);
                a.highlights = clamp100(a.highlights - 50.0 * p);
                if a.contrast > 0.0 {
                    a.contrast *= 1.0 - 0.5 * p;
                }
                if let Some(c) = a.curve.as_mut() {
                    for pts in [&mut c.rgb, &mut c.r, &mut c.g, &mut c.b] {
                        for q in pts.iter_mut().filter(|q| q[0] >= 0.5) {
                            q[1] += (q[0] - q[1]) * p;
                        }
                    }
                }
            }
            break;
        }
    }
    if !found && (ev != 0.0 || p > 0.0) {
        s.ops.insert(
            0,
            Op::Global(Adjust {
                exposure: ev - p * (ev.max(0.0) + 0.4),
                whites: -40.0 * p,
                highlights: -50.0 * p,
                ..Adjust::default()
            }),
        );
    }
    s
}

/// Slider granularity: exposure 0.01 EV, temp 10 K, curves 0.001, everything else whole units.
fn tidy_stack(mut s: EditStack, source: String) -> EditStack {
    for op in s.ops.iter_mut() {
        match op {
            Op::Global(a) => {
                let tidied = imp::tidy(std::mem::take(a));
                *a = tidied;
                let r3 = |v: f32| (v * 1000.0).round() / 1000.0 + 0.0;
                if let Some(c) = a.curve.as_mut() {
                    for pts in [&mut c.rgb, &mut c.r, &mut c.g, &mut c.b] {
                        for q in pts.iter_mut() {
                            *q = [r3(q[0]), r3(q[1])];
                        }
                    }
                }
                for h in a.hsl.values_mut() {
                    *h = Hsl {
                        h: h.h.round() + 0.0,
                        s: h.s.round() + 0.0,
                        l: h.l.round() + 0.0,
                    };
                }
                if let Some(g) = a.grading.as_mut() {
                    for w in [&mut g.shadows, &mut g.midtones, &mut g.highlights] {
                        w[1] = (w[1] * 100.0).round() / 100.0;
                    }
                    g.balance = g.balance.round() + 0.0;
                }
                a.source = Some(source.clone());
            }
            Op::Lut(l) => l.amount = (l.amount * 100.0).round() / 100.0,
            Op::OutputSharpen(o) => o.amount = o.amount.round(),
            _ => {}
        }
    }
    s
}

fn violates(before: &ToneStats, after: &ToneStats, t: &ToneIntent) -> bool {
    if after.clipped - before.clipped > t.max_new_clip {
        return true;
    }
    if let (Some(fb), Some(fa)) = (before.face_clipped, after.face_clipped) {
        if fa - fb > FACE_NEW_CLIP {
            return true;
        }
    }
    if let (Some(mb), Some(ma)) = (before.face_median, after.face_median) {
        if ma > t.face_max.max(mb) + 0.005 {
            return true;
        }
    }
    false
}

/// Resolve a built-in preset against `source` (an upright proxy of the photo, any size: it
/// is box-downscaled to <= 256 px for measuring). `None` for an unknown id.
pub fn resolve_preset(
    id: &str,
    source: &RgbImage,
    ctx: &PresetContext,
    amount: f32,
) -> Option<PresetResolution> {
    let spec = builtin_specs().into_iter().find(|s| s.id == id)?;
    Some(resolve_spec(&spec, source, ctx, amount))
}

pub(crate) fn resolve_spec(
    spec: &PresetSpec,
    source: &RgbImage,
    ctx: &PresetContext,
    amount: f32,
) -> PresetResolution {
    let a = if amount.is_finite() {
        amount.clamp(0.0, PRESET_AMOUNT_MAX)
    } else {
        1.0
    };
    let src = preset_source(spec.id, a);
    let img = downscale(source, MEASURE_EDGE);
    let faces = &ctx.faces;
    let before = measure_tone(&img, faces);
    let look = scale_preset_stack(&spec.look, a);
    let t = &spec.tone;
    let valid =
        img.width > 0 && img.height > 0 && img.data.len() >= (img.width * img.height * 3) as usize;
    if a == 0.0 || !valid {
        let stack = tidy_stack(look, src);
        return PresetResolution {
            stack,
            amount: a,
            adaptive: true,
            exposure: 0.0,
            protection: 0.0,
            satisfied: true,
            before,
            after: before,
        };
    }
    let measure = |s: &EditStack| -> ToneStats {
        render_small(&img, s)
            .map(|o| measure_tone(&o, faces))
            .unwrap_or(before)
    };

    // ---- exposure: move the subject level part of the way to the target
    let m0 = subject_level(&before);
    let gap = t.mid - m0;
    let pull = if gap > 0.0 { t.pull_up } else { t.pull_down };
    let want = m0 + gap * (pull * a).min(1.0);
    let (lo, hi) = (t.ev.0 * a, t.ev.1 * a);
    let level = |ev: f32| subject_level(&measure(&with_tone(&look, ev, 0.0)));
    let at0 = level(0.0);
    let ev = if (want - at0).abs() < EV_DEAD_ZONE {
        0.0
    } else if want > at0 {
        if level(hi) <= want {
            hi
        } else {
            let (mut l, mut h) = (0.0f32, hi);
            for _ in 0..9 {
                let m = 0.5 * (l + h);
                if level(m) < want {
                    l = m;
                } else {
                    h = m;
                }
            }
            0.5 * (l + h)
        }
    } else if level(lo) >= want {
        lo
    } else {
        let (mut l, mut h) = (lo, 0.0f32);
        for _ in 0..9 {
            let m = 0.5 * (l + h);
            if level(m) < want {
                l = m;
            } else {
                h = m;
            }
        }
        0.5 * (l + h)
    };
    let ev = (ev * 100.0).round() / 100.0 + 0.0;

    // ---- highlight protection: the smallest pull-back that keeps the budget
    let ok = |p: f32| !violates(&before, &measure(&with_tone(&look, ev, p)), t);
    let protection = if ok(0.0) {
        0.0
    } else if !ok(1.0) {
        1.0
    } else {
        let (mut l, mut h) = (0.0f32, 1.0f32);
        for _ in 0..7 {
            let m = 0.5 * (l + h);
            if ok(m) {
                h = m;
            } else {
                l = m;
            }
        }
        h
    };

    let stack = tidy_stack(with_tone(&look, ev, protection), src);
    let after = measure(&stack);
    let exposure = stack
        .ops
        .iter()
        .find_map(|o| match o {
            Op::Global(g) => Some(g.exposure),
            _ => None,
        })
        .unwrap_or(0.0);
    PresetResolution {
        stack,
        amount: a,
        adaptive: true,
        exposure,
        protection,
        satisfied: !violates(&before, &after, t),
        before,
        after,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_round_trips() {
        assert_eq!(
            preset_source("japanese_clean", 0.8),
            "preset:japanese_clean;amount=80"
        );
        assert_eq!(
            parse_preset_source("preset:japanese_clean;amount=80"),
            Some(("japanese_clean".into(), 0.8))
        );
        assert_eq!(
            parse_preset_source("preset:user_3"),
            Some(("user_3".into(), 1.0))
        );
        assert_eq!(parse_preset_source("preset"), None);
        assert_eq!(parse_preset_source("ai_auto@1"), None);
    }

    #[test]
    fn downscale_keeps_mean_and_bounds_size() {
        let img = crate::testutil::scene(1000, 600, 3);
        let d = downscale(&img, 256);
        assert!(
            d.width <= 256 && d.height <= 256 && d.width > 100,
            "{}x{}",
            d.width,
            d.height
        );
        let (a, b) = (
            crate::testutil::mean_rgb(&img),
            crate::testutil::mean_rgb(&d),
        );
        for k in 0..3 {
            assert!((a[k] - b[k]).abs() < 2.0, "{a:?} {b:?}");
        }
    }

    #[test]
    fn scale_zero_drops_everything_and_one_is_identity() {
        for (id, _, st) in crate::presets::builtin_presets() {
            assert!(scale_preset_stack(&st, 0.0).ops.is_empty(), "{id}");
            assert_eq!(scale_preset_stack(&st, 1.0), st, "{id}");
            // 150 % stays inside the validated ranges.
            for op in scale_preset_stack(&st, 1.5).ops {
                if let Op::Lut(l) = op {
                    assert!(l.amount <= 1.0);
                }
            }
        }
    }

    #[test]
    fn protection_engages_when_the_look_would_blow_highlights() {
        // An aggressive look on a bright scene: without protection it clips a lot.
        let spec = PresetSpec {
            id: "test_hot",
            look: EditStack {
                version: 1,
                ops: vec![Op::Global(Adjust {
                    whites: 60.0,
                    contrast: 30.0,
                    curve: Some(Curves {
                        rgb: vec![[0.0, 0.0], [0.6, 0.75], [1.0, 1.0]],
                        ..Default::default()
                    }),
                    ..Default::default()
                })],
            },
            tone: ToneIntent {
                mid: 0.9,
                pull_up: 1.0,
                pull_down: 1.0,
                ev: (-1.0, 2.0),
                max_new_clip: 0.005,
                face_max: 0.85,
            },
        };
        let img = crate::testutil::tint(&crate::testutil::scene(320, 220, 5), [1.2, 1.2, 1.2]);
        let ctx = PresetContext::default();
        let r = resolve_spec(&spec, &img, &ctx, 1.0);
        let unprotected = measure_tone(
            &render_small(
                &downscale(&img, MEASURE_EDGE),
                &with_tone(&spec.look, 0.0, 0.0),
            )
            .unwrap(),
            &[],
        );
        assert!(
            unprotected.clipped - r.before.clipped > 0.005,
            "test look should clip without protection: {unprotected:?}"
        );
        assert!(r.protection > 0.0, "{r:?}");
        assert!(r.satisfied, "{r:?}");
        assert!(r.after.clipped - r.before.clipped <= 0.005 + 1e-6, "{r:?}");
    }
}
