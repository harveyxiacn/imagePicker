//! JPEG metadata of exports: the source's EXIF (minus the orientation that the render already
//! baked in, the embedded thumbnail of the unedited picture and, on request, the GPS position),
//! its XMP packet and IPTC record (cleaned up the same way), and an sRGB ICC profile that
//! matches the pixels (decoding converts sources tagged with another colour space to sRGB).
//!
//! No external dependencies: the EXIF TIFF block is edited in place (offsets of everything
//! else, MakerNotes included, stay valid) and whatever is removed is zeroed so nothing of it
//! survives in the file. RAW, TIFF and HEIF sources contribute the block `ip-imaging` finds or
//! rebuilds in them; sources without EXIF (PNG, WebP...) get a small block synthesised from
//! the catalog metadata instead.

use std::path::Path;
use std::sync::OnceLock;

use ip_imaging::Metadata;

use crate::xmp::doc::{NS_CRS, NS_DARKTABLE, NS_EXIF, NS_TIFF, NS_XMP, NS_XMP_NOTE};
use crate::xmp::jpeg::{MAX_XMP, XMP_HEADER};
use crate::xmp::Xmp;

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
    ICC.get_or_init(|| {
        build_matrix_icc(
            "sRGB IEC61966-2.1",
            [
                [0.436_074_7, 0.222_504_5, 0.013_932_2],
                [0.385_064_9, 0.716_878_6, 0.097_104_5],
                [0.143_080_4, 0.060_616_9, 0.714_173_3],
            ],
            &srgb_curve(),
            Some((b"IEC ", b"sRGB")),
        )
    })
}

/// The sRGB transfer function as a 1024-entry ICC `curv` table.
fn srgb_curve() -> Vec<u16> {
    (0..1024u32)
        .map(|i| {
            let v = i as f64 / 1023.0;
            let lin = if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            };
            (lin * 65535.0).round() as u16
        })
        .collect()
}

/// A Display P3 profile in the same style as [`srgb_icc`] (tests only).
#[cfg(any(test, feature = "testutil"))]
pub fn display_p3_icc() -> Vec<u8> {
    build_matrix_icc(
        "Display P3",
        [
            [0.515_102, 0.241_182, -0.001_050],
            [0.291_965, 0.692_236, 0.041_882],
            [0.157_153, 0.066_582, 0.784_378],
        ],
        &srgb_curve(),
        None,
    )
}

/// An Adobe RGB (1998) profile like Adobe's own: pure gamma 2.2 (563/256) curve (tests only).
#[cfg(any(test, feature = "testutil"))]
pub fn adobe_rgb_icc() -> Vec<u8> {
    build_matrix_icc(
        "Adobe RGB (1998)",
        [
            [0.609_741, 0.311_112, 0.019_470],
            [0.205_276, 0.625_671, 0.060_867],
            [0.149_185, 0.063_217, 0.744_568],
        ],
        &[0x0233],
        None,
    )
}

