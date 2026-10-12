//! Output formats of exports and metadata clean-up of copied originals (P0-8,
//! `docs/api-contract-m5.md` section F).
//!
//! Re-encoded pictures (sRGB pixels) are written as JPEG, PNG, TIFF or WebP with what each
//! container can carry of the source's metadata ([`PreparedMeta`]) and an sRGB ICC profile:
//!
//! | format | EXIF | XMP | IPTC | ICC |
//! |---|---|---|---|---|
//! | JPEG | APP1 | APP1 | APP13 | APP2 |
//! | PNG | `eXIf` | `iTXt` `XML:com.adobe.xmp` | (XMP only) | `iCCP` |
//! | TIFF | IFD0 descriptive tags + Exif / GPS IFDs (no MakerNote) | tag 700 | tag 33723 | tag 34675 |
//! | WebP | `EXIF` | `XMP ` | (XMP only) | `ICCP` |
//!
//! TIFF is 8-bit RGB, uncompressed. WebP is lossless (VP8L): the pure-Rust encoder has no lossy
//! mode, and a side can be at most [`WEBP_MAX_SIDE`] pixels.
//!
//! Originals that keep their pixels (unedited, original size, already in the requested format)
//! are *cleaned* instead when the GPS position or all metadata has to go ([`clean_original`]):
//! JPEG, PNG and WebP are rewritten segment by segment / chunk by chunk, their image data is
//! copied byte for byte.

use std::borrow::Cow;

use ip_imaging::ImageFormat;
use ip_render::RgbImage;
use serde::{Deserialize, Serialize};

use crate::jpegmeta::{self, PreparedMeta};
use crate::xmp::doc::NS_XMP_NOTE;
use crate::xmp::Xmp;

/// Largest width / height of a WebP image.
pub const WEBP_MAX_SIDE: u32 = 16_383;

/// Output format of an export (`"format"` of `POST /api/export`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    /// Unchanged photos are copied in their own format, everything re-encoded is JPEG (the
    /// behaviour before formats could be chosen).
    #[default]
    Original,
    #[serde(alias = "jpg")]
    Jpeg,
    Png,
    #[serde(alias = "tif")]
    Tiff,
    /// Lossless WebP.
    Webp,
}

impl ExportFormat {
    /// The container re-encoded outputs are written in.
    pub fn encoded(self) -> ExportFormat {
        match self {
            ExportFormat::Original => ExportFormat::Jpeg,
            f => f,
        }
    }

    /// File extension of re-encoded outputs.
    pub fn extension(self) -> &'static str {
        match self.encoded() {
            ExportFormat::Png => "png",
            ExportFormat::Tiff => "tif",
            ExportFormat::Webp => "webp",
            _ => "jpg",
        }
    }

    /// Whether an unchanged source in `source` format can be handed out as it is (copied or
    /// cleaned) rather than re-encoded.
    pub fn keeps(self, source: ImageFormat) -> bool {
        match self {
            ExportFormat::Original => true,
            ExportFormat::Jpeg => source == ImageFormat::Jpeg,
            ExportFormat::Png => source == ImageFormat::Png,
            ExportFormat::Tiff => source == ImageFormat::Tiff,
            ExportFormat::Webp => source == ImageFormat::Webp,
        }
    }

    /// The re-encoding that keeps a source's container (JPEG for the rest).
    pub fn of_source(source: ImageFormat) -> ExportFormat {
        match source {
            ImageFormat::Png => ExportFormat::Png,
            ImageFormat::Tiff => ExportFormat::Tiff,
            ImageFormat::Webp => ExportFormat::Webp,
            _ => ExportFormat::Jpeg,
        }
    }

    /// Largest side the format can store.
    pub fn max_side(self) -> Option<u32> {
        (self.encoded() == ExportFormat::Webp).then_some(WEBP_MAX_SIDE)
    }
}

// ------------------------------------------------------------------ encoding

/// Encodes a rendered sRGB picture as `format` (`Original` = JPEG) with `meta` and an sRGB ICC
/// profile. `quality` applies to JPEG only (PNG, TIFF and WebP are lossless).
pub fn encode(
    img: &RgbImage,
    format: ExportFormat,
    quality: u8,
    meta: &PreparedMeta,
) -> Result<Vec<u8>, String> {
    match format.encoded() {
        ExportFormat::Png => encode_png(img, meta),
        ExportFormat::Tiff => encode_tiff(img, meta),
        ExportFormat::Webp => encode_webp(img, meta),
        _ => {
            let bytes =
                crate::edit::service::encode_jpeg(img, quality).map_err(|e| format!("{e:#}"))?;
            Ok(jpegmeta::with_meta(&bytes, meta))
        }
    }
}

fn encode_png(img: &RgbImage, meta: &PreparedMeta) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;
    let mut out = Vec::with_capacity(img.data.len() / 2 + 8192);
    let mut enc = image::codecs::png::PngEncoder::new(&mut out);
    enc.set_icc_profile(jpegmeta::srgb_icc().to_vec())
        .map_err(|e| e.to_string())?;
    if let Some(exif) = &meta.exif {
        enc.set_exif_metadata(exif.clone())
            .map_err(|e| e.to_string())?;
    }
    enc.write_image(
        &img.data,
        img.width,
        img.height,
        image::ExtendedColorType::Rgb8,
    )
    .map_err(|e| e.to_string())?;
    match &meta.xmp {
        Some(xmp) => png_insert_before_idat(&out, &png_chunk(b"iTXt", &itxt_xmp(xmp)))
            .ok_or_else(|| "the PNG encoder wrote no image data".to_string()),
        None => Ok(out),
    }
}

