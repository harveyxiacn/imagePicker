//! `cargo run --release -p ip-render --example bench_render`
//! 24 MP synthetic source, heavy stack; CPU vs GPU for preview / drag / export.
use std::collections::BTreeMap;
use std::time::Instant;

use ip_render::testutil::*;
use ip_render::*;

fn heavy_stack() -> EditStack {
    let mut hsl = BTreeMap::new();
    hsl.insert(
        HslBand::Orange,
        Hsl {
            h: 5.0,
            s: -8.0,
            l: 6.0,
        },
    );
    hsl.insert(
        HslBand::Blue,
        Hsl {
            h: -6.0,
            s: 15.0,
            l: -8.0,
        },
    );
    hsl.insert(
        HslBand::Green,
        Hsl {
            h: 10.0,
            s: -10.0,
            l: 0.0,
        },
    );
    let global = Adjust {
        exposure: 0.35,
        contrast: 14.0,
        highlights: -35.0,
        shadows: 28.0,
        whites: 8.0,
        blacks: -6.0,
        temp: 300.0,
        tint: 4.0,
        vibrance: 20.0,
        saturation: 4.0,
        clarity: 18.0,
        dehaze: 12.0,
        curve: Some(Curves {
            rgb: vec![[0.0, 0.02], [0.25, 0.23], [0.75, 0.8], [1.0, 1.0]],
            ..Default::default()
        }),
        hsl,
        grading: Some(Grading {
            shadows: [220.0, 0.2],
            midtones: [0.0, 0.0],
            highlights: [40.0, 0.15],
            balance: 0.0,
        }),
        ..Default::default()
    };
    EditStack {
        version: 1,
        ops: vec![
            Op::Crop(Crop {
                rect: [0.02, 0.03, 0.95, 0.94],
                angle: -1.2,
                aspect: None,
            }),
            Op::Global(global),
            Op::Local(LocalAdjust {
                mask: MaskRef::Ai {
                    target: MaskTarget::Subject,
                    person_id: None,
                },
                amount: 1.0,
                invert: false,
                adjust: Adjust {
                    exposure: 0.3,
                    shadows: 20.0,
                    clarity: 10.0,
                    ..Default::default()
                },
            }),
            Op::Local(LocalAdjust {
                mask: MaskRef::Linear {
                    start: [0.5, 0.0],
                    end: [0.5, 0.45],
                },
                amount: 1.0,
                invert: false,
                adjust: Adjust {
                    exposure: -0.4,
                    dehaze: 20.0,
                    saturation: 10.0,
                    ..Default::default()
                },
            }),
            Op::Lut(Lut {
                file: "film_warm".into(),
                amount: 0.5,
            }),
            Op::OutputSharpen(OutputSharpen { amount: 25.0 }),
        ],
    }
}

/// Face warp + body warp + full beauty (the M4 portrait stack) on top of a light global edit.
fn portrait_stack() -> EditStack {
    EditStack {
        version: 1,
        ops: vec![
            Op::Global(Adjust {
                exposure: 0.2,
                contrast: 8.0,
                shadows: 15.0,
                ..Default::default()
            }),
            Op::Warp(Warp::Face {
                person_id: None,
                level: Level::Standard,
                slim: 60.0,
                chin: 30.0,
                eyes: 50.0,
                nose: 30.0,
            }),
            Op::Warp(Warp::Body {
                person_id: None,
                level: Level::Standard,
                arms: 50.0,
                legs: 40.0,
                waist: 40.0,
                lengthen_legs: 30.0,
                protect_background: true,
            }),
            Op::Beauty(Beauty {
                person_id: None,
                level: Level::Standard,
                smooth: 60.0,
                whiten: 40.0,
                blemish: true,
                eye_brighten: 40.0,
                teeth_whiten: 40.0,
                dark_circles: 40.0,
            }),
        ],
    }
}

fn time(
    r: &dyn Renderer,
    src: &RgbImage,
    st: &EditStack,
    masks: &dyn MaskProvider,
    max: Option<u32>,
    runs: usize,
) -> f64 {
    let req = RenderRequest {
        source: src,
        stack: st,
        masks,
        luts: &NoAssets,
        max_long_edge: max,
    };
    let _ = r.render(&req).unwrap(); // warm-up (device init, pipelines)
    let mut best = f64::MAX;
    for _ in 0..runs {
        let t = Instant::now();
        let out = r.render(&req).unwrap();
        std::hint::black_box(&out);
        best = best.min(t.elapsed().as_secs_f64() * 1000.0);
    }
    best
}

