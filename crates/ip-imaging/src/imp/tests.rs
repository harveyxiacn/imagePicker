//! Unit tests; every fixture is generated programmatically.
use std::fs;
use std::path::{Path, PathBuf};

use crate::*;

// ------------------------------------------------------------ fixtures

pub fn make_jpeg(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            rgb.extend_from_slice(&f(x, y));
        }
    }
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, 90)
        .encode(&rgb, w as u16, h as u16, jpeg_encoder::ColorType::Rgb)
        .unwrap();
    out
}

fn gradient(x: u32, y: u32) -> [u8; 3] {
    [(x % 251) as u8, (y % 241) as u8, ((x + y) % 239) as u8]
}

/// Red 16x8 block in the top-left corner, blue elsewhere.
fn marker(x: u32, y: u32) -> [u8; 3] {
    if x < 16 && y < 8 {
        [255, 0, 0]
    } else {
        [0, 0, 255]
    }
}

pub enum V {
    Short(Vec<u16>),
    Long(Vec<u32>),
    Ascii(String),
    Rat(Vec<(u32, u32)>),
    Sub(Vec<Ifd>),
    /// Tag holds a LONG offset to this blob.
    Blob(Vec<u8>),
}

pub struct Ifd {
    pub ents: Vec<(u16, V)>,
    pub next: Option<Box<Ifd>>,
}

fn p16(b: &mut Vec<u8>, v: u16, le: bool) {
    b.extend_from_slice(&if le { v.to_le_bytes() } else { v.to_be_bytes() });
}
fn p32(b: &mut Vec<u8>, v: u32, le: bool) {
    b.extend_from_slice(&if le { v.to_le_bytes() } else { v.to_be_bytes() });
}

fn write_ifd(buf: &mut Vec<u8>, ifd: &Ifd, le: bool) -> usize {
    let next = ifd.next.as_ref().map(|n| write_ifd(buf, n, le));
    let mut table: Vec<(u16, u16, u32, [u8; 4])> = Vec::new();
    for (tag, v) in &ifd.ents {
        let (typ, count, payload): (u16, u32, Vec<u8>) = match v {
            V::Short(s) => {
                let mut p = vec![];
                s.iter().for_each(|x| p16(&mut p, *x, le));
                (3, s.len() as u32, p)
            }
            V::Long(s) => {
                let mut p = vec![];
                s.iter().for_each(|x| p32(&mut p, *x, le));
                (4, s.len() as u32, p)
            }
            V::Ascii(s) => {
                let mut p = s.clone().into_bytes();
                p.push(0);
                (2, p.len() as u32, p)
            }
            V::Rat(r) => {
                let mut p = vec![];
                r.iter().for_each(|(n, d)| {
                    p32(&mut p, *n, le);
                    p32(&mut p, *d, le);
                });
                (5, r.len() as u32, p)
            }
            V::Sub(subs) => {
                let offs: Vec<u32> = subs.iter().map(|s| write_ifd(buf, s, le) as u32).collect();
                let mut p = vec![];
                offs.iter().for_each(|x| p32(&mut p, *x, le));
                (4, offs.len() as u32, p)
            }
            V::Blob(b) => {
                if buf.len() % 2 == 1 {
                    buf.push(0);
                }
                let off = buf.len() as u32;
                buf.extend_from_slice(b);
                let mut p = vec![];
                p32(&mut p, off, le);
                (4, 1, p)
            }
        };
        let mut val = [0u8; 4];
        if payload.len() <= 4 {
            val[..payload.len()].copy_from_slice(&payload);
        } else {
            if buf.len() % 2 == 1 {
                buf.push(0);
            }
            let off = buf.len() as u32;
            buf.extend_from_slice(&payload);
            let mut p = vec![];
            p32(&mut p, off, le);
            val.copy_from_slice(&p);
        }
        table.push((*tag, typ, count, val));
    }
    table.sort_by_key(|t| t.0);
    if buf.len() % 2 == 1 {
        buf.push(0);
    }
    let off = buf.len();
    p16(buf, table.len() as u16, le);
    for (tag, typ, count, val) in table {
        p16(buf, tag, le);
        p16(buf, typ, le);
        p32(buf, count, le);
        buf.extend_from_slice(&val);
    }
    p32(buf, next.unwrap_or(0) as u32, le);
    off
}

