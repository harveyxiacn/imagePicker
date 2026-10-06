//! Copying edits between photos (`POST /api/edits/sync`) and the adaptive exposure /
//! white-balance matching of docs/03 section 6.2.
//!
//! Adaptive matching is deliberately simple and closed-loop, so it does not depend on the
//! exact slider-to-pixel response of the renderer:
//!
//! 1. Both photos are rendered on a small proxy with the *synced* ops (crop removed, so whole
//!    frames are compared): the source as is, the target with the copied ops.
//! 2. Each render is summarised by [`Stats`]: the subject-weighted mean *linear* luminance (a
//!    centre-weighted Gaussian plus extra weight inside detected faces) and the "grey point"
//!    (weighted mean linear RGB of the non-clipped pixels, expressed as the log ratios
//!    `ln(R/B)` for temperature and `ln(G/sqrt(RB))` for tint).
//! 3. The target's global `exposure`, `temp`, `tint` are corrected by the difference
//!    (`d_ev = log2(Y_src / Y_tgt)`, `temp += d_rb * K_PER_LN`, `tint -= d_gm * T_PER_LN`), damped,
//!    clamped to a window around the copied values, and re-rendered; up to [`MAX_STEPS`]
//!    corrections, the best (lowest error) candidate wins, so the result is never worse than
//!    the plain copy.

use ip_render::{Adjust, RgbImage};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncKind {
    Crop,
    Global,
    Local,
    Lut,
    OutputSharpen,
    Beauty,
    Warp,
}

impl SyncKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Crop => "crop",
            Self::Global => "global",
            Self::Local => "local",
            Self::Lut => "lut",
            Self::OutputSharpen => "output_sharpen",
            Self::Beauty => "beauty",
            Self::Warp => "warp",
        }
    }
}

pub fn op_type(op: &Value) -> Option<&str> {
    op.get("type").and_then(Value::as_str)
}

/// Canonical pipeline position of an op type; unknown (future) ops keep to the end.
pub(crate) fn rank(op: &Value) -> u8 {
    match op_type(op) {
        Some("crop") => 0,
        Some("warp") => 1,
        Some("global") => 2,
        Some("local") => 3,
        Some("beauty") => 4,
        Some("lut") => 5,
        Some("output_sharpen") => 6,
        _ => 7,
    }
}

