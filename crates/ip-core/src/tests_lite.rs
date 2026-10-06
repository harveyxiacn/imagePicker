//! The `lite` analysis profile end to end: real JPEGs on disk, `LiteWorker` (pure Rust, no
//! Python), grouping and scoring unchanged.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ip_worker_client::UnavailableWorker;

use crate::analysis::*;
use crate::fake_worker::FakeWorker;
use crate::testutil::FakeImaging;
use crate::*;

const BASE: u64 = 1_700_000_000;

struct Env {
    core: Arc<Core>,
    _data: tempfile::TempDir,
    src: tempfile::TempDir,
}

fn env_with(worker: Arc<dyn ip_worker_client::AiWorker>) -> Env {
    let data = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(2),
        worker: Some(worker),
        renderer: None,
        force_cpu: false,
    })
    .unwrap();
    Env {
        core,
        _data: data,
        src,
    }
}

/// A photo-like scene: smooth colour field plus a few hard-edged blocks, determined by `seed`.
fn scene(seed: u32, w: u32, h: u32) -> image::RgbImage {
    let mut s = seed.wrapping_mul(2654435761).wrapping_add(12345);
    let mut rnd = move || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        (s % 10_000) as f32 / 10_000.0
    };
    let (fx, fy, px, py): (Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>) = (
        (0..3).map(|_| 1.0 + rnd() * 3.0).collect(),
        (0..3).map(|_| 1.0 + rnd() * 3.0).collect(),
        (0..3).map(|_| rnd() * std::f32::consts::TAU).collect(),
        (0..3).map(|_| rnd() * std::f32::consts::TAU).collect(),
    );
    let blocks: Vec<(u32, u32, u32, u32, [u8; 3])> = (0..5)
        .map(|_| {
            let (bw, bh) = (
                (w as f32 * (0.1 + rnd() * 0.2)) as u32,
                (h as f32 * (0.1 + rnd() * 0.2)) as u32,
            );
            (
                (rnd() * (w - bw) as f32) as u32,
                (rnd() * (h - bh) as f32) as u32,
                bw,
                bh,
                [
                    (rnd() * 255.0) as u8,
                    (rnd() * 255.0) as u8,
                    (rnd() * 255.0) as u8,
                ],
            )
        })
        .collect();
    image::RgbImage::from_fn(w, h, |x, y| {
        for (bx, by, bw, bh, c) in &blocks {
            if x >= *bx && x < bx + bw && y >= *by && y < by + bh {
                return image::Rgb(*c);
            }
        }
        let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
        let ch = |i: usize| {
            let a = (fx[i] * std::f32::consts::TAU * u + px[i]).sin()
                * (fy[i] * std::f32::consts::TAU * v + py[i]).cos();
            (128.0 + 90.0 * a).clamp(0.0, 255.0) as u8
        };
        image::Rgb([ch(0), ch(1), ch(2)])
    })
}

/// Writes `img` as JPEG `name` with capture time BASE + `t_s`.
fn save(dir: &Path, name: &str, img: &image::RgbImage, t_s: u64) {
    let p = dir.join(name);
    img.save_with_format(&p, image::ImageFormat::Jpeg).unwrap();
    let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
    f.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(BASE + t_s))
        .unwrap();
}

fn brighten(img: &image::RgbImage, d: i32) -> image::RgbImage {
    image::RgbImage::from_fn(img.width(), img.height(), |x, y| {
        let p = img.get_pixel(x, y);
        image::Rgb(p.0.map(|c| (c as i32 + d).clamp(0, 255) as u8))
    })
}

/// burst 1: three frames of scene 1 (one soft), 1 s apart;
/// burst 2: two frames of scene 2, 1 s apart, an hour later;
/// single: scene 3, two hours later.
fn build(dir: &Path) {
    let (w, h) = (640, 480);
    let s1 = scene(1, w, h);
    save(dir, "a1.jpg", &s1, 0);
    save(dir, "a2_soft.jpg", &image::imageops::blur(&s1, 4.0), 1);
    save(dir, "a3.jpg", &brighten(&s1, 3), 2);
    let s2 = scene(2, w, h);
    save(dir, "b1.jpg", &s2, 3600);
    save(dir, "b2.jpg", &brighten(&s2, -3), 3601);
    save(dir, "c1.jpg", &scene(3, w, h), 7200);
}

async fn import(e: &Env) -> i64 {
    let s = e
        .core
        .import(ImportRequest {
            path: e.src.path().to_string_lossy().into_owned(),
            recursive: true,
            title: None,
        })
        .await
        .unwrap();
    e.core
        .wait_session_ready(s.id, Duration::from_secs(30))
        .await
        .unwrap();
    s.id
}