pub fn build_tiff(le: bool, ifd0: &Ifd) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(if le { b"II" } else { b"MM" });
    p16(&mut buf, 42, le);
    p32(&mut buf, 0, le);
    let off = write_ifd(&mut buf, ifd0, le) as u32;
    let mut h = Vec::new();
    p32(&mut h, off, le);
    buf[4..8].copy_from_slice(&h);
    buf
}

fn inject_exif(jpeg: &[u8], tiff: &[u8]) -> Vec<u8> {
    let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
    out.extend_from_slice(&((tiff.len() + 8) as u16).to_be_bytes());
    out.extend_from_slice(b"Exif\0\0");
    out.extend_from_slice(tiff);
    out.extend_from_slice(&jpeg[2..]);
    out
}

/// Red top-left eighth, blue elsewhere (survives heavy downscaling).
fn big_marker(x: u32, y: u32) -> [u8; 3] {
    if x < 100 && y < 80 {
        [255, 0, 0]
    } else {
        [0, 0, 255]
    }
}

fn full_exif_tiff(le: bool) -> Vec<u8> {
    let exif = Ifd {
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
        ],
        next: None,
    };
    let gps = Ifd {
        ents: vec![
            (0x0001, V::Ascii("N".into())),
            (0x0002, V::Rat(vec![(31, 1), (30, 1), (0, 1)])),
            (0x0003, V::Ascii("W".into())),
            (0x0004, V::Rat(vec![(121, 1), (0, 1), (36, 1)])),
        ],
        next: None,
    };
    let ifd0 = Ifd {
        ents: vec![
            (0x010F, V::Ascii("Acme".into())),
            (0x0110, V::Ascii("X-1000".into())),
            (0x0112, V::Short(vec![6])),
            (0x8769, V::Sub(vec![exif])),
            (0x8825, V::Sub(vec![gps])),
        ],
        next: None,
    };
    build_tiff(le, &ifd0)
}

fn bx(typ: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
    v.extend_from_slice(typ);
    v.extend_from_slice(payload);
    v
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, bytes).unwrap();
    p
}

fn px(rgb: &[u8], w: u32, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * w + x) * 3) as usize;
    [rgb[i], rgb[i + 1], rgb[i + 2]]
}

fn is_red(p: [u8; 3]) -> bool {
    p[0] > 180 && p[1] < 90 && p[2] < 90
}

fn decode_out(e: &EncodedImage) -> Vec<u8> {
    let img = image::load_from_memory_with_format(&e.bytes, image::ImageFormat::Jpeg)
        .unwrap()
        .to_rgb8();
    assert_eq!((img.width(), img.height()), (e.width, e.height));
    img.into_raw()
}

// ------------------------------------------------------------ tests

#[test]
fn format_detection() {
    let f = |s: &str| ImageFormat::from_path(Path::new(s));
    assert_eq!(f("a.JPG"), Some(ImageFormat::Jpeg));
    assert_eq!(f("a.jpeg"), Some(ImageFormat::Jpeg));
    assert_eq!(f("a.Png"), Some(ImageFormat::Png));
    assert_eq!(f("a.webp"), Some(ImageFormat::Webp));
    assert_eq!(f("a.HEIC"), Some(ImageFormat::Heif));
    assert_eq!(f("a.avif"), Some(ImageFormat::Avif));
    assert_eq!(f("a.tif"), Some(ImageFormat::Tiff));
    for e in [
        "cr2", "cr3", "NEF", "arw", "raf", "orf", "rw2", "dng", "pef",
    ] {
        assert_eq!(f(&format!("x.{e}")), Some(ImageFormat::Raw), "{e}");
    }
    assert_eq!(f("a.txt"), None);
    assert_eq!(f("noext"), None);
}

