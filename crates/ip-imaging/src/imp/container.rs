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
    pub width: u32,
    pub height: u32,
    pub exif_tiff: Option<&'a [u8]>,
}

pub fn parse_heif(data: &[u8]) -> HeifInfo<'_> {
    let mut info = HeifInfo::default();
    let Some((_, meta)) = iter_boxes(data).into_iter().find(|(t, _)| t == b"meta") else {
        return info;
    };
    if meta.len() < 4 {
        return info;
    }
    let meta = &meta[4..];
    let boxes = iter_boxes(meta);
    // dimensions: largest ispe in iprp/ipco
    for (t, p) in &boxes {
        if t == b"iprp" {
            for (t2, p2) in iter_boxes(p) {
                if &t2 == b"ipco" {
                    for (t3, p3) in iter_boxes(p2) {
                        if &t3 == b"ispe" && p3.len() >= 12 {
                            let w = u32::from_be_bytes(p3[4..8].try_into().unwrap());
                            let h = u32::from_be_bytes(p3[8..12].try_into().unwrap());
                            if w as u64 * h as u64 > info.width as u64 * info.height as u64 {
                                info.width = w;
                                info.height = h;
                            }
                        }
                    }
                }
            }
        }
    }
    // Exif item id
    let mut exif_id = None;
    for (t, p) in &boxes {
        if t == b"iinf" && p.len() > 6 {
            let ver = p[0];
            let body = if ver == 0 {
                &p[6..]
            } else {
                &p[8.min(p.len())..]
            };
            for (t2, p2) in iter_boxes(body) {
                if &t2 == b"infe" && p2.len() >= 12 && p2[0] >= 2 {
                    let (id, ty) = if p2[0] == 2 {
                        (u16::from_be_bytes([p2[4], p2[5]]) as u32, &p2[8..12])
                    } else if p2.len() >= 14 {
                        (
                            u32::from_be_bytes(p2[4..8].try_into().unwrap()),
                            &p2[10..14],
                        )
                    } else {
                        continue;
                    };
                    if ty == b"Exif" {
                        exif_id = Some(id);
                    }
                }
            }
        }
    }
    let Some(exif_id) = exif_id else { return info };
    for (t, p) in &boxes {
        if t == b"iloc" && p.len() >= 8 {
            let ver = p[0];
            let osz = (p[4] >> 4) as usize;
            let lsz = (p[4] & 15) as usize;
            let bsz = (p[5] >> 4) as usize;
            let isz = if ver == 1 || ver == 2 {
                (p[5] & 15) as usize
            } else {
                0
            };
            let rd = |b: &[u8], pos: &mut usize, n: usize| -> Option<u64> {
                let s = b.get(*pos..*pos + n)?;
                *pos += n;
                Some(s.iter().fold(0u64, |a, &x| (a << 8) | x as u64))
            };
            let mut pos = 6usize;
            let count = if ver < 2 {
                rd(p, &mut pos, 2)
            } else {
                rd(p, &mut pos, 4)
            };
            let Some(count) = count else { continue };
            for _ in 0..count.min(4096) {
                let id = if ver < 2 {
                    rd(p, &mut pos, 2)
                } else {
                    rd(p, &mut pos, 4)
                };
                let Some(id) = id else { break };
                let mut cm = 0;
                if ver == 1 || ver == 2 {
                    cm = rd(p, &mut pos, 2).unwrap_or(0) & 15;
                }
                let _dref = rd(p, &mut pos, 2);
                let base = rd(p, &mut pos, bsz).unwrap_or(0);
                let Some(ec) = rd(p, &mut pos, 2) else { break };
                for k in 0..ec {
                    if isz > 0 {
                        let _ = rd(p, &mut pos, isz);
                    }
                    let (Some(off), Some(len)) = (rd(p, &mut pos, osz), rd(p, &mut pos, lsz))
                    else {
                        break;
                    };
                    if id as u32 == exif_id && k == 0 && cm == 0 {
                        let start = (base + off) as usize;
                        if let Some(item) = data.get(start..start.saturating_add(len as usize)) {
                            if item.len() > 4 {
                                let skip =
                                    u32::from_be_bytes(item[0..4].try_into().unwrap()) as usize;
                                let body = &item[4..];
                                if let Some(tb) = body.get(skip..) {
                                    if Tiff::new(tb).is_some() {
                                        info.exif_tiff = Some(tb);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    info
}
