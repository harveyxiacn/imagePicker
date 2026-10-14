//! HTTP-level tests of the device clock offsets (docs/api-contract-m2.md, docs/09 P1-2).
//! FakeImaging + FakeWorker, synthetic images only.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use ip_core::testutil::{write_jpeg, FakeImaging, FakeSpec, FakeWorker};
use ip_core::{Core, CoreConfig};
use ip_server::build_router;
use serde_json::{json, Value};
use tower::ServiceExt;

/// 2023-11-14T22:13:20Z
const BASE: u64 = 1_700_000_000;
/// The phone's clock runs 2 h 3 min 7 s fast.
const ERR_S: u64 = 2 * 3_600 + 3 * 60 + 7;

struct Env {
    worker: Arc<FakeWorker>,
    app: Router,
    dir: tempfile::TempDir,
    _data: tempfile::TempDir,
}

fn env() -> Env {
    let data = tempfile::tempdir().unwrap();
    let worker = Arc::new(FakeWorker::new());
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(2),
        worker: Some(worker.clone()),
        renderer: None,
        force_cpu: false,
    })
    .unwrap();
    Env {
        worker,
        app: build_router(core, None),
        dir: tempfile::tempdir().unwrap(),
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

fn photo(dir: &std::path::Path, name: &str, t_s: u64, shade: u8) {
    let p = dir.join(name);
    write_jpeg(&p, 320, 240, shade);
    let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
    f.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(BASE + t_s))
        .unwrap();
}

