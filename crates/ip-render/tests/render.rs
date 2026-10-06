#![allow(clippy::chunks_exact_to_as_chunks)]
use ip_render::testutil::*;
use ip_render::*;

fn stack(ops: Vec<Op>) -> EditStack {
    EditStack { version: 1, ops }
}

fn global(a: Adjust) -> Op {
    Op::Global(a)
}

fn cpu() -> Box<dyn Renderer> {
    create_renderer(false)
}

fn render_with(r: &dyn Renderer, img: &RgbImage, st: &EditStack, max: Option<u32>) -> RgbImage {
    r.render(&RenderRequest {
        source: img,
        stack: st,
        masks: &NoAssets,
        luts: &NoAssets,
        max_long_edge: max,
    })
    .expect("render")
}

fn render(img: &RgbImage, st: &EditStack) -> RgbImage {
    render_with(cpu().as_ref(), img, st, None)
}

fn mean_lum(img: &RgbImage) -> f64 {
    let m = mean_rgb(img);
    0.2126 * m[0] + 0.7152 * m[1] + 0.0722 * m[2]
}

fn adj(f: impl FnOnce(&mut Adjust)) -> EditStack {
    let mut a = Adjust::default();
    f(&mut a);
    stack(vec![global(a)])
}

// ---------------------------------------------------------------- identity

#[test]
fn identity_stack_is_exact() {
    let imgs = [
        gradient(257, 129),
        macbeth(240, 160),
        noise(131, 97, 7),
        skin_patch(64, 64),
        scene(200, 150, 3),
    ];
    let neutral = stack(vec![
        Op::Crop(Crop {
            rect: [0.0, 0.0, 1.0, 1.0],
            angle: 0.0,
            aspect: None,
        }),
        global(Adjust::default()),
        Op::OutputSharpen(OutputSharpen { amount: 0.0 }),
    ]);
    for img in &imgs {
        for st in [EditStack::default(), neutral.clone()] {
            let out = render(img, &st);
            assert_eq!((out.width, out.height), (img.width, img.height));
            assert_eq!(max_abs_diff(img, &out), 0, "identity must be exact");
        }
    }
}

#[test]
fn neutral_local_and_lut_identity() {
    let img = scene(160, 120, 11);
    let st = stack(vec![
        Op::Local(LocalAdjust {
            mask: MaskRef::Radial {
                center: [0.5, 0.5],
                radius: [0.3, 0.3],
                feather: 0.5,
            },
            amount: 1.0,
            invert: false,
            adjust: Adjust::default(),
        }),
        Op::Lut(Lut {
            file: "identity".into(),
            amount: 1.0,
        }),
    ]);
    struct IdLut;
    impl LutProvider for IdLut {
        fn lut(&self, f: &str) -> Result<Option<Lut3d>> {
            Ok((f == "identity").then(|| identity_lut(17)))
        }
    }
    let out = cpu()
        .render(&RenderRequest {
            source: &img,
            stack: &st,
            masks: &NoAssets,
            luts: &IdLut,
            max_long_edge: None,
        })
        .unwrap();
    assert!(max_abs_diff(&img, &out) <= 1);
}

// ---------------------------------------------------------------- monotonicity

#[test]
fn exposure_brightens_and_darkens() {
    let img = scene(160, 120, 5);
    let base = mean_lum(&img);
    let up = mean_lum(&render(&img, &adj(|a| a.exposure = 1.0)));
    let dn = mean_lum(&render(&img, &adj(|a| a.exposure = -1.0)));
    assert!(up > base + 10.0, "{up} vs {base}");
    assert!(dn < base - 10.0, "{dn} vs {base}");
    // Roll-off: pure white stays white, does not wrap.
    let white = solid(8, 8, [255, 255, 255]);
    let o = render(&white, &adj(|a| a.exposure = 2.0));
    assert!(o.data.iter().all(|&v| v >= 253));
}

#[test]
fn temp_warms_and_tint_moves_green_magenta() {
    let grey = solid(16, 16, [128, 128, 128]);
    let base = mean_rgb(&grey);
    let warm = mean_rgb(&render(&grey, &adj(|a| a.temp = 1500.0)));
    let cool = mean_rgb(&render(&grey, &adj(|a| a.temp = -1500.0)));
    assert!(warm[0] / warm[2] > base[0] / base[2] + 0.1, "{warm:?}");
    assert!(cool[0] / cool[2] < base[0] / base[2] - 0.1, "{cool:?}");
    let mag = mean_rgb(&render(&grey, &adj(|a| a.tint = 60.0)));
    let grn = mean_rgb(&render(&grey, &adj(|a| a.tint = -60.0)));
    assert!(mag[1] < mag[0] && mag[1] < mag[2], "{mag:?}");
    assert!(grn[1] > grn[0] && grn[1] > grn[2], "{grn:?}");
}

