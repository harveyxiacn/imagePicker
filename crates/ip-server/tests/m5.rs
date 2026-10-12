//! HTTP-level tests of the M5 routes (FakeImaging + FakeWorker + FakeRenderer).

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use ip_core::ip_render::Backend;
use ip_core::jsonfix::max_fraction_digits;
use ip_core::testutil::{
    person, write_jpeg, FakeFace, FakeImaging, FakeRenderer, FakeSpec, FakeWorker, BESTTAKE_MODEL,
    ENHANCE_MODEL, INPAINT_MODEL,
};
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
    content_type: String,
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
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    Resp {
        status,
        content_type,
        body,
    }
}

fn photo(dir: &std::path::Path, name: &str, t_s: u64) {
    let p = dir.join(name);
    write_jpeg(&p, 320, 240, (t_s % 200) as u8);
    let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
    f.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(BASE + t_s))
        .unwrap();
}

/// A bbox whose `f64` values are widened `f32`s (what a model running in `f32` reports).
fn noisy(b: [f32; 4]) -> [f64; 4] {
    b.map(|v| v as f64)
}

struct Scene {
    ids: HashMap<String, i64>,
    burst: i64,
    sid: i64,
}

/// a1 (sharp, base) a2 (burst) with persons A and B; p1 has a main subject and a bystander; c1 a
/// stranger.
async fn scene(e: &Env) -> Scene {
    for (n, t) in [
        ("a1.jpg", 0),
        ("a2.jpg", 1),
        ("p1.jpg", 3600),
        ("c1.jpg", 7200),
    ] {
        photo(e.src.path(), n, t);
    }
    let face = |b: [f32; 4], who: usize, j: f32, smile: f64| {
        FakeFace::new(noisy(b), person(who, j)).smile(smile)
    };
    let fa = [0.2, 0.3, 0.25, 0.35];
    let fb = [0.6, 0.3, 0.2, 0.3];
    let mut s1 = FakeSpec::at(0.0)
        .sharp(0.99)
        .with_faces(vec![face(fa, 0, 1.0, 0.2), face(fb, 1, 1.0, 0.5)]);
    s1.iqa = Some(1.0);
    s1.aesthetic = Some(1.0);
    let mut s2 = FakeSpec::at(2.0)
        .sharp(0.2)
        .with_faces(vec![face(fa, 0, 0.0, 0.9), face(fb, 1, 0.0, 0.5)]);
    s2.iqa = Some(0.1);
    s2.aesthetic = Some(0.1);
    e.worker.set("a1.jpg", s1);
    e.worker.set("a2.jpg", s2);
    e.worker.set(
        "p1.jpg",
        FakeSpec::at(100.0).with_faces(vec![
            face([0.3, 0.2, 0.3, 0.4], 7, 0.0, 0.7),
            face([0.8, 0.7, 0.06, 0.08], 6, 0.0, 0.7),
        ]),
    );
    e.worker.set(
        "c1.jpg",
        FakeSpec::at(200.0).with_faces(vec![face([0.4, 0.4, 0.2, 0.3], 5, 0.0, 0.7)]),
    );
    let r = call(
        &e.app,
        Method::POST,
        "/api/import",
        Some(json!({"path": e.src.path().to_string_lossy()})),
    )
    .await;
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
    for _ in 0..1200 {
        if e.core.analysis_status(sid).state != RunState::Running {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/photos?session_id={sid}"),
        None,
    )
    .await;
    let photos = r.json()["photos"].as_array().unwrap().clone();
    let ids: HashMap<String, i64> = photos
        .iter()
        .map(|p| {
            (
                p["file_name"].as_str().unwrap().to_string(),
                p["id"].as_i64().unwrap(),
            )
        })
        .collect();
    let burst = photos.iter().find(|p| p["file_name"] == "a1.jpg").unwrap()["burst_id"]
        .as_i64()
        .unwrap();
    Scene { ids, burst, sid }
}

async fn wait_event(
    rx: &mut tokio::sync::broadcast::Receiver<Event>,
    mut pred: impl FnMut(&Event) -> bool,
) -> Event {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Ok(ev) = rx.recv().await {
                if pred(&ev) {
                    return ev;
                }
            }
        }
    })
    .await
    .expect("event")
}