fn encode_webp(img: &RgbImage, meta: &PreparedMeta) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;
    if img.width.max(img.height) > WEBP_MAX_SIDE {
        return Err(format!(
            "WebP images can be at most {WEBP_MAX_SIDE} pixels wide and high"
        ));
    }
    let mut out = Vec::with_capacity(img.data.len() / 2 + 8192);
    let mut enc = image::codecs::webp::WebPEncoder::new_lossless(&mut out);
    enc.set_icc_profile(jpegmeta::srgb_icc().to_vec())
        .map_err(|e| e.to_string())?;
    if let Some(exif) = &meta.exif {
        enc.set_exif_metadata(exif.clone())
            .map_err(|e| e.to_string())?;
    }
    enc.write_image(
        &img.data,
        img.width,
        img.height,
        image::ExtendedColorType::Rgb8,
    )
    .map_err(|e| e.to_string())?;
    match &meta.xmp {
        Some(xmp) => webp_with_xmp(&out, xmp.as_bytes())
            .ok_or_else(|| "the WebP encoder wrote no extended header".to_string()),
        None => Ok(out),
    }
}

/// Rows per strip are chosen so that a strip holds about this many bytes.
const STRIP_BYTES: usize = 256 << 10;
/// Text tags of the source's IFD0 that describe the picture (not its layout) and so carry over.
const TIFF_IFD0_TEXT: [u16; 8] = [
    0x010E, // ImageDescription
    0x010F, // Make
    0x0110, // Model
    0x0131, // Software
    0x0132, // DateTime
    0x013B, // Artist
    0x013C, // HostComputer
    0x8298, // Copyright
];
/// Exif IFD tags left out of TIFF exports: the MakerNote (its offsets point into the source
/// file), the Interoperability IFD pointer, padding and the offset schema.
const TIFF_EXIF_DROP: [u16; 4] = [0x927C, 0xA005, 0xEA1C, 0xEA1D];

/// A baseline 8-bit RGB TIFF (uncompressed strips) in the byte order of the source's EXIF,
/// whose values are therefore copied unchanged: IFD0 = layout + resolution + descriptive tags
/// + XMP (700) + IPTC (33723) + ICC (34675), then the Exif and GPS IFDs, then the pixels.
fn encode_tiff(img: &RgbImage, meta: &PreparedMeta) -> Result<Vec<u8>, String> {
    let (w, h) = (img.width, img.height);
    let row = w as usize * 3;
    if w == 0 || h == 0 || img.data.len() != row * h as usize {
        return Err("bad image buffer".into());
    }
    let src = meta.exif.as_deref().and_then(SourceDirs::read);
    let o = Order {
        le: src.as_ref().is_none_or(|s| s.le),
    };
    let rows = (STRIP_BYTES / row).clamp(1, h as usize);
    let n_strips = (h as usize).div_ceil(rows);
    let counts: Vec<u32> = (0..n_strips)
        .map(|i| (rows.min(h as usize - i * rows) * row) as u32)
        .collect();
    let mut ifd0 = vec![
        o.longs(0x0100, &[w]),
        o.longs(0x0101, &[h]),
        o.shorts(0x0102, &[8, 8, 8]),
        o.shorts(0x0103, &[1]), // no compression
        o.shorts(0x0106, &[2]), // RGB
        o.longs(0x0111, &vec![0; n_strips]),
        o.shorts(0x0115, &[3]),
        o.longs(0x0116, &[rows as u32]),
        o.longs(0x0117, &counts),
        o.shorts(0x011C, &[1]), // chunky
    ];
    let src_ifd0: &[Field] = src.as_ref().map(|s| s.ifd0.as_slice()).unwrap_or_default();
    let own = |tag: u16, typ: u16| {
        src_ifd0
            .iter()
            .find(|f| f.tag == tag && f.typ == typ && f.count == 1)
            .cloned()
    };
    ifd0.push(own(0x011A, 5).unwrap_or_else(|| o.rational(0x011A, 72, 1)));
    ifd0.push(own(0x011B, 5).unwrap_or_else(|| o.rational(0x011B, 72, 1)));
    ifd0.push(own(0x0128, 3).unwrap_or_else(|| o.shorts(0x0128, &[2])));
    ifd0.extend(
        src_ifd0
            .iter()
            .filter(|f| f.typ == 2 && TIFF_IFD0_TEXT.contains(&f.tag))
            .cloned(),
    );
    if let Some(xmp) = &meta.xmp {
        ifd0.push(Field::bytes(700, 1, xmp.as_bytes()));
    }
    if let Some(iptc) = meta.irb.as_deref().and_then(jpegmeta::iptc_record) {
        ifd0.push(Field::bytes(0x83BB, 7, &iptc));
    }
    ifd0.push(Field::bytes(0x8773, 7, jpegmeta::srgb_icc()));
    let (mut exif, mut gps): (Vec<Field>, Vec<Field>) = match &src {
        Some(s) => (
            s.exif
                .iter()
                .filter(|f| f.typ != 13 && !TIFF_EXIF_DROP.contains(&f.tag))
                .cloned()
                .collect(),
            s.gps.iter().filter(|f| f.typ != 13).cloned().collect(),
        ),
        None => Default::default(),
    };
    if !exif.is_empty() {
        ifd0.push(o.longs(0x8769, &[0]));
    }
    if !gps.is_empty() {
        ifd0.push(o.longs(0x8825, &[0]));
    }
    for dir in [&mut ifd0, &mut exif, &mut gps] {
        dir.sort_by_key(|f| f.tag);
        dir.dedup_by_key(|f| f.tag);
    }
    let len = |dir: &[Field]| if dir.is_empty() { 0 } else { dir_len(dir) };
    let exif_off = 8 + dir_len(&ifd0);
    let gps_off = exif_off + len(&exif);
    let pixels = gps_off + len(&gps);
    if pixels as u64 + img.data.len() as u64 > u32::MAX as u64 {
        return Err("too large for a TIFF file (4 GB)".into());
    }
    let offsets: Vec<u32> = (0..n_strips)
        .map(|i| (pixels + i * rows * row) as u32)
        .collect();
    set_field(&mut ifd0, o.longs(0x0111, &offsets));
    set_field(&mut ifd0, o.longs(0x8769, &[exif_off as u32]));
    set_field(&mut ifd0, o.longs(0x8825, &[gps_off as u32]));
    let mut out = Vec::with_capacity(pixels + img.data.len());
    out.extend(if o.le { b"II*\0" } else { b"MM\0*" });
    out.extend(o.w32(8));
    put_dir(&mut out, &ifd0, o);
    debug_assert_eq!(out.len(), exif_off);
    if !exif.is_empty() {
        put_dir(&mut out, &exif, o);
    }
    debug_assert_eq!(out.len(), gps_off);
    if !gps.is_empty() {
        put_dir(&mut out, &gps, o);
    }
    debug_assert_eq!(out.len(), pixels);
    out.extend_from_slice(&img.data);
    Ok(out)
}

