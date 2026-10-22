//! Full-resolution loupe images (`Core::full_path`, `GET /api/full/{id}`): native pixel size,
//! orientation, cache, errors.

use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use crate::testutil::{write_jpeg, FakeImaging};
use crate::*;

const WAIT: Duration = Duration::from_secs(30);

fn open(imaging: Arc<dyn Imaging>, data: &Path) -> Arc<Core> {
    Core::open(CoreConfig {
        data_dir: Some(data.to_path_buf()),
        imaging,
        thumb_workers: Some(2),
        worker: None,
        renderer: None,
        force_cpu: false,
    })
    .unwrap()
}

async fn import_dir(core: &Arc<Core>, dir: &Path) -> Vec<Photo> {
    let s = core
        .import(ImportRequest {
            path: dir.to_string_lossy().into_owned(),
            recursive: true,
            title: None,
        })
        .await
        .unwrap();
    core.wait_session_ready(s.id, WAIT).await.unwrap();
    let mut photos = core
        .photos(PhotoQuery {
            session_id: s.id,
            ..Default::default()
        })
        .await
        .unwrap()
        .photos;
    photos.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    photos
}

fn dims(path: &Path) -> (u32, u32) {
    image::image_dimensions(path).unwrap()
}

/// Inserts an EXIF block holding only `Orientation` right after the SOI marker.
pub(crate) fn with_orientation(jpeg: &[u8], orientation: u16) -> Vec<u8> {
    let mut tiff = Vec::new();
    tiff.extend_from_slice(b"II*\0");
    tiff.extend_from_slice(&8u32.to_le_bytes());
    tiff.extend_from_slice(&1u16.to_le_bytes());
    tiff.extend_from_slice(&0x0112u16.to_le_bytes());
    tiff.extend_from_slice(&3u16.to_le_bytes());
    tiff.extend_from_slice(&1u32.to_le_bytes());
    tiff.extend_from_slice(&orientation.to_le_bytes());
    tiff.extend_from_slice(&[0, 0]);
    tiff.extend_from_slice(&0u32.to_le_bytes());
    let mut payload = b"Exif\0\0".to_vec();
    payload.extend_from_slice(&tiff);
    let mut out = jpeg[..2].to_vec();
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(&payload);
    out.extend_from_slice(&jpeg[2..]);
    out
}

#[tokio::test]
async fn full_image_is_native_beyond_the_largest_preview_and_cached() {
    let data = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let imaging = Arc::new(FakeImaging::new());
    let core = open(imaging.clone(), data.path());
    write_jpeg(&src.path().join("wide.jpg"), 5000, 400, 90);
    let photos = import_dir(&core, src.path()).await;
    let id = photos[0].id;
    assert_eq!((photos[0].width, photos[0].height), (Some(5000), Some(400)));

    // the largest preview is capped ...
    let preview = core.preview_path(id, 4096).await.unwrap();
    assert_eq!(dims(&preview).0, 4096);

    // ... the full-resolution image is not; concurrent requests share one decode
    let before = imaging.thumbnails_generated.load(Ordering::SeqCst);
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let core = core.clone();
        tasks.push(tokio::spawn(
            async move { core.full_path(id).await.unwrap() },
        ));
    }
    let mut paths = std::collections::HashSet::new();
    for t in tasks {
        paths.insert(t.await.unwrap());
    }
    assert_eq!(paths.len(), 1);
    let full = paths.into_iter().next().unwrap();
    assert_eq!(dims(&full), (5000, 400));
    assert!(full.starts_with(data.path().join("cache").join("full")));
    assert!(full.to_string_lossy().ends_with("_full.jpg"));
    assert_eq!(
        imaging.thumbnails_generated.load(Ordering::SeqCst) - before,
        1
    );

    // cache hit: no second decode
    assert_eq!(core.full_path(id).await.unwrap(), full);
    assert_eq!(
        imaging.thumbnails_generated.load(Ordering::SeqCst) - before,
        1
    );

    // a vanished cache file is regenerated
    std::fs::remove_file(&full).unwrap();
    assert_eq!(core.full_path(id).await.unwrap(), full);
    assert!(full.exists());
    assert_eq!(
        imaging.thumbnails_generated.load(Ordering::SeqCst) - before,
        2
    );

    // counted (and cleared) with the previews
    let info = core.cache_info().await.unwrap();
    assert!(info.items.previews >= 2, "{info:?}");
    core.cache_clear(vec!["previews".into()]).await.unwrap();
    assert!(!full.exists());

    // deleting the photo removes its full-resolution image too
    assert_eq!(core.full_path(id).await.unwrap(), full);
    core.delete_session(photos[0].session_id).await.unwrap();
    assert!(!full.exists());
}

