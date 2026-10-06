//! Plan building: resolves an `EditStack` into flat, backend-neutral data (op records,
//! curve tables, mask planes, LUTs). Both the CPU and the GPU renderer consume a `Plan`,
//! which is what keeps the two in step.

use crate::color::{enc_guide, wb_matrix};
use crate::geom::{self, Geo};
use crate::*;

/// Floats per op record (16 vec4).
pub const REC: usize = 64;

pub mod r {
    pub const KIND: usize = 0;
    pub const FLAGS: usize = 1;
    pub const AMOUNT: usize = 2;
    pub const INVERT: usize = 3;
    pub const WB: usize = 4; // 9 floats, row-major
    pub const GAIN: usize = 13;
    pub const KNEE: usize = 14;
    pub const EV: usize = 15;
    pub const SH: usize = 16;
    pub const HI: usize = 17;
    pub const WHITES: usize = 18;
    pub const BLACKS: usize = 19;
    pub const CONTRAST: usize = 20;
    pub const CLARITY: usize = 21;
    pub const DEHAZE_W: usize = 22;
    pub const DEHAZE_S: usize = 23;
    pub const VIBRANCE: usize = 24;
    pub const SAT: usize = 25;
    pub const CURVE_OFF: usize = 26;
    pub const LAYER_OFF: usize = 27;
    pub const M0: usize = 28; // mask params (5 floats)
    pub const PIVOT: usize = 33;
    pub const GRADE: usize = 34; // 6 floats: shadows a,b; mids a,b; highlights a,b
    pub const HSL: usize = 40; // 8 x (h, s, l)

    pub const KIND_GLOBAL: f32 = 0.0;
    pub const KIND_AI: f32 = 1.0;
    pub const KIND_RADIAL: f32 = 2.0;
    pub const KIND_LINEAR: f32 = 3.0;

    pub const F_WB: u32 = 1;
    pub const F_EV: u32 = 2;
    pub const F_HS: u32 = 4;
    pub const F_WB_BL: u32 = 8;
    pub const F_CONTRAST: u32 = 16;
    pub const F_CLARITY: u32 = 32;
    pub const F_DEHAZE: u32 = 64;
    pub const F_CURVES: u32 = 128;
    pub const F_COLOR: u32 = 256;
    pub const F_HSL: u32 = 512;
    pub const F_GRADE: u32 = 1024;
}

/// Entries per curve channel table.
pub const CURVE_N: usize = 1024;
pub const MAX_LUTS: usize = 4;
/// Long edge of the low-resolution grid used for neighbourhood layers.
pub const PROXY_LONG: u32 = 512;
/// Long edge of the grid on which AI masks are guided-filtered.
pub const MASK_LONG: u32 = 512;
pub const HSL_CENTERS: [f32; 8] = [29.0, 55.0, 110.0, 142.0, 195.0, 264.0, 300.0, 330.0];

pub struct LutUse {
    pub lut: Lut3d,
    pub amount: f32,
}

pub struct Plan {
    pub src_w: u32,
    pub src_h: u32,
    pub geo: Geo,
    pub proxy: (u32, u32),
    pub ops: Vec<[f32; REC]>,
    /// Op indices that need neighbourhood layers (in order).
    pub layer_ops: Vec<usize>,
    /// Concatenated curve tables, 3 x CURVE_N per op that has curves.
    pub curves: Vec<f32>,
    /// Concatenated (a, b) guided-filter planes of AI masks.
    pub planes: Vec<f32>,
    pub luts: Vec<LutUse>,
    pub sharpen: f32,
}

impl Plan {
    pub fn out_size(&self) -> (u32, u32) {
        (self.geo.ow, self.geo.oh)
    }
    pub fn proxy_geo(&self) -> Geo {
        self.geo.for_grid(self.proxy.0, self.proxy.1)
    }
    pub fn layer_len(&self) -> usize {
        (self.proxy.0 * self.proxy.1) as usize
    }
}

