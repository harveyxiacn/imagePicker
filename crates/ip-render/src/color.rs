//! Colour maths shared by the CPU renderer, the plan builder and the tests.
//! The WGSL shader mirrors these formulas; keep them in sync.
#![allow(clippy::excessive_precision)]

use std::sync::OnceLock;

pub const LUM: [f32; 3] = [0.2126, 0.7152, 0.0722];

#[inline]
pub fn lum(c: [f32; 3]) -> f32 {
    c[0] * LUM[0] + c[1] * LUM[1] + c[2] * LUM[2]
}

/// Linear -> sRGB transfer (input clamped to 0..1).
#[inline]
pub fn srgb_enc(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if x <= 0.003_130_8 {
        x * 12.92
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    }
}

/// sRGB -> linear transfer.
#[inline]
pub fn srgb_dec(x: f32) -> f32 {
    if x <= 0.040_45 {
        x / 12.92
    } else {
        ((x + 0.055) / 1.055).powf(2.4)
    }
}

/// 8-bit sRGB -> linear lookup table.
pub fn dec_lut() -> &'static [f32; 256] {
    static LUT: OnceLock<[f32; 256]> = OnceLock::new();
    LUT.get_or_init(|| {
        let mut t = [0.0f32; 256];
        for (i, v) in t.iter_mut().enumerate() {
            *v = srgb_dec(i as f32 / 255.0);
        }
        t
    })
}

/// Perceptual luminance used as the guide for AI-mask upsampling.
#[inline]
pub fn enc_guide(c: [f32; 3]) -> f32 {
    srgb_enc(lum(c))
}

#[inline]
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn cbrt_pos(x: f32) -> f32 {
    if x > 1e-12 {
        x.powf(1.0 / 3.0)
    } else {
        0.0
    }
}

/// Linear sRGB -> OKLab.
#[inline]
pub fn to_oklab(c: [f32; 3]) -> [f32; 3] {
    let (r, g, b) = (c[0], c[1], c[2]);
    let l = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b;
    let m = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b;
    let s = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b;
    let (l, m, s) = (cbrt_pos(l), cbrt_pos(m), cbrt_pos(s));
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    ]
}

/// OKLab -> linear sRGB.
#[inline]
pub fn from_oklab(c: [f32; 3]) -> [f32; 3] {
    let (ll, a, b) = (c[0], c[1], c[2]);
    let l = ll + 0.3963377774 * a + 0.2158037573 * b;
    let m = ll - 0.1055613458 * a - 0.0638541728 * b;
    let s = ll - 0.0894841775 * a - 1.2914855480 * b;
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    [
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
    ]
}

// ---------------------------------------------------------------- white balance

type M3 = [[f64; 3]; 3];

const RGB2XYZ: M3 = [
    [0.4124564, 0.3575761, 0.1804375],
    [0.2126729, 0.7151522, 0.0721750],
    [0.0193339, 0.1191920, 0.9503041],
];
const BRADFORD: M3 = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];

fn mul(a: &M3, b: &M3) -> M3 {
    let mut o = [[0.0; 3]; 3];
    for (i, row) in o.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    o
}

