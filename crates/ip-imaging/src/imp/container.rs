//! Container-level parsing: JPEG segments, RAW previews (TIFF-based, CR3, RAF), ISO-BMFF/HEIF.
use super::tiff::*;

// ---------------------------------------------------------------- JPEG

#[derive(Default, Debug)]
pub struct JpegInfo<'a> {
    pub exif_tiff: Option<&'a [u8]>,
    pub width: u32,
    pub height: u32,
    /// SOF marker byte (0xC0 baseline, 0xC2 progressive, 0xC3 lossless, ...).
    pub sof: u8,
}

/// Single pass over header segments until SOS. Returns `None` if not a JPEG.
pub fn jpeg_info(data: &[u8]) -> Option<JpegInfo<'_>> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return None;
    }
    let mut info = JpegInfo::default();
    let mut p = 2usize;
    while p + 4 <= data.len() {
        if data[p] != 0xFF {
            p += 1;
            continue;
        }
        let m = data[p + 1];
        if m == 0xFF {
            p += 1;
            continue;
        }
        if m == 0x00 || m == 0x01 || (0xD0..=0xD8).contains(&m) {
            p += 2;
            continue;
        }
        if m == 0xDA || m == 0xD9 {
            break;
        }
        let len = u16::from_be_bytes([data[p + 2], data[p + 3]]) as usize;
        if len < 2 {
            break;
        }
        let seg = data.get(p + 4..(p + 2 + len).min(data.len()))?;
        match m {
            0xE1 if info.exif_tiff.is_none() && seg.starts_with(b"Exif\0\0") => {
                info.exif_tiff = Some(&seg[6..]);
            }
            0xC0..=0xCF if !matches!(m, 0xC4 | 0xC8 | 0xCC) && info.sof == 0 && seg.len() >= 5 => {
                info.sof = m;
                info.height = u16::from_be_bytes([seg[1], seg[2]]) as u32;
                info.width = u16::from_be_bytes([seg[3], seg[4]]) as u32;
            }
            _ => {}
        }
        p += 2 + len;
    }
    Some(info)
}

/// Is this slice a decodable (non-lossless) JPEG? Returns its dimensions.
pub fn usable_jpeg_dims(data: &[u8]) -> Option<(u32, u32)> {
    let i = jpeg_info(data)?;
    (i.sof != 0 && i.sof != 0xC3 && i.width > 0 && i.height > 0).then_some((i.width, i.height))
}

/// The JPEG's EXIF-embedded thumbnail (IFD1), if any.
pub fn exif_thumbnail(tiff_data: &[u8]) -> Option<&[u8]> {
    let t = Tiff::new(tiff_data)?;
    let w = t.walk();
    let ifd1 = w.chain.get(1)?;
    let off = t.first_uint(ifd1.get(TAG_JPEG_OFFSET)?)? as usize;
    let len = t.first_uint(ifd1.get(TAG_JPEG_LEN)?)? as usize;
    let s = tiff_data.get(off..off.checked_add(len)?)?;
    usable_jpeg_dims(s).map(|_| s)
}

// ---------------------------------------------------------------- previews

fn best_by_area<'a>(cands: impl IntoIterator<Item = &'a [u8]>) -> Option<(&'a [u8], (u32, u32))> {
    let mut best: Option<(&[u8], (u32, u32))> = None;
    for c in cands {
        if let Some(d) = usable_jpeg_dims(c) {
            if best.is_none_or(|(_, b)| d.0 as u64 * d.1 as u64 > b.0 as u64 * b.1 as u64) {
                best = Some((c, d));
            }
        }
    }
    best
}

