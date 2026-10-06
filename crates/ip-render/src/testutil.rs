//! Synthetic test images and comparison helpers (tests, benchmarks).

use std::collections::BTreeMap;

use crate::*;

/// Tiny deterministic PRNG (xorshift64*).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in 0..1.
    pub fn f(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    /// Uniform in lo..hi.
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f()
    }
}

pub fn solid(w: u32, h: u32, rgb: [u8; 3]) -> RgbImage {
    let mut data = Vec::with_capacity((w * h * 3) as usize);
    for _ in 0..w * h {
        data.extend_from_slice(&rgb);
    }
    RgbImage {
        width: w,
        height: h,
        data,
    }
}

/// Horizontal grey ramp (x) with a coloured vertical ramp (y).
pub fn gradient(w: u32, h: u32) -> RgbImage {
    let mut data = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let t = x as f32 / (w - 1).max(1) as f32;
            let s = y as f32 / (h - 1).max(1) as f32;
            let g = (t * 255.0).round() as u8;
            data.push(((1.0 - s) * g as f32 + s * (255.0 - g as f32) * 0.5) as u8);
            data.push(g);
            data.push(((1.0 - s) * g as f32 + s * 128.0) as u8);
        }
    }
    RgbImage {
        width: w,
        height: h,
        data,
    }
}

/// 6x4 Macbeth-like chart (sRGB values of the classic ColorChecker patches).
pub fn macbeth(w: u32, h: u32) -> RgbImage {
    const P: [[u8; 3]; 24] = [
        [115, 82, 68],
        [194, 150, 130],
        [98, 122, 157],
        [87, 108, 67],
        [133, 128, 177],
        [103, 189, 170],
        [214, 126, 44],
        [80, 91, 166],
        [193, 90, 99],
        [94, 60, 108],
        [157, 188, 64],
        [224, 163, 46],
        [56, 61, 150],
        [70, 148, 73],
        [175, 54, 60],
        [231, 199, 31],
        [187, 86, 149],
        [8, 133, 161],
        [243, 243, 242],
        [200, 200, 200],
        [160, 160, 160],
        [122, 122, 121],
        [85, 85, 85],
        [52, 52, 52],
    ];
    let mut data = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let cx = (x * 6 / w).min(5);
            let cy = (y * 4 / h).min(3);
            data.extend_from_slice(&P[(cy * 6 + cx) as usize]);
        }
    }
    RgbImage {
        width: w,
        height: h,
        data,
    }
}

/// A skin-tone patch with mild luminance variation.
pub fn skin_patch(w: u32, h: u32) -> RgbImage {
    let mut data = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let v =
                1.0 + 0.12 * ((x as f32 / w as f32) - 0.5) + 0.08 * ((y as f32 / h as f32) - 0.5);
            data.push((224.0 * v).clamp(0.0, 255.0) as u8);
            data.push((172.0 * v).clamp(0.0, 255.0) as u8);
            data.push((148.0 * v).clamp(0.0, 255.0) as u8);
        }
    }
    RgbImage {
        width: w,
        height: h,
        data,
    }
}

pub fn noise(w: u32, h: u32, seed: u64) -> RgbImage {
    let mut r = Rng::new(seed);
    let data = (0..w * h * 3).map(|_| (r.next_u64() >> 56) as u8).collect();
    RgbImage {
        width: w,
        height: h,
        data,
    }
}

