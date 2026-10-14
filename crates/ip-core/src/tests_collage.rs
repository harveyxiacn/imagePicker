//! F9.5 collage export through the whole task (real imaging layer): file written once, never
//! over an existing file, every format, edits rendered, bad requests refused.

use std::sync::Arc;
use std::time::Duration;

use ip_render::Backend;

use crate::collage::{CollageFit, CollageLayout, CollageRequest};
use crate::export::ExportFormat;
use crate::testutil::{FakeRenderer, FakeWorker};
use crate::*;

const WAIT: Duration = Duration::from_secs(60);
const COLORS: [[u8; 3]; 4] = [[220, 30, 30], [30, 200, 40], [40, 60, 230], [230, 210, 20]];

struct Env {
    core: Arc<Core>,
    _data: tempfile::TempDir,
    src: tempfile::TempDir,
    out: tempfile::TempDir,
}

fn env() -> Env {
    let data = tempfile::tempdir().unwrap();
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
        src: tempfile::tempdir().unwrap(),
        out: tempfile::tempdir().unwrap(),
    }
}

/// Imports four flat pictures `p1.jpg`..`p4.jpg` of different shapes; ids in file order.
async fn import4(e: &Env) -> Vec<i64> {
    for (i, c) in COLORS.iter().enumerate() {
        let (w, h) = [(320, 240), (240, 320), (300, 300), (400, 200)][i];
        image::RgbImage::from_pixel(w, h, image::Rgb(*c))
            .save(e.src.path().join(format!("p{}.jpg", i + 1)))
            .unwrap();
    }
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
    let mut photos = e
        .core
        .photos(PhotoQuery {
            session_id: s.id,
            limit: Some(10),
            ..Default::default()
        })
        .await
        .unwrap()
        .photos;
    photos.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    photos.iter().map(|p| p.id).collect()
}

fn request(e: &Env, ids: Vec<i64>) -> CollageRequest {
    CollageRequest {
        ids,
        dest: e.out.path().to_string_lossy().into_owned(),
        layout: CollageLayout::Grid,
        width: 400,
        height: 400,
        gap: 8,
        margin: 8,
        background: "#0a141e".into(),
        fit: CollageFit::Cover,
        caption_name: true,
        caption_rating: true,
        format: ExportFormat::Png,
        quality: 90,
        name_template: "collage".into(),
        apply_edits: true,
        watermark: None,
    }
}

async fn run(e: &Env, req: CollageRequest) -> (String, Option<String>) {
    let mut rx = e.core.events.subscribe();
    let id = e.core.export_collage(req).await.unwrap();
    tokio::time::timeout(WAIT, async {
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
    .unwrap()
}

fn files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|d| d.unwrap().path())
        .collect();
    v.sort();
    v
}

fn px(img: &image::RgbImage, x: u32, y: u32) -> [u8; 3] {
    img.get_pixel(x, y).0
}

fn near(a: [u8; 3], b: [u8; 3]) -> bool {
    (0..3).all(|c| (a[c] as i32 - b[c] as i32).abs() <= 6)
}

#[tokio::test(flavor = "multi_thread")]
async fn collage_task_writes_one_file_and_never_overwrites() {
    let e = env();
    let ids = import4(&e).await;
    let (state, error) = run(&e, request(&e, ids.clone())).await;
    assert_eq!((state.as_str(), error), ("done", None));
    let out = files(e.out.path());
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].file_name().unwrap(), "collage.png");
    let img = image::open(&out[0]).unwrap().to_rgb8();
    assert_eq!(img.dimensions(), (400, 400));
    // 2x2 grid with 8 px margin and gap: cells are 192 px
    let centres = [(104, 104), (296, 104), (104, 296), (296, 296)];
    for (i, (x, y)) in centres.iter().enumerate() {
        assert!(
            near(px(&img, *x, *y), COLORS[i]),
            "cell {i}: {:?}",
            px(&img, *x, *y)
        );
    }
    assert!(
        near(px(&img, 4, 4), [10, 20, 30]),
        "margin is the background"
    );
    assert!(
        near(px(&img, 200, 200), [10, 20, 30]),
        "gap is the background"
    );
    let before = std::fs::read(&out[0]).unwrap();
    // again: the existing file stays, the new one gets a suffix
    let (state, _) = run(&e, request(&e, ids)).await;
    assert_eq!(state, "done");
    let out = files(e.out.path());
    assert_eq!(
        out.iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        ["collage.png", "collage_1.png"]
    );
    assert_eq!(std::fs::read(&out[0]).unwrap(), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn every_format_and_layout_works() {
    let e = env();
    let ids = import4(&e).await;
    for (format, ext) in [
        (ExportFormat::Jpeg, "jpg"),
        (ExportFormat::Png, "png"),
        (ExportFormat::Tiff, "tif"),
        (ExportFormat::Webp, "webp"),
    ] {
        for layout in [
            CollageLayout::Grid,
            CollageLayout::Story,
            CollageLayout::Contact,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let mut req = request(&e, ids.clone());
            req.dest = dir.path().to_string_lossy().into_owned();
            req.format = format;
            req.layout = layout;
            req.fit = CollageFit::Contain;
            req.width = 480;
            req.height = 320;
            let (state, error) = run(&e, req).await;
            assert_eq!(
                (state.as_str(), error),
                ("done", None),
                "{format:?} {layout:?}"
            );
            let out = files(dir.path());
            assert_eq!(out.len(), 1);
            assert_eq!(out[0].extension().unwrap(), ext);
            assert_eq!(
                image::open(&out[0]).unwrap().to_rgb8().dimensions(),
                (480, 320)
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_requests_are_refused() {
    let e = env();
    let ids = import4(&e).await;
    let refused = |req: CollageRequest| {
        let core = e.core.clone();
        async move { core.export_collage(req).await.unwrap_err() }
    };
    let err = refused(request(&e, vec![ids[0]])).await;
    assert!(err.to_string().contains("2..30"), "{err}");
    let err = refused(request(&e, vec![ids[0], 999_999])).await;
    assert!(matches!(err, CoreError::NotFound(_)), "{err:?}");
    let mut req = request(&e, ids.clone());
    req.width = 100;
    req.height = 100;
    req.margin = 60;
    let err = refused(req).await;
    assert!(err.to_string().contains("too small"), "{err}");
    let mut req = request(&e, ids.clone());
    req.watermark = Some(crate::watermark::Watermark {
        opacity: 2.0,
        ..Default::default()
    });
    assert!(e.core.export_collage(req).await.is_err());
    assert!(files(e.out.path()).is_empty(), "nothing written");
}