#[tokio::test]
async fn besttake_plan_post_and_auto_over_http() {
    let e = env();
    let s = scene(&e).await;
    let (a1, a2) = (s.ids["a1.jpg"], s.ids["a2.jpg"]);

    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/bursts/{}/besttake", s.burst),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let j = r.json();
    assert_eq!(j["base_photo_id"], a1);
    let people = j["people"].as_array().unwrap();
    assert_eq!(people.len(), 2);
    for k in [
        "track_id",
        "person_id",
        "person_name",
        "base_face_id",
        "candidates",
        "best_photo_id",
    ] {
        assert!(people[0].get(k).is_some(), "missing {k}");
    }
    for k in [
        "photo_id",
        "face_id",
        "expression_score",
        "composable",
        "reason",
    ] {
        assert!(people[0]["candidates"][0].get(k).is_some(), "missing {k}");
    }
    // why this base: a2 would need no compositing but is blurry
    let why = &j["base_choice"];
    assert_eq!(
        (why["mode"].as_str(), why["reason"].as_str()),
        (Some("auto"), Some("group_best"))
    );
    assert_eq!(why["group_best_photo_id"], a1);
    assert_eq!(why["auto_photo_id"], a1);
    let frames = why["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 2);
    for k in [
        "photo_id",
        "replacements",
        "below_best",
        "missing",
        "issues",
        "cost",
    ] {
        assert!(frames[0].get(k).is_some(), "missing {k}");
    }
    assert_eq!(frames[0]["replacements"], 1);
    assert_eq!(frames[1]["issues"], json!(["blurry"]));
    // a manual base; it must be a photo of the burst
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/bursts/{}/besttake?base_photo_id={a2}", s.burst),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["base_photo_id"], a2);
    assert_eq!(r.json()["base_choice"]["mode"], "manual");
    assert_eq!(r.json()["base_choice"]["auto_photo_id"], a1);
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/bursts/{}/besttake?base_photo_id=999999", s.burst),
        None,
    )
    .await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, "unprocessable")
    );
    assert_eq!(
        call(&e.app, Method::GET, "/api/bursts/999999/besttake", None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );

    let a1_faces = call(
        &e.app,
        Method::GET,
        &format!("/api/photos/{a1}/analysis"),
        None,
    )
    .await
    .json()["faces"]
        .clone();
    let a2_faces = call(
        &e.app,
        Method::GET,
        &format!("/api/photos/{a2}/analysis"),
        None,
    )
    .await
    .json()["faces"]
        .clone();
    let body = json!({"base_photo_id": a1, "choices": [{
        "base_face_id": a1_faces[0]["id"], "source_photo_id": a2, "source_face_id": a2_faces[0]["id"]}]});

    // validation
    assert_eq!(
        call(
            &e.app,
            Method::POST,
            "/api/besttake",
            Some(json!({"base_photo_id": a1, "choices": []}))
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
    let bad = json!({"base_photo_id": a1, "choices": [{
        "base_face_id": a2_faces[0]["id"], "source_photo_id": a2, "source_face_id": a2_faces[0]["id"]}]});
    let r = call(&e.app, Method::POST, "/api/besttake", Some(bad)).await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, "unprocessable")
    );
    // 409 / 503
    e.worker.set_missing(&[BESTTAKE_MODEL]);
    let r = call(&e.app, Method::POST, "/api/besttake", Some(body.clone())).await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::CONFLICT, "models_missing")
    );
    assert_eq!(r.json()["models"], json!([BESTTAKE_MODEL]));
    e.worker.set_missing(&[]);
    e.worker.models_unavailable.store(true, Ordering::SeqCst);
    let r = call(&e.app, Method::POST, "/api/besttake", Some(body.clone())).await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::SERVICE_UNAVAILABLE, "worker_unavailable")
    );
    e.worker.models_unavailable.store(false, Ordering::SeqCst);

    // 202 + besttake.done + the stack
    let mut rx = e.core.events.subscribe();
    let r = call(&e.app, Method::POST, "/api/besttake", Some(body)).await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    assert!(r.json()["task_id"]
        .as_str()
        .unwrap()
        .starts_with("besttake-"));
    let done = wait_event(&mut rx, |ev| matches!(ev, Event::BestTakeDone { .. })).await;
    let v = serde_json::to_value(&done).unwrap();
    assert_eq!(v["type"], "besttake.done");
    assert_eq!(v["photo_id"], a1);
    assert_eq!(v["results"][0]["ok"], true);
    assert_eq!(v["results"][0]["warnings"], json!([]));
    assert!(v["results"][0]["reason"].is_null());
    let stack = call(&e.app, Method::GET, &format!("/api/edits/{a1}"), None)
        .await
        .json()["stack"]
        .clone();
    assert_eq!(stack["ops"][0]["type"], "patch");
    assert_eq!(stack["ops"][0]["kind"], "best_take");

    // the asset is served as PNG; bad ids are rejected
    let asset = stack["ops"][0]["asset"].as_str().unwrap();
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/assets/{a1}/{asset}"),
        None,
    )
    .await;
    assert_eq!(
        (r.status, r.content_type.as_str()),
        (StatusCode::OK, "image/png")
    );
    assert_eq!(&r.body[1..4], b"PNG");
    assert_eq!(
        call(&e.app, Method::GET, &format!("/api/assets/{a1}/nope"), None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &e.app,
            Method::GET,
            &format!("/api/assets/{a2}/{asset}"),
            None
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &e.app,
            Method::GET,
            &format!("/api/assets/999999/{asset}"),
            None
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&e.app, Method::GET, &format!("/api/assets/{a1}/a.b"), None)
            .await
            .status,
        StatusCode::BAD_REQUEST
    );

    // auto: 202; replaces person A's patch (still one patch for A), B joins
    let mut rx = e.core.events.subscribe();
    let r = call(
        &e.app,
        Method::POST,
        &format!("/api/bursts/{}/besttake/auto", s.burst),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    wait_event(&mut rx, |ev| matches!(ev, Event::BestTakeDone { .. })).await;
    let stack = call(&e.app, Method::GET, &format!("/api/edits/{a1}"), None)
        .await
        .json()["stack"]
        .clone();
    let n = stack["ops"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| o["type"] == "patch")
        .count();
    assert!((1..=2).contains(&n));
    assert_eq!(
        call(
            &e.app,
            Method::POST,
            "/api/bursts/999999/besttake/auto",
            None
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    // auto onto a chosen base (optional body)
    let auto = format!("/api/bursts/{}/besttake/auto", s.burst);
    let r = call(
        &e.app,
        Method::POST,
        &auto,
        Some(json!({"base_photo_id": 999_999})),
    )
    .await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, "unprocessable")
    );
    let r = call(
        &e.app,
        Method::POST,
        &auto,
        Some(json!({"base_photo_id": "a1"})),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let mut rx = e.core.events.subscribe();
    let r = call(
        &e.app,
        Method::POST,
        &auto,
        Some(json!({"base_photo_id": a2})),
    )
    .await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    let done = wait_event(&mut rx, |ev| matches!(ev, Event::BestTakeDone { .. })).await;
    assert_eq!(serde_json::to_value(&done).unwrap()["photo_id"], a2);

    // PUT validation of patch ops
    let put = |stack: Value| {
        let app = e.app.clone();
        async move {
            call(
                &app,
                Method::PUT,
                &format!("/api/edits/{a1}"),
                Some(json!({"stack": stack})),
            )
            .await
        }
    };
    let bad_stack = json!({"version":1,"ops":[{"type":"patch","kind":"inpaint","asset":"nope","rect":[0,0,1,1]}]});
    let r = put(bad_stack).await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, "unprocessable")
    );
    let mut ok_stack = stack.clone();
    ok_stack["ops"][0]["feather"] = json!(0.9);
    assert_eq!(put(ok_stack).await.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(put(stack).await.status, StatusCode::OK);
}

