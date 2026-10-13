//! F9.4 export watermark through the whole export task (real imaging layer): every container
//! carries the mark, byte-for-byte copies are re-encoded when a watermark is requested, bad
//! settings are refused.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ip_render::Backend;

use crate::export::ExportFormat;
use crate::testutil::{FakeRenderer, FakeWorker};
use crate::watermark::{Watermark, WatermarkKind, WatermarkPosition};
use crate::*;

const WAIT: Duration = Duration::from_secs(30);
const RGB: [u8; 3] = [200, 60, 40];

struct Env {
    core: Arc<Core>,
    _data: tempfile::TempDir,
    src: tempfile::TempDir,
    out: tempfile::TempDir,
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
        out: tempfile::tempdir().unwrap(),
    }
}

/// Imports `a.jpg` (flat 320x240) and returns its id and bytes.
async fn import_flat(e: &Env) -> (i64, Vec<u8>) {
    let path = e.src.path().join("a.jpg");
    let img = image::RgbImage::from_pixel(320, 240, image::Rgb(RGB));
    img.save(&path).unwrap();
    let s = e
        .core
        .import(ImportRequest {
            path: e.src.path().to_string_lossy().into_owned(),
            recursive: true,
            title: Some("t".into()),
        })
        .await
        .unwrap();
    e.core.wait_session_ready(s.id, WAIT).await.unwrap();
    let photos = e
        .core
        .photos(PhotoQuery {
            session_id: s.id,
            limit: Some(10),
            ..Default::default()
        })
        .await
        .unwrap()
        .photos;
    (photos[0].id, std::fs::read(&path).unwrap())
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

fn text_wm() -> Watermark {
    Watermark {
        text: "MMM".into(),
        font_size_pct: 12.0,
        opacity: 1.0,
        shadow: false,
        color: "#ffffff".into(),
        ..Default::default()
    }
}

fn request(e: &Env, id: i64, format: ExportFormat, wm: Option<Watermark>) -> ExportRequest {
    ExportRequest {
        ids: vec![id],
        dest: e.out.path().to_string_lossy().into_owned(),
        format,
        watermark: wm,
        ..Default::default()
    }
}

/// White-ish pixels (all channels > 150; the flat picture has green 60) in a quadrant:
/// `(right, bottom)` choose which.
fn bright_in(img: &image::RgbImage, right: bool, bottom: bool) -> usize {
    let (w, h) = img.dimensions();
    img.enumerate_pixels()
        .filter(|(x, y, p)| {
            (*x >= w / 2) == right && (*y >= h / 2) == bottom && p.0.iter().all(|c| *c > 150)
        })
        .count()
}

fn only_file(dir: &Path) -> std::path::PathBuf {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|d| d.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    files.remove(0)
}

fn clear(dir: &Path) {
    for d in std::fs::read_dir(dir).unwrap() {
        std::fs::remove_file(d.unwrap().path()).unwrap();
    }
}

#[tokio::test]
async fn every_format_carries_the_mark_only_where_it_belongs() {
    let e = env();
    let (id, _) = import_flat(&e).await;
    for (format, ext, long_edge) in [
        (ExportFormat::Jpeg, "jpg", None),
        (ExportFormat::Jpeg, "jpg", Some(200)), // the thumbnail fast path is bypassed
        (ExportFormat::Png, "png", None),
        (ExportFormat::Tiff, "tif", None),
        (ExportFormat::Webp, "webp", None),
        (ExportFormat::Original, "jpg", None),
    ] {
        let mut req = request(&e, id, format, Some(text_wm()));
        req.long_edge = long_edge;
        run(&e, req).await;
        let file = only_file(e.out.path());
        assert_eq!(file.extension().unwrap(), ext, "{format:?}");
        let img = image::open(&file).unwrap().to_rgb8();
        assert!(
            bright_in(&img, true, true) > 30,
            "{format:?}: the text is in the bottom right"
        );
        assert_eq!(bright_in(&img, false, false), 0, "{format:?}: top left clean");
        assert_eq!(bright_in(&img, true, false), 0, "{format:?}: top right clean");
        assert_eq!(bright_in(&img, false, true), 0, "{format:?}: bottom left clean");
        clear(e.out.path());
    }
}

#[tokio::test]
async fn a_copy_of_the_original_is_re_encoded_only_with_a_watermark() {
    let e = env();
    let (id, src) = import_flat(&e).await;
    run(&e, request(&e, id, ExportFormat::Original, None)).await;
    assert_eq!(std::fs::read(only_file(e.out.path())).unwrap(), src, "plain copy");
    clear(e.out.path());
    for (gps, meta) in [(false, false), (true, false), (false, true)] {
        let mut req = request(&e, id, ExportFormat::Original, Some(text_wm()));
        req.strip_gps = gps;
        req.strip_metadata = meta;
        run(&e, req).await;
        let file = only_file(e.out.path());
        let bytes = std::fs::read(&file).unwrap();
        assert_ne!(bytes, src, "gps={gps} meta={meta}: not a copy");
        let img = image::load_from_memory(&bytes).unwrap().to_rgb8();
        assert!(bright_in(&img, true, true) > 30);
        clear(e.out.path());
    }
    // the original itself is untouched
    assert_eq!(std::fs::read(e.src.path().join("a.jpg")).unwrap(), src);
}

#[tokio::test]
async fn a_logo_is_placed_by_position_size_and_margin() {
    let e = env();
    let (id, _) = import_flat(&e).await;
    // kept apart from the imported folder: a PNG there would be imported as a photo
    let logo_dir = tempfile::tempdir().unwrap();
    let logo_path = logo_dir.path().join("logo.png");
    image::RgbaImage::from_pixel(30, 30, image::Rgba([0, 0, 255, 255]))
        .save(&logo_path)
        .unwrap();
    let wm = Watermark {
        kind: WatermarkKind::Image,
        image_path: Some(logo_path.to_string_lossy().into_owned()),
        size_pct: 20.0,
        opacity: 1.0,
        position: WatermarkPosition::TopLeft,
        margin_pct: 5.0,
        ..Default::default()
    };
    run(&e, request(&e, id, ExportFormat::Png, Some(wm))).await;
    let img = image::open(only_file(e.out.path())).unwrap().to_rgb8();
    // 320 long edge: logo 64x64, margin 16
    assert_eq!(img.get_pixel(16 + 32, 16 + 32).0, [0, 0, 255]);
    assert_eq!(img.get_pixel(16 + 63, 16 + 63).0, [0, 0, 255]);
    // the source is a JPEG: the flat colour is only close to RGB
    let flat = |x: u32, y: u32| img.get_pixel(x, y).0.iter().zip(RGB).all(|(a, b)| a.abs_diff(b) <= 3);
    assert!(flat(15, 15) && flat(16 + 64, 16 + 64) && flat(300, 220));
}

#[tokio::test]
async fn bad_watermarks_are_refused_before_anything_is_written() {
    let e = env();
    let (id, _) = import_flat(&e).await;
    let bad: HashMap<&str, Watermark> = HashMap::from([
        (
            "opacity",
            Watermark {
                opacity: 2.0,
                ..text_wm()
            },
        ),
        (
            "empty",
            Watermark {
                text: " ".into(),
                ..text_wm()
            },
        ),
        (
            "font_path",
            Watermark {
                text: "水印".into(),
                ..text_wm()
            },
        ),
        (
            "image_path",
            Watermark {
                kind: WatermarkKind::Image,
                ..text_wm()
            },
        ),
        (
            "nope.png",
            Watermark {
                kind: WatermarkKind::Image,
                image_path: Some(e.src.path().join("nope.png").to_string_lossy().into_owned()),
                ..text_wm()
            },
        ),
    ]);
    for (needle, wm) in bad {
        let err = e
            .core
            .export(request(&e, id, ExportFormat::Jpeg, Some(wm)))
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::BadRequest(_)), "{needle}: {err:?}");
        assert!(err.to_string().contains(needle), "{needle}: {err}");
    }
    assert_eq!(std::fs::read_dir(e.out.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn text_tokens_come_from_the_photo() {
    let e = env();
    let (id, _) = import_flat(&e).await;
    // {name} is "a" and {year} the file's year: the expanded text is drawn
    let mut wm = text_wm();
    wm.text = "{name}{year}{camera}".into();
    run(&e, request(&e, id, ExportFormat::Png, Some(wm))).await;
    let img = image::open(only_file(e.out.path())).unwrap().to_rgb8();
    assert!(bright_in(&img, true, true) > 0);
}