/// Camera `cam_*` (7 photos) and phone `phone_*` (6): the same scenes, the phone's clock fast.
async fn session(e: &Env) -> i64 {
    for k in 0..6u64 {
        let angle = 50.0 * k as f32;
        let (cam, phone) = (format!("cam_{k}.jpg"), format!("phone_{k}.jpg"));
        photo(e.dir.path(), &cam, 900 * k, (k * 12) as u8);
        photo(
            e.dir.path(),
            &phone,
            900 * k + k % 3 + ERR_S,
            (k * 12 + 6) as u8,
        );
        e.worker.set(&cam, FakeSpec::at(angle));
        e.worker.set(&phone, FakeSpec::at(angle));
    }
    photo(e.dir.path(), "cam_x.jpg", 1, 1);
    e.worker.set("cam_x.jpg", FakeSpec::at(0.0));
    let r = call(
        &e.app,
        Method::POST,
        "/api/import",
        Some(json!({"path": e.dir.path().to_string_lossy()})),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let sid = r.json()["session"]["id"].as_i64().unwrap();
    for _ in 0..600 {
        let s = call(&e.app, Method::GET, &format!("/api/sessions/{sid}"), None).await;
        if s.json()["session"]["import_state"] == "ready" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let r = call(
        &e.app,
        Method::POST,
        "/api/analysis/run",
        Some(json!({"session_id": sid, "profile": "standard"})),
    )
    .await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    let t = Instant::now();
    loop {
        let s = call(
            &e.app,
            Method::GET,
            &format!("/api/analysis/status?session_id={sid}"),
            None,
        )
        .await
        .json();
        if s["state"] != "running" {
            assert_eq!(s["state"], "done", "{s}");
            return sid;
        }
        assert!(t.elapsed() < Duration::from_secs(300), "analysis timed out");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn burst_count(e: &Env, sid: i64) -> usize {
    let g = call(
        &e.app,
        Method::GET,
        &format!("/api/groups?session_id={sid}"),
        None,
    )
    .await
    .json();
    g["scenes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["bursts"].as_array().unwrap().len())
        .sum()
}

fn phone(v: &Value) -> &Value {
    v["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"].as_str().unwrap().starts_with("Apple"))
        .unwrap()
}

#[tokio::test]
async fn offsets_estimate_accept_set_reset() {
    let e = env();
    let sid = session(&e).await;
    let url = format!("/api/sessions/{sid}/devices/offsets");
    assert_eq!(burst_count(&e, sid).await, 12, "two clocks, nothing shared");

    let r = call(&e.app, Method::GET, &url, None).await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let v = r.json();
    assert_eq!(v["devices"].as_array().unwrap().len(), 2);
    assert!(v["estimated_at"].is_number());
    for d in v["devices"].as_array().unwrap() {
        for k in [
            "device_id",
            "name",
            "kind",
            "photo_count",
            "offset_ms",
            "source",
            "estimate",
        ] {
            assert!(d.get(k).is_some(), "device field {k}: {d}");
        }
    }
    let p = phone(&v);
    assert_eq!(p["source"], "none");
    assert_eq!(p["offset_ms"], 0);
    let est = &p["estimate"];
    assert_eq!(est["source"], "visual");
    let off = est["offset_ms"].as_i64().unwrap();
    assert!((off + ERR_S as i64 * 1000).abs() <= 5_000, "{est}");
    assert!(est["confidence"].as_f64().unwrap() >= 0.5);
    assert!(est["evidence"].as_i64().unwrap() >= 5);
    assert!(v["reference_device_id"].is_number());
    let phone_id = p["device_id"].as_i64().unwrap();

    // refresh=1 estimates again
    let r = call(&e.app, Method::GET, &format!("{url}?refresh=1"), None).await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&r.body)
    );

    // accept
    let r = call(&e.app, Method::PUT, &url, Some(json!({"action": "accept"}))).await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let v = r.json();
    assert_eq!(phone(&v)["source"], "auto");
    assert_eq!(phone(&v)["offset_ms"], off);
    assert_eq!(burst_count(&e, sid).await, 6, "grouped across devices");

    // reset one device
    let r = call(
        &e.app,
        Method::PUT,
        &url,
        Some(json!({"action": "reset", "device_id": phone_id})),
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    assert_eq!(phone(&r.json())["offset_ms"], 0);
    assert_eq!(burst_count(&e, sid).await, 12);

    // type an offset in
    let typed = -(ERR_S as i64) * 1000;
    let r = call(
        &e.app,
        Method::PUT,
        &url,
        Some(json!({"action": "set", "device_id": phone_id, "offset_ms": typed})),
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let v = r.json();
    assert_eq!(phone(&v)["source"], "manual");
    assert_eq!(phone(&v)["offset_ms"], typed);
    assert_eq!(burst_count(&e, sid).await, 6);

    // reset all
    let r = call(&e.app, Method::PUT, &url, Some(json!({"action": "reset"}))).await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    assert_eq!(burst_count(&e, sid).await, 12);
}

#[tokio::test]
async fn offsets_errors() {
    let e = env();
    let sid = session(&e).await;
    let url = format!("/api/sessions/{sid}/devices/offsets");
    let put = |b: Value| call(&e.app, Method::PUT, &url, Some(b));

    let r = put(json!({"action": "set", "offset_ms": 5})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = put(json!({"action": "set", "device_id": 1})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = put(json!({"action": "set", "device_id": 1, "offset_ms": 99_999_999_999_i64})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.json()["error"]["code"], "bad_request");
    let r = put(json!({"action": "set", "device_id": 424242, "offset_ms": 5})).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = put(json!({"action": "explode"})).await;
    assert!(r.status.is_client_error(), "{:?}", r.status);
    let r = call(&e.app, Method::PUT, &url, None).await;
    assert!(r.status.is_client_error(), "{:?}", r.status);

    let r = call(
        &e.app,
        Method::GET,
        "/api/sessions/999/devices/offsets",
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.json()["error"]["code"], "not_found");
    let r = call(
        &e.app,
        Method::GET,
        "/api/sessions/abc/devices/offsets",
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = call(
        &e.app,
        Method::PUT,
        "/api/sessions/999/devices/offsets",
        Some(json!({"action": "reset"})),
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}