// ---------------------------------------------------------------- curves

fn monotone_tangents(pts: &[[f32; 2]]) -> Vec<f32> {
    let n = pts.len();
    let mut d = vec![0.0f32; n - 1];
    for k in 0..n - 1 {
        d[k] = (pts[k + 1][1] - pts[k][1]) / (pts[k + 1][0] - pts[k][0]);
    }
    let mut m = vec![0.0f32; n];
    m[0] = d[0];
    m[n - 1] = d[n - 2];
    for k in 1..n - 1 {
        m[k] = if d[k - 1] * d[k] <= 0.0 {
            0.0
        } else {
            (d[k - 1] + d[k]) / 2.0
        };
    }
    // Fritsch-Carlson limiter.
    for k in 0..n - 1 {
        if d[k] == 0.0 {
            m[k] = 0.0;
            m[k + 1] = 0.0;
        } else {
            let a = m[k] / d[k];
            let b = m[k + 1] / d[k];
            let s = a * a + b * b;
            if s > 9.0 {
                let t = 3.0 / s.sqrt();
                m[k] = t * a * d[k];
                m[k + 1] = t * b * d[k];
            }
        }
    }
    m
}

/// Clean control points: clamp to 0..1, sort, drop duplicate x.
fn clean_points(pts: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let mut v: Vec<[f32; 2]> = pts
        .iter()
        .filter(|p| p[0].is_finite() && p[1].is_finite())
        .map(|p| [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)])
        .collect();
    v.sort_by(|a, b| a[0].total_cmp(&b[0]));
    v.dedup_by(|b, a| (b[0] - a[0]).abs() < 1e-5);
    v
}

fn eval_clean(pts: &[[f32; 2]], m: &[f32], x: f32) -> f32 {
    if pts.len() < 2 {
        return x;
    }
    if x <= pts[0][0] {
        return pts[0][1];
    }
    let n = pts.len();
    if x >= pts[n - 1][0] {
        return pts[n - 1][1];
    }
    let mut k = 0;
    while k + 2 < n && x > pts[k + 1][0] {
        k += 1;
    }
    let h = pts[k + 1][0] - pts[k][0];
    let t = (x - pts[k][0]) / h;
    let (t2, t3) = (t * t, t * t * t);
    let y = (2.0 * t3 - 3.0 * t2 + 1.0) * pts[k][1]
        + (t3 - 2.0 * t2 + t) * h * m[k]
        + (-2.0 * t3 + 3.0 * t2) * pts[k + 1][1]
        + (t3 - t2) * h * m[k + 1];
    y.clamp(0.0, 1.0)
}

/// Per-channel tables (r, g, b) of the composed curve (channel curve after the rgb curve),
/// or `None` when the curves are identity.
pub fn curve_tables(c: &Curves) -> Option<Vec<f32>> {
    let prep = |p: &[[f32; 2]]| {
        let v = clean_points(p);
        if v.len() < 2 {
            None
        } else {
            let m = monotone_tangents(&v);
            Some((v, m))
        }
    };
    let rgb = prep(&c.rgb);
    let ch = [prep(&c.r), prep(&c.g), prep(&c.b)];
    let mut out = vec![0.0f32; 3 * CURVE_N];
    let mut identity = true;
    for (k, chk) in ch.iter().enumerate() {
        for i in 0..CURVE_N {
            let x = i as f32 / (CURVE_N - 1) as f32;
            let mut y = x;
            if let Some((p, m)) = &rgb {
                y = eval_clean(p, m, y);
            }
            if let Some((p, m)) = chk {
                y = eval_clean(p, m, y);
            }
            if (y - x).abs() > 1e-4 {
                identity = false;
            }
            out[k * CURVE_N + i] = y;
        }
    }
    if identity {
        None
    } else {
        Some(out)
    }
}

