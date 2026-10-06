//! Remote AI (docs/api-contract-m8.md section C): a real host server (FakeWorker) and a "phone"
//! core whose analysis goes through `RemoteWorker` over a real loopback HTTP port.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use ip_core::auth::LanSettings;
use ip_core::fake_worker::FakeSpec;
use ip_core::testutil::{write_jpeg, FakeImaging, FakeWorker};
use ip_core::{AnalysisRunRequest, Core, CoreConfig, ImportRequest, PhotoQuery, Profile, RunState};
use ip_server::remote::RemoteOptions;
use ip_server::{serve_listener_with, RunningServer, SecurityStore, ServerOptions};
use ip_worker_client::*;
use reqwest::StatusCode;
use serde_json::{json, Value};
use tokio::net::TcpListener;

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const OWNER_PW: &str = "owner password 1";
const GUEST_PW: &str = "guest password 1";
const BASE: u64 = 1_700_000_000;

struct Host {
    server: RunningServer,
    core: Arc<Core>,
    fake: Arc<FakeWorker>,
    url: String,
    data: tempfile::TempDir,
}

fn open_core(data: &Path, worker: Arc<dyn AiWorker>) -> Arc<Core> {
    Core::open(CoreConfig {
        data_dir: Some(data.to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(2),
        worker: Some(worker),
        renderer: None,
        force_cpu: false,
    })
    .unwrap()
}

/// A host in token mode (the desktop token is the owner) with LAN login enabled.
async fn host_with(tweak: impl FnOnce(&mut ServerOptions)) -> Host {
    host_on(None, None, tweak).await
}

async fn host_on(
    listener: Option<TcpListener>,
    worker: Option<Arc<dyn AiWorker>>,
    tweak: impl FnOnce(&mut ServerOptions),
) -> Host {
    let data = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeWorker::new());
    let core = open_core(data.path(), worker.unwrap_or_else(|| fake.clone()));
    let store = SecurityStore::open(data.path());
    store.set_passwords(OWNER_PW, Some(GUEST_PW)).unwrap();
    store
        .set_lan(LanSettings {
            enabled: true,
            port: 7878,
            guest_enabled: true,
        })
        .unwrap();
    let mut opts = ServerOptions {
        session_token: Some(TOKEN.into()),
        lan: true,
        dev_cors: false,
        ..ServerOptions::default()
    };
    tweak(&mut opts);
    let listener = match listener {
        Some(l) => l,
        None => TcpListener::bind("127.0.0.1:0").await.unwrap(),
    };
    let server = serve_listener_with(listener, core.clone(), None, opts).unwrap();
    Host {
        url: server.url(),
        server,
        core,
        fake,
        data,
    }
}

async fn host() -> Host {
    host_with(|_| {}).await
}

/// The "phone": its own core + server (development mode, loopback = owner).
struct Phone {
    core: Arc<Core>,
    fake: Arc<FakeWorker>,
    url: String,
    data: tempfile::TempDir,
    _server: RunningServer,
}

async fn phone() -> Phone {
    let data = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeWorker::new());
    let core = open_core(data.path(), fake.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = serve_listener_with(
        listener,
        core.clone(),
        None,
        ServerOptions {
            dev_cors: false,
            ..ServerOptions::default()
        },
    )
    .unwrap();
    Phone {
        url: server.url(),
        core,
        fake,
        data,
        _server: server,
    }
}

fn http() -> reqwest::Client {
    ip_worker_client::remote::ensure_crypto_provider();
    reqwest::Client::builder().no_proxy().build().unwrap()
}

async fn send(rb: reqwest::RequestBuilder) -> (StatusCode, Value, reqwest::header::HeaderMap) {
    let r = rb.send().await.unwrap();
    let status = r.status();
    let headers = r.headers().clone();
    let bytes = r.bytes().await.unwrap();
    let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, v, headers)
}

fn owner(rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    rb.bearer_auth(TOKEN)
}

async fn pair_code(h: &Host) -> String {
    let (s, v, _) = send(owner(
        http().post(format!("{}/api/remote/pair/start", h.url)),
    ))
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v["code"].as_str().unwrap().to_string()
}

async fn complete(
    h: &Host,
    code: &str,
    name: &str,
) -> (StatusCode, Value, reqwest::header::HeaderMap) {
    send(
        http()
            .post(format!("{}/api/remote/pair/complete", h.url))
            .json(&json!({"code": code, "device_name": name})),
    )
    .await
}

/// Pairs a device and returns `(device_id, token)`.
async fn pair(h: &Host, name: &str) -> (String, String) {
    let code = pair_code(h).await;
    let (s, v, _) = complete(h, &code, name).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    (
        v["device_id"].as_str().unwrap().to_string(),
        v["token"].as_str().unwrap().to_string(),
    )
}

fn remote_worker(h: &Host, token: &str) -> RemoteWorker {
    let mut cfg = RemoteConfig::new(h.url.clone(), token);
    cfg.retries = 1;
    cfg.backoff_base = Duration::from_millis(20);
    RemoteWorker::new(cfg).unwrap()
}