#[tokio::test]
async fn full_image_honours_exif_orientation() {
    let data = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let core = open(Arc::new(RealImaging), data.path());
    // stored 600x400: left half red, right half blue; EXIF orientation 6 (rotate 90° clockwise)
    let img = image::RgbImage::from_fn(600, 400, |x, _| {
        if x < 300 {
            image::Rgb([230, 20, 20])
        } else {
            image::Rgb([20, 20, 230])
        }
    });
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95)
        .encode_image(&img)
        .unwrap();
    std::fs::write(src.path().join("rotated.jpg"), with_orientation(&jpeg, 6)).unwrap();

    let photos = import_dir(&core, src.path()).await;
    let p = &photos[0];
    assert_eq!((p.width, p.height), (Some(400), Some(600)), "display size");
    let full = core.full_path(p.id).await.unwrap();
    let out = image::open(&full).unwrap().to_rgb8();
    assert_eq!(out.dimensions(), (400, 600), "native size, upright");
    // the stored left edge is now the top
    let top = out.get_pixel(200, 100);
    let bottom = out.get_pixel(200, 500);
    assert!(top[0] > 180 && top[2] < 80, "top {top:?}");
    assert!(bottom[2] > 180 && bottom[0] < 80, "bottom {bottom:?}");
}

#[tokio::test]
async fn full_image_errors() {
    let data = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let core = open(Arc::new(FakeImaging::new()), data.path());
    assert!(matches!(
        core.full_path(424242).await,
        Err(CoreError::NotFound(_))
    ));

    write_jpeg(&src.path().join("a_damaged.jpg"), 320, 200, 10);
    write_jpeg(&src.path().join("b_missing.jpg"), 320, 200, 20);
    let photos = import_dir(&core, src.path()).await;
    let (damaged, missing) = (&photos[0], &photos[1]);

    // the original is there but cannot be decoded: 415
    std::fs::write(&damaged.path, b"not a jpeg at all").unwrap();
    match core.full_path(damaged.id).await {
        Err(e @ CoreError::Unsupported(_)) => assert_eq!(e.code(), "unsupported_media"),
        other => panic!("expected Unsupported, got {other:?}"),
    }

    // the original is gone (and nothing is cached): 404
    std::fs::remove_file(&missing.path).unwrap();
    match core.full_path(missing.id).await {
        Err(CoreError::NotFound(m)) => assert!(m.contains("missing"), "{m}"),
        other => panic!("expected NotFound, got {other:?}"),
    }
}