#[test]
fn orient_all_eight_mapping() {
    // 3x2 image, pixel value = index; expected from the EXIF spec.
    let (w, h) = (3u32, 2u32);
    let src: Vec<u8> = (0..6u8).flat_map(|i| [i, i, i]).collect();
    // rows of expected output (as source indices)
    let cases: [(u8, u32, u32, Vec<u8>); 8] = [
        (1, 3, 2, vec![0, 1, 2, 3, 4, 5]),
        (2, 3, 2, vec![2, 1, 0, 5, 4, 3]),
        (3, 3, 2, vec![5, 4, 3, 2, 1, 0]),
        (4, 3, 2, vec![3, 4, 5, 0, 1, 2]),
        (5, 2, 3, vec![0, 3, 1, 4, 2, 5]),
        (6, 2, 3, vec![3, 0, 4, 1, 5, 2]),
        (7, 2, 3, vec![5, 2, 4, 1, 3, 0]),
        (8, 2, 3, vec![2, 5, 1, 4, 0, 3]),
    ];
    for (o, ew, eh, exp) in cases {
        let (ow, oh, out) = super::orient::apply(w, h, src.clone(), o);
        assert_eq!((ow, oh), (ew, eh), "orientation {o}");
        let got: Vec<u8> = out.chunks(3).map(|p| p[0]).collect();
        assert_eq!(got, exp, "orientation {o}");
    }
}

#[test]
fn thumbnail_orientations_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "m.jpg", &make_jpeg(128, 64, marker));
    // expected corner of the red marker after each orientation
    let corner = |o: u8| match o {
        1 | 5 => (0, 0),
        2 | 6 => (1, 0),
        3 | 7 => (1, 1),
        _ => (0, 1),
    };
    for o in 1..=8u8 {
        let e = generate_thumbnail(&p, ImageFormat::Jpeg, o, 128, 90).unwrap();
        let (ew, eh) = if o >= 5 { (64, 128) } else { (128, 64) };
        assert_eq!((e.width, e.height), (ew, eh), "o={o}");
        let rgb = decode_out(&e);
        let (cx, cy) = corner(o);
        let x = if cx == 0 { 3 } else { e.width - 4 };
        let y = if cy == 0 { 3 } else { e.height - 4 };
        assert!(
            is_red(px(&rgb, e.width, x, y)),
            "o={o} expected red at ({x},{y}) got {:?}",
            px(&rgb, e.width, x, y)
        );
        // opposite corner is blue
        let ox = e.width - 1 - x;
        let oy = e.height - 1 - y;
        assert!(!is_red(px(&rgb, e.width, ox, oy)), "o={o}");
    }
}

#[test]
fn never_upscales() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "s.jpg", &make_jpeg(100, 50, gradient));
    let e = generate_thumbnail(&p, ImageFormat::Jpeg, 1, 256, 80).unwrap();
    assert_eq!((e.width, e.height), (100, 50));
    let (w, h, rgb) = decode_rgb8(&p, ImageFormat::Jpeg, 6, 4096).unwrap();
    assert_eq!((w, h), (50, 100));
    assert_eq!(rgb.len(), 50 * 100 * 3);
    let png = dir.path().join("s.png");
    image::RgbImage::new(40, 30).save(&png).unwrap();
    let e = generate_thumbnail(&png, ImageFormat::Png, 1, 256, 80).unwrap();
    assert_eq!((e.width, e.height), (40, 30));
}

#[test]
fn dct_scaled_path_and_exact_size() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "big.jpg", &make_jpeg(2000, 1500, gradient));
    let e = generate_thumbnail(&p, ImageFormat::Jpeg, 1, 256, 80).unwrap();
    assert_eq!(e.source, ThumbSource::DctScaled);
    assert_eq!((e.width, e.height), (256, 192));
    let (w, h, _) = decode_rgb8(&p, ImageFormat::Jpeg, 1, 1000).unwrap();
    assert_eq!((w, h), (1000, 750));
}