fn inv(m: &M3) -> M3 {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let d = 1.0 / det;
    [
        [
            (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * d,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * d,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * d,
        ],
        [
            (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * d,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * d,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * d,
        ],
        [
            (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * d,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * d,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * d,
        ],
    ]
}

/// Planckian locus chromaticity (Kim et al. cubic spline), valid 1667..25000 K.
fn planck_xy(t: f64) -> (f64, f64) {
    let t = t.clamp(1700.0, 24_000.0);
    let x = if t <= 4000.0 {
        -0.2661239e9 / (t * t * t) - 0.2343589e6 / (t * t) + 0.8776956e3 / t + 0.179910
    } else {
        -3.0258469e9 / (t * t * t) + 2.1070379e6 / (t * t) + 0.2226347e3 / t + 0.240390
    };
    let y = if t <= 2222.0 {
        -1.1063814 * x * x * x - 1.34811020 * x * x + 2.18555832 * x - 0.20219683
    } else if t <= 4000.0 {
        -0.9549476 * x * x * x - 1.37418593 * x * x + 2.09137015 * x - 0.16748867
    } else {
        3.0817580 * x * x * x - 5.87338670 * x * x + 3.75112997 * x - 0.37001483
    };
    (x, y)
}

fn xy_to_uv(x: f64, y: f64) -> (f64, f64) {
    let d = -2.0 * x + 12.0 * y + 3.0;
    (4.0 * x / d, 6.0 * y / d)
}

fn uv_to_xy(u: f64, v: f64) -> (f64, f64) {
    let d = 2.0 * u - 8.0 * v + 4.0;
    (3.0 * u / d, 2.0 * v / d)
}

/// xy of an illuminant on the locus at `t` K displaced by `duv` perpendicular to it
/// (positive = greener).
fn illuminant_xy(t: f64, duv: f64) -> (f64, f64) {
    let (x, y) = planck_xy(t);
    let (u, v) = xy_to_uv(x, y);
    let (x1, y1) = planck_xy(t - 50.0);
    let (x2, y2) = planck_xy(t + 50.0);
    let (u1, v1) = xy_to_uv(x1, y1);
    let (u2, v2) = xy_to_uv(x2, y2);
    let (du, dv) = (u2 - u1, v2 - v1);
    let n = (du * du + dv * dv).sqrt().max(1e-12);
    let (mut nu, mut nv) = (-dv / n, du / n);
    if nv < 0.0 {
        nu = -nu;
        nv = -nv;
    }
    uv_to_xy(u + nu * duv, v + nv * duv)
}

/// Linear-sRGB matrix (row-major) for a white-balance shift: the scene is assumed to have
/// been lit by an illuminant `temp` K away from the 6500 K reference (and `tint` off the
/// locus); Bradford adaptation maps that illuminant to the reference white.
/// Positive `temp` = warmer picture, positive `tint` = more magenta.
pub fn wb_matrix(temp: f32, tint: f32) -> [f32; 9] {
    if temp == 0.0 && tint == 0.0 {
        return [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    }
    let white = |x: f64, y: f64| [x / y, 1.0, (1.0 - x - y) / y];
    let (rx, ry) = illuminant_xy(6500.0, 0.0);
    let (sx, sy) = illuminant_xy(6500.0 + temp as f64, tint as f64 / 100.0 * 0.02);
    let (wr, ws) = (white(rx, ry), white(sx, sy));
    let lms = |w: [f64; 3]| -> [f64; 3] {
        let mut o = [0.0; 3];
        for (i, v) in o.iter_mut().enumerate() {
            *v = (0..3).map(|k| BRADFORD[i][k] * w[k]).sum();
        }
        o
    };
    let (lr, ls) = (lms(wr), lms(ws));
    let diag: M3 = [
        [lr[0] / ls[0], 0.0, 0.0],
        [0.0, lr[1] / ls[1], 0.0],
        [0.0, 0.0, lr[2] / ls[2]],
    ];
    let ad = mul(&inv(&BRADFORD), &mul(&diag, &BRADFORD));
    let m = mul(&inv(&RGB2XYZ), &mul(&ad, &RGB2XYZ));
    let mut o = [0.0f32; 9];
    for i in 0..3 {
        for j in 0..3 {
            o[i * 3 + j] = m[i][j] as f32;
        }
    }
    o
}

// ---------------------------------------------------------------- ΔE2000

/// CIEDE2000 between two 8-bit sRGB pixels (used by the parity tests and the bench).
pub fn delta_e2000(a: [u8; 3], b: [u8; 3]) -> f64 {
    de2000(lab_d65(a), lab_d65(b))
}

fn lab_d65(p: [u8; 3]) -> [f64; 3] {
    let lin: Vec<f64> = p
        .iter()
        .map(|&v| srgb_dec(v as f32 / 255.0) as f64)
        .collect();
    let mut xyz = [0.0; 3];
    for (i, v) in xyz.iter_mut().enumerate() {
        *v = RGB2XYZ[i][0] * lin[0] + RGB2XYZ[i][1] * lin[1] + RGB2XYZ[i][2] * lin[2];
    }
    let w = [0.95047, 1.0, 1.08883];
    let f = |t: f64| {
        if t > 216.0 / 24389.0 {
            t.cbrt()
        } else {
            (24389.0 / 27.0 * t + 16.0) / 116.0
        }
    };
    let (fx, fy, fz) = (f(xyz[0] / w[0]), f(xyz[1] / w[1]), f(xyz[2] / w[2]));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

fn de2000(a: [f64; 3], b: [f64; 3]) -> f64 {
    use std::f64::consts::PI;
    let (l1, a1, b1) = (a[0], a[1], a[2]);
    let (l2, a2, b2) = (b[0], b[1], b[2]);
    let c1 = a1.hypot(b1);
    let c2 = a2.hypot(b2);
    let cb = (c1 + c2) / 2.0;
    let g = 0.5 * (1.0 - (cb.powi(7) / (cb.powi(7) + 25f64.powi(7))).sqrt());
    let (a1p, a2p) = ((1.0 + g) * a1, (1.0 + g) * a2);
    let (c1p, c2p) = (a1p.hypot(b1), a2p.hypot(b2));
    let hp = |bb: f64, ap: f64| {
        if bb == 0.0 && ap == 0.0 {
            0.0
        } else {
            let h = bb.atan2(ap).to_degrees();
            if h < 0.0 {
                h + 360.0
            } else {
                h
            }
        }
    };
    let (h1p, h2p) = (hp(b1, a1p), hp(b2, a2p));
    let dl = l2 - l1;
    let dc = c2p - c1p;
    let dh = if c1p * c2p == 0.0 {
        0.0
    } else if (h2p - h1p).abs() <= 180.0 {
        h2p - h1p
    } else if h2p - h1p > 180.0 {
        h2p - h1p - 360.0
    } else {
        h2p - h1p + 360.0
    };
    let dhh = 2.0 * (c1p * c2p).sqrt() * (dh.to_radians() / 2.0).sin();
    let lbp = (l1 + l2) / 2.0;
    let cbp = (c1p + c2p) / 2.0;
    let hbp = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) / 2.0
    } else {
        (h1p + h2p - 360.0) / 2.0
    };
    let t = 1.0 - 0.17 * (hbp - 30.0).to_radians().cos()
        + 0.24 * (2.0 * hbp).to_radians().cos()
        + 0.32 * (3.0 * hbp + 6.0).to_radians().cos()
        - 0.20 * (4.0 * hbp - 63.0).to_radians().cos();
    let dtheta = 30.0 * (-((hbp - 275.0) / 25.0).powi(2)).exp();
    let rc = 2.0 * (cbp.powi(7) / (cbp.powi(7) + 25f64.powi(7))).sqrt();
    let sl = 1.0 + 0.015 * (lbp - 50.0).powi(2) / (20.0 + (lbp - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * cbp;
    let sh = 1.0 + 0.015 * cbp * t;
    let rt = -(2.0 * dtheta * PI / 180.0).sin() * rc;
    ((dl / sl).powi(2) + (dc / sc).powi(2) + (dhh / sh).powi(2) + rt * (dc / sc) * (dhh / sh))
        .max(0.0)
        .sqrt()
}