// ---------------------------------------------------------------- op records

fn is_neutral(a: &Adjust) -> bool {
    adjust_flags(a) == 0
}

fn adjust_flags(a: &Adjust) -> u32 {
    let mut f = 0;
    if a.temp != 0.0 || a.tint != 0.0 {
        f |= r::F_WB;
    }
    if a.exposure != 0.0 {
        f |= r::F_EV;
    }
    if a.shadows != 0.0 || a.highlights != 0.0 {
        f |= r::F_HS;
    }
    if a.whites != 0.0 || a.blacks != 0.0 {
        f |= r::F_WB_BL;
    }
    if a.contrast != 0.0 {
        f |= r::F_CONTRAST;
    }
    if a.clarity != 0.0 {
        f |= r::F_CLARITY;
    }
    if a.dehaze != 0.0 {
        f |= r::F_DEHAZE;
    }
    if a.curve.as_ref().and_then(curve_tables).is_some() {
        f |= r::F_CURVES;
    }
    let hsl = a
        .hsl
        .values()
        .any(|h| h.h != 0.0 || h.s != 0.0 || h.l != 0.0);
    let grade = a
        .grading
        .as_ref()
        .is_some_and(|g| g.shadows[1] != 0.0 || g.midtones[1] != 0.0 || g.highlights[1] != 0.0);
    if a.vibrance != 0.0 || a.saturation != 0.0 {
        f |= r::F_COLOR;
    }
    if hsl {
        f |= r::F_HSL;
    }
    if grade {
        f |= r::F_GRADE;
    }
    f
}

fn needs_layers(flags: u32) -> bool {
    flags & (r::F_HS | r::F_CLARITY | r::F_DEHAZE) != 0
}

/// Build the op record for an Adjust (kind/mask fields left for the caller).
fn adjust_record(a: &Adjust, curves: &mut Vec<f32>) -> [f32; REC] {
    let mut rec = [0.0f32; REC];
    let flags = adjust_flags(a);
    rec[r::FLAGS] = flags as f32;
    rec[r::AMOUNT] = 1.0;
    let m = wb_matrix(a.temp.clamp(-3000.0, 3000.0), a.tint.clamp(-100.0, 100.0));
    rec[r::WB..r::WB + 9].copy_from_slice(&m);
    let ev = a.exposure.clamp(-5.0, 5.0);
    rec[r::GAIN] = ev.exp2();
    rec[r::EV] = ev;
    rec[r::KNEE] = 1.0 - 0.25 * ev.clamp(0.0, 1.0);
    rec[r::SH] = a.shadows.clamp(-100.0, 100.0) / 100.0 * 1.2;
    rec[r::HI] = a.highlights.clamp(-100.0, 100.0) / 100.0 * 1.5;
    rec[r::WHITES] = a.whites.clamp(-100.0, 100.0) / 100.0 * 0.8;
    rec[r::BLACKS] = a.blacks.clamp(-100.0, 100.0) / 100.0 * 0.05;
    let c = a.contrast.clamp(-100.0, 100.0) / 100.0;
    rec[r::CONTRAST] = if c >= 0.0 {
        1.0 + 0.8 * c
    } else {
        1.0 / (1.0 + 0.8 * -c)
    };
    rec[r::CLARITY] = a.clarity.clamp(-100.0, 100.0) / 100.0 * 0.8;
    let d = a.dehaze.clamp(-100.0, 100.0) / 100.0;
    rec[r::DEHAZE_W] = d.max(0.0) * 0.8;
    rec[r::DEHAZE_S] = (-d).max(0.0) * 0.3;
    rec[r::VIBRANCE] = a.vibrance.clamp(-100.0, 100.0) / 100.0;
    rec[r::SAT] = a.saturation.clamp(-100.0, 100.0) / 100.0;
    if flags & r::F_CURVES != 0 {
        if let Some(t) = a.curve.as_ref().and_then(curve_tables) {
            rec[r::CURVE_OFF] = curves.len() as f32;
            curves.extend_from_slice(&t);
        }
    }
    for (band, h) in &a.hsl {
        let i = *band as usize;
        rec[r::HSL + i * 3] = h.h.clamp(-100.0, 100.0) / 100.0;
        rec[r::HSL + i * 3 + 1] = h.s.clamp(-100.0, 100.0) / 100.0;
        rec[r::HSL + i * 3 + 2] = h.l.clamp(-100.0, 100.0) / 100.0;
    }
    let g = a.grading.clone().unwrap_or_default();
    rec[r::PIVOT] = 0.5 - 0.2 * g.balance.clamp(-100.0, 100.0) / 100.0;
    for (i, w) in [g.shadows, g.midtones, g.highlights].iter().enumerate() {
        let hue = w[0].to_radians();
        let amt = w[1].clamp(0.0, 1.0) * 0.07;
        rec[r::GRADE + i * 2] = hue.cos() * amt;
        rec[r::GRADE + i * 2 + 1] = hue.sin() * amt;
    }
    rec
}