async fn rpc(
    h: &Host,
    token: Option<&str>,
    method: &str,
    params: Value,
    files: Vec<(&str, &str, Vec<u8>)>,
) -> (StatusCode, Value, reqwest::header::HeaderMap) {
    let mut form = reqwest::multipart::Form::new()
        .text("method", method.to_string())
        .text("params", params.to_string());
    for (field, name, bytes) in files {
        form = form.part(
            field.to_string(),
            reqwest::multipart::Part::bytes(bytes).file_name(name.to_string()),
        );
    }
    let mut rb = http()
        .post(format!("{}/api/remote/rpc", h.url))
        .multipart(form);
    if let Some(t) = token {
        rb = rb.bearer_auth(t);
    }
    send(rb).await
}

async fn login(h: &Host, pw: &str) -> String {
    let r = http()
        .post(format!("{}/api/auth/login", h.url))
        .json(&json!({"password": pw}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    r.headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

fn jpeg_bytes(w: u32, h: u32) -> Vec<u8> {
    let mut v = Vec::new();
    image::RgbImage::from_pixel(w, h, image::Rgb([90, 120, 150]))
        .write_to(&mut std::io::Cursor::new(&mut v), image::ImageFormat::Jpeg)
        .unwrap();
    v
}

fn mask_params() -> Value {
    json!({
        "photo": {"photo_id": 1, "path": "file:f0", "orientation": 1},
        "targets": ["sky"], "person_bbox": null, "size": 256,
        "out_dir": "x", "allow_download": false
    })
}

fn dated_photo(dir: &Path, name: &str, t_s: u64) -> PathBuf {
    let p = dir.join(name);
    write_jpeg(&p, 320, 240, (t_s % 200) as u8);
    let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
    f.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(BASE + t_s))
        .unwrap();
    p
}

fn count_entries(dir: &Path) -> usize {
    std::fs::read_dir(dir).map(|d| d.count()).unwrap_or(0)
}

// ----------------------------------------------------------------- the loop

#[tokio::test]
async fn phone_analyses_through_the_host() {
    let h = host().await;
    let p = phone().await;
    // burst a1/a2 (similar embeddings, one second apart) and an unrelated b
    h.fake.set("a1.jpg", FakeSpec::at(0.0));
    h.fake.set("a2.jpg", FakeSpec::at(4.0));
    h.fake.set("b.jpg", FakeSpec::at(120.0));
    let src = tempfile::tempdir().unwrap();
    dated_photo(src.path(), "a1.jpg", 0);
    dated_photo(src.path(), "a2.jpg", 1);
    dated_photo(src.path(), "b.jpg", 5000);

    // ---- pairing through the phone's own API
    let code = pair_code(&h).await;
    let (s, v, _) = send(
        http()
            .post(format!("{}/api/remote/connect", p.url))
            .json(&json!({"host_url": h.url, "code": code, "device_name": "Test Phone"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["enabled"], true);
    assert_eq!(v["connected"], true, "{v}");
    assert_eq!(v["host_url"], h.url);
    assert!(v["last_error"].is_null());

    // the token is stored on the phone, hashed on the host, and never in settings responses
    let secret = std::fs::read_to_string(p.data.path().join("remote.json")).unwrap();
    let token = serde_json::from_str::<Value>(&secret).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(token.len(), 64);
    let (s, settings, _) = send(http().get(format!("{}/api/settings", p.url))).await;
    assert_eq!(s, StatusCode::OK);
    assert!(!settings.to_string().contains(&token));
    assert_eq!(settings["remote_ai"]["enabled"], true);
    assert_eq!(settings["remote_ai"]["host_url"], h.url);
    let host_security = std::fs::read_to_string(h.data.path().join("security.json")).unwrap();
    assert!(
        !host_security.contains(&token),
        "the host stores only a digest"
    );
    let (_, devs, _) = send(owner(http().get(format!("{}/api/remote/devices", h.url)))).await;
    assert_eq!(devs["devices"].as_array().unwrap().len(), 1);
    assert_eq!(devs["devices"][0]["name"], "Test Phone");

    // ---- analysis on the phone goes to the host
    let s = p
        .core
        .import(ImportRequest {
            path: src.path().to_string_lossy().into_owned(),
            recursive: true,
            title: None,
        })
        .await
        .unwrap();
    p.core
        .wait_session_ready(s.id, Duration::from_secs(30))
        .await
        .unwrap();
    p.core
        .analysis_run(AnalysisRunRequest {
            session_id: s.id,
            profile: Profile::Standard,
            photo_ids: None,
            force: false,
            allow_download: false,
        })
        .await
        .unwrap();
    let t0 = std::time::Instant::now();
    let status = loop {
        let st = p.core.analysis_status(s.id);
        if st.state != RunState::Running {
            break st;
        }
        assert!(t0.elapsed() < Duration::from_secs(60), "{st:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(status.state, RunState::Done, "{status:?}");
    assert_eq!(
        p.fake.analyze_calls.load(Ordering::SeqCst),
        0,
        "not run locally"
    );
    assert!(
        h.fake.analyze_calls.load(Ordering::SeqCst) >= 1,
        "run on the host"
    );
    assert!(
        !h.fake.last_allow_download.lock().unwrap().unwrap(),
        "a phone cannot make the host download models"
    );
    let page = p
        .core
        .photos(PhotoQuery {
            session_id: s.id,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(page.photos.len(), 3);
    assert!(page.photos.iter().all(|x| x.analyzed));
    let by = |n: &str| page.photos.iter().find(|x| x.file_name == n).unwrap();
    // grouping needs the embeddings: they travelled host -> phone cache -> ingest
    assert_eq!(by("a1.jpg").burst_id, by("a2.jpg").burst_id);
    assert_ne!(by("a1.jpg").burst_id, by("b.jpg").burst_id);
    let d = p.core.photo_analysis(by("a1.jpg").id).await.unwrap();
    assert!(d.scores.unwrap().sharpness.is_some());
    // the host cleaned its inputs; results stay only until they expire
    let remote_dir = h.data.path().join("cache").join("remote");
    for e in std::fs::read_dir(&remote_dir)
        .into_iter()
        .flatten()
        .flatten()
    {
        assert!(!e.path().join("in").exists(), "uploaded copies are deleted");
    }

    // ---- status, settings guard and disconnect
    let (_, st, _) = send(http().get(format!("{}/api/remote/status", p.url))).await;
    assert_eq!(st["connected"], true);
    assert_eq!(st["host_tier"], "T3");
    let (s2, _, _) = send(
        http()
            .patch(format!("{}/api/settings", p.url))
            .json(&json!({"remote_ai": {"host_url": "ftp://x"}})),
    )
    .await;
    assert_eq!(s2, StatusCode::UNPROCESSABLE_ENTITY);
    // pointing the setting at another host must not send the token there
    let (s2, _, _) = send(
        http()
            .patch(format!("{}/api/settings", p.url))
            .json(&json!({"remote_ai": {"host_url": "http://127.0.0.1:9"}})),
    )
    .await;
    assert_eq!(s2, StatusCode::OK);
    let (_, st, _) = send(http().get(format!("{}/api/remote/status", p.url))).await;
    assert_eq!(st["connected"], false);
    assert!(st["last_error"].as_str().unwrap().contains("another host"));
    assert!(p.core.remote.worker().is_none());

    let (s3, _, _) = send(http().delete(format!("{}/api/remote/connect", p.url))).await;
    assert_eq!(s3, StatusCode::NO_CONTENT);
    assert!(!p.data.path().join("remote.json").exists());
    let (_, st, _) = send(http().get(format!("{}/api/remote/status", p.url))).await;
    assert_eq!(st["enabled"], false);
    assert_eq!(st["connected"], false);
    h.server.shutdown().await.unwrap();
}

#[tokio::test]
async fn connect_reports_clear_errors() {
    let h = host().await;
    let p = phone().await;
    let post = |body: Value| {
        let url = format!("{}/api/remote/connect", p.url);
        async move { send(http().post(url).json(&body)).await }
    };
    let (s, v, _) = post(json!({"host_url": h.url, "code": "123456", "device_name": "x"})).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "{v}");
    assert_eq!(v["error"]["code"], "invalid_pairing_code");
    let (s, v, _) = post(json!({"host_url": "http://127.0.0.1:9", "code": "123456"})).await;
    assert_eq!(s, StatusCode::BAD_GATEWAY, "{v}");
    assert_eq!(v["error"]["code"], "host_unreachable");
    let (s, _, _) = post(json!({"host_url": "ftp://nope", "code": "123456"})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (_, st, _) = send(http().get(format!("{}/api/remote/status", p.url))).await;
    assert_eq!(st["enabled"], false);
}

// -------------------------------------------------------- artifacts round trip

#[tokio::test]
async fn masks_and_patches_round_trip() {
    let h = host().await;
    let (_, token) = pair(&h, "Pixel").await;
    let w = remote_worker(&h, &token);
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let photo = src.path().join("holiday photo.jpg");
    write_jpeg(&photo, 640, 480, 40);

    // mask.generate: PNGs land in the phone's out_dir under their own names
    let r = w
        .mask_generate(&MaskRequest {
            photo: MaskPhoto {
                photo_id: 7,
                path: photo.to_string_lossy().into_owned(),
                orientation: 1,
            },
            targets: vec!["sky".into(), "person".into()],
            person_bbox: None,
            size: 512,
            out_dir: out.path().to_string_lossy().into_owned(),
            allow_download: true,
        })
        .await
        .unwrap();
    let sky = PathBuf::from(&r.masks["sky"]);
    assert!(sky.starts_with(out.path()), "{sky:?}");
    assert_eq!(sky.file_name().unwrap(), "7_sky.png");
    let m = image::open(&sky).unwrap().to_luma8();
    assert_eq!((m.width(), m.height()), (32, 24));
    assert_eq!(r.models["sky"], "fake-seg");
    let seen = h.fake.last_mask_request.lock().unwrap().clone().unwrap();
    assert_ne!(seen.photo.path, photo.to_string_lossy(), "host paths only");
    assert!(seen.photo.path.ends_with("holiday_photo.jpg"));
    assert_eq!(seen.photo.orientation, 1);
    assert!(!seen.allow_download);
    assert_ne!(Path::new(&seen.out_dir), out.path());

    // inpaint.run: the mask is uploaded as is, the patch comes back
    let mask = out.path().join("mask.png");
    let mut g = image::GrayImage::new(64, 64);
    for y in 10..30 {
        for x in 20..40 {
            g.put_pixel(x, y, image::Luma([255]));
        }
    }
    g.save(&mask).unwrap();
    let r = w
        .inpaint_run(&InpaintRequest {
            photo: MaskPhoto {
                photo_id: 7,
                path: photo.to_string_lossy().into_owned(),
                orientation: 1,
            },
            mask: mask.to_string_lossy().into_owned(),
            model: "lama".into(),
            out_dir: out.path().join("inp").to_string_lossy().into_owned(),
            allow_download: false,
        })
        .await
        .unwrap();
    let patch = PathBuf::from(r.patch.unwrap());
    assert!(patch.starts_with(out.path().join("inp")));
    let img = image::open(&patch).unwrap().to_rgba8();
    assert_eq!((img.width(), img.height()), (16, 16));
    assert!(r.rect.is_some());
    let m = h.fake.last_inpaint_mask.lock().unwrap().clone().unwrap();
    assert_eq!((m.width(), m.height()), (64, 64));
    assert_eq!(m.get_pixel(25, 15).0[0], 255);
    assert_eq!(r.model.as_deref(), Some("lama"));

    // faces.embed has no artifacts
    h.fake.set_embed_faces(vec![(
        [0.1, 0.1, 0.2, 0.2],
        ip_core::fake_worker::unit(30.0),
    )]);
    let e = w
        .faces_embed(&FacesEmbedRequest {
            path: photo.to_string_lossy().into_owned(),
            orientation: 1,
        })
        .await
        .unwrap();
    assert_eq!(e.faces.len(), 1);

    // the remaining whitelisted methods: two uploads (best take), artifacts of every shape
    let photo2 = src.path().join("other.jpg");
    write_jpeg(&photo2, 640, 480, 90);
    let mp = |id: i64, p: &Path| MaskPhoto {
        photo_id: id,
        path: p.to_string_lossy().into_owned(),
        orientation: 1,
    };
    let c = w
        .besttake_compose(&BestTakeComposeRequest {
            base: mp(1, &photo),
            source: mp(2, &photo2),
            base_face: [0.1, 0.1, 0.2, 0.2],
            source_face: [0.1, 0.1, 0.2, 0.2],
            out_dir: out.path().join("bt").to_string_lossy().into_owned(),
            allow_download: false,
        })
        .await
        .unwrap();
    if let Some(p) = c.patch {
        assert!(Path::new(&p).is_file() && Path::new(&p).starts_with(out.path().join("bt")));
    }
    let b = w
        .beauty_prepare(&BeautyPrepareRequest {
            photo: BeautyPhoto {
                photo_id: 1,
                path: photo.to_string_lossy().into_owned(),
                orientation: 1,
            },
            faces: vec![],
            size: 512,
            out_dir: out.path().join("beauty").to_string_lossy().into_owned(),
            allow_download: false,
        })
        .await
        .unwrap();
    for person in &b.people {
        for m in [&person.skin_mask, &person.body_mask].into_iter().flatten() {
            assert!(Path::new(m).is_file(), "{m}");
        }
    }
    let en = w
        .enhance_run(&EnhanceRequest {
            photo: mp(1, &photo),
            op: "denoise".into(),
            strength: 0.5,
            scale: None,
            faces: None,
            out_dir: out.path().join("enh").to_string_lossy().into_owned(),
            allow_download: false,
        })
        .await
        .unwrap();
    let enh_patch = en.patch.expect("denoise patch");
    assert!(Path::new(&enh_patch).is_file());

    // the host keeps no uploaded copies; results live in its store
    let remote_dir = h.data.path().join("cache").join("remote");
    for e in std::fs::read_dir(&remote_dir).unwrap().flatten() {
        assert!(!e.path().join("in").exists());
    }
    // unsupported methods are refused locally, whitelisted ones need no files
    assert!(matches!(
        w.llm_plan(&LlmPlanRequest {
            message: "x".into(),
            tools: vec![],
            context: json!({}),
            locale: "en".into(),
            allow_download: false
        })
        .await,
        Err(WorkerError::Unavailable(_))
    ));
    let info = w.system_info().await.unwrap();
    assert_eq!(info.tier.as_deref(), Some("T3"));
    assert_eq!(w.status().tier.as_deref(), Some("T3"));
    assert!(!w.models_list().await.unwrap().models.is_empty());
}

/// Records what the host's worker received from `analyze.batch` before delegating.
struct Spy {
    inner: Arc<FakeWorker>,
    seen: std::sync::Mutex<Vec<(u32, u32, u8, String)>>,
}

#[async_trait::async_trait]
impl AiWorker for Spy {
    fn status(&self) -> WorkerStatus {
        self.inner.status()
    }
    fn subscribe(&self) -> tokio::sync::watch::Receiver<WorkerStatus> {
        self.inner.subscribe()
    }
    async fn system_info(&self) -> Result<SystemInfo> {
        self.inner.system_info().await
    }
    async fn models_list(&self) -> Result<ModelsListing> {
        self.inner.models_list().await
    }
    async fn models_ensure(
        &self,
        ids: &[String],
        progress: Option<ProgressTx>,
        cancel: &CancelToken,
    ) -> Result<()> {
        self.inner.models_ensure(ids, progress, cancel).await
    }
    async fn analyze_batch(
        &self,
        req: &AnalyzeRequest,
        progress: Option<ProgressTx>,
        cancel: &CancelToken,
    ) -> Result<AnalyzeResponse> {
        for it in &req.items {
            let (w, h) = image::image_dimensions(&it.path).unwrap();
            self.seen
                .lock()
                .unwrap()
                .push((w, h, it.orientation, it.path.clone()));
        }
        self.inner.analyze_batch(req, progress, cancel).await
    }
    async fn shutdown(&self) {}
}

#[tokio::test]
async fn uploads_are_downscaled_upright_copies() {
    let fake = Arc::new(FakeWorker::new());
    let spy = Arc::new(Spy {
        inner: fake,
        seen: Default::default(),
    });
    let h = host_on(None, Some(spy.clone()), |_| {}).await;
    let (_, token) = pair(&h, "Pixel").await;
    let w = remote_worker(&h, &token);
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let big = src.path().join("big.jpg");
    write_jpeg(&big, 3000, 2000, 10);
    let small = src.path().join("small.jpg");
    write_jpeg(&small, 400, 300, 20);
    let broken = src.path().join("broken.jpg");
    std::fs::write(&broken, b"not a jpeg").unwrap();
    let missing = src.path().join("gone.jpg");
    let item = |id: i64, p: &Path, o: u8| AnalyzeRequestItem {
        photo_id: id,
        path: p.to_string_lossy().into_owned(),
        orientation: o,
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let resp = w
        .analyze_batch(
            &AnalyzeRequest {
                items: vec![
                    item(1, &big, 6),
                    item(2, &small, 8),
                    item(3, &broken, 1),
                    item(4, &missing, 1),
                ],
                profile: Some("standard".into()),
                steps: None,
                analysis_size: 1024,
                out_dir: out.path().to_string_lossy().into_owned(),
                allow_download: true,
            },
            Some(tx),
            &CancelToken::new(),
        )
        .await
        .unwrap();
    let seen = spy.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    // orientation 6 turns the 3000x2000 landscape upright (2000x3000) and the long edge is 1536
    assert_eq!((seen[0].0, seen[0].1, seen[0].2), (1024, 1536, 1));
    // never upscaled; orientation 8 swaps 400x300 to 300x400
    assert_eq!((seen[1].0, seen[1].1, seen[1].2), (300, 400, 1));
    // unreadable photos become item errors, the rest is analysed
    assert_eq!(resp.items.len(), 4);
    let err = |id: i64| resp.items.iter().find(|i| i.photo_id == id).unwrap();
    assert!(err(3).error.is_some() && err(4).error.is_some());
    assert!(err(1).error.is_none() && err(1).embedding_file.is_some());
    assert!(err(1)
        .embedding_file
        .as_ref()
        .unwrap()
        .ends_with("1.emb.npy"));
    let emb = std::fs::read(err(1).embedding_file.as_ref().unwrap()).unwrap();
    assert!(emb.starts_with(b"\x93NUMPY"), "a real .npy artifact");
    assert!(Path::new(err(2).embedding_file.as_ref().unwrap()).starts_with(out.path()));
    let mut done = 0;
    while let Ok(p) = rx.try_recv() {
        done = p["done"].as_u64().unwrap_or(0);
    }
    assert_eq!(done, 4);
}

// -------------------------------------------------------------------- errors

#[tokio::test]
async fn host_errors_map_to_worker_errors() {
    let h = host().await;
    let (id, token) = pair(&h, "Pixel").await;
    let w = remote_worker(&h, &token);
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let photo = src.path().join("p.jpg");
    write_jpeg(&photo, 200, 100, 5);
    let mask_req = |allow| MaskRequest {
        photo: MaskPhoto {
            photo_id: 1,
            path: photo.to_string_lossy().into_owned(),
            orientation: 1,
        },
        targets: vec!["sky".into()],
        person_bbox: None,
        size: 128,
        out_dir: out.path().to_string_lossy().into_owned(),
        allow_download: allow,
    };

    // 409 models_missing: the host would need a download, which a phone may not trigger
    h.fake.set_mask_missing(&["birefnet"]);
    let e = w.mask_generate(&mask_req(true)).await.unwrap_err();
    assert_eq!(e.model_unavailable().unwrap(), ["birefnet"], "{e:?}");
    h.fake.set_mask_missing(&[]);
    // 503
    h.fake.mask_unavailable.store(true, Ordering::SeqCst);
    let e = w.mask_generate(&mask_req(false)).await.unwrap_err();
    assert!(matches!(e, WorkerError::Unavailable(_)), "{e:?}");
    h.fake.mask_unavailable.store(false, Ordering::SeqCst);
    // 504 from the host's per-call timeout
    h.core
        .worker_timeouts
        .set("mask.generate", Some(Duration::from_millis(100)));
    h.fake.call_delay_ms.store(600, Ordering::SeqCst);
    let e = w.mask_generate(&mask_req(false)).await.unwrap_err();
    assert!(matches!(e, WorkerError::CallTimeout { .. }), "{e:?}");
    h.fake.call_delay_ms.store(0, Ordering::SeqCst);
    // and it works again
    assert!(w.mask_generate(&mask_req(false)).await.is_ok());
    assert!(w.status().error.is_none() || w.status().state == WorkerState::Ready);

    // 401 -> unpaired once the device is revoked
    let (s, _, _) = send(owner(
        http().delete(format!("{}/api/remote/devices/{id}", h.url)),
    ))
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let e = w.system_info().await.unwrap_err();
    assert!(matches!(e, WorkerError::Unpaired(_)), "{e:?}");
    assert_eq!(w.status().state, WorkerState::Unavailable);
    assert!(w.last_error().is_some());
}

#[tokio::test]
async fn reconnects_with_backoff() {
    // nobody listens: the error comes back after the retries
    let mut cfg = RemoteConfig::new("http://127.0.0.1:9", "t");
    cfg.retries = 2;
    cfg.backoff_base = Duration::from_millis(10);
    let w = RemoteWorker::new(cfg).unwrap();
    let t = std::time::Instant::now();
    let e = w.system_info().await.unwrap_err();
    assert!(matches!(e, WorkerError::Unavailable(_)), "{e:?}");
    assert!(
        t.elapsed() >= Duration::from_millis(30),
        "retried with backoff"
    );

    // the host comes up while the client is retrying
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    let mut cfg = RemoteConfig::new(format!("http://127.0.0.1:{port}"), "t");
    cfg.retries = 8;
    cfg.backoff_base = Duration::from_millis(40);
    let w = RemoteWorker::new(cfg).unwrap();
    let call = tokio::spawn(async move { w.ping().await });
    tokio::time::sleep(Duration::from_millis(150)).await;
    let l = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    let h = host_on(Some(l), None, |_| {}).await;
    // ping is a single try; the rpc path retries: use it through a paired device
    let _ = call.await.unwrap();
    let (_, token) = pair(&h, "Pixel").await;
    let mut cfg = RemoteConfig::new(h.url.clone(), token);
    cfg.retries = 1;
    assert!(RemoteWorker::new(cfg).unwrap().ping().await.is_ok());
}

// ---------------------------------------------------------------------- auth

#[tokio::test]
async fn roles_and_endpoints_are_separated() {
    let h = host().await;
    let (id, token) = pair(&h, "Pixel").await;
    let url = |p: &str| format!("{}{p}", h.url);
    let jpg = jpeg_bytes(64, 48);
    let good = || vec![("f0", "a.jpg", jpg.clone())];

    // no token / unknown token / device endpoints reject owners and guests
    let (s, _, _) = rpc(&h, None, "system.info", json!({}), vec![]).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _, _) = rpc(&h, Some("deadbeef"), "system.info", json!({}), vec![]).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, v, _) = send(owner(http().post(url("/api/remote/rpc")))).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "owner: {v}");
    let (s, _, _) = send(owner(http().get(url("/api/remote/ping")))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = send(owner(http().get(url("/api/remote/files/abc")))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let guest = login(&h, GUEST_PW).await;
    let owner_cookie = login(&h, OWNER_PW).await;
    let (s, _, _) = send(http().get(url("/api/remote/ping")).header("cookie", &guest)).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "guest on a device endpoint");
    let (s, _, _) = send(
        http()
            .get(url("/api/remote/ping"))
            .header("cookie", &owner_cookie),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "owner session on a device endpoint"
    );
    // guests see nothing of pairing; owners with a session can manage devices
    let (s, _, _) = send(
        http()
            .get(url("/api/remote/devices"))
            .header("cookie", &guest),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = send(
        http()
            .post(url("/api/remote/pair/start"))
            .header("cookie", &guest),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = send(
        http()
            .get(url("/api/remote/devices"))
            .header("cookie", &owner_cookie),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, _) = send(http().post(url("/api/remote/pair/start"))).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // a device reaches the remote endpoints and /api/health only
    for path in [
        "/api/photos?session_id=1",
        "/api/settings",
        "/api/remote/devices",
        "/api/system/lan",
        "/api/people",
        "/api/fs/roots",
    ] {
        let (s, _, _) = send(http().get(url(path)).bearer_auth(&token)).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{path}");
    }
    let (s, _, _) = send(
        http()
            .post(url("/api/remote/pair/start"))
            .bearer_auth(&token),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = send(
        http()
            .delete(url(&format!("/api/remote/devices/{id}")))
            .bearer_auth(&token),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "a device cannot revoke");
    let (s, v, _) = send(http().get(url("/api/remote/ping")).bearer_auth(&token)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["ok"], true);
    let (s, _, _) = send(http().get(url("/api/health"))).await;
    assert_eq!(s, StatusCode::OK);

    // the rpc whitelist and the params rules
    let (s, v, _) = rpc(&h, Some(&token), "llm.plan", json!({}), vec![]).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    assert_eq!(v["error"]["code"], "method_not_allowed");
    let (s, _, _) = rpc(
        &h,
        Some(&token),
        "models.delete",
        json!({"id": "x"}),
        vec![],
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // a host path instead of `file:<field>` is never read
    let mut p = mask_params();
    p["photo"]["path"] = json!("C:\\Windows\\win.ini");
    let (s, v, _) = rpc(&h, Some(&token), "mask.generate", p, good()).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("file:"));
    assert_eq!(h.fake.mask_calls.load(Ordering::SeqCst), 0);
    let mut p = mask_params();
    p["photo"]["path"] = json!("file:nope");
    let (s, _, _) = rpc(&h, Some(&token), "mask.generate", p, good()).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let mut p = mask_params();
    p["targets"] = json!(["file:f0"]);
    let (s, _, _) = rpc(&h, Some(&token), "mask.generate", p, good()).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "file refs only in path fields");
    let (s, _, _) = rpc(
        &h,
        Some(&token),
        "mask.generate",
        mask_params(),
        vec![("../x", "a.jpg", jpg.clone())],
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "invalid field name");
    let (s, _, _) = rpc(&h, Some(&token), "mask.generate", json!([1]), vec![]).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // a path-traversing file name stays inside the request directory
    let (s, v, _) = rpc(
        &h,
        Some(&token),
        "mask.generate",
        mask_params(),
        vec![("f0", "..\\..\\evil.jpg", jpg.clone())],
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let seen = h.fake.last_mask_request.lock().unwrap().clone().unwrap();
    assert!(seen.photo.path.contains("req-"), "{}", seen.photo.path);
    assert!(!seen.photo.path.contains(".."));
    // a proper call works and its answer carries remote: ids
    let (s, v, _) = rpc(&h, Some(&token), "mask.generate", mask_params(), good()).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let rid = v["masks"]["sky"]
        .as_str()
        .unwrap()
        .strip_prefix("remote:")
        .unwrap()
        .to_string();

    // result files are bound to their device
    let (id2, token2) = pair(&h, "Tablet").await;
    let (s, _, _) = send(
        http()
            .get(url(&format!("/api/remote/files/{rid}")))
            .bearer_auth(&token2),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let r = http()
        .get(url(&format!("/api/remote/files/{rid}")))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert!(r.headers().get(FILE_NAME_HEADER_FOR_TEST).is_some());
    assert!(r.bytes().await.unwrap().starts_with(b"\x89PNG"));

    // revoking: the token stops working at once, its files go away
    let (s, _, _) = send(owner(
        http().delete(url(&format!("/api/remote/devices/{id}"))),
    ))
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _, _) = rpc(&h, Some(&token), "system.info", json!({}), vec![]).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _, _) = send(
        http()
            .get(url(&format!("/api/remote/files/{rid}")))
            .bearer_auth(&token),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _, _) = send(owner(
        http().delete(url(&format!("/api/remote/devices/{id}"))),
    ))
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (_, devs, _) = send(owner(http().get(url("/api/remote/devices")))).await;
    assert_eq!(devs["devices"].as_array().unwrap().len(), 1);
    assert_eq!(devs["devices"][0]["device_id"], id2.as_str());
    assert!(devs["devices"][0]["last_seen"].is_number());
}

const FILE_NAME_HEADER_FOR_TEST: &str = "x-file-name";

#[tokio::test]
async fn pairing_codes_are_single_use_expire_and_are_rate_limited() {
    let h = host().await;
    let code = pair_code(&h).await;
    assert_eq!(code.len(), 6);
    let (s, v, hd) = complete(&h, &code, "Phone").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["token"].as_str().unwrap().len(), 64);
    assert_eq!(hd.get("cache-control").unwrap(), "no-store");
    let (s, v, _) = complete(&h, &code, "Again").await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "single use");
    assert_eq!(v["error"]["code"], "invalid_pairing_code");
    let (s, _, _) = complete(&h, "", "Phone").await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let code = pair_code(&h).await;
    let (s, _, _) = complete(&h, &code, "  ").await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "a bad name does not burn the code"
    );
    let (s, _, _) = complete(&h, &code, "Phone 2").await;
    assert_eq!(s, StatusCode::OK);
    // start returns the deep link and QR
    let (_, v, _) = send(owner(
        http().post(format!("{}/api/remote/pair/start", h.url)),
    ))
    .await;
    assert!(v["url"]
        .as_str()
        .unwrap()
        .starts_with("imagepicker://pair?url=http%3A%2F%2F"));
    assert!(v["url"]
        .as_str()
        .unwrap()
        .ends_with(&format!("&code={}", v["code"].as_str().unwrap())));
    assert!(v["qr_svg"].as_str().unwrap().contains("<svg"));
    assert!(v["expires_at"].as_i64().unwrap() > 0);

    // expiry
    let h2 = host_with(|o| o.remote.pairing_ttl = Duration::from_millis(150)).await;
    let code = pair_code(&h2).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (s, _, _) = complete(&h2, &code, "Late").await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "expired");

    // brute force: the fourth wrong attempt of the window is locked out, even with the right code
    let h3 = host_with(|o| {
        o.login_max_failures = 3;
        o.login_window = Duration::from_secs(30);
    })
    .await;
    let code = pair_code(&h3).await;
    let wrong = if code == "000000" { "000001" } else { "000000" };
    for _ in 0..3 {
        let (s, _, _) = complete(&h3, wrong, "x").await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }
    let (s, v, hd) = complete(&h3, &code, "x").await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS, "{v}");
    assert!(hd.get("retry-after").is_some());
    assert!(v["retry_after"].as_u64().unwrap() >= 1);
    assert!(h3.core.remote.worker().is_none());

    // a code is burnt after repeated wrong guesses regardless of the source
    let h4 = host().await;
    let code = pair_code(&h4).await;
    let wrong = if code == "000000" { "000001" } else { "000000" };
    for _ in 0..5 {
        let (s, _, _) = complete(&h4, wrong, "x").await;
        assert!(s == StatusCode::UNAUTHORIZED || s == StatusCode::TOO_MANY_REQUESTS);
    }
    // LAN login disabled -> no pairing
    let h5 = host_with(|o| o.lan = false).await;
    let (s, v, _) = send(owner(
        http().post(format!("{}/api/remote/pair/start", h5.url)),
    ))
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    assert_eq!(v["error"]["code"], "lan_disabled");
}

// -------------------------------------------------------------------- limits

#[tokio::test]
async fn size_limits_concurrency_and_expiry() {
    let h = host_with(|o| {
        o.remote = RemoteOptions {
            max_upload_bytes: 200 * 1024,
            max_file_bytes: 50 * 1024,
            file_ttl: Duration::from_millis(400),
            sweep_interval: Duration::from_millis(50),
            per_device_concurrency: 1,
            ..RemoteOptions::default()
        };
    })
    .await;
    let (_, token) = pair(&h, "Pixel").await;
    let t = Some(token.as_str());
    let small = jpeg_bytes(64, 48);

    // one file over the per-file cap
    let (s, v, _) = rpc(
        &h,
        t,
        "mask.generate",
        mask_params(),
        vec![("f0", "a.jpg", vec![7u8; 80 * 1024])],
    )
    .await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE, "{v}");
    // the whole body over the request cap
    let files = (0..8)
        .map(|i| (format!("f{i}"), vec![7u8; 40 * 1024]))
        .collect::<Vec<_>>();
    let mut form = reqwest::multipart::Form::new()
        .text("method", "mask.generate")
        .text("params", mask_params().to_string());
    for (f, b) in files {
        form = form.part(f, reqwest::multipart::Part::bytes(b).file_name("x.jpg"));
    }
    let (s, _, _) = send(
        http()
            .post(format!("{}/api/remote/rpc", h.url))
            .bearer_auth(&token)
            .multipart(form),
    )
    .await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(h.fake.mask_calls.load(Ordering::SeqCst), 0);
    // nothing is left behind by rejected uploads
    let remote_dir = h.data.path().join("cache").join("remote");
    assert_eq!(count_entries(&remote_dir), 0);

    // per-device concurrency
    h.fake.call_delay_ms.store(700, Ordering::SeqCst);
    let (hc, tc, bytes) = (h.url.clone(), token.clone(), small.clone());
    let first = tokio::spawn(async move {
        let form = reqwest::multipart::Form::new()
            .text("method", "mask.generate")
            .text("params", mask_params().to_string())
            .part(
                "f0",
                reqwest::multipart::Part::bytes(bytes).file_name("a.jpg"),
            );
        send(
            http()
                .post(format!("{hc}/api/remote/rpc"))
                .bearer_auth(&tc)
                .multipart(form),
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(250)).await;
    let (s, v, _) = rpc(
        &h,
        t,
        "mask.generate",
        mask_params(),
        vec![("f0", "a.jpg", small.clone())],
    )
    .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS, "{v}");
    assert_eq!(v["error"]["code"], "device_busy");
    let (s, v, _) = first.await.unwrap();
    assert_eq!(s, StatusCode::OK, "{v}");
    h.fake.call_delay_ms.store(0, Ordering::SeqCst);

    // result files: served until they expire, then gone from the API and from the disk
    let rid = v["masks"]["sky"]
        .as_str()
        .unwrap()
        .strip_prefix("remote:")
        .unwrap()
        .to_string();
    let get = || async {
        http()
            .get(format!("{}/api/remote/files/{rid}", h.url))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .status()
    };
    assert_eq!(get().await, StatusCode::OK);
    assert!(count_entries(&remote_dir) >= 1);
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert_eq!(get().await, StatusCode::NOT_FOUND, "expired");
    assert_eq!(
        count_entries(&remote_dir),
        0,
        "the sweeper removed the files"
    );
}

#[tokio::test]
async fn stale_files_are_removed_at_startup() {
    let data = tempfile::tempdir().unwrap();
    let stale = data
        .path()
        .join("cache")
        .join("remote")
        .join("req-old")
        .join("out");
    std::fs::create_dir_all(&stale).unwrap();
    std::fs::write(stale.join("x.npy"), b"x").unwrap();
    let core = open_core(data.path(), Arc::new(FakeWorker::new()));
    let _router = ip_server::build_router_with(core, None, ServerOptions::default());
    assert!(!data
        .path()
        .join("cache")
        .join("remote")
        .join("req-old")
        .exists());
}