#[test]
fn png_webp_tiff_thumbnails_and_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let img = image::RgbImage::from_fn(300, 200, |x, y| image::Rgb([x as u8, y as u8, 7]));
    for (name, fmt) in [
        ("a.png", ImageFormat::Png),
        ("a.tif", ImageFormat::Tiff),
        ("a.webp", ImageFormat::Webp),
    ] {
        let p = dir.path().join(name);
        if name.ends_with("webp") {
            // lossless webp via image's encoder
            let f = fs::File::create(&p).unwrap();
            image::codecs::webp::WebPEncoder::new_lossless(f)
                .encode(img.as_raw(), 300, 200, image::ExtendedColorType::Rgb8)
                .unwrap();
        } else {
            img.save(&p).unwrap();
        }
        let md = read_metadata(&p, fmt).unwrap();
        assert_eq!(
            (md.width, md.height, md.orientation),
            (Some(300), Some(200), 1),
            "{name}"
        );
        let e = generate_thumbnail(&p, fmt, 1, 100, 80).unwrap();
        assert_eq!(
            (e.width, e.height, e.source),
            (100, 67, ThumbSource::FullDecode),
            "{name}"
        );
    }
}

#[test]
fn metadata_full_exif_both_endians() {
    let dir = tempfile::tempdir().unwrap();
    for le in [true, false] {
        let jpeg = inject_exif(&make_jpeg(64, 48, gradient), &full_exif_tiff(le));
        let p = write(dir.path(), "e.jpg", &jpeg);
        let md = read_metadata(&p, ImageFormat::Jpeg).unwrap();
        assert_eq!((md.width, md.height), (Some(64), Some(48)));
        assert_eq!(md.orientation, 6);
        assert_eq!(md.camera_make.as_deref(), Some("Acme"));
        assert_eq!(md.camera_model.as_deref(), Some("X-1000"));
        assert_eq!(md.camera_serial.as_deref(), Some("SN12345"));
        assert_eq!(md.lens.as_deref(), Some("RF50mm F1.8"));
        assert_eq!(md.focal_mm, Some(50.0));
        assert_eq!(md.aperture, Some(2.8));
        assert_eq!(md.shutter_s, Some(0.004));
        assert_eq!(md.iso, Some(400));
        // 10:20:30.450 at +02:00 -> 08:20:30.450 UTC
        assert_eq!(md.taken_at_ms, Some(1_709_634_030_450 - 2 * 3_600_000));
        assert!((md.gps_lat.unwrap() - 31.5).abs() < 1e-9);
        assert!((md.gps_lon.unwrap() + 121.01).abs() < 1e-9);
    }
}

#[test]
fn missing_exif_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "n.jpg", &make_jpeg(80, 60, gradient));
    let md = read_metadata(&p, ImageFormat::Jpeg).unwrap();
    assert_eq!(
        (md.width, md.height, md.orientation),
        (Some(80), Some(60), 1)
    );
    assert!(md.taken_at_ms.is_none() && md.camera_make.is_none());
    // garbage file: still Ok with defaults
    let g = write(dir.path(), "g.jpg", b"not a jpeg at all");
    let md = read_metadata(&g, ImageFormat::Jpeg).unwrap();
    assert_eq!((md.width, md.orientation), (None, 1));
    // missing file: IO error
    assert!(read_metadata(&dir.path().join("nope.jpg"), ImageFormat::Jpeg).is_err());
}