#[test]
fn saturation_minus_100_is_grey() {
    let img = macbeth(240, 160);
    let out = render(&img, &adj(|a| a.saturation = -100.0));
    for p in out.data.chunks_exact(3) {
        let (mx, mn) = (p.iter().max().unwrap(), p.iter().min().unwrap());
        assert!(mx - mn <= 2, "{p:?}");
    }
    let more = render(&img, &adj(|a| a.saturation = 50.0));
    let chroma = |i: &RgbImage| {
        i.data
            .chunks_exact(3)
            .map(|p| (p.iter().max().unwrap() - p.iter().min().unwrap()) as f64)
            .sum::<f64>()
    };
    assert!(chroma(&more) > chroma(&img));
}

#[test]
fn tone_sliders_have_expected_direction() {
    let img = scene(160, 120, 9);
    let l0 = mean_lum(&img);
    assert!(mean_lum(&render(&img, &adj(|a| a.shadows = 80.0))) > l0);
    assert!(mean_lum(&render(&img, &adj(|a| a.highlights = -80.0))) < l0);
    assert!(mean_lum(&render(&img, &adj(|a| a.whites = 60.0))) > l0);
    assert!(mean_lum(&render(&img, &adj(|a| a.blacks = 60.0))) > l0);
    assert!(mean_lum(&render(&img, &adj(|a| a.blacks = -60.0))) < l0);
    // Contrast widens the spread of a ramp.
    let ramp = gradient(256, 4);
    let spread = |i: &RgbImage| {
        let m = mean_lum(i);
        i.data
            .chunks_exact(3)
            .map(|p| (p[1] as f64 - m).powi(2))
            .sum::<f64>()
    };
    assert!(spread(&render(&ramp, &adj(|a| a.contrast = 60.0))) > spread(&ramp));
    assert!(spread(&render(&ramp, &adj(|a| a.contrast = -60.0))) < spread(&ramp));
}

#[test]
fn clarity_adds_local_contrast_dehaze_changes_hazy_image() {
    // Checker of mild contrast: clarity must increase variance.
    let mut img = solid(128, 128, [120, 120, 120]);
    for y in 0..128usize {
        for x in 0..128usize {
            if (x / 8 + y / 8) % 2 == 0 {
                let i = (y * 128 + x) * 3;
                img.data[i..i + 3].copy_from_slice(&[150, 150, 150]);
            }
        }
    }
    let var = |i: &RgbImage| {
        let m = mean_lum(i);
        i.data
            .chunks_exact(3)
            .map(|p| (p[1] as f64 - m).powi(2))
            .sum::<f64>()
    };
    assert!(var(&render(&img, &adj(|a| a.clarity = 80.0))) > var(&img) * 1.05);
    assert!(var(&render(&img, &adj(|a| a.clarity = -80.0))) < var(&img));
    // Dehaze > 0 increases contrast of a hazy ramp; < 0 reduces it.
    let mut hazy = gradient(128, 32);
    for v in hazy.data.iter_mut() {
        *v = 140 + (*v as f32 * 0.4) as u8;
    }
    assert!(var(&render(&hazy, &adj(|a| a.dehaze = 80.0))) > var(&hazy));
    assert!(var(&render(&hazy, &adj(|a| a.dehaze = -80.0))) < var(&hazy));
}

#[test]
fn curves_are_monotone_and_apply() {
    let ramp = gradient(256, 2);
    let st = adj(|a| {
        a.curve = Some(Curves {
            rgb: vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]],
            ..Default::default()
        })
    });
    let out = render(&ramp, &st);
    assert!(mean_lum(&out) > mean_lum(&ramp) + 5.0);
    // Output of a monotone curve on a monotone ramp (green channel) is non-decreasing.
    let row: Vec<u8> = (0..256).map(|x| out.data[x * 3 + 1]).collect();
    assert!(row.windows(2).all(|w| w[1] >= w[0]));
    // Steep, overshoot-prone control points must not overshoot (Fritsch-Carlson).
    let st = adj(|a| {
        a.curve = Some(Curves {
            rgb: vec![[0.0, 0.0], [0.45, 0.1], [0.5, 0.9], [1.0, 1.0]],
            ..Default::default()
        })
    });
    let out = render(&ramp, &st);
    let row: Vec<u8> = (0..256).map(|x| out.data[x * 3 + 1]).collect();
    assert!(row.windows(2).all(|w| w[1] >= w[0]), "curve overshoot");
    // Per-channel curve only changes that channel.
    let st = adj(|a| {
        a.curve = Some(Curves {
            r: vec![[0.0, 0.0], [0.5, 0.8], [1.0, 1.0]],
            ..Default::default()
        })
    });
    let img = solid(8, 8, [128, 128, 128]);
    let o = render(&img, &st);
    assert!(o.data[0] > 150 && o.data[1] == 128 && o.data[2] == 128);
}