/// A matrix/TRC display profile: `colorants` are the D50-adapted XYZ of red, green and blue,
/// `curve` is the `curv` table the three channels share (one entry = a u8.8 gamma),
/// `device` the manufacturer / model signatures.
fn build_matrix_icc(
    desc: &str,
    colorants: [[f64; 3]; 3],
    curve: &[u16],
    device: Option<(&[u8; 4], &[u8; 4])>,
) -> Vec<u8> {
    let curve = {
        let mut t = b"curv\0\0\0\0".to_vec();
        t.extend((curve.len() as u32).to_be_bytes());
        for v in curve {
            t.extend(v.to_be_bytes());
        }
        t
    };
    let mut cprt = b"text\0\0\0\0".to_vec();
    cprt.extend(b"No copyright, use freely\0");
    let [r, g, b] = colorants;
    // (signature, data); the three curves share one table
    let tags: Vec<([u8; 4], Vec<u8>)> = vec![
        (*b"desc", text_desc(desc)),
        (*b"cprt", cprt),
        (*b"wtpt", xyz_tag(0.9642, 1.0, 0.8249)),
        (*b"rXYZ", xyz_tag(r[0], r[1], r[2])),
        (*b"gXYZ", xyz_tag(g[0], g[1], g[2])),
        (*b"bXYZ", xyz_tag(b[0], b[1], b[2])),
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
    if let Some((make, model)) = device {
        p[48..52].copy_from_slice(make);
        p[52..56].copy_from_slice(model);
    }
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
    insert_metadata(jpeg, exif_tiff, None, icc, None)
}

/// Inserts EXIF (APP1), XMP (APP1), ICC (APP2) and Photoshop image resources (APP13), in that
/// order, right after the SOI / JFIF header. A part too large for one segment is left out.
pub fn insert_metadata(
    jpeg: &[u8],
    exif_tiff: Option<&[u8]>,
    xmp: Option<&str>,
    icc: Option<&[u8]>,
    irb: Option<&[u8]>,
) -> Vec<u8> {
    if jpeg.len() < 2 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return jpeg.to_vec();
    }
    let mut pos = 2;
    while pos + 4 <= jpeg.len() && jpeg[pos] == 0xFF && jpeg[pos + 1] == 0xE0 {
        pos += 2 + u16::from_be_bytes([jpeg[pos + 2], jpeg[pos + 3]]) as usize;
    }
    let pos = pos.min(jpeg.len());
    /// (marker, segment header, payload)
    type Part<'a> = (u8, &'a [u8], Option<&'a [u8]>);
    let parts: [Part; 4] = [
        (0xE1, b"Exif\0\0", exif_tiff),
        (0xE1, XMP_HEADER, xmp.map(str::as_bytes)),
        (0xE2, b"ICC_PROFILE\0\x01\x01", icc),
        (0xED, PHOTOSHOP, irb),
    ];
    let mut out = Vec::with_capacity(jpeg.len() + 8192);
    out.extend(&jpeg[..pos]);
    for (marker, header, body) in parts {
        let Some(body) = body else { continue };
        let mut p = header.to_vec();
        p.extend(body);
        if p.len() + 2 <= 0xFFFF {
            out.extend(segment(marker, &p));
        }
    }
    out.extend(&jpeg[pos..]);
    out
}

// ------------------------------------------------------------------ IPTC (Photoshop APP13)

const PHOTOSHOP: &[u8] = b"Photoshop 3.0\0";
/// Image resources kept in exports: the IPTC-IIM record (0x0404) and its digest (0x0425).
/// The rest (thumbnails of the unedited picture, paths, slices, print settings...) is dropped.
const IRB_KEEP: [u16; 2] = [0x0404, 0x0425];

