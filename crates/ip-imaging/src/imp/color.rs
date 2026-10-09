//! Colour management of decoded pixels. Sources that declare a colour space other than sRGB (an
//! embedded ICC profile such as Display P3 or Adobe RGB (1998), or the DCF "Adobe RGB" EXIF
//! marker cameras write instead of a profile) are converted to sRGB right after decoding, so
//! thumbnails, previews, renders and exports (which are written as sRGB) keep their colours.
//! Untagged sources are taken to be sRGB and left alone, as are profiles that are sRGB under
//! another name. Out-of-gamut colours are clipped (relative colorimetric for matrix profiles).
//!
//! Uses `moxcms`: pure Rust (no C toolchain, so it builds for every target including Android),
//! BSD-3-Clause OR Apache-2.0, and already part of the build through the `image` crate.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use moxcms::{ColorProfile, DataColorSpace, Layout, Transform8BitExecutor, TransformOptions};
use rayon::prelude::*;

use super::container;
use super::io::open_data;
use super::tiff::{Tiff, TAG_EXIF_IFD};
use crate::ImageFormat;

/// Where the colours of a source are defined, when it says so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceSpace {
    /// An embedded ICC profile.
    Icc(Vec<u8>),
    /// DCF option file: EXIF ColorSpace = uncalibrated, interoperability index "R03".
    AdobeRgbDcf,
}

/// The ICC profile of a JPEG: its APP2 `ICC_PROFILE` chunks joined in sequence order. `None`
/// when there is none or the chunks are incomplete.
pub fn jpeg_icc(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return None;
    }
    let mut chunks: Vec<(u8, u8, &[u8])> = Vec::new();
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
        let Some(seg) = data.get(p + 4..p + 2 + len) else {
            break;
        };
        if m == 0xE2 && seg.len() > 14 && seg.starts_with(b"ICC_PROFILE\0") {
            chunks.push((seg[12], seg[13], &seg[14..]));
        }
        p += 2 + len;
    }
    let count = chunks.first()?.1;
    if count == 0 || chunks.len() != count as usize || chunks.iter().any(|c| c.1 != count) {
        return None;
    }
    chunks.sort_by_key(|c| c.0);
    if chunks
        .iter()
        .enumerate()
        .any(|(i, c)| c.0 as usize != i + 1)
    {
        return None;
    }
    Some(chunks.iter().flat_map(|c| c.2.iter().copied()).collect())
}

/// The DCF "Adobe RGB" marker of an EXIF block: ColorSpace (0xA001) = 0xFFFF and the
/// interoperability index (0x0001 of the Interop IFD) = "R03".
fn exif_says_adobe_rgb(tiff: &[u8]) -> bool {
    let Some(t) = Tiff::new(tiff) else {
        return false;
    };
    let ifd = |off: Option<u32>| off.and_then(|o| t.read_ifd(o as usize)).map(|(i, _)| i);
    let Some(ifd0) = ifd(t.first_ifd_offset().map(|o| o as u32)) else {
        return false;
    };
    let Some(exif) = ifd(ifd0.get(TAG_EXIF_IFD).and_then(|e| t.first_uint(e))) else {
        return false;
    };
    if exif.get(0xA001).and_then(|e| t.first_uint(e)) != Some(0xFFFF) {
        return false;
    }
    ifd(exif.get(0xA005).and_then(|e| t.first_uint(e)))
        .and_then(|iop| iop.get(0x0001).and_then(|e| t.ascii(e)))
        .is_some_and(|s| s == "R03")
}

fn jpeg_space(data: &[u8]) -> Option<SourceSpace> {
    if let Some(icc) = jpeg_icc(data) {
        return Some(SourceSpace::Icc(icc));
    }
    let tiff = container::jpeg_info(data)?.exif_tiff?;
    exif_says_adobe_rgb(tiff).then_some(SourceSpace::AdobeRgbDcf)
}

