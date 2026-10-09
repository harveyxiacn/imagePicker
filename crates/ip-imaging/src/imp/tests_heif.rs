//! HEIF/AVIF: container parsing, orientation, the decoder cascade. Synthetic files carry no
//! decodable HEVC, so every OS/libheif decoder fails on them and the pure-Rust layers are
//! tested deterministically; `real_heic_files_match_libheif_display` covers real decoders.
use std::path::{Path, PathBuf};

use super::container::{self, heif_orientation};
use super::orient;
use super::tests::{build_tiff, bx, decode_out, make_jpeg, write, Ifd, V};
use crate::*;

/// Red top-left and green top-right blocks (1/8 of each side) on blue.
fn markers(w: u32, h: u32) -> impl Fn(u32, u32) -> [u8; 3] {
    move |x, y| {
        if y < h / 8 && x < w / 8 {
            [255, 0, 0]
        } else if y < h / 8 && x >= w - w / 8 {
            [0, 255, 0]
        } else {
            [0, 0, 255]
        }
    }
}

/// Colour class of the four corners (TL, TR, BL, BR): `R`, `G` or `.`.
fn corners(rgb: &[u8], w: u32, h: u32) -> String {
    let pts = [
        (w / 16, h / 16),
        (w - 1 - w / 16, h / 16),
        (w / 16, h - 1 - h / 16),
        (w - 1 - w / 16, h - 1 - h / 16),
    ];
    pts.iter()
        .map(|&(x, y)| {
            let i = ((y * w + x) * 3) as usize;
            match (rgb[i], rgb[i + 1], rgb[i + 2]) {
                (r, g, b) if r > 180 && g < 90 && b < 90 => 'R',
                (r, g, b) if g > 180 && r < 90 && b < 90 => 'G',
                _ => '.',
            }
        })
        .collect()
}

fn full_box(typ: &[u8; 4], version: u8, payload: &[u8]) -> Vec<u8> {
    let mut p = vec![version, 0, 0, 0];
    p.extend_from_slice(payload);
    bx(typ, &p)
}

struct Item {
    id: u16,
    typ: &'static [u8; 4],
    data: Vec<u8>,
    /// 1-based indices into the `ipco` list.
    props: Vec<u8>,
}

/// `ftyp` + `meta` (pitm, iinf, iloc v1 with absolute offsets, iprp) + `mdat`.
fn build_heif(primary: u16, items: &[Item], ipco: &[Vec<u8>]) -> Vec<u8> {
    let meta = |mdat_start: u32| {
        let mut iinf = (items.len() as u16).to_be_bytes().to_vec();
        for it in items {
            let mut e = it.id.to_be_bytes().to_vec();
            e.extend_from_slice(&[0, 0]);
            e.extend_from_slice(it.typ);
            e.push(0);
            iinf.extend(full_box(b"infe", 2, &e));
        }
        let mut iloc = vec![0x44, 0x00];
        iloc.extend_from_slice(&(items.len() as u16).to_be_bytes());
        let mut off = mdat_start + 8;
        for it in items {
            iloc.extend_from_slice(&it.id.to_be_bytes());
            iloc.extend_from_slice(&[0, 0, 0, 0, 0, 1]);
            iloc.extend_from_slice(&off.to_be_bytes());
            iloc.extend_from_slice(&(it.data.len() as u32).to_be_bytes());
            off += it.data.len() as u32;
        }
        let mut ipma = (items.len() as u32).to_be_bytes().to_vec();
        for it in items {
            ipma.extend_from_slice(&it.id.to_be_bytes());
            ipma.push(it.props.len() as u8);
            ipma.extend_from_slice(&it.props);
        }
        let mut iprp = bx(b"ipco", &ipco.concat());
        iprp.extend(full_box(b"ipma", 0, &ipma));
        let mut m = full_box(b"hdlr", 0, b"\0\0\0\0pict\0\0\0\0\0\0\0\0\0\0\0\0\0");
        m.extend(full_box(b"pitm", 0, &primary.to_be_bytes()));
        m.extend(full_box(b"iinf", 0, &iinf));
        m.extend(full_box(b"iloc", 1, &iloc));
        m.extend(bx(b"iprp", &iprp));
        full_box(b"meta", 0, &m)
    };
    let ftyp = bx(b"ftyp", b"heic\0\0\0\0mif1heic");
    let start = (ftyp.len() + meta(0).len()) as u32;
    let mut file = ftyp;
    file.extend(meta(start));
    file.extend(bx(
        b"mdat",
        &items
            .iter()
            .flat_map(|i| i.data.clone())
            .collect::<Vec<u8>>(),
    ));
    file
}