/// A natural-looking synthetic scene: sky gradient, ground, soft blobs, a skin patch and grain.
pub fn scene(w: u32, h: u32, seed: u64) -> RgbImage {
    let mut r = Rng::new(seed);
    let blobs: Vec<(f32, f32, f32, [f32; 3])> = (0..14)
        .map(|_| {
            (
                r.f(),
                r.f(),
                r.range(0.05, 0.22),
                [r.range(0.1, 1.0), r.range(0.1, 1.0), r.range(0.1, 1.0)],
            )
        })
        .collect();
    let mut data = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        let v = y as f32 / h as f32;
        for x in 0..w {
            let u = x as f32 / w as f32;
            let mut c = if v < 0.5 {
                let t = v / 0.5;
                [0.35 + 0.35 * t, 0.55 + 0.25 * t, 0.9 - 0.1 * t]
            } else {
                let t = (v - 0.5) / 0.5;
                [0.25 - 0.1 * t, 0.4 - 0.2 * t, 0.15 + 0.05 * t]
            };
            for (bx, by, rad, col) in &blobs {
                let d = ((u - bx).powi(2) + (v - by).powi(2)).sqrt() / rad;
                let k = (1.0 - d).clamp(0.0, 1.0);
                let k = k * k * (3.0 - 2.0 * k) * 0.8;
                for i in 0..3 {
                    c[i] = c[i] * (1.0 - k) + col[i] * k;
                }
            }
            let n = (r.f() - 0.5) * 0.03;
            for ch in c {
                data.push(((ch + n).clamp(0.0, 1.0) * 255.0).round() as u8);
            }
        }
    }
    RgbImage {
        width: w,
        height: h,
        data,
    }
}

/// Per-channel multiplicative cast applied to an image.
pub fn tint(img: &RgbImage, m: [f32; 3]) -> RgbImage {
    let mut o = img.clone();
    for p in o.data.chunks_exact_mut(3) {
        for k in 0..3 {
            p[k] = (p[k] as f32 * m[k]).clamp(0.0, 255.0) as u8;
        }
    }
    o
}

pub fn mean_rgb(img: &RgbImage) -> [f64; 3] {
    let mut s = [0.0f64; 3];
    for p in img.data.chunks_exact(3) {
        for k in 0..3 {
            s[k] += p[k] as f64;
        }
    }
    let n = (img.width * img.height) as f64;
    [s[0] / n, s[1] / n, s[2] / n]
}

pub fn delta_e2000(a: [u8; 3], b: [u8; 3]) -> f64 {
    crate::color::delta_e2000(a, b)
}

/// (mean, max) CIEDE2000 between two equally sized images.
pub fn compare(a: &RgbImage, b: &RgbImage) -> (f64, f64) {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let (mut sum, mut max) = (0.0f64, 0.0f64);
    let n = (a.width * a.height) as usize;
    for i in 0..n {
        let pa = [a.data[i * 3], a.data[i * 3 + 1], a.data[i * 3 + 2]];
        let pb = [b.data[i * 3], b.data[i * 3 + 1], b.data[i * 3 + 2]];
        let d = if pa == pb { 0.0 } else { delta_e2000(pa, pb) };
        sum += d;
        max = max.max(d);
    }
    (sum / n as f64, max)
}

