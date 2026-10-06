//! Portrait retouching, CPU-side planning (docs/03 §7, docs/api-contract-m4.md §A).
//!
//! Everything here runs once per render at low resolution and produces flat data that
//! both the CPU renderer and the GPU shaders consume through the `Plan`, which is what
//! keeps the two backends identical:
//!
//! * **Warp**: one backward displacement field (`WarpField`, ~1/8 of the output
//!   resolution, bilinear-sampled) summed from face liquify and skeleton-guided body
//!   slimming. Exactly zero outside the support of the contributions.
//! * **Beauty**: skin-mask planes (guided upsampling, like AI masks), a feature map
//!   (eye white / iris / teeth / under-eye regions rasterised from landmark polygons
//!   through the inverse warp) and the blemish list.
//!
//! Coordinate systems. Person geometry arrives as normalised upright pre-crop
//! coordinates. Planning works in "L space": continuous output pixels (before the warp,
//! i.e. the cropped/straightened frame) divided by the output long edge, so distances
//! are isotropic and resolution independent. [`Space`] converts between the two.
//!
//! MediaPipe Face Mesh (478) indices used ("R" = subject's right = image left):
//! * face width: 234 (R cheek edge) .. 454 (L cheek edge); axis: 10 (forehead) .. 152 (chin)
//! * jaw/cheek contour for `slim`: R 234,93,132,58,172,136,150,149,176,148 and
//!   L 454,323,361,288,397,365,379,378,400,377 (weights peak at the jaw angle 58/288)
//! * chin: 152,175,148,377,176,400
//! * eyes (magnification centres): iris centres 468 (R) / 473 (L); corners 33/133, 362/263
//! * nasal alae (`nose`): 129,358 (outer), 98,327, 64,294; midline reference 168
//! * eye contours: R 33,246,161,160,159,158,157,173,133,155,154,153,145,144,163,7;
//!   L 362,398,384,385,386,387,388,466,263,249,390,373,374,380,381,382
//! * lower lids (dark circles): R 33,7,163,144,145,153,154,155,133;
//!   L 362,382,381,380,374,373,390,249,263; irises 468..=472 / 473..=477
//! * inner lip loop (teeth): 78,191,80,81,82,13,312,311,310,415,308,324,318,402,317,14,87,178,88,95
//! * outer lip / face oval (skin-mask fallback): see `LIP_OUTER`, `OVAL`
//!
//! MediaPipe Pose (33) keypoints used: shoulders 11/12, elbows 13/14, wrists 15/16,
//! hips 23/24, knees 25/26, ankles 27/28, heels 29/30, foot index 31/32. A limb is
//! skipped unless both of its keypoints have visibility >= 0.5.

use crate::color::{enc_guide, smoothstep};
use crate::cpu::bilerp_idx;
use crate::geom::{self, Geo};
use crate::prep::{box_mean, mask_at};
use crate::*;

pub type V2 = [f32; 2];

const MIN_VIS: f32 = 0.5;
/// Long edge of the luminance grid used for straight-line evidence.
const EVIDENCE_LONG: f32 = 320.0;

const OVAL: [usize; 36] = [
    10, 338, 297, 332, 284, 251, 389, 356, 454, 323, 361, 288, 397, 365, 379, 378, 400, 377, 152,
    148, 176, 149, 150, 136, 172, 58, 132, 93, 234, 127, 162, 21, 54, 103, 67, 109,
];
const JAW: [(usize, f32); 20] = [
    (234, 0.35),
    (93, 0.7),
    (132, 0.9),
    (58, 1.0),
    (172, 0.95),
    (136, 0.85),
    (150, 0.7),
    (149, 0.5),
    (176, 0.35),
    (148, 0.2),
    (454, 0.35),
    (323, 0.7),
    (361, 0.9),
    (288, 1.0),
    (397, 0.95),
    (365, 0.85),
    (379, 0.7),
    (378, 0.5),
    (400, 0.35),
    (377, 0.2),
];
const CHIN: [(usize, f32); 6] = [
    (152, 1.0),
    (175, 0.8),
    (148, 0.6),
    (377, 0.6),
    (176, 0.4),
    (400, 0.4),
];
const ALAE: [(usize, f32); 6] = [
    (129, 1.0),
    (358, 1.0),
    (98, 0.7),
    (327, 0.7),
    (64, 0.5),
    (294, 0.5),
];
/// (outer corner, inner corner, iris centre, iris ring start)
const EYES: [(usize, usize, usize, usize); 2] = [(33, 133, 468, 469), (263, 362, 473, 474)];
const EYE_LOOP: [[usize; 16]; 2] = [
    [
        33, 246, 161, 160, 159, 158, 157, 173, 133, 155, 154, 153, 145, 144, 163, 7,
    ],
    [
        362, 398, 384, 385, 386, 387, 388, 466, 263, 249, 390, 373, 374, 380, 381, 382,
    ],
];
const LOWER_LID: [[usize; 9]; 2] = [
    [33, 7, 163, 144, 145, 153, 154, 155, 133],
    [362, 382, 381, 380, 374, 373, 390, 249, 263],
];
const LIP_INNER: [usize; 20] = [
    78, 191, 80, 81, 82, 13, 312, 311, 310, 415, 308, 324, 318, 402, 317, 14, 87, 178, 88, 95,
];
const LIP_OUTER: [usize; 20] = [
    61, 185, 40, 39, 37, 0, 267, 269, 270, 409, 291, 375, 321, 405, 314, 17, 84, 181, 91, 146,
];

