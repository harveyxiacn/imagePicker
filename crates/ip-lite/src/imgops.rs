//! Small image primitives that reproduce the OpenCV behaviour the worker's classic metrics rely
//! on (`cvtColor` RGB2GRAY, `resize` INTER_AREA / INTER_LINEAR, `GaussianBlur`, `Sobel`,
//! `Laplacian`, `filter2D`, all with `BORDER_REFLECT_101`).

/// A single-channel float image.
#[derive(Clone)]
pub struct Gray32 {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

/// A single-channel 8-bit image.
#[derive(Clone)]
pub struct Gray8 {
    pub w: usize,
    pub h: usize,
    pub data: Vec<u8>,
}

impl Gray8 {
    pub fn to_f32(&self) -> Gray32 {
        Gray32 {
            w: self.w,
            h: self.h,
            data: self.data.iter().map(|&v| v as f32).collect(),
        }
    }

    /// Sub-rectangle `[y0, y1) x [x0, x1)`.
    pub fn crop(&self, x0: usize, y0: usize, x1: usize, y1: usize) -> Gray8 {
        let (w, h) = (x1 - x0, y1 - y0);
        let mut data = Vec::with_capacity(w * h);
        for y in y0..y1 {
            data.extend_from_slice(&self.data[y * self.w + x0..y * self.w + x1]);
        }
        Gray8 { w, h, data }
    }
}

impl Gray32 {
    pub fn crop(&self, x0: usize, y0: usize, x1: usize, y1: usize) -> Gray32 {
        let (w, h) = (x1 - x0, y1 - y0);
        let mut data = Vec::with_capacity(w * h);
        for y in y0..y1 {
            data.extend_from_slice(&self.data[y * self.w + x0..y * self.w + x1]);
        }
        Gray32 { w, h, data }
    }
}

/// `cv2.cvtColor(rgb, COLOR_RGB2GRAY)` for 8-bit input: `(4899 R + 9617 G + 1868 B + 8192) >> 14`.
pub fn rgb_to_gray(rgb: &[u8], w: usize, h: usize) -> Gray8 {
    assert_eq!(rgb.len(), w * h * 3, "rgb buffer size");
    let data = rgb
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| {
            ((p[0] as u32 * 4899 + p[1] as u32 * 9617 + p[2] as u32 * 1868 + 8192) >> 14) as u8
        })
        .collect();
    Gray8 { w, h, data }
}

/// OpenCV `cvRound` (round half to even) saturated to `u8`.
fn round_u8(v: f32) -> u8 {
    let r = v.round_ties_even();
    r.clamp(0.0, 255.0) as u8
}

/// Per-axis tap table of `cv::resize(INTER_AREA)`: (source index, weight) per destination index.
fn area_taps(src: usize, dst: usize) -> Vec<Vec<(usize, f32)>> {
    let scale = src as f64 / dst as f64;
    (0..dst)
        .map(|d| {
            let fs1 = d as f64 * scale;
            let fs2 = fs1 + scale;
            let cell = scale.min(src as f64 - fs1);
            let mut s1 = fs1.ceil() as i64;
            let mut s2 = fs2.floor() as i64;
            s2 = s2.min(src as i64 - 1);
            s1 = s1.min(s2);
            let mut taps = Vec::new();
            if s1 as f64 - fs1 > 1e-3 {
                taps.push(((s1 - 1) as usize, ((s1 as f64 - fs1) / cell) as f32));
            }
            for s in s1..s2 {
                taps.push((s as usize, (1.0 / cell) as f32));
            }
            if fs2 - s2 as f64 > 1e-3 {
                taps.push((
                    s2 as usize,
                    ((fs2 - s2 as f64).min(1.0).min(cell) / cell) as f32,
                ));
            }
            taps
        })
        .collect()
}

/// `cv2.resize(gray, (dw, dh), interpolation=INTER_AREA)` for downscaling (`dw <= w`, `dh <= h`);
/// integer ratios use OpenCV's integer box filter, others the fractional-area weights.
pub fn resize_area(src: &Gray8, dw: usize, dh: usize) -> Gray8 {
    let (w, h) = (src.w, src.h);
    assert!(
        dw >= 1 && dh >= 1 && dw <= w && dh <= h,
        "resize_area only downsamples"
    );
    let mut out = vec![0u8; dw * dh];
    if w % dw == 0 && h % dh == 0 {
        let (kx, ky) = (w / dw, h / dh);
        let area = (kx * ky) as u32;
        for dy in 0..dh {
            for dx in 0..dw {
                let mut sum = 0u32;
                for y in 0..ky {
                    let row = &src.data[(dy * ky + y) * w + dx * kx..][..kx];
                    sum += row.iter().map(|&v| v as u32).sum::<u32>();
                }
                out[dy * dw + dx] = ((sum + area / 2) / area) as u8;
            }
        }
        return Gray8 {
            w: dw,
            h: dh,
            data: out,
        };
    }
    let (tx, ty) = (area_taps(w, dw), area_taps(h, dh));
    // horizontal pass into f32 rows, vertical pass accumulates (as OpenCV's row buffers do)
    let mut hrows = vec![0f32; dw * h];
    for y in 0..h {
        let row = &src.data[y * w..(y + 1) * w];
        for (dx, taps) in tx.iter().enumerate() {
            let mut s = 0f32;
            for &(sx, a) in taps {
                s += row[sx] as f32 * a;
            }
            hrows[y * dw + dx] = s;
        }
    }
    for (dy, taps) in ty.iter().enumerate() {
        for dx in 0..dw {
            let mut s = 0f32;
            for &(sy, b) in taps {
                s += hrows[sy * dw + dx] * b;
            }
            out[dy * dw + dx] = round_u8(s);
        }
    }
    Gray8 {
        w: dw,
        h: dh,
        data: out,
    }
}