/// Largest per-channel code-value difference.
pub fn max_abs_diff(a: &RgbImage, b: &RgbImage) -> u8 {
    a.data
        .iter()
        .zip(&b.data)
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

/// A randomised but plausible `Adjust` (the "kitchen sink" for parity tests).
pub fn random_adjust(r: &mut Rng, strength: f32) -> Adjust {
    let mut s = |range: f32| r.range(-range, range) * strength;
    let mut a = Adjust {
        exposure: s(1.5),
        contrast: s(60.0),
        highlights: s(70.0),
        shadows: s(70.0),
        whites: s(50.0),
        blacks: s(50.0),
        temp: s(1500.0),
        tint: s(40.0),
        vibrance: s(60.0),
        saturation: s(50.0),
        clarity: s(60.0),
        dehaze: s(50.0),
        ..Default::default()
    };
    if r.f() < 0.5 {
        let x1 = r.range(0.15, 0.4);
        let x2 = r.range(0.6, 0.85);
        a.curve = Some(Curves {
            rgb: vec![
                [0.0, r.range(0.0, 0.06)],
                [x1, x1 + r.range(-0.08, 0.08)],
                [x2, x2 + r.range(-0.08, 0.08)],
                [1.0, r.range(0.94, 1.0)],
            ],
            r: if r.f() < 0.4 {
                vec![[0.0, 0.0], [0.5, r.range(0.45, 0.58)], [1.0, 1.0]]
            } else {
                vec![]
            },
            ..Default::default()
        });
    }
    if r.f() < 0.6 {
        let mut m = BTreeMap::new();
        for b in [
            HslBand::Red,
            HslBand::Orange,
            HslBand::Yellow,
            HslBand::Green,
            HslBand::Aqua,
            HslBand::Blue,
            HslBand::Purple,
            HslBand::Magenta,
        ] {
            if r.f() < 0.5 {
                m.insert(
                    b,
                    Hsl {
                        h: r.range(-50.0, 50.0),
                        s: r.range(-60.0, 60.0),
                        l: r.range(-40.0, 40.0),
                    },
                );
            }
        }
        a.hsl = m;
    }
    if r.f() < 0.5 {
        a.grading = Some(Grading {
            shadows: [r.range(0.0, 360.0), r.range(0.0, 0.5)],
            midtones: [r.range(0.0, 360.0), r.range(0.0, 0.3)],
            highlights: [r.range(0.0, 360.0), r.range(0.0, 0.5)],
            balance: r.range(-60.0, 60.0),
        });
    }
    a
}

/// A random full stack: crop, global, two locals (radial + linear), LUT, sharpen.
pub fn random_stack(seed: u64) -> EditStack {
    let mut r = Rng::new(seed);
    let mut ops = Vec::new();
    if r.f() < 0.5 {
        ops.push(Op::Crop(Crop {
            rect: [
                r.range(0.0, 0.1),
                r.range(0.0, 0.1),
                r.range(0.7, 0.88),
                r.range(0.7, 0.88),
            ],
            angle: r.range(-4.0, 4.0),
            aspect: None,
        }));
    }
    ops.push(Op::Global(random_adjust(&mut r, 1.0)));
    ops.push(Op::Local(LocalAdjust {
        mask: MaskRef::Radial {
            center: [r.range(0.3, 0.7), r.range(0.3, 0.7)],
            radius: [r.range(0.2, 0.5), r.range(0.2, 0.5)],
            feather: r.range(0.2, 0.9),
        },
        amount: r.range(0.5, 1.0),
        invert: r.f() < 0.3,
        adjust: random_adjust(&mut r, 0.6),
    }));
    ops.push(Op::Local(LocalAdjust {
        mask: MaskRef::Linear {
            start: [0.5, r.range(0.0, 0.3)],
            end: [0.5, r.range(0.5, 1.0)],
        },
        amount: 1.0,
        invert: false,
        adjust: random_adjust(&mut r, 0.5),
    }));
    if r.f() < 0.6 {
        ops.push(Op::Lut(Lut {
            file: ["film_warm", "film_cool", "cinematic", "bw_classic"]
                [(r.next_u64() % 4) as usize]
                .into(),
            amount: r.range(0.3, 1.0),
        }));
    }
    if r.f() < 0.6 {
        ops.push(Op::OutputSharpen(OutputSharpen {
            amount: r.range(5.0, 60.0),
        }));
    }
    EditStack { version: 1, ops }
}

/// Mask provider that serves one synthetic soft mask for every AI target.
pub struct SyntheticMasks(pub Mask);

impl MaskProvider for SyntheticMasks {
    fn mask(&self, _: MaskTarget, _: Option<i64>) -> Result<Option<Mask>> {
        Ok(Some(self.0.clone()))
    }
}

/// Soft elliptical blob mask, low resolution.
pub fn blob_mask(w: u32, h: u32) -> Mask {
    let mut data = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let dx = (x as f32 + 0.5) / w as f32 - 0.5;
            let dy = (y as f32 + 0.5) / h as f32 - 0.55;
            let d = (dx * dx / 0.1 + dy * dy / 0.12).sqrt();
            let v = (1.0 - d).clamp(0.0, 1.0);
            data.push((v * v * (3.0 - 2.0 * v) * 255.0) as u8);
        }
    }
    Mask {
        width: w,
        height: h,
        data,
    }
}

