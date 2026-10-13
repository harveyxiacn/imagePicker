//! `POST /api/edits/{id}/auto-crop` (auto straighten and auto crop): stored tilt, the CPU
//! fallback, validation, and that nothing is saved.

use std::sync::Arc;
use std::time::Duration;

use crate::edit::{plan_auto_crop, AutoCropKind, AutoCropRequest};
use crate::testutil::{FakeImaging, FakeWorker};
use crate::*;

const WAIT: Duration = Duration::from_secs(30);

struct Env {
    core: Arc<Core>,
    _data: tempfile::TempDir,
    src: tempfile::TempDir,
}

/// Real CPU renderer (so the proxy is real pixels), fake imaging / worker.
fn env() -> Env {
    let data = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(2),
        worker: Some(Arc::new(FakeWorker::new())),
        renderer: None,
        force_cpu: true,
    })
    .unwrap();
    Env {
        core,
        _data: data,
        src,
    }
}

async fn setup(e: &Env) -> i64 {
    image::RgbImage::from_pixel(240, 160, image::Rgb([90, 120, 150]))
        .save_with_format(e.src.path().join("a.jpg"), image::ImageFormat::Jpeg)
        .unwrap();
    let s = e
        .core
        .import(ImportRequest {
            path: e.src.path().to_string_lossy().into_owned(),
            recursive: true,
            title: None,
        })
        .await
        .unwrap();
    let s = e.core.wait_session_ready(s.id, WAIT).await.unwrap();
    let page = e
        .core
        .photos(PhotoQuery {
            session_id: s.id,
            ..Default::default()
        })
        .await
        .unwrap();
    page.photos[0].id
}

async fn store_tilt(e: &Env, id: i64, deg: f64, conf: f64) {
    e.core
        .db
        .call(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO analysis(photo_id, profile, steps_done, tilt_deg, tilt_confidence)
                 VALUES(?1,'fast',1,?2,?3)",
                rusqlite::params![id, deg, conf],
            )?;
            Ok(())
        })
        .await
        .unwrap();
}

fn req(kind: AutoCropKind, ratio: Option<f64>) -> AutoCropRequest {
    AutoCropRequest {
        kind,
        ratio,
        rect: None,
        angle: None,
    }
}

#[tokio::test]
async fn straighten_uses_the_stored_tilt_and_saves_nothing() {
    let e = env();
    let id = setup(&e).await;
    store_tilt(&e, id, -3.04, 0.7).await;
    let out = e
        .core
        .auto_crop(id, req(AutoCropKind::Straighten, Some(1.5)))
        .await
        .unwrap();
    assert_eq!(out.source, "stored");
    assert!(!out.level);
    let crop = out.crop.unwrap();
    assert_eq!(crop.angle, -3.0, "angle = tilt rounded to the slider step");
    assert!(
        ip_render::crop_assist::rect_fits(240, 160, crop.angle, crop.rect, 1e-6),
        "{crop:?}"
    );
    let px = (crop.rect[2] * 240.0) / (crop.rect[3] * 160.0);
    assert!((px - 1.5).abs() < 0.01, "{px}");
    assert!(!e.core.photo(id).await.unwrap().has_edits);

    // inside the dead zone: level, angle 0
    store_tilt(&e, id, 0.08, 0.9).await;
    let out = e
        .core
        .auto_crop(id, req(AutoCropKind::Straighten, Some(1.5)))
        .await
        .unwrap();
    assert!(out.level);
    assert_eq!(out.crop.unwrap().angle, 0.0);
}

#[tokio::test]
async fn straighten_without_confident_tilt_suggests_nothing() {
    let e = env();
    let id = setup(&e).await;
    // not analysed, flat image: the CPU estimate finds no structure
    let out = e
        .core
        .auto_crop(id, req(AutoCropKind::Straighten, None))
        .await
        .unwrap();
    assert_eq!((out.source, out.crop), ("none", None));
    // a stored but weak value is ignored the same way
    store_tilt(&e, id, 6.0, 0.05).await;
    let out = e
        .core
        .auto_crop(id, req(AutoCropKind::Straighten, None))
        .await
        .unwrap();
    assert!(out.crop.is_none() && out.source == "none");
}

#[tokio::test]
async fn compose_returns_a_valid_crop_and_validates_input() {
    let e = env();
    let id = setup(&e).await;
    let out = e
        .core
        .auto_crop(id, req(AutoCropKind::Compose, Some(1.0)))
        .await
        .unwrap();
    assert_eq!(out.source, "saliency");
    let crop = out.crop.unwrap();
    assert!(ip_render::crop_assist::rect_fits(
        240, 160, 0.0, crop.rect, 1e-6
    ));
    assert!(matches!(
        e.core
            .auto_crop(999, req(AutoCropKind::Compose, None))
            .await,
        Err(CoreError::NotFound(_))
    ));
    for bad in [
        AutoCropRequest {
            ratio: Some(0.0),
            ..req(AutoCropKind::Compose, None)
        },
        AutoCropRequest {
            angle: Some(60.0),
            ..req(AutoCropKind::Compose, None)
        },
        AutoCropRequest {
            rect: Some([0.0, 0.0, 0.0, 1.0]),
            ..req(AutoCropKind::Straighten, None)
        },
    ] {
        assert!(matches!(
            e.core.auto_crop(id, bad).await,
            Err(CoreError::BadRequest(_))
        ));
    }
    let r: AutoCropRequest = serde_json::from_value(serde_json::json!({"kind":"compose"})).unwrap();
    assert_eq!(r.kind, AutoCropKind::Compose);
    assert!(serde_json::from_value::<AutoCropRequest>(serde_json::json!({"kind":"x"})).is_err());
}

#[test]
fn compose_keeps_faces_inside_and_rounds_inward() {
    let img = ip_render::RgbImage {
        width: 300,
        height: 200,
        data: vec![100; 300 * 200 * 3],
    };
    let face = [0.62f32, 0.30, 0.10, 0.14];
    let out = plan_auto_crop(
        &img,
        AutoCropKind::Compose,
        None,
        &[face],
        Some(0.8),
        [0.0, 0.0, 1.0, 1.0],
        0.0,
    );
    assert_eq!(out.source, "faces");
    let c = out.crop.unwrap();
    let (x, y, w, h) = (
        face[0] as f64,
        face[1] as f64,
        face[2] as f64,
        face[3] as f64,
    );
    assert!(
        c.rect[0] <= x
            && c.rect[1] <= y
            && c.rect[0] + c.rect[2] >= x + w
            && c.rect[1] + c.rect[3] >= y + h
    );
    assert!(
        c.rect
            .iter()
            .all(|v| (v * 1e4 - (v * 1e4).round()).abs() < 1e-6),
        "1e-4 grid {c:?}"
    );
    assert!(ip_render::crop_assist::rect_fits(
        300, 200, 0.0, c.rect, 1e-9
    ));
}