/// `cv2.resize(gray, (dw, dh), interpolation=INTER_LINEAR)` for 8-bit input (11-bit fixed point,
/// as OpenCV). Used for the rare upscale of small images.
pub fn resize_linear(src: &Gray8, dw: usize, dh: usize) -> Gray8 {
    resize_bilinear_impl(src, dw, dh, false)
}

/// `cv2.resize(gray, ..., INTER_AREA)` when it cannot use true area averaging (an axis is
/// enlarged): OpenCV then emulates it with a bilinear variant whose taps follow the area grid.
pub fn resize_area_emulated(src: &Gray8, dw: usize, dh: usize) -> Gray8 {
    resize_bilinear_impl(src, dw, dh, true)
}

fn resize_bilinear_impl(src: &Gray8, dw: usize, dh: usize, area_mode: bool) -> Gray8 {
    const BITS: i32 = 11;
    const ONE: i32 = 1 << BITS;
    let taps = |ssize: usize, dsize: usize| -> Vec<(usize, usize, i32, i32)> {
        let scale = ssize as f64 / dsize as f64;
        let inv_scale = dsize as f64 / ssize as f64;
        (0..dsize)
            .map(|d| {
                let (mut s, mut f);
                if area_mode {
                    s = (d as f64 * scale).floor() as i64;
                    let t = ((d + 1) as f64 - (s + 1) as f64 * inv_scale) as f32;
                    f = if t <= 0.0 { 0.0 } else { t - t.floor() };
                } else {
                    let fx = ((d as f64 + 0.5) * scale - 0.5) as f32;
                    s = fx.floor() as i64;
                    f = fx - s as f32;
                }
                if s < 0 {
                    s = 0;
                    f = 0.0;
                }
                if s >= ssize as i64 - 1 {
                    s = ssize as i64 - 1;
                    f = 0.0;
                }
                let s1 = (s as usize + 1).min(ssize - 1);
                let a1 = (f * ONE as f32).round_ties_even() as i32;
                let a0 = ((1.0 - f) * ONE as f32).round_ties_even() as i32;
                (s as usize, s1, a0, a1)
            })
            .collect()
    };
    let (tx, ty) = (taps(src.w, dw), taps(src.h, dh));
    let mut out = vec![0u8; dw * dh];
    for (dy, &(y0, y1, b0, b1)) in ty.iter().enumerate() {
        for (dx, &(x0, x1, a0, a1)) in tx.iter().enumerate() {
            let h0 = src.data[y0 * src.w + x0] as i32 * a0 + src.data[y0 * src.w + x1] as i32 * a1;
            let h1 = src.data[y1 * src.w + x0] as i32 * a0 + src.data[y1 * src.w + x1] as i32 * a1;
            // OpenCV's SIMD vertical pass (`VResizeLinearVec_32s8u`): 16-bit mulhi per tap
            let v = (((b0 * (h0 >> 4)) >> 16) + ((b1 * (h1 >> 4)) >> 16) + 2) >> 2;
            out[dy * dw + dx] = v.clamp(0, 255) as u8;
        }
    }
    Gray8 {
        w: dw,
        h: dh,
        data: out,
    }
}

/// Index with `BORDER_REFLECT_101` (`dcb|abcd|cba`) for any distance from the image.
#[inline]
pub fn reflect101(mut i: isize, n: usize) -> usize {
    if n == 1 {
        return 0;
    }
    let period = 2 * (n as isize - 1);
    i = i.rem_euclid(period);
    if i >= n as isize {
        i = period - i;
    }
    i as usize
}

/// Image padded by `r` pixels on every side with reflect-101.
fn pad(src: &Gray32, r: usize) -> (Vec<f32>, usize) {
    let (w, h) = (src.w, src.h);
    let pw = w + 2 * r;
    let mut out = vec![0f32; pw * (h + 2 * r)];
    for py in 0..h + 2 * r {
        let sy = reflect101(py as isize - r as isize, h);
        let row = &src.data[sy * w..(sy + 1) * w];
        let dst = &mut out[py * pw..(py + 1) * pw];
        for (px, d) in dst.iter_mut().enumerate() {
            *d = row[reflect101(px as isize - r as isize, w)];
        }
    }
    (out, pw)
}

