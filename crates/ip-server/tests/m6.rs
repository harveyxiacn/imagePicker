//! HTTP-level tests of the M6 routes (assistant, settings, cache, models, onboarding, faces, XMP).

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use ip_core::ip_render::Backend;
use ip_core::testutil::{write_jpeg, FakeImaging, FakeRenderer, FakeSpec, FakeWorker};
use ip_core::{AnalysisRunRequest, Core, CoreConfig, Event, Profile, RunState};
use ip_server::build_router;
use serde_json::{json, Value};
use tower::ServiceExt;

const BASE: u64 = 1_700_000_000;

struct Env {
    core: Arc<Core>,
    worker: Arc<FakeWorker>,
    app: Router,
    src: tempfile::TempDir,
    _data: tempfile::TempDir,
}

fn env() -> Env {
    let data = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let worker = Arc::new(FakeWorker::new());
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(2),
        worker: Some(worker.clone()),
        renderer: Some(Arc::new(FakeRenderer::new(Backend::Cpu))),
        force_cpu: true,
    })
    .unwrap();
    let app = build_router(core.clone(), None);
    Env {
        core,
        worker,
        app,
        src,
        _data: data,
    }
}

struct Resp {
    status: StatusCode,
    body: Vec<u8>,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("not json ({e}): {}", String::from_utf8_lossy(&self.body)))
    }
    fn code(&self) -> String {
        self.json()["error"]["code"].as_str().unwrap_or("").into()
    }
}

