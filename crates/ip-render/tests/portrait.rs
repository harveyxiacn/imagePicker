//! Portrait retouching: warp (face liquify, body slimming, background protection) and
//! beauty (skin smoothing, whitening, blemishes, eyes/teeth/dark circles).
#![allow(clippy::chunks_exact_to_as_chunks)]
use std::sync::atomic::{AtomicUsize, Ordering};

use ip_render::testutil::*;
use ip_render::*;

const W: u32 = 480;
const H: u32 = 640;

fn stack(ops: Vec<Op>) -> EditStack {
    EditStack { version: 1, ops }
}

fn cpu() -> Box<dyn Renderer> {
    create_renderer(false)
}

fn render_with(
    r: &dyn Renderer,
    p: &SynthPortrait,
    img: &RgbImage,
    st: &EditStack,
    max: Option<u32>,
) -> RgbImage {
    r.render(&RenderRequest {
        source: img,
        stack: st,
        masks: &portrait_masks(p),
        luts: &NoAssets,
        max_long_edge: max,
    })
    .expect("render")
}

fn render(p: &SynthPortrait, img: &RgbImage, st: &EditStack) -> RgbImage {
    render_with(cpu().as_ref(), p, img, st, None)
}

fn face_warp(level: Level, slim: f32, chin: f32, eyes: f32, nose: f32) -> Op {
    Op::Warp(Warp::Face {
        person_id: None,
        level,
        slim,
        chin,
        eyes,
        nose,
    })
}

fn body_warp(level: Level, arms: f32, legs: f32, waist: f32, lengthen: f32, protect: bool) -> Op {
    Op::Warp(Warp::Body {
        person_id: None,
        level,
        arms,
        legs,
        waist,
        lengthen_legs: lengthen,
        protect_background: protect,
    })
}

fn beauty(f: impl FnOnce(&mut Beauty)) -> Op {
    let mut b = Beauty {
        person_id: None,
        level: Level::Refined,
        smooth: 0.0,
        whiten: 0.0,
        blemish: false,
        eye_brighten: 0.0,
        teeth_whiten: 0.0,
        dark_circles: 0.0,
    };
    f(&mut b);
    Op::Beauty(b)
}

fn px(img: &RgbImage, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * img.width + x) * 3) as usize;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

fn near(a: [u8; 3], b: [f32; 3], tol: f32) -> bool {
    (0..3).all(|k| (a[k] as f32 - b[k]).abs() <= tol)
}

/// Width (px) of the run of pixels near `colour` on row `y`, grown from `x0` both ways.
fn run_width(img: &RgbImage, y: u32, x0: u32, colour: [f32; 3], tol: f32) -> u32 {
    let mut l = x0;
    while l > 0 && near(px(img, l - 1, y), colour, tol) {
        l -= 1;
    }
    let mut r = x0;
    while r + 1 < img.width && near(px(img, r + 1, y), colour, tol) {
        r += 1;
    }
    r - l + 1
}

fn lum(c: [u8; 3]) -> f64 {
    0.2126 * c[0] as f64 + 0.7152 * c[1] as f64 + 0.0722 * c[2] as f64
}

fn mean_lum_rect(img: &RgbImage, r: [u32; 4]) -> f64 {
    let mut s = 0.0;
    for y in r[1]..r[3] {
        for x in r[0]..r[2] {
            s += lum(px(img, x, y));
        }
    }
    s / ((r[2] - r[0]) * (r[3] - r[1])) as f64
}

fn mean_ch_rect(img: &RgbImage, r: [u32; 4], ch: usize) -> f64 {
    let mut s = 0.0;
    for y in r[1]..r[3] {
        for x in r[0]..r[2] {
            s += px(img, x, y)[ch] as f64;
        }
    }
    s / ((r[2] - r[0]) * (r[3] - r[1])) as f64
}

/// Variance of (green - 3x3 box mean) over a rect: a high-frequency energy measure.
fn hf_var(img: &RgbImage, r: [u32; 4]) -> f64 {
    let g = |x: u32, y: u32| px(img, x, y)[1] as f64;
    let (mut s, mut n) = (0.0, 0.0);
    for y in r[1]..r[3] {
        for x in r[0]..r[2] {
            let mut m = 0.0;
            for dy in 0..3 {
                for dx in 0..3 {
                    m += g(x + dx - 1, y + dy - 1);
                }
            }
            let d = g(x, y) - m / 9.0;
            s += d * d;
            n += 1.0;
        }
    }
    s / n
}

fn pix(p: &SynthPortrait, nx: f32, ny: f32) -> (u32, u32) {
    (
        (nx * p.image.width as f32) as u32,
        (ny * p.image.height as f32) as u32,
    )
}