/// The Photoshop image resource block of a JPEG: its APP13 payloads joined, without headers.
pub fn extract_irb(jpeg: &[u8]) -> Option<Vec<u8>> {
    let segs = crate::xmp::jpeg::segments(jpeg)?;
    let mut out = Vec::new();
    for s in segs.iter().filter(|s| s.marker == 0xED) {
        if let Some(rest) = jpeg[s.start + 4..s.end].strip_prefix(PHOTOSHOP) {
            out.extend_from_slice(rest);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// The IPTC resources of an image resource block, re-serialised; `None` without IPTC.
pub fn iptc_resources(irb: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut has_iptc = false;
    let mut p = 0usize;
    while p + 12 <= irb.len() {
        let sig = &irb[p..p + 4];
        if !matches!(sig, b"8BIM" | b"MeSa" | b"PHUT" | b"AgHg" | b"DCSR") {
            break;
        }
        let id = u16::from_be_bytes([irb[p + 4], irb[p + 5]]);
        // Pascal name, padded to an even length
        let size_at = p + 6 + ((irb[p + 6] as usize + 2) & !1);
        let Some(size) = irb.get(size_at..size_at + 4) else {
            break;
        };
        let size = u32::from_be_bytes(size.try_into().expect("4 bytes")) as usize;
        let data_at = size_at + 4;
        let Some(data) = data_at
            .checked_add(size)
            .and_then(|end| irb.get(data_at..end))
        else {
            break;
        };
        if sig == b"8BIM" && IRB_KEEP.contains(&id) {
            has_iptc |= id == 0x0404;
            out.extend_from_slice(b"8BIM");
            out.extend_from_slice(&id.to_be_bytes());
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(&(size as u32).to_be_bytes());
            out.extend_from_slice(data);
            if size % 2 == 1 {
                out.push(0);
            }
        }
        p = data_at + size + size % 2;
    }
    has_iptc.then_some(out)
}

// ------------------------------------------------------------------ XMP

const NS_MWG_RS: &str = "http://www.metadataworkinggroup.com/schemas/regions/";
const NS_MS_PHOTO: &str = "http://ns.microsoft.com/photo/1.2/";
const NS_GCAMERA: &str = "http://ns.google.com/photos/1.0/camera/";
const NS_GCONTAINER: &str = "http://ns.google.com/photos/1.0/container/";
const NS_GDEPTH: &str = "http://ns.google.com/photos/1.0/depthmap/";
const NS_GIMAGE: &str = "http://ns.google.com/photos/1.0/image/";
const NS_HDR_GAIN_MAP: &str = "http://ns.adobe.com/hdr-gain-map/1.0/";
const NS_APPLE_GAIN_MAP: &str = "http://ns.apple.com/HDRGainMap/1.0/";

/// XMP properties that do not carry over to a rendered export: they describe the unedited
/// file (orientation, size, thumbnails, face regions in its geometry), its development
/// (Camera Raw settings, darktable history: applied again they would develop the export a
/// second time), or data that only the original carries (extended XMP, depth maps, motion
/// photo video, HDR gain maps). With `strip_gps` every `GPS*` property goes too.
fn dropped_from_export(ns: &str, local: &str, strip_gps: bool) -> bool {
    if strip_gps
        && local
            .get(..3)
            .is_some_and(|p| p.eq_ignore_ascii_case("gps"))
    {
        return true;
    }
    match ns {
        NS_TIFF => matches!(local, "Orientation" | "ImageWidth" | "ImageLength"),
        NS_EXIF => matches!(local, "PixelXDimension" | "PixelYDimension"),
        NS_XMP => local == "Thumbnails",
        NS_XMP_NOTE => local == "HasExtendedXMP",
        NS_DARKTABLE => local != "colorlabels",
        NS_GCAMERA => local.starts_with("MotionPhoto") || local.starts_with("MicroVideo"),
        NS_MS_PHOTO => local == "RegionInfo",
        NS_CRS | NS_MWG_RS | NS_GCONTAINER | NS_GDEPTH | NS_GIMAGE | NS_HDR_GAIN_MAP
        | NS_APPLE_GAIN_MAP => true,
        _ => false,
    }
}

/// The source's XMP packet as it goes into an export (see [`dropped_from_export`]). `None`
/// when it cannot be parsed (and so not be cleaned up) or does not fit one segment.
pub fn export_xmp(packet: &str, strip_gps: bool) -> Option<String> {
    let mut x = Xmp::parse(packet).ok()?;
    x.remove_properties(&|ns, local| dropped_from_export(ns, local, strip_gps));
    let out = x.serialize();
    (out.len() <= MAX_XMP).then_some(out)
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

/// What a source contributes to the metadata of its re-encoded export.
#[derive(Debug, Clone, Default)]
pub struct SourceMeta {
    /// EXIF TIFF block, as stored or as rebuilt by `ip_imaging::read_exif`.
    pub exif: Option<Vec<u8>>,
    /// Main XMP packet: embedded (JPEG) or from the sidecar.
    pub xmp: Option<String>,
    /// Photoshop image resources of the APP13 segment(s) (JPEG): the IPTC-IIM record.
    pub irb: Option<Vec<u8>>,
    /// Catalog metadata, synthesised into EXIF when there is no usable EXIF block.
    pub metadata: Option<Metadata>,
}

impl SourceMeta {
    /// EXIF, XMP and IPTC from the header segments of a JPEG file.
    pub fn of_jpeg_file(path: &Path) -> SourceMeta {
        let Some(head) = read_head(path) else {
            return SourceMeta::default();
        };
        SourceMeta {
            exif: extract_exif(&head),
            xmp: crate::xmp::jpeg::extract(&head),
            irb: extract_irb(&head),
            metadata: None,
        }
    }
}

/// Returns `encoded` (a baseline JPEG of `dims` sRGB pixels rendered from the source) with the
/// source's EXIF, XMP and IPTC (cleaned up as described in the module docs) and an sRGB ICC
/// profile embedded. Never fails: metadata is best effort.
pub fn finalize_export(
    encoded: Vec<u8>,
    source: &SourceMeta,
    dims: (u32, u32),
    strip_gps: bool,
) -> Vec<u8> {
    let exif = source
        .exif
        .as_deref()
        .and_then(|t| prepare_exif(t, dims, strip_gps))
        .or_else(|| {
            source
                .metadata
                .as_ref()
                .and_then(|m| synth_exif(m, dims, strip_gps))
        });
    let xmp = source.xmp.as_deref().and_then(|x| export_xmp(x, strip_gps));
    let irb = source.irb.as_deref().and_then(iptc_resources);
    insert_metadata(
        &encoded,
        exif.as_deref(),
        xmp.as_deref(),
        Some(srgb_icc()),
        irb.as_deref(),
    )
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

/// An uncompressed `w x h` RGB TIFF filled with `rgb`, carrying EXIF the way cameras and raw
/// converters write it into TIFF-structured files (tests only): Make / Model / Copyright in IFD0
/// next to the strip layout, an Exif IFD with capture time and UTC offset, ISO, body serial,
/// lens and a MakerNote, and a GPS IFD.
#[cfg(any(test, feature = "testutil"))]
pub fn build_test_tiff(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    let maker_note = b"MAKERNOTE-WITH-PRIVATE-OFFSETS";
    let exif = vec![
        ascii(0x9003, "2024:02:29 12:30:45"),
        ascii(0x9011, "+09:00"),
        short(0x8827, 200),
        ascii(0xA431, "SN-42"),
        ascii(0xA434, "Lens 35mm"),
        NewEntry {
            tag: 0x927C,
            typ: 7,
            count: maker_note.len() as u32,
            data: maker_note.to_vec(),
        },
    ];
    let gps = vec![
        ascii(0x0001, "N"),
        rationals(0x0002, &[(48, 1), (51, 1), (30, 1)]),
        ascii(0x0003, "E"),
        rationals(0x0004, &[(2, 1), (21, 1), (7, 1)]),
    ];
    let ifd0 = |strip: u32, exif_off: u32, gps_off: u32| {
        vec![
            long(0x0100, w),
            long(0x0101, h),
            NewEntry {
                tag: 0x0102,
                typ: 3,
                count: 3,
                data: [8u16, 8, 8].iter().flat_map(|v| v.to_le_bytes()).collect(),
            },
            short(0x0103, 1),
            short(0x0106, 2),
            ascii(0x010F, "ACME Corp"),
            ascii(0x0110, "Model One"),
            long(0x0111, strip),
            short(0x0112, 1),
            short(0x0115, 3),
            long(0x0116, h),
            long(0x0117, w * h * 3),
            short(0x011C, 1),
            ascii(0x8298, "(c) Test"),
            long(0x8769, exif_off),
            long(0x8825, gps_off),
        ]
    };
    let exif_off = 8 + ifd_size(&ifd0(0, 0, 0));
    let gps_off = exif_off + ifd_size(&exif);
    let strip = gps_off + ifd_size(&gps);
    let mut out = b"II*\0".to_vec();
    out.extend(8u32.to_le_bytes());
    out.extend(build_ifd(
        ifd0(strip as u32, exif_off as u32, gps_off as u32),
        8,
    ));
    out.extend(build_ifd(exif, exif_off));
    out.extend(build_ifd(gps, gps_off));
    debug_assert_eq!(out.len(), strip);
    for _ in 0..w * h {
        out.extend(rgb);
    }
    out
}

/// IPTC-IIM application records (tests only): title, two keywords, city. Odd length.
#[cfg(any(test, feature = "testutil"))]
pub fn build_test_iptc() -> Vec<u8> {
    let mut v = Vec::new();
    for (dataset, value) in [
        (5u8, "Temple at dawn"),
        (25, "kyoto"),
        (25, "temples"),
        (90, "Kyoto"),
    ] {
        v.extend([0x1C, 2, dataset]);
        v.extend((value.len() as u16).to_be_bytes());
        v.extend(value.as_bytes());
    }
    v
}

/// A Photoshop image resource block (tests only): a named thumbnail of the unedited picture
/// (0x040C), the IPTC record of [`build_test_iptc`] (0x0404), its digest (0x0425) and
/// resolution info (0x03ED).
#[cfg(any(test, feature = "testutil"))]
pub fn build_test_irb() -> Vec<u8> {
    let res = |id: u16, name: &[u8], data: &[u8]| {
        let mut v = b"8BIM".to_vec();
        v.extend(id.to_be_bytes());
        v.push(name.len() as u8);
        v.extend(name);
        if name.len().is_multiple_of(2) {
            v.push(0);
        }
        v.extend((data.len() as u32).to_be_bytes());
        v.extend(data);
        if data.len() % 2 == 1 {
            v.push(0);
        }
        v
    };
    let mut irb = res(0x040C, b"thumb", b"\xFF\xD8UNEDITED-THUMBNAIL\xFF\xD9");
    irb.extend(res(0x0404, b"", &build_test_iptc()));
    irb.extend(res(0x0425, b"", &[7u8; 16]));
    irb.extend(res(0x03ED, b"", &[0u8; 16]));
    irb
}

/// An XMP packet as a camera, phone or Lightroom leaves it in a JPEG (tests only): rating,
/// title, creator, keywords, city, GPS (EXIF and DJI style), orientation and size, Camera Raw
/// settings, face regions, a motion-photo flag and an extended-XMP pointer.
#[cfg(any(test, feature = "testutil"))]
pub const TEST_XMP: &str = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Test">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/"
    xmlns:tiff="http://ns.adobe.com/tiff/1.0/"
    xmlns:exif="http://ns.adobe.com/exif/1.0/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    xmlns:xmpNote="http://ns.adobe.com/xmp/note/"
    xmlns:GCamera="http://ns.google.com/photos/1.0/camera/"
    xmlns:drone-dji="http://www.dji.com/drone-dji/1.0/"
    xmp:Rating="4"
    xmp:CreatorTool="Camera Firmware 1.0"
    photoshop:City="Kyoto"
    tiff:Orientation="6"
    tiff:ImageWidth="4000"
    exif:PixelXDimension="4000"
    exif:GPSLatitude="35,0.6N"
    exif:GPSLongitude="135,46.2E"
    drone-dji:GpsLatitude="35.01"
    drone-dji:AbsoluteAltitude="+120.5"
    crs:Exposure2012="+0.35"
    xmpNote:HasExtendedXMP="0123456789ABCDEF0123456789ABCDEF"
    GCamera:MotionPhoto="1"
    GCamera:BurstID="b-1">
   <dc:title><rdf:Alt><rdf:li xml:lang="x-default">Temple at dawn</rdf:li></rdf:Alt></dc:title>
   <dc:creator><rdf:Seq><rdf:li>Jane Doe</rdf:li></rdf:Seq></dc:creator>
   <dc:subject><rdf:Bag><rdf:li>kyoto</rdf:li><rdf:li>temple</rdf:li></rdf:Bag></dc:subject>
   <crs:ToneCurvePV2012><rdf:Seq><rdf:li>0, 0</rdf:li><rdf:li>255, 255</rdf:li></rdf:Seq></crs:ToneCurvePV2012>
   <mwg-rs:Regions xmlns:mwg-rs="http://www.metadataworkinggroup.com/schemas/regions/" rdf:parseType="Resource"><mwg-rs:RegionList><rdf:Bag><rdf:li rdf:parseType="Resource"><mwg-rs:Name>Jane</mwg-rs:Name></rdf:li></rdf:Bag></mwg-rs:RegionList></mwg-rs:Regions>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;

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
    fn test_profiles_are_well_formed_matrix_profiles() {
        for p in [display_p3_icc(), adobe_rgb_icc()] {
            assert_eq!(&p[36..40], b"acsp");
            assert_eq!(
                u32::from_be_bytes(p[0..4].try_into().unwrap()) as usize,
                p.len()
            );
            assert_ne!(p.as_slice(), srgb_icc());
        }
    }

    #[test]
    fn only_the_iptc_resources_are_kept() {
        let (irb, iptc) = (build_test_irb(), build_test_iptc());
        assert_eq!(iptc.len() % 2, 1, "exercises the padding");
        let kept = iptc_resources(&irb).unwrap();
        // 0x0404 then 0x0425, nothing else
        let mut ids = Vec::new();
        let mut p = 0;
        while p + 12 <= kept.len() {
            assert_eq!(&kept[p..p + 4], b"8BIM");
            ids.push(u16::from_be_bytes([kept[p + 4], kept[p + 5]]));
            let size = u32::from_be_bytes(kept[p + 8..p + 12].try_into().unwrap()) as usize;
            if ids.len() == 1 {
                assert_eq!(&kept[p + 12..p + 12 + size], iptc.as_slice());
            }
            p += 12 + size + size % 2;
        }
        assert_eq!(ids, [0x0404, 0x0425]);
        assert_eq!(p, kept.len());
        assert!(!kept.windows(9).any(|w| w == b"UNEDITED-"));
        // no IPTC record -> nothing to keep; garbage and truncation are survived
        assert!(iptc_resources(&irb[..40]).is_none());
        assert!(iptc_resources(b"garbage, not resources").is_none());
    }

    #[test]
    fn export_xmp_keeps_descriptions_and_drops_what_no_longer_applies() {
        for strip in [false, true] {
            let out = export_xmp(TEST_XMP, strip).unwrap();
            let x = Xmp::parse(&out).unwrap();
            let v = crate::xmp::values_of(&x);
            assert_eq!(v.rating, Some(4));
            assert_eq!(v.keywords, ["kyoto", "temple"]);
            for kept in [
                "Temple at dawn",
                "Jane Doe",
                "photoshop:City=\"Kyoto\"",
                "xmp:CreatorTool=",
                "drone-dji:AbsoluteAltitude=",
                "GCamera:BurstID=",
                "<?xpacket end=\"w\"?>",
            ] {
                assert!(out.contains(kept), "{kept} (strip={strip}): {out}");
            }
            for gone in [
                "tiff:Orientation",
                "tiff:ImageWidth",
                "exif:PixelXDimension",
                "crs:Exposure2012",
                "crs:ToneCurvePV2012",
                "xmpNote:HasExtendedXMP",
                "GCamera:MotionPhoto",
                "mwg-rs:Regions",
            ] {
                assert!(!out.contains(gone), "{gone} (strip={strip}): {out}");
            }
            for gps in [
                "exif:GPSLatitude",
                "exif:GPSLongitude",
                "drone-dji:GpsLatitude",
            ] {
                assert_eq!(out.contains(gps), !strip, "{gps} (strip={strip})");
            }
        }
        assert!(export_xmp("<not xmp", false).is_none());
    }

    #[test]
    fn xmp_and_iptc_of_a_jpeg_source_reach_the_export() {
        let dir = tempfile::tempdir().unwrap();
        let src = insert_metadata(
            &jpeg_with(None),
            Some(&build_test_exif(6, false, true)),
            Some(TEST_XMP),
            None,
            Some(&build_test_irb()),
        );
        let p = dir.path().join("s.jpg");
        std::fs::write(&p, &src).unwrap();
        let meta = SourceMeta::of_jpeg_file(&p);
        assert!(meta.exif.is_some() && meta.xmp.is_some() && meta.irb.is_some());
        let iptc = build_test_iptc();
        for strip in [false, true] {
            let out = finalize_export(jpeg_with(None), &meta, (8, 8), strip);
            let segs = crate::xmp::jpeg::segments(&out).unwrap();
            let order: Vec<u8> = segs.iter().map(|s| s.marker).collect();
            assert_eq!(
                order[..5],
                [0xE0, 0xE1, 0xE1, 0xE2, 0xED],
                "APP0 EXIF XMP ICC IPTC"
            );
            let xmp = crate::xmp::jpeg::extract(&out).unwrap();
            assert_eq!(xmp.contains("exif:GPSLatitude"), !strip);
            assert!(!xmp.contains("tiff:Orientation"));
            let irb = extract_irb(&out).unwrap();
            assert!(irb.windows(iptc.len()).any(|w| w == iptc.as_slice()));
            let md = parse(&out);
            assert_eq!(md.gps_lat.is_some(), !strip);
            assert_eq!(md.orientation, 1);
        }
        // a source without any of it gets just the profile
        let bare = finalize_export(jpeg_with(None), &SourceMeta::default(), (8, 8), false);
        let order: Vec<u8> = crate::xmp::jpeg::segments(&bare)
            .unwrap()
            .iter()
            .map(|s| s.marker)
            .collect();
        assert_eq!(order[..2], [0xE0, 0xE2]);
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
