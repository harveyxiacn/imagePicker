//! P0-8 export tests (real imaging layer): every output format decoded and checked for size,
//! colour and metadata, the strip options on the copy and the re-encode path, catalog name
//! tokens.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use image::ImageDecoder;
use ip_imaging::{ImageFormat, Metadata};
use ip_render::Backend;
use serde_json::json;

use super::formats;
use super::{ExportFormat, ExportOptions};
use crate::imaging::Imaging;
use crate::jpegmeta::{self, PreparedMeta};
use crate::testutil::{write_jpeg, FakeRenderer, FakeWorker};
use crate::*;

const WAIT: Duration = Duration::from_secs(30);
const RGB: [u8; 3] = [200, 60, 40];

struct Env {
    core: Arc<Core>,
    _data: tempfile::TempDir,
    src: tempfile::TempDir,
}

fn env() -> Env {
    let data = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: Arc::new(RealImaging),
        thumb_workers: Some(2),
        worker: Some(Arc::new(FakeWorker::new())),
        renderer: Some(Arc::new(FakeRenderer::new(Backend::Cpu))),
        force_cpu: true,
    })
    .unwrap();
    Env {
        core,
        _data: data,
        src,
    }
}

async fn import(e: &Env, title: &str) -> HashMap<String, Photo> {
    let s = e
        .core
        .import(ImportRequest {
            path: e.src.path().to_string_lossy().into_owned(),
            recursive: true,
            title: Some(title.into()),
        })
        .await
        .unwrap();
    e.core.wait_session_ready(s.id, WAIT).await.unwrap();
    e.core
        .photos(PhotoQuery {
            session_id: s.id,
            limit: Some(5000),
            ..Default::default()
        })
        .await
        .unwrap()
        .photos
        .into_iter()
        .map(|p| (p.file_name.clone(), p))
        .collect()
}

fn req(ids: Vec<i64>, dest: &Path) -> ExportRequest {
    ExportRequest {
        ids,
        dest: dest.to_string_lossy().into_owned(),
        ..Default::default()
    }
}

async fn run(e: &Env, req: ExportRequest) {
    let mut rx = e.core.events.subscribe();
    let id = e.core.export(req).await.unwrap();
    let (state, error) = tokio::time::timeout(WAIT, async {
        loop {
            if let Ok(Event::TaskProgress {
                task_id,
                state,
                error,
                ..
            }) = rx.recv().await
            {
                if task_id == id && state != "running" {
                    return (state, error);
                }
            }
        }
    })
    .await
    .unwrap();
    assert_eq!((state.as_str(), error), ("done", None));
}

/// A flat-colour 320x240 JPEG with EXIF (`orientation`, GPS, thumbnail), the test XMP (GPS
/// included), IPTC, an sRGB profile and a comment.
fn rich_jpeg(path: &Path, orientation: u16) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(320, 240, image::Rgb(RGB));
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 95)
        .encode_image(&img)
        .unwrap();
    let mut out = jpegmeta::insert_metadata(
        &bytes,
        Some(&jpegmeta::build_test_exif(orientation, false, true)),
        Some(jpegmeta::TEST_XMP),
        Some(jpegmeta::srgb_icc()),
        Some(&jpegmeta::build_test_irb()),
    );
    let at = crate::xmp::jpeg::segments(&out)
        .unwrap()
        .last()
        .unwrap()
        .end;
    out = [&out[..at], b"\xFF\xFE\x00\x0Acomment!", &out[at..]].concat();
    std::fs::write(path, &out).unwrap();
    out
}

/// EXIF-derived metadata of an export, whatever its container.
fn exif_md(path: &Path, f: ExportFormat) -> Option<Metadata> {
    let block = match f {
        ExportFormat::Tiff => {
            return Some(ip_imaging::read_metadata(path, ImageFormat::Tiff).unwrap())
        }
        ExportFormat::Png => ip_imaging::read_exif(path, ImageFormat::Png).unwrap()?,
        ExportFormat::Webp => ip_imaging::read_exif(path, ImageFormat::Webp).unwrap()?,
        _ => jpegmeta::extract_exif(&std::fs::read(path).unwrap())?,
    };
    Some(md_of_block(&block))
}

