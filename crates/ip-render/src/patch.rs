//! Patch layer compositing (M5): generated RGBA rasters (best take, inpaint, denoise,
//! face restore) are blended onto the upright source *before* crop, so straighten, warp,
//! tone and portrait ops act on the patched image.
//!
//! The composite is computed once per render on the CPU (rayon, rows in parallel) and the
//! result is handed to whichever backend renders, so CPU and GPU see identical pixels and
//! patch parity is exact by construction. Only pixels inside a patch rect are written;
//! everything else stays bit-identical to the input.

use std::collections::HashMap;

use rayon::prelude::*;

use crate::color::{dec_lut, smoothstep, srgb_enc};
use crate::geom::MAX_TAPS;
use crate::*;

/// A validated patch placed on the source pixel grid.
struct Placed<'a> {
    img: &'a RgbaImage,
    /// Rect in source pixels.
    x0: f32,
    y0: f32,
    w: f32,
    h: f32,
    amount: f32,
    /// Feather width in source pixels (0 = hard edge).
    feather: f32,
    /// Supersampling taps per axis.
    tx: u32,
    ty: u32,
}

impl Placed<'_> {
    /// Source pixel range `[x0, x1) x [y0, y1)` touched by the rect.
    fn bounds(&self, sw: u32, sh: u32) -> (usize, usize, usize, usize) {
        let lo = |v: f32, n: u32| (v.floor().max(0.0) as usize).min(n as usize);
        let hi = |v: f32, n: u32| (v.ceil().max(0.0) as usize).min(n as usize);
        (
            lo(self.x0, sw),
            lo(self.y0, sh),
            hi(self.x0 + self.w, sw),
            hi(self.y0 + self.h, sh),
        )
    }

    /// Edge weight (feather) at source position `(sx, sy)`; 0 outside the rect.
    #[inline]
    fn edge(&self, sx: f32, sy: f32) -> f32 {
        let d = (sx - self.x0)
            .min(self.x0 + self.w - sx)
            .min(sy - self.y0)
            .min(self.y0 + self.h - sy);
        if d <= 0.0 {
            0.0
        } else if self.feather > 0.0 {
            smoothstep(0.0, self.feather, d)
        } else {
            1.0
        }
    }

    /// Premultiplied linear-light RGBA texel (clamped).
    #[inline]
    fn texel(&self, x: i32, y: i32, lut: &[f32; 256]) -> [f32; 4] {
        let xi = x.clamp(0, self.img.width as i32 - 1) as usize;
        let yi = y.clamp(0, self.img.height as i32 - 1) as usize;
        let i = (yi * self.img.width as usize + xi) * 4;
        let d = &self.img.data;
        let a = d[i + 3] as f32 / 255.0;
        [
            lut[d[i] as usize] * a,
            lut[d[i + 1] as usize] * a,
            lut[d[i + 2] as usize] * a,
            a,
        ]
    }

    /// Bilinear sample at asset pixel coordinates (centres at i + 0.5), premultiplied.
    #[inline]
    fn bilinear(&self, ax: f32, ay: f32, lut: &[f32; 256]) -> [f32; 4] {
        let x = ax - 0.5;
        let y = ay - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (xi, yi) = (x0 as i32, y0 as i32);
        let a = self.texel(xi, yi, lut);
        let b = self.texel(xi + 1, yi, lut);
        let c = self.texel(xi, yi + 1, lut);
        let d = self.texel(xi + 1, yi + 1, lut);
        let mut o = [0.0; 4];
        for k in 0..4 {
            let top = a[k] + (b[k] - a[k]) * fx;
            let bot = c[k] + (d[k] - c[k]) * fx;
            o[k] = top + (bot - top) * fy;
        }
        o
    }

    /// Supersampled premultiplied sample for source pixel `(px, py)`.
    fn sample(&self, px: usize, py: usize, lut: &[f32; 256]) -> [f32; 4] {
        let (sx, sy) = (px as f32 + 0.5, py as f32 + 0.5);
        let kx = self.img.width as f32 / self.w;
        let ky = self.img.height as f32 / self.h;
        let mut acc = [0.0f32; 4];
        for j in 0..self.ty {
            let oy = sy + ((j as f32 + 0.5) / self.ty as f32 - 0.5);
            for i in 0..self.tx {
                let ox = sx + ((i as f32 + 0.5) / self.tx as f32 - 0.5);
                let c = self.bilinear((ox - self.x0) * kx, (oy - self.y0) * ky, lut);
                for k in 0..4 {
                    acc[k] += c[k];
                }
            }
        }
        let n = 1.0 / (self.tx * self.ty) as f32;
        [acc[0] * n, acc[1] * n, acc[2] * n, acc[3] * n]
    }
}