#[test]
fn hsl_band_targets_its_hue() {
    let img = macbeth(240, 160);
    let mut m = std::collections::BTreeMap::new();
    m.insert(
        HslBand::Blue,
        Hsl {
            h: 0.0,
            s: -100.0,
            l: 0.0,
        },
    );
    let out = render(&img, &adj(|a| a.hsl = m));
    // Blue patch (index 12, row 2 col 0) gets desaturated; the orange patch (index 6) is intact.
    let px = |i: &RgbImage, idx: u32| {
        let (cx, cy) = (idx % 6, idx / 6);
        let x = cx * 40 + 20;
        let y = cy * 40 + 20;
        let o = ((y * 240 + x) * 3) as usize;
        [i.data[o], i.data[o + 1], i.data[o + 2]]
    };
    let chroma = |p: [u8; 3]| (p.iter().max().unwrap() - p.iter().min().unwrap()) as i32;
    assert!(chroma(px(&out, 12)) < chroma(px(&img, 12)) / 2);
    assert!(chroma(px(&out, 6)).abs_diff(chroma(px(&img, 6))) <= 6);
}

#[test]
fn grading_tints_shadows() {
    let dark = solid(8, 8, [40, 40, 40]);
    let st = adj(|a| {
        a.grading = Some(Grading {
            shadows: [250.0, 0.6], // blue
            ..Default::default()
        })
    });
    let o = render(&dark, &st);
    assert!(o.data[2] > o.data[0] + 5, "{:?}", &o.data[..3]);
}

// ---------------------------------------------------------------- geometry

#[test]
fn crop_selects_the_right_region() {
    let mut img = solid(200, 200, [0, 0, 0]);
    let quad = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 0]];
    for y in 0..200usize {
        for x in 0..200usize {
            let q = (y / 100) * 2 + x / 100;
            let i = (y * 200 + x) * 3;
            img.data[i..i + 3].copy_from_slice(&quad[q]);
        }
    }
    let st = stack(vec![Op::Crop(Crop {
        rect: [0.5, 0.0, 0.5, 0.5],
        angle: 0.0,
        aspect: None,
    })]);
    let out = render(&img, &st);
    assert_eq!((out.width, out.height), (100, 100));
    assert!(out.data.chunks_exact(3).all(|p| p == [0, 255, 0]));
    let st = stack(vec![Op::Crop(Crop {
        rect: [0.25, 0.25, 0.5, 0.5],
        angle: 0.0,
        aspect: None,
    })]);
    let out = render(&img, &st);
    assert_eq!((out.width, out.height), (100, 100));
    let at = |x: usize, y: usize| &out.data[(y * 100 + x) * 3..(y * 100 + x) * 3 + 3];
    assert_eq!(at(10, 10), [255, 0, 0]);
    assert_eq!(at(90, 10), [0, 255, 0]);
    assert_eq!(at(10, 90), [0, 0, 255]);
    assert_eq!(at(90, 90), [255, 255, 0]);
}

#[test]
fn straighten_rotates_counter_clockwise() {
    // Left half white, right half black; a vertical edge through the centre.
    let mut img = solid(200, 200, [0, 0, 0]);
    for y in 0..200usize {
        for x in 0..100usize {
            let i = (y * 200 + x) * 3;
            img.data[i..i + 3].copy_from_slice(&[255, 255, 255]);
        }
    }
    let st = stack(vec![Op::Crop(Crop {
        rect: [0.2, 0.2, 0.6, 0.6],
        angle: 10.0,
        aspect: None,
    })]);
    let out = render(&img, &st);
    let edge = |y: usize| {
        (0..out.width as usize)
            .take_while(|&x| out.data[(y * out.width as usize + x) * 3] > 127)
            .count()
    };
    let (top, bottom) = (edge(2), edge(out.height as usize - 3));
    assert!(
        top < bottom,
        "CCW rotation: top edge {top} should be left of bottom {bottom}"
    );
    // 0.6*200 = 120 px tall; tan(10 deg) * 115 ~ 20 px apart on each side.
    assert!(bottom - top > 15, "{top} {bottom}");
    // Centre of the rotation stays in place.
    assert!(edge(60).abs_diff(60) <= 2);
}