/// Reads an EXIF block by wrapping it in a JPEG.
fn md_of_block(block: &[u8]) -> Metadata {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("x.jpg");
    write_jpeg(&p, 8, 8, 1);
    let bytes = std::fs::read(&p).unwrap();
    std::fs::write(&p, jpegmeta::insert_segments(&bytes, Some(block), None)).unwrap();
    ip_imaging::read_metadata(&p, ImageFormat::Jpeg).unwrap()
}

fn icc_of(path: &Path) -> Option<Vec<u8>> {
    if path.extension().is_some_and(|e| e == "tif") {
        // `ImageReader::into_decoder` loses the profile of TIFFs (image 0.25.10); the TIFF
        // decoder itself reads it, as do Pillow and exiftool
        let f = std::io::BufReader::new(std::fs::File::open(path).unwrap());
        let mut d = image::codecs::tiff::TiffDecoder::new(f).unwrap();
        return d.icc_profile().unwrap();
    }
    let mut d = image::ImageReader::open(path)
        .unwrap()
        .with_guessed_format()
        .unwrap()
        .into_decoder()
        .unwrap();
    d.icc_profile().unwrap()
}

fn gradient(w: u32, h: u32) -> ip_render::RgbImage {
    let mut data = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            data.extend([(x * 4) as u8, (y * 5) as u8, 128]);
        }
    }
    ip_render::RgbImage {
        width: w,
        height: h,
        data,
    }
}

fn has(bytes: &[u8], needle: &[u8]) -> bool {
    bytes.windows(needle.len()).any(|w| w == needle)
}

fn center(img: &image::RgbImage) -> [u8; 3] {
    img.get_pixel(img.width() / 2, img.height() / 2).0
}

fn close(a: [u8; 3], b: [u8; 3], tol: u8) -> bool {
    a.iter().zip(&b).all(|(x, y)| x.abs_diff(*y) <= tol)
}

/// Everything after the header segments: the scans and the end marker.
fn image_data(jpeg: &[u8]) -> &[u8] {
    let end = crate::xmp::jpeg::segments(jpeg)
        .unwrap()
        .last()
        .unwrap()
        .end;
    &jpeg[end..]
}

