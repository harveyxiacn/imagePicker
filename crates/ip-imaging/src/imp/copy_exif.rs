//! The EXIF of a source as one self-contained TIFF block, for writing into exports.
//!
//! JPEG, HEIF and RAF (through its embedded JPEG) store such a block as is. TIFF-based RAW
//! files (CR2, NEF, ARW, DNG, ORF, PEF...), TIFF and CR3 keep their EXIF in directories spread
//! over the file, next to strips, sub-images and MakerNotes whose offsets cannot be carried
//! over; for those a fresh block is built from the descriptive IFD0 tags, the whole Exif IFD
//! except the MakerNote and the interoperability pointer, and the GPS IFD. Values are copied
//! byte for byte (converted to the byte order of the new block where needed).

use std::path::Path;

use super::container;
use super::io::{open_data, read_head};
use super::tiff::{Ifd, Tiff, TAG_EXIF_IFD, TAG_GPS_IFD};
use crate::{ImageFormat, Result};

/// Largest block an APP1 segment can hold (with its `Exif\0\0` prefix).
const MAX_BLOCK: usize = 65_000;
/// Single values larger than this (vendor blobs, huge comments) are not copied.
const MAX_VALUE: usize = 16 * 1024;

/// IFD0 tags worth keeping: they describe the picture, not the RAW data layout.
const IFD0_TAGS: [u16; 7] = [
    0x010E, // ImageDescription
    0x010F, // Make
    0x0110, // Model
    0x0131, // Software
    0x0132, // DateTime
    0x013B, // Artist
    0x8298, // Copyright
];

/// Exif IFD tags that are dropped: MakerNote (internal offsets), interoperability pointer,
/// Microsoft padding / offset schema.
const EXIF_DROP: [u16; 4] = [0x927C, 0xA005, 0xEA1C, 0xEA1D];

pub fn read_exif(path: &Path, format: ImageFormat) -> Result<Option<Vec<u8>>> {
    Ok(match format {
        ImageFormat::Jpeg => {
            let head = read_head(path, 1 << 20)?;
            jpeg_exif(&head)
        }
        ImageFormat::Raw => raw_exif(&open_data(path)?),
        ImageFormat::Tiff => {
            let data = open_data(path)?;
            Tiff::new(&data).and_then(|t| rebuild_tiff(&t))
        }
        ImageFormat::Heif | ImageFormat::Avif => {
            let data = open_data(path)?;
            container::parse_heif(&data)
                .exif_tiff
                .filter(|t| Tiff::new(t).is_some())
                .map(<[u8]>::to_vec)
        }
        ImageFormat::Png | ImageFormat::Webp => {
            use image::ImageDecoder;
            std::fs::metadata(path)?;
            image::ImageReader::open(path)?
                .with_guessed_format()?
                .into_decoder()
                .ok()
                .and_then(|mut d| d.exif_metadata().ok().flatten())
                .map(|b| b.strip_prefix(b"Exif\0\0").map(<[u8]>::to_vec).unwrap_or(b))
                .filter(|b| Tiff::new(b).is_some())
        }
    })
}

fn jpeg_exif(data: &[u8]) -> Option<Vec<u8>> {
    container::jpeg_info(data)?
        .exif_tiff
        .filter(|t| Tiff::new(t).is_some())
        .map(<[u8]>::to_vec)
}

fn raw_exif(data: &[u8]) -> Option<Vec<u8>> {
    if container::is_raf(data) {
        return container::raf_jpeg(data).and_then(jpeg_exif);
    }
    if container::is_cr3(data) {
        // CMT1 = IFD0, CMT2 = Exif IFD, CMT4 = GPS IFD, each a TIFF of its own
        let c = container::parse_cr3(data);
        let (ifd0, exif, gps) = (first_ifd(c.cmt1), first_ifd(c.cmt2), first_ifd(c.cmt4));
        let le = [&ifd0, &exif, &gps]
            .into_iter()
            .flatten()
            .map(|(t, _)| t.le)
            .next()?;
        return build(
            le,
            fields_of(&ifd0, &|tag| IFD0_TAGS.contains(&tag), le),
            fields_of(&exif, &|tag| !EXIF_DROP.contains(&tag), le),
            fields_of(&gps, &|_| true, le),
        );
    }
    let Some(t) = Tiff::new(data) else {
        return container::raw_preview(data).and_then(jpeg_exif);
    };
    let has_exif_ifd = t
        .first_ifd_offset()
        .and_then(|o| t.read_ifd(o))
        .is_some_and(|(ifd0, _)| ifd0.get(TAG_EXIF_IFD).is_some());
    // ORF/RW2/PEF may keep the details in the embedded preview's EXIF only
    let rebuilt = rebuild_tiff(&t);
    if has_exif_ifd {
        return rebuilt;
    }
    container::raw_preview(data).and_then(jpeg_exif).or(rebuilt)
}