fn taps(scale: f32) -> u32 {
    // A bilinear tap already covers ~2 asset pixels (same rule as geom.rs).
    ((scale / 2.0 - 0.01).ceil().max(1.0) as u32).min(MAX_TAPS)
}

/// Composite every enabled patch of `req.stack` onto a copy of the source. Returns `None`
/// when nothing would change (no patches, disabled, assets missing or degenerate), so the
/// caller keeps using the original source without a copy.
pub fn composite(req: &RenderRequest<'_>) -> Result<Option<RgbImage>> {
    let patches: Vec<&Patch> = req
        .stack
        .ops
        .iter()
        .filter_map(|o| match o {
            Op::Patch(p) if p.enabled && p.amount > 0.0 => Some(p),
            _ => None,
        })
        .collect();
    if patches.is_empty() {
        return Ok(None);
    }
    let src = req.source;
    let (sw, sh) = (src.width, src.height);

    // Each asset is fetched once per render.
    let mut assets: HashMap<&str, Option<RgbaImage>> = HashMap::new();
    for p in &patches {
        if !assets.contains_key(p.asset.as_str()) {
            let a = req.masks.patch(&p.asset)?.filter(|a| {
                a.width > 0 && a.height > 0 && a.data.len() == (a.width * a.height * 4) as usize
            });
            assets.insert(p.asset.as_str(), a);
        }
    }

    let mut placed = Vec::new();
    for p in &patches {
        let Some(img) = assets[p.asset.as_str()].as_ref() else {
            continue;
        };
        let [rx, ry, rw, rh] = p.rect;
        if !(rx.is_finite() && ry.is_finite() && rw.is_finite() && rh.is_finite())
            || rw <= 0.0
            || rh <= 0.0
        {
            continue;
        }
        let (w, h) = (rw * sw as f32, rh * sh as f32);
        let feather = p.feather.clamp(0.0, 0.5) * w.min(h);
        placed.push(Placed {
            img,
            x0: rx * sw as f32,
            y0: ry * sh as f32,
            w,
            h,
            amount: p.amount.clamp(0.0, 1.0),
            feather,
            tx: taps(img.width as f32 / w),
            ty: taps(img.height as f32 / h),
        });
    }
    if placed.is_empty() {
        return Ok(None);
    }

    let lut = dec_lut();
    let mut out = src.clone();
    let stride = sw as usize * 3;
    for pl in &placed {
        let (bx0, by0, bx1, by1) = pl.bounds(sw, sh);
        if bx0 >= bx1 || by0 >= by1 {
            continue;
        }
        out.data
            .par_chunks_mut(stride)
            .enumerate()
            .skip(by0)
            .take(by1 - by0)
            .for_each(|(y, row)| {
                for x in bx0..bx1 {
                    let e = pl.edge(x as f32 + 0.5, y as f32 + 0.5);
                    if e <= 0.0 {
                        continue;
                    }
                    let s = pl.sample(x, y, lut);
                    let alpha = s[3] * pl.amount * e;
                    if alpha <= 0.0 {
                        continue;
                    }
                    // Un-premultiply to straight colour, blend in linear light.
                    let inv = 1.0 / s[3];
                    for k in 0..3 {
                        let dst = lut[row[x * 3 + k] as usize];
                        let v = dst + (s[k] * inv - dst) * alpha;
                        row[x * 3 + k] = (srgb_enc(v) * 255.0 + 0.5) as u8;
                    }
                }
            });
    }
    Ok(Some(out))
}