/// Candidate JPEG slices referenced from TIFF IFDs.
pub fn tiff_jpeg_candidates(data: &[u8]) -> Vec<&[u8]> {
    let Some(t) = Tiff::new(data) else {
        return vec![];
    };
    let w = t.walk();
    let mut out = Vec::new();
    let slice =
        |off: u32, len: u32| data.get(off as usize..(off as usize).checked_add(len as usize)?);
    for ifd in t.all_ifds(&w) {
        if let (Some(o), Some(l)) = (ifd.get(TAG_JPEG_OFFSET), ifd.get(TAG_JPEG_LEN)) {
            if let (Some(o), Some(l)) = (t.first_uint(o), t.first_uint(l)) {
                out.extend(slice(o, l));
            }
        }
        let comp = ifd.get(TAG_COMPRESSION).and_then(|e| t.first_uint(e));
        if matches!(comp, Some(6 | 7)) {
            if let (Some(o), Some(l)) = (ifd.get(TAG_STRIP_OFFSETS), ifd.get(TAG_STRIP_BYTES)) {
                if let (Some(o), Some(l)) = (t.first_uint(o), t.first_uint(l)) {
                    out.extend(slice(o, l));
                }
            }
        }
        if let Some(e) = ifd.get(TAG_RW2_JPG) {
            // Panasonic: UNDEFINED blob whose count is the JPEG length.
            if e.count > 4 {
                out.extend(data.get(e.pos..e.pos + e.count as usize));
            }
        }
    }
    out
}

/// Heuristic scan for JPEG streams (used when IFDs yield nothing useful). Scans the first
/// `limit` bytes only; each candidate extends to the end of the scanned data.
pub fn scan_jpeg_candidates(data: &[u8], limit: usize) -> Vec<&[u8]> {
    let end = data.len().min(limit);
    let mut out = Vec::new();
    let mut i = 0;
    while i + 4 < end && out.len() < 16 {
        match data[i..end].iter().position(|&b| b == 0xFF) {
            None => break,
            Some(k) => i += k,
        }
        if i + 4 < end && data[i + 1] == 0xD8 && data[i + 2] == 0xFF && data[i + 3] >= 0xC0 {
            out.push(&data[i..]);
            i += 4;
        } else {
            i += 1;
        }
    }
    out
}

// ---------------------------------------------------------------- ISO-BMFF

pub fn iter_boxes(mut data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    while data.len() >= 8 {
        let size = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as u64;
        let typ = [data[4], data[5], data[6], data[7]];
        let (hdr, total) = match size {
            0 => (8usize, data.len() as u64),
            1 => {
                if data.len() < 16 {
                    break;
                }
                let s = u64::from_be_bytes(data[8..16].try_into().unwrap());
                (16usize, s)
            }
            s => (8usize, s),
        };
        if total < hdr as u64 || total > data.len() as u64 {
            // Tolerate truncated trailing box: take what is there.
            if total >= hdr as u64 {
                out.push((typ, &data[hdr..]));
            }
            break;
        }
        out.push((typ, &data[hdr..total as usize]));
        data = &data[total as usize..];
    }
    out
}

fn plausible_box_type(t: &[u8; 4]) -> bool {
    t.iter().all(|c| c.is_ascii_alphanumeric() || *c == b' ')
}

#[derive(Default)]
pub struct Cr3<'a> {
    pub cmt1: Option<&'a [u8]>,
    pub cmt2: Option<&'a [u8]>,
    pub cmt4: Option<&'a [u8]>,
    pub thmb: Option<&'a [u8]>,
    pub prvw: Option<&'a [u8]>,
}

fn jpeg_in_payload(p: &[u8]) -> Option<&[u8]> {
    let lim = p.len().min(64);
    let k = p[..lim].windows(3).position(|w| w == [0xFF, 0xD8, 0xFF])?;
    Some(&p[k..])
}

pub fn parse_cr3(data: &[u8]) -> Cr3<'_> {
    let mut r = Cr3::default();
    for (t, payload) in iter_boxes(data) {
        match &t {
            b"moov" => {
                for (t2, p2) in iter_boxes(payload) {
                    if &t2 == b"uuid" && p2.len() > 16 {
                        collect_cr3_children(&p2[16..], &mut r);
                    }
                }
            }
            b"uuid" if payload.len() > 16 => collect_cr3_children(&payload[16..], &mut r),
            _ => {}
        }
    }
    r
}

