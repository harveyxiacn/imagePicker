//! M9 retouch ops: texture, dodge & burn, effects (post-crop vignette + grain), detail
//! (sharpening + noise reduction) and the range mask.
//!
//! Covers what each control does to pixels, that a neutral op changes nothing, that the
//! results are deterministic, and — for every new per-pixel op — that the GPU and the CPU
//! agree. The `detail` stage is whole-image and runs in shared code, so its parity is
//! structural; the test still pins it so a future GPU port cannot silently drift.

use ip_render::testutil::*;
use ip_render::*;

fn stack(ops: Vec<Op>) -> EditStack {
    EditStack { version: 1, ops }
}

fn cpu() -> Box<dyn Renderer> {
    create_renderer(false)
}

fn render(img: &RgbImage, st: &EditStack) -> RgbImage {
    cpu()
        .render(&RenderRequest {
            source: img,
            stack: st,
            masks: &NoAssets,
            luts: &NoAssets,
            max_long_edge: None,
        })
        .expect("render")
}

fn px(i: &RgbImage, x: u32, y: u32) -> [u8; 3] {
    let o = ((y * i.width + x) * 3) as usize;
    [i.data[o], i.data[o + 1], i.data[o + 2]]
}

fn lum8(p: [u8; 3]) -> f32 {
    0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32
}

fn global(f: impl FnOnce(&mut Adjust)) -> EditStack {
    let mut a = Adjust::default();
    f(&mut a);
    stack(vec![Op::Global(a)])
}

/// Mean absolute deviation of the high-frequency band — the same quantity the noise
/// reduction estimates internally, so it is the honest way to assert "noise went down".
fn hf_energy(img: &RgbImage) -> f32 {
    let (w, h) = (img.width as usize, img.height as usize);
    let l: Vec<f32> = (0..w * h)
        .map(|i| lum8(px(img, (i % w) as u32, (i / w) as u32)))
        .collect();
    let mut acc = 0.0;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            let blur = (l[i] * 4.0 + l[i - 1] + l[i + 1] + l[i - w] + l[i + w]) / 8.0;
            acc += (l[i] - blur).abs();
        }
    }
    acc / ((w - 2) * (h - 2)) as f32
}

/// Mean absolute deviation from the image's own mean luminance. Grain is smooth value
/// noise, so it carries little *high-frequency* energy even when it is plainly visible;
/// this is the measure that says "there is grain here" rather than "there is fine detail".
fn spread(img: &RgbImage) -> f32 {
    let (w, h) = (img.width as usize, img.height as usize);
    let l: Vec<f32> = (0..w * h)
        .map(|i| lum8(px(img, (i % w) as u32, (i / w) as u32)))
        .collect();
    let mean = l.iter().sum::<f32>() / l.len() as f32;
    l.iter().map(|v| (v - mean).abs()).sum::<f32>() / l.len() as f32
}

fn parity(gpu: &dyn Renderer, img: &RgbImage, st: &EditStack, label: &str) {
    let req = RenderRequest {
        source: img,
        stack: st,
        masks: &NoAssets,
        luts: &NoAssets,
        max_long_edge: None,
    };
    let a = create_renderer(false).render(&req).unwrap();
    let b = gpu.render(&req).unwrap();
    let (mean, max) = compare(&a, &b);
    println!("parity {label}: mean dE {mean:.4}, max dE {max:.3}");
    assert!(mean < 1.0 && max < 3.0, "{label}: mean {mean} max {max}");
}

/// Run `f` against every new op on a few images, asserting CPU/GPU agreement.
fn parity_all(label: &str, stacks: &[(&str, EditStack)]) {
    let Some(gpu) = create_strict_gpu_renderer() else {
        eprintln!("SKIP {label}: no GPU adapter");
        return;
    };
    for img in [scene(320, 240, 3), macbeth(300, 200), noise(160, 120, 7)] {
        for (name, st) in stacks {
            parity(gpu.as_ref(), &img, st, &format!("{label}/{name}"));
        }
    }
}