// ---------------------------------------------------------------- AI mask planes

fn box_mean(src: &[f32], w: usize, h: usize, rad: usize) -> Vec<f32> {
    // Separable clipped-window mean via f64 prefix sums.
    let mut tmp = vec![0.0f32; w * h];
    let mut pre = vec![0.0f64; w.max(h) + 1];
    for y in 0..h {
        for x in 0..w {
            pre[x + 1] = pre[x] + src[y * w + x] as f64;
        }
        for x in 0..w {
            let lo = x.saturating_sub(rad);
            let hi = (x + rad + 1).min(w);
            tmp[y * w + x] = ((pre[hi] - pre[lo]) / (hi - lo) as f64) as f32;
        }
    }
    let mut out = vec![0.0f32; w * h];
    for x in 0..w {
        for y in 0..h {
            pre[y + 1] = pre[y] + tmp[y * w + x] as f64;
        }
        for y in 0..h {
            let lo = y.saturating_sub(rad);
            let hi = (y + rad + 1).min(h);
            out[y * w + x] = ((pre[hi] - pre[lo]) / (hi - lo) as f64) as f32;
        }
    }
    out
}

/// Guided-filter planes (a, b) such that `mask = clamp(a * guide + b)` at any resolution.
fn guided_planes(p: &[f32], guide: &[f32], w: usize, h: usize) -> Vec<f32> {
    let rad = (w.max(h) / 64).max(2);
    let eps = 2e-3f32;
    let ii: Vec<f32> = guide.iter().map(|v| v * v).collect();
    let ip: Vec<f32> = guide.iter().zip(p).map(|(a, b)| a * b).collect();
    let mi = box_mean(guide, w, h, rad);
    let mp = box_mean(p, w, h, rad);
    let mii = box_mean(&ii, w, h, rad);
    let mip = box_mean(&ip, w, h, rad);
    let n = w * h;
    let mut a = vec![0.0f32; n];
    let mut b = vec![0.0f32; n];
    for i in 0..n {
        let var = mii[i] - mi[i] * mi[i];
        let cov = mip[i] - mi[i] * mp[i];
        a[i] = cov / (var.max(0.0) + eps);
        b[i] = mp[i] - a[i] * mi[i];
    }
    let ma = box_mean(&a, w, h, rad);
    let mb = box_mean(&b, w, h, rad);
    let mut out = vec![0.0f32; n * 2];
    for i in 0..n {
        out[i * 2] = ma[i];
        out[i * 2 + 1] = mb[i];
    }
    out
}