fn collect_cr3_children<'a>(mut body: &'a [u8], r: &mut Cr3<'a>) {
    let mut boxes = iter_boxes(body);
    if boxes.first().is_none_or(|(t, _)| !plausible_box_type(t)) && body.len() > 8 {
        body = &body[8..];
        boxes = iter_boxes(body);
    }
    for (t, p) in boxes {
        match &t {
            b"CMT1" => r.cmt1 = Some(p),
            b"CMT2" => r.cmt2 = Some(p),
            b"CMT4" => r.cmt4 = Some(p),
            b"THMB" => r.thmb = jpeg_in_payload(p),
            b"PRVW" => r.prvw = jpeg_in_payload(p),
            _ => {}
        }
    }
}

pub fn is_cr3(data: &[u8]) -> bool {
    data.len() > 12 && &data[4..8] == b"ftyp"
}

// ---------------------------------------------------------------- RAF

pub fn is_raf(data: &[u8]) -> bool {
    data.starts_with(b"FUJIFILMCCD-RAW")
}

pub fn raf_jpeg(data: &[u8]) -> Option<&[u8]> {
    let off = u32::from_be_bytes(data.get(84..88)?.try_into().ok()?) as usize;
    let len = u32::from_be_bytes(data.get(88..92)?.try_into().ok()?) as usize;
    let s = data.get(off..off.checked_add(len)?)?;
    usable_jpeg_dims(s).map(|_| s)
}

// ---------------------------------------------------------------- RAW preview selection

/// Largest embedded JPEG preview of a RAW file.
pub fn raw_preview(data: &[u8]) -> Option<&[u8]> {
    if is_raf(data) {
        return raf_jpeg(data)
            .or_else(|| best_by_area(scan_jpeg_candidates(data, 4 << 20)).map(|b| b.0));
    }
    if is_cr3(data) {
        let c = parse_cr3(data);
        return best_by_area(c.prvw.into_iter().chain(c.thmb)).map(|b| b.0);
    }
    let mut best = best_by_area(tiff_jpeg_candidates(data));
    let small = best.is_none_or(|(_, (w, h))| w.max(h) < 1000);
    if small {
        let scanned = best_by_area(scan_jpeg_candidates(data, 12 << 20));
        if let Some(s) = scanned {
            if best.is_none_or(|(_, b)| s.1 .0 as u64 * s.1 .1 as u64 > b.0 as u64 * b.1 as u64) {
                best = Some(s);
            }
        }
    }
    best.map(|b| b.0)
}

// ---------------------------------------------------------------- HEIF / AVIF

#[derive(Default)]
pub struct HeifInfo<'a> {
    /// Stored (unrotated) size of the primary image, after its `clap` crop; the largest `ispe`
    /// when the primary item's properties cannot be resolved.
    pub width: u32,
    pub height: u32,
    /// The primary item's clean aperture when it crops the coded image (encoders pad sizes
    /// the codec cannot represent, e.g. below 64 px or odd).
    pub crop: Option<Crop>,
    pub exif_tiff: Option<&'a [u8]>,
    /// EXIF-style orientation (1..=8) of the primary item's `irot`/`imir` properties; `None`
    /// when its property associations were not found.
    pub orientation: Option<u8>,
    /// The primary image itself when it is JPEG-coded (legal, if rare).
    pub primary_jpeg: Option<&'a [u8]>,
    /// JPEG thumbnails/previews: other JPEG-coded items and the EXIF IFD1 thumbnail.
    pub jpeg_previews: Vec<&'a [u8]>,
    /// The primary item's colour space (`colr`); an ICC profile wins over `nclx`.
    pub colr: Option<Colr<'a>>,
    /// No decoder can read this file: there is no primary image item (no `meta`, `pitm` or
    /// matching `iinf` entry), or the primary image is HEVC / AV1 coded without its decoder
    /// configuration (`hvcC` / `av1C`). Handing such a file to an OS decoder is pointless and
    /// crashed macOS ImageIO (SIGTRAP), so the loader does not.
    pub undecodable: bool,
}