fn ispe(w: u32, h: u32) -> Vec<u8> {
    let mut p = w.to_be_bytes().to_vec();
    p.extend_from_slice(&h.to_be_bytes());
    full_box(b"ispe", 0, &p)
}

/// EXIF item payload: offset 6 to the TIFF header past an `Exif\0\0` prefix.
fn exif_item(tiff: &[u8]) -> Vec<u8> {
    let mut v = 6u32.to_be_bytes().to_vec();
    v.extend_from_slice(b"Exif\0\0");
    v.extend_from_slice(tiff);
    v
}

/// A phone-like HEIC: 1200x800 HEVC primary (garbage payload) with `irot` = 3 (EXIF 6), a
/// stale EXIF orientation 1 with a 120x80 IFD1 thumbnail, and a 240x160 JPEG thumbnail item.
fn phone_like() -> Vec<u8> {
    let thumb = make_jpeg(120, 80, markers(120, 80));
    let ifd1 = Ifd {
        ents: vec![
            (0x0201, V::Blob(thumb.clone())),
            (0x0202, V::Long(vec![thumb.len() as u32])),
        ],
        next: None,
    };
    let tiff = build_tiff(
        false,
        &Ifd {
            ents: vec![
                (0x010F, V::Ascii("Apple".into())),
                (0x0110, V::Ascii("iPhone 15".into())),
                (0x0112, V::Short(vec![1])),
            ],
            next: Some(Box::new(ifd1)),
        },
    );
    build_heif(
        1,
        &[
            Item {
                id: 1,
                typ: b"hvc1",
                data: vec![0x5A; 256],
                props: vec![0x81, 2],
            },
            Item {
                id: 2,
                typ: b"Exif",
                data: exif_item(&tiff),
                props: vec![],
            },
            Item {
                id: 3,
                typ: b"jpeg",
                data: make_jpeg(240, 160, markers(240, 160)),
                props: vec![3, 2],
            },
        ],
        &[ispe(1200, 800), bx(b"irot", &[3]), ispe(240, 160)],
    )
}

#[test]
fn heif_container_items_and_orientation() {
    let file = phone_like();
    let info = container::parse_heif(&file);
    assert_eq!((info.width, info.height), (1200, 800));
    assert_eq!(info.orientation, Some(6));
    assert!(info.primary_jpeg.is_none());
    assert!(info.exif_tiff.is_some());
    let mut dims: Vec<_> = info
        .jpeg_previews
        .iter()
        .map(|j| container::usable_jpeg_dims(j).unwrap())
        .collect();
    dims.sort();
    assert_eq!(dims, vec![(120, 80), (240, 160)]);

    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "IMG_0001.HEIC", &file);
    let md = read_metadata(&p, ImageFormat::Heif).unwrap();
    assert_eq!((md.width, md.height), (Some(1200), Some(800)));
    assert_eq!(md.camera_model.as_deref(), Some("iPhone 15"));
    // `irot` wins over the stale EXIF tag
    assert_eq!(md.orientation, 6);
}

#[test]
fn heif_without_associations_keeps_exif_orientation() {
    // no ipma entry for the primary item: largest ispe, orientation from EXIF
    let tiff = build_tiff(
        true,
        &Ifd {
            ents: vec![(0x0112, V::Short(vec![8]))],
            next: None,
        },
    );
    let file = build_heif(
        7,
        &[Item {
            id: 2,
            typ: b"Exif",
            data: exif_item(&tiff),
            props: vec![],
        }],
        &[ispe(64, 48), ispe(640, 480)],
    );
    let info = container::parse_heif(&file);
    assert_eq!(
        (info.width, info.height, info.orientation),
        (640, 480, None)
    );
    let dir = tempfile::tempdir().unwrap();
    let md = read_metadata(&write(dir.path(), "a.heif", &file), ImageFormat::Heif).unwrap();
    assert_eq!(md.orientation, 8);
}