fn first_ifd(block: Option<&[u8]>) -> Option<(Tiff<'_>, Ifd)> {
    let t = Tiff::new(block?)?;
    let (ifd, _) = t.read_ifd(t.first_ifd_offset()?)?;
    Some((t, ifd))
}

fn fields_of(dir: &Option<(Tiff, Ifd)>, keep: &dyn Fn(u16) -> bool, le: bool) -> Vec<Field> {
    dir.as_ref()
        .map(|(t, ifd)| fields(t, ifd, keep, le))
        .unwrap_or_default()
}

/// Rebuilds IFD0 / Exif / GPS of a TIFF-structured file.
fn rebuild_tiff(t: &Tiff) -> Option<Vec<u8>> {
    let (ifd0, _) = t.read_ifd(t.first_ifd_offset()?)?;
    let sub = |tag: u16| {
        let off = t.first_uint(ifd0.get(tag)?)?;
        Some(t.read_ifd(off as usize)?.0)
    };
    let (exif, gps) = (sub(TAG_EXIF_IFD), sub(TAG_GPS_IFD));
    build(
        t.le,
        fields(t, &ifd0, &|tag| IFD0_TAGS.contains(&tag), t.le),
        exif.map(|i| fields(t, &i, &|tag| !EXIF_DROP.contains(&tag), t.le))
            .unwrap_or_default(),
        gps.map(|i| fields(t, &i, &|_| true, t.le))
            .unwrap_or_default(),
    )
}

/// One directory entry: its value bytes in the byte order of the block being built.
struct Field {
    tag: u16,
    typ: u16,
    count: u32,
    value: Vec<u8>,
}

/// Byte size of one value of a TIFF type, and the unit to swap for a byte-order change.
fn type_units(typ: u16) -> Option<(usize, usize)> {
    Some(match typ {
        1 | 2 | 6 | 7 => (1, 1),
        3 | 8 => (2, 2),
        4 | 9 | 11 => (4, 4),
        5 | 10 => (8, 4),
        12 => (8, 8),
        _ => return None, // incl. 13 (IFD pointers)
    })
}

fn fields(t: &Tiff, ifd: &Ifd, keep: &dyn Fn(u16) -> bool, le: bool) -> Vec<Field> {
    let mut out = Vec::new();
    for e in &ifd.entries {
        if !keep(e.tag) || matches!(e.tag, TAG_EXIF_IFD | TAG_GPS_IFD) {
            continue;
        }
        let Some((size, unit)) = type_units(e.typ) else {
            continue;
        };
        let Some(len) = size.checked_mul(e.count as usize) else {
            continue;
        };
        if len == 0 || len > MAX_VALUE {
            continue;
        }
        let Some(bytes) = e
            .pos
            .checked_add(len)
            .and_then(|end| t.data.get(e.pos..end))
        else {
            continue;
        };
        let mut value = bytes.to_vec();
        if t.le != le && unit > 1 {
            value.chunks_mut(unit).for_each(<[u8]>::reverse);
        }
        out.push(Field {
            tag: e.tag,
            typ: e.typ,
            count: e.count,
            value,
        });
    }
    out
}

fn ifd_len(fields: &[Field]) -> usize {
    2 + 12 * fields.len()
        + 4
        + fields
            .iter()
            .filter(|f| f.value.len() > 4)
            .map(|f| f.value.len() + f.value.len() % 2)
            .sum::<usize>()
}