impl Level {
    /// Maximum face-warp displacement as a fraction of the face width (docs/03 §7.2).
    pub fn face_warp_cap(self) -> f32 {
        match self {
            Level::Natural => 0.03,
            Level::Standard => 0.045,
            Level::Refined => 0.06,
        }
    }
    /// Maximum body-warp displacement as a fraction of the limb (or waist) width.
    pub fn body_warp_cap(self) -> f32 {
        match self {
            Level::Natural => 0.08,
            Level::Standard => 0.115,
            Level::Refined => 0.15,
        }
    }
    /// Maximum leg-lengthening displacement at the feet as a fraction of hip-to-foot length.
    pub fn leg_stretch_cap(self) -> f32 {
        match self {
            Level::Natural => 0.03,
            Level::Standard => 0.045,
            Level::Refined => 0.06,
        }
    }
    /// Scale of the skin-retouching strengths (the slider value maps `0..100 -> 0..scale`).
    pub fn beauty_scale(self) -> f32 {
        match self {
            Level::Natural => 0.5,
            Level::Standard => 0.75,
            Level::Refined => 1.0,
        }
    }
}

// ---------------------------------------------------------------- vector helpers

#[inline]
fn sub(a: V2, b: V2) -> V2 {
    [a[0] - b[0], a[1] - b[1]]
}
#[inline]
fn add(a: V2, b: V2) -> V2 {
    [a[0] + b[0], a[1] + b[1]]
}
#[inline]
fn mul(a: V2, k: f32) -> V2 {
    [a[0] * k, a[1] * k]
}
#[inline]
fn dot(a: V2, b: V2) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}
#[inline]
fn len(a: V2) -> f32 {
    dot(a, a).sqrt()
}
fn norm(a: V2) -> V2 {
    let l = len(a);
    if l < 1e-9 {
        [0.0, 0.0]
    } else {
        mul(a, 1.0 / l)
    }
}

// ---------------------------------------------------------------- spaces

/// Converts between normalised source coordinates and "L space" (see module docs).
#[derive(Clone, Copy)]
pub struct Space {
    pub geo: Geo,
    pub sw: f32,
    pub sh: f32,
    pub ow: f32,
    pub oh: f32,
    pub long: f32,
}

impl Space {
    pub fn new(geo: &Geo) -> Space {
        Space {
            geo: *geo,
            sw: geo.sw as f32,
            sh: geo.sh as f32,
            ow: geo.ow as f32,
            oh: geo.oh as f32,
            long: geo.ow.max(geo.oh) as f32,
        }
    }
    /// Normalised upright pre-crop coordinates -> L space.
    pub fn src_to_l(&self, p: V2) -> V2 {
        let (fx, fy) = self.geo.unmap(p[0] * self.sw, p[1] * self.sh);
        [fx / self.long, fy / self.long]
    }
    /// L space -> normalised upright pre-crop coordinates.
    pub fn l_to_src(&self, p: V2) -> V2 {
        let (sx, sy) = self.geo.map(p[0] * self.long, p[1] * self.long);
        [sx / self.sw, sy / self.sh]
    }
    /// L-space extent of the output frame.
    pub fn ext(&self) -> V2 {
        [self.ow / self.long, self.oh / self.long]
    }
    /// Source pixels per output pixel (isotropic estimate).
    pub fn src_per_out(&self) -> f32 {
        (self.geo.xs + self.geo.ys) * 0.5
    }
}

/// People a portrait op applies to: `None` = every detected face, `Some(id)` = that
/// person only (an unknown id selects nobody, so the op is skipped).
pub fn select(people: &[PersonGeometry], person_id: Option<i64>) -> Vec<&PersonGeometry> {
    match person_id {
        None => people.iter().collect(),
        Some(id) => people.iter().filter(|p| p.person_id == Some(id)).collect(),
    }
}

// ---------------------------------------------------------------- warp field

/// Backward displacement field: output position `(u, v)` is read from
/// `(u, v) + sample(u, v)` (normalised output coordinates). Two floats per cell.
#[derive(Debug, Clone)]
pub struct WarpField {
    pub gw: u32,
    pub gh: u32,
    pub data: Vec<f32>,
}

impl WarpField {
    /// Bilinear sample at normalised output coordinates (cell centres at `i + 0.5`,
    /// clamped at the borders). Mirrored in `shaders/render.wgsl`.
    pub fn sample(&self, u: f32, v: f32) -> [f32; 2] {
        let (x0, x1, fx) = bilerp_idx(u, self.gw);
        let (y0, y1, fy) = bilerp_idx(v, self.gh);
        let w = self.gw as usize;
        let mut o = [0.0f32; 2];
        for (k, ov) in o.iter_mut().enumerate() {
            let at = |x: usize, y: usize| self.data[(y * w + x) * 2 + k];
            let t = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * fx;
            let b = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * fx;
            *ov = t + (b - t) * fy;
        }
        o
    }
    /// Largest displacement magnitude in L space (given the L extent).
    pub fn max_l(&self, ext: V2) -> f32 {
        self.data
            .chunks_exact(2)
            .map(|d| (d[0] * ext[0]).hypot(d[1] * ext[1]))
            .fold(0.0, f32::max)
    }
}

struct Grid {
    gw: usize,
    gh: usize,
    ex: f32,
    ey: f32,
}

impl Grid {
    fn cell(&self, i: usize, j: usize) -> V2 {
        [
            (i as f32 + 0.5) / self.gw as f32 * self.ex,
            (j as f32 + 0.5) / self.gh as f32 * self.ey,
        ]
    }
    /// Cell index ranges `(i0, i1, j0, j1)` (end exclusive) covering an L-space box.
    fn range(&self, lo: V2, hi: V2) -> (usize, usize, usize, usize) {
        let f = |v: f32, ext: f32, n: usize, up: bool| -> usize {
            let x = v / ext * n as f32 - 0.5;
            let x = if up { x.ceil() + 1.0 } else { x.floor() };
            x.clamp(0.0, n as f32) as usize
        };
        (
            f(lo[0], self.ex, self.gw, false),
            f(hi[0], self.ex, self.gw, true),
            f(lo[1], self.ey, self.gh, false),
            f(hi[1], self.ey, self.gh, true),
        )
    }
    fn n(&self) -> usize {
        self.gw * self.gh
    }
    fn cell_len(&self) -> f32 {
        self.ex / self.gw as f32
    }
}