/// Applies HEIF transforms literally: `irot` turns anticlockwise, `imir` axis 1 mirrors
/// left-right and axis 0 top-bottom (libheif's reading, confirmed against pillow-heif).
fn apply_heif_ops(mut w: usize, mut h: usize, mut px: Vec<u8>, ops: &[(&[u8; 4], u8)]) -> Vec<u8> {
    for &(t, v) in ops {
        let old = std::mem::take(&mut px);
        if t == b"irot" {
            px = old;
            for _ in 0..v {
                // a quarter turn anticlockwise: new (x, y) shows old (w - 1 - y, x)
                let old = std::mem::take(&mut px);
                px = (0..w)
                    .flat_map(|y| (0..h).map(move |x| (x, y)))
                    .map(|(x, y)| old[x * w + (w - 1 - y)])
                    .collect();
                (w, h) = (h, w);
            }
        } else {
            px = (0..h)
                .flat_map(|y| (0..w).map(move |x| (x, y)))
                .map(|(x, y)| {
                    if v == 1 {
                        old[y * w + (w - 1 - x)]
                    } else {
                        old[(h - 1 - y) * w + x]
                    }
                })
                .collect();
        }
    }
    px
}

#[test]
fn heif_transforms_map_to_exif_orientation() {
    let ops: Vec<(&[u8; 4], u8)> = vec![
        (b"irot", 0),
        (b"irot", 1),
        (b"irot", 2),
        (b"irot", 3),
        (b"imir", 0),
        (b"imir", 1),
    ];
    let (w, h) = (3usize, 2usize);
    let cells: Vec<u8> = (0..(w * h) as u8).collect();
    let mut seqs: Vec<Vec<(&[u8; 4], u8)>> = vec![vec![]];
    seqs.extend(ops.iter().map(|o| vec![*o]));
    for a in &ops {
        for b in &ops {
            seqs.push(vec![*a, *b]);
        }
    }
    for seq in seqs {
        let props: Vec<([u8; 4], &[u8])> = seq
            .iter()
            .map(|(t, v)| (**t, std::slice::from_ref(v)))
            .collect();
        let o = heif_orientation(&props);
        let want = apply_heif_ops(w, h, cells.clone(), &seq);
        let rgb: Vec<u8> = cells.iter().flat_map(|&c| [c, c, c]).collect();
        let (_, _, got) = orient::apply(w as u32, h as u32, rgb, o);
        let got: Vec<u8> = got.chunks(3).map(|p| p[0]).collect();
        assert_eq!(got, want, "ops {seq:?} -> orientation {o}");
    }
}

#[test]
fn heif_thumbnail_from_embedded_jpeg_item() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "IMG_0001.HEIC", &phone_like());
    let e = generate_thumbnail(&p, ImageFormat::Heif, 6, 128, 80).unwrap();
    assert_eq!(e.source, ThumbSource::Embedded);
    assert_eq!((e.width, e.height), (85, 128));
    assert_eq!(corners(&decode_out(&e), e.width, e.height), ".R.G");
    // 120 px is enough for 100: the smallest sufficient preview is used
    let e = generate_thumbnail(&p, ImageFormat::Heif, 1, 100, 80).unwrap();
    assert_eq!(
        (e.width, e.height, e.source),
        (100, 67, ThumbSource::Embedded)
    );
}

#[test]
fn heif_without_decoder_degrades_to_small_preview_or_errors() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "IMG_0001.HEIC", &phone_like());
    // 512 px asked, no decoder for the HEVC primary: the largest preview, never upscaled
    let e = generate_thumbnail(&p, ImageFormat::Heif, 6, 512, 80).unwrap();
    assert_eq!(
        (e.width, e.height, e.source),
        (160, 240, ThumbSource::Embedded)
    );
    // analysis/render proxies never fall back to a preview
    let err = decode_rgb8(&p, ImageFormat::Heif, 6, 1024)
        .unwrap_err()
        .to_string();
    assert!(err.contains("cannot decode HEIF/AVIF"), "{err}");
    // no preview at all: the error says why
    let bare = build_heif(
        1,
        &[Item {
            id: 1,
            typ: b"hvc1",
            data: vec![0x5A; 64],
            props: vec![1],
        }],
        &[ispe(64, 48)],
    );
    let p = write(dir.path(), "bare.heic", &bare);
    let err = generate_thumbnail(&p, ImageFormat::Heif, 1, 256, 80)
        .unwrap_err()
        .to_string();
    assert!(err.contains("cannot decode HEIF/AVIF"), "{err}");
    #[cfg(not(target_os = "android"))]
    assert!(err.contains("libheif"), "{err}");
    #[cfg(windows)]
    assert!(err.contains("Windows"), "{err}");
}

