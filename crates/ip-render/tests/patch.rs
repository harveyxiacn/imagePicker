//! Patch layer compositing: behaviour and CPU/GPU parity.
use std::sync::atomic::Ordering;

use ip_render::testutil::*;
use ip_render::*;

fn patch(asset: &str, rect: [f32; 4]) -> Patch {
    Patch {
        kind: PatchKind::Inpaint,
        asset: asset.into(),
        rect,
        feather: 0.0,
        amount: 1.0,
        enabled: true,
        person_id: None,
        source_photo_id: None,
    }
}

fn stack(ops: Vec<Op>) -> EditStack {
    EditStack { version: 1, ops }
}

fn render_with(
    r: &dyn Renderer,
    img: &RgbImage,
    st: &EditStack,
    masks: &dyn MaskProvider,
    max: Option<u32>,
) -> RgbImage {
    r.render(&RenderRequest {
        source: img,
        stack: st,
        masks,
        luts: &NoAssets,
        max_long_edge: max,
    })
    .expect("render")
}

fn cpu() -> Box<dyn Renderer> {
    create_renderer(false)
}

fn px(img: &RgbImage, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * img.width + x) * 3) as usize;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

const W: u32 = 400;
const H: u32 = 300;

fn base() -> RgbImage {
    scene(W, H, 11)
}

#[test]
fn identity_outside_rects_is_bit_exact() {
    let img = base();
    let assets = PatchAssets::default()
        .with("a", synth_patch(120, 90, 1, None))
        .with("b", synth_patch(64, 64, 2, Some(0.8)));
    let r1 = [0.1, 0.2, 0.3, 0.3];
    let r2 = [0.55, 0.5, 0.25, 0.3];
    let mut p1 = patch("a", r1);
    p1.feather = 0.2;
    let p2 = patch("b", r2);
    let st = stack(vec![Op::Patch(p1), Op::Patch(p2)]);
    let a = render_with(cpu().as_ref(), &img, &st, &assets, None);
    let none = render_with(cpu().as_ref(), &img, &EditStack::default(), &assets, None);
    assert_eq!(none.data, img.data);
    let inside = |x: u32, y: u32| {
        let (u, v) = (x as f32 / W as f32, y as f32 / H as f32);
        let m = 1.5 / W as f32;
        [r1, r2]
            .iter()
            .any(|r| u > r[0] - m && u < r[0] + r[2] + m && v > r[1] - m && v < r[1] + r[3] + m)
    };
    let (mut changed, mut outside) = (0, 0);
    for y in 0..H {
        for x in 0..W {
            if inside(x, y) {
                if px(&a, x, y) != px(&img, x, y) {
                    changed += 1;
                }
            } else {
                outside += 1;
                assert_eq!(px(&a, x, y), px(&img, x, y), "pixel {x},{y} changed");
            }
        }
    }
    assert!(outside > 1000 && changed > 5000);
}

#[test]
fn disabled_and_missing_assets_are_identity() {
    let img = base();
    let assets = PatchAssets::default().with("a", synth_patch(64, 64, 1, None));
    let mut off = patch("a", [0.0, 0.0, 1.0, 1.0]);
    off.enabled = false;
    let mut zero = patch("a", [0.0, 0.0, 1.0, 1.0]);
    zero.amount = 0.0;
    let missing = patch("nope", [0.0, 0.0, 1.0, 1.0]);
    let bad_rect = patch("a", [0.2, 0.2, 0.0, 0.5]);
    let st = stack(vec![
        Op::Patch(off),
        Op::Patch(zero),
        Op::Patch(missing),
        Op::Patch(bad_rect),
    ]);
    let out = render_with(cpu().as_ref(), &img, &st, &assets, None);
    assert_eq!(out.data, img.data);
}

#[test]
fn opaque_patch_reproduces_the_asset_inside_the_rect() {
    let img = base();
    let asset = synth_patch(160, 120, 5, None);
    let assets = PatchAssets::default().with("a", asset.clone());
    // rect 0.25..0.75 x 0.2..0.8 -> 200x180 px at (100, 60); the asset is resampled to it.
    let st = stack(vec![Op::Patch(patch("a", [0.25, 0.2, 0.5, 0.6]))]);
    let out = render_with(cpu().as_ref(), &img, &st, &assets, None);
    let (mut sum, mut n, mut worst) = (0.0f64, 0, 0.0f64);
    for y in (60 + 3)..(60 + 180 - 3) {
        for x in (100 + 3)..(100 + 200 - 3) {
            let u = (x as f32 + 0.5 - 100.0) / 200.0;
            let v = (y as f32 + 0.5 - 60.0) / 180.0;
            let (ax, ay) = (((u * 160.0) as u32).min(159), ((v * 120.0) as u32).min(119));
            let i = ((ay * 160 + ax) * 4) as usize;
            let want = [asset.data[i], asset.data[i + 1], asset.data[i + 2]];
            let de = delta_e2000(px(&out, x, y), want);
            sum += de;
            n += 1;
            worst = worst.max(de);
        }
    }
    let mean = sum / n as f64;
    println!("opaque patch vs asset: mean dE {mean:.3}, max {worst:.3}");
    assert!(mean < 1.5, "mean {mean}");
    assert!(worst < 6.0, "max {worst}");
}