type Cells = (usize, usize, usize, usize);

fn bbox(pts: impl Iterator<Item = V2>, pad: f32) -> (V2, V2) {
    let mut lo = [f32::MAX; 2];
    let mut hi = [f32::MIN; 2];
    for p in pts {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    ([lo[0] - pad, lo[1] - pad], [hi[0] + pad, hi[1] + pad])
}

/// Smooth compact kernel `(1 - t^2)^2`, zero for `t >= 1`.
#[inline]
fn kern(r2: f32, rho: f32) -> f32 {
    let t2 = r2 / (rho * rho);
    if t2 >= 1.0 {
        0.0
    } else {
        (1.0 - t2) * (1.0 - t2)
    }
}

/// Radial-basis liquify of a group of controls: each control `(p, d)` moves the content
/// at `p` by the forward displacement `d`; contributions are blended with a partition of
/// unity (`sum / max(1, sum of weights)`) so neighbouring controls average instead of
/// piling up (no fold-over, magnitude never exceeds `max |d|`). Writes the backward field.
fn rbf(g: &Grid, tmp: &mut [V2], ctrl: &[(V2, V2)], rho: f32) {
    if ctrl.is_empty() {
        return;
    }
    let (lo, hi) = bbox(ctrl.iter().map(|c| c.0), rho);
    let (i0, i1, j0, j1) = g.range(lo, hi);
    for j in j0..j1 {
        for i in i0..i1 {
            let x = g.cell(i, j);
            let (mut sp, mut sv) = (0.0f32, [0.0f32; 2]);
            for (p, d) in ctrl {
                let dx = sub(x, *p);
                let k = kern(dot(dx, dx), rho);
                if k > 0.0 {
                    sp += k;
                    sv = add(sv, mul(*d, k));
                }
            }
            if sp > 0.0 {
                let c = &mut tmp[j * g.gw + i];
                *c = sub(*c, mul(sv, 1.0 / sp.max(1.0)));
            }
        }
    }
}

/// Local radial magnification centred on `c`; `amp` is the forward displacement at the
/// peak of the profile `t (1 - t^2)^2` (positive = enlarge, negative = shrink).
fn radial(g: &Grid, tmp: &mut [V2], c: V2, rho: f32, amp: f32) {
    let (i0, i1, j0, j1) = g.range([c[0] - rho, c[1] - rho], [c[0] + rho, c[1] + rho]);
    for j in j0..j1 {
        for i in i0..i1 {
            let d = sub(g.cell(i, j), c);
            let r = len(d);
            if r > 1e-9 && r < rho {
                let t = r / rho;
                let prof = t * (1.0 - t * t) * (1.0 - t * t) / 0.2862;
                let cell = &mut tmp[j * g.gw + i];
                *cell = sub(*cell, mul(d, amp * prof / r));
            }
        }
    }
}

/// Add `tmp` (clamped to `cap` magnitude) into `total` inside the cell box.
fn merge(g: &Grid, total: &mut [V2], tmp: &[V2], cap: Option<f32>, bx: Cells) {
    for j in bx.2..bx.3 {
        for i in bx.0..bx.1 {
            let k = j * g.gw + i;
            let mut v = tmp[k];
            if let Some(cap) = cap {
                let m = len(v);
                if m > cap {
                    v = mul(v, cap / m);
                }
            }
            total[k] = add(total[k], v);
        }
    }
}

fn landmark(p: &PersonGeometry, sp: &Space, i: usize) -> V2 {
    sp.src_to_l(p.face_landmarks[i])
}

/// Face liquify for one person. Returns `false` when the geometry is unusable.
fn face_warp(
    g: &Grid,
    sp: &Space,
    p: &PersonGeometry,
    level: Level,
    amounts: [f32; 4],
    total: &mut [V2],
) -> bool {
    if p.face_landmarks.len() < 468 {
        return false;
    }
    let pt = |i: usize| landmark(p, sp, i);
    let w = len(sub(pt(234), pt(454)));
    if w < 1e-4 {
        return false;
    }
    let cap = level.face_warp_cap() * w;
    let [slim, chin, eyes, nose] = amounts.map(|a| (a / 100.0).clamp(-1.0, 1.0));
    let top = pt(10);
    let axis = {
        let a = norm(sub(pt(152), top));
        if len(a) < 0.5 {
            [0.0, 1.0]
        } else {
            a
        }
    };
    let (lo, hi) = bbox(p.face_landmarks.iter().map(|q| sp.src_to_l(*q)), 0.4 * w);
    let bx = g.range(lo, hi);
    let mut tmp = vec![[0.0f32; 2]; g.n()];
    if slim != 0.0 {
        let ctrl: Vec<(V2, V2)> = JAW
            .iter()
            .map(|&(i, wt)| {
                let q = pt(i);
                let r = sub(q, top);
                let perp = sub(r, mul(axis, dot(r, axis)));
                (q, mul(norm(perp), -slim * cap * wt))
            })
            .collect();
        rbf(g, &mut tmp, &ctrl, 0.30 * w);
    }
    if chin != 0.0 {
        // chin > 0 moves the chin up (shorter), < 0 down.
        let ctrl: Vec<(V2, V2)> = CHIN
            .iter()
            .map(|&(i, wt)| (pt(i), mul(axis, -chin * cap * wt)))
            .collect();
        rbf(g, &mut tmp, &ctrl, 0.28 * w);
    }
    if nose != 0.0 && p.face_landmarks.len() > 358 {
        let bridge = pt(168);
        let ctrl: Vec<(V2, V2)> = ALAE
            .iter()
            .map(|&(i, wt)| {
                let q = pt(i);
                let r = sub(q, bridge);
                let perp = sub(r, mul(axis, dot(r, axis)));
                (q, mul(norm(perp), -nose * 0.6 * cap * wt))
            })
            .collect();
        rbf(g, &mut tmp, &ctrl, 0.14 * w);
    }
    if eyes != 0.0 {
        for &(outer, inner, iris, _) in &EYES {
            let (a, b) = (pt(outer), pt(inner));
            let c = if p.face_landmarks.len() >= 478 {
                pt(iris)
            } else {
                mul(add(a, b), 0.5)
            };
            let rho = 1.9 * 0.5 * len(sub(a, b));
            let amp = (eyes * cap).clamp(-0.157 * rho, 0.157 * rho);
            radial(g, &mut tmp, c, rho, amp);
        }
    }
    merge(g, total, &tmp, Some(cap), bx);
    true
}

/// Context for one person's body contributions.
struct Body<'a> {
    g: &'a Grid,
    sp: &'a Space,
    p: &'a PersonGeometry,
    /// Per-cell raw body matte (0..1) when the person has one.
    raw: Option<Vec<f32>>,
    /// Region weight: 1 inside the matte, smooth falloff band outside.
    band: Option<Vec<f32>>,
    /// Core ("inside the limb") weight, used by background protection.
    core: Vec<f32>,
    /// Accumulated field of this person (before background protection).
    tmp: Vec<V2>,
}