/// A `clap` clean aperture inside the coded (`ispe`) image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Crop {
    pub coded: (u32, u32),
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// `clap`: width, height and the centre offset as fractions; rounded like libheif.
fn clean_aperture(p: &[u8], (cw, ch): (u32, u32)) -> Option<Crop> {
    let v = |i: usize| Some(i32::from_be_bytes(p.get(i * 4..i * 4 + 4)?.try_into().ok()?) as f64);
    let frac = |i: usize| {
        let d = v(i + 1)?;
        (d != 0.0).then_some(v(i)? / d)
    };
    let (w, h) = (frac(0)?.round(), frac(2)?.round());
    let x = (frac(4)? + (cw as f64 - 1.0) / 2.0 - (w - 1.0) / 2.0).floor();
    let y = (frac(6)? + (ch as f64 - 1.0) / 2.0 - (h - 1.0) / 2.0).floor();
    if w < 1.0 || h < 1.0 || x < 0.0 || y < 0.0 || x + w > cw as f64 || y + h > ch as f64 {
        return None;
    }
    Some(Crop {
        coded: (cw, ch),
        x: x as u32,
        y: y as u32,
        w: w as u32,
        h: h as u32,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Colr<'a> {
    /// `prof` / `rICC`: an ICC profile.
    Icc(&'a [u8]),
    /// `nclx`: ISO/IEC 23091-2 code points (the matrix only matters to the decoder).
    Nclx { primaries: u16, transfer: u16 },
}

fn parse_colr(p: &[u8]) -> Option<Colr<'_>> {
    let be16 = |r: std::ops::Range<usize>| Some(u16::from_be_bytes(p.get(r)?.try_into().ok()?));
    match p.get(0..4)? {
        b"prof" | b"rICC" => Some(Colr::Icc(p.get(4..).filter(|icc| !icc.is_empty())?)),
        b"nclx" => Some(Colr::Nclx {
            primaries: be16(4..6)?,
            transfer: be16(6..8)?,
        }),
        _ => None,
    }
}

/// Big-endian unsigned integer of `n` bytes at `*pos` (`n == 0` reads 0), advancing `pos`.
fn be_uint(b: &[u8], pos: &mut usize, n: usize) -> Option<u64> {
    let s = b.get(*pos..pos.checked_add(n)?)?;
    *pos += n;
    Some(s.iter().fold(0u64, |a, &x| (a << 8) | x as u64))
}

/// One `iloc` entry: construction method (0 file, 1 `idat`) and the first extent.
struct ItemLoc {
    id: u32,
    method: u8,
    offset: u64,
    len: u64,
    extents: u64,
}

fn parse_iloc(p: &[u8]) -> Vec<ItemLoc> {
    let mut out = Vec::new();
    if p.len() < 8 {
        return out;
    }
    let ver = p[0];
    let osz = (p[4] >> 4) as usize;
    let lsz = (p[4] & 15) as usize;
    let bsz = (p[5] >> 4) as usize;
    let isz = if ver == 1 || ver == 2 {
        (p[5] & 15) as usize
    } else {
        0
    };
    let idsz = if ver < 2 { 2 } else { 4 };
    let mut pos = 6usize;
    let Some(count) = be_uint(p, &mut pos, idsz) else {
        return out;
    };
    for _ in 0..count.min(4096) {
        let Some(id) = be_uint(p, &mut pos, idsz) else {
            break;
        };
        let mut method = 0;
        if ver == 1 || ver == 2 {
            let Some(v) = be_uint(p, &mut pos, 2) else {
                break;
            };
            method = (v & 15) as u8;
        }
        let (Some(_dref), Some(base), Some(ec)) = (
            be_uint(p, &mut pos, 2),
            be_uint(p, &mut pos, bsz),
            be_uint(p, &mut pos, 2),
        ) else {
            break;
        };
        let mut first = None;
        for _ in 0..ec {
            if be_uint(p, &mut pos, isz).is_none() {
                return out;
            }
            let (Some(off), Some(len)) = (be_uint(p, &mut pos, osz), be_uint(p, &mut pos, lsz))
            else {
                return out;
            };
            first = first.or(Some((off, len)));
        }
        if let Some((off, len)) = first {
            out.push(ItemLoc {
                id: id as u32,
                method,
                offset: base.saturating_add(off),
                len,
                extents: ec,
            });
        }
    }
    out
}

/// Bytes of the first extent (length 0 = to the end of the source).
fn item_bytes<'a>(data: &'a [u8], idat: Option<&'a [u8]>, l: &ItemLoc) -> Option<&'a [u8]> {
    let src = match l.method {
        0 => data,
        1 => idat?,
        _ => return None,
    };
    let start = usize::try_from(l.offset).ok()?;
    let end = if l.len == 0 {
        src.len()
    } else {
        start.checked_add(usize::try_from(l.len).ok()?)?
    };
    src.get(start..end)
}

/// `iinf`: (item id, item type) of every `infe` (version >= 2).
fn parse_iinf(p: &[u8]) -> Vec<(u32, [u8; 4])> {
    let mut out = Vec::new();
    if p.len() <= 6 {
        return out;
    }
    let body = if p[0] == 0 {
        &p[6..]
    } else {
        &p[8.min(p.len())..]
    };
    for (t, e) in iter_boxes(body) {
        if &t != b"infe" || e.len() < 12 || e[0] < 2 {
            continue;
        }
        let (id, ty) = if e[0] == 2 {
            (u16::from_be_bytes([e[4], e[5]]) as u32, &e[8..12])
        } else if e.len() >= 14 {
            (u32::from_be_bytes(e[4..8].try_into().unwrap()), &e[10..14])
        } else {
            continue;
        };
        out.push((id, ty.try_into().unwrap()));
    }
    out
}

/// `ipma`: item id -> 1-based `ipco` property indices, in association order.
fn parse_ipma(p: &[u8], out: &mut Vec<(u32, Vec<usize>)>) {
    if p.len() < 8 {
        return;
    }
    let idsz = if p[0] < 1 { 2 } else { 4 };
    let wide = p[3] & 1 == 1;
    let mut pos = 4usize;
    let Some(n) = be_uint(p, &mut pos, 4) else {
        return;
    };
    for _ in 0..n.min(65536) {
        let (Some(id), Some(cnt)) = (be_uint(p, &mut pos, idsz), be_uint(p, &mut pos, 1)) else {
            return;
        };
        let mut props = Vec::with_capacity(cnt as usize);
        for _ in 0..cnt {
            let Some(v) = be_uint(p, &mut pos, if wide { 2 } else { 1 }) else {
                return;
            };
            // the top bit is the `essential` flag
            props.push((v & if wide { 0x7FFF } else { 0x7F }) as usize);
        }
        out.push((id as u32, props));
    }
}

/// EXIF orientation equivalent to HEIF transformative properties applied in order. The state
/// is `(r, f)`: mirror left-right if `f`, then rotate `r` quarter turns clockwise.
pub fn heif_orientation(props: &[([u8; 4], &[u8])]) -> u8 {
    let (mut r, mut f) = (0u8, false);
    for (t, p) in props {
        match t {
            // irot: anticlockwise quarter turns
            b"irot" if !p.is_empty() => r = (r + 4 - (p[0] & 3)) % 4,
            // imir: axis 1 mirrors left-right, axis 0 top-bottom (= left-right + 180°), as
            // libheif reads it. Mirroring after a rotation negates the rotation.
            b"imir" if !p.is_empty() => {
                r = (4 - r) % 4;
                f = !f;
                if p[0] & 1 == 0 {
                    r = (r + 2) % 4;
                }
            }
            _ => {}
        }
    }
    match (r, f) {
        (0, false) => 1,
        (0, true) => 2,
        (2, false) => 3,
        (2, true) => 4,
        (3, true) => 5,
        (1, false) => 6,
        (1, true) => 7,
        _ => 8,
    }
}

pub fn parse_heif(data: &[u8]) -> HeifInfo<'_> {
    let mut info = HeifInfo {
        undecodable: true,
        ..HeifInfo::default()
    };
    let Some((_, meta)) = iter_boxes(data).into_iter().find(|(t, _)| t == b"meta") else {
        return info;
    };
    if meta.len() < 4 {
        return info;
    }
    let mut primary = None;
    let mut items = Vec::new();
    let mut locs = Vec::new();
    let mut idat = None;
    let mut ipco: Vec<([u8; 4], &[u8])> = Vec::new();
    let mut ipma = Vec::new();
    for (t, p) in iter_boxes(&meta[4..]) {
        match &t {
            b"pitm" if p.len() >= 6 => {
                let mut pos = 4;
                primary = be_uint(p, &mut pos, if p[0] == 0 { 2 } else { 4 }).map(|v| v as u32);
            }
            b"iinf" => items = parse_iinf(p),
            b"iloc" => locs = parse_iloc(p),
            b"idat" => idat = Some(p),
            b"iprp" => {
                for (t2, p2) in iter_boxes(p) {
                    match &t2 {
                        b"ipco" => ipco = iter_boxes(p2),
                        b"ipma" => parse_ipma(p2, &mut ipma),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    let ispe = |p: &[u8]| -> Option<(u32, u32)> {
        let w = u32::from_be_bytes(p.get(4..8)?.try_into().ok()?);
        let h = u32::from_be_bytes(p.get(8..12)?.try_into().ok()?);
        Some((w, h))
    };
    // primary item: size and transformative properties
    let mut primary_props: Option<Vec<[u8; 4]>> = None;
    if let Some(props) = primary.and_then(|id| ipma.iter().find(|(i, _)| *i == id)) {
        let props: Vec<([u8; 4], &[u8])> = props
            .1
            .iter()
            .filter_map(|&k| ipco.get(k.checked_sub(1)?).copied())
            .collect();
        primary_props = Some(props.iter().map(|(t, _)| *t).collect());
        if let Some((w, h)) = props
            .iter()
            .find(|(t, _)| t == b"ispe")
            .and_then(|&(_, p)| ispe(p))
        {
            info.width = w;
            info.height = h;
            let clap = props.iter().find(|(t, _)| t == b"clap");
            info.crop = clap
                .and_then(|&(_, p)| clean_aperture(p, (w, h)))
                .filter(|c| (c.w, c.h) != (w, h));
            if let Some(c) = info.crop {
                info.width = c.w;
                info.height = c.h;
            }
        }
        info.orientation = Some(heif_orientation(&props));
        for &(_, p) in props.iter().filter(|(t, _)| t == b"colr") {
            match parse_colr(p) {
                Some(c @ Colr::Icc(_)) if !matches!(info.colr, Some(Colr::Icc(_))) => {
                    info.colr = Some(c)
                }
                Some(c @ Colr::Nclx { .. }) if info.colr.is_none() => info.colr = Some(c),
                _ => {}
            }
        }
    }
    if info.width == 0 {
        // no associations: the largest `ispe` (the primary image or its grid)
        for &(_, p) in ipco.iter().filter(|(t, _)| t == b"ispe") {
            if let Some((w, h)) = ispe(p) {
                if w as u64 * h as u64 > info.width as u64 * info.height as u64 {
                    info.width = w;
                    info.height = h;
                }
            }
        }
    }
    for (id, ty) in &items {
        let Some(loc) = locs.iter().find(|l| l.id == *id) else {
            continue;
        };
        match ty {
            b"Exif" if info.exif_tiff.is_none() => {
                // 4-byte offset to the TIFF header, then the EXIF block
                let tiff = item_bytes(data, idat, loc).and_then(|item| {
                    let skip = u32::from_be_bytes(item.get(0..4)?.try_into().ok()?) as usize;
                    item.get(4..)?.get(skip..)
                });
                if let Some(t) = tiff.filter(|t| Tiff::new(t).is_some()) {
                    info.exif_tiff = Some(t);
                    info.jpeg_previews.extend(exif_thumbnail(t));
                }
            }
            b"jpeg" if loc.extents == 1 => {
                if let Some(j) =
                    item_bytes(data, idat, loc).filter(|j| usable_jpeg_dims(j).is_some())
                {
                    if Some(*id) == primary {
                        info.primary_jpeg = Some(j);
                    } else {
                        info.jpeg_previews.push(j);
                    }
                }
            }
            _ => {}
        }
    }
    let primary_type = primary.and_then(|id| items.iter().find(|(i, _)| *i == id).map(|e| e.1));
    let config: Option<&[u8; 4]> = match primary_type.as_ref() {
        Some(b"hvc1" | b"hev1") => Some(b"hvcC"),
        Some(b"av01") => Some(b"av1C"),
        _ => None,
    };
    info.undecodable = match (primary_type, config, &primary_props) {
        (None, _, _) => true,
        (Some(_), Some(c), Some(props)) => !props.contains(c),
        _ => false,
    };
    info
}