/// One output format end to end: resized (with and without GPS) and edited (no metadata)
/// exports decoded and checked for size, colour, profile, EXIF, XMP and IPTC.
async fn check_format(format: ExportFormat) {
    let e = env();
    let src = e.src.path().join("m.jpg");
    rich_jpeg(&src, 6);
    let ph = import(&e, "Kyoto").await;
    let id = ph["m.jpg"].id;
    let stored = center(&image::open(&src).unwrap().to_rgb8());
    let iptc = jpegmeta::build_test_iptc();
    for strip in [false, true] {
        let out = e.src.path().join(format!("out-{format:?}-{strip}"));
        let mut r = req(vec![id], &out);
        r.long_edge = Some(160);
        r.format = format;
        r.strip_gps = strip;
        run(&e, r).await;
        let file = out.join(format!("m.{}", format.extension()));
        let bytes = std::fs::read(&file).unwrap();
        let what = format!("{format:?} strip={strip}");
        // pixels: orientation 6 baked in, the colour kept
        let img = image::load_from_memory(&bytes).unwrap().to_rgb8();
        assert_eq!(img.dimensions(), (120, 160), "{what}");
        let tol = if format == ExportFormat::Jpeg { 6 } else { 3 };
        assert!(
            close(center(&img), stored, tol),
            "{what}: {:?} vs {stored:?}",
            center(&img)
        );
        // sRGB profile
        assert_eq!(
            icc_of(&file).as_deref(),
            Some(jpegmeta::srgb_icc()),
            "{what}"
        );
        // EXIF: the camera's, upright, GPS on request
        let md = exif_md(&file, format).expect(&what);
        assert_eq!(md.camera_make.as_deref(), Some("ACME Corp"), "{what}");
        assert_eq!(md.camera_model.as_deref(), Some("Model One"), "{what}");
        assert_eq!(md.iso, Some(200), "{what}");
        assert!(md.taken_at_ms.is_some(), "{what}");
        assert_eq!(md.orientation, 1, "{what}");
        assert_eq!(md.gps_lat.is_some(), !strip, "{what}");
        // XMP: description kept, orientation dropped, GPS on request
        assert!(has(&bytes, b"Temple at dawn"), "{what}");
        assert!(!has(&bytes, b"tiff:Orientation"), "{what}");
        assert_eq!(has(&bytes, b"exif:GPSLatitude"), !strip, "{what}");
        assert_eq!(has(&bytes, b"drone-dji:GpsLatitude"), !strip, "{what}");
        // IPTC where the container has a place for it
        let with_iptc = matches!(format, ExportFormat::Jpeg | ExportFormat::Tiff);
        assert_eq!(has(&bytes, &iptc), with_iptc, "{what}");
        // the container's own structure
        match format {
            ExportFormat::Png => {
                for c in [b"iCCP", b"eXIf", b"iTXt"] {
                    assert!(has(&bytes, c), "{what}");
                }
            }
            ExportFormat::Webp => {
                for c in [b"VP8X", b"ICCP", b"VP8L", b"EXIF", b"XMP "] {
                    assert!(has(&bytes, c), "{what}");
                }
            }
            ExportFormat::Tiff => {
                let md = ip_imaging::read_metadata(&file, ImageFormat::Tiff).unwrap();
                assert_eq!((md.width, md.height), (Some(120), Some(160)));
            }
            _ => {}
        }
    }

    // the edit render path, everything but the profile stripped
    e.core
        .put_edit(
            id,
            json!({"version":1,"ops":[{"type":"global","exposure":0.3}]}),
        )
        .await
        .unwrap();
    let out = e.src.path().join(format!("edited-{format:?}"));
    let mut r = req(vec![id], &out);
    r.format = format;
    r.strip_metadata = true;
    run(&e, r).await;
    let file = out.join(format!("m.{}", format.extension()));
    let img = image::open(&file).unwrap();
    assert_eq!((img.width(), img.height()), (240, 320), "{format:?}");
    assert!(exif_md(&file, format).is_none_or(|m| m.camera_make.is_none()));
    assert!(!has(&std::fs::read(&file).unwrap(), b"Temple at dawn"));
    assert!(icc_of(&file).is_some(), "{format:?}: the profile stays");
}

#[tokio::test]
async fn jpeg_export_decodes_with_size_colour_and_metadata() {
    check_format(ExportFormat::Jpeg).await;
}

#[tokio::test]
async fn png_export_decodes_with_size_colour_and_metadata() {
    check_format(ExportFormat::Png).await;
}

#[tokio::test]
async fn tiff_export_decodes_with_size_colour_and_metadata() {
    check_format(ExportFormat::Tiff).await;
}

#[tokio::test]
async fn webp_export_decodes_with_size_colour_and_metadata() {
    check_format(ExportFormat::Webp).await;
}