/// Cheek rectangle (pixels) on the image-right cheek, fully inside the skin.
fn cheek_rect(p: &SynthPortrait) -> [u32; 4] {
    let [cx, cy, rx, ry] = p.face;
    let (x0, y0) = pix(p, cx + 0.45 * rx, cy + 0.12 * ry);
    let (x1, y1) = pix(p, cx + 0.75 * rx, cy + 0.40 * ry);
    [x0, y0, x1, y1]
}

fn count_diff(a: &RgbImage, b: &RgbImage) -> usize {
    a.data
        .chunks_exact(3)
        .zip(b.data.chunks_exact(3))
        .filter(|(x, y)| x != y)
        .count()
}

// ---------------------------------------------------------------- identity / skipping

#[test]
fn zero_amounts_missing_geometry_and_unknown_person_are_identity() {
    let p = synth_portrait(W, H);
    let img = &p.image;
    // Zero amounts.
    let zero = stack(vec![
        face_warp(Level::Refined, 0.0, 0.0, 0.0, 0.0),
        body_warp(Level::Refined, 0.0, 0.0, 0.0, 0.0, true),
        beauty(|_| {}),
    ]);
    assert_eq!(max_abs_diff(img, &render(&p, img, &zero)), 0);
    // Active ops but no geometry (NoAssets: people() is empty).
    let active = stack(vec![
        face_warp(Level::Refined, 100.0, 50.0, 100.0, 50.0),
        body_warp(Level::Refined, 100.0, 100.0, 100.0, 100.0, true),
        beauty(|b| {
            b.smooth = 100.0;
            b.whiten = 100.0;
            b.blemish = true;
            b.eye_brighten = 100.0;
        }),
    ]);
    let none = cpu()
        .render(&RenderRequest {
            source: img,
            stack: &active,
            masks: &NoAssets,
            luts: &NoAssets,
            max_long_edge: None,
        })
        .unwrap();
    assert_eq!(max_abs_diff(img, &none), 0);
    // Unknown person id is skipped.
    let mut other = active.clone();
    for op in &mut other.ops {
        match op {
            Op::Warp(Warp::Face { person_id, .. }) | Op::Warp(Warp::Body { person_id, .. }) => {
                *person_id = Some(999)
            }
            Op::Beauty(b) => b.person_id = Some(999),
            _ => {}
        }
    }
    assert_eq!(max_abs_diff(img, &render(&p, img, &other)), 0);
    // Geometry with no landmarks / pose / masks does nothing and does not fail.
    let mut bare = p.person.clone();
    bare.face_landmarks.clear();
    bare.pose.clear();
    bare.skin = None;
    bare.body = None;
    let out = cpu()
        .render(&RenderRequest {
            source: img,
            stack: &active,
            masks: &PortraitMasks(vec![bare]),
            luts: &NoAssets,
            max_long_edge: None,
        })
        .unwrap();
    assert_eq!(max_abs_diff(img, &out), 0);
}

struct Counting {
    inner: PortraitMasks,
    calls: AtomicUsize,
}

impl MaskProvider for Counting {
    fn mask(&self, t: MaskTarget, id: Option<i64>) -> Result<Option<Mask>> {
        self.inner.mask(t, id)
    }
    fn people(&self) -> Result<Vec<PersonGeometry>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.people()
    }
}