// ---------------------------------------------------------------- texture

#[test]
fn texture_is_off_at_zero_and_moves_in_both_directions() {
    // A scene, not `noise()`: texture is a mid-scale operator and the weight deliberately
    // falls away as local amplitude grows, so a pure noise field is the one input it is
    // meant to leave alone (pinned separately below).
    let img = scene(200, 150, 11);
    let base = render(&img, &stack(vec![]));
    let zero = render(&img, &global(|a| a.texture = 0.0));
    assert_eq!(base.data, zero.data, "texture = 0 must be a no-op");

    let up = render(&img, &global(|a| a.texture = 70.0));
    let down = render(&img, &global(|a| a.texture = -70.0));
    let e0 = hf_energy(&base);
    assert!(
        hf_energy(&up) > e0 * 1.05,
        "positive texture must add fine detail"
    );
    assert!(
        hf_energy(&down) < e0 * 0.98,
        "negative texture must even detail out"
    );

    // Sensor noise must not be amplified: that is the whole reason texture is a separate
    // control from clarity and sharpening.
    let noisy = noise(200, 150, 11);
    let nb = hf_energy(&render(&noisy, &stack(vec![])));
    let nu = hf_energy(&render(&noisy, &global(|a| a.texture = 70.0)));
    assert!(
        nu < nb * 1.10,
        "texture must not amplify noise ({nb} -> {nu})"
    );
    // Neither direction may clip a mid-grey field to a flat plate.
    let flat = solid(64, 64, [128, 128, 128]);
    let f = render(&flat, &global(|a| a.texture = 100.0));
    assert_eq!(
        px(&f, 32, 32),
        [128, 128, 128],
        "flat areas must be left alone"
    );
}

#[test]
fn texture_is_deterministic() {
    let img = scene(180, 140, 5);
    let st = global(|a| a.texture = 55.0);
    assert_eq!(render(&img, &st).data, render(&img, &st).data);
}

// ---------------------------------------------------------------- dodge & burn

#[test]
fn dodge_brightens_burn_darkens_and_hue_survives() {
    let img = solid(80, 80, [150, 110, 90]); // a skin-ish tone
    let base = px(&render(&img, &stack(vec![])), 40, 40);
    let dodged = px(&render(&img, &global(|a| a.dodge = 80.0)), 40, 40);
    let burned = px(&render(&img, &global(|a| a.burn = 80.0)), 40, 40);
    assert!(lum8(dodged) > lum8(base) + 6.0, "dodge must brighten");
    assert!(lum8(burned) < lum8(base) - 6.0, "burn must darken");

    // Hue is preserved exactly: only the chroma magnitude moves with luminance.
    let hue = |p: [u8; 3]| {
        let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
        let mx = r.max(g).max(b);
        let mn = r.min(g).min(b);
        if (mx - mn).abs() < 1.0 {
            return None;
        }
        let d = mx - mn;
        let h = if mx == r {
            ((g - b) / d).rem_euclid(6.0)
        } else if mx == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        Some(h * 60.0)
    };
    let (h0, h1) = (hue(base).unwrap(), hue(dodged).unwrap());
    assert!((h0 - h1).abs() < 3.0, "dodge changed the hue: {h0} -> {h1}");
}

#[test]
fn dodge_and_burn_reach_shadows_and_highlights_differently() {
    // Dodge protects highlights, burn protects shadows — that asymmetry is the point.
    let dark = render(&solid(32, 32, [40, 40, 40]), &global(|a| a.dodge = 100.0));
    let bright = render(
        &solid(32, 32, [215, 215, 215]),
        &global(|a| a.dodge = 100.0),
    );
    let d = lum8(px(&dark, 16, 16)) - 40.0;
    let b = lum8(px(&bright, 16, 16)) - 215.0;
    assert!(
        d > b + 8.0,
        "dodge must lift shadows more than highlights ({d} vs {b})"
    );

    let dark = render(&solid(32, 32, [40, 40, 40]), &global(|a| a.burn = 100.0));
    let bright = render(&solid(32, 32, [215, 215, 215]), &global(|a| a.burn = 100.0));
    let d = 40.0 - lum8(px(&dark, 16, 16));
    let b = 215.0 - lum8(px(&bright, 16, 16));
    assert!(
        b > d + 8.0,
        "burn must bite highlights more than shadows ({b} vs {d})"
    );
}