#[test]
fn heif_with_jpeg_primary_decodes_without_codecs() {
    let file = build_heif(
        5,
        &[Item {
            id: 5,
            typ: b"jpeg",
            data: make_jpeg(400, 300, markers(400, 300)),
            props: vec![1, 2],
        }],
        &[ispe(400, 300), bx(b"irot", &[2])],
    );
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "j.heif", &file);
    let md = read_metadata(&p, ImageFormat::Heif).unwrap();
    assert_eq!(md.orientation, 3);
    let (w, h, rgb) = decode_rgb8(&p, ImageFormat::Heif, md.orientation, 200).unwrap();
    assert_eq!((w, h), (200, 150));
    assert_eq!(corners(&rgb, w, h), "..GR");
}

#[cfg(not(target_os = "android"))]
#[test]
fn libheif_private_copy_and_names() {
    use super::libheif::{is_libheif, private_copy};
    assert!(is_libheif(Path::new("libheif-30fc1b3c.dll")));
    assert!(is_libheif(Path::new("/x/libheif.so.1.17.6")));
    assert!(is_libheif(Path::new("libheif.1.dylib")));
    assert!(!is_libheif(Path::new("libde265-0.dll")));
    assert!(!is_libheif(Path::new("heif.txt")));
    let src = tempfile::tempdir().unwrap();
    let private = tempfile::tempdir().unwrap();
    let lib = write(src.path(), "libheif-1.dll", b"MZ fake");
    write(src.path(), "libde265-0.dll", b"MZ dep");
    write(src.path(), "notes.txt", b"x");
    write(private.path(), "0123456789abcdef/libheif-0.dll", b"stale");
    let copy = private_copy(&lib, private.path()).unwrap();
    assert_eq!(copy.file_name().unwrap(), "libheif-1.dll");
    let dir = copy.parent().unwrap();
    assert!(dir.join("libde265-0.dll").is_file());
    assert!(!dir.join("notes.txt").exists());
    assert!(!private.path().join("0123456789abcdef").exists());
    assert_eq!(private_copy(&lib, private.path()).unwrap(), copy);
}

#[cfg(windows)]
#[test]
fn wic_decodes_through_com_and_names_the_missing_codec() {
    let dir = tempfile::tempdir().unwrap();
    let img = image::RgbImage::from_fn(64, 48, |x, y| image::Rgb(markers(64, 48)(x, y)));
    let png = dir.path().join("m.png");
    img.save(&png).unwrap();
    let rgb = super::wic::decode(&png).unwrap();
    assert_eq!((rgb.w, rgb.h), (64, 48));
    assert_eq!(corners(&rgb.data, rgb.w, rgb.h), "RG..");
    let p = write(dir.path(), "IMG_0001.HEIC", &phone_like());
    let Err(err) = super::wic::decode(&p) else {
        panic!("WIC decoded a garbage HEVC payload");
    };
    assert!(err.contains("Windows"), "{err}");
}