// ------------------------------------------------------------------ TIFF directories

/// TIFF byte order.
#[derive(Debug, Clone, Copy)]
struct Order {
    le: bool,
}

impl Order {
    fn w16(self, v: u16) -> [u8; 2] {
        if self.le {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        }
    }
    fn w32(self, v: u32) -> [u8; 4] {
        if self.le {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        }
    }
    fn r16(self, b: &[u8]) -> Option<u16> {
        let a: [u8; 2] = b.get(..2)?.try_into().ok()?;
        Some(if self.le {
            u16::from_le_bytes(a)
        } else {
            u16::from_be_bytes(a)
        })
    }
    fn r32(self, b: &[u8]) -> Option<u32> {
        let a: [u8; 4] = b.get(..4)?.try_into().ok()?;
        Some(if self.le {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        })
    }
    fn shorts(self, tag: u16, vals: &[u16]) -> Field {
        Field {
            tag,
            typ: 3,
            count: vals.len() as u32,
            value: vals.iter().flat_map(|v| self.w16(*v)).collect(),
        }
    }
    fn longs(self, tag: u16, vals: &[u32]) -> Field {
        Field {
            tag,
            typ: 4,
            count: vals.len() as u32,
            value: vals.iter().flat_map(|v| self.w32(*v)).collect(),
        }
    }
    fn rational(self, tag: u16, num: u32, den: u32) -> Field {
        Field {
            tag,
            typ: 5,
            count: 1,
            value: [self.w32(num), self.w32(den)].concat(),
        }
    }
}

/// One directory entry with its value bytes in the byte order of the file.
#[derive(Debug, Clone)]
struct Field {
    tag: u16,
    typ: u16,
    count: u32,
    value: Vec<u8>,
}

impl Field {
    fn bytes(tag: u16, typ: u16, data: &[u8]) -> Field {
        Field {
            tag,
            typ,
            count: data.len() as u32,
            value: data.to_vec(),
        }
    }
}

fn type_size(typ: u16) -> Option<usize> {
    Some(match typ {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 => 8,
        _ => return None,
    })
}

/// The entries of the directory at `off` of a TIFF block, values copied. Entries whose value
/// lies outside the block are skipped.
fn read_dir(block: &[u8], o: Order, off: usize) -> Option<Vec<Field>> {
    let n = o.r16(block.get(off..)?)? as usize;
    if n > 1000 {
        return None;
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let at = off + 2 + 12 * i;
        let e = block.get(at..at + 12)?;
        let (tag, typ, count) = (o.r16(e)?, o.r16(&e[2..])?, o.r32(&e[4..])?);
        let Some(len) = type_size(typ).and_then(|s| s.checked_mul(count as usize)) else {
            continue;
        };
        let value = if len <= 4 {
            e[8..8 + len].to_vec()
        } else {
            let at = o.r32(&e[8..])? as usize;
            match at.checked_add(len).and_then(|end| block.get(at..end)) {
                Some(v) => v.to_vec(),
                None => continue,
            }
        };
        out.push(Field {
            tag,
            typ,
            count,
            value,
        });
    }
    Some(out)
}

/// IFD0 / Exif / GPS directories of an EXIF TIFF block.
struct SourceDirs {
    le: bool,
    ifd0: Vec<Field>,
    exif: Vec<Field>,
    gps: Vec<Field>,
}

impl SourceDirs {
    fn read(block: &[u8]) -> Option<SourceDirs> {
        let le = match block.get(..2)? {
            b"II" => true,
            b"MM" => false,
            _ => return None,
        };
        let o = Order { le };
        if o.r16(block.get(2..)?)? != 42 {
            return None;
        }
        let ifd0 = read_dir(block, o, o.r32(block.get(4..)?)? as usize)?;
        let sub = |tag: u16| {
            ifd0.iter()
                .find(|f| f.tag == tag && f.value.len() == 4)
                .and_then(|f| o.r32(&f.value))
                .and_then(|off| read_dir(block, o, off as usize))
                .unwrap_or_default()
        };
        let (exif, gps) = (sub(0x8769), sub(0x8825));
        Some(SourceDirs {
            le,
            ifd0,
            exif,
            gps,
        })
    }
}