#[tokio::test]
async fn inpaint_and_enhance_over_http() {
    let e = env();
    let s = scene(&e).await;
    let p1 = s.ids["p1.jpg"];

    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/photos/{p1}/bystanders"),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let faces = r.json()["faces"].clone();
    assert_eq!(faces.as_array().unwrap().len(), 1);
    assert!(faces[0]["face_id"].is_i64() && faces[0]["bbox"].as_array().unwrap().len() == 4);
    assert_eq!(
        call(&e.app, Method::GET, "/api/photos/999999/bystanders", None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );

    // validation
    for body in [
        json!({}),
        json!({"bystanders": false, "face_ids": []}),
        json!({"strokes": [{"points": [], "radius": 0.02}]}),
        json!({"strokes": [{"points": [[0.5, 0.5]], "radius": 3.0}]}),
        json!({"face_ids": [999999]}),
        json!({"bystanders": true, "model": "x"}),
    ] {
        let r = call(
            &e.app,
            Method::POST,
            &format!("/api/photos/{p1}/inpaint"),
            Some(body.clone()),
        )
        .await;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    }
    e.worker.set_missing(&[INPAINT_MODEL]);
    let r = call(
        &e.app,
        Method::POST,
        &format!("/api/photos/{p1}/inpaint"),
        Some(json!({"bystanders": true})),
    )
    .await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::CONFLICT, "models_missing")
    );
    e.worker.set_missing(&[]);

    let mut rx = e.core.events.subscribe();
    let r = call(
        &e.app,
        Method::POST,
        &format!("/api/photos/{p1}/inpaint"),
        Some(json!({"bystanders": true})),
    )
    .await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    let done = wait_event(&mut rx, |ev| matches!(ev, Event::InpaintDone { .. })).await;
    let v = serde_json::to_value(&done).unwrap();
    assert_eq!(
        (v["type"].as_str(), v["ok"].clone()),
        (Some("inpaint.done"), json!(true))
    );
    assert!(v["reason"].is_null());
    let stack = call(&e.app, Method::GET, &format!("/api/edits/{p1}"), None)
        .await
        .json()["stack"]
        .clone();
    assert_eq!(stack["ops"][0]["kind"], "inpaint");

    // a failing task still ends with inpaint.done {ok:false, reason}
    e.worker.set_mask_missing(&["person"]);
    let mut rx = e.core.events.subscribe();
    call(
        &e.app,
        Method::POST,
        &format!("/api/photos/{p1}/inpaint"),
        Some(json!({"bystanders": true})),
    )
    .await;
    let done = wait_event(&mut rx, |ev| matches!(ev, Event::InpaintDone { .. })).await;
    let v = serde_json::to_value(&done).unwrap();
    assert_eq!(v["ok"], false);
    assert!(v["reason"].is_string());

    // enhance
    let u = format!("/api/photos/{p1}/enhance");
    assert_eq!(
        call(&e.app, Method::POST, &u, Some(json!({"op": "upscale"})))
            .await
            .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        call(
            &e.app,
            Method::POST,
            &u,
            Some(json!({"op": "denoise", "strength": 2}))
        )
        .await
        .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    e.worker.set_missing(&[ENHANCE_MODEL]);
    assert_eq!(
        call(&e.app, Method::POST, &u, Some(json!({"op": "denoise"})))
            .await
            .status,
        StatusCode::CONFLICT
    );
    e.worker.set_missing(&[]);
    let mut rx = e.core.events.subscribe();
    let r = call(
        &e.app,
        Method::POST,
        &u,
        Some(json!({"op": "denoise", "strength": 0.5})),
    )
    .await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    let done = wait_event(&mut rx, |ev| matches!(ev, Event::EnhanceDone { .. })).await;
    let v = serde_json::to_value(&done).unwrap();
    assert_eq!(
        (v["type"].as_str(), v["op"].as_str(), v["ok"].clone()),
        (Some("enhance.done"), Some("denoise"), json!(true))
    );
}

