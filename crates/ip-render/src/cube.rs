//! `.cube` 3D LUT parsing.

use crate::{Lut3d, Result};

pub fn parse_cube(text: &str) -> Result<Lut3d> {
    let mut size: Option<u32> = None;
    let mut dmin = [0.0f32; 3];
    let mut dmax = [1.0f32; 3];
    let mut data: Vec<[f32; 3]> = Vec::new();
    for (ln, raw) in text.lines().enumerate() {
        let line = raw.trim_start_matches('\u{feff}');
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut it = line.split_whitespace();
        let first = it.next().unwrap_or("");
        let upper = first.to_ascii_uppercase();
        let ctx = |m: &str| anyhow::anyhow!("cube line {}: {m}", ln + 1);
        match upper.as_str() {
            "TITLE" => continue,
            "LUT_1D_SIZE" => return Err(ctx("1D LUTs are not supported")),
            "LUT_3D_SIZE" => {
                let n: u32 = it
                    .next()
                    .ok_or_else(|| ctx("missing size"))?
                    .parse()
                    .map_err(|_| ctx("bad LUT_3D_SIZE"))?;
                if !(2..=256).contains(&n) {
                    return Err(ctx("LUT_3D_SIZE out of range (2..=256)"));
                }
                size = Some(n);
            }
            "DOMAIN_MIN" | "DOMAIN_MAX" => {
                let v = parse3(&mut it).ok_or_else(|| ctx("bad DOMAIN triple"))?;
                if upper == "DOMAIN_MIN" {
                    dmin = v;
                } else {
                    dmax = v;
                }
            }
            k if k.starts_with(|c: char| c.is_ascii_alphabetic()) => {
                // Unknown keyword (e.g. LUT_IN_VIDEO_RANGE): ignore.
                continue;
            }
            _ => {
                let mut v = line.split_whitespace();
                let t = parse3(&mut v).ok_or_else(|| ctx("expected three numbers"))?;
                if v.next().is_some() {
                    return Err(ctx("too many values"));
                }
                data.push(t);
            }
        }
    }
    let n = size.ok_or_else(|| anyhow::anyhow!("cube: missing LUT_3D_SIZE"))?;
    let expect = (n as usize).pow(3);
    anyhow::ensure!(
        data.len() == expect,
        "cube: expected {expect} entries, found {}",
        data.len()
    );
    anyhow::ensure!(
        (0..3).all(|k| dmax[k] > dmin[k]),
        "cube: DOMAIN_MAX must exceed DOMAIN_MIN"
    );
    let mut lut = Lut3d { size: n, data };
    if dmin != [0.0; 3] || dmax != [1.0; 3] {
        lut = rebase_domain(&lut, dmin, dmax);
    }
    Ok(lut)
}

fn parse3<'a>(it: &mut impl Iterator<Item = &'a str>) -> Option<[f32; 3]> {
    let mut o = [0.0f32; 3];
    for v in o.iter_mut() {
        let x: f32 = it.next()?.parse().ok()?;
        if !x.is_finite() {
            return None;
        }
        *v = x;
    }
    Some(o)
}

/// Resample a LUT with a non-default input domain onto the 0..1 domain the renderer uses.
fn rebase_domain(l: &Lut3d, dmin: [f32; 3], dmax: [f32; 3]) -> Lut3d {
    let n = l.size as usize;
    let nm = (n - 1) as f32;
    let mut out = Vec::with_capacity(n * n * n);
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let e = [r as f32 / nm, g as f32 / nm, b as f32 / nm];
                let mut t = [0.0f32; 3];
                for k in 0..3 {
                    t[k] = ((e[k] - dmin[k]) / (dmax[k] - dmin[k])).clamp(0.0, 1.0);
                }
                out.push(crate::cpu::lut_lookup(l, t));
            }
        }
    }
    Lut3d {
        size: l.size,
        data: out,
    }
}

/// Serialise a LUT to `.cube` text (used by tests and tooling).
pub fn write_cube(l: &Lut3d, title: &str) -> String {
    let mut s = format!("TITLE \"{title}\"\nLUT_3D_SIZE {}\n", l.size);
    for e in &l.data {
        s.push_str(&format!("{:.6} {:.6} {:.6}\n", e[0], e[1], e[2]));
    }
    s
}

/// Identity LUT of the given size.
pub fn identity_lut(n: u32) -> Lut3d {
    let nm = (n - 1) as f32;
    let mut data = Vec::with_capacity((n * n * n) as usize);
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                data.push([r as f32 / nm, g as f32 / nm, b as f32 / nm]);
            }
        }
    }
    Lut3d { size: n, data }
}