fn dir_len(dir: &[Field]) -> usize {
    2 + 12 * dir.len()
        + 4
        + dir
            .iter()
            .filter(|f| f.value.len() > 4)
            .map(|f| f.value.len() + f.value.len() % 2)
            .sum::<usize>()
}

/// Appends a directory (sorted entries) followed by its out-of-line values.
fn put_dir(out: &mut Vec<u8>, dir: &[Field], o: Order) {
    let data_at = out.len() + 2 + 12 * dir.len() + 4;
    let mut extra: Vec<u8> = Vec::new();
    out.extend(o.w16(dir.len() as u16));
    for f in dir {
        out.extend(o.w16(f.tag));
        out.extend(o.w16(f.typ));
        out.extend(o.w32(f.count));
        if f.value.len() <= 4 {
            let mut v = [0u8; 4];
            v[..f.value.len()].copy_from_slice(&f.value);
            out.extend(v);
        } else {
            out.extend(o.w32((data_at + extra.len()) as u32));
            extra.extend(&f.value);
            if extra.len() % 2 == 1 {
                extra.push(0);
            }
        }
    }
    out.extend(o.w32(0));
    out.extend(extra);
}

fn set_field(dir: &mut [Field], f: Field) {
    if let Some(slot) = dir.iter_mut().find(|x| x.tag == f.tag) {
        *slot = f;
    }
}

// ------------------------------------------------------------------ PNG chunks

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
const PNG_XMP_KEYWORD: &[u8] = b"XML:com.adobe.xmp";
/// Ancillary chunks a metadata-free PNG keeps: colour, transparency, density and animation.
const PNG_KEEP: [&[u8; 4]; 18] = [
    b"tRNS", b"cHRM", b"gAMA", b"iCCP", b"sBIT", b"sRGB", b"cICP", b"mDCV", b"mDCv", b"cLLI",
    b"cLLi", b"pHYs", b"bKGD", b"hIST", b"sPLT", b"acTL", b"fcTL", b"fdAT",
];

struct PngChunk<'a> {
    kind: [u8; 4],
    data: &'a [u8],
    start: usize,
    end: usize,
}

/// The chunks of a PNG up to and including `IEND`.
fn png_chunks(png: &[u8]) -> Option<Vec<PngChunk<'_>>> {
    if !png.starts_with(PNG_SIGNATURE) {
        return None;
    }
    let mut out: Vec<PngChunk> = Vec::new();
    let mut p = PNG_SIGNATURE.len();
    while p + 12 <= png.len() {
        let len = u32::from_be_bytes(png[p..p + 4].try_into().ok()?) as usize;
        let end = p.checked_add(12)?.checked_add(len)?;
        if end > png.len() {
            return None;
        }
        let kind: [u8; 4] = png[p + 4..p + 8].try_into().ok()?;
        out.push(PngChunk {
            kind,
            data: &png[p + 8..p + 8 + len],
            start: p,
            end,
        });
        p = end;
        if &kind == b"IEND" {
            break;
        }
    }
    out.last().filter(|c| &c.kind == b"IEND")?;
    Some(out)
}

fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut c = Vec::with_capacity(data.len() + 12);
    c.extend((data.len() as u32).to_be_bytes());
    c.extend(kind);
    c.extend(data);
    let crc = crc32(&c[4..]);
    c.extend(crc.to_be_bytes());
    c
}

/// An uncompressed international text chunk holding an XMP packet.
fn itxt_xmp(xmp: &str) -> Vec<u8> {
    let mut d = PNG_XMP_KEYWORD.to_vec();
    // keyword terminator, not compressed, method 0, empty language tag and translated keyword
    d.extend([0, 0, 0, 0, 0]);
    d.extend(xmp.as_bytes());
    d
}

fn png_insert_before_idat(png: &[u8], chunk: &[u8]) -> Option<Vec<u8>> {
    let at = png_chunks(png)?.iter().find(|c| &c.kind == b"IDAT")?.start;
    let mut out = Vec::with_capacity(png.len() + chunk.len());
    out.extend(&png[..at]);
    out.extend(chunk);
    out.extend(&png[at..]);
    Some(out)
}

/// CRC-32 (ISO 3309) as used by PNG.
fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

// ------------------------------------------------------------------ WebP (RIFF) chunks

const VP8X_EXIF: u8 = 0x08;
const VP8X_XMP: u8 = 0x04;

struct RiffChunk<'a> {
    id: [u8; 4],
    data: &'a [u8],
}

fn riff_chunks(webp: &[u8]) -> Option<Vec<RiffChunk<'_>>> {
    if webp.len() < 12 || &webp[..4] != b"RIFF" || &webp[8..12] != b"WEBP" {
        return None;
    }
    let end = (u32::from_le_bytes(webp[4..8].try_into().ok()?) as usize).checked_add(8)?;
    if end > webp.len() {
        return None;
    }
    let mut out = Vec::new();
    let mut p = 12;
    while p + 8 <= end {
        let id: [u8; 4] = webp[p..p + 4].try_into().ok()?;
        let len = u32::from_le_bytes(webp[p + 4..p + 8].try_into().ok()?) as usize;
        let data_end = (p + 8).checked_add(len)?;
        if data_end > end {
            return None;
        }
        out.push(RiffChunk {
            id,
            data: &webp[p + 8..data_end],
        });
        p = data_end + len % 2;
    }
    Some(out)
}

fn riff_build(chunks: &[([u8; 4], Cow<[u8]>)]) -> Vec<u8> {
    let mut body = b"WEBP".to_vec();
    for (id, data) in chunks {
        body.extend(id);
        body.extend((data.len() as u32).to_le_bytes());
        body.extend(data.iter());
        if data.len() % 2 == 1 {
            body.push(0);
        }
    }
    let mut out = b"RIFF".to_vec();
    out.extend((body.len() as u32).to_le_bytes());
    out.extend(body);
    out
}