#[tokio::test]
async fn worker_timeouts_are_504() {
    let e = env();
    let s = scene(&e).await;
    e.core
        .worker_timeouts
        .set("mask.generate", Some(Duration::from_millis(100)));
    e.worker.call_delay_ms.store(1000, Ordering::SeqCst);
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/masks/{}?target=subject", s.ids["a1.jpg"]),
        None,
    )
    .await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::GATEWAY_TIMEOUT, "worker_timeout")
    );
    assert_eq!(e.worker.kills.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn tasks_are_listed_and_cancelled_over_http() {
    let e = env();
    let s = scene(&e).await;
    e.worker.gen_delay_ms.store(60_000, Ordering::SeqCst);
    let mut rx = e.core.events.subscribe();
    let r = call(
        &e.app,
        Method::POST,
        &format!("/api/photos/{}/enhance", s.ids["a1.jpg"]),
        Some(json!({"op": "denoise", "strength": 0.5})),
    )
    .await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    let task = r.json()["task_id"].as_str().unwrap().to_string();
    for _ in 0..2000 {
        if e.worker.enhance_calls.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let r = call(&e.app, Method::GET, "/api/tasks?limit=5", None).await;
    assert_eq!(r.status, StatusCode::OK);
    let first = r.json()["tasks"][0].clone();
    assert_eq!(first["id"], task.as_str());
    assert_eq!(
        (&first["kind"], &first["status"], &first["cancellable"]),
        (&json!("enhance"), &json!("running"), &json!(true))
    );
    assert_eq!(first["params"]["photo_id"], s.ids["a1.jpg"]);
    // the analysis of the scene is in the history too
    let r = call(&e.app, Method::GET, "/api/tasks", None).await;
    assert!(r.json()["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["kind"] == "analysis" && t["status"] == "done"));

    let r = call(
        &e.app,
        Method::POST,
        &format!("/api/tasks/{task}/cancel"),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    assert_eq!(r.json()["task"]["status"], "cancelling");
    let done = wait_event(&mut rx, |ev| matches!(ev, Event::EnhanceDone { .. })).await;
    let v = serde_json::to_value(&done).unwrap();
    assert_eq!(
        (&v["ok"], &v["reason"]),
        (&json!(false), &json!("cancelled"))
    );
    let last = wait_event(
        &mut rx,
        |ev| matches!(ev, Event::TaskProgress { state, .. } if state != "running"),
    )
    .await;
    assert_eq!(serde_json::to_value(&last).unwrap()["state"], "cancelled");
    let r = call(&e.app, Method::GET, "/api/tasks?limit=1", None).await;
    assert_eq!(r.json()["tasks"][0]["status"], "cancelled");
    assert!(r.json()["tasks"][0]["finished_at"].is_i64());
    assert!(e.core.get_edit(s.ids["a1.jpg"]).await.unwrap().stack["ops"]
        .as_array()
        .is_none_or(|ops| ops.iter().all(|o| o["type"] != "patch")));

    // already over: 409; unknown: 404; bad limits: 400
    let r = call(
        &e.app,
        Method::POST,
        &format!("/api/tasks/{task}/cancel"),
        None,
    )
    .await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::CONFLICT, "conflict")
    );
    let r = call(
        &e.app,
        Method::POST,
        "/api/tasks/enhance-424242/cancel",
        None,
    )
    .await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::NOT_FOUND, "not_found")
    );
    for q in ["limit=0", "limit=201", "limit=x"] {
        let r = call(&e.app, Method::GET, &format!("/api/tasks?{q}"), None).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{q}");
    }
}

#[tokio::test]
async fn people_singletons_and_float_cleanup_over_http() {
    let e = env();
    let s = scene(&e).await;
    let c1 = s.ids["c1.jpg"];

    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/people?session_id={}", s.sid),
        None,
    )
    .await;
    let shown = r.json()["people"].as_array().unwrap().clone();
    assert_eq!(shown.len(), 2);
    assert!(shown.iter().all(|p| p["singleton"] == false));
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/people?session_id={}&include_singletons=1", s.sid),
        None,
    )
    .await;
    let all = r.json()["people"].as_array().unwrap().clone();
    assert_eq!(all.len(), 4);
    assert_eq!(all.iter().filter(|p| p["singleton"] == true).count(), 2);
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/photos/{c1}/people"),
        None,
    )
    .await;
    assert!(
        r.json()["people"][0]["person_id"].is_i64(),
        "singletons stay selectable"
    );

    // the stored bboxes are widened f32s: the core's own JSON has long tails, the API's has none
    let raw = serde_json::to_value(e.core.photo_analysis(c1).await.unwrap()).unwrap();
    assert!(max_fraction_digits(&raw) > 8, "{raw}");
    let burst_faces = format!("/api/bursts/{}/faces", s.burst);
    for uri in [
        format!("/api/photos/{c1}/analysis"),
        format!("/api/photos/{}/analysis", s.ids["a1.jpg"]),
        format!("/api/photos/{c1}/people"),
        burst_faces,
        format!("/api/bursts/{}/besttake", s.burst),
        format!("/api/photos/{}/bystanders", s.ids["p1.jpg"]),
        format!("/api/photos?session_id={}", s.sid),
    ] {
        let r = call(&e.app, Method::GET, &uri, None).await;
        assert_eq!(r.status, StatusCode::OK, "{uri}");
        let n = max_fraction_digits(&r.json());
        assert!(
            n <= 6,
            "{uri}: {n} fraction digits in {}",
            String::from_utf8_lossy(&r.body)
        );
    }
    // sanity: the values are still the right ones
    let j = call(
        &e.app,
        Method::GET,
        &format!("/api/photos/{c1}/analysis"),
        None,
    )
    .await
    .json();
    let b = j["faces"][0]["bbox"].as_array().unwrap();
    assert_eq!(b[0].as_f64(), Some(0.4));
    assert_eq!(b[3].as_f64(), Some(0.3));
}
