//! Spawns the REAL Python worker (`ai-worker/`, needs `uv` and `uv sync --extra cuda --extra mediapipe`)
//! and analyses a few generated images end to end.
//!
//! Run manually (downloads models into `ai-worker/.models` on first use):
//!   cargo test -p ip-core --test real_worker -- --ignored --nocapture

use std::path::Path;
use std::time::Duration;

use ip_core::{AnalysisRunRequest, Core, CoreConfig, ImportRequest, PhotoQuery, Profile, RunState};

const BASE: u64 = 1_700_000_000;

/// A coloured scene with a few shapes; `shift` moves the shapes slightly (burst frames), `seed`
/// changes the whole picture.
fn scene(seed: u32, shift: i32, blur_noise: u8) -> image::RgbImage {
    let (w, h) = (640u32, 480u32);
    let mut img = image::RgbImage::from_fn(w, h, |x, y| {
        let t = y as f32 / h as f32;
        let base = [
            (40.0 + 180.0 * t) as u8,
            (90.0 + 80.0 * ((seed as f32 + x as f32 * 0.01).sin() * 0.5 + 0.5)) as u8,
            (200.0 - 150.0 * t) as u8,
        ];
        let n = ((x * 7 + y * 13 + seed * 31) % (blur_noise as u32 + 1)) as u8;
        image::Rgb([
            base[0].saturating_add(n),
            base[1].saturating_add(n),
            base[2].saturating_add(n),
        ])
    });
    let cx = 200 + (seed as i32 % 5) * 40 + shift;
    for y in 150..330 {
        for x in cx..cx + 180 {
            if (0..w as i32).contains(&x) {
                img.put_pixel(x as u32, y, image::Rgb([230, 60 + (seed % 100) as u8, 40]));
            }
        }
    }
    img
}

fn write(dir: &Path, name: &str, img: &image::RgbImage, t_s: u64) {
    let p = dir.join(name);
    img.save_with_format(&p, image::ImageFormat::Jpeg).unwrap();
    let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
    f.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(BASE + t_s))
        .unwrap();
}

#[tokio::test]
#[ignore = "spawns the Python worker (uv) and downloads models on first use"]
async fn real_worker_analyses_generated_images() {
    let worker_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ai-worker");
    std::env::set_var("IMAGEPICKER_WORKER_DIR", &worker_dir);
    std::env::set_var("IMAGEPICKER_MODELS_DIR", worker_dir.join(".models"));

    let data = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    // a burst of three near-identical frames one second apart, plus two unrelated pictures
    write(src.path(), "burst_1.jpg", &scene(1, 0, 4), 0);
    write(src.path(), "burst_2.jpg", &scene(1, 3, 4), 1);
    write(src.path(), "burst_3.jpg", &scene(1, 6, 6), 2);
    write(src.path(), "other_a.jpg", &scene(42, 0, 30), 4000);
    write(src.path(), "other_b.jpg", &scene(77, 90, 2), 9000);

    let core = Core::open(CoreConfig::new(Some(data.path().to_path_buf()))).unwrap();
    let s = core
        .import(ImportRequest {
            path: src.path().to_string_lossy().into_owned(),
            recursive: true,
            title: None,
        })
        .await
        .unwrap();
    core.wait_session_ready(s.id, Duration::from_secs(60))
        .await
        .unwrap();

    let t0 = std::time::Instant::now();
    core.analysis_run(AnalysisRunRequest {
        session_id: s.id,
        profile: Profile::Standard,
        photo_ids: None,
        force: false,
        allow_download: true,
    })
    .await
    .unwrap();
    let status = loop {
        let st = core.analysis_status(s.id);
        if st.state != RunState::Running {
            break st;
        }
        assert!(
            t0.elapsed() < Duration::from_secs(1800),
            "analysis takes too long: {st:?}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    eprintln!(
        "analysis took {:.1}s: {status:?}",
        t0.elapsed().as_secs_f64()
    );
    assert_eq!(status.state, RunState::Done, "{status:?}");

    let page = core
        .photos(PhotoQuery {
            session_id: s.id,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(page.photos.len(), 5);
    assert!(page
        .photos
        .iter()
        .all(|p| p.analyzed && p.ai_rating.is_some()));
    let by = |n: &str| page.photos.iter().find(|p| p.file_name == n).unwrap();
    assert_eq!(by("burst_1.jpg").burst_id, by("burst_2.jpg").burst_id);
    assert_eq!(by("burst_1.jpg").burst_id, by("burst_3.jpg").burst_id);
    assert_ne!(by("burst_1.jpg").burst_id, by("other_a.jpg").burst_id);
    assert_ne!(by("other_a.jpg").burst_id, by("other_b.jpg").burst_id);
    let d = core.photo_analysis(by("burst_1.jpg").id).await.unwrap();
    let sc = d.scores.unwrap();
    assert!(sc.sharpness.is_some() && sc.exposure.is_some());
    eprintln!("scores: {sc:?}, scene: {:?}", d.scene_type);
    let hw = core.hardware(false).await.unwrap();
    eprintln!("worker: {hw:?}");
    assert!(hw.tier.is_some());
    core.worker.shutdown().await;
}