fn smooth_band(raw: &[f32], g: &Grid) -> Vec<f32> {
    let b = ((0.03 / g.cell_len()).round() as usize).clamp(1, 24);
    let rad = b.div_ceil(2);
    let s = box_mean(&box_mean(raw, g.gw, g.gh, rad), g.gw, g.gh, rad);
    s.iter().map(|v| (v * 2.0).clamp(0.0, 1.0)).collect()
}

impl<'a> Body<'a> {
    fn new(g: &'a Grid, sp: &'a Space, p: &'a PersonGeometry) -> Body<'a> {
        let mut raw = None;
        let mut band = None;
        let mut core = vec![0.0f32; g.n()];
        if let Some(m) = p.body.as_ref().filter(|m| mask_ok(m)) {
            let mut r = vec![0.0f32; g.n()];
            for j in 0..g.gh {
                for i in 0..g.gw {
                    let uv = sp.l_to_src(g.cell(i, j));
                    r[j * g.gw + i] = mask_at(m, uv[0], uv[1]);
                }
            }
            band = Some(smooth_band(&r, g));
            core = box_mean(&r, g.gw, g.gh, 1);
            raw = Some(r);
        }
        Body {
            g,
            sp,
            p,
            raw,
            band,
            core,
            tmp: vec![[0.0; 2]; g.n()],
        }
    }

    fn vis(&self, i: usize) -> bool {
        self.p.pose.len() > i && self.p.pose[i][2] >= MIN_VIS
    }
    fn pt(&self, i: usize) -> V2 {
        self.sp.src_to_l([self.p.pose[i][0], self.p.pose[i][1]])
    }
    fn matte(&self, q: V2) -> f32 {
        match self.p.body.as_ref().filter(|m| mask_ok(m)) {
            Some(m) => {
                let uv = self.sp.l_to_src(q);
                mask_at(m, uv[0], uv[1])
            }
            None => 0.0,
        }
    }
    /// Width of the matte across `normal` at `centre` (None without a matte or when the
    /// centre is outside it).
    fn measure(&self, centre: V2, normal: V2, max_r: f32) -> Option<f32> {
        if self.raw.is_none() || self.matte(centre) < 0.5 {
            return None;
        }
        let step = (self.g.cell_len() * 0.5).max(1e-4);
        let mut w = 0.0;
        for dir in [1.0f32, -1.0] {
            let mut r = 0.0;
            while r < max_r && self.matte(add(centre, mul(normal, dir * r))) >= 0.5 {
                r += step;
            }
            w += r;
        }
        Some(w)
    }

    /// Contract the content around the axis `a`-`b` (limb or torso) by up to
    /// `cap * width * amt`, with `along(t)` shaping the effect along the axis.
    fn contract(
        &mut self,
        axis: (V2, V2),
        amt: f32,
        cap: f32,
        fallback_half_width: f32,
        t_mid: f32,
        along: &dyn Fn(f32) -> f32,
    ) {
        let (a, b) = axis;
        let g = self.g;
        let l = len(sub(b, a));
        if l < 0.01 || amt <= 0.0 {
            return;
        }
        let u = mul(sub(b, a), 1.0 / l);
        let n = [-u[1], u[0]];
        let mid = add(a, mul(sub(b, a), t_mid));
        let hw = self
            .measure(mid, n, 0.6 * l)
            .map(|w| 0.5 * w)
            .unwrap_or(fallback_half_width)
            .clamp(0.03 * l, 0.6 * l);
        let amp = cap * 2.0 * hw * amt;
        let band_l = 0.6 * hw;
        let (lo, hi) = bbox([a, b].into_iter(), hw + band_l + 0.03);
        let (i0, i1, j0, j1) = g.range(lo, hi);
        for j in j0..j1 {
            for i in i0..i1 {
                let k = j * g.gw + i;
                let r = sub(g.cell(i, j), a);
                let t = dot(r, u) / l;
                if t <= 0.0 || t >= 1.0 {
                    continue;
                }
                let tp = along(t);
                if tp <= 0.0 {
                    continue;
                }
                let d = dot(r, n);
                let ad = d.abs();
                let inner = smoothstep(0.0, 1.0, ad / hw);
                let wm = match &self.band {
                    Some(bd) => bd[k],
                    None => 1.0 - smoothstep(hw, hw + band_l, ad),
                };
                if self.raw.is_none() {
                    let c = tp * (1.0 - smoothstep(0.95 * hw, 1.15 * hw, ad));
                    self.core[k] = self.core[k].max(c);
                }
                let s = if d >= 0.0 { 1.0 } else { -1.0 };
                self.tmp[k] = add(self.tmp[k], mul(n, s * amp * inner * tp * wm));
            }
        }
    }

    fn limbs(&mut self, bones: &[(usize, usize)], amt: f32, cap: f32, ratio: f32) {
        for &(i, j) in bones {
            if self.vis(i) && self.vis(j) {
                let (a, b) = (self.pt(i), self.pt(j));
                let taper = |t: f32| smoothstep(0.0, 0.18, t) * (1.0 - smoothstep(0.82, 1.0, t));
                self.contract((a, b), amt, cap, ratio * len(sub(b, a)), 0.5, &taper);
            }
        }
    }

    fn waist(&mut self, amt: f32, cap: f32) {
        if !(self.vis(11) && self.vis(12) && self.vis(23) && self.vis(24)) {
            return;
        }
        let sh = mul(add(self.pt(11), self.pt(12)), 0.5);
        let hp = mul(add(self.pt(23), self.pt(24)), 0.5);
        let hipw = len(sub(self.pt(23), self.pt(24)));
        let bell = |t: f32| smoothstep(0.30, 0.55, t) * (1.0 - smoothstep(0.70, 0.95, t));
        self.contract((sh, hp), amt, cap, 0.7 * hipw, 0.62, &bell);
    }

    fn lengthen(&mut self, amt: f32, stretch_cap: f32) {
        let g = self.g;
        let ext = g.ey;
        if amt <= 0.0 || !(self.vis(23) && self.vis(24)) {
            return;
        }
        let feet: Vec<usize> = [27, 28, 29, 30, 31, 32]
            .into_iter()
            .filter(|&i| self.vis(i))
            .collect();
        if !(self.vis(27) || self.vis(28)) || feet.is_empty() {
            return;
        }
        let hip_y = 0.5 * (self.pt(23)[1] + self.pt(24)[1]);
        let feet_y = feet.iter().map(|&i| self.pt(i)[1]).fold(f32::MIN, f32::max);
        let l = feet_y - hip_y;
        // Only when there is room below the feet (lower image area to absorb the stretch).
        if l < 0.05 || feet_y + 0.03 * ext > ext {
            return;
        }
        let d = stretch_cap * amt * l;
        // Region weight: the legs' matte propagated downwards to the image bottom
        // (below the feet the stretch must continue), then smoothed.
        let region: Vec<f32> = if let Some(raw) = &self.raw {
            let mut r = vec![0.0f32; g.n()];
            for i in 0..g.gw {
                let mut run = 0.0f32;
                for j in 0..g.gh {
                    if g.cell(i, j)[1] < hip_y {
                        continue;
                    }
                    run = run.max(raw[j * g.gw + i]);
                    r[j * g.gw + i] = run;
                }
            }
            smooth_band(&r, g)
        } else {
            let hipw = len(sub(self.pt(23), self.pt(24)));
            let mut r = vec![0.0f32; g.n()];
            let chains: Vec<[V2; 3]> = [(23usize, 25usize, 27usize), (24, 26, 28)]
                .iter()
                .filter(|c| self.vis(c.0) && self.vis(c.1) && self.vis(c.2))
                .map(|c| [self.pt(c.0), self.pt(c.1), self.pt(c.2)])
                .collect();
            for j in 0..g.gh {
                for i in 0..g.gw {
                    let x = g.cell(i, j);
                    if x[1] < hip_y {
                        continue;
                    }
                    let mut best = 0.0f32;
                    for c in &chains {
                        let mut dmin = seg_dist(x, c[0], c[1]).min(seg_dist(x, c[1], c[2]));
                        if x[1] > c[2][1] {
                            dmin = dmin.min((x[0] - c[2][0]).abs());
                        }
                        best = best.max(1.0 - smoothstep(0.45 * hipw, 0.45 * hipw + 0.03, dmin));
                    }
                    r[j * g.gw + i] = best;
                }
            }
            r
        };
        for j in 0..g.gh {
            for i in 0..g.gw {
                let y = g.cell(i, j)[1];
                if y <= hip_y {
                    continue;
                }
                let ramp = smoothstep(0.0, 1.0, ((y - hip_y) / l).min(1.0));
                let k = j * g.gw + i;
                self.tmp[k][1] -= d * ramp * region[k];
                self.core[k] = self.core[k].max(region[k]);
            }
        }
    }
}

fn seg_dist(p: V2, a: V2, b: V2) -> f32 {
    let ab = sub(b, a);
    let l2 = dot(ab, ab).max(1e-12);
    let t = (dot(sub(p, a), ab) / l2).clamp(0.0, 1.0);
    len(sub(p, add(a, mul(ab, t))))
}

fn mask_ok(m: &Mask) -> bool {
    m.width > 0 && m.height > 0 && m.data.len() == (m.width * m.height) as usize
}

/// Straight-line evidence in the output frame (structure-tensor coherence) turned into a
/// per-cell attenuation `1 - 0.9 * evidence * (1 - inside)`.
///
/// Method: luminance (sRGB-encoded) on a ~320 px grid of the *unwarped* frame, 3x3 box
/// pre-blur, central-difference gradients, structure tensor smoothed with a 5x5 box,
/// coherence `((l1 - l2) / (l1 + l2))^2` gated by gradient energy, dilated by ~2 px, then
/// resampled to the field grid. Edges that are not straight (blobs, texture) have low
/// coherence and are left alone; long straight edges (walls, horizons, door frames) are
/// protected. `inside` (the person's own matte core) is never attenuated.
fn line_attenuation(src: &RgbImage, sp: &Space, g: &Grid, inside: &[f32]) -> Vec<f32> {
    use rayon::prelude::*;
    let ll = sp.ow.max(sp.oh).min(EVIDENCE_LONG);
    let lw = ((sp.ow * ll / sp.long).round() as usize).max(4);
    let lh = ((sp.oh * ll / sp.long).round() as usize).max(4);
    let gg = sp.geo.for_grid(lw as u32, lh as u32);
    let mut lum = vec![0.0f32; lw * lh];
    lum.par_chunks_mut(lw).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            *o = enc_guide(geom::sample(src, &gg, x as u32, y as u32));
        }
    });
    let lum = box_mean(&lum, lw, lh, 1);
    // Structure tensor components (gx^2, gx gy, gy^2).
    let mut jj = vec![[0.0f32; 3]; lw * lh];
    jj.par_chunks_mut(lw).enumerate().for_each(|(y, row)| {
        let at = |x: i32, y: i32| {
            lum[y.clamp(0, lh as i32 - 1) as usize * lw + x.clamp(0, lw as i32 - 1) as usize]
        };
        for (x, o) in row.iter_mut().enumerate() {
            let (xi, yi) = (x as i32, y as i32);
            let gx = (at(xi + 1, yi) - at(xi - 1, yi)) * 0.5;
            let gy = (at(xi, yi + 1) - at(xi, yi - 1)) * 0.5;
            *o = [gx * gx, gx * gy, gy * gy];
        }
    });
    let split = |k: usize| -> Vec<f32> { jj.iter().map(|v| v[k]).collect() };
    let (j11, j12, j22) = (
        box_mean(&split(0), lw, lh, 2),
        box_mean(&split(1), lw, lh, 2),
        box_mean(&split(2), lw, lh, 2),
    );
    let mut ev = vec![0.0f32; lw * lh];
    ev.par_chunks_mut(lw).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let i = y * lw + x;
            let tr = j11[i] + j22[i];
            let disc = ((j11[i] - j22[i]).powi(2) + 4.0 * j12[i] * j12[i]).sqrt();
            let coh = (disc / (tr + 1e-9)).powi(2);
            *o = coh * smoothstep(0.012, 0.045, tr.sqrt());
        }
    });
    // Max-filter dilation (separable) by the cell footprint.
    let rad = 2 + (lw as f32 / g.gw as f32).ceil() as i32;
    let mut tmp = vec![0.0f32; lw * lh];
    tmp.par_chunks_mut(lw).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut m = 0.0f32;
            for d in -rad..=rad {
                m = m.max(ev[y * lw + (x as i32 + d).clamp(0, lw as i32 - 1) as usize]);
            }
            *o = m;
        }
    });
    let mut dil = vec![0.0f32; lw * lh];
    dil.par_chunks_mut(lw).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut m = 0.0f32;
            for d in -rad..=rad {
                m = m.max(tmp[(y as i32 + d).clamp(0, lh as i32 - 1) as usize * lw + x]);
            }
            *o = m;
        }
    });
    let mut att = vec![1.0f32; g.n()];
    for j in 0..g.gh {
        for i in 0..g.gw {
            let x =
                ((i as f32 + 0.5) / g.gw as f32 * lw as f32).clamp(0.0, lw as f32 - 1.0) as usize;
            let y =
                ((j as f32 + 0.5) / g.gh as f32 * lh as f32).clamp(0.0, lh as f32 - 1.0) as usize;
            let k = j * g.gw + i;
            att[k] = 1.0 - 0.9 * dil[y * lw + x] * (1.0 - inside[k].clamp(0.0, 1.0));
        }
    }
    box_mean(&att, g.gw, g.gh, 1)
}