fn main() {
    let (w, h) = (6000u32, 4000u32);
    println!(
        "generating {}x{} ({} MP) source...",
        w,
        h,
        w * h / 1_000_000
    );
    let big = {
        let tile = scene(750, 500, 3);
        let mut data = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h as usize {
            let ty = (y * 500 / h as usize).min(499);
            for x in 0..w as usize {
                let tx = (x * 750 / w as usize).min(749);
                let i = (ty * 750 + tx) * 3;
                let n = ((x * 7 + y * 13) % 5) as u8;
                data.push(tile.data[i].saturating_add(n));
                data.push(tile.data[i + 1].saturating_add(n));
                data.push(tile.data[i + 2].saturating_add(n));
            }
        }
        RgbImage {
            width: w,
            height: h,
            data,
        }
    };
    let masks = SyntheticMasks(blob_mask(1024, 683));
    let st = heavy_stack();
    let cpu = create_renderer(false);
    let gpu = create_renderer(true);
    println!(
        "cpu threads: {}, gpu: {:?} (backend {:?})",
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1),
        gpu_adapter_name(),
        gpu.backend()
    );
    // Pre-scaled proxy source, as ip-core would hand to the preview path.
    let proxy = cpu
        .render(&RenderRequest {
            source: &big,
            stack: &EditStack::default(),
            masks: &NoAssets,
            luts: &NoAssets,
            max_long_edge: Some(2400),
        })
        .unwrap();
    println!("{:<34} {:>10} {:>10}", "case", "CPU ms", "GPU ms");
    let cases: [(&str, &RgbImage, Option<u32>, usize); 5] = [
        ("preview 1600 (from 2400 proxy)", &proxy, Some(1600), 5),
        ("drag 800 (from 2400 proxy)", &proxy, Some(800), 5),
        ("preview 1600 (from 24 MP)", &big, Some(1600), 3),
        ("drag 800 (from 24 MP)", &big, Some(800), 3),
        ("export full-res 24 MP", &big, None, 2),
    ];
    for (name, src, max, runs) in cases {
        let c = time(cpu.as_ref(), src, &st, &masks, max, runs);
        let g = if gpu.backend() == Backend::Gpu {
            format!("{:10.1}", time(gpu.as_ref(), src, &st, &masks, max, runs))
        } else {
            "      n/a".into()
        };
        println!("{name:<34} {c:>10.1} {g}");
    }

    // ---- portrait stack (face warp + body warp + beauty, all on)
    println!(
        "
portrait stack (synthetic 3:4 portrait, people geometry from testutil)"
    );
    let (pw, ph) = (4000u32, 6000u32);
    let t = Instant::now();
    let mut portrait = synth_portrait(pw, ph);
    portrait.person.blemishes = (0..24)
        .map(|i| {
            let a = i as f32 * 0.7;
            [
                portrait.face[0] + 0.5 * portrait.face[2] * a.cos() * (0.4 + (i % 3) as f32 * 0.2),
                portrait.face[1] + 0.6 * portrait.face[3] * a.sin() * (0.4 + (i % 4) as f32 * 0.15),
                (3.0 + (i % 4) as f32) / ph as f32,
            ]
        })
        .collect();
    println!(
        "generated {}x{} portrait in {:.1}s",
        pw,
        ph,
        t.elapsed().as_secs_f64()
    );
    // ---- patch stack (full-frame denoise-like + two face-sized patches)
    println!("\npatch stack (3 patches: full-frame denoise 0.8, two face-sized, feathered)");
    let assets = PatchAssets::default()
        .with("dn", synth_patch(3000, 2000, 1, None))
        .with("f1", synth_patch(512, 512, 2, Some(1.0)))
        .with("f2", synth_patch(512, 512, 3, Some(1.0)));
    let mk = |kind, asset: &str, rect, feather, amount| {
        Op::Patch(Patch {
            kind,
            asset: asset.into(),
            rect,
            feather,
            amount,
            enabled: true,
            person_id: None,
            source_photo_id: None,
        })
    };
    let patch_st = EditStack {
        version: 1,
        ops: vec![
            mk(PatchKind::Denoise, "dn", [0.0, 0.0, 1.0, 1.0], 0.0, 0.8),
            mk(
                PatchKind::BestTake,
                "f1",
                [0.2, 0.25, 0.12, 0.18],
                0.15,
                1.0,
            ),
            mk(PatchKind::BestTake, "f2", [0.6, 0.3, 0.12, 0.18], 0.15, 1.0),
            Op::Crop(Crop {
                rect: [0.02, 0.03, 0.95, 0.94],
                angle: -1.2,
                aspect: None,
            }),
            Op::Global(Adjust {
                exposure: 0.2,
                contrast: 10.0,
                ..Default::default()
            }),
        ],
    };
    println!("{:<34} {:>10} {:>10}", "case", "CPU ms", "GPU ms");
    let cases: [(&str, &RgbImage, Option<u32>, usize); 3] = [
        ("preview 1600 (from 2400 proxy)", &proxy, Some(1600), 5),
        ("preview 1600 (from 24 MP)", &big, Some(1600), 3),
        ("export full-res 24 MP", &big, None, 2),
    ];
    for (name, src, max, runs) in cases {
        let c = time(cpu.as_ref(), src, &patch_st, &assets, max, runs);
        let g = if gpu.backend() == Backend::Gpu {
            format!(
                "{:10.1}",
                time(gpu.as_ref(), src, &patch_st, &assets, max, runs)
            )
        } else {
            "      n/a".into()
        };
        println!("{name:<34} {c:>10.1} {g}");
    }

    let people = PortraitMasks(vec![portrait.person.clone()]);
    let pst = portrait_stack();
    let pproxy = cpu
        .render(&RenderRequest {
            source: &portrait.image,
            stack: &EditStack::default(),
            masks: &NoAssets,
            luts: &NoAssets,
            max_long_edge: Some(2400),
        })
        .unwrap();
    println!("{:<34} {:>10} {:>10}", "case", "CPU ms", "GPU ms");
    let cases: [(&str, &RgbImage, Option<u32>, usize); 3] = [
        ("preview 1600 (from 2400 proxy)", &pproxy, Some(1600), 5),
        ("preview 1600 (from 24 MP)", &portrait.image, Some(1600), 3),
        ("export full-res 24 MP", &portrait.image, None, 2),
    ];
    for (name, src, max, runs) in cases {
        let c = time(cpu.as_ref(), src, &pst, &people, max, runs);
        let g = if gpu.backend() == Backend::Gpu {
            format!("{:10.1}", time(gpu.as_ref(), src, &pst, &people, max, runs))
        } else {
            "      n/a".into()
        };
        println!("{name:<34} {c:>10.1} {g}");
    }
}
