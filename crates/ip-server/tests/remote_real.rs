//! Smoke test of remote AI against the REAL Python worker: a "phone" core analyses synthetic
//! JPEGs through a host server whose worker is the repo's `ai-worker` (started with `uv`).
//!
//! Run manually (needs `uv`, the installed worker environment and its models):
//!   IMAGEPICKER_WORKER_DIR=<repo>/ai-worker IMAGEPICKER_MODELS_DIR=<repo>/ai-worker/.models \
//!     cargo test -p ip-server --test remote_real -- --ignored --nocapture
//! Without the two variables the worktree's own `ai-worker/` and `ai-worker/.models` are used.

use std::path::Path;
use std::time::{Duration, Instant};

use ip_core::auth::LanSettings;
use ip_core::{AnalysisRunRequest, Core, CoreConfig, ImportRequest, PhotoQuery, Profile, RunState};
use ip_server::{serve_listener_with, SecurityStore, ServerOptions};
use serde_json::{json, Value};
use tokio::net::TcpListener;

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const BASE: u64 = 1_700_000_000;

fn scene(seed: u32, shift: i32, noise: u8) -> image::RgbImage {
    let (w, h) = (2400u32, 1600u32);
    let mut img = image::RgbImage::from_fn(w, h, |x, y| {
        let t = y as f32 / h as f32;
        let base = [
            (40.0 + 180.0 * t) as u8,
            (90.0 + 80.0 * ((seed as f32 + x as f32 * 0.004).sin() * 0.5 + 0.5)) as u8,
            (200.0 - 150.0 * t) as u8,
        ];
        let n = ((x * 7 + y * 13 + seed * 31) % (noise as u32 + 1)) as u8;
        image::Rgb([
            base[0].saturating_add(n),
            base[1].saturating_add(n),
            base[2].saturating_add(n),
        ])
    });
    let cx = 700 + (seed as i32 % 5) * 120 + shift;
    for y in 500..1100 {
        for x in cx..cx + 600 {
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

async fn analyse(core: &std::sync::Arc<Core>, dir: &Path) -> (std::time::Duration, usize) {
    let s = core
        .import(ImportRequest {
            path: dir.to_string_lossy().into_owned(),
            recursive: true,
            title: None,
        })
        .await
        .unwrap();
    core.wait_session_ready(s.id, Duration::from_secs(60))
        .await
        .unwrap();
    let t0 = Instant::now();
    core.analysis_run(AnalysisRunRequest {
        session_id: s.id,
        profile: Profile::Standard,
        photo_ids: None,
        force: false,
        allow_download: false,
    })
    .await
    .unwrap();
    let status = loop {
        let st = core.analysis_status(s.id);
        if st.state != RunState::Running {
            break st;
        }
        assert!(t0.elapsed() < Duration::from_secs(1800), "{st:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let took = t0.elapsed();
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
    // grouping uses the SigLIP embeddings that came back as .npy artifacts
    assert_eq!(by("burst_1.jpg").burst_id, by("burst_2.jpg").burst_id);
    assert_eq!(by("burst_1.jpg").burst_id, by("burst_3.jpg").burst_id);
    assert_ne!(by("burst_1.jpg").burst_id, by("other_a.jpg").burst_id);
    let d = core.photo_analysis(by("burst_1.jpg").id).await.unwrap();
    assert!(d.scores.unwrap().sharpness.is_some());
    (took, page.photos.len())
}

#[tokio::test]
#[ignore = "spawns the real Python worker (uv) and needs its models"]
async fn phone_core_analyses_through_the_real_host_worker() {
    let worker_dir = std::env::var_os("IMAGEPICKER_WORKER_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ai-worker"));
    std::env::set_var("IMAGEPICKER_WORKER_DIR", &worker_dir);
    if std::env::var_os("IMAGEPICKER_MODELS_DIR").is_none() {
        std::env::set_var("IMAGEPICKER_MODELS_DIR", worker_dir.join(".models"));
    }

    let src = tempfile::tempdir().unwrap();
    write(src.path(), "burst_1.jpg", &scene(1, 0, 4), 0);
    write(src.path(), "burst_2.jpg", &scene(1, 10, 4), 1);
    write(src.path(), "burst_3.jpg", &scene(1, 20, 6), 2);
    write(src.path(), "other_a.jpg", &scene(42, 0, 30), 4000);
    write(src.path(), "other_b.jpg", &scene(77, 300, 2), 9000);

    // ---- host: real core + real worker, served in token mode with LAN login
    let host_data = tempfile::tempdir().unwrap();
    let host_core = Core::open(CoreConfig::new(Some(host_data.path().to_path_buf()))).unwrap();
    let store = SecurityStore::open(host_data.path());
    store.set_passwords("owner password 1", None).unwrap();
    store
        .set_lan(LanSettings {
            enabled: true,
            port: 7878,
            guest_enabled: false,
        })
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let host = serve_listener_with(
        listener,
        host_core.clone(),
        None,
        ServerOptions {
            session_token: Some(TOKEN.into()),
            lan: true,
            dev_cors: false,
            ..ServerOptions::default()
        },
    )
    .unwrap();

    // ---- phone: its own core and server, paired through the API
    let phone_data = tempfile::tempdir().unwrap();
    let phone_core = Core::open(CoreConfig::new(Some(phone_data.path().to_path_buf()))).unwrap();
    let phone = serve_listener_with(
        TcpListener::bind("127.0.0.1:0").await.unwrap(),
        phone_core.clone(),
        None,
        ServerOptions {
            dev_cors: false,
            ..ServerOptions::default()
        },
    )
    .unwrap();
    ip_worker_client::remote::ensure_crypto_provider();
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let start: Value = http
        .post(format!("{}/api/remote/pair/start", host.url()))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let t_pair = Instant::now();
    let st: Value = http
        .post(format!("{}/api/remote/connect", phone.url()))
        .json(&json!({"host_url": host.url(), "code": start["code"], "device_name": "smoke phone"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    eprintln!("paired in {:.2}s: {st}", t_pair.elapsed().as_secs_f64());
    assert_eq!(st["connected"], true, "{st}");

    // 1. phone -> host (real CUDA worker), cold: includes starting the worker
    let (remote_cold, n) = analyse(&phone_core, src.path()).await;
    eprintln!(
        "remote analysis of {n} photos (cold worker): {:.1}s",
        remote_cold.as_secs_f64()
    );
    let status: Value = http
        .get(format!("{}/api/remote/status", phone.url()))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    eprintln!("phone status: {status}");
    assert!(status["host_tier"].is_string());

    // 2. again, forced, warm worker: remote
    let sid = phone_core.sessions().await.unwrap()[0].id;
    let t = Instant::now();
    phone_core
        .analysis_run(AnalysisRunRequest {
            session_id: sid,
            profile: Profile::Standard,
            photo_ids: None,
            force: true,
            allow_download: false,
        })
        .await
        .unwrap();
    while phone_core.analysis_status(sid).state == RunState::Running {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let remote_warm = t.elapsed();
    assert_eq!(phone_core.analysis_status(sid).state, RunState::Done);
    eprintln!(
        "remote analysis (warm worker): {:.2}s",
        remote_warm.as_secs_f64()
    );

    // 3. the same photos analysed directly on the host (warm): the overhead of the remote path
    let (local_warm, _) = analyse(&host_core, src.path()).await;
    eprintln!(
        "direct analysis on the host (warm worker): {:.2}s",
        local_warm.as_secs_f64()
    );
    eprintln!(
        "remote overhead (warm): {:+.2}s",
        remote_warm.as_secs_f64() - local_warm.as_secs_f64()
    );

    host_core.worker.shutdown().await;
    let _ = host.shutdown().await;
    let _ = phone.shutdown().await;
}