// ---------------------------------------------------------------- synthetic portrait

/// A synthetic upright portrait with consistent geometry: a standing figure with a drawn
/// face (ellipse skin, eyes with irises, brows, mouth with teeth), arms, torso and legs,
/// plus the matching `PersonGeometry` (478 face landmarks, 33 pose keypoints, skin and
/// body mattes). The face landmark indices that the renderer uses are placed exactly on
/// the drawn features (see `portrait.rs`), the rest sit at the face centre.
pub struct SynthPortrait {
    pub image: RgbImage,
    pub person: PersonGeometry,
    /// Face ellipse centre and radii, normalised `[cx, cy, rx, ry]` (x by width, y by height).
    pub face: [f32; 4],
    /// Skin base colour (sRGB).
    pub skin_rgb: [u8; 3],
}

/// Mask provider serving fixed people (and no AI masks).
pub struct PortraitMasks(pub Vec<PersonGeometry>);

impl MaskProvider for PortraitMasks {
    fn mask(&self, _: MaskTarget, _: Option<i64>) -> Result<Option<Mask>> {
        Ok(None)
    }
    fn people(&self) -> Result<Vec<PersonGeometry>> {
        Ok(self.0.clone())
    }
}

/// The portrait's person wrapped as a [`MaskProvider`].
pub fn portrait_masks(p: &SynthPortrait) -> PortraitMasks {
    PortraitMasks(vec![p.person.clone()])
}

const SKIN: [f32; 3] = [224.0, 172.0, 148.0];

struct Layout {
    a: f32,
}

#[derive(Clone, Copy, Default)]
struct Smp {
    rgb: [f32; 3],
    skin: f32,
    body: f32,
}

type Pt = (f32, f32);

fn seg_d(p: Pt, a: Pt, b: Pt) -> f32 {
    let (abx, aby) = (b.0 - a.0, b.1 - a.1);
    let l2 = (abx * abx + aby * aby).max(1e-12);
    let t = (((p.0 - a.0) * abx + (p.1 - a.1) * aby) / l2).clamp(0.0, 1.0);
    ((p.0 - a.0 - abx * t).powi(2) + (p.1 - a.1 - aby * t).powi(2)).sqrt()
}

fn in_ell(p: Pt, c: Pt, rx: f32, ry: f32) -> bool {
    ((p.0 - c.0) / rx).powi(2) + ((p.1 - c.1) / ry).powi(2) <= 1.0
}

fn in_poly(p: Pt, poly: &[Pt]) -> bool {
    let mut inside = false;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < a.0 + (p.1 - a.1) / (b.1 - a.1) * (b.0 - a.0) {
            inside = !inside;
        }
    }
    inside
}

const FACE_RX: f32 = 0.115;
const FACE_RY: f32 = 0.125;
const EYE_DX: f32 = 0.38;
const EYE_DY: f32 = -0.12;
const EYE_W: f32 = 0.24;
const EYE_H: f32 = 0.12;
const IRIS_R: f32 = 0.0105;

fn paint(s: &mut Smp, rgb: [f32; 3], skin: f32, body: f32) {
    s.rgb = rgb;
    s.skin = skin;
    s.body = body;
}