/// Sample an 8-bit mask bilinearly at normalised coordinates.
fn mask_at(m: &Mask, u: f32, v: f32) -> f32 {
    let x = u * m.width as f32 - 0.5;
    let y = v * m.height as f32 - 0.5;
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let px = |xi: i32, yi: i32| {
        let xi = xi.clamp(0, m.width as i32 - 1) as usize;
        let yi = yi.clamp(0, m.height as i32 - 1) as usize;
        m.data[yi * m.width as usize + xi] as f32 / 255.0
    };
    let (xi, yi) = (x0 as i32, y0 as i32);
    let t = px(xi, yi) * (1.0 - fx) + px(xi + 1, yi) * fx;
    let b = px(xi, yi + 1) * (1.0 - fx) + px(xi + 1, yi + 1) * fx;
    t * (1.0 - fy) + b * fy
}

struct MaskGrid {
    geo: Geo,
    guide: Vec<f32>,
}

fn mask_grid(src: &RgbImage, geo: &Geo) -> MaskGrid {
    use rayon::prelude::*;
    let long = geo.ow.max(geo.oh);
    let ml = long.min(MASK_LONG);
    let mw = ((geo.ow as f64 * ml as f64 / long as f64).round() as u32).max(1);
    let mh = ((geo.oh as f64 * ml as f64 / long as f64).round() as u32).max(1);
    let g = geo.for_grid(mw, mh);
    let mut guide = vec![0.0f32; (mw * mh) as usize];
    guide
        .par_chunks_mut(mw as usize)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                *o = enc_guide(geom::sample(src, &g, x as u32, y as u32));
            }
        });
    MaskGrid { geo: g, guide }
}

fn build_ai_planes(src: &RgbImage, grid: &MaskGrid, mask: &Mask) -> Vec<f32> {
    let g = &grid.geo;
    let (w, h) = (g.ow as usize, g.oh as usize);
    let mut p = vec![0.0f32; w * h];
    {
        use rayon::prelude::*;
        p.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                let (sx, sy) = g.map(x as f32 + 0.5, y as f32 + 0.5);
                *o = mask_at(mask, sx / src.width as f32, sy / src.height as f32);
            }
        });
    }
    guided_planes(&p, &grid.guide, w, h)
}

// ---------------------------------------------------------------- plan

