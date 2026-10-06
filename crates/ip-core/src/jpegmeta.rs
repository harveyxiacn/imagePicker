//! Minimal JPEG metadata handling for exports: copy the source's EXIF into the re-encoded
//! JPEG (minus the orientation that the render already baked in, the embedded thumbnail of the
//! unedited picture and, on request, the GPS position) and embed an sRGB ICC profile.
//!
//! No external dependencies: the EXIF TIFF block is edited in place (offsets of everything
//! else, MakerNotes included, stay valid) and whatever is removed is zeroed so nothing of it
//! survives in the file. Sources that are not JPEG (RAW, HEIC, PNG...) get a small EXIF block
//! synthesised from the catalog metadata instead.

use std::path::Path;
use std::sync::OnceLock;

use ip_imaging::Metadata;

/// EXIF APP1 payloads must fit one JPEG segment.
const MAX_EXIF: usize = 65_000;
/// How much of a source file is scanned for its EXIF segment.
const SCAN_BYTES: u64 = 1 << 20;

// ------------------------------------------------------------------ sRGB ICC profile

fn s15(v: f64) -> [u8; 4] {
    ((v * 65536.0).round() as i32).to_be_bytes()
}

fn pad4(v: &mut Vec<u8>) {
    while !v.len().is_multiple_of(4) {
        v.push(0);
    }
}

fn xyz_tag(x: f64, y: f64, z: f64) -> Vec<u8> {
    let mut t = b"XYZ \0\0\0\0".to_vec();
    t.extend(s15(x));
    t.extend(s15(y));
    t.extend(s15(z));
    t
}

fn text_desc(s: &str) -> Vec<u8> {
    let mut t = b"desc\0\0\0\0".to_vec();
    t.extend(((s.len() + 1) as u32).to_be_bytes());
    t.extend(s.as_bytes());
    t.push(0);
    t.extend([0u8; 4 + 4 + 2 + 1 + 67]); // unicode + scriptcode parts left empty
    t
}

/// A compact sRGB IEC61966-2.1 compatible profile (ICC v2.1, matrix/TRC, D50-adapted primaries,
/// 1024-entry curves), generated rather than shipped as a blob.
pub fn srgb_icc() -> &'static [u8] {
    static ICC: OnceLock<Vec<u8>> = OnceLock::new();
    ICC.get_or_init(build_srgb_icc)
}

fn build_srgb_icc() -> Vec<u8> {
    let curve = {
        let mut t = b"curv\0\0\0\0".to_vec();
        t.extend(1024u32.to_be_bytes());
        for i in 0..1024u32 {
            let v = i as f64 / 1023.0;
            let lin = if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            };
            t.extend(((lin * 65535.0).round() as u16).to_be_bytes());
        }
        t
    };
    let mut cprt = b"text\0\0\0\0".to_vec();
    cprt.extend(b"No copyright, use freely\0");
    // (signature, data); the three curves share one table
    let tags: Vec<([u8; 4], Vec<u8>)> = vec![
        (*b"desc", text_desc("sRGB IEC61966-2.1")),
        (*b"cprt", cprt),
        (*b"wtpt", xyz_tag(0.9642, 1.0, 0.8249)),
        (*b"rXYZ", xyz_tag(0.436_074_7, 0.222_504_5, 0.013_932_2)),
        (*b"gXYZ", xyz_tag(0.385_064_9, 0.716_878_6, 0.097_104_5)),
        (*b"bXYZ", xyz_tag(0.143_080_4, 0.060_616_9, 0.714_173_3)),
        (*b"rTRC", curve),
    ];
    let n_tags = tags.len() + 2; // + gTRC, bTRC pointing at rTRC
    let mut offset = 128 + 4 + 12 * n_tags;
    let mut table = Vec::new();
    let mut body = Vec::new();
    let mut curve_off = 0usize;
    let mut curve_len = 0usize;
    for (sig, data) in &tags {
        let mut d = data.clone();
        let len = d.len();
        pad4(&mut d);
        table.extend(sig);
        table.extend((offset as u32).to_be_bytes());
        table.extend((len as u32).to_be_bytes());
        if sig == b"rTRC" {
            curve_off = offset;
            curve_len = len;
        }
        offset += d.len();
        body.extend(d);
    }
    for sig in [b"gTRC", b"bTRC"] {
        table.extend(sig);
        table.extend((curve_off as u32).to_be_bytes());
        table.extend((curve_len as u32).to_be_bytes());
    }
    let total = 128 + 4 + table.len() + body.len();
    let mut p = vec![0u8; 128];
    p[0..4].copy_from_slice(&(total as u32).to_be_bytes());
    p[8..12].copy_from_slice(&0x0210_0000u32.to_be_bytes());
    p[12..16].copy_from_slice(b"mntr");
    p[16..20].copy_from_slice(b"RGB ");
    p[20..24].copy_from_slice(b"XYZ ");
    p[24..26].copy_from_slice(&2024u16.to_be_bytes());
    p[26..28].copy_from_slice(&1u16.to_be_bytes());
    p[28..30].copy_from_slice(&1u16.to_be_bytes());
    p[36..40].copy_from_slice(b"acsp");
    p[40..44].copy_from_slice(b"MSFT");
    p[48..52].copy_from_slice(b"IEC ");
    p[52..56].copy_from_slice(b"sRGB");
    p[68..72].copy_from_slice(&s15(0.9642));
    p[72..76].copy_from_slice(&s15(1.0));
    p[76..80].copy_from_slice(&s15(0.8249));
    p.extend((n_tags as u32).to_be_bytes());
    p.extend(table);
    p.extend(body);
    debug_assert_eq!(p.len(), total);
    p
}