/// pillow-heif writes the EXIF orientation as `irot`/`imir` over unchanged pixels; libheif's
/// own (transformed) decode is the reference for the upright result.
const GEN_HEIC: &str = r#"
import os, sys, sysconfig
import pillow_heif
from PIL import Image
d = sys.argv[1]
print('SITE|' + sysconfig.get_paths()['platlib'])
def img(w, h):
    im = Image.new('RGB', (w, h), (0, 0, 255))
    im.paste((255, 0, 0), (0, 0, w // 8, h // 8))
    im.paste((0, 255, 0), (w - w // 8, 0, w, h // 8))
    return im
def corners(im):
    w, h = im.size
    s = ''
    for x, y in [(w // 16, h // 16), (w - 1 - w // 16, h // 16), (w // 16, h - 1 - h // 16), (w - 1 - w // 16, h - 1 - h // 16)]:
        r, g, b = im.convert('RGB').getpixel((x, y))
        s += 'R' if r > 180 and g < 90 and b < 90 else 'G' if g > 180 and r < 90 and b < 90 else '.'
    return s
def save(name, w, h, orientation=1, thumbnails=(), mode='RGB'):
    data = img(w, h)
    if mode == 'RGB;16':
        raw = b''.join(int(c * 257).to_bytes(2, 'little') for px in data.getdata() for c in px)
    else:
        raw = data.tobytes()
    e = Image.Exif()
    e[0x0112] = orientation
    p = os.path.join(d, name)
    pillow_heif.from_bytes(mode=mode, size=(w, h), data=raw).save(p, quality=90, exif=e.tobytes(), thumbnails=list(thumbnails))
    im = pillow_heif.open_heif(p, convert_hdr_to_8bit=True).to_pillow()
    print('|'.join([name, str(im.size[0]), str(im.size[1]), corners(im)]))
for o in range(1, 9):
    save(f'o{o}.heic', 320, 240, o)
save('thumbs.heic', 1280, 960, 6, thumbnails=[320])
save('tenbit.heic', 160, 120, 1, mode='RGB;16')
"#;

/// Real decoders on files written by pillow-heif. Runs when `IMAGEPICKER_TEST_PYTHON` names a
/// Python with pillow-heif (e.g. the AI worker's venv); libheif comes from that environment
/// unless the OS codec decodes first.
#[test]
fn real_heic_files_match_libheif_display() {
    let Some(py) = std::env::var_os("IMAGEPICKER_TEST_PYTHON") else {
        eprintln!("skipped: set IMAGEPICKER_TEST_PYTHON to a Python with pillow-heif");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let private = tempfile::tempdir().unwrap();
    let out = std::process::Command::new(py)
        .arg("-c")
        .arg(GEN_HEIC)
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut cases = Vec::new();
    for line in stdout.lines() {
        let f: Vec<&str> = line.trim().split('|').collect();
        if f[0] == "SITE" {
            let sp = PathBuf::from(f[1]);
            set_heif_library_dirs(
                vec![sp.join("pillow_heif.libs"), sp.clone()],
                Some(private.path().to_path_buf()),
            );
        } else if f.len() == 4 {
            let (w, h): (u32, u32) = (f[1].parse().unwrap(), f[2].parse().unwrap());
            cases.push((dir.path().join(f[0]), (w, h), f[3].to_string()));
        }
    }
    assert_eq!(cases.len(), 10, "{stdout}");
    for (p, (dw, dh), want) in &cases {
        let md = read_metadata(p, ImageFormat::Heif).unwrap();
        let long = 256;
        let e = generate_thumbnail(p, ImageFormat::Heif, md.orientation, long, 90).unwrap();
        let name = p.file_name().unwrap().to_string_lossy();
        assert_eq!(
            e.width.max(e.height),
            long.min((*dw).max(*dh)),
            "{name}: {e:?}"
        );
        assert_eq!(e.width > e.height, dw > dh, "{name}");
        assert_eq!(&corners(&decode_out(&e), e.width, e.height), want, "{name}");
        let (w, h, rgb) = decode_rgb8(p, ImageFormat::Heif, md.orientation, 4096).unwrap();
        assert_eq!((w, h), (*dw, *dh), "{name}");
        assert_eq!(&corners(&rgb, w, h), want, "{name}");
    }
    // without an OS codec libheif decodes: its 320 px HEVC thumbnail item serves 256 px
    let thumbs = dir.path().join("thumbs.heic");
    if super::heif::os_decode(&thumbs, 256).is_some_and(|r| r.is_ok()) {
        eprintln!("the OS codec decodes HEIC here; libheif checks skipped");
        return;
    }
    let e = generate_thumbnail(&thumbs, ImageFormat::Heif, 6, 256, 90).unwrap();
    assert_eq!(e.source, ThumbSource::Embedded);
    let e = generate_thumbnail(&thumbs, ImageFormat::Heif, 6, 512, 90).unwrap();
    assert_eq!(e.source, ThumbSource::FullDecode);
    if cfg!(windows) {
        // loaded from the private copy, not from the venv
        assert!(std::fs::read_dir(private.path()).unwrap().next().is_some());
    }
}