#[test]
fn dodge_through_a_local_mask_is_local() {
    // The classic gesture: dodge only inside the mask.
    let img = solid(120, 120, [110, 110, 110]);
    let st = stack(vec![Op::Local(LocalAdjust {
        mask: MaskRef::Radial {
            center: [0.5, 0.5],
            radius: [0.25, 0.25],
            feather: 0.4,
        },
        amount: 1.0,
        invert: false,
        adjust: Adjust {
            dodge: 90.0,
            ..Default::default()
        },
        range: None,
    })]);
    let o = render(&img, &st);
    assert!(
        lum8(px(&o, 60, 60)) > 118.0,
        "inside the mask must be dodged"
    );
    assert!(
        lum8(px(&o, 3, 3)) < 112.0,
        "outside the mask must be untouched"
    );
}

// ---------------------------------------------------------------- effects

#[test]
fn vignette_darkens_the_corners_only() {
    let img = solid(200, 200, [180, 180, 180]);
    let o = render(
        &img,
        &stack(vec![Op::Effects(Effects {
            vignette: Vignette {
                amount: -100.0,
                ..Default::default()
            },
            grain: Grain::default(),
        })]),
    );
    let centre = lum8(px(&o, 100, 100));
    let corner = lum8(px(&o, 2, 2));
    assert!(
        (centre - 180.0).abs() < 2.0,
        "the centre must stay put ({centre})"
    );
    assert!(
        corner < centre - 40.0,
        "the corner must darken ({corner} vs {centre})"
    );

    let bright = render(
        &img,
        &stack(vec![Op::Effects(Effects {
            vignette: Vignette {
                amount: 100.0,
                ..Default::default()
            },
            grain: Grain::default(),
        })]),
    );
    assert!(
        lum8(px(&bright, 2, 2)) > centre + 10.0,
        "positive amount must brighten"
    );
}

#[test]
fn vignette_roundness_and_highlight_contrast_do_something() {
    let img = solid(200, 200, [170, 170, 170]);
    let v = |roundness: f32, hl: f32| {
        let o = render(
            &img,
            &stack(vec![Op::Effects(Effects {
                vignette: Vignette {
                    amount: -100.0,
                    roundness,
                    highlight_contrast: hl,
                    ..Default::default()
                },
                grain: Grain::default(),
            })]),
        );
        // Off-axis, where the super-ellipse families actually disagree: every one of them
        // crosses the axes and the diagonals at the same places. The third sample sits
        // further out, inside the feather rather than in its tail, so it is where highlight
        // contrast has something to hold back.
        (
            lum8(px(&o, 160, 70)),
            lum8(px(&o, 140, 60)),
            lum8(px(&o, 170, 170)),
        )
    };
    let (rect, oval) = (v(-100.0, 0.0), v(100.0, 0.0));
    // A rectangular vignette is the one whose *clear* area is a diamond, so its dark frame
    // reaches in further between the axes than an oval's does.
    assert!(
        rect.0 < oval.0 - 5.0 && rect.1 < oval.1 - 5.0,
        "roundness must change the contour: {rect:?} vs {oval:?}"
    );
    // Highlight contrast eases off on a bright field.
    let (.., hl) = v(0.0, 100.0);
    let (.., plain) = v(0.0, 0.0);
    assert!(
        hl > plain + 3.0,
        "highlight contrast must hold the effect back ({hl} vs {plain})"
    );
}

