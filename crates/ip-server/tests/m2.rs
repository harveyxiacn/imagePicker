//! HTTP-level tests of the M2 routes (FakeImaging + FakeWorker).

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use futures_util::StreamExt;
use http_body_util::BodyExt;
use ip_core::testutil::{person, write_jpeg, FakeFace, FakeImaging, FakeSpec, FakeWorker};
use ip_core::{Core, CoreConfig};
use ip_server::{build_router, spawn_with_core, ServerConfig};
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
    headers: axum::http::HeaderMap,
    body: Vec<u8>,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("not json ({e}): {}", String::from_utf8_lossy(&self.body)))
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
    let (status, headers) = (resp.status(), resp.headers().clone());
    let body = resp.into_body().collect().await.unwrap().to_bytes().to_vec();
    Resp {
        status,
        headers,
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

/// a1 a2 a3 (burst, two people), b1 (alone, same two people but B has closed eyes).
async fn setup(e: &Env) -> i64 {
    let d = e.src.path();
    for (n, t) in [("a1.jpg", 0), ("a2.jpg", 1), ("a3.jpg", 2), ("b1.jpg", 4000)] {
        photo(d, n, t);
    }
    let faces = |eyes_b: f64, j: f32| {
        vec![
            FakeFace::new([0.2, 0.3, 0.2, 0.3], person(0, j)),
            FakeFace::new([0.6, 0.3, 0.2, 0.3], person(1, j)).eyes(eyes_b),
        ]
    };
    e.worker.set("a1.jpg", FakeSpec::at(0.0).sharp(0.6).with_faces(faces(0.9, 1.0)));
    e.worker.set("a2.jpg", FakeSpec::at(1.0).sharp(0.9).with_faces(faces(0.9, 2.0)));
    e.worker.set("a3.jpg", FakeSpec::at(2.0).sharp(0.7).with_faces(faces(0.9, 0.0)));
    e.worker.set("b1.jpg", FakeSpec::at(120.0).sharp(0.8).with_faces(faces(0.1, 1.0)));
    let r = call(
        &e.app,
        Method::POST,
        "/api/import",
        Some(json!({"path": d.to_string_lossy()})),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let sid = r.json()["session"]["id"].as_i64().unwrap();
    e.core.wait_session_ready(sid, Duration::from_secs(30)).await.unwrap();
    sid
}

async fn analyse(e: &Env, sid: i64) {
    let r = call(
        &e.app,
        Method::POST,
        "/api/analysis/run",
        Some(json!({"session_id": sid, "profile": "standard"})),
    )
    .await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{}", String::from_utf8_lossy(&r.body));
    assert!(r.json()["task_id"].as_str().unwrap().starts_with("analysis-"));
    for _ in 0..400 {
        let s = call(&e.app, Method::GET, &format!("/api/analysis/status?session_id={sid}"), None).await;
        assert_eq!(s.status, StatusCode::OK);
        if s.json()["state"] != "running" {
            assert_eq!(s.json()["state"], "done", "{}", s.json());
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("analysis did not finish");
}

async fn photos(e: &Env, sid: i64, extra: &str) -> Vec<Value> {
    let r = call(&e.app, Method::GET, &format!("/api/photos?session_id={sid}{extra}"), None).await;
    assert_eq!(r.status, StatusCode::OK, "{}", String::from_utf8_lossy(&r.body));
    r.json()["photos"].as_array().unwrap().clone()
}

fn by_name(photos: &[Value], name: &str) -> Value {
    photos.iter().find(|p| p["file_name"] == name).unwrap().clone()
}

#[tokio::test]
async fn hardware_and_models() {
    let e = env();
    let r = call(&e.app, Method::GET, "/api/system/hardware", None).await;
    assert_eq!(r.status, StatusCode::OK);
    let w = &r.json()["worker"];
    assert_eq!(w["state"], "stopped");
    assert!(w["tier"].is_null() && w["gpu"].is_null() && w["error"].is_null());
    assert!(w["providers"].as_array().unwrap().is_empty());
    let r = call(&e.app, Method::GET, "/api/system/hardware?probe=1", None).await;
    let w = &r.json()["worker"];
    assert_eq!(w["tier"], "T3");
    assert_eq!(w["device"], "cuda");
    assert_eq!(w["gpu"]["vram_mb"], 24564);
    assert_eq!(w["state"], "ready");

    e.worker.set_missing(&["siglip2-base"]);
    let r = call(&e.app, Method::GET, "/api/models", None).await;
    let m = r.json()["models"].as_array().unwrap().clone();
    let sig = m.iter().find(|x| x["id"] == "siglip2-base").unwrap();
    assert_eq!(sig["installed"], false);
    assert_eq!(sig["required_for"], json!(["standard"]));
    assert_eq!(sig["noncommercial"], false);
    assert!(sig["size_mb"].as_f64().unwrap() > 100.0);
    assert!(sig["task"].is_array() && sig["license"].is_string());

    let r = call(&e.app, Method::POST, "/api/models/ensure", Some(json!({"ids": ["siglip2-base"]}))).await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    assert!(r.json()["task_id"].is_string());
    let r = call(&e.app, Method::POST, "/api/models/ensure", Some(json!({"ids": ["nope"]}))).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = call(&e.app, Method::POST, "/api/models/ensure", Some(json!({"ids": []}))).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn models_missing_is_a_409_with_ids() {
    let e = env();
    let sid = setup(&e).await;
    e.worker.set_missing(&["siglip2-base"]);
    let r = call(
        &e.app,
        Method::POST,
        "/api/analysis/run",
        Some(json!({"session_id": sid, "profile": "standard"})),
    )
    .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let v = r.json();
    assert_eq!(v["error"]["code"], "models_missing");
    assert!(v["error"]["message"].as_str().unwrap().contains("siglip2-base"));
    assert_eq!(v["models"], json!(["siglip2-base"]));
    // the server never asks the worker to download by itself
    assert!(e.worker.last_allow_download.lock().unwrap().is_none());
    // after the user consents the retry works
    let r = call(&e.app, Method::POST, "/api/models/ensure", Some(json!({"ids": ["siglip2-base"]}))).await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    tokio::time::sleep(Duration::from_millis(100)).await;
    analyse(&e, sid).await;
    assert_eq!(*e.worker.last_allow_download.lock().unwrap(), Some(false));
}

#[tokio::test]
async fn validation_errors() {
    let e = env();
    let sid = setup(&e).await;
    let post = |uri: &'static str, body: Value| {
        let app = e.app.clone();
        async move { call(&app, Method::POST, uri, Some(body)).await }
    };
    assert_eq!(post("/api/analysis/run", json!({"session_id": sid, "profile": "deep"})).await.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(post("/api/analysis/run", json!({"session_id": 999, "profile": "fast"})).await.status, StatusCode::NOT_FOUND);
    assert_eq!(post("/api/analysis/run", json!({"profile": "fast"})).await.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(post("/api/analysis/cancel", json!({"session_id": 999})).await.status, StatusCode::NOT_FOUND);
    assert_eq!(call(&e.app, Method::GET, "/api/analysis/status", None).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(call(&e.app, Method::GET, "/api/analysis/status?session_id=999", None).await.status, StatusCode::NOT_FOUND);
    let idle = call(&e.app, Method::GET, &format!("/api/analysis/status?session_id={sid}"), None).await.json();
    assert_eq!(idle["state"], "idle");
    assert!(idle["stage"].is_null() && idle["profile"].is_null() && idle["error"].is_null());
    assert_eq!((idle["done"].as_i64(), idle["total"].as_i64()), (Some(0), Some(0)));
    assert_eq!(call(&e.app, Method::GET, "/api/groups", None).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(call(&e.app, Method::GET, "/api/groups?session_id=999", None).await.status, StatusCode::NOT_FOUND);
    assert_eq!(call(&e.app, Method::GET, "/api/photos/999/analysis", None).await.status, StatusCode::NOT_FOUND);
    assert_eq!(call(&e.app, Method::GET, "/api/bursts/999/faces", None).await.status, StatusCode::NOT_FOUND);
    assert_eq!(call(&e.app, Method::GET, "/api/faces/999/crop", None).await.status, StatusCode::NOT_FOUND);
    assert_eq!(call(&e.app, Method::PATCH, "/api/people/999", Some(json!({"hidden": true}))).await.status, StatusCode::NOT_FOUND);
    assert_eq!(call(&e.app, Method::PATCH, "/api/people/999", Some(json!({}))).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(call(&e.app, Method::GET, "/api/people?session_id=999", None).await.status, StatusCode::NOT_FOUND);
    assert_eq!(post("/api/photos/accept-ai", json!({"ids": []})).await.status, StatusCode::BAD_REQUEST);
    for bad in [
        "&ai_rating_gte=9",
        "&issues_any=bogus",
        "&person_mode=some",
        "&person_state=sharp",
        "&persons=a,b",
        "&issues_none=maybe",
        "&faces_min=-1",
        "&sort=bogus",
    ] {
        let r = call(&e.app, Method::GET, &format!("/api/photos?session_id={sid}{bad}"), None).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}");
    }
}

#[tokio::test]
async fn full_flow_over_http() {
    let e = env();
    let sid = setup(&e).await;
    // photos carry the M2 fields before analysis
    let p = by_name(&photos(&e, sid, "").await, "a1.jpg");
    assert_eq!(p["analyzed"], false);
    assert!(p["ai_score"].is_null() && p["ai_rating"].is_null() && p["burst_id"].is_null());
    assert_eq!(p["issues"], json!([]));
    assert!(p["scene_type"].is_null() && p["face_count"].is_null() && p["rank_in_burst"].is_null());

    analyse(&e, sid).await;
    let all = photos(&e, sid, "").await;
    let a2 = by_name(&all, "a2.jpg");
    assert_eq!(a2["analyzed"], true);
    assert_eq!(a2["rank_in_burst"], 0);
    assert_eq!(a2["burst_size"], 3);
    assert_eq!(a2["face_count"], 2);
    assert_eq!(a2["subject_face_count"], 2);
    assert!(a2["ai_score"].as_f64().unwrap() > 0.0);
    assert_eq!(by_name(&all, "b1.jpg")["issues"], json!(["closed_eyes"]));
    let burst = a2["burst_id"].as_i64().unwrap();

    // analysis detail
    let r = call(&e.app, Method::GET, &format!("/api/photos/{}/analysis", by_name(&all, "b1.jpg")["id"]), None).await;
    let d = r.json();
    assert_eq!(d["analyzed"], true);
    assert_eq!(d["profile"], "standard");
    assert!(d["scores"]["sharpness"].is_number() && d["scores"]["composition"].is_null());
    assert!(d["contributions"].as_array().unwrap().iter().any(|c| c["key"] == "closed_eyes"));
    assert!(d["contributions"][0]["label_key"].as_str().unwrap().contains('.'));
    assert!(d["reasons"].as_array().unwrap().iter().any(|c| c["key"] == "closed_eyes"));
    let f = &d["faces"][0];
    for k in ["id", "photo_id", "person_id", "person_name", "bbox", "eyes_open", "smile", "gaze", "yaw", "pitch", "roll", "sharpness", "expression_score", "is_subject"] {
        assert!(f.get(k).is_some(), "face field {k}");
    }
    assert_eq!(f["bbox"].as_array().unwrap().len(), 4);

    // groups
    let g = call(&e.app, Method::GET, &format!("/api/groups?session_id={sid}"), None).await.json();
    let scenes = g["scenes"].as_array().unwrap();
    assert_eq!(scenes.len(), 2);
    let b = &scenes[0]["bursts"][0];
    assert_eq!(b["id"], burst);
    assert_eq!(b["size"], 3);
    assert_eq!(b["best_photo_id"], a2["id"]);
    assert_eq!(b["photo_ids"][0], a2["id"]);
    assert!(b["start_at"].is_number() && scenes[0]["start_at"].is_number() && scenes[0]["end_at"].is_number());

    // expression matrix
    let m = call(&e.app, Method::GET, &format!("/api/bursts/{burst}/faces"), None).await.json();
    assert_eq!(m["photo_ids"].as_array().unwrap().len(), 3);
    let tracks = m["tracks"].as_array().unwrap();
    assert_eq!(tracks.len(), 2);
    let t = &tracks[0];
    assert!(t["track_id"].is_number() && t["person_id"].is_number());
    assert_eq!(t["cells"].as_object().unwrap().len(), 3);
    assert_eq!(t["best_photo_ids"].as_array().unwrap().len(), 3);
    let any_face_id = t["cells"].as_object().unwrap().values().next().unwrap()["id"].as_i64().unwrap();

    // face crop
    for s in [128, 256] {
        let r = call(&e.app, Method::GET, &format!("/api/faces/{any_face_id}/crop?s={s}"), None).await;
        assert_eq!(r.status, StatusCode::OK);
        assert_eq!(r.headers[header::CONTENT_TYPE], "image/jpeg");
        let img = image::load_from_memory(&r.body).unwrap();
        assert_eq!((img.width(), img.height()), (s, s));
    }
    assert_eq!(call(&e.app, Method::GET, &format!("/api/faces/{any_face_id}/crop?s=99"), None).await.status, StatusCode::BAD_REQUEST);

    // people
    let ppl = call(&e.app, Method::GET, &format!("/api/people?session_id={sid}"), None).await.json();
    let list = ppl["people"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["photo_count"], 4);
    assert!(list[0]["name"].is_null() && list[0]["hidden"] == false && list[0]["cover_face_id"].is_number());
    let (pa, pb) = (list[0]["id"].as_i64().unwrap(), list[1]["id"].as_i64().unwrap());
    let r = call(&e.app, Method::PATCH, &format!("/api/people/{pa}"), Some(json!({"name": "小明", "hidden": false}))).await;
    assert_eq!(r.json()["person"]["name"], "小明");
    // filter with the new person
    let q = |extra: String| {
        let e = &e;
        async move { photos(e, sid, &extra).await.len() }
    };
    assert_eq!(q(format!("&persons={pa},{pb}")).await, 4);
    assert_eq!(q(format!("&persons={pa},{pb}&person_state=eyes_open")).await, 3);
    assert_eq!(q(format!("&persons={pb}&person_mode=any&person_state=eyes_open,smiling")).await, 3);
    assert_eq!(q(format!("&exclude_persons={pb}")).await, 0);
    assert_eq!(q("&faces_min=2".into()).await, 4);
    assert_eq!(q("&faces_max=0".into()).await, 0);
    assert_eq!(q("&issues_none=1".into()).await, 3);
    assert_eq!(q("&issues_any=closed_eyes,blurry".into()).await, 1);
    assert_eq!(q("&burst_best_only=1".into()).await, 2);
    assert_eq!(q(format!("&burst_id={burst}")).await, 3);
    assert_eq!(q("&include_background=true&persons=".to_string() + &pa.to_string()).await, 4);
    let sorted = photos(&e, sid, "&sort=ai").await;
    let sc: Vec<f64> = sorted.iter().map(|p| p["ai_score"].as_f64().unwrap()).collect();
    assert!(sc.windows(2).all(|w| w[0] >= w[1]));

    // correct a face / merge people
    let r = call(&e.app, Method::POST, &format!("/api/faces/{any_face_id}/person"), Some(json!({"person_id": null}))).await;
    assert_eq!(r.status, StatusCode::OK);
    let nf = r.json()["face"].clone();
    assert!(nf["person_id"].is_number() && nf["id"] == any_face_id);
    let r = call(&e.app, Method::POST, "/api/people/merge", Some(json!({"ids": [pb], "into": pa}))).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["person"]["id"], pa);

    // split / merge
    let r = call(&e.app, Method::POST, "/api/groups/split", Some(json!({"burst_id": burst, "at_photo_id": by_name(&all, "a3.jpg")["id"]}))).await;
    assert_eq!(r.status, StatusCode::OK, "{}", String::from_utf8_lossy(&r.body));
    let ids = r.json()["burst_ids"].as_array().unwrap().clone();
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], burst);
    let r = call(&e.app, Method::POST, "/api/groups/merge", Some(json!({"burst_ids": ids}))).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["burst_id"], burst);
    assert_eq!(call(&e.app, Method::POST, "/api/groups/merge", Some(json!({"burst_ids": [burst]}))).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(call(&e.app, Method::POST, "/api/groups/split", Some(json!({"burst_id": burst, "at_photo_id": 424242}))).await.status, StatusCode::BAD_REQUEST);

    // accept AI
    let ids = vec![by_name(&all, "a2.jpg")["id"].clone(), by_name(&all, "b1.jpg")["id"].clone()];
    let r = call(&e.app, Method::POST, "/api/photos/accept-ai", Some(json!({ "ids": ids }))).await;
    assert_eq!(r.json()["updated"], 2);
    let after = photos(&e, sid, "").await;
    let a2 = by_name(&after, "a2.jpg");
    assert_eq!(a2["user_rating"].as_f64().unwrap(), a2["ai_rating"].as_f64().unwrap().round());
}

#[tokio::test]
async fn cancel_and_events_over_websocket() {
    let e = env();
    let sid = setup(&e).await;
    let server = spawn_with_core(
        e.core.clone(),
        &ServerConfig {
            host: "127.0.0.1".into(),
            port: 0,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/api/events", server.addr))
        .await
        .unwrap();
    e.worker.delay_ms.store(100, std::sync::atomic::Ordering::SeqCst);
    let r = call(
        &server_app(&e),
        Method::POST,
        "/api/analysis/run",
        Some(json!({"session_id": sid, "profile": "fast"})),
    )
    .await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    let mut types = std::collections::HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut done_seen = false;
    while tokio::time::Instant::now() < deadline && !(done_seen && types.contains("groups.updated") && types.contains("people.updated")) {
        let Ok(Some(Ok(msg))) = tokio::time::timeout(Duration::from_secs(5), ws.next()).await else {
            break;
        };
        if let tokio_tungstenite::tungstenite::Message::Text(t) = msg {
            let v: Value = serde_json::from_str(t.as_str()).unwrap();
            let ty = v["type"].as_str().unwrap().to_string();
            if ty == "analysis.progress" {
                assert_eq!(v["session_id"], sid);
                assert!(v.get("stage").is_some() && v.get("done").is_some() && v.get("total").is_some());
                if v["state"] == "done" {
                    done_seen = true;
                    assert!(v["stage"].is_null());
                }
            }
            if ty == "analysis.updated" {
                assert!(v["ids"].is_array());
            }
            types.insert(ty);
        }
    }
    for t in ["analysis.progress", "analysis.updated", "groups.updated", "people.updated", "worker.status"] {
        assert!(types.contains(t), "missing event {t}: {types:?}");
    }
    assert!(done_seen);

    // cancel a long run over HTTP
    e.worker.delay_ms.store(5_000, std::sync::atomic::Ordering::SeqCst);
    let r = call(
        &server_app(&e),
        Method::POST,
        "/api/analysis/run",
        Some(json!({"session_id": sid, "profile": "standard", "force": true})),
    )
    .await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let again = call(&server_app(&e), Method::POST, "/api/analysis/run", Some(json!({"session_id": sid, "profile": "fast"}))).await;
    assert_eq!(again.status, StatusCode::CONFLICT);
    assert_eq!(again.json()["error"]["code"], "conflict");
    let r = call(&server_app(&e), Method::POST, "/api/analysis/cancel", Some(json!({"session_id": sid}))).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    for _ in 0..200 {
        let s = call(&server_app(&e), Method::GET, &format!("/api/analysis/status?session_id={sid}"), None).await.json();
        if s["state"] != "running" {
            assert_eq!(s["state"], "idle");
            server.shutdown().await.unwrap();
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("cancel did not take effect");
}

fn server_app(e: &Env) -> Router {
    e.app.clone()
}
