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