// ------------------------------------------------------------------ JPEG segments

/// The EXIF TIFF block (without the `Exif\0\0` prefix) of a JPEG, if it has one.
pub fn extract_exif(jpeg: &[u8]) -> Option<Vec<u8>> {
    if jpeg.len() < 4 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return None;
    }
    let mut pos = 2;
    while pos + 4 <= jpeg.len() && jpeg[pos] == 0xFF {
        let marker = jpeg[pos + 1];
        if marker == 0xFF {
            pos += 1; // fill byte
            continue;
        }
        if marker == 0xDA || marker == 0xD9 {
            break; // image data starts
        }
        if (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            pos += 2;
            continue;
        }
        let len = u16::from_be_bytes([jpeg[pos + 2], jpeg[pos + 3]]) as usize;
        if len < 2 || pos + 2 + len > jpeg.len() {
            return None;
        }
        let payload = &jpeg[pos + 4..pos + 2 + len];
        if marker == 0xE1 && payload.starts_with(b"Exif\0\0") {
            return Some(payload[6..].to_vec());
        }
        pos += 2 + len;
    }
    None
}

fn segment(marker: u8, payload: &[u8]) -> Vec<u8> {
    let mut s = vec![0xFF, marker];
    s.extend(((payload.len() + 2) as u16).to_be_bytes());
    s.extend(payload);
    s
}

/// Inserts an EXIF APP1 and an ICC APP2 segment right after the SOI / JFIF header.
pub fn insert_segments(jpeg: &[u8], exif_tiff: Option<&[u8]>, icc: Option<&[u8]>) -> Vec<u8> {
    if jpeg.len() < 2 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return jpeg.to_vec();
    }
    let mut pos = 2;
    while pos + 4 <= jpeg.len() && jpeg[pos] == 0xFF && jpeg[pos + 1] == 0xE0 {
        pos += 2 + u16::from_be_bytes([jpeg[pos + 2], jpeg[pos + 3]]) as usize;
    }
    let pos = pos.min(jpeg.len());
    let mut out = Vec::with_capacity(jpeg.len() + 4096);
    out.extend(&jpeg[..pos]);
    if let Some(t) = exif_tiff {
        let mut p = b"Exif\0\0".to_vec();
        p.extend(t);
        if p.len() + 2 <= 0xFFFF {
            out.extend(segment(0xE1, &p));
        }
    }
    if let Some(icc) = icc {
        let mut p = b"ICC_PROFILE\0\x01\x01".to_vec();
        p.extend(icc);
        if p.len() + 2 <= 0xFFFF {
            out.extend(segment(0xE2, &p));
        }
    }
    out.extend(&jpeg[pos..]);
    out
}

