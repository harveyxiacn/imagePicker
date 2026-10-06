//! f16 vectors (stored as BLOBs) and a minimal `.npy` reader/writer for the worker's artifacts.

use crate::error::{CoreError, Result};

pub fn f16_to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) & 1) as u32;
    let exp = ((h >> 10) & 0x1f) as u32;
    let frac = (h & 0x3ff) as u32;
    let bits = if exp == 0 {
        if frac == 0 {
            sign << 31
        } else {
            // subnormal: normalise
            let mut e = 127 - 15 + 1;
            let mut f = frac;
            while f & 0x400 == 0 {
                f <<= 1;
                e -= 1;
            }
            (sign << 31) | ((e as u32) << 23) | ((f & 0x3ff) << 13)
        }
    } else if exp == 0x1f {
        (sign << 31) | (0xff << 23) | (frac << 13)
    } else {
        (sign << 31) | ((exp + 127 - 15) << 23) | (frac << 13)
    };
    f32::from_bits(bits)
}

pub fn f32_to_f16(f: f32) -> u16 {
    let bits = f.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let frac = bits & 0x7f_ffff;
    if exp == 0xff {
        return sign | 0x7c00 | if frac != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00; // overflow -> inf
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = frac | 0x80_0000;
        let shift = (14 - e) as u32;
        let half = (m >> shift) as u16;
        let rem = m & ((1 << shift) - 1);
        let halfway = 1 << (shift - 1);
        let round = (rem > halfway || (rem == halfway && half & 1 == 1)) as u16;
        return sign | (half + round);
    }
    let half = sign | ((e as u16) << 10) | (frac >> 13) as u16;
    let rem = frac & 0x1fff;
    let round = (rem > 0x1000 || (rem == 0x1000 && half & 1 == 1)) as u16;
    half + round
}

pub fn encode_f16(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 2);
    for x in v {
        out.extend_from_slice(&f32_to_f16(*x).to_le_bytes());
    }
    out
}

#[allow(clippy::chunks_exact_to_as_chunks)]
pub fn decode_f16(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(2)
        .map(|c| f16_to_f32(u16::from_le_bytes([c[0], c[1]])))
        .collect()
}

pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

pub fn norm(a: &[f32]) -> f32 {
    dot(a, a).sqrt()
}

pub fn normalize(v: &mut [f32]) {
    let n = norm(v);
    if n > 1e-12 {
        v.iter_mut().for_each(|x| *x /= n);
    }
}

/// Cosine similarity (0 when either vector is empty / zero or the lengths differ).
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (na, nb) = (norm(a), norm(b));
    if na < 1e-12 || nb < 1e-12 {
        return 0.0;
    }
    dot(a, b) / (na * nb)
}

/// A parsed `.npy` array flattened to f32 plus its shape.
#[derive(Debug, Clone, PartialEq)]
pub struct Npy {
    pub shape: Vec<usize>,
    pub data: Vec<f32>,
}

impl Npy {
    /// Rows of a 2-D array (a 1-D array is a single row).
    pub fn rows(&self) -> Vec<&[f32]> {
        let cols = *self.shape.last().unwrap_or(&0);
        if cols == 0 {
            return Vec::new();
        }
        self.data.chunks_exact(cols).collect()
    }
}

fn bad(msg: &str) -> CoreError {
    CoreError::Internal(anyhow::anyhow!("invalid .npy: {msg}"))
}