/// The person an op is scoped to: `person_id` of a beauty/warp op or of a local op with an AI
/// mask. `None` = applies to everyone / not person specific.
pub fn op_person(op: &Value) -> Option<i64> {
    match op_type(op) {
        Some("beauty") | Some("warp") => op.get("person_id").and_then(Value::as_i64),
        Some("local") => {
            let m = op.get("mask")?;
            if m.get("kind").and_then(Value::as_str) == Some("ai") {
                m.get("person_id").and_then(Value::as_i64)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Target ops with the `include`d kinds replaced by the source's ops of those kinds.
pub fn merge_ops(target: &[Value], source: &[Value], include: &[SyncKind]) -> Vec<Value> {
    merge_ops_scoped(target, source, include, None)
}

/// [`merge_ops`] for a target whose people are known: source ops scoped to a person
/// (see [`op_person`]) are copied only when that person appears in the target.
pub fn merge_ops_scoped(
    target: &[Value],
    source: &[Value],
    include: &[SyncKind],
    target_people: Option<&std::collections::HashSet<i64>>,
) -> Vec<Value> {
    let wanted = |op: &Value| op_type(op).is_some_and(|t| include.iter().any(|k| k.as_str() == t));
    let applies = |op: &Value| match (op_person(op), target_people) {
        (Some(p), Some(people)) => people.contains(&p),
        _ => true,
    };
    let mut out: Vec<Value> = target.iter().filter(|o| !wanted(o)).cloned().collect();
    out.extend(source.iter().filter(|o| wanted(o) && applies(o)).cloned());
    out.sort_by_key(rank); // stable
    out
}

// ------------------------------------------------------------------ adaptive matching

pub const MAX_STEPS: usize = 4;
/// Kelvin per unit of `ln(R/B)` (initial guess; the loop corrects residual errors).
const K_PER_LN: f64 = 2500.0;
/// Tint units per unit of `ln(G/sqrt(RB))`.
const T_PER_LN: f64 = 150.0;
const DAMPING: f64 = 0.75;
/// How far the adaptation may move away from the copied values.
const MAX_EV_DELTA: f64 = 2.5;
const MAX_TEMP_DELTA: f64 = 1500.0;
const MAX_TINT_DELTA: f64 = 30.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stats {
    /// Subject-weighted mean linear luminance.
    pub lum: f64,
    /// Weighted mean linear RGB of non-clipped pixels (the grey point).
    pub rgb: [f64; 3],
}

fn srgb_to_linear(v: u8) -> f64 {
    let c = v as f64 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Subject weight at normalised `(x, y)`: a centre-weighted Gaussian floor plus a strong bonus
/// inside (slightly grown) face boxes.
pub fn subject_weight(x: f32, y: f32, faces: &[[f32; 4]]) -> f64 {
    let (dx, dy) = ((x - 0.5) as f64, (y - 0.5) as f64);
    let mut w = 0.3 + (-(dx * dx + dy * dy) / (2.0 * 0.3 * 0.3)).exp();
    for f in faces {
        let (gx, gy) = (f[2] * 0.2, f[3] * 0.2);
        if x >= f[0] - gx && x <= f[0] + f[2] + gx && y >= f[1] - gy && y <= f[1] + f[3] + gy {
            w += 3.0;
            break;
        }
    }
    w
}

pub fn measure(img: &RgbImage, faces: &[[f32; 4]]) -> Stats {
    let lin: Vec<f64> = (0..=255u8).map(srgb_to_linear).collect();
    let (w, h) = (img.width.max(1) as usize, img.height.max(1) as usize);
    // sample at most ~160k pixels
    let step = ((w * h) as f64 / 160_000.0).sqrt().ceil().max(1.0) as usize;
    let (mut wsum, mut lsum) = (0.0, 0.0);
    let (mut csum, mut rgb) = (0.0, [0.0f64; 3]);
    let mut y = 0;
    while y < h {
        let mut x = 0;
        while x < w {
            let i = (y * w + x) * 3;
            if i + 2 < img.data.len() {
                let (r, g, b) = (
                    lin[img.data[i] as usize],
                    lin[img.data[i + 1] as usize],
                    lin[img.data[i + 2] as usize],
                );
                let wt = subject_weight(
                    (x as f32 + 0.5) / w as f32,
                    (y as f32 + 0.5) / h as f32,
                    faces,
                );
                let lum = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                wsum += wt;
                lsum += wt * lum;
                if (0.01..0.98).contains(&lum) {
                    csum += wt;
                    rgb[0] += wt * r;
                    rgb[1] += wt * g;
                    rgb[2] += wt * b;
                }
            }
            x += step;
        }
        y += step;
    }
    let lum = if wsum > 0.0 { lsum / wsum } else { 0.0 };
    let rgb = if csum > 0.0 {
        [rgb[0] / csum, rgb[1] / csum, rgb[2] / csum]
    } else {
        [lum; 3]
    };
    Stats { lum, rgb }
}

/// `(d_ev, d_rb, d_gm)`: how far `cur` is from `target`; positive = `target` is brighter /
/// warmer (more red than blue) / greener.
fn errors(target: &Stats, cur: &Stats) -> (f64, f64, f64) {
    const EPS: f64 = 1e-4;
    let l = |s: &Stats| s.lum.max(EPS);
    let rb = |s: &Stats| (s.rgb[0].max(EPS) / s.rgb[2].max(EPS)).ln();
    let gm = |s: &Stats| (s.rgb[1].max(EPS) / (s.rgb[0].max(EPS) * s.rgb[2].max(EPS)).sqrt()).ln();
    (
        (l(target) / l(cur)).log2(),
        rb(target) - rb(cur),
        gm(target) - gm(cur),
    )
}

fn cost(e: (f64, f64, f64)) -> f64 {
    e.0.abs() + 2.0 * e.1.abs() + 2.0 * e.2.abs()
}

/// Adjusts `start`'s exposure/temp/tint until `eval` (renders the target with an adjust and
/// measures it) matches `source`. Returns the best adjust found (possibly `start` itself).
pub fn solve(
    source: &Stats,
    start: &Adjust,
    mut eval: impl FnMut(&Adjust) -> anyhow::Result<Stats>,
) -> anyhow::Result<Adjust> {
    let mut adj = start.clone();
    let mut best = (f64::MAX, adj.clone());
    for step in 0..=MAX_STEPS {
        let st = eval(&adj)?;
        let e = errors(source, &st);
        let c = cost(e);
        if c < best.0 {
            best = (c, adj.clone());
        }
        if (e.0.abs() < 0.02 && e.1.abs() < 0.01 && e.2.abs() < 0.01) || step == MAX_STEPS {
            break;
        }
        // temp: more red than blue in the source -> warmer (+)
        adj.exposure = (adj.exposure as f64 + e.0 * DAMPING)
            .clamp(
                start.exposure as f64 - MAX_EV_DELTA,
                start.exposure as f64 + MAX_EV_DELTA,
            )
            .clamp(-5.0, 5.0) as f32;
        adj.temp = (adj.temp as f64 + e.1 * K_PER_LN * DAMPING)
            .clamp(
                start.temp as f64 - MAX_TEMP_DELTA,
                start.temp as f64 + MAX_TEMP_DELTA,
            )
            .clamp(-3000.0, 3000.0) as f32;
        // tint: green(-) .. magenta(+); source greener than target -> negative
        adj.tint = (adj.tint as f64 - e.2 * T_PER_LN * DAMPING)
            .clamp(
                start.tint as f64 - MAX_TINT_DELTA,
                start.tint as f64 + MAX_TINT_DELTA,
            )
            .clamp(-100.0, 100.0) as f32;
    }
    Ok(best.1)
}

// ------------------------------------------------------------------ raw stack helpers

/// The first `global` op parsed as an [`Adjust`] (defaults when there is none).
pub fn first_global(ops: &[Value]) -> Adjust {
    ops.iter()
        .find(|o| op_type(o) == Some("global"))
        .and_then(|o| serde_json::from_value(o.clone()).ok())
        .unwrap_or_default()
}

fn round(v: f32, digits: i32) -> f64 {
    let m = 10f64.powi(digits);
    (v as f64 * m).round() / m
}

/// Writes exposure/temp/tint into the first `global` op (creating one when needed), leaving
/// every other field of the op untouched.
pub fn set_global_wb(ops: &mut Vec<Value>, adj: &Adjust) {
    let (e, t, g) = (
        round(adj.exposure, 3),
        round(adj.temp, 0),
        round(adj.tint, 1),
    );
    if let Some(op) = ops.iter_mut().find(|o| op_type(o) == Some("global")) {
        if let Some(m) = op.as_object_mut() {
            m.insert("exposure".into(), json!(e));
            m.insert("temp".into(), json!(t));
            m.insert("tint".into(), json!(g));
            m.insert("source".into(), json!("sync_adaptive"));
            return;
        }
    }
    if e != 0.0 || t != 0.0 || g != 0.0 {
        ops.push(json!({"type":"global","exposure":e,"temp":t,"tint":g,"source":"sync_adaptive"}));
        ops.sort_by_key(rank);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, c: [u8; 3]) -> RgbImage {
        RgbImage {
            width: w,
            height: h,
            data: (0..w * h).flat_map(|_| c).collect(),
        }
    }

    #[test]
    fn merge_replaces_only_included_kinds() {
        let t = vec![
            json!({"type":"crop","rect":[0,0,1,1]}),
            json!({"type":"global","exposure":1.0}),
            json!({"type":"lut","file":"a"}),
            json!({"type":"future_op","x":1}),
        ];
        let s = vec![
            json!({"type":"global","exposure":-1.0}),
            json!({"type":"local","amount":1}),
            json!({"type":"lut","file":"b"}),
        ];
        let m = merge_ops(&t, &s, &[SyncKind::Global, SyncKind::Local]);
        let types: Vec<_> = m.iter().map(|o| op_type(o).unwrap()).collect();
        assert_eq!(types, ["crop", "global", "local", "lut", "future_op"]);
        assert_eq!(m[1]["exposure"], -1.0);
        assert_eq!(m[3]["file"], "a"); // lut untouched
        let none = merge_ops(&t, &s, &[SyncKind::Crop]);
        assert!(none.iter().all(|o| op_type(o) != Some("local")));
        assert_eq!(none.len(), 3); // crop removed (source has none), others kept
    }

    #[test]
    fn measure_weights_centre_and_faces() {
        // dark frame with a bright centre block
        let (w, h) = (100u32, 100u32);
        let mut img = solid(w, h, [20, 20, 20]);
        for y in 35..65 {
            for x in 35..65 {
                let i = ((y * w + x) * 3) as usize;
                img.data[i..i + 3].copy_from_slice(&[200, 200, 200]);
            }
        }
        let plain = measure(&img, &[]);
        // unweighted mean luminance of the frame
        let unweighted = {
            let d = srgb_to_linear(20);
            let b = srgb_to_linear(200);
            0.09 * b + 0.91 * d
        };
        assert!(plain.lum > unweighted, "centre weighting favours the block");
        // a face box on the dark corner pulls the measurement down
        let face = measure(&img, &[[0.0, 0.0, 0.3, 0.3]]);
        assert!(face.lum < plain.lum);
    }

    #[test]
    fn grey_point_reflects_colour_cast() {
        let neutral = measure(&solid(32, 32, [120, 120, 120]), &[]);
        let warm = measure(&solid(32, 32, [140, 120, 100]), &[]);
        let (_, d_rb, _) = errors(&warm, &neutral);
        assert!(d_rb > 0.0, "warm target has more red than blue");
        let (d_ev, ..) = errors(&neutral, &neutral);
        assert!(d_ev.abs() < 1e-9);
    }

    /// Synthetic renderer response: gain `2^ev` and channel gains from temp/tint.
    fn model(base: [f64; 3], a: &Adjust) -> Stats {
        let g = 2f64.powf(a.exposure as f64);
        let r = base[0] * g * 2f64.powf(a.temp as f64 / 3000.0 * 0.4);
        let b = base[2] * g * 2f64.powf(-(a.temp as f64) / 3000.0 * 0.4);
        let gg = base[1] * g * 2f64.powf(-(a.tint as f64) / 100.0 * 0.2);
        Stats {
            lum: 0.2126 * r + 0.7152 * gg + 0.0722 * b,
            rgb: [r, gg, b],
        }
    }

    #[test]
    fn solver_matches_exposure_and_white_balance() {
        let src = model([0.30, 0.30, 0.30], &Adjust::default());
        // target is 1.3 EV darker and cooler
        let base = [0.30 / 2.5 * 0.95, 0.30 / 2.5, 0.30 / 2.5 * 1.07];
        let start = Adjust {
            exposure: 0.2,
            ..Default::default()
        };
        let out = solve(&src, &start, |a| Ok(model(base, a))).unwrap();
        let fin = model(base, &out);
        let e = errors(&src, &fin);
        assert!(e.0.abs() < 0.05, "ev error {e:?}");
        assert!(e.1.abs() < 0.03 && e.2.abs() < 0.03, "wb error {e:?}");
        assert!(out.exposure > start.exposure + 1.0);
        assert!(out.temp > 0.0, "cooler target needs warming");
    }

    #[test]
    fn solver_never_worse_than_the_plain_copy() {
        let src = Stats {
            lum: 0.2,
            rgb: [0.2; 3],
        };
        // a renderer that ignores the sliders entirely cannot be improved
        let flat = Stats {
            lum: 0.4,
            rgb: [0.4; 3],
        };
        let start = Adjust::default();
        let out = solve(&src, &start, |_| Ok(flat)).unwrap();
        assert_eq!(out, start);
    }

    #[test]
    fn set_global_keeps_other_fields() {
        let mut ops = vec![json!({"type":"global","contrast":10,"curve":{"rgb":[[0,0],[1,1]]}})];
        let adj = Adjust {
            exposure: 0.5,
            temp: 120.0,
            tint: -3.0,
            ..Default::default()
        };
        set_global_wb(&mut ops, &adj);
        assert_eq!(ops[0]["contrast"], 10);
        assert_eq!(ops[0]["exposure"], 0.5);
        assert_eq!(ops[0]["temp"], 120.0);
        assert!(ops[0]["curve"].is_object());
        let mut empty: Vec<Value> = vec![json!({"type":"lut","file":"x"})];
        set_global_wb(&mut empty, &Adjust::default());
        assert_eq!(empty.len(), 1, "no global added for a zero adjustment");
        set_global_wb(&mut empty, &adj);
        assert_eq!(op_type(&empty[0]), Some("global"));
    }
}