// ------------------------------------------------------------------ EXIF TIFF editing

struct Tiff<'a> {
    d: &'a mut Vec<u8>,
    le: bool,
}

#[derive(Clone, Copy)]
struct Entry {
    /// Offset of the 12-byte entry.
    at: usize,
    tag: u16,
    typ: u16,
    count: u32,
}

fn type_size(t: u16) -> Option<usize> {
    Some(match t {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 => 4,
        5 | 10 | 12 => 8,
        _ => return None,
    })
}

impl Tiff<'_> {
    fn u16(&self, o: usize) -> Option<u16> {
        let b: [u8; 2] = self.d.get(o..o + 2)?.try_into().ok()?;
        Some(if self.le {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    }
    fn u32(&self, o: usize) -> Option<u32> {
        let b: [u8; 4] = self.d.get(o..o + 4)?.try_into().ok()?;
        Some(if self.le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    }
    fn put16(&mut self, o: usize, v: u16) -> Option<()> {
        let b = if self.le {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        };
        self.d.get_mut(o..o + 2)?.copy_from_slice(&b);
        Some(())
    }
    fn put32(&mut self, o: usize, v: u32) -> Option<()> {
        let b = if self.le {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        };
        self.d.get_mut(o..o + 4)?.copy_from_slice(&b);
        Some(())
    }
    fn zero(&mut self, o: usize, len: usize) {
        if let Some(s) = self.d.get_mut(o..o.saturating_add(len)) {
            s.fill(0);
        }
    }

    fn entries(&self, ifd: usize) -> Option<Vec<Entry>> {
        let n = self.u16(ifd)? as usize;
        if n > 512 {
            return None;
        }
        (0..n)
            .map(|i| {
                let at = ifd + 2 + 12 * i;
                Some(Entry {
                    at,
                    tag: self.u16(at)?,
                    typ: self.u16(at + 2)?,
                    count: self.u32(at + 4)?,
                })
            })
            .collect()
    }

    /// `(offset, length)` of an entry's value bytes.
    fn data_range(&self, e: &Entry) -> Option<(usize, usize)> {
        let len = type_size(e.typ)?.checked_mul(e.count as usize)?;
        if len <= 4 {
            Some((e.at + 8, len))
        } else {
            let off = self.u32(e.at + 8)? as usize;
            (off.checked_add(len)? <= self.d.len()).then_some((off, len))
        }
    }

    fn find(&self, ifd: usize, tag: u16) -> Option<Entry> {
        self.entries(ifd)?.into_iter().find(|e| e.tag == tag)
    }

    /// Removes `tag` from the IFD at `ifd`, zeroing its out-of-line data. The entry table
    /// shrinks by one (the next-IFD pointer moves up with it).
    fn remove_tag(&mut self, ifd: usize, tag: u16) -> Option<bool> {
        let entries = self.entries(ifd)?;
        let Some(idx) = entries.iter().position(|e| e.tag == tag) else {
            return Some(false);
        };
        let n = entries.len();
        if let Some((off, len)) = self.data_range(&entries[idx]) {
            if len > 4 {
                self.zero(off, len);
            }
        }
        let table = ifd + 2;
        let next_at = table + 12 * n;
        let next = self.u32(next_at)?;
        self.d
            .copy_within(table + 12 * (idx + 1)..table + 12 * n, table + 12 * idx);
        self.put16(ifd, (n - 1) as u16)?;
        let new_next = table + 12 * (n - 1);
        self.put32(new_next, next)?;
        self.zero(new_next + 4, next_at + 4 - (new_next + 4));
        Some(true)
    }

    /// Zeroes a whole (leaf) IFD and everything its entries point to.
    fn wipe_ifd(&mut self, ifd: usize) -> Option<()> {
        let entries = self.entries(ifd)?;
        for e in &entries {
            if let Some((off, len)) = self.data_range(e) {
                if len > 4 {
                    self.zero(off, len);
                }
            }
        }
        // a thumbnail is referenced by offset + length tags instead of a value
        let (jo, jl) = (self.find(ifd, 0x0201), self.find(ifd, 0x0202));
        if let (Some(jo), Some(jl)) = (jo, jl) {
            if let (Some(off), Some(len)) = (self.u32(jo.at + 8), self.u32(jl.at + 8)) {
                self.zero(off as usize, len as usize);
            }
        }
        self.zero(ifd, 2 + 12 * entries.len() + 4);
        Some(())
    }

    /// Overwrites an inline SHORT/LONG value.
    fn set_inline(&mut self, ifd: usize, tag: u16, value: u32) {
        let Some(e) = self.find(ifd, tag) else { return };
        if e.count != 1 {
            return;
        }
        let _ = match e.typ {
            3 => self.put16(e.at + 8, value.min(u16::MAX as u32) as u16),
            4 => self.put32(e.at + 8, value),
            _ => None,
        };
    }
}

/// Prepares a source's EXIF TIFF block for an exported, re-encoded picture of `dims` pixels:
/// drops the orientation (baked in), the thumbnail and (with `strip_gps`) the GPS IFD, and
/// updates the pixel dimensions. `None` when the block is malformed or too large.
pub fn prepare_exif(tiff: &[u8], dims: (u32, u32), strip_gps: bool) -> Option<Vec<u8>> {
    let mut data = tiff.to_vec();
    let le = match data.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let mut t = Tiff { d: &mut data, le };
    if t.u16(2)? != 42 {
        return None;
    }
    let ifd0 = t.u32(4)? as usize;
    t.entries(ifd0)?; // validates
    t.remove_tag(ifd0, 0x0112)?;
    if strip_gps {
        if let Some(g) = t.find(ifd0, 0x8825) {
            if let Some(off) = t.u32(g.at + 8) {
                t.wipe_ifd(off as usize);
            }
            t.remove_tag(ifd0, 0x8825)?;
        }
    }
    // the embedded thumbnail shows the unedited picture: drop IFD1
    let entries = t.entries(ifd0)?;
    let next_at = ifd0 + 2 + 12 * entries.len();
    let ifd1 = t.u32(next_at)? as usize;
    if ifd1 != 0 {
        t.wipe_ifd(ifd1);
        t.put32(next_at, 0)?;
    }
    t.set_inline(ifd0, 0x0100, dims.0);
    t.set_inline(ifd0, 0x0101, dims.1);
    if let Some(x) = t.find(ifd0, 0x8769) {
        if let Some(off) = t.u32(x.at + 8) {
            let off = off as usize;
            t.set_inline(off, 0xA002, dims.0);
            t.set_inline(off, 0xA003, dims.1);
        }
    }
    (data.len() <= MAX_EXIF).then_some(data)
}

// ------------------------------------------------------------------ synthesised EXIF

struct NewEntry {
    tag: u16,
    typ: u16,
    count: u32,
    data: Vec<u8>,
}

fn ascii(tag: u16, s: &str) -> NewEntry {
    let mut data: Vec<u8> = s.bytes().filter(|b| *b != 0).collect();
    data.push(0);
    NewEntry {
        tag,
        typ: 2,
        count: data.len() as u32,
        data,
    }
}

fn short(tag: u16, v: u16) -> NewEntry {
    NewEntry {
        tag,
        typ: 3,
        count: 1,
        data: v.to_le_bytes().to_vec(),
    }
}

fn long(tag: u16, v: u32) -> NewEntry {
    NewEntry {
        tag,
        typ: 4,
        count: 1,
        data: v.to_le_bytes().to_vec(),
    }
}

fn rationals(tag: u16, vals: &[(u32, u32)]) -> NewEntry {
    let mut data = Vec::new();
    for (n, d) in vals {
        data.extend(n.to_le_bytes());
        data.extend(d.to_le_bytes());
    }
    NewEntry {
        tag,
        typ: 5,
        count: vals.len() as u32,
        data,
    }
}

/// Approximates `x >= 0` as a fraction with a bounded denominator.
fn frac(x: f64) -> (u32, u32) {
    let d = if x < 1000.0 { 10_000u32 } else { 100 };
    (
        ((x * d as f64).round() as u64).min(u32::MAX as u64) as u32,
        d,
    )
}

fn ifd_size(entries: &[NewEntry]) -> usize {
    2 + 12 * entries.len()
        + 4
        + entries
            .iter()
            .filter(|e| e.data.len() > 4)
            .map(|e| e.data.len() + e.data.len() % 2)
            .sum::<usize>()
}

/// Little-endian IFD + its out-of-line data, placed at TIFF offset `base`.
fn build_ifd(mut entries: Vec<NewEntry>, base: usize) -> Vec<u8> {
    entries.sort_by_key(|e| e.tag);
    let n = entries.len();
    let mut out = Vec::new();
    out.extend((n as u16).to_le_bytes());
    let mut extra: Vec<u8> = Vec::new();
    let data_start = base + 2 + 12 * n + 4;
    for e in &entries {
        out.extend(e.tag.to_le_bytes());
        out.extend(e.typ.to_le_bytes());
        out.extend(e.count.to_le_bytes());
        if e.data.len() <= 4 {
            let mut v = e.data.clone();
            v.resize(4, 0);
            out.extend(v);
        } else {
            out.extend(((data_start + extra.len()) as u32).to_le_bytes());
            extra.extend(&e.data);
            if extra.len() % 2 == 1 {
                extra.push(0);
            }
        }
    }
    out.extend(0u32.to_le_bytes());
    out.extend(extra);
    out
}

/// Builds a small EXIF block from catalog metadata (for sources that are not JPEG).
pub fn synth_exif(md: &Metadata, dims: (u32, u32), strip_gps: bool) -> Option<Vec<u8>> {
    let mut ifd0: Vec<NewEntry> = Vec::new();
    if let Some(m) = &md.camera_make {
        ifd0.push(ascii(0x010F, m));
    }
    if let Some(m) = &md.camera_model {
        ifd0.push(ascii(0x0110, m));
    }
    let mut exif: Vec<NewEntry> = vec![long(0xA002, dims.0), long(0xA003, dims.1)];
    if let Some(ms) = md.taken_at_ms {
        let local = ms + md.taken_at_offset_min.unwrap_or(0) as i64 * 60_000;
        let secs = local.div_euclid(1000);
        let (y, mo, d) = crate::export::civil_from_days(secs.div_euclid(86_400));
        let tod = secs.rem_euclid(86_400);
        let s = format!(
            "{y:04}:{mo:02}:{d:02} {:02}:{:02}:{:02}",
            tod / 3600,
            tod % 3600 / 60,
            tod % 60
        );
        exif.push(ascii(0x9003, &s));
        exif.push(ascii(0x9004, &s));
    }
    if let Some(v) = md.shutter_s.filter(|v| *v > 0.0) {
        let (n, d) = if v < 1.0 {
            (1, (1.0 / v).round().max(1.0) as u32)
        } else {
            frac(v as f64)
        };
        exif.push(rationals(0x829A, &[(n, d)]));
    }
    if let Some(v) = md.aperture.filter(|v| *v > 0.0) {
        exif.push(rationals(0x829D, &[frac(v as f64)]));
    }
    if let Some(v) = md.focal_mm.filter(|v| *v > 0.0) {
        exif.push(rationals(0x920A, &[frac(v as f64)]));
    }
    if let Some(v) = md.iso {
        exif.push(short(0x8827, v.min(u16::MAX as u32) as u16));
    }
    if let Some(l) = &md.lens {
        exif.push(ascii(0xA434, l));
    }
    let mut gps: Vec<NewEntry> = Vec::new();
    if !strip_gps {
        if let (Some(lat), Some(lon)) = (md.gps_lat, md.gps_lon) {
            let dms = |v: f64| {
                let v = v.abs();
                let deg = v.floor();
                let min = ((v - deg) * 60.0).floor();
                let sec = ((v - deg) * 60.0 - min) * 60.0;
                [(deg as u32, 1), (min as u32, 1), frac(sec)]
            };
            gps.push(ascii(0x0001, if lat >= 0.0 { "N" } else { "S" }));
            gps.push(rationals(0x0002, &dms(lat)));
            gps.push(ascii(0x0003, if lon >= 0.0 { "E" } else { "W" }));
            gps.push(rationals(0x0004, &dms(lon)));
        }
    }
    if ifd0.is_empty() && md.taken_at_ms.is_none() && gps.is_empty() {
        return None;
    }
    // pointers: IFD0 -> Exif IFD -> GPS IFD, laid out back to back after the header
    let n_ptrs = 1 + usize::from(!gps.is_empty());
    let ifd0_len = ifd_size(&ifd0) + 12 * n_ptrs;
    let exif_off = 8 + ifd0_len;
    let gps_off = exif_off + ifd_size(&exif);
    ifd0.push(long(0x8769, exif_off as u32));
    if !gps.is_empty() {
        ifd0.push(long(0x8825, gps_off as u32));
    }
    let mut out = b"II*\0".to_vec();
    out.extend(8u32.to_le_bytes());
    out.extend(build_ifd(ifd0, 8));
    debug_assert_eq!(out.len(), exif_off);
    out.extend(build_ifd(exif, exif_off));
    if !gps.is_empty() {
        debug_assert_eq!(out.len(), gps_off);
        out.extend(build_ifd(gps, gps_off));
    }
    (out.len() <= MAX_EXIF).then_some(out)
}

// ------------------------------------------------------------------ export entry point

/// Returns `encoded` (a baseline JPEG of `dims` pixels rendered from `source`) with the source's
/// EXIF and an sRGB ICC profile embedded. `metadata` supplies what a non-JPEG source cannot
/// provide itself. Never fails: metadata is best effort.
pub fn finalize_export(
    encoded: Vec<u8>,
    source: &Path,
    source_is_jpeg: bool,
    metadata: Option<&Metadata>,
    dims: (u32, u32),
    strip_gps: bool,
) -> Vec<u8> {
    let exif = if source_is_jpeg {
        read_head(source)
            .and_then(|h| extract_exif(&h))
            .and_then(|t| prepare_exif(&t, dims, strip_gps))
    } else {
        None
    }
    .or_else(|| metadata.and_then(|m| synth_exif(m, dims, strip_gps)));
    insert_segments(&encoded, exif.as_deref(), Some(srgb_icc()))
}

fn read_head(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut buf = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(SCAN_BYTES)
        .read_to_end(&mut buf)
        .ok()?;
    Some(buf)
}

/// An EXIF TIFF block like a camera writes it (tests only): Make/Model, orientation, an Exif
/// IFD with the capture time, a GPS IFD (when asked) and an IFD1 with a fake thumbnail. `be`
/// selects big-endian ("MM") byte order.
#[cfg(any(test, feature = "testutil"))]
pub fn build_test_exif(orientation: u16, be: bool, with_gps: bool) -> Vec<u8> {
    struct E {
        tag: u16,
        typ: u16,
        count: u32,
        data: Vec<u8>,
    }
    let w16 = |v: u16| if be { v.to_be_bytes() } else { v.to_le_bytes() };
    let w32 = |v: u32| if be { v.to_be_bytes() } else { v.to_le_bytes() };
    let asc = |tag: u16, s: &str| {
        let mut d = s.as_bytes().to_vec();
        d.push(0);
        E {
            tag,
            typ: 2,
            count: d.len() as u32,
            data: d,
        }
    };
    let sh = |tag: u16, v: u16| E {
        tag,
        typ: 3,
        count: 1,
        data: w16(v).to_vec(),
    };
    let lg = |tag: u16, v: u32| E {
        tag,
        typ: 4,
        count: 1,
        data: w32(v).to_vec(),
    };
    let rat = |tag: u16, vals: &[(u32, u32)]| {
        let mut d = Vec::new();
        for (n, m) in vals {
            d.extend(w32(*n));
            d.extend(w32(*m));
        }
        E {
            tag,
            typ: 5,
            count: vals.len() as u32,
            data: d,
        }
    };
    let size = |es: &[E]| {
        2 + 12 * es.len()
            + 4
            + es.iter()
                .filter(|e| e.data.len() > 4)
                .map(|e| e.data.len() + e.data.len() % 2)
                .sum::<usize>()
    };
    // `next` = offset of the next IFD; thumbnail offsets are patched by the caller via `extra`
    let build = |mut es: Vec<E>, base: usize, next: u32| {
        es.sort_by_key(|e| e.tag);
        let mut out = Vec::new();
        out.extend(w16(es.len() as u16));
        let mut extra: Vec<u8> = Vec::new();
        let start = base + 2 + 12 * es.len() + 4;
        for e in &es {
            out.extend(w16(e.tag));
            out.extend(w16(e.typ));
            out.extend(w32(e.count));
            if e.data.len() <= 4 {
                let mut v = e.data.clone();
                v.resize(4, 0);
                out.extend(v);
            } else {
                out.extend(w32((start + extra.len()) as u32));
                extra.extend(&e.data);
                if extra.len() % 2 == 1 {
                    extra.push(0);
                }
            }
        }
        out.extend(w32(next));
        out.extend(extra);
        out
    };
    // layout: header | IFD0 | Exif IFD | GPS IFD | IFD1 | thumbnail bytes
    let thumb = b"\xFF\xD8THUMBNAIL-OF-THE-UNEDITED-PICTURE\xFF\xD9".to_vec();
    let ifd0 = |exif: u32, gps: u32| {
        let mut v = vec![
            asc(0x010F, "ACME Corp"),
            asc(0x0110, "Model One"),
            sh(0x0112, orientation),
            lg(0x8769, exif),
        ];
        if with_gps {
            v.push(lg(0x8825, gps));
        }
        v
    };
    let exif = || {
        vec![
            asc(0x9003, "2024:02:29 12:30:45"),
            sh(0x8827, 200),
            lg(0xA002, 4000),
            lg(0xA003, 3000),
        ]
    };
    let gps = || {
        vec![
            asc(0x0001, "N"),
            rat(0x0002, &[(48, 1), (51, 1), (30, 1)]),
            asc(0x0003, "E"),
            rat(0x0004, &[(2, 1), (21, 1), (7, 1)]),
        ]
    };
    let ifd0_len = size(&ifd0(0, 0));
    let exif_off = 8 + ifd0_len;
    let gps_off = exif_off + size(&exif());
    let ifd1_off = gps_off + if with_gps { size(&gps()) } else { 0 };
    let ifd1_len = size(&[lg(0x0201, 0), lg(0x0202, 0)]);
    let thumb_off = ifd1_off + ifd1_len;
    let mut out = if be {
        b"MM\0*".to_vec()
    } else {
        b"II*\0".to_vec()
    };
    out.extend(w32(8));
    out.extend(build(
        ifd0(exif_off as u32, gps_off as u32),
        8,
        ifd1_off as u32,
    ));
    out.extend(build(exif(), exif_off, 0));
    if with_gps {
        out.extend(build(gps(), gps_off, 0));
    }
    out.extend(build(
        vec![lg(0x0201, thumb_off as u32), lg(0x0202, thumb.len() as u32)],
        ifd1_off,
        0,
    ));
    out.extend(&thumb);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny valid JPEG (via the `image` crate) with `exif` inserted.
    fn jpeg_with(exif: Option<&[u8]>) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(8, 8, image::Rgb([10, 200, 30]));
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut bytes)
            .encode_image(&img)
            .unwrap();
        insert_segments(&bytes, exif, None)
    }

    fn parse(jpeg: &[u8]) -> Metadata {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.jpg");
        std::fs::write(&p, jpeg).unwrap();
        ip_imaging::read_metadata(&p, ip_imaging::ImageFormat::Jpeg).unwrap()
    }

    #[test]
    fn exif_is_copied_without_orientation_thumbnail_and_gps_on_request() {
        for be in [false, true] {
            let src = build_test_exif(6, be, true);
            let md = parse(&jpeg_with(Some(&src)));
            assert_eq!(md.orientation, 6);
            assert!(md.gps_lat.is_some());
            for strip in [false, true] {
                let out = prepare_exif(&src, (1000, 750), strip).unwrap();
                let md = parse(&jpeg_with(Some(&out)));
                assert_eq!(md.orientation, 1, "be={be}");
                assert_eq!(md.camera_make.as_deref(), Some("ACME Corp"));
                assert_eq!(md.camera_model.as_deref(), Some("Model One"));
                assert!(md.taken_at_ms.is_some());
                assert_eq!(md.iso, Some(200));
                assert_eq!(md.gps_lat.is_some(), !strip, "be={be} strip={strip}");
                // the unedited thumbnail is gone, and GPS bytes with it when stripped
                assert!(!out.windows(9).any(|w| w == b"THUMBNAIL"));
                let n = out.len();
                assert!(n <= src.len());
            }
            let stripped = prepare_exif(&src, (1, 1), true).unwrap();
            // no GPS coordinate bytes survive: the rationals (48/1, 51/1, 30/1) were zeroed
            let le48: Vec<u8> = [48u32, 1, 51, 1, 30, 1]
                .iter()
                .flat_map(|v| if be { v.to_be_bytes() } else { v.to_le_bytes() })
                .collect();
            assert!(!stripped.windows(le48.len()).any(|w| w == le48.as_slice()));
        }
    }

    #[test]
    fn pixel_dimensions_follow_the_export() {
        let src = build_test_exif(1, false, false);
        let out = prepare_exif(&src, (1234, 567), false).unwrap();
        let find = |tag: u16| {
            // PixelXDimension / PixelYDimension are LONG entries of the Exif IFD
            let b = tag.to_le_bytes();
            let i = out.windows(2).position(|w| w == b).unwrap();
            u32::from_le_bytes(out[i + 8..i + 12].try_into().unwrap())
        };
        assert_eq!((find(0xA002), find(0xA003)), (1234, 567));
    }

    #[test]
    fn malformed_exif_is_rejected_not_copied() {
        assert!(prepare_exif(b"garbage", (1, 1), false).is_none());
        let mut bad = build_test_exif(1, false, false);
        bad.truncate(20);
        assert!(prepare_exif(&bad, (1, 1), false).is_none());
    }

    #[test]
    fn icc_profile_is_well_formed() {
        let p = srgb_icc();
        assert_eq!(&p[36..40], b"acsp");
        assert_eq!(
            u32::from_be_bytes(p[0..4].try_into().unwrap()) as usize,
            p.len()
        );
        assert_eq!(&p[12..16], b"mntr");
        assert_eq!(&p[16..20], b"RGB ");
        let n = u32::from_be_bytes(p[128..132].try_into().unwrap()) as usize;
        assert_eq!(n, 9);
        for i in 0..n {
            let e = 132 + 12 * i;
            let off = u32::from_be_bytes(p[e + 4..e + 8].try_into().unwrap()) as usize;
            let len = u32::from_be_bytes(p[e + 8..e + 12].try_into().unwrap()) as usize;
            assert!(off + len <= p.len(), "tag {i} out of range");
            assert_eq!(off % 4, 0);
        }
    }

    #[test]
    fn synthesised_exif_parses_back() {
        let md = Metadata {
            camera_make: Some("ACME".into()),
            camera_model: Some("X100".into()),
            taken_at_ms: Some(1_709_164_800_000 + 3_661_000),
            iso: Some(400),
            aperture: Some(2.8),
            shutter_s: Some(0.004),
            focal_mm: Some(35.0),
            gps_lat: Some(48.8584),
            gps_lon: Some(-2.2945),
            ..Default::default()
        };
        let t = synth_exif(&md, (640, 480), false).unwrap();
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xD9];
        jpeg = insert_segments(&jpeg, Some(&t), None);
        let back = extract_exif(&jpeg).unwrap();
        assert_eq!(back, t);
        let no_gps = synth_exif(&md, (640, 480), true).unwrap();
        assert!(no_gps.len() < t.len());
    }

    #[test]
    fn segments_go_after_jfif() {
        let jpeg = [
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x4A, 0x46, 0xFF, 0xDA, 0x00, 0x02, 0xFF, 0xD9,
        ];
        let out = insert_segments(
            &jpeg,
            Some(b"II*\0\x08\0\0\0\0\0\0\0\0\0"),
            Some(srgb_icc()),
        );
        assert_eq!(&out[2..4], &[0xFF, 0xE0]);
        assert_eq!(&out[8..10], &[0xFF, 0xE1]);
        assert!(out.windows(11).any(|w| w == b"ICC_PROFILE"));
    }
}