/// Parses `.npy` v1-v3 holding little-endian float16 or float32, C order.
#[allow(clippy::chunks_exact_to_as_chunks)]
pub fn parse_npy(bytes: &[u8]) -> Result<Npy> {
    if bytes.len() < 10 || &bytes[..6] != b"\x93NUMPY" {
        return Err(bad("bad magic"));
    }
    let major = bytes[6];
    let (hlen, start) = match major {
        1 => (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10),
        2 | 3 => {
            if bytes.len() < 12 {
                return Err(bad("short header"));
            }
            (
                u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
                12,
            )
        }
        _ => return Err(bad("unsupported version")),
    };
    let end = start + hlen;
    if bytes.len() < end {
        return Err(bad("truncated header"));
    }
    let header = std::str::from_utf8(&bytes[start..end]).map_err(|_| bad("header not utf-8"))?;
    let descr = header
        .split("'descr'")
        .nth(1)
        .and_then(|s| s.split('\'').nth(1))
        .ok_or_else(|| bad("no descr"))?;
    if header.contains("'fortran_order': True") {
        return Err(bad("fortran order unsupported"));
    }
    let shape_txt = header
        .split("'shape'")
        .nth(1)
        .and_then(|s| s.split('(').nth(1))
        .and_then(|s| s.split(')').next())
        .ok_or_else(|| bad("no shape"))?;
    let shape: Vec<usize> = shape_txt
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<usize>().map_err(|_| bad("bad shape")))
        .collect::<Result<_>>()?;
    let count: usize = shape.iter().product();
    let body = &bytes[end..];
    let data: Vec<f32> = match descr {
        "<f2" | "|f2" => {
            if body.len() < count * 2 {
                return Err(bad("truncated data"));
            }
            decode_f16(&body[..count * 2])
        }
        "<f4" => {
            if body.len() < count * 4 {
                return Err(bad("truncated data"));
            }
            body[..count * 4]
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect()
        }
        other => return Err(bad(&format!("unsupported dtype {other}"))),
    };
    Ok(Npy { shape, data })
}

/// Writes a float16 `.npy` (v1) — used by fakes and tests.
pub fn write_npy_f16(shape: &[usize], data: &[f32]) -> Vec<u8> {
    let shape_txt = match shape.len() {
        1 => format!("({},)", shape[0]),
        _ => format!(
            "({})",
            shape
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    let mut header = format!("{{'descr': '<f2', 'fortran_order': False, 'shape': {shape_txt}, }}");
    // total (10 + header + '\n') must be a multiple of 64
    let total = 10 + header.len() + 1;
    header.push_str(&" ".repeat((64 - total % 64) % 64));
    header.push('\n');
    let mut out = Vec::new();
    out.extend_from_slice(b"\x93NUMPY\x01\x00");
    out.extend_from_slice(&(header.len() as u16).to_le_bytes());
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(&encode_f16(data));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f16_roundtrip() {
        for x in [
            0.0f32,
            1.0,
            -1.0,
            0.5,
            0.333_251_95,
            65504.0,
            6.1e-5,
            -2.5e-7,
        ] {
            let h = f32_to_f16(x);
            let y = f16_to_f32(h);
            assert!((x - y).abs() <= x.abs() * 1e-3 + 1e-7, "{x} -> {y}");
        }
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f32_to_f16(1.0), 0x3c00);
        assert!(f16_to_f32(0x7c00).is_infinite());
        // idempotent after the first quantisation
        let h = f32_to_f16(0.1234);
        assert_eq!(f32_to_f16(f16_to_f32(h)), h);
    }

    #[test]
    fn npy_roundtrip_1d_and_2d() {
        let v: Vec<f32> = (0..12).map(|i| i as f32 * 0.25).collect();
        let n = parse_npy(&write_npy_f16(&[12], &v)).unwrap();
        assert_eq!(n.shape, vec![12]);
        assert_eq!(n.data, v);
        let n = parse_npy(&write_npy_f16(&[3, 4], &v)).unwrap();
        assert_eq!(n.shape, vec![3, 4]);
        assert_eq!(n.rows().len(), 3);
        assert_eq!(n.rows()[1], &v[4..8]);
        assert!(parse_npy(b"junk").is_err());
    }

    #[test]
    fn cosine_basics() {
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert_eq!(cosine(&[1.0], &[1.0, 2.0]), 0.0);
    }
}
