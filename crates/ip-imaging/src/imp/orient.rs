//! EXIF orientation (all 8 values) on packed RGB8 buffers.

/// Returns upright `(width, height, rgb)`. Orientation outside 2..=8 is a no-op.
pub fn apply(w: u32, h: u32, rgb: Vec<u8>, orientation: u8) -> (u32, u32, Vec<u8>) {
    if !(2..=8).contains(&orientation) || w == 0 || h == 0 {
        return (w, h, rgb);
    }
    let (wu, hu) = (w as usize, h as usize);
    let swap = orientation >= 5;
    let (dw, dh) = if swap { (hu, wu) } else { (wu, hu) };
    let mut out = vec![0u8; dw * dh * 3];
    for oy in 0..dh {
        for ox in 0..dw {
            let (sx, sy) = match orientation {
                2 => (wu - 1 - ox, oy),
                3 => (wu - 1 - ox, hu - 1 - oy),
                4 => (ox, hu - 1 - oy),
                5 => (oy, ox),
                6 => (oy, hu - 1 - ox),
                7 => (wu - 1 - oy, hu - 1 - ox),
                _ => (wu - 1 - oy, ox),
            };
            let s = (sy * wu + sx) * 3;
            let d = (oy * dw + ox) * 3;
            out[d..d + 3].copy_from_slice(&rgb[s..s + 3]);
        }
    }
    (dw as u32, dh as u32, out)
}