#[test]
fn vignette_styles_differ() {
    let img = scene(160, 160, 4);
    let with = |style: VignetteStyle| {
        render(
            &img,
            &stack(vec![Op::Effects(Effects {
                vignette: Vignette {
                    amount: -100.0,
                    style,
                    ..Default::default()
                },
                grain: Grain::default(),
            })]),
        )
    };
    let a = with(VignetteStyle::HighlightPriority);
    let b = with(VignetteStyle::ColorPriority);
    let c = with(VignetteStyle::PaintOverlay);
    assert!(
        max_abs_diff(&a, &b) > 1,
        "colour priority must differ from highlight priority"
    );
    assert!(
        max_abs_diff(&a, &c) > 1,
        "paint overlay must differ from highlight priority"
    );
}

#[test]
fn grain_adds_deterministic_noise_at_the_requested_scale() {
    let img = solid(240, 240, [128, 128, 128]);
    let grain = |size: f32| {
        render(
            &img,
            &stack(vec![Op::Effects(Effects {
                vignette: Vignette::default(),
                grain: Grain {
                    amount: 80.0,
                    size,
                    frequency: 50.0,
                },
            })]),
        )
    };
    let a = grain(30.0);
    assert!(
        spread(&a) > 8.0,
        "grain must move pixels off the flat field: {}",
        spread(&a)
    );
    assert_eq!(a.data, grain(30.0).data, "grain must be deterministic");
    // Bigger cells = coarser noise: the same amplitude over a wider cell is smoother, so it
    // carries less high-frequency energy per pixel.
    assert!(
        hf_energy(&grain(90.0)) < hf_energy(&a) * 0.5,
        "size must change the cell scale"
    );
}

#[test]
fn grain_is_anchored_to_the_source_not_the_output() {
    // The same stack rendered at two output sizes must show grain of the same physical
    // size, otherwise the preview lies about the export.
    let img = scene(600, 400, 6);
    let st = stack(vec![Op::Effects(Effects {
        vignette: Vignette::default(),
        grain: Grain {
            amount: 90.0,
            size: 40.0,
            frequency: 50.0,
        },
    })]);
    let full = render(&img, &st);
    let small = cpu()
        .render(&RenderRequest {
            source: &img,
            stack: &st,
            masks: &NoAssets,
            luts: &NoAssets,
            max_long_edge: Some(300),
        })
        .unwrap();
    // Both keep grain: a resolution-independent implementation would leave the small one flat.
    assert!(hf_energy(&full) > 1.0 && hf_energy(&small) > 1.0);
}

#[test]
fn neutral_effects_op_is_identity() {
    let img = scene(120, 90, 2);
    let base = render(&img, &stack(vec![]));
    let same = render(
        &img,
        &stack(vec![Op::Effects(Effects {
            vignette: Vignette {
                amount: 0.0,
                ..Default::default()
            },
            grain: Grain {
                amount: 0.0,
                ..Default::default()
            },
        })]),
    );
    assert_eq!(base.data, same.data);
}

// ---------------------------------------------------------------- detail

#[test]
fn sharpening_raises_edge_contrast_and_masking_holds_it_back() {
    // A soft edge: a hard step blurred by the test image itself would be ideal, so build a
    // two-tone frame and measure across the seam.
    let mut data = vec![0u8; 200 * 200 * 3];
    for y in 0..200u32 {
        for x in 0..200u32 {
            let v = if x < 100 { 90u8 } else { 150u8 };
            let o = ((y * 200 + x) * 3) as usize;
            data[o] = v;
            data[o + 1] = v;
            data[o + 2] = v;
        }
    }
    let img = RgbImage {
        width: 200,
        height: 200,
        data,
    };
    let detail = |amount: f32, masking: f32| {
        stack(vec![Op::Detail(Detail {
            sharpen: SharpenDetail {
                amount,
                radius: 1.5,
                detail: 25.0,
                masking,
            },
            noise: NoiseReduction::default(),
        })])
    };
    let base = render(&img, &stack(vec![]));
    let sharp = render(&img, &detail(120.0, 0.0));
    // Overshoot/undershoot appear on both sides of the seam.
    let l = lum8(px(&sharp, 98, 100));
    let r = lum8(px(&sharp, 101, 100));
    let (bl, br) = (lum8(px(&base, 98, 100)), lum8(px(&base, 101, 100)));
    assert!(r - l > br - bl + 8.0, "sharpening must raise edge contrast");

    // Masking keeps the flat areas but still finds the seam.
    let masked = render(&img, &detail(120.0, 90.0));
    assert_eq!(
        px(&masked, 20, 20),
        px(&base, 20, 20),
        "masking must spare flat areas"
    );
    let l = lum8(px(&masked, 98, 100));
    let r = lum8(px(&masked, 101, 100));
    assert!(r - l > br - bl + 3.0, "masking must keep the edge");
}