#[tokio::test]
async fn strip_options_clean_copies_without_touching_the_image_data() {
    let e = env();
    let src_path = e.src.path().join("m.jpg");
    let src = rich_jpeg(&src_path, 6);
    let ph = import(&e, "Kyoto").await;
    let id = ph["m.jpg"].id;
    let src_md = ip_imaging::read_metadata(&src_path, ImageFormat::Jpeg).unwrap();
    assert!(src_md.gps_lat.is_some() && src_md.orientation == 6);

    // nothing asked: still the original byte for byte
    let out = e.src.path().join("copy");
    run(&e, req(vec![id], &out)).await;
    assert_eq!(std::fs::read(out.join("m.jpg")).unwrap(), src);

    // GPS only: same size, same scans, everything but the position kept
    let out = e.src.path().join("nogps");
    let mut r = req(vec![id], &out);
    r.strip_gps = true;
    run(&e, r).await;
    let file = std::fs::read(out.join("m.jpg")).unwrap();
    assert_eq!(file.len(), src.len(), "cleaned in place");
    assert_eq!(image_data(&file), image_data(&src));
    let md = ip_imaging::read_metadata(&out.join("m.jpg"), ImageFormat::Jpeg).unwrap();
    assert!(md.gps_lat.is_none() && md.gps_lon.is_none());
    assert_eq!(md.orientation, 6);
    assert_eq!(md.camera_make.as_deref(), Some("ACME Corp"));
    assert_eq!(md.taken_at_ms, src_md.taken_at_ms);
    let xmp = crate::xmp::jpeg::extract(&file).unwrap();
    assert!(!xmp.contains("exif:GPSLatitude") && !xmp.contains("drone-dji:GpsLatitude"));
    assert!(xmp.contains("Temple at dawn") && xmp.contains("tiff:Orientation"));
    assert_eq!(jpegmeta::extract_irb(&file), jpegmeta::extract_irb(&src));
    assert!(has(&file, b"comment!") && has(&file, b"THUMBNAIL"));
    assert!(has(&file, b"ICC_PROFILE"));

    // all metadata: only the orientation the stored pixels need, and the profile
    let out = e.src.path().join("bare");
    let mut r = req(vec![id], &out);
    r.strip_metadata = true;
    run(&e, r).await;
    let file = std::fs::read(out.join("m.jpg")).unwrap();
    assert_eq!(image_data(&file), image_data(&src));
    assert_eq!(
        jpegmeta::extract_exif(&file),
        Some(formats::orientation_exif(6))
    );
    let md = ip_imaging::read_metadata(&out.join("m.jpg"), ImageFormat::Jpeg).unwrap();
    assert_eq!(md.orientation, 6);
    assert!(md.camera_make.is_none() && md.gps_lat.is_none() && md.taken_at_ms.is_none());
    assert!(crate::xmp::jpeg::extract(&file).is_none());
    assert!(jpegmeta::extract_irb(&file).is_none());
    assert!(!has(&file, b"comment!") && !has(&file, b"THUMBNAIL"));
    assert!(has(&file, b"ICC_PROFILE"));
    let img = image::load_from_memory(&file).unwrap();
    assert_eq!(
        (img.width(), img.height()),
        (320, 240),
        "stored, not rotated"
    );

    // re-encoded with all metadata stripped: the profile only
    let out = e.src.path().join("bare-resized");
    let mut r = req(vec![id], &out);
    r.strip_metadata = true;
    r.long_edge = Some(100);
    run(&e, r).await;
    let file = std::fs::read(out.join("m.jpg")).unwrap();
    assert!(jpegmeta::extract_exif(&file).is_none());
    assert!(crate::xmp::jpeg::extract(&file).is_none());
    assert!(jpegmeta::extract_irb(&file).is_none());
    assert!(has(&file, b"ICC_PROFILE"));
    let img = image::load_from_memory(&file).unwrap();
    assert_eq!((img.width(), img.height()), (75, 100), "rotated upright");
}