/// True when the op would move pixels (at least one non-zero amount).
pub fn warp_active(w: &Warp) -> bool {
    match w {
        Warp::Face {
            slim,
            chin,
            eyes,
            nose,
            ..
        } => [slim, chin, eyes, nose].iter().any(|a| **a != 0.0),
        Warp::Body {
            arms,
            legs,
            waist,
            lengthen_legs,
            ..
        } => [arms, legs, waist, lengthen_legs].iter().any(|a| **a > 0.0),
        Warp::Unknown => false,
    }
}

/// Build the warp field for all `Warp` ops of a stack. `None` when nothing moves (no
/// active op, no matching person, unusable geometry).
pub fn build_warp(
    src: &RgbImage,
    geo: &Geo,
    ops: &[&Warp],
    people: &[PersonGeometry],
) -> Option<WarpField> {
    let sp = Space::new(geo);
    let ext = sp.ext();
    // 1/8 of the output resolution, bounded.
    let gl = (sp.long / 8.0).ceil().clamp(16.0, 768.0);
    let gw = ((sp.ow * gl / sp.long).round() as usize).max(2);
    let gh = ((sp.oh * gl / sp.long).round() as usize).max(2);
    let g = Grid {
        gw,
        gh,
        ex: ext[0],
        ey: ext[1],
    };
    let mut total = vec![[0.0f32; 2]; g.n()];
    let mut any = false;
    for op in ops {
        match op {
            Warp::Face {
                person_id,
                level,
                slim,
                chin,
                eyes,
                nose,
            } => {
                for p in select(people, *person_id) {
                    any |= face_warp(&g, &sp, p, *level, [*slim, *chin, *eyes, *nose], &mut total);
                }
            }
            Warp::Body {
                person_id,
                level,
                arms,
                legs,
                waist,
                lengthen_legs,
                protect_background,
            } => {
                for p in select(people, *person_id) {
                    if p.pose.len() < 29 {
                        continue;
                    }
                    let mut b = Body::new(&g, &sp, p);
                    let cap = level.body_warp_cap();
                    let n01 = |a: f32| (a / 100.0).clamp(0.0, 1.0);
                    b.limbs(
                        &[(11, 13), (13, 15), (12, 14), (14, 16)],
                        n01(*arms),
                        cap,
                        0.17,
                    );
                    b.limbs(
                        &[(23, 25), (25, 27), (24, 26), (26, 28)],
                        n01(*legs),
                        cap,
                        0.2,
                    );
                    b.waist(n01(*waist), cap);
                    b.lengthen(n01(*lengthen_legs), level.leg_stretch_cap());
                    let att = if *protect_background {
                        Some(line_attenuation(src, &sp, &g, &b.core))
                    } else {
                        None
                    };
                    let mut touched = false;
                    for (k, v) in b.tmp.iter().enumerate() {
                        if v[0] != 0.0 || v[1] != 0.0 {
                            touched = true;
                            let a = att.as_ref().map(|a| a[k]).unwrap_or(1.0);
                            total[k] = add(total[k], mul(*v, a));
                        }
                    }
                    any |= touched;
                }
            }
            Warp::Unknown => {}
        }
    }
    if !any {
        return None;
    }
    let mut data = Vec::with_capacity(g.n() * 2);
    let mut nonzero = false;
    for v in &total {
        // L units -> normalised output units.
        let (du, dv) = (v[0] / ext[0], v[1] / ext[1]);
        nonzero |= du != 0.0 || dv != 0.0;
        data.push(du);
        data.push(dv);
    }
    nonzero.then_some(WarpField {
        gw: gw as u32,
        gh: gh as u32,
        data,
    })
}