#[test]
fn noise_reduction_lowers_high_frequency_energy() {
    let img = noise(200, 150, 13);
    let e0 = hf_energy(&img);
    let st = stack(vec![Op::Detail(Detail {
        sharpen: SharpenDetail::default(),
        noise: NoiseReduction {
            luminance: 80.0,
            color: 80.0,
            ..Default::default()
        },
    })]);
    let o = render(&img, &st);
    assert!(
        hf_energy(&o) < e0 * 0.9,
        "noise reduction must smooth: {} -> {}",
        e0,
        hf_energy(&o)
    );

    // Detail puts part of the removed band back.
    let keep = render(
        &img,
        &stack(vec![Op::Detail(Detail {
            sharpen: SharpenDetail::default(),
            noise: NoiseReduction {
                luminance: 80.0,
                luminance_detail: 100.0,
                ..Default::default()
            },
        })]),
    );
    assert!(
        hf_energy(&keep) > hf_energy(&o),
        "detail must restore texture"
    );
}

#[test]
fn a_clean_image_is_not_flattened_by_noise_reduction() {
    // The adaptive part: a low-noise frame must keep its edges even at full strength.
    let img = scene(200, 150, 8);
    let st = stack(vec![Op::Detail(Detail {
        sharpen: SharpenDetail::default(),
        noise: NoiseReduction {
            luminance: 100.0,
            ..Default::default()
        },
    })]);
    let o = render(&img, &st);
    let (base_e, out_e) = (hf_energy(&img), hf_energy(&o));
    assert!(
        out_e > base_e * 0.5,
        "clean detail must survive: {base_e} -> {out_e}"
    );
}

#[test]
fn output_sharpen_runs_after_the_detail_stage() {
    // With a detail op present the output sharpen moves into the stage, so the two still
    // compose instead of one cancelling the other.
    let img = noise(160, 120, 21);
    let both = stack(vec![
        Op::Detail(Detail {
            sharpen: SharpenDetail {
                amount: 60.0,
                ..Default::default()
            },
            noise: NoiseReduction {
                luminance: 40.0,
                ..Default::default()
            },
        }),
        Op::OutputSharpen(OutputSharpen { amount: 40.0 }),
    ]);
    let a = render(&img, &both);
    let b = render(
        &img,
        &stack(vec![Op::OutputSharpen(OutputSharpen { amount: 40.0 })]),
    );
    assert!(
        max_abs_diff(&a, &b) > 1,
        "the stage must actually apply the output sharpen"
    );
}

#[test]
fn neutral_detail_op_is_identity() {
    let img = scene(120, 90, 3);
    let base = render(&img, &stack(vec![]));
    let same = render(
        &img,
        &stack(vec![Op::Detail(Detail {
            sharpen: SharpenDetail {
                amount: 0.0,
                ..Default::default()
            },
            noise: NoiseReduction {
                luminance: 0.0,
                color: 0.0,
                ..Default::default()
            },
        })]),
    );
    assert_eq!(base.data, same.data);
}

// ---------------------------------------------------------------- range masks