/// `cv2.filter2D` with a 3x3 kernel (correlation), reflect-101 borders, float32.
pub fn filter3x3(src: &Gray32, k: &[[f32; 3]; 3]) -> Gray32 {
    let (w, h) = (src.w, src.h);
    let (p, pw) = pad(src, 1);
    let mut data = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0f32;
            for (ky, krow) in k.iter().enumerate() {
                let base = (y + ky) * pw + x;
                for (kx, &kv) in krow.iter().enumerate() {
                    s += p[base + kx] * kv;
                }
            }
            data[y * w + x] = s;
        }
    }
    Gray32 { w, h, data }
}

pub const SOBEL_X: [[f32; 3]; 3] = [[-1.0, 0.0, 1.0], [-2.0, 0.0, 2.0], [-1.0, 0.0, 1.0]];
pub const SOBEL_Y: [[f32; 3]; 3] = [[-1.0, -2.0, -1.0], [0.0, 0.0, 0.0], [1.0, 2.0, 1.0]];
/// `cv2.Laplacian(..., ksize=1)`.
pub const LAPLACIAN1: [[f32; 3]; 3] = [[0.0, 1.0, 0.0], [1.0, -4.0, 1.0], [0.0, 1.0, 0.0]];
pub const IMMERKAER: [[f32; 3]; 3] = [[1.0, -2.0, 1.0], [-2.0, 4.0, -2.0], [1.0, -2.0, 1.0]];

/// `cv2.getGaussianKernel(ksize, sigma)` as float32.
fn gaussian_kernel(ksize: usize, sigma: f64) -> Vec<f32> {
    let c = (ksize as f64 - 1.0) * 0.5;
    let s2 = -0.5 / (sigma * sigma);
    let v: Vec<f64> = (0..ksize)
        .map(|i| {
            let d = i as f64 - c;
            (s2 * d * d).exp()
        })
        .collect();
    let sum: f64 = v.iter().sum();
    v.iter().map(|x| (x / sum) as f32).collect()
}

/// `cv2.GaussianBlur(g, (0, 0), sigma)` on float32 (kernel size derived as OpenCV does).
pub fn gaussian_blur(src: &Gray32, sigma: f64) -> Gray32 {
    let ksize = (((sigma * 6.0 + 1.0).round_ties_even() as usize) | 1).max(1);
    let k = gaussian_kernel(ksize, sigma);
    let r = ksize / 2;
    let (w, h) = (src.w, src.h);
    let (p, pw) = pad(src, r);
    // horizontal pass over the padded rows
    let ph = h + 2 * r;
    let mut tmp = vec![0f32; w * ph];
    for y in 0..ph {
        let row = &p[y * pw..(y + 1) * pw];
        for x in 0..w {
            let mut s = 0f32;
            for (i, &kv) in k.iter().enumerate() {
                s += row[x + i] * kv;
            }
            tmp[y * w + x] = s;
        }
    }
    let mut data = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0f32;
            for (i, &kv) in k.iter().enumerate() {
                s += tmp[(y + i) * w + x] * kv;
            }
            data[y * w + x] = s;
        }
    }
    Gray32 { w, h, data }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reflect_matches_opencv() {
        // n = 4: ... c b | a b c d | c b ...
        let idx: Vec<usize> = (-3..7).map(|i| reflect101(i, 4)).collect();
        assert_eq!(idx, [3, 2, 1, 0, 1, 2, 3, 2, 1, 0]);
        assert_eq!(reflect101(5, 1), 0);
    }

    #[test]
    fn gray_weights() {
        let g = rgb_to_gray(&[255, 255, 255, 0, 0, 0, 255, 0, 0], 3, 1);
        assert_eq!(g.data, [255, 0, 76]);
    }

    #[test]
    fn area_integer_box_rounds_half_up() {
        let g = Gray8 {
            w: 2,
            h: 2,
            data: vec![0, 1, 1, 1],
        }; // mean 0.75 -> 1; sum 3, area 4
        assert_eq!(resize_area(&g, 1, 1).data, [1]);
        let g = Gray8 {
            w: 2,
            h: 2,
            data: vec![0, 0, 1, 1],
        }; // mean 0.5 -> 1 (half up)
        assert_eq!(resize_area(&g, 1, 1).data, [1]);
    }

    #[test]
    fn area_fractional_constant_image_stays_constant() {
        let g = Gray8 {
            w: 50,
            h: 37,
            data: vec![123; 50 * 37],
        };
        let r = resize_area(&g, 32, 32);
        assert!(r.data.iter().all(|&v| v == 123));
    }

    #[test]
    fn gaussian_preserves_constant() {
        let g = Gray32 {
            w: 9,
            h: 7,
            data: vec![10.0; 63],
        };
        let b = gaussian_blur(&g, 0.7);
        assert!(b.data.iter().all(|v| (v - 10.0).abs() < 1e-4));
    }
}