#[tokio::test]
async fn png_and_webp_originals_are_cleaned_chunk_by_chunk() {
    let e = env();
    let img = gradient(64, 48);
    let meta = PreparedMeta {
        exif: Some(jpegmeta::build_test_exif(1, false, true)),
        xmp: Some(jpegmeta::TEST_XMP.to_string()),
        irb: None,
    };
    for (name, format) in [("p.png", ExportFormat::Png), ("w.webp", ExportFormat::Webp)] {
        std::fs::write(
            e.src.path().join(name),
            formats::encode(&img, format, 90, &meta).unwrap(),
        )
        .unwrap();
    }
    let ph = import(&e, "Kyoto").await;
    for (name, format, image_chunk) in [
        ("p.png", ExportFormat::Png, &b"IDAT"[..]),
        ("w.webp", ExportFormat::Webp, &b"VP8L"[..]),
    ] {
        let src = std::fs::read(e.src.path().join(name)).unwrap();
        let src_md = exif_md(&e.src.path().join(name), format).unwrap();
        assert!(src_md.gps_lat.is_some(), "{name}");
        // the encoded image data, from its chunk header on
        let pixels = |b: &[u8]| {
            let at = b.windows(4).position(|w| w == image_chunk).unwrap();
            b[at..at + 64].to_vec()
        };
        for (strip_all, dir) in [(false, "nogps"), (true, "bare")] {
            let out = e.src.path().join(format!("{dir}-{name}"));
            let mut r = req(vec![ph[name].id], &out);
            r.format = format; // already the source's: cleaned, not re-encoded
            r.strip_gps = !strip_all;
            r.strip_metadata = strip_all;
            run(&e, r).await;
            let file = out.join(name);
            let bytes = std::fs::read(&file).unwrap();
            assert_eq!(pixels(&bytes), pixels(&src), "{name} {dir}");
            let a = image::load_from_memory(&bytes).unwrap().to_rgb8();
            let b = image::load_from_memory(&src).unwrap().to_rgb8();
            assert_eq!(a, b, "{name} {dir}: same pixels");
            assert_eq!(
                icc_of(&file),
                icc_of(&e.src.path().join(name)),
                "{name} {dir}"
            );
            if strip_all {
                assert!(exif_md(&file, format).is_none(), "{name}");
                assert!(!has(&bytes, b"Temple at dawn"), "{name}");
            } else {
                let md = exif_md(&file, format).unwrap();
                assert!(md.gps_lat.is_none(), "{name}");
                assert_eq!(md.camera_make.as_deref(), Some("ACME Corp"), "{name}");
                assert!(has(&bytes, b"Temple at dawn"), "{name}");
                assert!(!has(&bytes, b"exif:GPSLatitude"), "{name}");
            }
        }
    }
}

#[tokio::test]
async fn originals_that_cannot_be_cleaned_are_re_encoded_without_gps() {
    let e = env();
    std::fs::write(
        e.src.path().join("scan.tif"),
        jpegmeta::build_test_tiff(64, 48, [120, 130, 140]),
    )
    .unwrap();
    let ph = import(&e, "Scans").await;
    let id = ph["scan.tif"].id;
    // "original" keeps an unchanged TIFF as it is...
    let out = e.src.path().join("copy");
    run(&e, req(vec![id], &out)).await;
    assert!(out.join("scan.tif").exists());
    // ...but GPS cannot be cut out of it in place: JPEG (or the chosen format) instead
    for (format, file) in [
        (ExportFormat::Original, "scan.jpg"),
        (ExportFormat::Tiff, "scan.tif"),
    ] {
        let out = e.src.path().join(format!("nogps-{format:?}"));
        let mut r = req(vec![id], &out);
        r.format = format;
        r.strip_gps = true;
        run(&e, r).await;
        let fmt = if file.ends_with("tif") {
            ExportFormat::Tiff
        } else {
            ExportFormat::Jpeg
        };
        let md = exif_md(&out.join(file), fmt).unwrap();
        assert!(md.gps_lat.is_none(), "{file}");
        assert_eq!(md.camera_make.as_deref(), Some("ACME Corp"), "{file}");
        let img = image::open(out.join(file)).unwrap().to_rgb8();
        assert_eq!(img.dimensions(), (64, 48));
        assert!(close(center(&img), [120, 130, 140], 4), "{file}");
    }
}