#[test]
fn max_long_edge_resizes_and_preserves_flat_colour() {
    let img = solid(300, 200, [90, 140, 200]);
    let out = render_with(cpu().as_ref(), &img, &EditStack::default(), Some(150));
    assert_eq!((out.width, out.height), (150, 100));
    assert!(out.data.chunks_exact(3).all(|p| p == [90, 140, 200]));
    // Never upscales.
    let out = render_with(cpu().as_ref(), &img, &EditStack::default(), Some(900));
    assert_eq!((out.width, out.height), (300, 200));
    // Box-averaged downscale of a checker is the mean.
    let mut chk = solid(64, 64, [0, 0, 0]);
    for y in 0..64usize {
        for x in 0..64usize {
            if (x + y) % 2 == 0 {
                let i = (y * 64 + x) * 3;
                chk.data[i..i + 3].copy_from_slice(&[255, 255, 255]);
            }
        }
    }
    let out = render_with(cpu().as_ref(), &chk, &EditStack::default(), Some(16));
    let m = mean_rgb(&out)[1];
    // 50 % linear mix = sRGB ~188.
    assert!((m - 188.0).abs() < 6.0, "{m}");
}

// ---------------------------------------------------------------- local adjustments

#[test]
fn radial_linear_and_invert_masks() {
    let img = solid(200, 200, [100, 100, 100]);
    let lum_at = |i: &RgbImage, x: usize, y: usize| i.data[(y * i.width as usize + x) * 3] as i32;
    let radial = |invert: bool| {
        stack(vec![Op::Local(LocalAdjust {
            mask: MaskRef::Radial {
                center: [0.5, 0.5],
                radius: [0.3, 0.3],
                feather: 0.5,
            },
            amount: 1.0,
            invert,
            adjust: Adjust {
                exposure: 1.0,
                ..Default::default()
            },
        })])
    };
    let o = render(&img, &radial(false));
    assert!(lum_at(&o, 100, 100) > 130 && lum_at(&o, 5, 5) == 100);
    let o = render(&img, &radial(true));
    assert!(lum_at(&o, 100, 100) == 100 && lum_at(&o, 5, 5) > 130);
    let lin = stack(vec![Op::Local(LocalAdjust {
        mask: MaskRef::Linear {
            start: [0.5, 0.0],
            end: [0.5, 0.5],
        },
        amount: 1.0,
        invert: false,
        adjust: Adjust {
            exposure: -1.0,
            ..Default::default()
        },
    })]);
    let o = render(&img, &lin);
    assert!(lum_at(&o, 100, 2) < 75 && lum_at(&o, 100, 150) == 100);
    // amount scales the effect.
    let half = stack(vec![Op::Local(LocalAdjust {
        mask: MaskRef::Radial {
            center: [0.5, 0.5],
            radius: [0.3, 0.3],
            feather: 0.5,
        },
        amount: 0.5,
        invert: false,
        adjust: Adjust {
            exposure: 1.0,
            ..Default::default()
        },
    })]);
    let (f, h) = (
        lum_at(&render(&img, &radial(false)), 100, 100),
        lum_at(&render(&img, &half), 100, 100),
    );
    assert!(h > 100 && h < f);
}

#[test]
fn ai_mask_is_resampled_edge_aware() {
    let img = scene(240, 180, 21);
    let masks = SyntheticMasks(blob_mask(32, 24));
    let st = stack(vec![Op::Local(LocalAdjust {
        mask: MaskRef::Ai {
            target: MaskTarget::Subject,
            person_id: None,
        },
        amount: 1.0,
        invert: false,
        adjust: Adjust {
            exposure: 1.0,
            ..Default::default()
        },
    })]);
    let out = cpu()
        .render(&RenderRequest {
            source: &img,
            stack: &st,
            masks: &masks,
            luts: &NoAssets,
            max_long_edge: None,
        })
        .unwrap();
    let centre = (90 * 240 + 120) * 3;
    let corner = 3;
    assert!(out.data[centre] as i32 > img.data[centre] as i32 + 10);
    assert_eq!(out.data[corner], img.data[corner]);
    // A provider with no mask skips the op.
    let skipped = cpu()
        .render(&RenderRequest {
            source: &img,
            stack: &st,
            masks: &NoAssets,
            luts: &NoAssets,
            max_long_edge: None,
        })
        .unwrap();
    assert_eq!(max_abs_diff(&img, &skipped), 0);
}