/// Appends an `XMP ` chunk to an extended (VP8X) WebP and flags it.
fn webp_with_xmp(webp: &[u8], xmp: &[u8]) -> Option<Vec<u8>> {
    let chunks = riff_chunks(webp)?;
    if chunks.first()?.id != *b"VP8X" || chunks[0].data.is_empty() {
        return None;
    }
    let mut list: Vec<([u8; 4], Cow<[u8]>)> = chunks
        .iter()
        .map(|c| (c.id, Cow::Borrowed(c.data)))
        .collect();
    list[0].1.to_mut()[0] |= VP8X_XMP;
    list.push((*b"XMP ", Cow::Borrowed(xmp)));
    Some(riff_build(&list))
}

// ------------------------------------------------------------------ cleaning originals

const EXIF_HEADER: &[u8] = b"Exif\0\0";
const XMP_HEADER: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
const XMP_EXT_HEADER: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";

/// Whether an original in `format` can be cleaned of metadata without re-encoding it.
pub fn cleanable(format: ImageFormat) -> bool {
    matches!(
        format,
        ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::Webp
    )
}

/// The original file `bytes` without its GPS position (EXIF GPS IFD, XMP `GPS*` properties)
/// or, with `strip_all`, without any EXIF / XMP / IPTC / comments; the image data is copied
/// byte for byte. A metadata-free copy keeps the colour profile and, when the pixels are stored
/// rotated, a minimal EXIF block with just the `orientation` (else the picture would display
/// sideways). `None` when the file is not a well-formed JPEG / PNG / WebP.
pub fn clean_original(
    format: ImageFormat,
    bytes: &[u8],
    strip_all: bool,
    orientation: u8,
) -> Option<Vec<u8>> {
    let orientation = (2..=8).contains(&orientation).then_some(orientation);
    match format {
        ImageFormat::Jpeg => clean_jpeg(bytes, strip_all, orientation),
        ImageFormat::Png => clean_png(bytes, strip_all, orientation),
        ImageFormat::Webp => clean_webp(bytes, strip_all, orientation),
        _ => None,
    }
}

/// An EXIF block that holds only the orientation.
pub fn orientation_exif(orientation: u8) -> Vec<u8> {
    let mut v = b"II*\0".to_vec();
    v.extend(8u32.to_le_bytes());
    v.extend(1u16.to_le_bytes());
    v.extend(0x0112u16.to_le_bytes());
    v.extend(3u16.to_le_bytes());
    v.extend(1u32.to_le_bytes());
    v.extend((orientation as u16).to_le_bytes());
    v.extend([0, 0]);
    v.extend(0u32.to_le_bytes());
    v
}

/// The packet without `GPS*` properties (and, with `drop_ext`, without its extended-XMP
/// pointer): `Ok(None)` when nothing had to go, `Err` when it cannot be parsed.
fn xmp_without_gps(packet: &[u8], drop_ext: bool) -> Result<Option<String>, ()> {
    let text = std::str::from_utf8(packet).map_err(|_| ())?;
    let mut x = Xmp::parse(text).map_err(|_| ())?;
    let n = x.remove_properties(&|ns, local| {
        jpegmeta::is_gps_property(local)
            || (drop_ext && ns == NS_XMP_NOTE && local == "HasExtendedXMP")
    });
    Ok((n > 0).then(|| x.serialize()))
}

/// An EXIF payload (TIFF block, optionally behind `Exif\0\0`) without its GPS IFD, same size.
fn exif_without_gps(payload: &[u8]) -> Option<Vec<u8>> {
    let (prefix, tiff) = match payload.strip_prefix(EXIF_HEADER) {
        Some(t) => (EXIF_HEADER, t),
        None => (&[][..], payload),
    };
    Some([prefix, &jpegmeta::without_gps(tiff)?[..]].concat())
}

/// `packet` padded with spaces in front of its `<?xpacket end` trailer (else at the end) to
/// `len` bytes, so a rewritten segment keeps its size.
fn pad_xmp(mut packet: String, len: usize) -> String {
    if packet.len() < len {
        let pad = " ".repeat(len - packet.len());
        match packet.rfind("<?xpacket end") {
            Some(i) => packet.insert_str(i, &pad),
            None => packet.push_str(&pad),
        }
    }
    packet
}

/// A JPEG marker segment: `start..end` covers marker, length and payload.
#[derive(Debug, Clone, Copy)]
struct Seg {
    marker: u8,
    start: usize,
    end: usize,
}

impl Seg {
    fn payload(self, b: &[u8]) -> &[u8] {
        &b[self.start + 4..self.end]
    }
}

/// The marker segments in front of the first scan, and the offset of its SOS marker.
fn jpeg_header(b: &[u8]) -> Option<(Vec<Seg>, usize)> {
    if b.len() < 4 || b[0] != 0xFF || b[1] != 0xD8 {
        return None;
    }
    let mut segs = Vec::new();
    let mut p = 2;
    loop {
        if p + 4 > b.len() || b[p] != 0xFF {
            return None;
        }
        let marker = b[p + 1];
        match marker {
            0xFF => {
                p += 1;
                continue;
            }
            0xDA => return Some((segs, p)),
            0x01 | 0xD0..=0xD9 => return None,
            _ => {}
        }
        let len = u16::from_be_bytes([b[p + 2], b[p + 3]]) as usize;
        if len < 2 || p + 2 + len > b.len() {
            return None;
        }
        segs.push(Seg {
            marker,
            start: p,
            end: p + 2 + len,
        });
        p += 2 + len;
    }
}