/// Reproduce opening the grid while import has inserted rows but has not committed EXIF.
/// Use real pixels and every EXIF transform, including mirrors/180 degrees (dimensions alone
/// cannot detect those). Also seed a legacy sideways cache as an existing user would have.
#[tokio::test]
async fn images_requested_before_metadata_are_upright_and_skip_legacy_cache() {
    let data = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let core = open(Arc::new(RealImaging), data.path());
    let img = image::RgbImage::from_fn(600, 400, |x, y| {
        if x < 300 && y < 200 {
            image::Rgb([230, 20, 20])
        } else {
            image::Rgb([20, 20, 230])
        }
    });
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95)
        .encode_image(&img)
        .unwrap();
    for orientation in 1..=8 {
        std::fs::write(
            src.path().join(format!("o{orientation}.jpg")),
            with_orientation(&jpeg, orientation),
        )
        .unwrap();
    }
    let photos = import_dir(&core, src.path()).await;
    for (index, p) in photos.iter().enumerate() {
        let orientation = (index + 1) as u8;
        let r = core
            .db
            .with(|c| crate::catalog::photo_ref(c, p.id))
            .unwrap();
        core.thumbs.purge(std::slice::from_ref(&r.fast_key));
        let legacy = ip_imaging::ThumbCache::new(data.path().join("cache/thumbs"))
            .path_for(&r.fast_key, 256);
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, &jpeg).unwrap();
        core.db
            .with(|c| {
                c.execute(
                    "UPDATE photo SET meta_done=0, orientation=1, thumb_state=0 WHERE id=?1",
                    [p.id],
                )?;
                Ok(())
            })
            .unwrap();
        let stale = core
            .db
            .with(|c| crate::catalog::photo_ref(c, p.id))
            .unwrap();
        // Check the background grid worker as well as direct preview/full requests.
        let tracker = crate::thumbs::Tracker::new();
        core.thumbs.enqueue(&[p.id], Some(&tracker), true);
        tracker.wait_idle().await;
        for size in [256, 1024, 0] {
            let path = core.thumbs.ensure_ref(stale.clone(), size).await.unwrap();
            assert_ne!(path, legacy);
            let out = image::open(path).unwrap().to_rgb8();
            let edge = if size == 0 { 65535 } else { size };
            let expected =
                ip_imaging::generate_thumbnail(&r.path, r.format, orientation, edge, 90).unwrap();
            let reference = image::load_from_memory(&expected.bytes).unwrap().to_rgb8();
            assert_eq!(
                out.dimensions(),
                reference.dimensions(),
                "orientation {orientation}, size {size}"
            );
            for (x, y) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                let (x, y) = (
                    (out.width() as f64 * x) as u32,
                    (out.height() as f64 * y) as u32,
                );
                let a = out.get_pixel(x, y);
                let b = reference.get_pixel(x, y);
                assert_eq!(
                    a[0] > a[2],
                    b[0] > b[2],
                    "orientation {orientation}, size {size}, pixel {x},{y}"
                );
            }
        }
        assert_ne!(
            p.thumb_version,
            r.fast_key.chars().take(8).collect::<String>()
        );
        // Metadata finished after the caller read its PhotoRef: use the current row.
        core.thumbs.purge(std::slice::from_ref(&r.fast_key));
        core.db
            .with(|c| {
                c.execute(
                    "UPDATE photo SET meta_done=1, orientation=?2 WHERE id=?1",
                    rusqlite::params![p.id, orientation],
                )?;
                Ok(())
            })
            .unwrap();
        let path = core.thumbs.ensure_ref(stale, 256).await.unwrap();
        let want = if orientation >= 5 {
            (171, 256)
        } else {
            (256, 171)
        };
        assert_eq!(dims(&path), want);
    }
}

fn mean_of_file(p: &Path) -> f64 {
    let img = image::open(p).unwrap().to_rgb8();
    img.as_raw().iter().map(|&v| v as f64).sum::<f64>() / img.as_raw().len() as f64
}

#[tokio::test]
async fn edited_photos_show_the_rendered_edit_at_full_resolution() {
    let data = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let renderer = Arc::new(crate::testutil::FakeRenderer::new(ip_render::Backend::Gpu));
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(2),
        worker: Some(Arc::new(crate::testutil::FakeWorker::new())),
        renderer: Some(renderer),
        force_cpu: true,
    })
    .unwrap();
    write_jpeg(&src.path().join("e.jpg"), 4600, 300, 100);
    let id = import_dir(&core, src.path()).await[0].id;
    let plain = core.full_path(id).await.unwrap();

    core.put_edit(
        id,
        serde_json::json!({"version":1,"ops":[{"type":"global","exposure":1.5}]}),
    )
    .await
    .unwrap();
    let edited = core.full_path(id).await.unwrap();
    assert_ne!(edited, plain);
    assert!(edited.starts_with(data.path().join("cache").join("full")));
    assert_eq!(
        dims(&edited),
        (4600, 300),
        "rendered at the photo's own size"
    );
    assert!(mean_of_file(&edited) > mean_of_file(&plain) + 20.0);
    assert_eq!(core.full_path(id).await.unwrap(), edited, "cached");

    // clearing the edit shows the original pixels again
    core.delete_edit(id).await.unwrap();
    assert_eq!(core.full_path(id).await.unwrap(), plain);
}