#[test]
fn output_sharpen_increases_edge_contrast_only_in_luma() {
    let mut img = solid(64, 64, [100, 100, 100]);
    for y in 0..64usize {
        for x in 32..64usize {
            let i = (y * 64 + x) * 3;
            img.data[i..i + 3].copy_from_slice(&[160, 160, 160]);
        }
    }
    let st = stack(vec![Op::OutputSharpen(OutputSharpen { amount: 80.0 })]);
    let o = render(&img, &st);
    let p = |x: usize| o.data[(10 * 64 + x) * 3] as i32;
    assert!(p(31) < 100 && p(32) > 160, "{} {}", p(31), p(32));
    assert_eq!(p(5), 100);
    // Colour is not shifted.
    assert!(o.data.chunks_exact(3).all(|c| c[0] == c[1] && c[1] == c[2]));
}

// ---------------------------------------------------------------- LUT & cube

#[test]
fn cube_identity_roundtrip_through_text() {
    let id = identity_lut(9);
    let text = write_cube(&id, "id");
    let parsed = parse_cube(&text).unwrap();
    assert_eq!(parsed.size, 9);
    for (a, b) in id.data.iter().zip(&parsed.data) {
        for k in 0..3 {
            assert!((a[k] - b[k]).abs() < 1e-5);
        }
    }
    struct P(Lut3d);
    impl LutProvider for P {
        fn lut(&self, _: &str) -> Result<Option<Lut3d>> {
            Ok(Some(self.0.clone()))
        }
    }
    let img = macbeth(120, 80);
    let st = stack(vec![Op::Lut(Lut {
        file: "x.cube".into(),
        amount: 1.0,
    })]);
    let out = cpu()
        .render(&RenderRequest {
            source: &img,
            stack: &st,
            masks: &NoAssets,
            luts: &P(parsed),
            max_long_edge: None,
        })
        .unwrap();
    assert!(max_abs_diff(&img, &out) <= 1);
}

#[test]
fn lut_amount_blends() {
    // LUT that inverts; amount 0.5 gives mid-grey.
    let n = 5u32;
    let mut inv = identity_lut(n);
    for e in inv.data.iter_mut() {
        *e = [1.0 - e[0], 1.0 - e[1], 1.0 - e[2]];
    }
    struct P(Lut3d);
    impl LutProvider for P {
        fn lut(&self, _: &str) -> Result<Option<Lut3d>> {
            Ok(Some(self.0.clone()))
        }
    }
    let img = solid(8, 8, [30, 30, 30]);
    let run = |amount: f32| {
        cpu()
            .render(&RenderRequest {
                source: &img,
                stack: &stack(vec![Op::Lut(Lut {
                    file: "inv".into(),
                    amount,
                })]),
                masks: &NoAssets,
                luts: &P(inv.clone()),
                max_long_edge: None,
            })
            .unwrap()
    };
    assert!(run(1.0).data[0] > 220);
    let half = run(0.5).data[0] as i32;
    assert!((half - 128).abs() <= 3, "{half}");
    assert_eq!(max_abs_diff(&run(0.0), &img), 0);
}

#[test]
fn cube_parser_edge_cases() {
    let n2 = "LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n";
    let l = parse_cube(n2).unwrap();
    assert_eq!(l.size, 2);
    assert_eq!(l.data[1], [1.0, 0.0, 0.0], "red varies fastest");
    assert_eq!(l.data[2], [0.0, 1.0, 0.0]);
    // Comments, title, CRLF, blank lines, trailing comments, BOM, lowercase keyword.
    let messy = "\u{feff}# made by hand\r\nTITLE \"Hello # not a comment\"\r\n\r\nlut_3d_size 2 # size\r\nDOMAIN_MIN 0 0 0\r\nDOMAIN_MAX 1.0 1.0 1.0\r\n0 0 0\r\n1 0 0 # r\r\n0 1 0\r\n1 1 0\r\n0 0 1\r\n1 0 1\r\n0 1 1\r\n1 1 1\r\n";
    assert_eq!(parse_cube(messy).unwrap().data.len(), 8);
    // Scientific notation / negative numbers.
    let sci = n2.replace("1 1 1\n", "1e0 1.0E+0 +1.0\n");
    assert_eq!(parse_cube(&sci).unwrap().data[7], [1.0, 1.0, 1.0]);
    // Errors.
    assert!(parse_cube("0 0 0\n").is_err(), "missing size");
    assert!(parse_cube("LUT_3D_SIZE 2\n0 0 0\n").is_err(), "short data");
    assert!(
        parse_cube(&format!("{n2}0 0 0\n")).is_err(),
        "too much data"
    );
    assert!(
        parse_cube("LUT_1D_SIZE 4\n0 0 0\n").is_err(),
        "1D unsupported"
    );
    assert!(
        parse_cube("LUT_3D_SIZE 1\n0 0 0\n").is_err(),
        "size too small"
    );
    assert!(parse_cube("LUT_3D_SIZE abc\n").is_err());
    assert!(
        parse_cube(&n2.replace("0 1 1", "0 1 x")).is_err(),
        "bad number"
    );
    assert!(
        parse_cube(&n2.replace("0 1 1", "0 1 1 1")).is_err(),
        "extra column"
    );
    assert!(parse_cube("LUT_3D_SIZE 2\nDOMAIN_MIN 1 1 1\nDOMAIN_MAX 0 0 0\n").is_err());
    // Non-default domain is rebased onto 0..1: identity over 0..2 stays "identity * 2".
    let dom = n2.replace(
        "LUT_3D_SIZE 2\n",
        "LUT_3D_SIZE 2\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 2 2 2\n",
    );
    let r = parse_cube(&dom).unwrap();
    assert_eq!(r.data.len(), 8);
    assert!(r
        .data
        .iter()
        .all(|c| c.iter().all(|v| (0.0..=1.0).contains(v))));
}