#[test]
fn scan_excludes_hidden_sorted() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    let j = make_jpeg(8, 8, gradient);
    write(r, "b.jpg", &j);
    write(r, "a.JPG", &j);
    write(r, "sub/c.cr2", b"x");
    write(r, "sub/deep/d.png", b"x");
    write(r, ".hidden/h.jpg", &j);
    write(r, ".imagepicker/cache/t.jpg", &j);
    write(r, ".dot.jpg", &j);
    write(r, "SkipMe/e.jpg", &j);
    write(r, "sub/readme.txt", b"x");
    let names = |v: Vec<ScannedFile>| -> Vec<String> {
        v.iter()
            .map(|f| {
                f.path
                    .strip_prefix(r)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    };
    let all = scan_dir(r, true, &[]).unwrap();
    assert_eq!(
        names(all.clone()),
        [
            "SkipMe/e.jpg",
            "a.JPG",
            "b.jpg",
            "sub/c.cr2",
            "sub/deep/d.png"
        ]
    );
    assert!(all.iter().all(|f| f.size > 0 && f.mtime_ms > 0));
    assert_eq!(all[3].format, ImageFormat::Raw);
    let ex = scan_dir(r, true, &["skipme".into(), "DEEP".into()]).unwrap();
    assert_eq!(names(ex), ["a.JPG", "b.jpg", "sub/c.cr2"]);
    let flat = scan_dir(r, false, &[]).unwrap();
    assert_eq!(names(flat), ["a.JPG", "b.jpg"]);
    assert!(scan_dir(&r.join("missing"), true, &[]).is_err());
}

#[test]
fn content_key_stable_and_sensitive() {
    let dir = tempfile::tempdir().unwrap();
    let data: Vec<u8> = (0..300_000u32)
        .map(|i| (i.wrapping_mul(2654435761) >> 24) as u8)
        .collect();
    let a = write(dir.path(), "a.bin", &data);
    let b = write(dir.path(), "moved/b.bin", &data);
    let ka = content_key(&a).unwrap();
    assert_eq!(ka.len(), 32);
    assert_eq!(ka, content_key(&a).unwrap());
    assert_eq!(ka, content_key(&b).unwrap());
    let mut d2 = data.clone();
    *d2.last_mut().unwrap() ^= 1;
    assert_ne!(ka, content_key(&write(dir.path(), "c.bin", &d2)).unwrap());
    let mut d3 = data.clone();
    d3[0] ^= 1;
    assert_ne!(ka, content_key(&write(dir.path(), "d.bin", &d3)).unwrap());
    let mut d4 = data.clone();
    d4.push(0);
    assert_ne!(ka, content_key(&write(dir.path(), "e.bin", &d4)).unwrap());
    // tiny and empty files work
    content_key(&write(dir.path(), "t.bin", b"abc")).unwrap();
    content_key(&write(dir.path(), "z.bin", b"")).unwrap();
}

#[test]
fn embedded_exif_thumbnail_used_when_large_enough() {
    let dir = tempfile::tempdir().unwrap();
    let thumb = make_jpeg(160, 120, gradient);
    let ifd1 = Ifd {
        ents: vec![
            (0x0201, V::Blob(thumb.clone())),
            (0x0202, V::Long(vec![thumb.len() as u32])),
        ],
        next: None,
    };
    let ifd0 = Ifd {
        ents: vec![(0x0112, V::Short(vec![6]))],
        next: Some(Box::new(ifd1)),
    };
    let jpeg = inject_exif(&make_jpeg(800, 600, gradient), &build_tiff(true, &ifd0));
    let p = write(dir.path(), "t.jpg", &jpeg);
    let e = generate_thumbnail(&p, ImageFormat::Jpeg, 6, 128, 80).unwrap();
    assert_eq!(e.source, ThumbSource::Embedded);
    assert_eq!((e.width, e.height), (96, 128));
    let e = generate_thumbnail(&p, ImageFormat::Jpeg, 6, 160, 80).unwrap();
    assert_eq!(e.source, ThumbSource::Embedded);
    let e = generate_thumbnail(&p, ImageFormat::Jpeg, 6, 256, 80).unwrap();
    assert_ne!(e.source, ThumbSource::Embedded);
    assert_eq!((e.width, e.height), (192, 256));
    // decode_rgb8 never uses the tiny embedded thumbnail
    let (w, h, _) = decode_rgb8(&p, ImageFormat::Jpeg, 1, 128).unwrap();
    assert_eq!((w, h), (128, 96));
}

fn nef_like(le: bool) -> Vec<u8> {
    let small = make_jpeg(160, 120, gradient);
    let big = make_jpeg(1200, 800, big_marker);
    let big_ifd = Ifd {
        ents: vec![
            (0x0100, V::Long(vec![1200])),
            (0x0101, V::Long(vec![800])),
            (0x0103, V::Short(vec![6])),
            (0x0201, V::Blob(big.clone())),
            (0x0202, V::Long(vec![big.len() as u32])),
        ],
        next: None,
    };
    let raw_ifd = Ifd {
        ents: vec![
            (0x0100, V::Long(vec![6048])),
            (0x0101, V::Long(vec![4032])),
            (0x0103, V::Short(vec![34713])),
        ],
        next: None,
    };
    let exif = Ifd {
        ents: vec![(0x9003, V::Ascii("2023:01:02 03:04:05".into()))],
        next: None,
    };
    let ifd1 = Ifd {
        ents: vec![
            (0x0201, V::Blob(small.clone())),
            (0x0202, V::Long(vec![small.len() as u32])),
        ],
        next: None,
    };
    let ifd0 = Ifd {
        ents: vec![
            (0x010F, V::Ascii("NIKON".into())),
            (0x0110, V::Ascii("Z 6".into())),
            (0x0112, V::Short(vec![6])),
            (0x0100, V::Long(vec![160])),
            (0x0101, V::Long(vec![120])),
            (0x014A, V::Sub(vec![big_ifd, raw_ifd])),
            (0x8769, V::Sub(vec![exif])),
        ],
        next: Some(Box::new(ifd1)),
    };
    build_tiff(le, &ifd0)
}

#[test]
fn tiff_raw_largest_preview() {
    let dir = tempfile::tempdir().unwrap();
    for le in [true, false] {
        let p = write(dir.path(), "x.nef", &nef_like(le));
        let md = read_metadata(&p, ImageFormat::Raw).unwrap();
        assert_eq!(md.camera_make.as_deref(), Some("NIKON"));
        assert_eq!(md.orientation, 6);
        assert_eq!((md.width, md.height), (Some(6048), Some(4032)));
        assert_eq!(md.taken_at_ms, Some(1_672_628_645_000));
        let e = generate_thumbnail(&p, ImageFormat::Raw, md.orientation, 256, 85).unwrap();
        assert_eq!(e.source, ThumbSource::Embedded);
        assert_eq!((e.width, e.height), (171, 256));
        let rgb = decode_out(&e);
        assert!(
            is_red(px(&rgb, e.width, e.width - 4, 3)),
            "red marker should be top-right after rot90"
        );
        let (w, h, _) = decode_rgb8(&p, ImageFormat::Raw, 6, 1000).unwrap();
        assert_eq!((w, h), (667, 1000));
    }
}

#[test]
fn raf_like() {
    let dir = tempfile::tempdir().unwrap();
    let jpeg = inject_exif(&make_jpeg(600, 400, marker), &full_exif_tiff(true));
    let mut raf = b"FUJIFILMCCD-RAW 0201FF39".to_vec();
    raf.resize(84, 0);
    raf.extend_from_slice(&200u32.to_be_bytes());
    raf.extend_from_slice(&(jpeg.len() as u32).to_be_bytes());
    raf.resize(200, 0);
    raf.extend_from_slice(&jpeg);
    raf.extend_from_slice(&[0u8; 100]);
    let p = write(dir.path(), "x.raf", &raf);
    let md = read_metadata(&p, ImageFormat::Raw).unwrap();
    assert_eq!(md.camera_make.as_deref(), Some("Acme"));
    assert_eq!((md.width, md.height), (Some(600), Some(400)));
    let e = generate_thumbnail(&p, ImageFormat::Raw, 1, 300, 80).unwrap();
    assert_eq!(
        (e.width, e.height, e.source),
        (300, 200, ThumbSource::Embedded)
    );
}

#[test]
fn cr3_like() {
    let dir = tempfile::tempdir().unwrap();
    let thmb_jpeg = make_jpeg(160, 120, gradient);
    let prvw_jpeg = make_jpeg(1024, 683, big_marker);
    let cmt1 = build_tiff(
        true,
        &Ifd {
            ents: vec![
                (0x010F, V::Ascii("Canon".into())),
                (0x0110, V::Ascii("EOS R6".into())),
                (0x0112, V::Short(vec![3])),
            ],
            next: None,
        },
    );
    let cmt2 = build_tiff(
        true,
        &Ifd {
            ents: vec![
                (0x9003, V::Ascii("2022:06:07 08:09:10".into())),
                (0x8827, V::Short(vec![800])),
                (0xA002, V::Long(vec![6000])),
                (0xA003, V::Long(vec![4000])),
            ],
            next: None,
        },
    );
    let mut thmb = vec![0u8; 16];
    thmb.extend_from_slice(&thmb_jpeg);
    let mut prvw = vec![0u8; 14];
    prvw.extend_from_slice(&prvw_jpeg);
    let mut canon = vec![
        0x85, 0xc0, 0xb6, 0x87, 0x82, 0x0f, 0x11, 0xe0, 0x81, 0x11, 0xf4, 0xce, 0x46, 0x2b, 0x6a,
        0x48,
    ];
    canon.extend(bx(b"CMT1", &cmt1));
    canon.extend(bx(b"CMT2", &cmt2));
    canon.extend(bx(b"THMB", &thmb));
    let mut prev_uuid = vec![
        0xea, 0xf4, 0x2b, 0x5e, 0x1c, 0x98, 0x4b, 0x88, 0xb9, 0xfb, 0xb7, 0xdc, 0x40, 0x6e, 0x4d,
        0x16,
    ];
    prev_uuid.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
    prev_uuid.extend(bx(b"PRVW", &prvw));
    let mut file = bx(b"ftyp", b"crx \0\0\0\x01crx isom");
    file.extend(bx(b"moov", &bx(b"uuid", &canon)));
    file.extend(bx(b"uuid", &prev_uuid));
    file.extend(bx(b"mdat", &[0u8; 64]));
    let p = write(dir.path(), "x.cr3", &file);
    let md = read_metadata(&p, ImageFormat::Raw).unwrap();
    assert_eq!(md.camera_model.as_deref(), Some("EOS R6"));
    assert_eq!(md.orientation, 3);
    assert_eq!(md.iso, Some(800));
    assert_eq!((md.width, md.height), (Some(6000), Some(4000)));
    assert!(md.taken_at_ms.is_some());
    let e = generate_thumbnail(&p, ImageFormat::Raw, 3, 512, 80).unwrap();
    assert_eq!(
        (e.width, e.height, e.source),
        (512, 342, ThumbSource::Embedded)
    );
    let rgb = decode_out(&e);
    assert!(
        is_red(px(&rgb, e.width, e.width - 4, e.height - 4)),
        "rot180 puts marker bottom-right"
    );
}

#[test]
fn raw_without_preview_and_heif_errors() {
    let dir = tempfile::tempdir().unwrap();
    let tiff = build_tiff(
        true,
        &Ifd {
            ents: vec![(0x0112, V::Short(vec![1]))],
            next: None,
        },
    );
    let p = write(dir.path(), "np.dng", &tiff);
    let err = generate_thumbnail(&p, ImageFormat::Raw, 1, 256, 80)
        .unwrap_err()
        .to_string();
    assert!(err.contains("no embedded JPEG preview"), "{err}");
    let h = write(dir.path(), "a.heic", b"\0\0\0\x18ftypheic\0\0\0\0mif1heic");
    let err = generate_thumbnail(&h, ImageFormat::Heif, 1, 256, 80)
        .unwrap_err()
        .to_string();
    assert!(err.contains("not supported yet"), "{err}");
    assert!(read_metadata(&h, ImageFormat::Heif).is_ok());
}