pub fn build(req: &RenderRequest<'_>) -> Result<Plan> {
    let src = req.source;
    anyhow::ensure!(
        src.width > 0 && src.height > 0 && src.data.len() == (src.width * src.height * 3) as usize,
        "invalid source image"
    );
    let (sw, sh) = (src.width, src.height);
    // Geometry.
    let mut rect = [0.0f64, 0.0, 1.0, 1.0];
    let mut angle = 0.0f64;
    if let Some(Op::Crop(c)) = req.stack.ops.iter().find(|o| matches!(o, Op::Crop(_))) {
        let x = c.rect[0].clamp(0.0, 0.999) as f64;
        let y = c.rect[1].clamp(0.0, 0.999) as f64;
        let w = (c.rect[2] as f64).clamp(1.0 / sw as f64, 1.0);
        let h = (c.rect[3] as f64).clamp(1.0 / sh as f64, 1.0);
        rect = [x, y, w, h];
        angle = c.angle.clamp(-45.0, 45.0) as f64;
    }
    let mut ow = ((rect[2] * sw as f64).round() as u32).max(1);
    let mut oh = ((rect[3] * sh as f64).round() as u32).max(1);
    if let Some(max) = req.max_long_edge {
        let max = max.max(1);
        let long = ow.max(oh);
        if long > max {
            let f = max as f64 / long as f64;
            ow = ((ow as f64 * f).round() as u32).max(1);
            oh = ((oh as f64 * f).round() as u32).max(1);
        }
    }
    let geo = Geo::new(sw, sh, rect, angle, ow, oh);
    let long = ow.max(oh);
    let pl = long.min(PROXY_LONG);
    let proxy = (
        ((ow as f64 * pl as f64 / long as f64).round() as u32).max(1),
        ((oh as f64 * pl as f64 / long as f64).round() as u32).max(1),
    );

    let mut plan = Plan {
        src_w: sw,
        src_h: sh,
        geo,
        proxy,
        ops: vec![],
        layer_ops: vec![],
        curves: vec![],
        planes: vec![],
        luts: vec![],
        sharpen: 0.0,
    };
    let mut grid: Option<MaskGrid> = None;
    let mut layer_count = 0usize;

    for op in &req.stack.ops {
        match op {
            Op::Global(a) => {
                if is_neutral(a) {
                    continue;
                }
                let mut rec = adjust_record(a, &mut plan.curves);
                rec[r::KIND] = r::KIND_GLOBAL;
                push_op(&mut plan, rec, &mut layer_count);
            }
            Op::Local(l) => {
                if is_neutral(&l.adjust) || l.amount <= 0.0 {
                    continue;
                }
                let mut rec = adjust_record(&l.adjust, &mut plan.curves);
                rec[r::AMOUNT] = l.amount.clamp(0.0, 1.0);
                rec[r::INVERT] = if l.invert { 1.0 } else { 0.0 };
                match &l.mask {
                    MaskRef::Ai { target, person_id } => {
                        let Some(mask) = req.masks.mask(*target, *person_id)? else {
                            continue;
                        };
                        if mask.width == 0
                            || mask.height == 0
                            || mask.data.len() != (mask.width * mask.height) as usize
                        {
                            continue;
                        }
                        let g = grid.get_or_insert_with(|| mask_grid(src, &plan.geo));
                        let planes = build_ai_planes(src, g, &mask);
                        rec[r::KIND] = r::KIND_AI;
                        rec[r::M0] = plan.planes.len() as f32;
                        rec[r::M0 + 1] = g.geo.ow as f32;
                        rec[r::M0 + 2] = g.geo.oh as f32;
                        plan.planes.extend_from_slice(&planes);
                    }
                    MaskRef::Radial {
                        center,
                        radius,
                        feather,
                    } => {
                        rec[r::KIND] = r::KIND_RADIAL;
                        rec[r::M0] = center[0];
                        rec[r::M0 + 1] = center[1];
                        rec[r::M0 + 2] = radius[0].max(1e-4);
                        rec[r::M0 + 3] = radius[1].max(1e-4);
                        rec[r::M0 + 4] = feather.clamp(0.01, 1.0);
                    }
                    MaskRef::Linear { start, end } => {
                        rec[r::KIND] = r::KIND_LINEAR;
                        rec[r::M0] = start[0];
                        rec[r::M0 + 1] = start[1];
                        rec[r::M0 + 2] = end[0];
                        rec[r::M0 + 3] = end[1];
                    }
                }
                push_op(&mut plan, rec, &mut layer_count);
            }
            Op::Lut(l) => {
                if plan.luts.len() >= MAX_LUTS || l.amount <= 0.0 {
                    continue;
                }
                let lut = match req.luts.lut(&l.file)? {
                    Some(x) => Some(x),
                    None => builtin_lut(&l.file),
                };
                if let Some(lut) = lut {
                    if lut.size >= 2 && lut.data.len() == (lut.size as usize).pow(3) {
                        plan.luts.push(LutUse {
                            lut,
                            amount: l.amount.clamp(0.0, 1.0),
                        });
                    }
                }
            }
            Op::OutputSharpen(s) => plan.sharpen = s.amount.clamp(0.0, 100.0),
            Op::Crop(_) | Op::Unknown => {}
        }
    }
    Ok(plan)
}

fn push_op(plan: &mut Plan, mut rec: [f32; REC], layer_count: &mut usize) {
    let flags = rec[r::FLAGS] as u32;
    if needs_layers(flags) {
        rec[r::LAYER_OFF] = (*layer_count * plan.layer_len()) as f32;
        plan.layer_ops.push(plan.ops.len());
        *layer_count += 1;
    }
    plan.ops.push(rec);
}