#[test]
fn luminance_range_mask_narrows_a_local_adjustment() {
    // Left half dark, right half bright; a global "brighten everything" local op gated to
    // the dark band must only touch the left half.
    let mut data = vec![0u8; 200 * 100 * 3];
    for y in 0..100u32 {
        for x in 0..200u32 {
            let v = if x < 100 { 50u8 } else { 200u8 };
            let o = ((y * 200 + x) * 3) as usize;
            data[o] = v;
            data[o + 1] = v;
            data[o + 2] = v;
        }
    }
    let img = RgbImage {
        width: 200,
        height: 100,
        data,
    };
    let st = stack(vec![Op::Local(LocalAdjust {
        mask: MaskRef::Linear {
            start: [0.0, 0.0],
            end: [0.0, 1.0],
        },
        amount: 1.0,
        invert: false,
        adjust: Adjust {
            exposure: 1.0,
            ..Default::default()
        },
        range: Some(MaskRange::Luminance {
            min: 0.0,
            max: 35.0,
            softness: 10.0,
        }),
    })]);
    let o = render(&img, &st);
    assert!(lum8(px(&o, 20, 50)) > 60.0, "the dark band must be lifted");
    assert!(
        (lum8(px(&o, 180, 50)) - 200.0).abs() < 2.0,
        "the bright band must be spared"
    );
}

#[test]
fn color_range_mask_selects_by_hue() {
    // Two flat colour fields; the mask keeps only the red one.
    let mut data = vec![0u8; 200 * 100 * 3];
    for y in 0..100u32 {
        for x in 0..200u32 {
            let o = ((y * 200 + x) * 3) as usize;
            let c = if x < 100 {
                [200u8, 60, 60]
            } else {
                [60, 90, 200]
            };
            data[o] = c[0];
            data[o + 1] = c[1];
            data[o + 2] = c[2];
        }
    }
    let img = RgbImage {
        width: 200,
        height: 100,
        data,
    };
    let st = stack(vec![Op::Local(LocalAdjust {
        mask: MaskRef::Linear {
            start: [0.0, 0.0],
            end: [0.0, 1.0],
        },
        amount: 1.0,
        invert: false,
        adjust: Adjust {
            exposure: -2.0,
            ..Default::default()
        },
        range: Some(MaskRange::Color {
            hue: 20.0,
            width: 30.0,
            sat_min: 0.0,
            sat_max: 100.0,
            softness: 20.0,
        }),
    })]);
    let o = render(&img, &st);
    assert!(
        lum8(px(&o, 20, 50)) < lum8(px(&img, 20, 50)) - 15.0,
        "red must darken"
    );
    assert_eq!(px(&o, 180, 50), px(&img, 180, 50), "blue must be untouched");
}

#[test]
fn unknown_range_kind_is_ignored() {
    let img = solid(80, 80, [120, 120, 120]);
    let st = stack(vec![Op::Local(LocalAdjust {
        mask: MaskRef::Linear {
            start: [0.0, 0.0],
            end: [0.0, 1.0],
        },
        amount: 1.0,
        invert: false,
        adjust: Adjust {
            exposure: 1.0,
            ..Default::default()
        },
        range: Some(MaskRange::Unknown),
    })]);
    let o = render(&img, &st);
    assert!(
        lum8(px(&o, 40, 40)) > 130.0,
        "an unknown range must not block the op"
    );
}

// ---------------------------------------------------------------- schema & parity