// ---------------------------------------------------------------- presets / auto

#[test]
fn presets_are_complete_and_render() {
    let ps = builtin_presets();
    let ids: Vec<&str> = ps.iter().map(|p| p.0.as_str()).collect();
    for want in [
        "natural",
        "vivid",
        "film_warm",
        "film_cool",
        "japanese_clean",
        "cinematic",
        "bw_classic",
        "bw_contrast",
        "portrait_soft",
        "landscape_pop",
    ] {
        assert!(ids.contains(&want), "missing preset {want}");
    }
    let img = scene(120, 90, 4);
    for (id, name, st) in &ps {
        assert_eq!(name, &format!("preset.{id}"));
        assert!(!st.is_identity());
        // JSON round trip.
        let j = serde_json::to_string(st).unwrap();
        assert_eq!(&serde_json::from_str::<EditStack>(&j).unwrap(), st);
        for op in &st.ops {
            if let Op::Lut(l) = op {
                assert!(
                    builtin_lut_ids().contains(&l.file.as_str()) && builtin_lut(&l.file).is_some(),
                    "{id}: lut {}",
                    l.file
                );
            }
        }
        let out = render(&img, st);
        assert_eq!((out.width, out.height), (120, 90));
        assert!(max_abs_diff(&img, &out) > 0, "{id} should change the image");
    }
    // B&W presets really are B&W.
    for id in ["bw_classic", "bw_contrast"] {
        let st = &ps.iter().find(|p| p.0 == id).unwrap().2;
        let out = render(&img, st);
        let worst = out
            .data
            .chunks_exact(3)
            .map(|p| p.iter().max().unwrap() - p.iter().min().unwrap())
            .max()
            .unwrap();
        assert!(worst <= 6, "{id}: colour leaked ({worst})");
    }
}

#[test]
fn auto_dark_image_gets_positive_exposure() {
    let dark = tint(&scene(256, 192, 2), [0.25, 0.25, 0.25]);
    let a = auto_adjust(&dark, &AutoContext::default(), AutoMode::Auto);
    assert!(a.exposure > 0.5, "{a:?}");
    assert_eq!(a.source.as_deref(), Some("ai_auto@1"));
    let bright = tint(&scene(256, 192, 2), [1.0, 1.0, 1.0]);
    let bright = RgbImage {
        data: bright
            .data
            .iter()
            .map(|&v| (v as f32 * 0.5 + 128.0) as u8)
            .collect(),
        ..bright
    };
    let b = auto_adjust(&bright, &AutoContext::default(), AutoMode::Auto);
    assert!(b.exposure < 0.0, "{b:?}");
}

#[test]
fn auto_blue_cast_gets_positive_temp() {
    let base = macbeth(240, 160);
    let blue = tint(&base, [0.8, 0.95, 1.25]);
    let a = auto_adjust(&blue, &AutoContext::default(), AutoMode::Auto);
    assert!(a.temp > 150.0, "{a:?}");
    let warm = tint(&base, [1.25, 1.0, 0.75]);
    let w = auto_adjust(&warm, &AutoContext::default(), AutoMode::Auto);
    assert!(w.temp < -150.0, "{w:?}");
    // Applying the suggestion reduces the cast.
    let st = stack(vec![global(a)]);
    let fixed = mean_rgb(&render(&blue, &st));
    let m0 = mean_rgb(&blue);
    assert!((fixed[2] / fixed[0] - 1.0).abs() < (m0[2] / m0[0] - 1.0).abs());
}