fn put_ifd(out: &mut Vec<u8>, mut fields: Vec<Field>, le: bool) {
    let w16 = |v: u16| if le { v.to_le_bytes() } else { v.to_be_bytes() };
    let w32 = |v: u32| if le { v.to_le_bytes() } else { v.to_be_bytes() };
    fields.sort_by_key(|f| f.tag);
    let data_at = out.len() + 2 + 12 * fields.len() + 4;
    let mut extra: Vec<u8> = Vec::new();
    out.extend(w16(fields.len() as u16));
    for f in &fields {
        out.extend(w16(f.tag));
        out.extend(w16(f.typ));
        out.extend(w32(f.count));
        if f.value.len() <= 4 {
            let mut v = f.value.clone();
            v.resize(4, 0);
            out.extend(v);
        } else {
            out.extend(w32((data_at + extra.len()) as u32));
            extra.extend(&f.value);
            if extra.len() % 2 == 1 {
                extra.push(0);
            }
        }
    }
    out.extend(w32(0));
    out.extend(extra);
}

/// header | IFD0 (+ pointers) | Exif IFD | GPS IFD, each followed by its out-of-line values.
fn build(le: bool, mut ifd0: Vec<Field>, exif: Vec<Field>, gps: Vec<Field>) -> Option<Vec<u8>> {
    if ifd0.is_empty() && exif.is_empty() && gps.is_empty() {
        return None;
    }
    let ptr = |tag: u16, off: usize| Field {
        tag,
        typ: 4,
        count: 1,
        value: if le {
            (off as u32).to_le_bytes()
        } else {
            (off as u32).to_be_bytes()
        }
        .to_vec(),
    };
    let n_ptrs = usize::from(!exif.is_empty()) + usize::from(!gps.is_empty());
    let exif_off = 8 + ifd_len(&ifd0) + 12 * n_ptrs;
    let gps_off = exif_off + if exif.is_empty() { 0 } else { ifd_len(&exif) };
    if !exif.is_empty() {
        ifd0.push(ptr(TAG_EXIF_IFD, exif_off));
    }
    if !gps.is_empty() {
        ifd0.push(ptr(TAG_GPS_IFD, gps_off));
    }
    let mut out = if le {
        b"II*\0".to_vec()
    } else {
        b"MM\0*".to_vec()
    };
    out.extend(if le {
        8u32.to_le_bytes()
    } else {
        8u32.to_be_bytes()
    });
    put_ifd(&mut out, ifd0, le);
    debug_assert!(exif.is_empty() || out.len() == exif_off);
    if !exif.is_empty() {
        put_ifd(&mut out, exif, le);
    }
    debug_assert!(gps.is_empty() || out.len() == gps_off);
    if !gps.is_empty() {
        put_ifd(&mut out, gps, le);
    }
    (out.len() <= MAX_BLOCK).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::super::exif;
    use super::super::tests::{build_tiff, make_jpeg, Ifd as TIfd, V};
    use super::*;
    use crate::Metadata;

    /// Exif + GPS directories like a camera writes them, with a MakerNote and an Interop IFD.
    fn camera_ifds() -> (Vec<(u16, V)>, TIfd, TIfd) {
        let exif = TIfd {
            ents: vec![
                (0x9003, V::Ascii("2024:03:05 10:20:30".into())),
                (0x9291, V::Ascii("45".into())),
                (0x9011, V::Ascii("+02:00".into())),
                (0x829A, V::Rat(vec![(1, 250)])),
                (0x829D, V::Rat(vec![(28, 10)])),
                (0x8827, V::Short(vec![400])),
                (0x920A, V::Rat(vec![(50, 1)])),
                (0xA434, V::Ascii("RF50mm F1.8".into())),
                (0xA431, V::Ascii("SN12345".into())),
                (0xA002, V::Long(vec![6000])),
                (0xA003, V::Long(vec![4000])),
                (0x927C, V::Blob(b"MAKERNOTE-WITH-OFFSETS".to_vec())),
                (
                    0xA005,
                    V::Sub(vec![TIfd {
                        ents: vec![(0x0001, V::Ascii("R98".into()))],
                        next: None,
                    }]),
                ),
            ],
            next: None,
        };
        let gps = TIfd {
            ents: vec![
                (0x0001, V::Ascii("N".into())),
                (0x0002, V::Rat(vec![(31, 1), (30, 1), (0, 1)])),
                (0x0003, V::Ascii("W".into())),
                (0x0004, V::Rat(vec![(121, 1), (0, 1), (36, 1)])),
            ],
            next: None,
        };
        let ifd0 = vec![
            (0x010F, V::Ascii("Acme".into())),
            (0x0110, V::Ascii("X-1000".into())),
            (0x8298, V::Ascii("(c) Someone".into())),
            (0x0112, V::Short(vec![6])),
        ];
        (ifd0, exif, gps)
    }

    fn parse(block: &[u8]) -> Metadata {
        let mut md = Metadata::default();
        exif::fill_from_tiff(&mut md, &Tiff::new(block).unwrap());
        md
    }

    fn assert_camera_fields(block: &[u8], what: &str) {
        let md = parse(block);
        assert_eq!(md.camera_make.as_deref(), Some("Acme"), "{what}");
        assert_eq!(md.camera_model.as_deref(), Some("X-1000"), "{what}");
        assert_eq!(md.camera_serial.as_deref(), Some("SN12345"), "{what}");
        assert_eq!(md.lens.as_deref(), Some("RF50mm F1.8"), "{what}");
        assert_eq!(md.iso, Some(400), "{what}");
        assert_eq!(md.taken_at_offset_min, Some(120), "{what}");
        assert!(
            md.taken_at_ms.is_some_and(|t| t % 1000 == 450),
            "subseconds: {what}"
        );
        assert!((md.gps_lat.unwrap() - 31.5).abs() < 1e-9, "{what}");
        assert!((md.gps_lon.unwrap() + 121.01).abs() < 1e-9, "{what}");
        assert_eq!(md.orientation, 0, "orientation is not copied: {what}");
        assert!(!block.windows(9).any(|w| w == b"MAKERNOTE"), "{what}");
        let t = Tiff::new(block).unwrap();
        let w = t.walk();
        assert!(w.exif.as_ref().unwrap().get(0xA005).is_none(), "{what}");
        assert!(w.chain[0].get(0x8298).is_some(), "copyright kept: {what}");
    }

    #[test]
    fn tiff_based_raw_exif_is_rebuilt_without_layout_tags_and_makernote() {
        let dir = tempfile::tempdir().unwrap();
        for le in [true, false] {
            let (mut ifd0, exif, gps) = camera_ifds();
            let preview = make_jpeg(64, 48, |_, _| [9, 9, 9]);
            ifd0.extend([
                (0x0100, V::Long(vec![64])),
                (0x0101, V::Long(vec![48])),
                (0x0111, V::Long(vec![1234])),
                (0x0201, V::Blob(preview.clone())),
                (0x0202, V::Long(vec![preview.len() as u32])),
                (0x8769, V::Sub(vec![exif])),
                (0x8825, V::Sub(vec![gps])),
            ]);
            let raw = build_tiff(
                le,
                &TIfd {
                    ents: ifd0,
                    next: None,
                },
            );
            let p = dir.path().join(format!("x{le}.nef"));
            std::fs::write(&p, &raw).unwrap();
            let block = read_exif(&p, ImageFormat::Raw).unwrap().unwrap();
            assert_eq!(&block[..2], if le { b"II" } else { b"MM" });
            assert_camera_fields(&block, &format!("nef le={le}"));
            let t = Tiff::new(&block).unwrap();
            let ifd0 = &t.walk().chain[0];
            for tag in [0x0100, 0x0101, 0x0111, 0x0112, 0x0201, 0x0202] {
                assert!(ifd0.get(tag).is_none(), "tag {tag:#06x} le={le}");
            }
            // a plain TIFF picture goes the same way
            let p = dir.path().join(format!("x{le}.tif"));
            std::fs::write(&p, &raw).unwrap();
            let block = read_exif(&p, ImageFormat::Tiff).unwrap().unwrap();
            assert_camera_fields(&block, &format!("tiff le={le}"));
        }
    }

    #[test]
    fn cr3_directories_are_merged_into_one_block() {
        let dir = tempfile::tempdir().unwrap();
        let (ifd0, exif, gps) = camera_ifds();
        let cmt1 = build_tiff(
            true,
            &TIfd {
                ents: ifd0,
                next: None,
            },
        );
        let cmt2 = build_tiff(true, &exif);
        // big-endian GPS: values are converted to the block's byte order
        let cmt4 = build_tiff(false, &gps);
        let bx = |typ: &[u8; 4], payload: &[u8]| {
            let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
            v.extend_from_slice(typ);
            v.extend_from_slice(payload);
            v
        };
        let mut canon = vec![0u8; 16];
        canon.extend(bx(b"CMT1", &cmt1));
        canon.extend(bx(b"CMT2", &cmt2));
        canon.extend(bx(b"CMT4", &cmt4));
        let mut file = bx(b"ftyp", b"crx \0\0\0\x01crx isom");
        file.extend(bx(b"moov", &bx(b"uuid", &canon)));
        let p = dir.path().join("x.cr3");
        std::fs::write(&p, &file).unwrap();
        let block = read_exif(&p, ImageFormat::Raw).unwrap().unwrap();
        assert_eq!(&block[..2], b"II");
        assert_camera_fields(&block, "cr3");
    }

    #[test]
    fn jpeg_and_raf_blocks_are_returned_as_stored() {
        let dir = tempfile::tempdir().unwrap();
        let (mut ifd0, exif, gps) = camera_ifds();
        ifd0.extend([(0x8769, V::Sub(vec![exif])), (0x8825, V::Sub(vec![gps]))]);
        let tiff = build_tiff(
            true,
            &TIfd {
                ents: ifd0,
                next: None,
            },
        );
        let jpeg = make_jpeg(32, 24, |_, _| [1, 2, 3]);
        let mut with = vec![0xFF, 0xD8, 0xFF, 0xE1];
        with.extend(((tiff.len() + 8) as u16).to_be_bytes());
        with.extend(b"Exif\0\0");
        with.extend(&tiff);
        with.extend(&jpeg[2..]);
        let p = dir.path().join("a.jpg");
        std::fs::write(&p, &with).unwrap();
        assert_eq!(
            read_exif(&p, ImageFormat::Jpeg).unwrap(),
            Some(tiff.clone())
        );
        let mut raf = b"FUJIFILMCCD-RAW 0201FF39".to_vec();
        raf.resize(84, 0);
        raf.extend(200u32.to_be_bytes());
        raf.extend((with.len() as u32).to_be_bytes());
        raf.resize(200, 0);
        raf.extend(&with);
        let p = dir.path().join("a.raf");
        std::fs::write(&p, &raf).unwrap();
        assert_eq!(read_exif(&p, ImageFormat::Raw).unwrap(), Some(tiff));
        // no EXIF at all
        let p = dir.path().join("b.jpg");
        std::fs::write(&p, &jpeg).unwrap();
        assert_eq!(read_exif(&p, ImageFormat::Jpeg).unwrap(), None);
        assert!(read_exif(&dir.path().join("missing.jpg"), ImageFormat::Jpeg).is_err());
    }

    #[test]
    fn png_exif_chunk_is_read() {
        use image::ImageEncoder;
        let dir = tempfile::tempdir().unwrap();
        let (ifd0, _, _) = camera_ifds();
        let tiff = build_tiff(
            true,
            &TIfd {
                ents: ifd0,
                next: None,
            },
        );
        let mut bytes = Vec::new();
        let mut enc = image::codecs::png::PngEncoder::new(&mut bytes);
        enc.set_exif_metadata(tiff.clone()).unwrap();
        enc.write_image(&[0u8; 12], 2, 2, image::ExtendedColorType::Rgb8)
            .unwrap();
        let p = dir.path().join("a.png");
        std::fs::write(&p, bytes).unwrap();
        assert_eq!(read_exif(&p, ImageFormat::Png).unwrap(), Some(tiff));
    }
}