#[test]
fn new_ops_round_trip_through_json() {
    let st = stack(vec![
        Op::Effects(Effects {
            vignette: Vignette {
                amount: -40.0,
                style: VignetteStyle::ColorPriority,
                ..Default::default()
            },
            grain: Grain {
                amount: 20.0,
                ..Default::default()
            },
        }),
        Op::Detail(Detail {
            sharpen: SharpenDetail {
                amount: 60.0,
                ..Default::default()
            },
            noise: NoiseReduction {
                luminance: 30.0,
                ..Default::default()
            },
        }),
    ]);
    let text = serde_json::to_string(&st).unwrap();
    let back: EditStack = serde_json::from_str(&text).unwrap();
    assert_eq!(st, back);
    // A stack written before M9 has neither field and must still load.
    let old: EditStack = serde_json::from_str(
        r#"{"version":1,"ops":[{"type":"global","exposure":0.5},
            {"type":"local","mask":{"kind":"linear","start":[0,0],"end":[0,1]},"adjust":{"contrast":10}}]}"#,
    )
    .unwrap();
    assert_eq!(old.ops.len(), 2);
    match &old.ops[1] {
        Op::Local(l) => assert!(l.range.is_none()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn gpu_cpu_parity_new_per_pixel_ops() {
    let effects = stack(vec![Op::Effects(Effects {
        vignette: Vignette {
            amount: -70.0,
            midpoint: 40.0,
            roundness: 35.0,
            feather: 65.0,
            highlight_contrast: 40.0,
            style: VignetteStyle::ColorPriority,
        },
        grain: Grain {
            amount: 60.0,
            size: 35.0,
            frequency: 70.0,
        },
    })]);
    let overlay = stack(vec![Op::Effects(Effects {
        vignette: Vignette {
            amount: 55.0,
            style: VignetteStyle::PaintOverlay,
            ..Default::default()
        },
        grain: Grain::default(),
    })]);
    let texture = global(|a| {
        a.texture = 65.0;
        a.clarity = 20.0;
    });
    let dodge = global(|a| {
        a.dodge = 70.0;
        a.burn = 35.0;
        a.saturation = 15.0;
    });
    let ranged = stack(vec![Op::Local(LocalAdjust {
        mask: MaskRef::Radial {
            center: [0.45, 0.5],
            radius: [0.4, 0.35],
            feather: 0.6,
        },
        amount: 0.85,
        invert: false,
        adjust: Adjust {
            exposure: 0.6,
            texture: -40.0,
            dodge: 40.0,
            ..Default::default()
        },
        range: Some(MaskRange::Luminance {
            min: 10.0,
            max: 70.0,
            softness: 25.0,
        }),
    })]);
    let hue_range = stack(vec![Op::Local(LocalAdjust {
        mask: MaskRef::Linear {
            start: [0.0, 0.0],
            end: [0.0, 1.0],
        },
        amount: 1.0,
        invert: false,
        adjust: Adjust {
            saturation: -60.0,
            ..Default::default()
        },
        range: Some(MaskRange::Color {
            hue: 210.0,
            width: 60.0,
            sat_min: 5.0,
            sat_max: 95.0,
            softness: 30.0,
        }),
    })]);
    parity_all(
        "m9",
        &[
            ("effects", effects),
            ("overlay", overlay),
            ("texture", texture),
            ("dodge", dodge),
            ("luminance-range", ranged),
            ("color-range", hue_range),
        ],
    );
}

#[test]
fn gpu_cpu_parity_detail_stage() {
    let st = stack(vec![
        Op::Detail(Detail {
            sharpen: SharpenDetail {
                amount: 90.0,
                radius: 2.0,
                detail: 40.0,
                masking: 30.0,
            },
            noise: NoiseReduction {
                luminance: 60.0,
                luminance_detail: 40.0,
                luminance_contrast: 20.0,
                color: 50.0,
                color_detail: 60.0,
            },
        }),
        Op::OutputSharpen(OutputSharpen { amount: 30.0 }),
    ]);
    parity_all("detail", &[("full", st)]);
}

#[test]
fn gpu_cpu_parity_random_stacks_cover_the_new_ops() {
    // `random_stack` now emits texture / dodge / burn / effects / detail / range masks, so
    // the fuzz in render.rs exercises them too; this pins the ones added here directly.
    let Some(gpu) = create_strict_gpu_renderer() else {
        eprintln!("SKIP gpu_cpu_parity_random_stacks_cover_the_new_ops: no GPU adapter");
        return;
    };
    for seed in 0..12u64 {
        let img = scene(240, 180, seed);
        let st = random_stack(7000 + seed);
        parity(gpu.as_ref(), &img, &st, &format!("random#{seed}"));
    }
}