#[test]
fn gps_goes_from_embedded_images_too_and_offsets_stay_valid() {
    // primary with EXIF GPS + an MPF index + a secondary JPEG (gain map style) with its own GPS
    let base = {
        let img = image::RgbImage::from_pixel(16, 16, image::Rgb([1, 2, 3]));
        let mut b = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut b)
            .encode_image(&img)
            .unwrap();
        b
    };
    let secondary =
        jpegmeta::insert_segments(&base, Some(&jpegmeta::build_test_exif(1, true, true)), None);
    let mut mpf = b"MPF\0II*\0\x08\0\0\0\x01\0\x02\xB0\x07\0\x20\0\0\0\x1A\0\0\0\0\0\0\0".to_vec();
    mpf.extend([0u8; 32]); // two entries, patched below
    let primary = jpegmeta::insert_metadata(
        &base,
        Some(&jpegmeta::build_test_exif(1, false, true)),
        Some(jpegmeta::TEST_XMP),
        None,
        None,
    );
    let at = crate::xmp::jpeg::segments(&primary)
        .unwrap()
        .last()
        .unwrap()
        .end;
    let mut seg = vec![0xFF, 0xE2];
    seg.extend(((mpf.len() + 2) as u16).to_be_bytes());
    seg.extend(&mpf);
    let primary = [&primary[..at], &seg[..], &primary[at..]].concat();
    let mut file = primary.clone();
    file.extend(&secondary);
    let cleaned = formats::clean_original(ImageFormat::Jpeg, &file, false, 1).unwrap();
    assert_eq!(cleaned.len(), file.len(), "nothing moved");
    let (p, s) = cleaned.split_at(primary.len());
    assert_eq!(image_data(s), image_data(&secondary));
    for part in [p, s] {
        let exif = jpegmeta::extract_exif(part).unwrap();
        let md = md_of_block(&exif);
        assert!(md.gps_lat.is_none());
        assert_eq!(md.camera_make.as_deref(), Some("ACME Corp"));
    }
    assert!(has(p, b"MPF\0"));
    // stripping everything drops the index and what it points to
    let bare = formats::clean_original(ImageFormat::Jpeg, &file, true, 1).unwrap();
    assert!(!has(&bare, b"MPF\0") && !has(&bare, b"ACME"));
    assert!(bare.len() < primary.len());
    image::load_from_memory(&bare).unwrap();
}

