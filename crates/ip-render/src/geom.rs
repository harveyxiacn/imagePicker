//! Geometry stage: output pixel -> source coordinates (rotate about centre, crop rect,
//! resize) and the supersampled bilinear source sampler. Mirrored by `shaders/render.wgsl`.

use crate::color::dec_lut;
use crate::RgbImage;

/// Maximum supersampling taps per axis.
pub const MAX_TAPS: u32 = 8;

#[derive(Clone, Copy, Debug)]
pub struct Geo {
    pub sw: u32,
    pub sh: u32,
    /// Crop rect (normalised, on the rotated canvas) and angle in degrees.
    pub rect: [f64; 4],
    pub angle: f64,
    pub ow: u32,
    pub oh: u32,
    // Derived (f32, what the shader receives).
    pub cx0: f32,
    pub cy0: f32,
    pub xs: f32,
    pub ys: f32,
    pub cos: f32,
    pub sin: f32,
    pub hw: f32,
    pub hh: f32,
    pub taps: u32,
}

impl Geo {
    pub fn new(sw: u32, sh: u32, rect: [f64; 4], angle: f64, ow: u32, oh: u32) -> Geo {
        let (w, h) = (sw as f64, sh as f64);
        let th = angle.to_radians();
        let (sin, cos) = if angle == 0.0 {
            (0.0, 1.0)
        } else {
            th.sin_cos()
        };
        let xs = rect[2] * w / ow as f64;
        let ys = rect[3] * h / oh as f64;
        let scale = xs.max(ys);
        // A bilinear tap already covers ~2 source pixels, so taps = scale / 2.
        let taps = ((scale / 2.0 - 0.01).ceil().max(1.0) as u32).min(MAX_TAPS);
        Geo {
            sw,
            sh,
            rect,
            angle,
            ow,
            oh,
            cx0: (rect[0] * w) as f32,
            cy0: (rect[1] * h) as f32,
            xs: xs as f32,
            ys: ys as f32,
            cos: cos as f32,
            sin: sin as f32,
            hw: (w / 2.0) as f32,
            hh: (h / 2.0) as f32,
            taps,
        }
    }

    /// Same crop, different output grid.
    pub fn for_grid(&self, ow: u32, oh: u32) -> Geo {
        Geo::new(self.sw, self.sh, self.rect, self.angle, ow, oh)
    }

    /// Inverse of [`Geo::map`]: source pixel coordinates -> continuous output position.
    pub fn unmap(&self, sx: f32, sy: f32) -> (f32, f32) {
        let dxs = sx - self.hw;
        let dys = sy - self.hh;
        let dx = dxs * self.cos + dys * self.sin;
        let dy = -dxs * self.sin + dys * self.cos;
        (
            (self.hw + dx - self.cx0) / self.xs,
            (self.hh + dy - self.cy0) / self.ys,
        )
    }

    /// Continuous output position (pixel centres at i + 0.5) -> source pixel coordinates.
    #[inline]
    pub fn map(&self, fx: f32, fy: f32) -> (f32, f32) {
        let cx = self.cx0 + fx * self.xs;
        let cy = self.cy0 + fy * self.ys;
        let dx = cx - self.hw;
        let dy = cy - self.hh;
        (
            self.hw + dx * self.cos - dy * self.sin,
            self.hh + dx * self.sin + dy * self.cos,
        )
    }
}

#[inline]
fn texel(src: &RgbImage, x: i32, y: i32, lut: &[f32; 256]) -> [f32; 3] {
    let xi = x.clamp(0, src.width as i32 - 1) as usize;
    let yi = y.clamp(0, src.height as i32 - 1) as usize;
    let i = (yi * src.width as usize + xi) * 3;
    [
        lut[src.data[i] as usize],
        lut[src.data[i + 1] as usize],
        lut[src.data[i + 2] as usize],
    ]
}

/// Bilinear sample in linear light at source pixel coordinates (centres at i + 0.5).
#[inline]
pub fn bilinear(src: &RgbImage, sx: f32, sy: f32, lut: &[f32; 256]) -> [f32; 3] {
    let x = sx - 0.5;
    let y = sy - 0.5;
    let x0 = x.floor();
    let y0 = y.floor();
    let fx = x - x0;
    let fy = y - y0;
    let (xi, yi) = (x0 as i32, y0 as i32);
    let a = texel(src, xi, yi, lut);
    let b = texel(src, xi + 1, yi, lut);
    let c = texel(src, xi, yi + 1, lut);
    let d = texel(src, xi + 1, yi + 1, lut);
    let mut o = [0.0; 3];
    for k in 0..3 {
        let top = a[k] + (b[k] - a[k]) * fx;
        let bot = c[k] + (d[k] - c[k]) * fx;
        o[k] = top + (bot - top) * fy;
    }
    o
}

/// Supersampled (area-ish) sample of output pixel `(ox, oy)` in linear light.
pub fn sample(src: &RgbImage, g: &Geo, ox: u32, oy: u32) -> [f32; 3] {
    sample_off(src, g, ox, oy, 0.0, 0.0)
}

/// Like [`sample`], reading the source at the output position displaced by `(dx, dy)`
/// output pixels (the warp stage). A zero offset is bit-identical to [`sample`].
pub fn sample_off(src: &RgbImage, g: &Geo, ox: u32, oy: u32, dx: f32, dy: f32) -> [f32; 3] {
    let lut = dec_lut();
    let (fx, fy) = (ox as f32 + 0.5 + dx, oy as f32 + 0.5 + dy);
    let n = g.taps;
    if n == 1 {
        let (sx, sy) = g.map(fx, fy);
        return bilinear(src, sx, sy, lut);
    }
    let inv = 1.0 / n as f32;
    let mut acc = [0.0f32; 3];
    for j in 0..n {
        let oyy = fy + ((j as f32 + 0.5) * inv - 0.5);
        for i in 0..n {
            let oxx = fx + ((i as f32 + 0.5) * inv - 0.5);
            let (sx, sy) = g.map(oxx, oyy);
            let c = bilinear(src, sx, sy, lut);
            acc[0] += c[0];
            acc[1] += c[1];
            acc[2] += c[2];
        }
    }
    let k = inv * inv;
    [acc[0] * k, acc[1] * k, acc[2] * k]
}

#[cfg(test)]
mod tests {
    use super::Geo;

    #[test]
    fn unmap_inverts_map() {
        let g = Geo::new(640, 480, [0.1, 0.05, 0.8, 0.85], 7.5, 300, 250);
        for (fx, fy) in [(0.5, 0.5), (10.25, 200.5), (299.5, 3.0)] {
            let (sx, sy) = g.map(fx, fy);
            let (bx, by) = g.unmap(sx, sy);
            assert!((bx - fx).abs() < 1e-3 && (by - fy).abs() < 1e-3);
        }
    }
}