fn lin(v: f32) -> f32 {
    let c = v / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn enc(l: f32) -> f32 {
    255.0
        * if l <= 0.0031308 {
            l * 12.92
        } else {
            1.055 * l.powf(1.0 / 2.4) - 0.055
        }
}

#[test]
fn amount_blends_halfway_in_linear_light() {
    let img = solid(W, H, [30, 30, 30]);
    let assets = PatchAssets::default().with("w", solid_patch(8, 8, [230, 230, 230]));
    let mut p = patch("w", [0.0, 0.0, 1.0, 1.0]);
    p.amount = 0.5;
    let out = render_with(
        cpu().as_ref(),
        &img,
        &stack(vec![Op::Patch(p)]),
        &assets,
        None,
    );
    let want = enc(0.5 * lin(30.0) + 0.5 * lin(230.0));
    let got = px(&out, 200, 150)[0] as f32;
    assert!((got - want).abs() <= 1.0, "got {got}, want {want}");
}

#[test]
fn later_patches_are_on_top() {
    let img = solid(W, H, [100, 100, 100]);
    let assets = PatchAssets::default()
        .with("dark", solid_patch(4, 4, [20, 20, 20]))
        .with("light", solid_patch(4, 4, [220, 220, 220]));
    let a = patch("dark", [0.2, 0.2, 0.5, 0.5]);
    let b = patch("light", [0.4, 0.4, 0.4, 0.4]);
    let r = |ops: Vec<Op>| render_with(cpu().as_ref(), &img, &stack(ops), &assets, None);
    let ab = r(vec![Op::Patch(a.clone()), Op::Patch(b.clone())]);
    let ba = r(vec![Op::Patch(b), Op::Patch(a)]);
    // Overlap centre (0.5, 0.5).
    assert_eq!(px(&ab, 200, 150), [220, 220, 220]);
    assert_eq!(px(&ba, 200, 150), [20, 20, 20]);
    // Non-overlapping parts agree.
    assert_eq!(px(&ab, 100, 100), px(&ba, 100, 100));
}

#[test]
fn feather_ramps_the_edge_inside_the_rect() {
    let img = solid(W, H, [30, 30, 30]);
    let assets = PatchAssets::default().with("a", solid_patch(8, 8, [220, 220, 220]));
    let mut p = patch("a", [0.25, 0.25, 0.5, 0.5]);
    p.feather = 0.3; // 0.3 * 150 = 45 px
    let out = render_with(
        cpu().as_ref(),
        &img,
        &stack(vec![Op::Patch(p)]),
        &assets,
        None,
    );
    let v = |x: u32| px(&out, x, 150)[0];
    assert_eq!(v(99), 30); // outside the rect
    assert!(v(102) < v(115) && v(115) < v(130) && v(130) < v(144));
    assert_eq!(v(200), 220); // plateau
    assert!(v(102) < 60);
}

#[test]
fn assets_are_fetched_once_per_render() {
    let img = base();
    let assets = PatchAssets::default().with("a", synth_patch(32, 32, 1, None));
    let st = stack(vec![
        Op::Patch(patch("a", [0.0, 0.0, 0.5, 0.5])),
        Op::Patch(patch("a", [0.5, 0.5, 0.5, 0.5])),
        Op::Patch(patch("gone", [0.1, 0.1, 0.2, 0.2])),
    ]);
    render_with(cpu().as_ref(), &img, &st, &assets, None);
    assert_eq!(assets.fetches.load(Ordering::SeqCst), 2);
}

#[test]
fn patch_is_applied_before_crop() {
    let img = solid(W, H, [40, 40, 40]);
    let assets = PatchAssets::default().with("a", solid_patch(8, 8, [200, 200, 200]));
    let p = Op::Patch(patch("a", [0.0, 0.0, 0.5, 1.0]));
    let crop = |x: f32| {
        Op::Crop(Crop {
            rect: [x, 0.25, 0.25, 0.5],
            angle: 0.0,
            aspect: None,
        })
    };
    let left = render_with(
        cpu().as_ref(),
        &img,
        &stack(vec![p.clone(), crop(0.1)]),
        &assets,
        None,
    );
    let right = render_with(
        cpu().as_ref(),
        &img,
        &stack(vec![p, crop(0.6)]),
        &assets,
        None,
    );
    assert_eq!(px(&left, 50, 50), [200, 200, 200]);
    assert_eq!(px(&right, 50, 50), [40, 40, 40]);
}

// ---------------------------------------------------------------- parity

fn parity(img: &RgbImage, st: &EditStack, assets: &PatchAssets, max: Option<u32>, label: &str) {
    let Some(gpu) = create_strict_gpu_renderer() else {
        eprintln!("SKIP parity {label}: no GPU adapter");
        return;
    };
    let a = render_with(cpu().as_ref(), img, st, assets, max);
    let b = render_with(gpu.as_ref(), img, st, assets, max);
    let (mean, mx) = compare(&a, &b);
    println!("parity {label}: mean dE {mean:.4}, max dE {mx:.3}");
    assert!(mean < 1.0, "{label}: mean dE {mean}");
    assert!(mx < 3.0, "{label}: max dE {mx}");
}

#[test]
fn gpu_cpu_parity_patches() {
    let img = scene(640, 480, 21);
    let assets = PatchAssets::default()
        .with("full", synth_patch(320, 240, 1, Some(0.9)))
        .with("f1", synth_patch(200, 200, 2, Some(1.0)))
        .with("f2", synth_patch(96, 128, 3, None))
        .with("big", synth_patch(2000, 1500, 4, None));
    let mut full = patch("full", [0.0, 0.0, 1.0, 1.0]);
    full.kind = PatchKind::Denoise;
    full.amount = 0.7;
    let mut f1 = patch("f1", [0.3, 0.2, 0.3, 0.4]);
    f1.feather = 0.25;
    let mut f2 = patch("f2", [0.45, 0.35, 0.3, 0.4]);
    f2.amount = 0.8;
    f2.feather = 0.1;
    let mut big = patch("big", [0.05, 0.6, 0.35, 0.3]); // downscaled asset
    big.feather = 0.15;
    let patches = vec![
        Op::Patch(full),
        Op::Patch(f1),
        Op::Patch(f2),
        Op::Patch(big),
    ];
    parity(&img, &stack(patches.clone()), &assets, None, "patches");
    let mut ops = vec![Op::Crop(Crop {
        rect: [0.08, 0.06, 0.84, 0.88],
        angle: -5.0,
        aspect: None,
    })];
    ops.extend(patches);
    ops.push(Op::Global(Adjust {
        exposure: 0.2,
        contrast: 15.0,
        clarity: 20.0,
        ..Default::default()
    }));
    parity(
        &img,
        &stack(ops),
        &assets,
        Some(300),
        "patches + crop + downscale",
    );
}

struct Both<'a>(&'a PortraitMasks, &'a PatchAssets);

impl MaskProvider for Both<'_> {
    fn mask(&self, t: MaskTarget, i: Option<i64>) -> Result<Option<Mask>> {
        self.0.mask(t, i)
    }
    fn people(&self) -> Result<Vec<PersonGeometry>> {
        self.0.people()
    }
    fn patch(&self, a: &str) -> Result<Option<RgbaImage>> {
        self.1.patch(a)
    }
}

#[test]
fn patches_follow_the_portrait_warp() {
    // The patch is composited before the warp, so the warp displaces it with the base.
    let p = synth_portrait(480, 640);
    let [cx, cy, rx, ry] = p.face;
    let assets = PatchAssets::default().with("a", synth_patch(100, 120, 8, None));
    let rect = [cx - 1.6 * rx, cy - 1.6 * ry, 3.2 * rx, 3.2 * ry];
    let warp = Op::Warp(Warp::Face {
        person_id: None,
        level: Level::Refined,
        slim: 80.0,
        chin: 40.0,
        eyes: 70.0,
        nose: 50.0,
    });
    let pm = portrait_masks(&p);
    let both = Both(&pm, &assets);
    let pt = Op::Patch(patch("a", rect));
    let plain = render_with(
        cpu().as_ref(),
        &p.image,
        &stack(vec![pt.clone()]),
        &both,
        None,
    );
    let warped = render_with(
        cpu().as_ref(),
        &p.image,
        &stack(vec![pt, warp]),
        &both,
        None,
    );
    assert_ne!(plain.data, warped.data);
}