#[test]
fn auto_good_image_is_near_zero_and_deterministic() {
    let good = scene(512, 384, 8);
    let a = auto_adjust(&good, &AutoContext::default(), AutoMode::Auto);
    let b = auto_adjust(&good, &AutoContext::default(), AutoMode::Auto);
    assert_eq!(a, b, "deterministic");
    assert!(a.exposure.abs() < 0.35, "{a:?}");
    for (n, v) in [
        ("contrast", a.contrast),
        ("highlights", a.highlights),
        ("shadows", a.shadows),
        ("whites", a.whites),
        ("blacks", a.blacks),
        ("vibrance", a.vibrance),
        ("clarity", a.clarity),
        ("dehaze", a.dehaze),
        ("tint", a.tint),
    ] {
        assert!(v.abs() <= 30.0, "{n}={v} {a:?}");
    }
    // The macbeth chart is "already right".
    let m = auto_adjust(&macbeth(240, 160), &AutoContext::default(), AutoMode::Auto);
    assert!(m.temp.abs() <= 400.0 && m.exposure.abs() < 0.6, "{m:?}");
    // Speed: well under budget even in debug-ish builds on a 1024 px proxy.
    let big = scene(1024, 768, 8);
    let t = std::time::Instant::now();
    let _ = auto_adjust(&big, &AutoContext::default(), AutoMode::Auto);
    assert!(t.elapsed().as_millis() < 200, "{:?}", t.elapsed());
}

#[test]
fn auto_scene_rules() {
    let img = scene(256, 192, 6);
    let ctx = |s: &str| AutoContext {
        scene_type: Some(s.into()),
        faces: vec![],
    };
    let land = auto_adjust(&img, &ctx("landscape"), AutoMode::Auto);
    let port = auto_adjust(&img, &ctx("portrait"), AutoMode::Auto);
    let night = auto_adjust(&img, &ctx("night"), AutoMode::Auto);
    let plain = auto_adjust(&img, &ctx("other"), AutoMode::Auto);
    assert!(land.vibrance > plain.vibrance);
    assert!(port.clarity < plain.clarity);
    assert!(port.contrast < plain.contrast + 1.0);
    assert!(night.exposure <= 0.5 && night.blacks <= 0.0);
    let forced = auto_adjust(&img, &ctx("other"), AutoMode::Landscape);
    assert_eq!(forced.vibrance, land.vibrance);
}

#[test]
fn auto_backlit_face_lifts_shadows_and_exposure() {
    // Bright background with a dark face region.
    let mut img = solid(256, 256, [200, 205, 215]);
    for y in 80..180usize {
        for x in 90..170usize {
            let i = (y * 256 + x) * 3;
            img.data[i..i + 3].copy_from_slice(&[70, 50, 40]);
        }
    }
    let faces = vec![[90.0 / 256.0, 80.0 / 256.0, 80.0 / 256.0, 100.0 / 256.0]];
    let with = auto_adjust(
        &img,
        &AutoContext {
            scene_type: Some("portrait".into()),
            faces,
        },
        AutoMode::Auto,
    );
    let without = auto_adjust(&img, &AutoContext::default(), AutoMode::Auto);
    assert!(
        with.shadows > without.shadows + 5.0,
        "{with:?} vs {without:?}"
    );
    assert!(with.exposure > without.exposure, "{with:?} vs {without:?}");
}

#[test]
fn auto_skin_anchoring_steers_white_balance() {
    // Skin patch with a green cast inside the face box.
    let mut img = tint(&scene(256, 256, 13), [1.0, 1.0, 1.0]);
    for y in 60..200usize {
        for x in 80..180usize {
            let i = (y * 256 + x) * 3;
            img.data[i..i + 3].copy_from_slice(&[196, 190, 140]); // greenish-yellow skin
        }
    }
    let faces = vec![[80.0 / 256.0, 60.0 / 256.0, 100.0 / 256.0, 140.0 / 256.0]];
    let with = auto_adjust(
        &img,
        &AutoContext {
            scene_type: Some("portrait".into()),
            faces,
        },
        AutoMode::Auto,
    );
    let without = auto_adjust(
        &img,
        &AutoContext {
            scene_type: Some("portrait".into()),
            faces: vec![],
        },
        AutoMode::Auto,
    );
    // Skin that is too green/yellow is pushed towards magenta and/or cooler.
    assert!(
        with.tint > without.tint + 2.0 || with.temp < without.temp - 100.0,
        "{with:?} vs {without:?}"
    );
}

// ---------------------------------------------------------------- CPU vs GPU parity