impl Layout {
    fn face_c(&self) -> Pt {
        (0.5, 0.20 * self.a)
    }
    fn eye_c(&self, right: bool) -> Pt {
        let c = self.face_c();
        let s = if right { -1.0 } else { 1.0 };
        (c.0 + s * EYE_DX * FACE_RX, c.1 + EYE_DY * FACE_RY)
    }
    fn mouth_c(&self) -> Pt {
        let c = self.face_c();
        (c.0, c.1 + 0.52 * FACE_RY)
    }
    fn torso(&self) -> [Pt; 6] {
        let a = self.a;
        [
            (0.37, 0.35 * a),
            (0.63, 0.35 * a),
            (0.60, 0.50 * a),
            (0.615, 0.62 * a),
            (0.385, 0.62 * a),
            (0.40, 0.50 * a),
        ]
    }
    /// Pose keypoints in w-units, indexed as MediaPipe Pose (only the used ones are set).
    fn pose_pts(&self) -> Vec<Option<Pt>> {
        let a = self.a;
        let mut p: Vec<Option<Pt>> = vec![None; 33];
        for (i, x, y) in [
            (11usize, 0.64f32, 0.365f32),
            (12, 0.36, 0.365),
            (13, 0.73, 0.52),
            (14, 0.27, 0.52),
            (15, 0.77, 0.68),
            (16, 0.23, 0.68),
            (23, 0.56, 0.62),
            (24, 0.44, 0.62),
            (25, 0.57, 0.79),
            (26, 0.43, 0.79),
            (27, 0.58, 0.93),
            (28, 0.42, 0.93),
            (29, 0.58, 0.945),
            (30, 0.42, 0.945),
            (31, 0.60, 0.955),
            (32, 0.40, 0.955),
        ] {
            p[i] = Some((x, y * a));
        }
        p
    }

    fn sample(&self, x: f32, y: f32) -> Smp {
        let p = (x, y);
        let a = self.a;
        let t = y / a;
        let mut s = Smp {
            rgb: [120.0 - 20.0 * t, 150.0 - 20.0 * t, 135.0 - 20.0 * t],
            ..Default::default()
        };
        let fc = self.face_c();
        // Hair (behind the head).
        if in_ell(
            p,
            (fc.0, fc.1 - 0.3 * FACE_RY),
            FACE_RX * 1.06,
            FACE_RY * 1.05,
        ) {
            paint(&mut s, [40.0, 30.0, 25.0], 0.0, 1.0);
        }
        // Neck.
        if (0.455..0.545).contains(&x) && (fc.1 + 0.5 * FACE_RY..0.36 * a).contains(&y) {
            paint(&mut s, [210.0, 160.0, 138.0], 1.0, 1.0);
        }
        // Torso.
        if in_poly(p, &self.torso()) {
            paint(&mut s, [60.0, 90.0, 140.0], 0.0, 1.0);
        }
        let pp = self.pose_pts();
        let at = |i: usize| pp[i].unwrap();
        // Legs.
        for (h, k, an) in [(23, 25, 27), (24, 26, 28)] {
            let d = seg_d(p, at(h), at(k)).min(seg_d(p, at(k), at(an)));
            if d < 0.04 {
                paint(&mut s, [50.0, 52.0, 62.0], 0.0, 1.0);
            }
        }
        // Arms (bare skin).
        for (sh, el, wr) in [(11, 13, 15), (12, 14, 16)] {
            let d = seg_d(p, at(sh), at(el)).min(seg_d(p, at(el), at(wr)));
            if d < 0.03 {
                paint(&mut s, [214.0, 164.0, 140.0], 1.0, 1.0);
            }
        }
        // Face.
        if in_ell(p, fc, FACE_RX, FACE_RY) {
            let blotch = 4.0 * (x * 47.0).sin() * (y * 41.0).sin();
            paint(
                &mut s,
                [SKIN[0] + blotch, SKIN[1] + blotch, SKIN[2] + blotch],
                1.0,
                1.0,
            );
            // Eyes, brows, mouth are not skin.
            for right in [true, false] {
                let e = self.eye_c(right);
                if in_ell(p, e, EYE_W * FACE_RX, EYE_H * FACE_RY) {
                    paint(&mut s, [244.0, 244.0, 244.0], 0.0, 1.0);
                    if in_ell(p, e, IRIS_R, IRIS_R) {
                        paint(&mut s, [70.0, 45.0, 30.0], 0.0, 1.0);
                    }
                }
                if in_ell(p, (e.0, e.1 - 0.30 * FACE_RY), 0.32 * FACE_RX, 0.016) {
                    paint(&mut s, [60.0, 40.0, 30.0], 0.0, 1.0);
                }
            }
            let m = self.mouth_c();
            if in_ell(p, m, 0.34 * FACE_RX, 0.20 * FACE_RY) {
                paint(&mut s, [185.0, 85.0, 95.0], 0.0, 1.0);
                if in_ell(p, m, 0.30 * FACE_RX, 0.11 * FACE_RY) {
                    paint(&mut s, [80.0, 25.0, 35.0], 0.0, 1.0);
                    if in_ell(
                        p,
                        (m.0, m.1 - 0.02 * FACE_RY),
                        0.26 * FACE_RX,
                        0.07 * FACE_RY,
                    ) {
                        paint(&mut s, [236.0, 226.0, 200.0], 0.0, 1.0);
                    }
                }
            }
        }
        s
    }
}

