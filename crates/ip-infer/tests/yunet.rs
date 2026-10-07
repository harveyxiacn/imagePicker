//! YuNet detection tests. They need the model file and download nothing: set
//! `IMAGEPICKER_YUNET_MODEL` (see `scripts/fetch-yunet.sh`), otherwise the tests skip.
//! `IMAGEPICKER_FACE_TEST_IMAGES` may name extra images (path-separator list) containing faces;
//! their detections are printed (`--nocapture`) and must be non-empty and well-formed.

use std::path::PathBuf;

use ip_infer::{FaceDetector, YUNET_ENV};

fn detector() -> Option<FaceDetector> {
    let Some(p) = std::env::var_os(YUNET_ENV).map(PathBuf::from) else {
        eprintln!("skip: {YUNET_ENV} is not set (run scripts/fetch-yunet.sh)");
        return None;
    };
    if !p.is_file() {
        eprintln!("skip: {} does not exist", p.display());
        return None;
    }
    Some(FaceDetector::load(&p).expect("load YuNet"))
}

#[test]
fn synthetic_image_has_no_faces() {
    let Some(det) = detector() else { return };
    // Smooth gradient with a hard-edged block: not a face.
    let (w, h) = (320usize, 241usize); // deliberately not a multiple of 32
    let mut rgb = vec![0u8; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let o = (y * w + x) * 3;
            rgb[o] = (x * 255 / w) as u8;
            rgb[o + 1] = (y * 255 / h) as u8;
            rgb[o + 2] = if (100..180).contains(&x) && (80..150).contains(&y) {
                255
            } else {
                64
            };
        }
    }
    let faces = det.try_detect(&rgb, w, h).expect("inference runs");
    println!("synthetic: {} faces", faces.len());
    assert!(faces.is_empty());
    assert!(det.try_detect(&rgb[..10], w, h).is_err());
}

#[test]
fn detects_faces_in_provided_images() {
    let Some(det) = detector() else { return };
    let Some(list) = std::env::var_os("IMAGEPICKER_FACE_TEST_IMAGES") else {
        eprintln!("skip: IMAGEPICKER_FACE_TEST_IMAGES is not set");
        return;
    };
    for path in std::env::split_paths(&list) {
        let img = image::open(&path).expect("decode").to_rgb8();
        let (w, h) = (img.width() as usize, img.height() as usize);
        let faces = det.try_detect(img.as_raw(), w, h).expect("inference");
        println!("{}: {} faces", path.display(), faces.len());
        for f in &faces {
            println!(
                "  score {:.4} bbox {:?} lm {:?}",
                f.score, f.bbox, f.landmarks
            );
            assert!(f.score >= 0.7 && f.score <= 1.0);
            assert!(f.bbox[2] > 0.0 && f.bbox[3] > 0.0);
        }
        assert!(
            !faces.is_empty(),
            "{} should contain a face",
            path.display()
        );
        assert!(faces.windows(2).all(|p| p[0].score >= p[1].score));
    }
}