// ---------------------------------------------------------------- polygons

fn poly_signed(poly: &[V2], q: V2) -> f32 {
    let n = poly.len();
    let mut inside = false;
    let mut dmin = f32::MAX;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a[1] > q[1]) != (b[1] > q[1]) {
            let t = (q[1] - a[1]) / (b[1] - a[1]);
            if q[0] < a[0] + t * (b[0] - a[0]) {
                inside = !inside;
            }
        }
        dmin = dmin.min(seg_dist(q, a, b));
    }
    if inside {
        dmin
    } else {
        -dmin
    }
}

fn poly_cov(poly: &[V2], q: V2, feather: f32) -> f32 {
    smoothstep(-feather, feather, poly_signed(poly, q))
}

fn disp_l(warp: Option<&WarpField>, ext: V2, q: V2) -> V2 {
    match warp {
        Some(w) => {
            let d = w.sample(q[0] / ext[0], q[1] / ext[1]);
            [d[0] * ext[0], d[1] * ext[1]]
        }
        None => [0.0, 0.0],
    }
}

fn face_poly(p: &PersonGeometry, sp: &Space, idx: &[usize]) -> Vec<V2> {
    idx.iter().map(|&i| landmark(p, sp, i)).collect()
}

// ---------------------------------------------------------------- skin fallback