/// Offset just past the EOI marker of the image whose first scan starts at `sos` (the end of
/// the data when it is truncated). What follows is a trailer: MPF images, gain maps, videos.
fn jpeg_end(b: &[u8], sos: usize) -> usize {
    let mut p = sos;
    while p + 1 < b.len() {
        if b[p] != 0xFF {
            p += 1; // entropy-coded data
            continue;
        }
        match b[p + 1] {
            0xFF => p += 1,
            0x00 | 0x01 | 0xD0..=0xD7 => p += 2,
            0xD9 => return p + 2,
            _ => match b.get(p + 2..p + 4) {
                // SOS / DHT / DQT / DRI ... of the next scans
                Some(l) => p += 2 + u16::from_be_bytes([l[0], l[1]]) as usize,
                None => return b.len(),
            },
        }
    }
    b.len()
}

fn jpeg_segment(marker: u8, payload: &[u8]) -> Option<Vec<u8>> {
    if payload.len() + 2 > 0xFFFF {
        return None;
    }
    let mut s = vec![0xFF, marker];
    s.extend(((payload.len() + 2) as u16).to_be_bytes());
    s.extend(payload);
    Some(s)
}

/// Whether the extended XMP of a JPEG (APP1 `xmp/extension` chunks) carries a GPS position or
/// cannot be checked.
fn extended_xmp_has_gps(b: &[u8], segs: &[Seg]) -> bool {
    const HEAD: usize = 35 + 32 + 4 + 4; // signature, GUID, full length, offset
    let mut full: Vec<u8> = Vec::new();
    for s in segs {
        let p = s.payload(b);
        if s.marker != 0xE1 || !p.starts_with(XMP_EXT_HEADER) {
            continue;
        }
        if p.len() < HEAD {
            return true;
        }
        let total = u32::from_be_bytes(p[67..71].try_into().expect("4 bytes")) as usize;
        let off = u32::from_be_bytes(p[71..75].try_into().expect("4 bytes")) as usize;
        let chunk = &p[HEAD..];
        if total > 64 << 20 || off.saturating_add(chunk.len()) > total {
            return true;
        }
        full.resize(total.max(full.len()), b' ');
        full[off..off + chunk.len()].copy_from_slice(chunk);
    }
    !full.is_empty() && !matches!(xmp_without_gps(&full, false), Ok(None))
}

fn clean_jpeg(src: &[u8], strip_all: bool, orientation: Option<u8>) -> Option<Vec<u8>> {
    let (segs, sos) = jpeg_header(src)?;
    let end = jpeg_end(src, sos);
    let mut out = Vec::with_capacity(src.len());
    out.extend(&src[..2]);
    if strip_all {
        let orientation_seg = orientation
            .and_then(|o| jpeg_segment(0xE1, &[EXIF_HEADER, &orientation_exif(o)[..]].concat()));
        let mut pending = orientation_seg;
        for s in &segs {
            let p = s.payload(src);
            let keep = match s.marker {
                0xE0 => p.starts_with(b"JFIF\0"),
                0xE2 => p.starts_with(b"ICC_PROFILE\0"),
                0xEE => p.starts_with(b"Adobe"),
                0xE1 | 0xE3..=0xED | 0xEF | 0xFE => false,
                _ => true,
            };
            if s.marker != 0xE0 {
                // the EXIF block goes right after SOI / JFIF
                if let Some(seg) = pending.take() {
                    out.extend(seg);
                }
            }
            if keep {
                out.extend(&src[s.start..s.end]);
            }
        }
        if let Some(seg) = pending.take() {
            out.extend(seg);
        }
        out.extend(&src[sos..end]); // scans; the trailer (MPF images, videos) goes
        return Some(out);
    }

    let drop_ext = extended_xmp_has_gps(src, &segs);
    // header size changes in front of the MPF segment and in total, for its index
    let (mut before_mpf, mut total) = (0i64, 0i64);
    let mut mpf_at: Option<(usize, usize)> = None;
    for s in &segs {
        let p = s.payload(src);
        let raw = &src[s.start..s.end];
        let new: Option<Cow<[u8]>> = match s.marker {
            0xE1 if p.starts_with(EXIF_HEADER) => {
                // unparseable EXIF might still hold a position: it goes
                exif_without_gps(p).and_then(|e| jpeg_segment(0xE1, &e).map(Cow::Owned))
            }
            0xE1 if p.starts_with(XMP_HEADER) => {
                match xmp_without_gps(&p[XMP_HEADER.len()..], drop_ext) {
                    Ok(None) => Some(Cow::Borrowed(raw)),
                    Ok(Some(x)) => {
                        let x = pad_xmp(x, p.len() - XMP_HEADER.len());
                        jpeg_segment(0xE1, &[XMP_HEADER, x.as_bytes()].concat()).map(Cow::Owned)
                    }
                    Err(()) => None,
                }
            }
            0xE1 if drop_ext && p.starts_with(XMP_EXT_HEADER) => None,
            _ => Some(Cow::Borrowed(raw)),
        };
        let delta = new.as_ref().map_or(0, |n| n.len() as i64) - raw.len() as i64;
        total += delta;
        if mpf_at.is_none() {
            if s.marker == 0xE2 && p.starts_with(b"MPF\0") {
                mpf_at = Some((out.len() + 4, p.len()));
            } else {
                before_mpf += delta;
            }
        }
        if let Some(n) = new {
            out.extend(n.iter());
        }
    }
    out.extend(&src[sos..end]);
    let trailer = out.len();
    out.extend(&src[end..]);
    if let Some((at, len)) = mpf_at {
        if total != 0 {
            fix_mpf(&mut out[at..at + len], before_mpf, total);
        }
    }
    wipe_trailer_gps(&mut out, trailer);
    Some(out)
}