async fn call(app: &Router, method: Method, uri: &str, body: Option<Value>) -> Resp {
    let b = Request::builder().method(method).uri(uri);
    let req = match body {
        Some(v) => b
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    Resp { status, body }
}

async fn session(e: &Env) -> i64 {
    for (n, t) in [("a1.jpg", 0), ("a2.jpg", 1), ("c1.jpg", 4000)] {
        let p = e.src.path().join(n);
        write_jpeg(&p, 320, 240, (t % 200) as u8);
        let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
        f.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(BASE + t))
            .unwrap();
        e.worker.set(n, FakeSpec::at(0.0));
    }
    let r = call(
        &e.app,
        Method::POST,
        "/api/import",
        Some(json!({"path": e.src.path().to_string_lossy()})),
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let sid = r.json()["session"]["id"].as_i64().unwrap();
    e.core
        .wait_session_ready(sid, Duration::from_secs(30))
        .await
        .unwrap();
    e.core
        .analysis_run(AnalysisRunRequest {
            session_id: sid,
            profile: Profile::Standard,
            photo_ids: None,
            force: false,
            allow_download: false,
        })
        .await
        .unwrap();
    for _ in 0..400 {
        if e.core.analysis_status(sid).state != RunState::Running {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    sid
}

#[tokio::test]
async fn settings_get_patch_and_security_store() {
    let e = env();
    let r = call(&e.app, Method::GET, "/api/settings", None).await;
    assert_eq!(r.status, StatusCode::OK);
    let s = r.json();
    // the bare object, with the security-store parts, never any password material
    assert_eq!(s["language"], "zh-CN");
    assert_eq!(s["lan"]["port"], 7878);
    assert_eq!(s["lan"]["enabled"], false);
    assert!(s["roots"].is_array());
    let text = String::from_utf8_lossy(&r.body).to_lowercase();
    assert!(!text.contains("hash") && !text.contains("password"));

    let mut rx = e.core.events.subscribe();
    let r = call(
        &e.app,
        Method::PATCH,
        "/api/settings",
        Some(json!({"theme":"dark","faces":{"enabled":false},"cache":{"max_gb":5}})),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let s = r.json();
    assert_eq!(s["theme"], "dark");
    assert_eq!(s["faces"]["enabled"], false);
    assert_eq!(s["cache"]["max_gb"], 5.0);
    assert_eq!(s["lan"]["guest_enabled"], false);
    let mut seen = false;
    while let Ok(ev) = rx.try_recv() {
        if let Event::SettingsUpdated { settings } = ev {
            assert_eq!(settings["theme"], "dark");
            assert!(settings["lan"].is_object());
            seen = true;
        }
    }
    assert!(seen, "settings.updated");
    assert_eq!(
        call(&e.app, Method::GET, "/api/settings", None)
            .await
            .json()["theme"],
        "dark"
    );

    // validation
    let bad = call(
        &e.app,
        Method::PATCH,
        "/api/settings",
        Some(json!({"theme":"neon"})),
    )
    .await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    let bad = call(
        &e.app,
        Method::PATCH,
        "/api/settings",
        Some(json!({"nope":1})),
    )
    .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    let bad = call(&e.app, Method::PATCH, "/api/settings", Some(json!([1]))).await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);

    // LAN needs an owner password (422), a port of 0 is refused, nothing changed
    let r = call(
        &e.app,
        Method::PATCH,
        "/api/settings",
        Some(json!({"lan":{"enabled":true}})),
    )
    .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    let r = call(
        &e.app,
        Method::PATCH,
        "/api/settings",
        Some(json!({"lan":{"port":0}})),
    )
    .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        call(&e.app, Method::GET, "/api/settings", None)
            .await
            .json()["lan"]["enabled"],
        false
    );
    // port and guest flag are accepted without enabling LAN; roots are stored in the security store
    let dir = e.src.path().to_string_lossy().into_owned();
    let r = call(
        &e.app,
        Method::PATCH,
        "/api/settings",
        Some(json!({"lan":{"port":9000,"guest_enabled":true},"roots":[dir]})),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let s = r.json();
    assert_eq!(s["lan"]["port"], 9000);
    assert_eq!(s["lan"]["guest_enabled"], true);
    assert_eq!(s["roots"], json!([dir]));

    // the models directory is a client path: the whitelist applies
    let outside = if cfg!(windows) {
        "C:\\Windows\\Temp\\ip-models"
    } else {
        "/etc/ip-models"
    };
    let r = call(
        &e.app,
        Method::PATCH,
        "/api/settings",
        Some(json!({"models":{"dir":outside}})),
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(r.code(), "forbidden_path");
    let inside = e.src.path().join("models").to_string_lossy().into_owned();
    let r = call(
        &e.app,
        Method::PATCH,
        "/api/settings",
        Some(json!({"models":{"dir":inside}})),
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
}

#[tokio::test]
async fn assistant_over_http() {
    let e = env();
    let sid = session(&e).await;
    let r = call(&e.app, Method::GET, "/api/assistant/status", None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["engine"], "rules");
    assert_eq!(r.json()["llm_available"], false);

    let r = call(
        &e.app,
        Method::POST,
        "/api/assistant/plan",
        Some(json!({
            "session_id": sid,
            "message": "每个场景只留1张，其他淘汰",
            "context": {"filter": "", "selection": [], "current_photo_id": null, "locale": "zh-CN"},
            "engine": "auto"
        })),
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let plan = r.json();
    assert_eq!(plan["engine"], "rules");
    assert_eq!(plan["needs_confirmation"], true);
    assert!(plan["unsupported"].is_null());
    let step = &plan["steps"][0];
    assert_eq!(step["tool"], "scene_keep_top");
    assert_eq!(step["destructive"], true);
    assert_eq!(step["affects"], 3);
    assert!(step["summary"].as_str().unwrap().contains("场景"));

    let mut rx = e.core.events.subscribe();
    let pid = plan["plan_id"].as_str().unwrap().to_string();
    let r = call(
        &e.app,
        Method::POST,
        "/api/assistant/execute",
        Some(json!({"plan_id": pid})),
    )
    .await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    assert!(r.json()["task_id"]
        .as_str()
        .unwrap()
        .starts_with("assistant-"));
    let done = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Ok(ev) = rx.recv().await {
                if let Event::AssistantDone { .. } = ev {
                    return serde_json::to_value(&ev).unwrap();
                }
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(done["type"], "assistant.done");
    assert_eq!(done["plan_id"], pid);
    assert_eq!(done["ok"], true);
    assert_eq!(done["results"][0]["affected"], 3);
    assert_eq!(done["undo"]["photos"].as_array().unwrap().len(), 3);

    // unknown / used plans
    let r = call(
        &e.app,
        Method::POST,
        "/api/assistant/execute",
        Some(json!({"plan_id": pid})),
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    // unsupported messages are a 200 with a reason
    let r = call(
        &e.app,
        Method::POST,
        "/api/assistant/plan",
        Some(json!({"session_id": sid, "message": "sing me a song"})),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.json()["unsupported"].as_str().unwrap().contains("Try"));
    assert_eq!(r.json()["steps"], json!([]));
    // describe without a VLM
    let r = call(
        &e.app,
        Method::POST,
        "/api/assistant/describe",
        Some(json!({"photo_id": 1})),
    )
    .await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(r.code(), "worker_unavailable");
    let r = call(
        &e.app,
        Method::POST,
        "/api/assistant/plan",
        Some(json!({"session_id": 999, "message": "x"})),
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    // a missing VLM model is a 409
    e.worker.enable_vlm("c", &["k"], &[], json!({}), "r");
    e.worker.set_missing(&[ip_core::testutil::VLM_MODEL]);
    let r = call(
        &e.app,
        Method::POST,
        "/api/assistant/describe",
        Some(json!({"photo_id": 1})),
    )
    .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.code(), "models_missing");
}

#[tokio::test]
async fn cache_models_onboarding_faces_and_xmp_routes() {
    let e = env();
    let sid = session(&e).await;
    let r = call(&e.app, Method::GET, "/api/cache", None).await;
    assert_eq!(r.status, StatusCode::OK);
    let c = r.json();
    assert!(c["bytes"].as_u64().unwrap() > 0, "thumbnails exist");
    assert!(c["items"]["thumbs"].as_u64().unwrap() >= 3);
    for k in ["thumbs", "previews", "masks", "edits", "gen"] {
        assert!(c["items"][k].is_u64(), "{k}");
    }
    let r = call(
        &e.app,
        Method::POST,
        "/api/cache/clear",
        Some(json!({"kinds":["thumbs"]})),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["cache"]["items"]["thumbs"], 0);
    let r = call(
        &e.app,
        Method::POST,
        "/api/cache/clear",
        Some(json!({"kinds":["nope"]})),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // models
    let r = call(&e.app, Method::DELETE, "/api/models/siglip2-base", None).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let r = call(
        &e.app,
        Method::DELETE,
        "/api/models/never-heard-of-it",
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = call(&e.app, Method::DELETE, "/api/models/.hidden", None).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // onboarding
    let r = call(&e.app, Method::GET, "/api/onboarding", None).await;
    assert_eq!(r.status, StatusCode::OK);
    let o = r.json();
    assert_eq!(o["first_run"], true);
    assert!(o["recommended_tier"].is_string());
    assert_eq!(o["recommended_models"], json!(["siglip2-base"]));
    assert_eq!(
        call(&e.app, Method::POST, "/api/onboarding/done", None)
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(&e.app, Method::GET, "/api/onboarding", None)
            .await
            .json()["first_run"],
        false
    );

    // faces
    let r = call(&e.app, Method::DELETE, "/api/faces", None).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = call(&e.app, Method::DELETE, "/api/faces?confirm=true", None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.json()["faces"].is_i64());

    // xmp
    let r = call(
        &e.app,
        Method::POST,
        "/api/xmp/sync",
        Some(json!({"session_id": sid, "direction": "read"})),
    )
    .await;
    assert_eq!(r.status, StatusCode::CONFLICT, "xmp_mode is off");
    call(
        &e.app,
        Method::PATCH,
        "/api/settings",
        Some(json!({"xmp_mode":"sidecar"})),
    )
    .await;
    let r = call(
        &e.app,
        Method::POST,
        "/api/xmp/sync",
        Some(json!({"session_id": sid, "direction": "write"})),
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    assert_eq!(r.json()["written"], 3);
    assert!(e.src.path().join("a1.xmp").exists());
    let r = call(
        &e.app,
        Method::POST,
        "/api/xmp/sync",
        Some(json!({"session_id": sid, "direction": "up"})),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // keywords
    let photos = call(
        &e.app,
        Method::GET,
        &format!("/api/photos?session_id={sid}"),
        None,
    )
    .await
    .json();
    let id = photos["photos"][0]["id"].as_i64().unwrap();
    let r = call(
        &e.app,
        Method::POST,
        "/api/photos/tags",
        Some(json!({"ids":[id],"add":["beach"]})),
    )
    .await;
    assert_eq!(r.json()["updated"], 1);
    let r = call(&e.app, Method::GET, &format!("/api/photos/{id}/tags"), None).await;
    assert_eq!(r.json()["tags"], json!(["beach"]));
}