/// The colour space a source file declares for the pixels the decoder returns; `None` for
/// untagged (sRGB) files. RAW files are judged by the embedded preview that gets decoded.
pub fn source_space(path: &Path, format: ImageFormat) -> Option<SourceSpace> {
    match format {
        ImageFormat::Jpeg => jpeg_space(&open_data(path).ok()?),
        ImageFormat::Raw => jpeg_space(container::raw_preview(&open_data(path).ok()?)?),
        ImageFormat::Png | ImageFormat::Webp | ImageFormat::Tiff => {
            use image::ImageDecoder;
            let mut dec = image::ImageReader::open(path)
                .ok()?
                .with_guessed_format()
                .ok()?
                .into_decoder()
                .ok()?;
            dec.icc_profile().ok().flatten().map(SourceSpace::Icc)
        }
        // No pixel decoder for these yet; whoever adds one decides whether its output still
        // needs the `colr` profile applied here.
        ImageFormat::Heif | ImageFormat::Avif => None,
    }
}

type Transform = Arc<Transform8BitExecutor>;

/// A probe of 9^3 colours: a transform that moves none of them by more than one code value is
/// an sRGB profile under another name.
fn is_identity(t: &Transform8BitExecutor) -> Option<bool> {
    let steps: Vec<u8> = (0..=8u32).map(|i| (i * 255 / 8) as u8).collect();
    let mut src = Vec::with_capacity(steps.len().pow(3) * 3);
    for &r in &steps {
        for &g in &steps {
            for &b in &steps {
                src.extend_from_slice(&[r, g, b]);
            }
        }
    }
    let mut dst = vec![0u8; src.len()];
    t.transform(&src, &mut dst).ok()?;
    Some(src.iter().zip(&dst).all(|(a, b)| a.abs_diff(*b) <= 1))
}

/// `None` when there is nothing to do: an sRGB-equivalent, non-RGB or unusable profile.
fn build_transform(space: &SourceSpace) -> Option<Transform> {
    let src = match space {
        SourceSpace::Icc(bytes) => ColorProfile::new_from_slice(bytes).ok()?,
        SourceSpace::AdobeRgbDcf => ColorProfile::new_adobe_rgb(),
    };
    if src.color_space != DataColorSpace::Rgb {
        return None;
    }
    let t = src
        .create_transform_8bit(
            Layout::Rgb,
            &ColorProfile::new_srgb(),
            Layout::Rgb,
            TransformOptions::default(),
        )
        .ok()?;
    (!is_identity(&*t)?).then_some(t)
}

/// Transforms are cached per profile (a library holds few distinct ones).
fn transform_for(space: &SourceSpace) -> Option<Transform> {
    type Cache = Mutex<HashMap<[u8; 32], Option<Transform>>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let key = match space {
        SourceSpace::Icc(bytes) => *blake3::hash(bytes).as_bytes(),
        SourceSpace::AdobeRgbDcf => [0; 32],
    };
    let cache = CACHE.get_or_init(Cache::default);
    if let Some(t) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return t.clone();
    }
    let t = build_transform(space);
    let mut c = cache.lock().unwrap_or_else(|e| e.into_inner());
    if c.len() >= 64 {
        c.clear();
    }
    c.insert(key, t.clone());
    t
}

/// Converts packed RGB8 pixels from `space` to sRGB. Returns whether anything was done; on
/// failure the pixels are left as they were.
pub fn convert(space: &SourceSpace, rgb: &mut Vec<u8>) -> bool {
    const CHUNK: usize = 3 * 16 * 1024;
    let Some(t) = transform_for(space) else {
        return false;
    };
    let mut out = vec![0u8; rgb.len()];
    let ok = out
        .par_chunks_mut(CHUNK)
        .zip(rgb.par_chunks(CHUNK))
        .all(|(d, s)| s.len() % 3 == 0 && t.transform(s, d).is_ok());
    if ok {
        *rgb = out;
    }
    ok
}