/// Keeps the Multi-Picture index (APP2 `MPF\0` payload) valid after the primary image changed
/// size by `total` bytes, `before` of them in front of the index: the primary's size grows by
/// `total`, the offsets of the other images (relative to the index) by `total - before`.
fn fix_mpf(payload: &mut [u8], before: i64, total: i64) {
    let Some(t) = payload.get_mut(4..) else {
        return;
    };
    let le = match t.get(..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return,
    };
    let o = Order { le };
    let Some(ifd) = o.r32(t.get(4..).unwrap_or_default()) else {
        return;
    };
    let ifd = ifd as usize;
    let n = o.r16(t.get(ifd..).unwrap_or_default()).unwrap_or(0) as usize;
    for i in 0..n.min(64) {
        let e = ifd + 2 + 12 * i;
        if o.r16(t.get(e..).unwrap_or_default()) != Some(0xB002) {
            continue;
        }
        let (Some(count), Some(at)) = (
            o.r32(t.get(e + 4..).unwrap_or_default()),
            o.r32(t.get(e + 8..).unwrap_or_default()),
        ) else {
            return;
        };
        for k in 0..(count as usize / 16).min(64) {
            let entry = at as usize + 16 * k;
            let shift = |t: &mut [u8], pos: usize, by: i64| {
                if let Some(v) = o.r32(t.get(pos..).unwrap_or_default()) {
                    let nv = (v as i64 + by).clamp(0, u32::MAX as i64) as u32;
                    if let Some(dst) = t.get_mut(pos..pos + 4) {
                        dst.copy_from_slice(&o.w32(nv));
                    }
                }
            };
            if k == 0 {
                shift(t, entry + 4, total);
            } else if o.r32(t.get(entry + 8..).unwrap_or_default()).unwrap_or(0) != 0 {
                shift(t, entry + 8, total - before);
            }
        }
    }
}

/// Removes the GPS IFD from the EXIF of every JPEG embedded after `from` (MPF images, gain
/// maps), in place: sizes and so all offsets stay as they are.
fn wipe_trailer_gps(out: &mut [u8], from: usize) {
    let mut p = from;
    while let Some(i) = out[p..].windows(3).position(|w| w == b"\xFF\xD8\xFF") {
        let at = p + i;
        let exif: Vec<(usize, usize)> = jpeg_header(&out[at..])
            .map(|(segs, _)| {
                segs.iter()
                    .filter(|s| s.marker == 0xE1 && s.payload(&out[at..]).starts_with(EXIF_HEADER))
                    .map(|s| (at + s.start + 4 + EXIF_HEADER.len(), at + s.end))
                    .collect()
            })
            .unwrap_or_default();
        for (a, b) in exif {
            if let Some(clean) = jpegmeta::without_gps(&out[a..b]) {
                out[a..b].copy_from_slice(&clean);
            }
        }
        p = at + 3;
    }
}

fn clean_png(src: &[u8], strip_all: bool, orientation: Option<u8>) -> Option<Vec<u8>> {
    let chunks = png_chunks(src)?;
    let mut out = Vec::with_capacity(src.len());
    out.extend(PNG_SIGNATURE);
    for c in &chunks {
        let raw = &src[c.start..c.end];
        if strip_all {
            if c.kind[0].is_ascii_uppercase() || PNG_KEEP.contains(&&c.kind) {
                out.extend(raw);
            }
            if &c.kind == b"IHDR" {
                if let Some(o) = orientation {
                    out.extend(png_chunk(b"eXIf", &orientation_exif(o)));
                }
            }
            continue;
        }
        let keyword = c.data.split(|b| *b == 0).next().unwrap_or_default();
        match &c.kind {
            b"eXIf" => {
                if let Some(t) = jpegmeta::without_gps(c.data) {
                    out.extend(png_chunk(b"eXIf", &t));
                }
            }
            b"tEXt" | b"zTXt" | b"iTXt" if keyword.starts_with(b"Raw profile type") => {}
            b"tEXt" | b"zTXt" if keyword == PNG_XMP_KEYWORD => {}
            b"iTXt" if keyword == PNG_XMP_KEYWORD => {
                // keyword \0 flag method language \0 translated \0 text; a compressed or
                // malformed packet cannot be checked and goes
                let compressed = c.data.get(keyword.len() + 1).is_none_or(|f| *f != 0);
                if let (Some(h), false) = (itxt_head_len(c.data), compressed) {
                    match xmp_without_gps(&c.data[h..], false) {
                        Ok(None) => out.extend(raw),
                        Ok(Some(x)) => {
                            let text = [&c.data[..h], x.as_bytes()].concat();
                            out.extend(png_chunk(b"iTXt", &text));
                        }
                        Err(()) => {}
                    }
                }
            }
            _ => out.extend(raw),
        }
    }
    Some(out)
}

/// Length of the iTXt fields in front of the text.
fn itxt_head_len(d: &[u8]) -> Option<usize> {
    let k = d.iter().position(|b| *b == 0)?;
    let lang = k + 3;
    let l = d.get(lang..)?.iter().position(|b| *b == 0)?;
    let tk = lang + l + 1;
    let t = d.get(tk..)?.iter().position(|b| *b == 0)?;
    Some(tk + t + 1)
}

