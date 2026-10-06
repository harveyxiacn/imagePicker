//! 64-bit DCT perceptual hash, bit-exact port of `ai-worker/imagepicker_ai/steps/phash.py`:
//! gray -> area-resample to 32x32 -> 2-D DCT-II -> 8x8 low-frequency block ->
//! `bit = coef > median(63 non-DC coefs)`, packed row-major MSB first.

use crate::imgops::{resize_area, resize_area_emulated, rgb_to_gray, Gray8};

const DCT_SIZE: usize = 32;
const HASH_SIZE: usize = 8;

/// Orthonormal 1-D DCT-II basis, `basis[k][n]` (OpenCV `cv::dct` scaling).
fn basis() -> &'static [[f64; DCT_SIZE]; DCT_SIZE] {
    use std::sync::OnceLock;
    static B: OnceLock<[[f64; DCT_SIZE]; DCT_SIZE]> = OnceLock::new();
    B.get_or_init(|| {
        let mut b = [[0f64; DCT_SIZE]; DCT_SIZE];
        let n = DCT_SIZE as f64;
        for (k, row) in b.iter_mut().enumerate() {
            let a = if k == 0 {
                (1.0 / n).sqrt()
            } else {
                (2.0 / n).sqrt()
            };
            for (i, v) in row.iter_mut().enumerate() {
                *v = a
                    * (std::f64::consts::PI * (2.0 * i as f64 + 1.0) * k as f64 / (2.0 * n)).cos();
            }
        }
        b
    })
}

/// Low-frequency 8x8 block of the 2-D DCT of a 32x32 image, as float32 (row-major).
fn dct_low(small: &Gray8) -> [f32; 64] {
    let b = basis();
    // rows first, only the 8 lowest horizontal frequencies are needed
    let mut tmp = [[0f64; HASH_SIZE]; DCT_SIZE];
    for (y, trow) in tmp.iter_mut().enumerate() {
        for (u, t) in trow.iter_mut().enumerate() {
            *t = (0..DCT_SIZE)
                .map(|x| small.data[y * DCT_SIZE + x] as f64 * b[u][x])
                .sum();
        }
    }
    let mut out = [0f32; 64];
    for v in 0..HASH_SIZE {
        for u in 0..HASH_SIZE {
            let s: f64 = (0..DCT_SIZE).map(|y| tmp[y][u] * b[v][y]).sum();
            out[v * HASH_SIZE + u] = s as f32;
        }
    }
    out
}

/// The 64 hash bits (row-major) of an RGB8 image.
pub fn phash_bits(rgb: &[u8], w: usize, h: usize) -> [bool; 64] {
    let gray = rgb_to_gray(rgb, w, h);
    let small = if w >= DCT_SIZE && h >= DCT_SIZE {
        resize_area(&gray, DCT_SIZE, DCT_SIZE)
    } else {
        // OpenCV emulates INTER_AREA with a bilinear variant when an axis is enlarged
        resize_area_emulated(&gray, DCT_SIZE, DCT_SIZE)
    };
    let low = dct_low(&small);
    let mut rest: Vec<f32> = low[1..].to_vec();
    rest.sort_by(|a, b| a.total_cmp(b));
    let med = rest[31]; // median of 63 values
    let mut bits = [false; 64];
    for (b, &c) in bits.iter_mut().zip(low.iter()) {
        *b = c > med;
    }
    bits
}

/// The hash as a `u64` (bit 63 = first coefficient).
pub fn phash_u64(rgb: &[u8], w: usize, h: usize) -> u64 {
    phash_bits(rgb, w, h)
        .iter()
        .fold(0u64, |v, &b| (v << 1) | b as u64)
}

/// 16 lowercase hex chars, identical to the worker's `phash_hex`.
pub fn phash_hex(rgb: &[u8], w: usize, h: usize) -> String {
    format!("{:016x}", phash_u64(rgb, w, h))
}

/// Hamming distance of two hex hashes (`None` if either is not hex).
pub fn hamming_hex(a: &str, b: &str) -> Option<u32> {
    Some((u64::from_str_radix(a, 16).ok()? ^ u64::from_str_radix(b, 16).ok()?).count_ones())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(w: usize, h: usize) -> Vec<u8> {
        let mut v = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f64 / w as f64, y as f64 / h as f64);
                let l = 128.0 + 80.0 * (fx * 9.0).sin() * (fy * 7.0 + 0.5).cos() + 30.0 * fx;
                let l = l.clamp(0.0, 255.0) as u8;
                v.extend_from_slice(&[l, l / 2 + 40, 255 - l]);
            }
        }
        v
    }

    #[test]
    fn deterministic_and_hex_shaped() {
        let img = pattern(128, 96);
        let a = phash_hex(&img, 128, 96);
        assert_eq!(a.len(), 16);
        assert_eq!(a, phash_hex(&img, 128, 96));
    }

    #[test]
    fn resize_changes_few_bits() {
        let big = pattern(256, 192);
        let small = pattern(128, 96);
        let d = hamming_hex(&phash_hex(&big, 256, 192), &phash_hex(&small, 128, 96)).unwrap();
        assert!(d <= 6, "distance {d}");
    }

    #[test]
    fn different_pictures_differ() {
        let a = pattern(128, 96);
        let mut c = vec![0u8; 128 * 96 * 3];
        for y in 0..96 {
            for x in 0..128 {
                let v = if (x / 16 + y / 16) % 2 == 0 { 220 } else { 30 };
                c[(y * 128 + x) * 3..][..3].copy_from_slice(&[v, v, v]);
            }
        }
        let d = hamming_hex(&phash_hex(&a, 128, 96), &phash_hex(&c, 128, 96)).unwrap();
        assert!(d > 10, "distance {d}");
    }

    #[test]
    fn tiny_images_do_not_panic() {
        let img = vec![100u8; 10 * 10 * 3];
        assert_eq!(phash_hex(&img, 10, 10).len(), 16);
    }
}
