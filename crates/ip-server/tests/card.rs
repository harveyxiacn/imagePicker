//! PRD F1.8 over HTTP: card listing and `POST /api/import/card` validation and a full run
//! against a fake card (a DCIM tree in a temp dir).

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use ip_core::testutil::{write_jpeg, FakeImaging};
use ip_core::{Core, CoreConfig};
use ip_server::build_router;
use serde_json::{json, Value};
use tower::ServiceExt;

struct Env {
    core: Arc<Core>,
    app: Router,
    card: tempfile::TempDir,
    dest: tempfile::TempDir,
    _data: tempfile::TempDir,
}

fn env() -> Env {
    let data = tempfile::tempdir().unwrap();
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(2),
        worker: None,
        renderer: None,
        force_cpu: false,
    })
    .unwrap();
    let app = build_router(core.clone(), None);
    let card = tempfile::tempdir().unwrap();
    write_jpeg(&card.path().join("DCIM/100CANON/IMG_0001.JPG"), 32, 32, 10);
    write_jpeg(&card.path().join("DCIM/100CANON/IMG_0002.JPG"), 32, 32, 80);
    std::fs::write(
        card.path().join("DCIM/100CANON/IMG_0001.CR2"),
        vec![3u8; 2048],
    )
    .unwrap();
    Env {
        core,
        app,
        card,
        dest: tempfile::tempdir().unwrap(),
        _data: data,
    }
}

async fn call(app: &Router, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
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
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn body(e: &Env) -> Value {
    json!({
        "source": e.card.path().to_string_lossy(),
        "dest": e.dest.path().to_string_lossy(),
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn sources_lists_cards_without_importing_anything() {
    let e = env();
    let (status, v) = call(&e.app, Method::GET, "/api/import/sources", None).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["default_pattern"], "{yyyy}/{yyyy-mm-dd}");
    let list = v["sources"].as_array().expect("sources is an array");
    for s in list {
        // whatever is mounted on the machine running the test: the shape is fixed
        assert!(s["label"].is_string() && s["path"].is_string());
        assert!(s["photo_estimate"].is_u64() && s["estimate_truncated"].is_boolean());
        assert!(s.get("kind").is_some() && s.get("free_bytes").is_some());
    }
    let (_, sessions) = call(&e.app, Method::GET, "/api/sessions", None).await;
    assert!(sessions["sessions"].as_array().unwrap().is_empty());
    let (_, tasks) = call(&e.app, Method::GET, "/api/tasks", None).await;
    assert!(tasks["tasks"].as_array().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn card_import_is_validated() {
    let e = env();
    let mut cases: Vec<(&str, Value, StatusCode, &str)> = Vec::new();
    let mut b = body(&e);
    b["pattern"] = json!("../escape");
    cases.push(("pattern", b, StatusCode::BAD_REQUEST, "bad_request"));
    let mut b = body(&e);
    b["source"] = json!(e.dest.path().to_string_lossy()); // a folder without DCIM
    cases.push(("no DCIM", b, StatusCode::BAD_REQUEST, "bad_request"));
    let mut b = body(&e);
    b["source"] = json!("");
    cases.push(("empty source", b, StatusCode::BAD_REQUEST, "bad_request"));
    let mut b = body(&e);
    b["dest"] = json!("");
    cases.push(("empty dest", b, StatusCode::BAD_REQUEST, "bad_request"));
    let mut b = body(&e);
    b["dest"] = json!("relative/dir");
    cases.push(("relative dest", b, StatusCode::BAD_REQUEST, "bad_request"));
    let outside = if cfg!(windows) { "C:\\Windows" } else { "/etc" };
    let mut b = body(&e);
    b["dest"] = json!(outside);
    cases.push((
        "dest outside the roots",
        b,
        StatusCode::FORBIDDEN,
        "forbidden_path",
    ));
    let mut b = body(&e);
    b["source"] = json!(outside);
    cases.push((
        "source outside the roots",
        b,
        StatusCode::FORBIDDEN,
        "forbidden_path",
    ));
    let mut b = body(&e);
    b["dest"] = json!(format!("{}/..", e.dest.path().display()));
    cases.push(("traversal", b, StatusCode::FORBIDDEN, "forbidden_path"));
    let mut b = body(&e);
    b["dest"] = json!(e.card.path().join("DCIM").to_string_lossy());
    cases.push((
        "dest on the card",
        b,
        StatusCode::BAD_REQUEST,
        "bad_request",
    ));
    for (what, b, status, code) in cases {
        let (s, v) = call(&e.app, Method::POST, "/api/import/card", Some(b)).await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (status, Some(code)),
            "{what}: {v}"
        );
    }
    assert!(!e.dest.path().join("2024").exists());
    let (_, tasks) = call(&e.app, Method::GET, "/api/tasks", None).await;
    assert!(tasks["tasks"].as_array().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn card_import_copies_and_creates_a_session() {
    let e = env();
    let mut b = body(&e);
    b["pattern"] = json!("card/{yyyy}");
    b["title"] = json!("Fake card");
    let (status, v) = call(&e.app, Method::POST, "/api/import/card", Some(b)).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{v}");
    let id = v["task_id"].as_str().unwrap().to_string();
    assert!(id.starts_with("card_import-"));
    let t = std::time::Instant::now();
    let task = loop {
        let (_, v) = call(&e.app, Method::GET, "/api/tasks", None).await;
        let task = v["tasks"][0].clone();
        if task["id"] == id && task["status"] != "running" && task["status"] != "cancelling" {
            break task;
        }
        assert!(
            t.elapsed() < Duration::from_secs(60),
            "task never ended: {task}"
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
    };
    assert_eq!(task["kind"], "card_import");
    assert_eq!(task["status"], "done", "{task}");
    let report = &task["params"]["report"];
    assert_eq!(report["found"], 3);
    assert_eq!(report["copied"], 3);
    let sid = report["session_id"]
        .as_i64()
        .expect("session id in the report");
    e.core
        .wait_session_ready(sid, Duration::from_secs(30))
        .await
        .unwrap();
    let (_, v) = call(
        &e.app,
        Method::GET,
        &format!("/api/photos?session_id={sid}"),
        None,
    )
    .await;
    assert_eq!(
        v["photos"].as_array().unwrap().len(),
        2,
        "RAW+JPEG is one photo"
    );
    let (_, v) = call(&e.app, Method::GET, "/api/sessions", None).await;
    assert_eq!(v["sessions"][0]["title"], "Fake card");
    // copies, not moves
    let year_dir = std::fs::read_dir(e.dest.path().join("card"))
        .unwrap()
        .count();
    assert_eq!(year_dir, 1);
    assert!(e.card.path().join("DCIM/100CANON/IMG_0001.CR2").exists());
}
