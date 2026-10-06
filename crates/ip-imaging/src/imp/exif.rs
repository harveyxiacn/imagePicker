//! Fill [`Metadata`] from TIFF/EXIF structures.
use super::tiff::*;
use crate::Metadata;

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn num(b: &[u8]) -> Option<i64> {
    if b.is_empty() || !b.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(b).ok()?.parse().ok()
}

/// Parse `YYYY:MM:DD HH:MM:SS` (also `-`/`/` separators), optional sub-second digits and
/// `+HH:MM` offset. Naive times are treated as UTC.
pub fn parse_exif_datetime(dt: &str, subsec: Option<&str>, offset: Option<&str>) -> Option<i64> {
    let b = dt.trim().as_bytes();
    if b.len() < 19 {
        return None;
    }
    let (y, mo, d) = (num(&b[0..4])?, num(&b[5..7])?, num(&b[8..10])?);
    let (h, mi, s) = (num(&b[11..13])?, num(&b[14..16])?, num(&b[17..19])?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 || y < 1 {
        return None;
    }
    let mut ms = (days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + s) * 1000;
    if let Some(ss) = subsec {
        let digits: Vec<u8> = ss
            .trim()
            .bytes()
            .take_while(u8::is_ascii_digit)
            .take(3)
            .collect();
        if !digits.is_empty() {
            let mut v = num(&digits)?;
            for _ in digits.len()..3 {
                v *= 10;
            }
            ms += v;
        }
    }
    if let Some(min) = offset.and_then(parse_exif_offset) {
        ms -= i64::from(min) * 60_000;
    }
    Some(ms)
}

/// Parse an EXIF `OffsetTime*` value (`+HH:MM` / `-HH:MM`) into minutes east of UTC.
pub fn parse_exif_offset(off: &str) -> Option<i32> {
    let o = off.trim().as_bytes();
    if o.len() < 6 || !(o[0] == b'+' || o[0] == b'-') || o[3] != b':' {
        return None;
    }
    let min = (num(&o[1..3])? * 60 + num(&o[4..6])?) as i32;
    Some(if o[0] == b'-' { -min } else { min })
}

fn get_str(t: &Tiff, ifd: &Ifd, tag: u16) -> Option<String> {
    ifd.get(tag).and_then(|e| t.ascii(e))
}

fn get_f(t: &Tiff, ifd: &Ifd, tag: u16) -> Option<f64> {
    ifd.get(tag).and_then(|e| t.rational(e, 0))
}

fn set_if_none<T>(slot: &mut Option<T>, v: Option<T>) {
    if slot.is_none() {
        *slot = v;
    }
}

pub fn fill_ifd0(md: &mut Metadata, t: &Tiff, ifd: &Ifd) {
    set_if_none(&mut md.camera_make, get_str(t, ifd, TAG_MAKE));
    set_if_none(&mut md.camera_model, get_str(t, ifd, TAG_MODEL));
    set_if_none(&mut md.camera_serial, get_str(t, ifd, 0xC62F));
    if md.orientation == 0 {
        if let Some(o) = ifd.get(TAG_ORIENTATION).and_then(|e| t.first_uint(e)) {
            if (1..=8).contains(&o) {
                md.orientation = o as u8;
            }
        }
    }
}

pub fn fill_exif(md: &mut Metadata, t: &Tiff, ifd: &Ifd) {
    if md.taken_at_ms.is_none() {
        let dt = get_str(t, ifd, 0x9003).or_else(|| get_str(t, ifd, 0x9004));
        if let Some(dt) = dt {
            let (sub, off) = if get_str(t, ifd, 0x9003).is_some() {
                (get_str(t, ifd, 0x9291), get_str(t, ifd, 0x9011))
            } else {
                (get_str(t, ifd, 0x9292), get_str(t, ifd, 0x9012))
            };
            md.taken_at_ms = parse_exif_datetime(&dt, sub.as_deref(), off.as_deref());
            md.taken_at_offset_min = off.as_deref().and_then(parse_exif_offset);
        }
    }
    set_if_none(&mut md.camera_serial, get_str(t, ifd, 0xA431));
    set_if_none(&mut md.lens, get_str(t, ifd, 0xA434));
    set_if_none(&mut md.focal_mm, get_f(t, ifd, 0x920A).map(|v| v as f32));
    set_if_none(
        &mut md.aperture,
        get_f(t, ifd, 0x829D)
            .or_else(|| get_f(t, ifd, 0x9202).map(|a| 2f64.sqrt().powf(a)))
            .filter(|v| *v > 0.0)
            .map(|v| v as f32),
    );
    set_if_none(
        &mut md.shutter_s,
        get_f(t, ifd, 0x829A)
            .or_else(|| get_f(t, ifd, 0x9201).map(|s| 2f64.powf(-s)))
            .filter(|v| *v > 0.0)
            .map(|v| v as f32),
    );
    if md.iso.is_none() {
        let iso = ifd
            .get(0x8827)
            .and_then(|e| t.first_uint(e))
            .or_else(|| ifd.get(0x8833).and_then(|e| t.first_uint(e)))
            .filter(|v| *v != 0 && *v != 65535);
        md.iso = iso;
    }
}

pub fn fill_gps(md: &mut Metadata, t: &Tiff, ifd: &Ifd) {
    if md.gps_lat.is_some() {
        return;
    }
    let coord = |tag: u16, rtag: u16, neg: u8| -> Option<f64> {
        let e = ifd.get(tag)?;
        let d = t.rational(e, 0)?;
        let m = t.rational(e, 1).unwrap_or(0.0);
        let s = t.rational(e, 2).unwrap_or(0.0);
        let mut v = d + m / 60.0 + s / 3600.0;
        if let Some(r) = get_str(t, ifd, rtag) {
            if r.as_bytes()[0].to_ascii_uppercase() == neg {
                v = -v;
            }
        }
        Some(v)
    };
    let lat = coord(0x0002, 0x0001, b'S');
    let lon = coord(0x0004, 0x0003, b'W');
    if let (Some(la), Some(lo)) = (lat, lon) {
        if la.abs() <= 90.0 && lo.abs() <= 180.0 && !(la == 0.0 && lo == 0.0) {
            md.gps_lat = Some(la);
            md.gps_lon = Some(lo);
        }
    }
}

/// Largest pixel area among IFD ImageWidth/Length pairs and Exif PixelX/YDimension.
pub fn tiff_dims(t: &Tiff, w: &Walked) -> Option<(u32, u32)> {
    let mut best: Option<(u32, u32)> = None;
    let mut consider = |wd: Option<u32>, ht: Option<u32>| {
        if let (Some(a), Some(b)) = (wd, ht) {
            if a > 0
                && b > 0
                && best.is_none_or(|(x, y)| (a as u64 * b as u64) > (x as u64 * y as u64))
            {
                best = Some((a, b));
            }
        }
    };
    for ifd in t.all_ifds(w) {
        consider(
            ifd.get(TAG_IMAGE_WIDTH).and_then(|e| t.first_uint(e)),
            ifd.get(TAG_IMAGE_LENGTH).and_then(|e| t.first_uint(e)),
        );
        consider(
            ifd.get(0xA002).and_then(|e| t.first_uint(e)),
            ifd.get(0xA003).and_then(|e| t.first_uint(e)),
        );
    }
    best
}

/// Fill everything from a full TIFF structure; returns the best dimensions found.
pub fn fill_from_tiff(md: &mut Metadata, t: &Tiff) -> Option<(u32, u32)> {
    let w = t.walk();
    if let Some(ifd0) = w.chain.first() {
        fill_ifd0(md, t, ifd0);
    }
    if let Some(e) = &w.exif {
        fill_exif(md, t, e);
    }
    if let Some(g) = &w.gps {
        fill_gps(md, t, g);
    }
    tiff_dims(t, &w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datetime() {
        assert_eq!(
            parse_exif_datetime("1970:01:01 00:00:00", None, None),
            Some(0)
        );
        assert_eq!(
            parse_exif_datetime("2024:03:05 10:20:30", Some("45"), None),
            Some(1_709_634_030_450)
        );
        assert_eq!(
            parse_exif_datetime("2024:03:05 10:20:30", None, Some("+08:00")),
            Some(1_709_634_030_000 - 8 * 3_600_000)
        );
        assert_eq!(parse_exif_datetime("    :  :     :  :  ", None, None), None);
    }
}

#[cfg(test)]
mod offset_tests {
    use super::*;

    #[test]
    fn parses_offsets() {
        assert_eq!(parse_exif_offset("+08:00"), Some(480));
        assert_eq!(parse_exif_offset("-03:00"), Some(-180));
        assert_eq!(parse_exif_offset("+05:45"), Some(345));
        assert_eq!(parse_exif_offset("   "), None);
        assert_eq!(parse_exif_offset("0800"), None);
    }

    #[test]
    fn offset_shifts_to_utc() {
        let naive = parse_exif_datetime("2025:05:13 15:50:43", Some("930"), None).unwrap();
        let west = parse_exif_datetime("2025:05:13 15:50:43", Some("930"), Some("-03:00")).unwrap();
        assert_eq!(west - naive, 3 * 3600 * 1000);
    }
}