fn grain(ix: u32, iy: u32) -> f32 {
    let mut h = ix.wrapping_mul(0x9E37_79B1) ^ iy.wrapping_mul(0x85EB_CA77);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    (h & 0xFFFF) as f32 / 32767.5 - 1.0
}

/// Synthetic portrait of size `w x h` (a 3:4 frame gives the intended layout). Skin gets a
/// low-frequency blotch plus per-pixel grain so smoothing is measurable. Deterministic.
pub fn synth_portrait(w: u32, h: u32) -> SynthPortrait {
    use rayon::prelude::*;
    let lay = Layout {
        a: h as f32 / w as f32,
    };
    let ss = if (w as u64 * h as u64) > 2_000_000 {
        1u32
    } else {
        3
    };
    let render = |tw: u32, th: u32, with_grain: bool| -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let mut rgb = vec![0u8; (tw * th * 3) as usize];
        let mut skin = vec![0u8; (tw * th) as usize];
        let mut body = vec![0u8; (tw * th) as usize];
        rgb.par_chunks_mut(tw as usize * 3)
            .zip(skin.par_chunks_mut(tw as usize))
            .zip(body.par_chunks_mut(tw as usize))
            .enumerate()
            .for_each(|(y, ((rrow, srow), brow))| {
                for x in 0..tw as usize {
                    let mut acc = Smp::default();
                    for j in 0..ss {
                        for i in 0..ss {
                            let nx = (x as f32 + (i as f32 + 0.5) / ss as f32) / tw as f32;
                            let ny = (y as f32 + (j as f32 + 0.5) / ss as f32) / th as f32;
                            let s = lay.sample(nx, ny * lay.a);
                            for k in 0..3 {
                                acc.rgb[k] += s.rgb[k];
                            }
                            acc.skin += s.skin;
                            acc.body += s.body;
                        }
                    }
                    let k = 1.0 / (ss * ss) as f32;
                    let g = if with_grain {
                        5.0 * grain(x as u32, y as u32) * acc.skin * k
                    } else {
                        0.0
                    };
                    for c in 0..3 {
                        rrow[x * 3 + c] = (acc.rgb[c] * k + g).clamp(0.0, 255.0).round() as u8;
                    }
                    srow[x] = (acc.skin * k * 255.0).round() as u8;
                    brow[x] = (acc.body * k * 255.0).round() as u8;
                }
            });
        (rgb, skin, body)
    };
    let (data, _, _) = render(w, h, true);
    // Mattes at the worker's resolution (long edge <= 1536).
    let long = w.max(h);
    let f = (1536.0 / long as f32).min(1.0);
    let (mw, mh) = (
        ((w as f32 * f) as u32).max(1),
        ((h as f32 * f) as u32).max(1),
    );
    let (_, skin, body) = render(mw, mh, false);

    let a = lay.a;
    let nrm = |p: Pt| [p.0, p.1 / a];
    let fc = lay.face_c();
    let mut lm = vec![nrm(fc); 478];
    let ell = |c: Pt, rx: f32, ry: f32, deg: f32| {
        let t = deg.to_radians();
        nrm((c.0 + rx * t.cos(), c.1 + ry * t.sin()))
    };
    // Face oval and jaw (angle 0 = subject's left cheek = image right, 90 = chin).
    let oval: [(usize, f32); 36] = [
        (10, 270.0),
        (338, 281.25),
        (297, 292.5),
        (332, 303.75),
        (284, 315.0),
        (251, 326.25),
        (389, 337.5),
        (356, 348.75),
        (454, 0.0),
        (323, 9.0),
        (361, 18.0),
        (288, 27.0),
        (397, 36.0),
        (365, 45.0),
        (379, 54.0),
        (378, 63.0),
        (400, 72.0),
        (377, 81.0),
        (152, 90.0),
        (148, 99.0),
        (176, 108.0),
        (149, 117.0),
        (150, 126.0),
        (136, 135.0),
        (172, 144.0),
        (58, 153.0),
        (132, 162.0),
        (93, 171.0),
        (234, 180.0),
        (127, 191.25),
        (162, 202.5),
        (21, 213.75),
        (54, 225.0),
        (103, 236.25),
        (67, 247.5),
        (109, 258.75),
    ];
    for (i, d) in oval {
        lm[i] = ell(fc, FACE_RX, FACE_RY, d);
    }
    lm[175] = nrm((fc.0, fc.1 + 0.8 * FACE_RY));
    // Eyes: contour loops (upper, lower) and irises. The image-left eye is the subject's
    // right: outer corner 33 at 180 degrees, inner 133 at 0; the other is mirrored.
    for right in [true, false] {
        let c = lay.eye_c(right);
        let (ew, eh) = (EYE_W * FACE_RX, EYE_H * FACE_RY);
        let (corner180, corner0, iris, upper, lower) = if right {
            (
                33,
                133,
                468,
                [246, 161, 160, 159, 158, 157, 173],
                [155, 154, 153, 145, 144, 163, 7],
            )
        } else {
            (
                362,
                263,
                473,
                [398, 384, 385, 386, 387, 388, 466],
                [249, 390, 373, 374, 380, 381, 382],
            )
        };
        lm[corner180] = ell(c, ew, eh, 180.0);
        lm[corner0] = ell(c, ew, eh, 0.0);
        for (k, i) in upper.iter().enumerate() {
            lm[*i] = ell(c, ew, eh, 202.5 + 22.5 * k as f32);
        }
        for (k, i) in lower.iter().enumerate() {
            lm[*i] = ell(c, ew, eh, 22.5 + 22.5 * k as f32);
        }
        lm[iris] = nrm(c);
        for k in 0..4 {
            lm[iris + 1 + k] = ell(c, IRIS_R, IRIS_R, 90.0 * k as f32);
        }
    }
    // Mouth loops (inner and outer lips).
    let m = lay.mouth_c();
    let inner_loop = [
        78usize, 191, 80, 81, 82, 13, 312, 311, 310, 415, 308, 324, 318, 402, 317, 14, 87, 178, 88,
        95,
    ];
    let outer_loop = [
        61usize, 185, 40, 39, 37, 0, 267, 269, 270, 409, 291, 375, 321, 405, 314, 17, 84, 181, 91,
        146,
    ];
    for (loop_, rx, ry) in [
        (inner_loop, 0.30 * FACE_RX, 0.11 * FACE_RY),
        (outer_loop, 0.34 * FACE_RX, 0.20 * FACE_RY),
    ] {
        for (k, i) in loop_.iter().enumerate() {
            let deg = match k {
                0 => 180.0,
                1..=9 => 180.0 + 18.0 * k as f32,
                10 => 0.0,
                _ => 18.0 * (k - 10) as f32,
            };
            lm[*i] = ell(m, rx, ry, deg);
        }
    }
    // Nose.
    let nose = |dx: f32, dy: f32| nrm((fc.0 + dx * FACE_RX, fc.1 + dy * FACE_RY));
    lm[1] = nose(0.0, 0.22);
    lm[4] = nose(0.0, 0.25);
    lm[168] = nose(0.0, -0.2);
    lm[129] = nose(-0.17, 0.30);
    lm[358] = nose(0.17, 0.30);
    lm[98] = nose(-0.11, 0.34);
    lm[327] = nose(0.11, 0.34);
    lm[64] = nose(-0.18, 0.22);
    lm[294] = nose(0.18, 0.22);

    let pose: Vec<[f32; 3]> = lay
        .pose_pts()
        .iter()
        .map(|p| match p {
            Some(q) => [q.0, q.1 / a, 1.0],
            None => [fc.0, fc.1 / a, 0.0],
        })
        .collect();
    let person = PersonGeometry {
        person_id: Some(1),
        face_box: [
            fc.0 - FACE_RX,
            (fc.1 - FACE_RY) / a,
            2.0 * FACE_RX,
            2.0 * FACE_RY / a,
        ],
        face_landmarks: lm,
        pose,
        skin: Some(Mask {
            width: mw,
            height: mh,
            data: skin,
        }),
        body: Some(Mask {
            width: mw,
            height: mh,
            data: body,
        }),
        blemishes: vec![],
    };
    SynthPortrait {
        image: RgbImage {
            width: w,
            height: h,
            data,
        },
        person,
        face: [fc.0, fc.1 / a, FACE_RX, FACE_RY / a],
        skin_rgb: [SKIN[0] as u8, SKIN[1] as u8, SKIN[2] as u8],
    }
}

