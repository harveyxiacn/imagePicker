//! Built-in presets and procedurally generated LUTs (no binary assets).

use std::collections::BTreeMap;

use crate::*;

fn global(a: Adjust) -> Op {
    Op::Global(a)
}

fn hsl(h: f32, s: f32, l: f32) -> Hsl {
    Hsl { h, s, l }
}

fn stack(ops: Vec<Op>) -> EditStack {
    EditStack { version: 1, ops }
}

fn pts(p: &[[f32; 2]]) -> Vec<[f32; 2]> {
    p.to_vec()
}

/// A preset's tonal intent, resolved against the photo when it is applied (docs/03 §6.1.1).
/// Levels are sRGB-encoded luminance (0.46 ≈ 18 % grey).
#[derive(Debug, Clone, Copy)]
pub(crate) struct ToneIntent {
    /// Target subject level (the median, faces weighted 60 % when present).
    pub mid: f32,
    /// Share of the gap to `mid` closed when the photo is darker / brighter than it.
    pub pull_up: f32,
    pub pull_down: f32,
    /// Exposure range the solver may use at 100 % (EV; scaled by the strength).
    pub ev: (f32, f32),
    /// Share of pixels the preset may newly clip (luminance >= 250/255).
    pub max_new_clip: f32,
    /// Face median level the preset must not push faces above (unless they already were).
    pub face_max: f32,
}

const KEEP: ToneIntent = ToneIntent {
    mid: 0.46,
    pull_up: 0.3,
    pull_down: 0.25,
    ev: (-0.5, 0.5),
    max_new_clip: 0.005,
    face_max: 0.85,
};

/// A built-in preset: the look (colour and tonal character; exposure is solved, so it is 0
/// here) plus its tonal intent.
pub(crate) struct PresetSpec {
    pub id: &'static str,
    pub look: EditStack,
    pub tone: ToneIntent,
}