fn parity_case(
    gpu: &dyn Renderer,
    img: &RgbImage,
    st: &EditStack,
    masks: &dyn MaskProvider,
    max: Option<u32>,
    label: &str,
) -> (f64, f64) {
    let c = create_renderer(false);
    let req = RenderRequest {
        source: img,
        stack: st,
        masks,
        luts: &NoAssets,
        max_long_edge: max,
    };
    let a = c.render(&req).unwrap();
    let b = gpu.render(&req).unwrap();
    let (mean, max) = compare(&a, &b);
    println!("parity {label}: mean dE {mean:.4}, max dE {max:.3}");
    (mean, max)
}

#[test]
fn gpu_cpu_parity_on_random_stacks() {
    let Some(gpu) = create_strict_gpu_renderer() else {
        eprintln!("SKIP gpu_cpu_parity_on_random_stacks: no GPU adapter");
        return;
    };
    println!("GPU: {:?}", gpu_adapter_name());
    let imgs = [
        ("scene", scene(320, 240, 1)),
        ("macbeth", macbeth(300, 200)),
        ("skin", skin_patch(160, 120)),
        ("gradient", gradient(256, 160)),
        ("noise", noise(128, 96, 5)),
    ];
    let masks = SyntheticMasks(blob_mask(48, 36));
    let (mut worst_mean, mut worst_max) = (0.0f64, 0.0f64);
    for seed in 0..24u64 {
        let (name, img) = &imgs[(seed % imgs.len() as u64) as usize];
        let mut st = random_stack(1000 + seed);
        if seed % 3 == 0 {
            st.ops.push(Op::Local(LocalAdjust {
                mask: MaskRef::Ai {
                    target: MaskTarget::Subject,
                    person_id: None,
                },
                amount: 0.9,
                invert: seed % 2 == 0,
                adjust: random_adjust(&mut ip_render::testutil::Rng::new(seed), 0.5),
            }));
        }
        let max = if seed % 4 == 1 { Some(150) } else { None };
        let (m, x) = parity_case(
            gpu.as_ref(),
            img,
            &st,
            &masks,
            max,
            &format!("{name}#{seed}"),
        );
        worst_mean = worst_mean.max(m);
        worst_max = worst_max.max(x);
        assert!(m < 1.0, "{name}#{seed}: mean dE {m}");
        assert!(x < 3.0, "{name}#{seed}: max dE {x}");
    }
    println!("parity worst: mean {worst_mean:.4} max {worst_max:.3}");
}

#[test]
fn gpu_cpu_parity_identity_presets_and_downscale() {
    let Some(gpu) = create_strict_gpu_renderer() else {
        eprintln!("SKIP gpu_cpu_parity_identity_presets_and_downscale: no GPU adapter");
        return;
    };
    let img = scene(640, 480, 9);
    // Identity is exact on the GPU too.
    let id = render_with(gpu.as_ref(), &img, &EditStack::default(), None);
    assert_eq!(max_abs_diff(&img, &id), 0);
    // Presets.
    for (id, _, st) in builtin_presets() {
        let (m, x) = parity_case(gpu.as_ref(), &img, &st, &NoAssets, None, &id);
        assert!(m < 1.0 && x < 3.0, "preset {id}: {m} {x}");
    }
    // Heavy downscale + rotation (supersampled geometry).
    let st = stack(vec![
        Op::Crop(Crop {
            rect: [0.05, 0.05, 0.9, 0.9],
            angle: -7.5,
            aspect: None,
        }),
        global(Adjust {
            exposure: 0.4,
            contrast: 20.0,
            ..Default::default()
        }),
    ]);
    let (m, x) = parity_case(gpu.as_ref(), &img, &st, &NoAssets, Some(100), "downscale");
    assert!(m < 1.0 && x < 3.0, "{m} {x}");
}

#[test]
fn gpu_tiling_matches_untiled_semantics() {
    // Many rows with a sharpen (neighbour reads cross tile edges only through recompute).
    let Some(gpu) = create_strict_gpu_renderer() else {
        eprintln!("SKIP gpu_tiling: no GPU adapter");
        return;
    };
    let img = scene(1500, 1100, 17);
    let st = stack(vec![
        global(Adjust {
            clarity: 30.0,
            shadows: 30.0,
            vibrance: 20.0,
            ..Default::default()
        }),
        Op::OutputSharpen(OutputSharpen { amount: 40.0 }),
    ]);
    let (m, x) = parity_case(gpu.as_ref(), &img, &st, &NoAssets, None, "1.6MP");
    assert!(m < 1.0 && x < 3.0, "{m} {x}");
}
