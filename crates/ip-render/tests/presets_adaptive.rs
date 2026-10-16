//! Adaptive presets (docs/03 §6.1.1): per-preset invariants on synthetic photos.
//! `cargo test -p ip-render --test presets_adaptive -- --nocapture` prints the before/after table.
#![allow(clippy::chunks_exact_to_as_chunks)]
use ip_render::testutil::*;
use ip_render::*;

fn render(img: &RgbImage, st: &EditStack) -> RgbImage {
    create_renderer(false)
        .render(&RenderRequest {
            source: img,
            stack: st,
            masks: &NoAssets,
            luts: &NoAssets,
            max_long_edge: None,
        })
        .expect("render")
}

fn map(img: &RgbImage, f: impl Fn(u8) -> u8) -> RgbImage {
    RgbImage {
        data: img.data.iter().map(|&v| f(v)).collect(),
        ..img.clone()
    }
}

const FACE: [f32; 4] = [0.4, 0.3, 0.2, 0.3];

/// Paint an elliptic skin patch of colour `c` into the face box.
fn with_face(img: &RgbImage, c: [f32; 3]) -> RgbImage {
    let mut o = img.clone();
    let (w, h) = (img.width as f32, img.height as f32);
    let (cx, cy) = ((FACE[0] + FACE[2] / 2.0) * w, (FACE[1] + FACE[3] / 2.0) * h);
    let (rx, ry) = (FACE[2] / 2.0 * w, FACE[3] / 2.0 * h);
    for y in 0..img.height {
        for x in 0..img.width {
            let d = ((x as f32 - cx) / rx).powi(2) + ((y as f32 - cy) / ry).powi(2);
            if d <= 1.0 {
                let shade = 1.0 + 0.1 * ((x as f32 - cx) / rx);
                let i = ((y * img.width + x) * 3) as usize;
                for (k, ck) in c.iter().enumerate() {
                    o.data[i + k] = (ck * shade).clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
    o
}

struct Case {
    name: &'static str,
    img: RgbImage,
    faces: Vec<[f32; 4]>,
}

fn cases() -> Vec<Case> {
    let base = scene(360, 240, 4);
    let none = Vec::new;
    vec![
        Case {
            name: "normal",
            img: base.clone(),
            faces: none(),
        },
        Case {
            name: "dark",
            img: tint(&base, [0.22, 0.22, 0.22]),
            faces: none(),
        },
        Case {
            name: "high-key",
            img: map(&base, |v| (v as f32 * 0.33 + 168.0) as u8),
            faces: none(),
        },
        Case {
            name: "backlit-face",
            img: with_face(
                &map(&base, |v| (v as f32 * 0.25 + 185.0) as u8),
                [96.0, 70.0, 56.0],
            ),
            faces: vec![FACE],
        },
        Case {
            name: "portrait",
            img: with_face(&base, [200.0, 150.0, 125.0]),
            faces: vec![FACE],
        },
        Case {
            name: "snow",
            img: map(&base, |v| (v as f32 * 0.16 + 214.0) as u8),
            faces: none(),
        },
        Case {
            name: "bright-sky",
            img: bright_sky(&base),
            faces: none(),
        },
    ]
}

/// A bright, nearly white sky over the top 45 % (225..=253 with a slight blue), the scene
/// below: real highlight detail just under clipping, which fixed lifts used to blow out.
fn bright_sky(base: &RgbImage) -> RgbImage {
    let mut o = base.clone();
    let h = (base.height as f32 * 0.45) as u32;
    for y in 0..h {
        let t = y as f32 / h as f32;
        for x in 0..base.width {
            let n = ((x * 7 + y * 13) % 5) as f32;
            let v = 253.0 - 28.0 * t - n;
            let i = ((y * base.width + x) * 3) as usize;
            o.data[i] = (v - 6.0) as u8;
            o.data[i + 1] = (v - 2.0) as u8;
            o.data[i + 2] = v as u8;
        }
    }
    o
}

/// The pre-adaptive japanese_clean (fixed +0.35 EV etc.), for the before/after comparison.
fn old_japanese_clean() -> EditStack {
    let mut st = builtin_presets()
        .into_iter()
        .find(|p| p.0 == "japanese_clean")
        .unwrap()
        .2;
    if let Op::Global(a) = &mut st.ops[0] {
        a.exposure = 0.35;
        a.shadows = 24.0;
        a.whites = 10.0;
        a.highlights = -22.0;
        a.curve = Some(Curves {
            rgb: vec![[0.0, 0.05], [0.25, 0.28], [0.75, 0.8], [1.0, 1.0]],
            ..Default::default()
        });
    }
    st
}

fn ids() -> Vec<String> {
    builtin_presets().into_iter().map(|p| p.0).collect()
}

#[test]
fn every_preset_keeps_the_highlight_budget_on_every_photo() {
    println!(
        "{:<14} {:<15} {:>6} {:>6} {:>6} {:>8} {:>8} {:>6} {:>6}",
        "image", "preset", "ev", "med0", "med1", "clip0%", "clip1%", "face0", "face1"
    );
    for c in cases() {
        let ctx = PresetContext {
            faces: c.faces.clone(),
        };
        let before = measure_tone(&c.img, &c.faces);
        for id in ids() {
            let r = resolve_preset(&id, &c.img, &ctx, 1.0).unwrap();
            assert!(r.satisfied, "{} / {id}: {r:?}", c.name);
            // Check on the full-size render too, not only the measuring proxy.
            let after = measure_tone(&render(&c.img, &r.stack), &c.faces);
            let new_clip = after.clipped - before.clipped;
            println!(
                "{:<14} {:<15} {:>6.2} {:>6.3} {:>6.3} {:>8.3} {:>8.3} {:>6} {:>6}",
                c.name,
                id,
                r.exposure,
                before.median,
                after.median,
                before.clipped * 100.0,
                after.clipped * 100.0,
                before.face_median.map_or("-".into(), |v| format!("{v:.3}")),
                after.face_median.map_or("-".into(), |v| format!("{v:.3}")),
            );
            assert!(
                new_clip <= 0.008,
                "{} / {id}: newly clipped {new_clip}",
                c.name
            );
            if let (Some(fb), Some(fa)) = (before.face_clipped, after.face_clipped) {
                assert!(
                    fa - fb <= 0.004,
                    "{} / {id}: face clipped {fb} -> {fa}",
                    c.name
                );
            }
            if let (Some(fb), Some(fa)) = (before.face_median, after.face_median) {
                assert!(
                    fa <= 0.86_f32.max(fb + 0.01),
                    "{} / {id}: face {fb} -> {fa}",
                    c.name
                );
            }
            // Concrete values in validated ranges, provenance recorded.
            let g = r
                .stack
                .ops
                .iter()
                .find_map(|o| match o {
                    Op::Global(a) => Some(a),
                    _ => None,
                })
                .expect("global op");
            assert_eq!(
                parse_preset_source(g.source.as_deref().unwrap()),
                Some((id.clone(), 1.0))
            );
            assert!((-5.0..=5.0).contains(&g.exposure));
        }
    }
}

#[test]
fn japanese_clean_brightens_dark_photos_and_spares_bright_ones() {
    let cs = cases();
    let get = |n: &str| cs.iter().find(|c| c.name == n).unwrap();
    let ctx = PresetContext::default();

    let dark = get("dark");
    let r = resolve_preset("japanese_clean", &dark.img, &ctx, 1.0).unwrap();
    assert!(r.exposure >= 0.4, "{r:?}");
    assert!(r.after.median > r.before.median + 0.1, "{r:?}");

    let hk = get("high-key");
    let r = resolve_preset("japanese_clean", &hk.img, &ctx, 1.0).unwrap();
    assert!(r.exposure <= 0.1, "{r:?}");
    let old = measure_tone(&render(&hk.img, &old_japanese_clean()), &[]);
    let new = measure_tone(&render(&hk.img, &r.stack), &[]);
    let base = measure_tone(&hk.img, &[]);
    println!(
        "japanese_clean on high-key: clipped {:.2}% -> old {:.2}% / adaptive {:.2}%, median {:.3} -> old {:.3} / adaptive {:.3}",
        base.clipped * 100.0,
        old.clipped * 100.0,
        new.clipped * 100.0,
        base.median,
        old.median,
        new.median
    );
    assert!(new.clipped <= old.clipped, "old {old:?} new {new:?}");
    assert!(new.clipped - base.clipped <= 0.005, "{new:?}");

    // Normal photos still get the bright, airy look.
    let n = get("normal");
    let r = resolve_preset("japanese_clean", &n.img, &ctx, 1.0).unwrap();
    assert!(r.after.median > r.before.median + 0.04, "{r:?}");
}

#[test]
fn no_preset_darkens_a_dark_photo_and_old_vs_new_on_bright_photos() {
    let cs = cases();
    let ctx = PresetContext::default();
    let dark = cs.iter().find(|c| c.name == "dark").unwrap();
    for id in ids() {
        let r = resolve_preset(&id, &dark.img, &ctx, 1.0).unwrap();
        assert!(
            r.after.median >= r.before.median - 0.005,
            "{id} darkens a dark photo: {r:?}"
        );
    }
    // The fixed pre-adaptive japanese_clean vs the adaptive one, full-size renders.
    for c in cs.iter().filter(|c| {
        matches!(
            c.name,
            "high-key" | "snow" | "backlit-face" | "normal" | "dark" | "bright-sky"
        )
    }) {
        let ctx = PresetContext {
            faces: c.faces.clone(),
        };
        let base = measure_tone(&c.img, &c.faces);
        let old = measure_tone(&render(&c.img, &old_japanese_clean()), &c.faces);
        let r = resolve_preset("japanese_clean", &c.img, &ctx, 1.0).unwrap();
        let new = measure_tone(&render(&c.img, &r.stack), &c.faces);
        println!(
            "OLDNEW {:<13} clipped {:.2}% -> fixed {:.2}% / adaptive {:.2}% | median {:.3} -> fixed {:.3} / adaptive {:.3} | ev fixed +0.35 / adaptive {:+.2}",
            c.name,
            base.clipped * 100.0,
            old.clipped * 100.0,
            new.clipped * 100.0,
            base.median,
            old.median,
            new.median,
            r.exposure
        );
        assert!(
            new.clipped <= old.clipped + 1e-6,
            "{}: {old:?} {new:?}",
            c.name
        );
    }
}

#[test]
fn fitting_on_an_app_proxy_is_fast_enough() {
    // The app fits on its 1024 px proxy (box-downscaled to <= 256 px for measuring).
    let img = scene(1024, 683, 11);
    let ctx = PresetContext::default();
    let t0 = std::time::Instant::now();
    for id in ids() {
        resolve_preset(&id, &img, &ctx, 1.0).unwrap();
    }
    let ms = t0.elapsed().as_millis();
    println!(
        "TIMING all {} presets on a 1024 px proxy: {ms} ms",
        ids().len()
    );
    // Generous bound (debug build, loaded 2-core CI runner): catches an accidental full-size solve.
    assert!(ms < 120_000, "{ms} ms");
}

#[test]
fn backlit_faces_are_lifted_but_not_blown() {
    let cs = cases();
    let c = cs.iter().find(|c| c.name == "backlit-face").unwrap();
    let ctx = PresetContext {
        faces: c.faces.clone(),
    };
    for id in ["japanese_clean", "portrait_soft", "natural"] {
        let r = resolve_preset(id, &c.img, &ctx, 1.0).unwrap();
        let (fb, fa) = (r.before.face_median.unwrap(), r.after.face_median.unwrap());
        assert!(fa > fb, "{id}: face {fb} -> {fa}");
        assert!(fa < 0.86, "{id}: face {fa}");
        assert!(r.after.face_clipped.unwrap() <= 0.002, "{id}: {r:?}");
    }
}

#[test]
fn strength_scales_the_effect() {
    let cs = cases();
    let c = cs.iter().find(|c| c.name == "dark").unwrap();
    let ctx = PresetContext::default();
    let at = |a: f32| resolve_preset("japanese_clean", &c.img, &ctx, a).unwrap();
    let (r0, r5, r10, r15) = (at(0.0), at(0.5), at(1.0), at(1.5));
    assert!(r0.stack.ops.is_empty(), "{r0:?}");
    let lift = |r: &PresetResolution| r.after.median - r.before.median;
    assert!(
        lift(&r5) > 0.0 && lift(&r5) < lift(&r10),
        "{} {}",
        lift(&r5),
        lift(&r10)
    );
    assert!(
        lift(&r15) >= lift(&r10) - 0.005,
        "{} {}",
        lift(&r15),
        lift(&r10)
    );
    assert!(r15.satisfied);
    assert_eq!(r5.amount, 0.5);
    // Out-of-range strength is clamped.
    assert_eq!(at(9.0).amount, PRESET_AMOUNT_MAX);
}

#[test]
fn resolution_is_deterministic_and_unknown_ids_are_none() {
    let img = scene(300, 200, 7);
    let ctx = PresetContext::default();
    for id in ids() {
        let a = resolve_preset(&id, &img, &ctx, 0.8).unwrap();
        let b = resolve_preset(&id, &img, &ctx, 0.8).unwrap();
        assert_eq!(a.stack, b.stack, "{id}");
    }
    assert!(resolve_preset("no_such", &img, &ctx, 1.0).is_none());
}

#[test]
fn character_is_kept_while_tone_adapts() {
    // Colour / HSL character is the same on a dark and a bright photo; only tone differs.
    let cs = cases();
    let ctx = PresetContext::default();
    let g = |r: &PresetResolution| match &r.stack.ops[0] {
        Op::Global(a) => a.clone(),
        _ => panic!(),
    };
    for id in ids() {
        let d = g(&resolve_preset(&id, &cs[1].img, &ctx, 1.0).unwrap());
        let b = g(&resolve_preset(&id, &cs[2].img, &ctx, 1.0).unwrap());
        assert_eq!(
            (d.temp, d.tint, d.saturation, d.vibrance),
            (b.temp, b.tint, b.saturation, b.vibrance),
            "{id}"
        );
        assert_eq!(d.hsl, b.hsl, "{id}");
    }
}