#[tokio::test]
async fn name_tokens_come_from_the_catalog() {
    let e = env();
    write_jpeg(&e.src.path().join("a.jpg"), 32, 24, 10);
    write_jpeg(&e.src.path().join("b.jpg"), 32, 24, 20);
    let ph = import(&e, "Kyoto: day 1").await;
    let (a, b) = (ph["a.jpg"].id, ph["b.jpg"].id);
    e.core
        .db
        .call(move |c| {
            c.execute(
                "UPDATE photo SET user_rating=4, scene_type='portrait', camera='ACME Model One',
                 taken_at=1709164800000 WHERE id=?1",
                [a],
            )?;
            c.execute(
                "UPDATE photo SET user_rating=NULL, scene_type=NULL, camera=NULL, taken_at=1709164800000 WHERE id=?1",
                [b],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let out = e.src.path().join("named");
    let tpl = "{date}_{event}_{scene}_{camera}_{rating}_{seq}";
    for _ in 0..2 {
        let mut r = req(vec![a, b], &out);
        r.name_template = tpl.into();
        run(&e, r).await;
    }
    let mut names: Vec<String> = std::fs::read_dir(&out)
        .unwrap()
        .map(|d| d.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "20240229_Kyoto_ day 1_noscene_nocamera_0_0002.jpg",
            "20240229_Kyoto_ day 1_noscene_nocamera_0_0002_1.jpg",
            "20240229_Kyoto_ day 1_portrait_ACME Model One_4_0001.jpg",
            "20240229_Kyoto_ day 1_portrait_ACME Model One_4_0001_1.jpg",
        ],
        "sanitised, and the second run never overwrites"
    );
}

#[tokio::test]
async fn event_token_is_the_events_name_or_else_the_import_title() {
    let e = env();
    write_jpeg(&e.src.path().join("a.jpg"), 32, 24, 10);
    write_jpeg(&e.src.path().join("b.jpg"), 32, 24, 20);
    let ph = import(&e, "Kyoto: day 1").await;
    let (a, b, sid) = (ph["a.jpg"].id, ph["b.jpg"].id, ph["a.jpg"].session_id);
    // `a` is in an event of 2024-02-29, 10:00-11:30 at UTC+9; `b` is in none
    const DAY: i64 = 1_709_164_800_000;
    let event = e
        .core
        .db
        .call(move |c| {
            c.execute(
                "INSERT INTO event(session_id, start_at, end_at, offset_min) VALUES(?1,?2,?3,540)",
                rusqlite::params![sid, DAY + 3_600_000, DAY + 9_000_000],
            )?;
            let ev = c.last_insert_rowid();
            c.execute(
                "INSERT INTO scene(session_id, start_at, end_at, event_id) VALUES(?1,?2,?3,?4)",
                rusqlite::params![sid, DAY + 3_600_000, DAY + 9_000_000, ev],
            )?;
            let sc = c.last_insert_rowid();
            c.execute(
                "INSERT INTO burst(session_id, scene_id, size, manual) VALUES(?1,?2,1,0)",
                rusqlite::params![sid, sc],
            )?;
            let bu = c.last_insert_rowid();
            c.execute(
                "UPDATE photo SET burst_id=?2, taken_at=?3 WHERE id=?1",
                rusqlite::params![a, bu, DAY + 3_600_000],
            )?;
            c.execute(
                "UPDATE photo SET taken_at=?2 WHERE id=?1",
                rusqlite::params![b, DAY],
            )?;
            Ok(ev)
        })
        .await
        .unwrap();
    let names = |dir: &Path| -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|d| d.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    };
    let out = e.src.path().join("generated");
    let mut r = req(vec![a, b], &out);
    r.name_template = "{event}_{seq}".into();
    run(&e, r).await;
    assert_eq!(
        names(&out),
        ["2024-02-29 1000-1130_0001.jpg", "Kyoto_ day 1_0002.jpg"],
        "the event name has no colon; photos outside any event keep the import title"
    );
    // a title the user gave the event wins
    e.core
        .db
        .call(move |c| {
            c.execute("UPDATE event SET title='Hanami' WHERE id=?1", [event])?;
            Ok(())
        })
        .await
        .unwrap();
    let out = e.src.path().join("titled");
    let mut r = req(vec![a], &out);
    r.name_template = "{event}_{seq}".into();
    run(&e, r).await;
    assert_eq!(names(&out), ["Hanami_0001.jpg"]);
}

/// Writes one export of every format and the cleaned originals into
/// `$IMAGEPICKER_EXPORT_SAMPLES`, for checking them with other tools (exiftool, Pillow):
/// `cargo test -p ip-core --lib write_export_samples -- --ignored`.
#[test]
#[ignore]
fn write_export_samples() {
    let Some(dir) = std::env::var_os("IMAGEPICKER_EXPORT_SAMPLES") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let src = rich_jpeg(&dir.join("source.jpg"), 6);
    let source = jpegmeta::SourceMeta {
        exif: jpegmeta::extract_exif(&src),
        xmp: crate::xmp::jpeg::extract(&src),
        irb: jpegmeta::extract_irb(&src),
        metadata: None,
    };
    let img = gradient(64, 48);
    for strip in [false, true] {
        let meta = jpegmeta::prepare_meta(&source, (64, 48), strip);
        for format in [
            ExportFormat::Jpeg,
            ExportFormat::Png,
            ExportFormat::Tiff,
            ExportFormat::Webp,
        ] {
            let kind = if strip { "nogps" } else { "full" };
            let name = format!("export-{kind}.{}", format.extension());
            std::fs::write(
                dir.join(name),
                formats::encode(&img, format, 90, &meta).unwrap(),
            )
            .unwrap();
        }
    }
    for (name, all) in [("clean-nogps.jpg", false), ("clean-bare.jpg", true)] {
        let out = formats::clean_original(ImageFormat::Jpeg, &src, all, 6).unwrap();
        std::fs::write(dir.join(name), out).unwrap();
    }
}

/// Puts a file at the planned output name while the export is producing that photo, as
/// another program saving into the same folder would.
struct Racing {
    target: std::path::PathBuf,
}

impl Imaging for Racing {
    fn scan_dir(
        &self,
        _root: &Path,
        _recursive: bool,
        _exclude: &[String],
    ) -> anyhow::Result<Vec<ip_imaging::ScannedFile>> {
        Ok(Vec::new())
    }
    fn read_metadata(&self, _path: &Path, _format: ImageFormat) -> anyhow::Result<Metadata> {
        Ok(Metadata::default())
    }
    fn read_exif(&self, _path: &Path, _format: ImageFormat) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn content_key(&self, _path: &Path) -> anyhow::Result<String> {
        Ok(String::new())
    }
    fn generate_thumbnail(
        &self,
        path: &Path,
        format: ImageFormat,
        orientation: u8,
        long_edge: u32,
        quality: u8,
    ) -> anyhow::Result<ip_imaging::EncodedImage> {
        std::fs::write(&self.target, b"someone else's file")?;
        RealImaging.generate_thumbnail(path, format, orientation, long_edge, quality)
    }
}

#[test]
fn a_file_that_appears_meanwhile_is_neither_overwritten_nor_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("a.jpg");
    write_jpeg(&src, 64, 48, 10);
    let dest = dir.path().join("out");
    std::fs::create_dir_all(&dest).unwrap();
    let r = crate::catalog::PhotoRef {
        id: 1,
        path: src.clone(),
        file_name: "a.jpg".into(),
        format: ImageFormat::Jpeg,
        orientation: 1,
        fast_key: "0".repeat(32),
        taken_at: None,
        mtime_ms: None,
        edit_hash: None,
        width: None,
        height: None,
    };
    let imaging = Racing {
        target: dest.join("a.jpg"),
    };
    let opts = ExportOptions {
        long_edge: Some(32),
        ..Default::default()
    };
    let rep = super::export_photos(&imaging, &[r], &dest, &opts, None, &|_, _| {});
    assert!(rep.written.is_empty());
    assert_eq!(rep.errors.len(), 1, "{:?}", rep.errors);
    assert_eq!(
        std::fs::read(dest.join("a.jpg")).unwrap(),
        b"someone else's file",
        "neither overwritten nor deleted"
    );
    // the writers themselves: an existing file is an error and stays as it was
    let taken = dest.join("taken.jpg");
    std::fs::write(&taken, b"mine").unwrap();
    let err = super::write_new(&taken, b"other").unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    let err = super::copy_new(&src, &taken).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read(&taken).unwrap(), b"mine");
}

#[tokio::test]
async fn exporting_into_the_source_folder_never_touches_the_originals() {
    let e = env();
    let src_path = e.src.path().join("m.jpg");
    let src = rich_jpeg(&src_path, 6);
    let ph = import(&e, "Kyoto").await;
    let id = ph["m.jpg"].id;
    for (strip_gps, strip_metadata, format) in [
        (false, false, ExportFormat::Original),
        (true, false, ExportFormat::Original),
        (false, true, ExportFormat::Original),
        (false, false, ExportFormat::Png),
    ] {
        let mut r = req(vec![id], e.src.path());
        r.strip_gps = strip_gps;
        r.strip_metadata = strip_metadata;
        r.format = format;
        run(&e, r).await;
        assert_eq!(std::fs::read(&src_path).unwrap(), src, "original unchanged");
    }
    let mut names: Vec<String> = std::fs::read_dir(e.src.path())
        .unwrap()
        .map(|d| d.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("m"))
        .collect();
    names.sort();
    assert_eq!(names, ["m.jpg", "m.png", "m_1.jpg", "m_2.jpg", "m_3.jpg"]);
}