#[allow(clippy::vec_init_then_push)]
pub(crate) fn builtin_specs() -> Vec<PresetSpec> {
    let mut v: Vec<PresetSpec> = Vec::new();

    // Natural: a gentle polish; nudges clearly under/over-exposed photos toward a normal level.
    v.push(PresetSpec {
        id: "natural",
        look: stack(vec![
            global(Adjust {
                contrast: 6.0,
                highlights: -10.0,
                shadows: 10.0,
                whites: 4.0,
                vibrance: 12.0,
                clarity: 8.0,
                ..Default::default()
            }),
            Op::OutputSharpen(OutputSharpen { amount: 12.0 }),
        ]),
        tone: ToneIntent {
            mid: 0.47,
            pull_up: 0.35,
            ev: (-0.5, 0.6),
            ..KEEP
        },
    });

    // Vivid: punchy colour and contrast; keeps the photo's brightness.
    v.push(PresetSpec {
        id: "vivid",
        look: stack(vec![
            global(Adjust {
                contrast: 16.0,
                highlights: -8.0,
                shadows: 8.0,
                vibrance: 32.0,
                saturation: 8.0,
                clarity: 14.0,
                dehaze: 8.0,
                ..Default::default()
            }),
            Op::OutputSharpen(OutputSharpen { amount: 18.0 }),
        ]),
        tone: ToneIntent {
            pull_up: 0.25,
            pull_down: 0.2,
            ev: (-0.4, 0.4),
            ..KEEP
        },
    });

    // Film warm: faded blacks, rolled-off highlights, warm cast; slightly bright.
    v.push(PresetSpec {
        id: "film_warm",
        look: stack(vec![
            global(Adjust {
                contrast: -4.0,
                temp: 250.0,
                saturation: -6.0,
                highlights: -12.0,
                curve: Some(Curves {
                    rgb: pts(&[[0.0, 0.03], [0.25, 0.25], [0.75, 0.78], [1.0, 0.98]]),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            Op::Lut(Lut {
                file: "film_warm".into(),
                amount: 0.8,
            }),
        ]),
        tone: ToneIntent {
            mid: 0.48,
            pull_up: 0.35,
            pull_down: 0.3,
            ev: (-0.5, 0.6),
            ..KEEP
        },
    });

    // Film cool: the same film base with a cool cast.
    v.push(PresetSpec {
        id: "film_cool",
        look: stack(vec![
            global(Adjust {
                contrast: -2.0,
                temp: -280.0,
                tint: -3.0,
                saturation: -8.0,
                highlights: -6.0,
                curve: Some(Curves {
                    rgb: pts(&[[0.0, 0.025], [0.25, 0.24], [0.75, 0.77], [1.0, 0.99]]),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            Op::Lut(Lut {
                file: "film_cool".into(),
                amount: 0.8,
            }),
        ]),
        tone: ToneIntent {
            mid: 0.47,
            pull_up: 0.35,
            pull_down: 0.3,
            ev: (-0.5, 0.6),
            ..KEEP
        },
    });

    // Japanese clean (日系清新): bright and airy (subject level ≈ 0.60), low contrast, lifted
    // shadows and blacks, cool and slightly desaturated greens/blues. Dark photos are brightened
    // a lot; photos that are already bright are left alone or even brought down a little, and
    // highlights / faces never blow out (the old fixed +0.35 EV did on high-key photos).
    let mut jp_hsl = BTreeMap::new();
    jp_hsl.insert(HslBand::Green, hsl(-12.0, -18.0, 8.0));
    jp_hsl.insert(HslBand::Aqua, hsl(0.0, -6.0, 8.0));
    jp_hsl.insert(HslBand::Blue, hsl(-4.0, -12.0, 10.0));
    jp_hsl.insert(HslBand::Orange, hsl(0.0, -4.0, 8.0));
    v.push(PresetSpec {
        id: "japanese_clean",
        look: stack(vec![
            global(Adjust {
                contrast: -14.0,
                highlights: -24.0,
                shadows: 20.0,
                whites: 4.0,
                blacks: 12.0,
                temp: -170.0,
                tint: -5.0,
                vibrance: 8.0,
                saturation: -14.0,
                clarity: -10.0,
                dehaze: -4.0,
                curve: Some(Curves {
                    rgb: pts(&[[0.0, 0.05], [0.25, 0.28], [0.75, 0.78], [1.0, 1.0]]),
                    ..Default::default()
                }),
                hsl: jp_hsl,
                ..Default::default()
            }),
            Op::OutputSharpen(OutputSharpen { amount: 8.0 }),
        ]),
        tone: ToneIntent {
            mid: 0.60,
            pull_up: 0.8,
            pull_down: 0.25,
            ev: (-0.8, 1.2),
            max_new_clip: 0.003,
            face_max: 0.82,
        },
    });

    // Cinematic: teal shadows, warm highlights, a little moodier than the original.
    v.push(PresetSpec {
        id: "cinematic",
        look: stack(vec![
            global(Adjust {
                contrast: 10.0,
                saturation: -8.0,
                highlights: -10.0,
                grading: Some(Grading {
                    shadows: [205.0, 0.35],
                    midtones: [0.0, 0.0],
                    highlights: [45.0, 0.25],
                    balance: 5.0,
                }),
                ..Default::default()
            }),
            Op::Lut(Lut {
                file: "cinematic".into(),
                amount: 0.85,
            }),
        ]),
        tone: ToneIntent {
            mid: 0.40,
            pull_up: 0.3,
            pull_down: 0.4,
            ev: (-0.6, 0.3),
            ..KEEP
        },
    });

    v.push(PresetSpec {
        id: "bw_classic",
        look: stack(vec![
            global(Adjust {
                contrast: 12.0,
                saturation: -100.0,
                clarity: 10.0,
                ..Default::default()
            }),
            Op::Lut(Lut {
                file: "bw_classic".into(),
                amount: 1.0,
            }),
            Op::OutputSharpen(OutputSharpen { amount: 12.0 }),
        ]),
        tone: KEEP,
    });

    // High-contrast B&W: deep blacks and bright whites, but whites stop short of clipping.
    v.push(PresetSpec {
        id: "bw_contrast",
        look: stack(vec![
            global(Adjust {
                contrast: 35.0,
                saturation: -100.0,
                clarity: 25.0,
                whites: 6.0,
                blacks: -12.0,
                curve: Some(Curves {
                    rgb: pts(&[[0.0, 0.0], [0.25, 0.18], [0.75, 0.82], [1.0, 1.0]]),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            Op::OutputSharpen(OutputSharpen { amount: 20.0 }),
        ]),
        tone: ToneIntent {
            mid: 0.45,
            ..KEEP
        },
    });

    // Soft portrait: bright, glowing skin (face level ≈ 0.56) with soft contrast; faces are
    // kept below 0.80 so skin keeps its texture.
    let mut pt_hsl = BTreeMap::new();
    pt_hsl.insert(HslBand::Orange, hsl(0.0, -6.0, 7.0));
    pt_hsl.insert(HslBand::Red, hsl(0.0, -6.0, 4.0));
    v.push(PresetSpec {
        id: "portrait_soft",
        look: stack(vec![global(Adjust {
            contrast: -8.0,
            highlights: -16.0,
            shadows: 14.0,
            temp: 80.0,
            vibrance: 8.0,
            clarity: -14.0,
            dehaze: -4.0,
            hsl: pt_hsl,
            ..Default::default()
        })]),
        tone: ToneIntent {
            mid: 0.56,
            pull_up: 0.7,
            pull_down: 0.3,
            ev: (-0.6, 0.9),
            max_new_clip: 0.005,
            face_max: 0.80,
        },
    });

    // Landscape pop: rich skies and foliage; highlights (skies) protected.
    let mut ls_hsl = BTreeMap::new();
    ls_hsl.insert(HslBand::Green, hsl(0.0, 16.0, 0.0));
    ls_hsl.insert(HslBand::Yellow, hsl(0.0, 8.0, 0.0));
    ls_hsl.insert(HslBand::Blue, hsl(0.0, 16.0, -8.0));
    ls_hsl.insert(HslBand::Aqua, hsl(0.0, 8.0, -4.0));
    v.push(PresetSpec {
        id: "landscape_pop",
        look: stack(vec![
            global(Adjust {
                contrast: 15.0,
                highlights: -18.0,
                shadows: 16.0,
                whites: 6.0,
                blacks: -6.0,
                vibrance: 30.0,
                saturation: 6.0,
                clarity: 20.0,
                dehaze: 18.0,
                hsl: ls_hsl,
                ..Default::default()
            }),
            Op::OutputSharpen(OutputSharpen { amount: 25.0 }),
        ]),
        tone: ToneIntent {
            mid: 0.45,
            ..KEEP
        },
    });

    v
}

/// The presets as nominal stacks (the look at 100 % with exposure 0, before it is resolved
/// against a photo). Their global op carries `source: "preset:<id>"`.
pub fn builtin_presets() -> Vec<(String, String, EditStack)> {
    builtin_specs()
        .into_iter()
        .map(|p| {
            let mut s = p.look;
            for op in s.ops.iter_mut() {
                if let Op::Global(a) = op {
                    a.source = Some(format!("preset:{}", p.id));
                }
            }
            (p.id.to_string(), format!("preset.{}", p.id), s)
        })
        .collect()
}

// ---------------------------------------------------------------- procedural LUTs

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn luma(e: [f32; 3]) -> f32 {
    0.2126 * e[0] + 0.7152 * e[1] + 0.0722 * e[2]
}

fn sat_about_luma(e: [f32; 3], s: f32) -> [f32; 3] {
    let l = luma(e);
    [mix(l, e[0], s), mix(l, e[1], s), mix(l, e[2], s)]
}

fn s_curve(x: f32, k: f32) -> f32 {
    // Smooth contrast around 0.5, k in 0..1.
    let t = x.clamp(0.0, 1.0);
    mix(t, t * t * (3.0 - 2.0 * t), k)
}

fn gen_lut(n: u32, f: impl Fn([f32; 3]) -> [f32; 3]) -> Lut3d {
    let nm = (n - 1) as f32;
    let mut data = Vec::with_capacity((n * n * n) as usize);
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let o = f([r as f32 / nm, g as f32 / nm, b as f32 / nm]);
                data.push([
                    o[0].clamp(0.0, 1.0),
                    o[1].clamp(0.0, 1.0),
                    o[2].clamp(0.0, 1.0),
                ]);
            }
        }
    }
    Lut3d { size: n, data }
}

/// Procedurally generated built-in LUTs, resolvable by id:
/// `film_warm`, `film_cool`, `cinematic`, `bw_classic`.
pub fn builtin_lut(id: &str) -> Option<Lut3d> {
    const N: u32 = 33;
    match id {
        "film_warm" => Some(gen_lut(N, |e| {
            let l = luma(e);
            let lift = [0.025, 0.02, 0.04];
            let gain = [1.0, 0.975, 0.91];
            let gamma = [0.96, 1.0, 1.05];
            let mut o = [0.0; 3];
            for k in 0..3 {
                let v = s_curve(e[k], 0.18);
                o[k] = lift[k] + (gain[k] - lift[k]) * v.powf(gamma[k]);
            }
            // Warm highlights, slightly green-teal shadows.
            let wh = l * l;
            let ws = (1.0 - l) * (1.0 - l);
            o[0] += 0.02 * wh;
            o[2] -= 0.02 * wh;
            o[1] += 0.008 * ws;
            o[2] += 0.012 * ws;
            sat_about_luma(o, 0.94)
        })),
        "film_cool" => Some(gen_lut(N, |e| {
            let l = luma(e);
            let lift = [0.02, 0.03, 0.05];
            let gain = [0.95, 0.99, 1.0];
            let gamma = [1.04, 1.0, 0.95];
            let mut o = [0.0; 3];
            for k in 0..3 {
                let v = s_curve(e[k], 0.15);
                o[k] = lift[k] + (gain[k] - lift[k]) * v.powf(gamma[k]);
            }
            let ws = (1.0 - l) * (1.0 - l);
            o[2] += 0.02 * ws;
            o[0] -= 0.01 * ws;
            sat_about_luma(o, 0.92)
        })),
        "cinematic" => Some(gen_lut(N, |e| {
            let l = luma(e);
            let ws = (1.0 - l) * (1.0 - l);
            let wh = l * l;
            let mut o = [
                e[0] + (-0.035) * ws + 0.055 * wh,
                e[1] + 0.0 * ws + 0.018 * wh,
                e[2] + 0.045 * ws - 0.045 * wh,
            ];
            for v in o.iter_mut() {
                *v = s_curve(*v, 0.22);
            }
            sat_about_luma(o, 0.92)
        })),
        "teal_orange" => Some(gen_lut(N, |e| {
            let l = luma(e);
            let ws = (1.0 - l) * (1.0 - l);
            let wh = l * l;
            let mut o = [
                e[0] - 0.05 * ws + 0.07 * wh,
                e[1] + 0.01 * ws + 0.01 * wh,
                e[2] + 0.06 * ws - 0.06 * wh,
            ];
            for v in o.iter_mut() {
                *v = s_curve(*v, 0.25);
            }
            sat_about_luma(o, 1.05)
        })),
        "fade" => Some(gen_lut(N, |e| {
            let o = [
                0.06 + 0.88 * s_curve(e[0], 0.1),
                0.06 + 0.88 * s_curve(e[1], 0.1),
                0.07 + 0.87 * s_curve(e[2], 0.1),
            ];
            sat_about_luma(o, 0.85)
        })),
        "vivid" => Some(gen_lut(N, |e| {
            let o = [s_curve(e[0], 0.3), s_curve(e[1], 0.3), s_curve(e[2], 0.3)];
            sat_about_luma(o, 1.25)
        })),
        "bw_classic" => Some(gen_lut(N, |e| {
            // Channel-mixer greyscale with a gentle S-curve and a hint of warm tone.
            let g = 0.30 * e[0] + 0.59 * e[1] + 0.11 * e[2];
            let g = s_curve(g, 0.2);
            [g * 1.0 + 0.004, g, g * 0.985]
        })),
        _ => None,
    }
}

/// Ids resolvable by [`builtin_lut`].
pub fn builtin_lut_ids() -> &'static [&'static str] {
    &[
        "film_warm",
        "film_cool",
        "bw_classic",
        "teal_orange",
        "fade",
        "vivid",
        "cinematic",
    ]
}