/// Skin coverage from the face oval minus eyes and lips (used when the worker supplied no
/// skin matte but landmarks exist).
struct SkinPolys {
    oval: Vec<V2>,
    eyes: [Vec<V2>; 2],
    lips: Vec<V2>,
    feather: f32,
}

impl SkinPolys {
    fn new(p: &PersonGeometry, sp: &Space) -> Option<SkinPolys> {
        if p.face_landmarks.len() < 468 {
            return None;
        }
        let w = len(sub(landmark(p, sp, 234), landmark(p, sp, 454)));
        if w < 1e-4 {
            return None;
        }
        Some(SkinPolys {
            oval: face_poly(p, sp, &OVAL),
            eyes: [
                face_poly(p, sp, &EYE_LOOP[0]),
                face_poly(p, sp, &EYE_LOOP[1]),
            ],
            lips: face_poly(p, sp, &LIP_OUTER),
            feather: 0.02 * w,
        })
    }
    fn at(&self, s: V2) -> f32 {
        let mut v = poly_cov(&self.oval, s, self.feather);
        if v <= 0.0 {
            return 0.0;
        }
        for e in &self.eyes {
            v *= 1.0 - poly_cov(e, s, self.feather);
        }
        v * (1.0 - poly_cov(&self.lips, s, self.feather))
    }
}

/// Union (max) of the selected people's skin coverage on a `gw x gh` grid of the output
/// frame (read through the warp), as a flat plane. `None` when nobody has skin geometry.
pub fn skin_plane(
    sp: &Space,
    warp: Option<&WarpField>,
    people: &[&PersonGeometry],
    gw: usize,
    gh: usize,
) -> Option<Vec<f32>> {
    use rayon::prelude::*;
    enum Src<'a> {
        Mask(&'a Mask),
        Poly(Box<SkinPolys>),
    }
    let srcs: Vec<Src> = people
        .iter()
        .filter_map(|p| match p.skin.as_ref().filter(|m| mask_ok(m)) {
            Some(m) => Some(Src::Mask(m)),
            None => SkinPolys::new(p, sp).map(|s| Src::Poly(Box::new(s))),
        })
        .collect();
    if srcs.is_empty() {
        return None;
    }
    let ext = sp.ext();
    let g = sp.geo.for_grid(gw as u32, gh as u32);
    let mut out = vec![0.0f32; gw * gh];
    out.par_chunks_mut(gw).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let (u, v) = ((x as f32 + 0.5) / gw as f32, (y as f32 + 0.5) / gh as f32);
            let d = warp.map(|w| w.sample(u, v)).unwrap_or([0.0, 0.0]);
            let (px, py) = (
                x as f32 + 0.5 + d[0] * gw as f32,
                y as f32 + 0.5 + d[1] * gh as f32,
            );
            let mut best = 0.0f32;
            for s in &srcs {
                let c = match s {
                    Src::Mask(m) => {
                        let (sx, sy) = g.map(px, py);
                        mask_at(m, sx / sp.sw, sy / sp.sh)
                    }
                    Src::Poly(pl) => pl.at([px / gw as f32 * ext[0], py / gh as f32 * ext[1]]),
                };
                best = best.max(c);
            }
            *o = best;
        }
    });
    Some(out)
}

// ---------------------------------------------------------------- feature map

/// Per-pixel feature weights on a grid of the output frame, 4 floats per cell:
/// `x` eye white, `y` iris, `z` teeth, `w` under-eye (dark circles) region.
pub struct FeatureMap {
    pub w: u32,
    pub h: u32,
    pub data: Vec<f32>,
}