/// Converts decoded pixels of `path` to sRGB when the file declares another colour space.
/// Best effort: unreadable or unsupported profiles leave the pixels as they are.
pub fn to_srgb(path: &Path, format: ImageFormat, rgb: &mut Vec<u8>) {
    if let Some(space) = source_space(path, format) {
        convert(&space, rgb);
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{build_tiff, make_jpeg, Ifd, V};
    use super::*;
    use crate::{decode_rgb8, generate_thumbnail};

    fn lin_srgb(v: u8) -> f64 {
        let c = v as f64 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    fn lin_adobe(v: u8) -> f64 {
        (v as f64 / 255.0).powf(563.0 / 256.0)
    }

    fn enc_srgb(l: f64) -> u8 {
        let l = l.clamp(0.0, 1.0);
        let c = if l <= 0.003_130_8 {
            12.92 * l
        } else {
            1.055 * l.powf(1.0 / 2.4) - 0.055
        };
        (c * 255.0).round() as u8
    }

    /// Linear Display P3 -> linear sRGB (both D65; from the primaries, not from moxcms).
    const P3_TO_SRGB: [[f64; 3]; 3] = [
        [1.224_940_2, -0.224_940_4, 0.0],
        [-0.042_056_9, 1.042_057_1, 0.0],
        [-0.019_637_6, -0.078_636_1, 1.098_273_5],
    ];
    /// Linear Adobe RGB (1998) -> linear sRGB.
    const ADOBE_TO_SRGB: [[f64; 3]; 3] = [
        [1.398_283_2, -0.398_283_1, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, -0.042_938_3, 1.042_938_3],
    ];

    /// Reference conversion of one colour.
    pub fn expected(m: &[[f64; 3]; 3], lin: fn(u8) -> f64, px: [u8; 3]) -> [u8; 3] {
        let l = px.map(lin);
        let row = |r: &[f64; 3]| enc_srgb(r[0] * l[0] + r[1] * l[1] + r[2] * l[2]);
        [row(&m[0]), row(&m[1]), row(&m[2])]
    }

    fn s15(v: f64) -> [u8; 4] {
        ((v * 65536.0).round() as i32).to_be_bytes()
    }

    /// A minimal ICC v2 display profile in the style of Adobe's own "Adobe RGB (1998)": D50
    /// colorants, a pure gamma `curv` shared by the three channels, `desc` + `wtpt`.
    fn adobe_rgb_v2() -> Vec<u8> {
        let xyz = |v: [f64; 3]| {
            let mut t = b"XYZ \0\0\0\0".to_vec();
            v.iter().for_each(|c| t.extend(s15(*c)));
            t
        };
        let mut desc = b"desc\0\0\0\0".to_vec();
        let name = b"Adobe RGB (1998)\0";
        desc.extend((name.len() as u32).to_be_bytes());
        desc.extend(name);
        desc.extend([0u8; 4 + 4 + 2 + 1 + 67]);
        let mut curv = b"curv\0\0\0\0".to_vec();
        curv.extend(1u32.to_be_bytes());
        curv.extend(0x0233u16.to_be_bytes()); // 2.19921875 as u8Fixed8
        let tags: Vec<([u8; 4], Vec<u8>)> = vec![
            (*b"desc", desc),
            (*b"wtpt", xyz([0.964_20, 1.0, 0.824_91])),
            (*b"rXYZ", xyz([0.609_74, 0.311_11, 0.019_47])),
            (*b"gXYZ", xyz([0.205_28, 0.625_67, 0.060_87])),
            (*b"bXYZ", xyz([0.149_19, 0.063_22, 0.744_57])),
            (*b"rTRC", curv.clone()),
            (*b"gTRC", curv.clone()),
            (*b"bTRC", curv),
        ];
        let mut table = Vec::new();
        let mut body = Vec::new();
        let mut off = 128 + 4 + 12 * tags.len();
        for (sig, mut data) in tags {
            let len = data.len();
            while data.len() % 4 != 0 {
                data.push(0);
            }
            table.extend(sig);
            table.extend((off as u32).to_be_bytes());
            table.extend((len as u32).to_be_bytes());
            off += data.len();
            body.extend(data);
        }
        let mut p = vec![0u8; 128];
        let total = 128 + 4 + table.len() + body.len();
        p[0..4].copy_from_slice(&(total as u32).to_be_bytes());
        p[8..12].copy_from_slice(&0x0210_0000u32.to_be_bytes());
        p[12..16].copy_from_slice(b"mntr");
        p[16..20].copy_from_slice(b"RGB ");
        p[20..24].copy_from_slice(b"XYZ ");
        p[36..40].copy_from_slice(b"acsp");
        p[68..72].copy_from_slice(&s15(0.9642));
        p[72..76].copy_from_slice(&s15(1.0));
        p[76..80].copy_from_slice(&s15(0.8249));
        p.extend(((table.len() / 12) as u32).to_be_bytes());
        p.extend(table);
        p.extend(body);
        p
    }

    pub fn display_p3() -> Vec<u8> {
        ColorProfile::new_display_p3().encode().unwrap()
    }

    /// `jpeg` with `icc` in APP2 chunks of at most `chunk` bytes, written in `order`.
    pub fn with_icc(jpeg: &[u8], icc: &[u8], chunk: usize, order: &[usize]) -> Vec<u8> {
        let parts: Vec<&[u8]> = icc.chunks(chunk).collect();
        let mut out = jpeg[..2].to_vec();
        for &i in order {
            let mut seg = b"ICC_PROFILE\0".to_vec();
            seg.push(i as u8 + 1);
            seg.push(parts.len() as u8);
            seg.extend(parts[i]);
            out.extend([0xFF, 0xE2]);
            out.extend(((seg.len() + 2) as u16).to_be_bytes());
            out.extend(seg);
        }
        out.extend(&jpeg[2..]);
        out
    }

    fn solid(px: [u8; 3]) -> Vec<u8> {
        make_jpeg(32, 24, |_, _| px)
    }

    fn center(path: &Path) -> [u8; 3] {
        let (w, h, rgb) = decode_rgb8(path, ImageFormat::Jpeg, 1, 4096).unwrap();
        let i = (((h / 2) * w + w / 2) * 3) as usize;
        [rgb[i], rgb[i + 1], rgb[i + 2]]
    }

    fn close(a: [u8; 3], b: [u8; 3], tol: u8) -> bool {
        a.iter().zip(&b).all(|(x, y)| x.abs_diff(*y) <= tol)
    }

    #[test]
    fn icc_chunks_are_joined_in_sequence_order() {
        let icc: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        let jpeg = solid([10, 20, 30]);
        let tagged = with_icc(&jpeg, &icc, 400, &[2, 0, 1]);
        assert_eq!(jpeg_icc(&tagged).as_deref(), Some(icc.as_slice()));
        // a missing chunk invalidates the profile
        let partial = with_icc(&jpeg, &icc, 400, &[0, 2]);
        assert!(jpeg_icc(&partial).is_none());
        assert!(jpeg_icc(&jpeg).is_none());
        assert!(jpeg_icc(b"not a jpeg").is_none());
    }

    /// (name, ICC profile, primaries matrix to sRGB, linearisation of the source)
    type Case = (&'static str, Vec<u8>, &'static [[f64; 3]; 3], fn(u8) -> f64);

    #[test]
    fn display_p3_and_adobe_rgb_sources_are_converted_to_srgb() {
        let dir = tempfile::tempdir().unwrap();
        let cases: [Case; 2] = [
            ("p3", display_p3(), &P3_TO_SRGB, lin_srgb),
            ("adobe", adobe_rgb_v2(), &ADOBE_TO_SRGB, lin_adobe),
        ];
        for (name, icc, m, lin) in cases {
            for px in [[230, 60, 40], [40, 200, 90], [70, 90, 220], [128, 128, 128]] {
                let jpeg = solid(px);
                let plain = dir.path().join(format!("{name}_plain.jpg"));
                std::fs::write(&plain, &jpeg).unwrap();
                let tagged = dir.path().join(format!("{name}.jpg"));
                std::fs::write(&tagged, with_icc(&jpeg, &icc, 60_000, &[0])).unwrap();
                // what the decoder returns for the stored values, then the reference conversion
                let stored = center(&plain);
                let want = expected(m, lin, stored);
                let got = center(&tagged);
                assert!(
                    close(got, want, 2),
                    "{name} {px:?}: got {got:?}, want {want:?}"
                );
                if px != [128, 128, 128] {
                    assert!(!close(got, stored, 4), "{name} {px:?} was not converted");
                } else {
                    assert!(close(got, stored, 1), "neutral grey stays grey");
                }
                // thumbnails go through the same path
                let e = generate_thumbnail(&tagged, ImageFormat::Jpeg, 1, 16, 95).unwrap();
                let t = image::load_from_memory(&e.bytes).unwrap().to_rgb8();
                let p = t.get_pixel(t.width() / 2, t.height() / 2).0;
                assert!(close(p, want, 4), "{name} thumbnail {p:?} vs {want:?}");
            }
        }
    }

    #[test]
    fn untagged_and_srgb_tagged_sources_are_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let jpeg = make_jpeg(40, 30, |x, y| [(x * 6) as u8, (y * 8) as u8, 200]);
        let plain = dir.path().join("plain.jpg");
        std::fs::write(&plain, &jpeg).unwrap();
        let srgb = ColorProfile::new_srgb().encode().unwrap();
        let tagged = dir.path().join("srgb.jpg");
        std::fs::write(&tagged, with_icc(&jpeg, &srgb, 60_000, &[0])).unwrap();
        let junk = dir.path().join("junk.jpg");
        std::fs::write(
            &junk,
            with_icc(&jpeg, b"definitely not a profile", 100, &[0]),
        )
        .unwrap();
        let a = decode_rgb8(&plain, ImageFormat::Jpeg, 1, 100).unwrap();
        assert_eq!(decode_rgb8(&tagged, ImageFormat::Jpeg, 1, 100).unwrap(), a);
        assert_eq!(decode_rgb8(&junk, ImageFormat::Jpeg, 1, 100).unwrap(), a);
        assert_eq!(source_space(&plain, ImageFormat::Jpeg), None);
    }

    #[test]
    fn dcf_adobe_rgb_marker_without_a_profile_is_honoured() {
        let dir = tempfile::tempdir().unwrap();
        let exif = |space: u16, index: &str| {
            build_tiff(
                true,
                &Ifd {
                    ents: vec![(
                        TAG_EXIF_IFD,
                        V::Sub(vec![Ifd {
                            ents: vec![
                                (0xA001, V::Short(vec![space])),
                                (
                                    0xA005,
                                    V::Sub(vec![Ifd {
                                        ents: vec![(0x0001, V::Ascii(index.into()))],
                                        next: None,
                                    }]),
                                ),
                            ],
                            next: None,
                        }]),
                    )],
                    next: None,
                },
            )
        };
        let px = [200, 80, 60];
        let jpeg = solid(px);
        let inject = |tiff: &[u8]| {
            let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
            out.extend(((tiff.len() + 8) as u16).to_be_bytes());
            out.extend(b"Exif\0\0");
            out.extend(tiff);
            out.extend(&jpeg[2..]);
            out
        };
        let plain = dir.path().join("plain.jpg");
        std::fs::write(&plain, &jpeg).unwrap();
        let adobe = dir.path().join("_DSC0001.jpg");
        std::fs::write(&adobe, inject(&exif(0xFFFF, "R03"))).unwrap();
        let srgb = dir.path().join("DSC0002.jpg");
        std::fs::write(&srgb, inject(&exif(1, "R98"))).unwrap();
        assert_eq!(
            source_space(&adobe, ImageFormat::Jpeg),
            Some(SourceSpace::AdobeRgbDcf)
        );
        assert_eq!(source_space(&srgb, ImageFormat::Jpeg), None);
        let stored = center(&plain);
        let want = expected(&ADOBE_TO_SRGB, lin_adobe, stored);
        assert!(
            close(center(&adobe), want, 2),
            "{:?} vs {want:?}",
            center(&adobe)
        );
        assert_eq!(center(&srgb), stored);
    }

    #[test]
    fn png_profiles_are_applied_too() {
        use image::ImageEncoder;
        let dir = tempfile::tempdir().unwrap();
        let px = [230u8, 60, 40];
        let img = image::RgbImage::from_pixel(8, 8, image::Rgb(px));
        let mut bytes = Vec::new();
        let mut enc = image::codecs::png::PngEncoder::new(&mut bytes);
        enc.set_icc_profile(display_p3()).unwrap();
        enc.write_image(img.as_raw(), 8, 8, image::ExtendedColorType::Rgb8)
            .unwrap();
        let p = dir.path().join("p3.png");
        std::fs::write(&p, bytes).unwrap();
        let (_, _, rgb) = decode_rgb8(&p, ImageFormat::Png, 1, 64).unwrap();
        let want = expected(&P3_TO_SRGB, lin_srgb, px);
        assert!(
            close([rgb[0], rgb[1], rgb[2]], want, 2),
            "{:?} vs {want:?}",
            &rgb[..3]
        );
    }
}