fn clean_webp(src: &[u8], strip_all: bool, orientation: Option<u8>) -> Option<Vec<u8>> {
    let chunks = riff_chunks(src)?;
    let first = chunks.first()?;
    if first.id != *b"VP8X" || first.data.is_empty() {
        return Some(src.to_vec()); // simple format: no room for metadata
    }
    let mut flags = first.data[0];
    let mut list: Vec<([u8; 4], Cow<[u8]>)> = Vec::new();
    for c in &chunks[1..] {
        let keep = Cow::Borrowed(c.data);
        match &c.id {
            b"ICCP" | b"ANIM" | b"ANMF" | b"ALPH" | b"VP8 " | b"VP8L" => list.push((c.id, keep)),
            b"EXIF" => match (strip_all, exif_without_gps(c.data)) {
                (false, Some(e)) => list.push((c.id, Cow::Owned(e))),
                _ => flags &= !VP8X_EXIF,
            },
            b"XMP " => match (strip_all, xmp_without_gps(c.data, false)) {
                (false, Ok(None)) => list.push((c.id, keep)),
                (false, Ok(Some(x))) => list.push((c.id, Cow::Owned(x.into_bytes()))),
                _ => flags &= !VP8X_XMP,
            },
            _ if !strip_all => list.push((c.id, keep)),
            _ => {}
        }
    }
    if strip_all {
        if let Some(o) = orientation {
            list.push((*b"EXIF", Cow::Owned(orientation_exif(o))));
            flags |= VP8X_EXIF;
        }
    }
    let mut vp8x = first.data.to_vec();
    vp8x[0] = flags;
    list.insert(0, (*b"VP8X", Cow::Owned(vp8x)));
    Some(riff_build(&list))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: u32, h: u32, rgb: [u8; 3]) -> RgbImage {
        RgbImage {
            width: w,
            height: h,
            data: rgb.repeat((w * h) as usize),
        }
    }

    #[test]
    fn crc_matches_the_png_reference() {
        // CRC of the IEND chunk type, as every PNG ends with it
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
    }

    #[test]
    fn format_names_and_rules() {
        let f: ExportFormat = serde_json::from_str("\"tif\"").unwrap();
        assert_eq!(f, ExportFormat::Tiff);
        let f: ExportFormat = serde_json::from_str("\"jpg\"").unwrap();
        assert_eq!(f, ExportFormat::Jpeg);
        assert_eq!(
            serde_json::to_string(&ExportFormat::Webp).unwrap(),
            "\"webp\""
        );
        assert_eq!(ExportFormat::Original.extension(), "jpg");
        assert_eq!(ExportFormat::Tiff.extension(), "tif");
        assert!(ExportFormat::Original.keeps(ImageFormat::Raw));
        assert!(!ExportFormat::Jpeg.keeps(ImageFormat::Raw));
        assert!(ExportFormat::Png.keeps(ImageFormat::Png));
        assert_eq!(ExportFormat::Webp.max_side(), Some(WEBP_MAX_SIDE));
        assert_eq!(ExportFormat::Png.max_side(), None);
    }

    #[test]
    fn oversized_webp_is_refused_not_truncated() {
        let e = encode(
            &img(WEBP_MAX_SIDE + 1, 1, [1, 2, 3]),
            ExportFormat::Webp,
            90,
            &PreparedMeta::default(),
        );
        assert!(e.unwrap_err().contains("16383"));
    }

    #[test]
    fn tiff_directories_are_well_formed_in_both_byte_orders() {
        for be in [false, true] {
            let meta = PreparedMeta {
                exif: jpegmeta::prepare_exif(
                    &jpegmeta::build_test_exif(1, be, true),
                    (5, 3),
                    false,
                ),
                ..Default::default()
            };
            let out = encode(&img(5, 3, [9, 8, 7]), ExportFormat::Tiff, 90, &meta).unwrap();
            assert_eq!(&out[..2], if be { b"MM" } else { b"II" });
            let dirs = SourceDirs::read(&out).unwrap();
            let tags: Vec<u16> = dirs.ifd0.iter().map(|f| f.tag).collect();
            let mut sorted = tags.clone();
            sorted.sort();
            assert_eq!(tags, sorted, "ascending tags");
            assert!(tags.contains(&0x8773) && tags.contains(&0x8769) && tags.contains(&0x8825));
            assert!(dirs.exif.iter().any(|f| f.tag == 0x9003));
            assert_eq!(dirs.gps.len(), 4);
            let dec = image::load_from_memory(&out).unwrap().to_rgb8();
            assert_eq!(dec.dimensions(), (5, 3));
            assert_eq!(dec.get_pixel(4, 2).0, [9, 8, 7]);
        }
    }

    #[test]
    fn mpf_offsets_follow_a_size_change() {
        // "MPF\0" + LE TIFF header + IFD with one MPEntry tag holding two entries
        let mut p = b"MPF\0II*\0\x08\0\0\0".to_vec();
        p.extend(1u16.to_le_bytes());
        p.extend(0xB002u16.to_le_bytes());
        p.extend(7u16.to_le_bytes());
        p.extend(32u32.to_le_bytes());
        p.extend(26u32.to_le_bytes()); // entries right after the IFD (8 + 2 + 12 + 4)
        p.extend(0u32.to_le_bytes());
        for (size, off) in [(1000u32, 0u32), (500, 2000)] {
            p.extend(0u32.to_le_bytes());
            p.extend(size.to_le_bytes());
            p.extend(off.to_le_bytes());
            p.extend([0u8; 4]);
        }
        fix_mpf(&mut p, -10, -30);
        let at = |i: usize| u32::from_le_bytes(p[4 + 26 + i..4 + 30 + i].try_into().unwrap());
        assert_eq!((at(4), at(8)), (970, 0), "primary shrank by 30");
        assert_eq!((at(20), at(24)), (500, 1980), "20 of it after the index");
    }
}