#[test]
fn people_are_fetched_once_and_only_for_portrait_ops() {
    let p = synth_portrait(160, 214);
    let counting = Counting {
        inner: portrait_masks(&p),
        calls: AtomicUsize::new(0),
    };
    let run = |st: &EditStack| {
        cpu()
            .render(&RenderRequest {
                source: &p.image,
                stack: st,
                masks: &counting,
                luts: &NoAssets,
                max_long_edge: None,
            })
            .unwrap();
    };
    run(&stack(vec![Op::Global(Adjust {
        exposure: 0.2,
        ..Default::default()
    })]));
    assert_eq!(counting.calls.load(Ordering::SeqCst), 0);
    run(&stack(vec![
        face_warp(Level::Standard, 50.0, 0.0, 0.0, 0.0),
        body_warp(Level::Standard, 50.0, 0.0, 0.0, 0.0, true),
        beauty(|b| b.smooth = 40.0),
        beauty(|b| b.whiten = 30.0),
    ]));
    assert_eq!(counting.calls.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------- face warp

fn cheek_row(p: &SynthPortrait) -> u32 {
    // Row of the jaw-angle landmark 58 / 288.
    (p.person.face_landmarks[58][1] * p.image.height as f32) as u32
}

fn face_width(p: &SynthPortrait, img: &RgbImage) -> u32 {
    let skin = [
        p.skin_rgb[0] as f32,
        p.skin_rgb[1] as f32,
        p.skin_rgb[2] as f32,
    ];
    let x0 = ((p.face[0] + 0.6 * p.face[2]) * img.width as f32) as u32;
    run_width(img, cheek_row(p), x0, skin, 30.0)
}

fn face_px_width(p: &SynthPortrait) -> f32 {
    let lm = &p.person.face_landmarks;
    (lm[454][0] - lm[234][0]).abs() * p.image.width as f32
}

#[test]
fn slim_reduces_face_width_within_the_cap_and_negative_inverts() {
    let p = synth_portrait(W, H);
    let w0 = face_width(&p, &p.image);
    let fw = face_px_width(&p);
    let mut widths = vec![];
    for level in [Level::Natural, Level::Standard, Level::Refined] {
        let out = render(
            &p,
            &p.image,
            &stack(vec![face_warp(level, 100.0, 0.0, 0.0, 0.0)]),
        );
        let w1 = face_width(&p, &out);
        let dec = w0 as f32 - w1 as f32;
        // One jaw side is measured: it moves by at most cap * face width.
        let max_dec = level.face_warp_cap() * fw + 1.5;
        println!("{level:?}: width {w0} -> {w1} (cap decrease {max_dec:.1})");
        assert!(dec >= 1.0, "{level:?}: face did not slim ({w0} -> {w1})");
        assert!(dec <= max_dec, "{level:?}: slimmed {dec} > cap {max_dec}");
        widths.push(w1);
    }
    assert!(
        widths[0] >= widths[1] && widths[1] >= widths[2],
        "{widths:?}"
    );
    // Half amount slims less than full amount; negative widens.
    let half = render(
        &p,
        &p.image,
        &stack(vec![face_warp(Level::Refined, 50.0, 0.0, 0.0, 0.0)]),
    );
    let wh = face_width(&p, &half);
    assert!(wh >= widths[2] && wh <= w0, "half {wh}");
    let wide = render(
        &p,
        &p.image,
        &stack(vec![face_warp(Level::Refined, -100.0, 0.0, 0.0, 0.0)]),
    );
    assert!(face_width(&p, &wide) > w0, "negative slim must widen");
}

#[test]
fn face_warp_is_exactly_zero_outside_its_support() {
    let p = synth_portrait(W, H);
    let out = render(
        &p,
        &p.image,
        &stack(vec![face_warp(Level::Refined, 100.0, 100.0, 100.0, 100.0)]),
    );
    assert!(count_diff(&p.image, &out) > 100, "warp changed nothing");
    // Everything below the neck (rows beyond the chin + margin) and the top rows are untouched.
    let chin_y = (p.person.face_landmarks[152][1] * H as f32) as u32;
    let top_y = (p.person.face_landmarks[10][1] * H as f32) as u32;
    let fw = face_px_width(&p) as u32;
    for y in (0..top_y.saturating_sub(fw / 2)).chain(chin_y + fw / 2..H) {
        for x in 0..W {
            assert_eq!(px(&p.image, x, y), px(&out, x, y), "pixel {x},{y} moved");
        }
    }
}

#[test]
fn eyes_enlarge_the_eye_region() {
    let p = synth_portrait(W, H);
    let white = |img: &RgbImage| -> usize {
        let [cx, cy, rx, ry] = p.face;
        let (x0, y0) = pix(&p, cx - rx, cy - 0.45 * ry);
        let (x1, y1) = pix(&p, cx + rx, cy + 0.25 * ry);
        let mut n = 0;
        for y in y0..y1 {
            for x in x0..x1 {
                let c = px(img, x, y);
                if c.iter().all(|v| *v > 200) {
                    n += 1;
                }
            }
        }
        n
    };
    let n0 = white(&p.image);
    let big = render(
        &p,
        &p.image,
        &stack(vec![face_warp(Level::Refined, 0.0, 0.0, 100.0, 0.0)]),
    );
    let small = render(
        &p,
        &p.image,
        &stack(vec![face_warp(Level::Refined, 0.0, 0.0, -100.0, 0.0)]),
    );
    let (nb, ns) = (white(&big), white(&small));
    println!("eye-white pixels: {ns} (shrunk) < {n0} < {nb} (enlarged)");
    assert!(nb as f32 > n0 as f32 * 1.04, "{n0} -> {nb}");
    assert!((ns as f32) < n0 as f32 * 0.97, "{n0} -> {ns}");
    // Natural caps below Refined.
    let nat = render(
        &p,
        &p.image,
        &stack(vec![face_warp(Level::Natural, 0.0, 0.0, 100.0, 0.0)]),
    );
    assert!(white(&nat) <= nb && white(&nat) as f32 > n0 as f32 * 1.005);
}

#[test]
fn chin_and_nose_move_pixels_locally() {
    let p = synth_portrait(W, H);
    for (chin, nose) in [(100.0, 0.0), (-100.0, 0.0), (0.0, 100.0)] {
        let out = render(
            &p,
            &p.image,
            &stack(vec![face_warp(Level::Refined, 0.0, chin, 0.0, nose)]),
        );
        let n = count_diff(&p.image, &out);
        assert!(n > 20, "chin {chin} nose {nose}: {n} px changed");
        assert!(n < (W * H / 8) as usize, "too many pixels changed: {n}");
    }
}

#[test]
fn warp_maps_through_crop_and_straighten() {
    let p = synth_portrait(W, H);
    let st = |warp: bool| {
        let mut ops = vec![Op::Crop(Crop {
            rect: [0.1, 0.05, 0.8, 0.85],
            angle: 6.0,
            aspect: None,
        })];
        if warp {
            ops.push(face_warp(Level::Refined, 100.0, 0.0, 100.0, 0.0));
        }
        stack(ops)
    };
    let a = render(&p, &p.image, &st(false));
    let b = render(&p, &p.image, &st(true));
    // Centroid of changed pixels must sit on the face as seen through the crop.
    let (mut sx, mut sy, mut n) = (0.0f64, 0.0f64, 0.0f64);
    for y in 0..a.height {
        for x in 0..a.width {
            if px(&a, x, y) != px(&b, x, y) {
                sx += x as f64;
                sy += y as f64;
                n += 1.0;
            }
        }
    }
    assert!(n > 50.0);
    let (cx, cy) = (sx / n / a.width as f64, sy / n / a.height as f64);
    // Expected face position: invert the crop (rotate about the image centre).
    let (fw, fh) = (W as f64, H as f64);
    let (fcx, fcy) = (p.face[0] as f64 * fw, p.face[1] as f64 * fh);
    let th = 6.0f64.to_radians();
    let (dx, dy) = (fcx - fw / 2.0, fcy - fh / 2.0);
    let (rx, ry) = (
        fw / 2.0 + dx * th.cos() + dy * th.sin(),
        fh / 2.0 - dx * th.sin() + dy * th.cos(),
    );
    let (ex, ey) = ((rx - 0.1 * fw) / (0.8 * fw), (ry - 0.05 * fh) / (0.85 * fh));
    println!("changed centroid ({cx:.3}, {cy:.3}), expected ({ex:.3}, {ey:.3})");
    assert!((cx - ex).abs() < 0.06 && (cy - ey).abs() < 0.06);
}

// ---------------------------------------------------------------- body warp

fn arm_row(p: &SynthPortrait) -> u32 {
    (0.61 * p.image.height as f32) as u32
}

/// Width of the right-arm (image-left) skin run at row `y`, grown from its axis.
fn arm_width(p: &SynthPortrait, img: &RgbImage) -> u32 {
    let y = arm_row(p);
    let (e, w) = (p.person.pose[14], p.person.pose[16]);
    let t = ((y as f32 / img.height as f32) - e[1]) / (w[1] - e[1]);
    let x = (e[0] + (w[0] - e[0]) * t) * img.width as f32;
    run_width(img, y, x as u32, [214.0, 164.0, 140.0], 14.0)
}

#[test]
fn arms_and_waist_contract_within_caps_and_legs_lengthen() {
    let p = synth_portrait(W, H);
    let a0 = arm_width(&p, &p.image);
    let arm_w = 0.06 * W as f32;
    let mut last = a0;
    for level in [Level::Natural, Level::Standard, Level::Refined] {
        let out = render(
            &p,
            &p.image,
            &stack(vec![body_warp(level, 100.0, 0.0, 0.0, 0.0, true)]),
        );
        let a1 = arm_width(&p, &out);
        let dec = a0 as f32 - a1 as f32;
        let max_dec = 2.0 * level.body_warp_cap() * arm_w + 2.5;
        println!("{level:?}: arm {a0} -> {a1} (max decrease {max_dec:.1})");
        assert!(dec >= 1.0, "{level:?}: arm did not slim");
        assert!(dec <= max_dec, "{level:?}: {dec} > {max_dec}");
        assert!(a1 <= last, "levels must be monotone");
        last = a1;
    }
    // Waist: torso width at the waist row.
    let wy = (0.523 * H as f32) as u32;
    let torso = [60.0, 90.0, 140.0];
    let t0 = run_width(&p.image, wy, W / 2, torso, 14.0);
    let out = render(
        &p,
        &p.image,
        &stack(vec![body_warp(Level::Refined, 0.0, 0.0, 100.0, 0.0, true)]),
    );
    let t1 = run_width(&out, wy, W / 2, torso, 14.0);
    println!("waist {t0} -> {t1}");
    assert!(t1 + 2 <= t0, "waist did not narrow ({t0} -> {t1})");
    assert!((t0 - t1) as f32 <= 2.0 * 0.15 * t0 as f32 + 2.0);
    // Legs contract.
    let ly = (0.70 * H as f32) as u32;
    let leg = [50.0, 52.0, 62.0];
    let lx = (0.435 * W as f32) as u32;
    let l0 = run_width(&p.image, ly, lx, leg, 14.0);
    let out = render(
        &p,
        &p.image,
        &stack(vec![body_warp(Level::Refined, 0.0, 100.0, 0.0, 0.0, true)]),
    );
    let l1 = run_width(&out, ly, lx, leg, 14.0);
    assert!(l1 < l0, "legs did not contract ({l0} -> {l1})");
    // Lengthen legs: the lowest dark leg pixel moves down.
    let lowest = |img: &RgbImage| -> u32 {
        let x = (0.42 * W as f32) as u32;
        (0..H)
            .rev()
            .find(|&y| near(px(img, x, y), leg, 14.0))
            .unwrap()
    };
    let out = render(
        &p,
        &p.image,
        &stack(vec![body_warp(Level::Refined, 0.0, 0.0, 0.0, 100.0, true)]),
    );
    let (f0, f1) = (lowest(&p.image), lowest(&out));
    println!("feet {f0} -> {f1}");
    assert!(f1 >= f0 + 3, "legs did not lengthen ({f0} -> {f1})");
    let max_shift = 0.06 * (0.335 * H as f32) + 3.0;
    assert!((f1 - f0) as f32 <= max_shift);
    // The head is untouched.
    for y in 0..(0.3 * H as f32) as u32 {
        for x in 0..W {
            assert_eq!(px(&p.image, x, y), px(&out, x, y));
        }
    }
}

#[test]
fn low_visibility_limbs_are_skipped() {
    let mut p = synth_portrait(W, H);
    for i in [13, 14, 15, 16, 25, 26, 27, 28] {
        p.person.pose[i][2] = 0.2;
    }
    let out = render(
        &p,
        &p.image,
        &stack(vec![body_warp(
            Level::Refined,
            100.0,
            100.0,
            0.0,
            100.0,
            true,
        )]),
    );
    assert_eq!(max_abs_diff(&p.image, &out), 0);
}

/// Image with a dark vertical line beside the right arm; returns (image, x of the line).
fn with_line(p: &SynthPortrait) -> (RgbImage, f32) {
    let mut img = p.image.clone();
    let lx = 0.195f32;
    let x0 = (lx * W as f32) as u32;
    for y in (0.40 * H as f32) as u32..(0.92 * H as f32) as u32 {
        for x in x0 - 1..=x0 + 1 {
            let i = ((y * W + x) * 3) as usize;
            img.data[i..i + 3].copy_from_slice(&[25, 25, 25]);
        }
    }
    (img, lx)
}

fn line_bend(img: &RgbImage, lx: f32) -> f32 {
    let (x0, x1) = ((lx * W as f32) as i32 - 14, (lx * W as f32) as i32 + 14);
    let mut cs = vec![];
    for y in (0.46 * H as f32) as u32..(0.84 * H as f32) as u32 {
        let (mut s, mut m) = (0.0f64, 0.0f64);
        for x in x0..x1 {
            let l = lum(px(img, x as u32, y));
            let w = (70.0 - l).max(0.0);
            s += w * x as f64;
            m += w;
        }
        if m > 0.0 {
            cs.push((s / m) as f32);
        }
    }
    let lo = cs.iter().cloned().fold(f32::MAX, f32::min);
    let hi = cs.iter().cloned().fold(f32::MIN, f32::max);
    hi - lo
}

#[test]
fn protect_background_reduces_line_bending() {
    let p = synth_portrait(W, H);
    let (img, lx) = with_line(&p);
    let base = line_bend(&img, lx);
    let off = render(
        &p,
        &img,
        &stack(vec![body_warp(Level::Refined, 100.0, 0.0, 0.0, 0.0, false)]),
    );
    let on = render(
        &p,
        &img,
        &stack(vec![body_warp(Level::Refined, 100.0, 0.0, 0.0, 0.0, true)]),
    );
    let (b_off, b_on) = (line_bend(&off, lx), line_bend(&on, lx));
    println!("line bend: original {base:.2}px, unprotected {b_off:.2}px, protected {b_on:.2}px");
    assert!(
        b_off > base + 0.7,
        "the slimmed arm must bend the line ({b_off} vs {base})"
    );
    assert!(
        b_on < b_off * 0.7,
        "protection must reduce bending ({b_on} vs {b_off})"
    );
    // The arm itself still slims with protection on.
    let (a0, a1) = (arm_width(&p, &img), arm_width(&p, &on));
    assert!(a1 < a0, "arm must still slim ({a0} -> {a1})");
}

// ---------------------------------------------------------------- beauty

#[test]
fn smooth_reduces_skin_texture_but_not_outside() {
    let p = synth_portrait(W, H);
    let r = cheek_rect(&p);
    let v0 = hf_var(&p.image, r);
    let out = render(&p, &p.image, &stack(vec![beauty(|b| b.smooth = 100.0)]));
    let v1 = hf_var(&out, r);
    println!("skin high-frequency variance {v0:.3} -> {v1:.3}");
    assert!(
        v1 < v0 * 0.8,
        "smoothing must reduce texture ({v0} -> {v1})"
    );
    assert!(v1 > v0 * 0.1, "skin must not turn plastic ({v0} -> {v1})");
    // Natural caps below Refined.
    let nat = render(
        &p,
        &p.image,
        &stack(vec![Op::Beauty(Beauty {
            person_id: None,
            level: Level::Natural,
            smooth: 100.0,
            whiten: 0.0,
            blemish: false,
            eye_brighten: 0.0,
            teeth_whiten: 0.0,
            dark_circles: 0.0,
        })]),
    );
    let vn = hf_var(&nat, r);
    assert!(
        vn > v1 && vn < v0,
        "levels must be ordered: {v1} < {vn} < {v0}"
    );
    // Outside the skin (torso, legs, background) nothing changes at all.
    let [cx, cy, _, ry] = p.face;
    let (_, y_below) = pix(&p, cx, cy + 3.0 * ry);
    let skin = p.person.skin.as_ref().unwrap();
    for y in 0..H {
        for x in 0..W {
            // Far from any skin pixel: clearly outside the (dilated) skin mask.
            let (mx, my) = (x * skin.width / W, y * skin.height / H);
            let mut near_skin = false;
            for dy in -40i32..=40 {
                for dx in -40i32..=40 {
                    if dx % 8 != 0 || dy % 8 != 0 {
                        continue;
                    }
                    let (qx, qy) = (mx as i32 + dx, my as i32 + dy);
                    if qx >= 0
                        && qy >= 0
                        && (qx as u32) < skin.width
                        && (qy as u32) < skin.height
                        && skin.data[qy as usize * skin.width as usize + qx as usize] > 0
                    {
                        near_skin = true;
                    }
                }
            }
            if !near_skin {
                assert_eq!(
                    px(&p.image, x, y),
                    px(&out, x, y),
                    "{x},{y} changed outside skin"
                );
            }
        }
    }
    let _ = y_below;
}

#[test]
fn whiten_raises_lightness_inside_skin_only() {
    let p = synth_portrait(W, H);
    let r = cheek_rect(&p);
    let l0 = mean_lum_rect(&p.image, r);
    let out = render(&p, &p.image, &stack(vec![beauty(|b| b.whiten = 100.0)]));
    let l1 = mean_lum_rect(&out, r);
    println!("cheek luminance {l0:.1} -> {l1:.1}");
    assert!(l1 > l0 + 3.0, "{l0} -> {l1}");
    let nat = render(
        &p,
        &p.image,
        &stack(vec![Op::Beauty(Beauty {
            person_id: None,
            level: Level::Natural,
            smooth: 0.0,
            whiten: 100.0,
            blemish: false,
            eye_brighten: 0.0,
            teeth_whiten: 0.0,
            dark_circles: 0.0,
        })]),
    );
    let ln = mean_lum_rect(&nat, r);
    assert!(
        ln > l0 + 1.0 && ln < l1,
        "levels ordered: {l0} < {ln} < {l1}"
    );
    // Torso and trousers are not skin.
    for (x0, y0, x1, y1) in [
        (0.42, 0.40, 0.58, 0.60),
        (0.40, 0.70, 0.60, 0.85),
        (0.02, 0.02, 0.2, 0.2),
    ] {
        let (a, b) = pix(&p, x0, y0);
        let (c, d) = pix(&p, x1, y1);
        for y in b..d {
            for x in a..c {
                assert_eq!(px(&p.image, x, y), px(&out, x, y));
            }
        }
    }
}

#[test]
fn blemish_removes_a_dark_dot() {
    let mut p = synth_portrait(W, H);
    let [cx, cy, rx, ry] = p.face;
    let (bx, by) = (cx + 0.55 * rx, cy + 0.25 * ry);
    let (dx, dy) = pix(&p, bx, by);
    let mut img = p.image.clone();
    let rad = 4i32;
    for y in -rad..=rad {
        for x in -rad..=rad {
            if x * x + y * y <= rad * rad {
                let i = (((dy as i32 + y) as u32 * W + (dx as i32 + x) as u32) * 3) as usize;
                img.data[i..i + 3].copy_from_slice(&[120, 70, 60]);
            }
        }
    }
    p.person.blemishes = vec![[bx, by, rad as f32 / H as f32]];
    let dot = |im: &RgbImage| mean_lum_rect(im, [dx - 2, dy - 2, dx + 3, dy + 3]);
    let ring = |im: &RgbImage| mean_lum_rect(im, [dx - 14, dy - 14, dx + 15, dy + 15]);
    let on = render(&p, &img, &stack(vec![beauty(|b| b.blemish = true)]));
    let off = render(&p, &img, &stack(vec![beauty(|b| b.smooth = 1.0)]));
    let surround = ring(&p.image);
    let (d0, d1) = (dot(&img), dot(&on));
    println!(
        "dot luminance {d0:.1} -> {d1:.1}, skin around {surround:.1}, unflagged {:.1}",
        dot(&off)
    );
    assert!(d0 < surround - 40.0);
    assert!(
        (d1 - surround).abs() < (d0 - surround).abs() * 0.25,
        "dot not removed"
    );
    assert!(
        dot(&off) < surround - 30.0,
        "without the flag the dot stays"
    );
}

#[test]
fn eyes_teeth_and_dark_circles_act_on_their_regions() {
    let p = synth_portrait(W, H);
    let lm = &p.person.face_landmarks;
    let at = |i: usize| ((lm[i][0] * W as f32) as u32, (lm[i][1] * H as f32) as u32);
    // Iris of the image-left eye.
    let (ix, iy) = at(468);
    let iris = [ix - 2, iy - 2, ix + 3, iy + 3];
    let out = render(
        &p,
        &p.image,
        &stack(vec![beauty(|b| b.eye_brighten = 100.0)]),
    );
    let (i0, i1) = (mean_lum_rect(&p.image, iris), mean_lum_rect(&out, iris));
    println!("iris {i0:.1} -> {i1:.1}");
    assert!(i1 > i0 + 2.0);
    // Skin away from the eyes is untouched by eye brightening.
    let c = cheek_rect(&p);
    assert_eq!(
        mean_lum_rect(&p.image, c),
        mean_lum_rect(&out, c),
        "eye_brighten must stay on the eyes"
    );
    // Teeth: less yellow (blue up) and brighter.
    let (mx, my) = at(13); // upper inner lip centre
    let (_, my2) = at(14);
    let teeth = [mx - 6, my + 1, mx + 7, (my + my2) / 2];
    let out = render(
        &p,
        &p.image,
        &stack(vec![beauty(|b| b.teeth_whiten = 100.0)]),
    );
    let (b0, b1) = (
        mean_ch_rect(&p.image, teeth, 2),
        mean_ch_rect(&out, teeth, 2),
    );
    println!("teeth blue {b0:.1} -> {b1:.1}");
    assert!(b1 > b0 + 2.0);
    assert_eq!(mean_lum_rect(&p.image, c), mean_lum_rect(&out, c));
    // Dark circles: skin just below the lower lid gets lighter.
    let (lx, ly) = at(145); // lower lid centre
    let under = [lx - 5, ly + 5, lx + 6, ly + 9];
    let out = render(
        &p,
        &p.image,
        &stack(vec![beauty(|b| b.dark_circles = 100.0)]),
    );
    let (u0, u1) = (mean_lum_rect(&p.image, under), mean_lum_rect(&out, under));
    println!("under-eye {u0:.1} -> {u1:.1}");
    assert!(u1 > u0 + 1.5);
}

#[test]
fn beauty_without_a_skin_matte_uses_the_face_oval() {
    let mut p = synth_portrait(W, H);
    p.person.skin = None;
    let r = cheek_rect(&p);
    let v0 = hf_var(&p.image, r);
    let out = render(&p, &p.image, &stack(vec![beauty(|b| b.smooth = 100.0)]));
    assert!(hf_var(&out, r) < v0 * 0.85);
}

#[test]
fn beauty_follows_the_warp() {
    // Slim + whiten: the skin mask must follow the moved face, so the (moved) cheek edge
    // pixels are brightened too and pixels outside the new face stay background.
    let p = synth_portrait(W, H);
    let st = stack(vec![
        face_warp(Level::Refined, 100.0, 0.0, 0.0, 0.0),
        beauty(|b| b.whiten = 100.0),
    ]);
    let warped = render(
        &p,
        &p.image,
        &stack(vec![face_warp(Level::Refined, 100.0, 0.0, 0.0, 0.0)]),
    );
    let both = render(&p, &p.image, &st);
    let y = cheek_row(&p);
    let skin = [
        p.skin_rgb[0] as f32,
        p.skin_rgb[1] as f32,
        p.skin_rgb[2] as f32,
    ];
    let x0 = ((p.face[0] + 0.6 * p.face[2]) * W as f32) as u32;
    let w_warp = run_width(&warped, y, x0, skin, 30.0);
    // Whitening lifts skin colour by only a few levels, so the run stays about as wide.
    let w_both = run_width(&both, y, x0, skin, 40.0);
    assert!(
        (w_both as i32 - w_warp as i32).abs() <= 3,
        "{w_warp} vs {w_both}"
    );
    // The background right outside the new jaw is not whitened.
    // (the run grew in both directions from x0; find its right end)
    let mut x_out = x0;
    while near(px(&warped, x_out, y), skin, 30.0) {
        x_out += 1;
    }
    x_out += 16; // beyond the guided-filter radius
    let (o, b) = (px(&warped, x_out, y), px(&both, x_out, y));
    assert!((0..3).all(|k| o[k].abs_diff(b[k]) <= 2), "{o:?} vs {b:?}");
    // ... while skin just inside the moved jaw edge is whitened.
    let x_in = x_out - 16 - 8;
    assert!(lum(px(&both, x_in, y)) > lum(px(&warped, x_in, y)) + 8.0);
}

// ---------------------------------------------------------------- CPU / GPU parity

fn parity(p: &SynthPortrait, img: &RgbImage, st: &EditStack, max: Option<u32>, label: &str) {
    let Some(gpu) = create_strict_gpu_renderer() else {
        eprintln!("SKIP parity {label}: no GPU adapter");
        return;
    };
    let a = render_with(cpu().as_ref(), p, img, st, max);
    let b = render_with(gpu.as_ref(), p, img, st, max);
    let (mean, mx) = compare(&a, &b);
    println!("parity {label}: mean dE {mean:.4}, max dE {mx:.3}");
    assert!(mean < 1.0, "{label}: mean dE {mean}");
    assert!(mx < 3.0, "{label}: max dE {mx}");
}

#[test]
fn gpu_cpu_parity_portrait_ops() {
    let mut p = synth_portrait(W, H);
    let [cx, cy, rx, ry] = p.face;
    p.person.blemishes = vec![[cx + 0.5 * rx, cy + 0.3 * ry, 3.0 / H as f32]];
    let mut img = p.image.clone();
    for k in 0..3usize {
        let (x, y) = pix(&p, cx + 0.5 * rx, cy + 0.3 * ry);
        let i = ((y * W + x) * 3) as usize + k;
        img.data[i] = 90;
    }
    let face = face_warp(Level::Refined, 80.0, 40.0, 70.0, 50.0);
    let body = body_warp(Level::Refined, 80.0, 60.0, 70.0, 60.0, true);
    let all_beauty = beauty(|b| {
        b.smooth = 80.0;
        b.whiten = 60.0;
        b.blemish = true;
        b.eye_brighten = 70.0;
        b.teeth_whiten = 70.0;
        b.dark_circles = 60.0;
    });
    parity(&p, &img, &stack(vec![face.clone()]), None, "face warp");
    parity(&p, &img, &stack(vec![body.clone()]), None, "body warp");
    parity(&p, &img, &stack(vec![all_beauty.clone()]), None, "beauty");
    let global = Op::Global(Adjust {
        exposure: 0.2,
        clarity: 25.0,
        shadows: 20.0,
        ..Default::default()
    });
    parity(
        &p,
        &img,
        &stack(vec![
            global.clone(),
            face.clone(),
            body.clone(),
            all_beauty.clone(),
        ]),
        None,
        "everything",
    );
    // Through crop + straighten + downscale.
    let crop = Op::Crop(Crop {
        rect: [0.08, 0.04, 0.84, 0.9],
        angle: -4.0,
        aspect: None,
    });
    parity(
        &p,
        &img,
        &stack(vec![crop.clone(), face, body, global, all_beauty]),
        Some(300),
        "crop + downscale",
    );
}