async fn run(e: &Env, sid: i64, profile: Profile) -> AnalysisStatus {
    e.core
        .analysis_run(AnalysisRunRequest {
            session_id: sid,
            profile,
            photo_ids: None,
            force: false,
            allow_download: false,
        })
        .await
        .unwrap();
    for _ in 0..800 {
        let s = e.core.analysis_status(sid);
        if s.state != RunState::Running {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("analysis did not finish");
}

async fn by_name(e: &Env, sid: i64) -> HashMap<String, Photo> {
    e.core
        .photos(PhotoQuery {
            session_id: sid,
            limit: Some(1000),
            ..Default::default()
        })
        .await
        .unwrap()
        .photos
        .into_iter()
        .map(|p| (p.file_name.clone(), p))
        .collect()
}

#[tokio::test]
async fn lite_profile_analyses_groups_and_scores_without_a_worker() {
    // the worker is permanently unavailable: lite must not touch it
    let e = env_with(Arc::new(UnavailableWorker::new("no worker on this device")));
    build(e.src.path());
    let sid = import(&e).await;

    let st = run(&e, sid, Profile::Lite).await;
    assert_eq!(st.state, RunState::Done, "{st:?}");
    assert_eq!(st.profile, Some(Profile::Lite));
    assert_eq!((st.done, st.total), (6, 6));
    assert!(st.error.is_none());

    // grouping on pHash + time only: {a1,a2,a3}, {b1,b2}, {c1}
    let groups = e.core.groups(sid).await.unwrap();
    let mut bursts: Vec<BurstOut> = groups.scenes.into_iter().flat_map(|s| s.bursts).collect();
    bursts.sort_by_key(|b| b.start_at);
    assert_eq!(
        bursts.iter().map(|b| b.size).collect::<Vec<_>>(),
        [3, 2, 1],
        "{bursts:?}"
    );

    let photos = by_name(&e, sid).await;
    for n in [
        "a1.jpg",
        "a2_soft.jpg",
        "a3.jpg",
        "b1.jpg",
        "b2.jpg",
        "c1.jpg",
    ] {
        let d = e.core.photo_analysis(photos[n].id).await.unwrap();
        assert!(d.analyzed, "{n}");
        assert_eq!(d.profile.as_deref(), Some("lite"));
        let s = d.scores.unwrap();
        // lite scores: classic components only
        assert!(s.sharpness.is_some() && s.exposure.is_some() && s.noise.is_some());
        assert!(s.iqa.is_none() && s.aesthetic.is_none() && s.face.is_none());
        assert!(
            d.ai_score.is_some() && d.ai_rating.is_some(),
            "{n}: scores renormalise"
        );
        assert!(d.faces.is_empty());
    }

    // the soft frame scores lower than its sharp siblings and is never the burst's best
    let soft = e
        .core
        .photo_analysis(photos["a2_soft.jpg"].id)
        .await
        .unwrap();
    let sharp = e.core.photo_analysis(photos["a1.jpg"].id).await.unwrap();
    assert!(
        soft.scores.as_ref().unwrap().sharpness < sharp.scores.as_ref().unwrap().sharpness,
        "{:?} vs {:?}",
        soft.scores,
        sharp.scores
    );
    assert!(soft.ai_score < sharp.ai_score);
    let first = bursts.iter().find(|b| b.size == 3).unwrap();
    assert_ne!(first.best_photo_id, Some(photos["a2_soft.jpg"].id));

    // lite results do not count as analysed for `fast`/`standard`, but a repeat lite run is a no-op
    let again = run(&e, sid, Profile::Lite).await;
    assert_eq!((again.state, again.total), (RunState::Done, 0));
}

#[tokio::test]
async fn lite_results_are_upgraded_by_a_fast_run() {
    let worker = Arc::new(FakeWorker::new());
    let e = env_with(worker.clone());
    build(e.src.path());
    let sid = import(&e).await;
    let lite = run(&e, sid, Profile::Lite).await;
    assert_eq!((lite.state, lite.total), (RunState::Done, 6));
    assert_eq!(
        worker
            .analyze_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "lite never calls the worker"
    );
    let fast = run(&e, sid, Profile::Fast).await;
    assert_eq!((fast.state, fast.total), (RunState::Done, 6));
    assert!(
        worker
            .analyze_calls
            .load(std::sync::atomic::Ordering::SeqCst)
            > 0
    );
    let photos = by_name(&e, sid).await;
    let d = e.core.photo_analysis(photos["a1.jpg"].id).await.unwrap();
    assert_eq!(d.profile.as_deref(), Some("fast"));
}

#[tokio::test]
async fn hardware_reports_an_unavailable_worker_cleanly() {
    let e = env_with(Arc::new(UnavailableWorker::new("no worker on this device")));
    let hw = e.core.hardware(true).await.unwrap();
    assert_eq!(hw.state, "unavailable");
    assert!(hw.error.unwrap().contains("no worker"));
}

#[test]
fn profile_wire_names() {
    assert_eq!(Profile::parse("lite"), Some(Profile::Lite));
    assert_eq!(serde_json::to_string(&Profile::Lite).unwrap(), "\"lite\"");
    assert_eq!(Profile::Lite.level(), Profile::Fast.level());
}