// ---------------------------------------------------------------- patch assets

/// Synthetic RGBA patch asset: a smooth coloured pattern with some fine detail.
/// `alpha` = `None` gives an opaque asset, otherwise a soft round alpha falling off to
/// zero at the border (like a best-take face blend mask) with that peak value.
pub fn synth_patch(w: u32, h: u32, seed: u64, alpha: Option<f32>) -> RgbaImage {
    let mut r = Rng::new(seed);
    let base = [
        r.range(40.0, 215.0),
        r.range(40.0, 215.0),
        r.range(40.0, 215.0),
    ];
    let (fx, fy) = (r.range(2.0, 6.0), r.range(2.0, 6.0));
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
            let tau = std::f32::consts::TAU;
            let wave = (u * fx * tau).sin() * (v * fy * tau).cos();
            for (k, b) in base.iter().enumerate() {
                let c = b + 35.0 * wave * if k == 1 { -1.0 } else { 1.0 } + 20.0 * (u - v);
                data.push(c.clamp(0.0, 255.0) as u8);
            }
            let a = match alpha {
                None => 1.0,
                Some(peak) => {
                    let d = ((u - 0.5).powi(2) + (v - 0.5).powi(2)).sqrt() * 2.0;
                    peak * (1.0 - d).clamp(0.0, 1.0).sqrt()
                }
            };
            data.push((a * 255.0 + 0.5) as u8);
        }
    }
    RgbaImage {
        width: w,
        height: h,
        data,
    }
}

/// Uniform-colour opaque patch asset.
pub fn solid_patch(w: u32, h: u32, rgb: [u8; 3]) -> RgbaImage {
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..w * h {
        data.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }
    RgbaImage {
        width: w,
        height: h,
        data,
    }
}

/// Mask provider serving named patch assets and counting fetches.
#[derive(Default)]
pub struct PatchAssets {
    pub assets: BTreeMap<String, RgbaImage>,
    pub fetches: std::sync::atomic::AtomicUsize,
}

impl PatchAssets {
    pub fn with(mut self, id: &str, img: RgbaImage) -> PatchAssets {
        self.assets.insert(id.to_string(), img);
        self
    }
}

impl MaskProvider for PatchAssets {
    fn mask(&self, _: MaskTarget, _: Option<i64>) -> Result<Option<Mask>> {
        Ok(None)
    }
    fn patch(&self, asset: &str) -> Result<Option<RgbaImage>> {
        self.fetches
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(self.assets.get(asset).cloned())
    }
}
