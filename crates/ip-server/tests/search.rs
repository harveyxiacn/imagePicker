//! HTTP-level tests of `GET /api/search` (FakeImaging + FakeWorker with a fake `text.embed`).

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use ip_core::testutil::{write_jpeg, FakeImaging, FakeSpec, FakeWorker};
use ip_core::{AnalysisRunRequest, Core, CoreConfig, Profile, RunState};
use ip_server::build_router;
use serde_json::{json, Value};
use tower::ServiceExt;

const BASE: u64 = 1_700_000_000;

struct Env {
    worker: Arc<FakeWorker>,
    app: Router,
    sid: i64,
    /// file name -> photo id
    ids: std::collections::HashMap<String, i64>,
    _data: tempfile::TempDir,
    _src: tempfile::TempDir,
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

/// Five photos whose fake embeddings sit at 0, 25, 70, 86 and 90 degrees from the "sunset" query
/// vector: cosines 1.0, .91, .34, .07, 0 (the last two fall under the default threshold).
async fn env() -> Env {
    let data = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let worker = Arc::new(FakeWorker::new());
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(2),
        worker: Some(worker.clone()),
        renderer: None,
        force_cpu: true,
    })
    .unwrap();
    for (i, (name, angle)) in [
        ("a.jpg", 0.0),
        ("b.jpg", 25.0),
        ("c.jpg", 70.0),
        ("d.jpg", 86.0),
        ("e.jpg", 90.0),
    ]
    .into_iter()
    .enumerate()
    {
        let p = src.path().join(name);
        write_jpeg(&p, 320, 240, (i * 40) as u8);
        let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
        f.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(BASE + 3600 * i as u64))
            .unwrap();
        worker.set(name, FakeSpec::at(angle));
    }
    worker.set_text_vector("sunset", vec![1.0, 0.0, 0.0, 0.0]);
    let app = build_router(core.clone(), None);
    let r = call(
        &app,
        Method::POST,
        "/api/import",
        Some(json!({"path": src.path().to_string_lossy()})),
    )
    .await;
    let sid = r.json()["session"]["id"].as_i64().unwrap();
    core.wait_session_ready(sid, Duration::from_secs(30))
        .await
        .unwrap();
    core.analysis_run(AnalysisRunRequest {
        session_id: sid,
        profile: Profile::Standard,
        photo_ids: None,
        force: false,
        allow_download: false,
        fallback_lite: false,
    })
    .await
    .unwrap();
    for _ in 0..1200 {
        if core.analysis_status(sid).state != RunState::Running {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let r = call(
        &app,
        Method::GET,
        &format!("/api/photos?session_id={sid}"),
        None,
    )
    .await;
    let ids = r.json()["photos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["file_name"].as_str().unwrap().to_string(),
                p["id"].as_i64().unwrap(),
            )
        })
        .collect();
    Env {
        worker,
        app,
        sid,
        ids,
        _data: data,
        _src: src,
    }
}

fn names(e: &Env, r: &Resp) -> Vec<String> {
    r.json()["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| {
            let id = h["photo_id"].as_i64().unwrap();
            e.ids.iter().find(|(_, v)| **v == id).unwrap().0.clone()
        })
        .collect()
}

#[tokio::test]
async fn search_returns_ranked_hits_with_the_documented_shape() {
    let e = env().await;
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/search?q=sunset&session_id={}", e.sid),
        None,
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let j = r.json();
    assert_eq!(names(&e, &r), ["a.jpg", "b.jpg", "c.jpg"]);
    assert_eq!(j["query"], "sunset");
    assert_eq!(j["model"], "fake-siglip-text");
    assert_eq!(j["min_score"], 0.05);
    assert_eq!(j["total_matched"], 3);
    assert_eq!(j["searched"], 5);
    assert_eq!(j["unindexed"], 0);
    let h = &j["hits"][0];
    assert!(h["score"].as_f64().unwrap() > 0.99);
    assert!((h["similarity"].as_f64().unwrap() - 1.0).abs() < 0.01);
    // scores of strong matches saturate at 1.0: the cosine carries the order
    assert!(j["hits"][2]["similarity"].as_f64().unwrap() < h["similarity"].as_f64().unwrap());
}

#[tokio::test]
async fn threshold_limit_and_library_scope() {
    let e = env().await;
    let r = call(
        &e.app,
        Method::GET,
        "/api/search?q=sunset&min_score=0",
        None,
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "no session_id searches the whole library"
    );
    assert_eq!(names(&e, &r).len(), 5);
    let r = call(
        &e.app,
        Method::GET,
        &format!(
            "/api/search?q=sunset&session_id={}&limit=1&min_score=0",
            e.sid
        ),
        None,
    )
    .await;
    assert_eq!(names(&e, &r), ["a.jpg"]);
    assert_eq!(r.json()["total_matched"], 5);
}

#[tokio::test]
async fn photo_filters_combine_with_the_search() {
    let e = env().await;
    let pick = [e.ids["b.jpg"], e.ids["d.jpg"]];
    let r = call(
        &e.app,
        Method::PATCH,
        "/api/photos",
        Some(json!({"ids": pick, "flag": 1})),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/search?q=sunset&session_id={}&flag=picked", e.sid),
        None,
    )
    .await;
    assert_eq!(
        names(&e, &r),
        ["b.jpg"],
        "d.jpg is picked but too far from the query"
    );
    assert_eq!(r.json()["searched"], 2);
    // the photo list takes the hits back as `ids`, with the same filters, in the grid's order
    let ids = format!("{},{}", e.ids["a.jpg"], e.ids["c.jpg"]);
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/photos?session_id={}&ids={ids}", e.sid),
        None,
    )
    .await;
    assert_eq!(r.json()["total"], 2);
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/photos?session_id={}&ids=", e.sid),
        None,
    )
    .await;
    assert_eq!(r.json()["total"], 0);
}

#[tokio::test]
async fn bad_requests_are_400() {
    let e = env().await;
    let sid = e.sid;
    for uri in [
        "/api/search".to_string(),
        "/api/search?q=".to_string(),
        "/api/search?q=%20%20".to_string(),
        format!("/api/search?q=sunset&session_id={sid}&min_score=2"),
        format!("/api/search?q=sunset&session_id={sid}&limit=0"),
        format!("/api/search?q=sunset&session_id={sid}&limit=abc"),
        format!("/api/search?q=sunset&session_id={sid}&flag=maybe"),
        "/api/search?q=sunset&flag=picked".to_string(),
    ] {
        let r = call(&e.app, Method::GET, &uri, None).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(r.code(), "bad_request", "{uri}");
    }
    let r = call(
        &e.app,
        Method::GET,
        "/api/search?q=sunset&session_id=99999",
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(
        e.worker
            .text_embed_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn missing_text_model_is_409_models_missing() {
    let e = env().await;
    e.worker.set_text_missing(&["siglip2-base-text"]);
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/search?q=sunset&session_id={}", e.sid),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.code(), "models_missing");
    assert_eq!(r.json()["models"], json!(["siglip2-base-text"]));
}