pub fn feature_map(
    sp: &Space,
    warp: Option<&WarpField>,
    people: &[&PersonGeometry],
    eyes: bool,
    teeth: bool,
    dark: bool,
) -> Option<FeatureMap> {
    let ext = sp.ext();
    let fl = sp.long.min(768.0);
    let fw = ((sp.ow * fl / sp.long).round() as usize).max(2);
    let fh = ((sp.oh * fl / sp.long).round() as usize).max(2);
    let g = Grid {
        gw: fw,
        gh: fh,
        ex: ext[0],
        ey: ext[1],
    };
    let maxd = warp.map(|w| w.max_l(ext)).unwrap_or(0.0);
    let mut data = vec![0.0f32; fw * fh * 4];
    let mut any = false;
    // Evaluate `f(s)` for every cell whose warped source position may touch the box.
    let mut raster = |lo: V2, hi: V2, ch: usize, f: &dyn Fn(V2) -> f32| {
        let (i0, i1, j0, j1) = g.range([lo[0] - maxd, lo[1] - maxd], [hi[0] + maxd, hi[1] + maxd]);
        for j in j0..j1 {
            for i in i0..i1 {
                let q = g.cell(i, j);
                let d = disp_l(warp, ext, q);
                let c = f(add(q, d));
                let slot = &mut data[(j * fw + i) * 4 + ch];
                if c > *slot {
                    *slot = c;
                }
            }
        }
    };
    for p in people {
        let n = p.face_landmarks.len();
        if n < 468 {
            continue;
        }
        let pt = |i: usize| landmark(p, sp, i);
        let width = len(sub(pt(234), pt(454)));
        if width < 1e-4 {
            continue;
        }
        let down = {
            let a = norm(sub(pt(152), pt(10)));
            if len(a) < 0.5 {
                [0.0, 1.0]
            } else {
                a
            }
        };
        if eyes {
            for (e, &(outer, inner, iris, ring)) in EYES.iter().enumerate() {
                let poly = face_poly(p, sp, &EYE_LOOP[e]);
                let ew = len(sub(pt(outer), pt(inner)));
                let fe = 0.06 * ew;
                let (lo, hi) = bbox(poly.iter().copied(), fe * 2.0);
                let (ic, ir) = if n >= 478 {
                    let c = pt(iris);
                    let r = (0..4).map(|k| len(sub(pt(ring + k), c))).sum::<f32>() / 4.0;
                    (Some(c), r)
                } else {
                    (None, 0.0)
                };
                let iris_cov = move |s: V2| match ic {
                    Some(c) => 1.0 - smoothstep(ir * 0.85, ir * 1.1, len(sub(s, c))),
                    None => 0.0,
                };
                raster(lo, hi, 0, &|s| poly_cov(&poly, s, fe) * (1.0 - iris_cov(s)));
                if ic.is_some() {
                    raster(lo, hi, 1, &|s| poly_cov(&poly, s, fe) * iris_cov(s));
                }
                any = true;
            }
        }
        if teeth {
            let poly = face_poly(p, sp, &LIP_INNER);
            let fe = 0.012 * width;
            let (lo, hi) = bbox(poly.iter().copied(), fe * 2.0);
            raster(lo, hi, 2, &|s| {
                smoothstep(-fe, fe, poly_signed(&poly, s) - 0.5 * fe)
            });
            any = true;
        }
        if dark {
            for lid in &LOWER_LID {
                let pts = face_poly(p, sp, lid);
                let ew = len(sub(pts[0], pts[8]));
                let h = 0.5 * ew;
                let mut poly = pts.clone();
                for (i, q) in pts.iter().enumerate().rev() {
                    let f = 0.3 + 0.7 * (std::f32::consts::PI * i as f32 / 8.0).sin();
                    poly.push(add(*q, mul(down, h * f)));
                }
                let fe = 0.2 * h;
                let (lo, hi) = bbox(poly.iter().copied(), fe * 2.0);
                raster(lo, hi, 3, &|s| poly_cov(&poly, s, fe));
                any = true;
            }
        }
    }
    any.then_some(FeatureMap {
        w: fw as u32,
        h: fh as u32,
        data,
    })
}

// ---------------------------------------------------------------- blemishes

/// Maximum blemishes per render (largest first).
pub const MAX_BLEMISHES: usize = 64;

/// Blemishes inside the skin as `[u, v, r, 0]`: position in normalised output coordinates
/// (pre-warp positions pushed through the warp), radius in units of the output long edge.
pub fn blemishes(
    sp: &Space,
    warp: Option<&WarpField>,
    people: &[&PersonGeometry],
) -> Vec<[f32; 4]> {
    let ext = sp.ext();
    let mut all: Vec<(f32, V2)> = Vec::new();
    for p in people {
        for b in &p.blemishes {
            let c = [b[0], b[1]];
            if !(0.0..=1.0).contains(&c[0]) || !(0.0..=1.0).contains(&c[1]) {
                continue;
            }
            if let Some(m) = p.skin.as_ref().filter(|m| mask_ok(m)) {
                if mask_at(m, c[0], c[1]) < 0.2 {
                    continue;
                }
            }
            let r = b[2] * sp.sw.max(sp.sh) / sp.src_per_out() / sp.long;
            if r > 0.0 && r < 0.2 {
                all.push((r, sp.src_to_l(c)));
            }
        }
    }
    all.sort_by(|a, b| b.0.total_cmp(&a.0));
    all.truncate(MAX_BLEMISHES);
    all.into_iter()
        .map(|(r, s)| {
            // Invert q + d(q) = s by fixed-point iteration.
            let mut q = s;
            for _ in 0..4 {
                q = sub(s, disp_l(warp, ext, q));
            }
            [q[0] / ext[0], q[1] / ext[1], r, 0.0]
        })
        .collect()
}

/// Mean face width in L units (from `face_box`) of the people, or 0.
pub fn mean_face_width(sp: &Space, people: &[&PersonGeometry]) -> f32 {
    if people.is_empty() {
        return 0.0;
    }
    let s: f32 = people
        .iter()
        .map(|p| p.face_box[2].max(0.0) * sp.sw / sp.src_per_out() / sp.long)
        .sum();
    s / people.len() as f32
}
