//! HTTP-level tests of `POST /api/export/collage` (PRD F9.5): validation, the destination
//! whitelist and a finished task (FakeImaging).

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use ip_core::ip_render::Backend;
use ip_core::testutil::{write_jpeg, FakeImaging, FakeRenderer, FakeWorker};
use ip_core::{Core, CoreConfig, Event};
use ip_server::build_router;
use serde_json::{json, Value};
use tower::ServiceExt;

struct Env {
    core: Arc<Core>,
    app: Router,
    _data: tempfile::TempDir,
}

fn env() -> Env {
    let data = tempfile::tempdir().unwrap();
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(2),
        worker: Some(Arc::new(FakeWorker::new())),
        renderer: Some(Arc::new(FakeRenderer::new(Backend::Cpu))),
        force_cpu: true,
    })
    .unwrap();
    let app = build_router(core.clone(), None);
    Env {
        core,
        app,
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
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// Imports three JPEGs and returns their ids.
async fn photos(e: &Env, src: &std::path::Path) -> Vec<i64> {
    for i in 0..3 {
        write_jpeg(&src.join(format!("p{i}.jpg")), 300, 200, 40 * i as u8);
    }
    let (status, s) = call(
        &e.app,
        Method::POST,
        "/api/import",
        Some(json!({"path": src.to_string_lossy()})),
    )
    .await;
    assert!(status.is_success(), "{s}");
    let sid = s["session"]["id"].as_i64().unwrap();
    e.core
        .wait_session_ready(sid, Duration::from_secs(30))
        .await
        .unwrap();
    let (_, r) = call(&e.app, Method::GET, &format!("/api/photos?session_id={sid}"), None).await;
    r["photos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_i64().unwrap())
        .collect()
}

fn body(ids: &[i64], dest: &std::path::Path) -> Value {
    json!({
        "ids": ids,
        "dest": dest.to_string_lossy(),
        "layout": "grid",
        "width": 400,
        "height": 300,
        "format": "png",
    })
}

#[tokio::test]
async fn collage_requests_are_validated_before_anything_runs() {
    let e = env();
    let out = tempfile::tempdir().unwrap();
    let post = |v: Value| call(&e.app, Method::POST, "/api/export/collage", Some(v));
    let bad = |what: &str, v: Value| {
        let what = what.to_string();
        let post = &post;
        async move {
            let (status, r) = post(v).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{what}: {r}");
            assert_eq!(r["error"]["code"], "bad_request", "{what}");
        }
    };
    let ok = body(&[1, 2], out.path());
    let with = |k: &str, v: Value| {
        let mut b = ok.clone();
        b[k] = v;
        b
    };
    bad("one photo", with("ids", json!([1]))).await;
    bad("31 photos", with("ids", json!((1..=31).collect::<Vec<i64>>()))).await;
    bad("repeated", with("ids", json!([1, 1]))).await;
    bad("width", with("width", json!(10))).await;
    bad("height", with("height", json!(20000))).await;
    bad("megapixels", {
        let mut b = ok.clone();
        b["width"] = json!(12000);
        b["height"] = json!(12000);
        b
    })
    .await;
    bad("gap", with("gap", json!(5000))).await;
    bad("background", with("background", json!("red"))).await;
    bad("quality", with("quality", json!(0))).await;
    bad("relative dest", with("dest", json!("relative/dir"))).await;
    // unknown layout / fit are rejected while parsing
    let (status, _) = post(with("layout", json!("spiral"))).await;
    assert!(status.is_client_error());
    let (status, _) = post(with("fit", json!("stretch"))).await;
    assert!(status.is_client_error());
    // unknown photos: 404, nothing written
    let (status, r) = post(ok.clone()).await;
    assert_eq!((status, r["error"]["code"].as_str()), (StatusCode::NOT_FOUND, Some("not_found")));
    assert_eq!(std::fs::read_dir(out.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn collage_task_runs_and_writes_one_file() {
    let e = env();
    let src = tempfile::tempdir().unwrap();
    let ids = photos(&e, src.path()).await;
    assert_eq!(ids.len(), 3);
    let out = tempfile::tempdir().unwrap();
    let mut rx = e.core.events.subscribe();
    let (status, r) = call(
        &e.app,
        Method::POST,
        "/api/export/collage",
        Some(body(&ids, out.path())),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{r}");
    let task = r["task_id"].as_str().unwrap().to_string();
    let (state, error) = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Ok(Event::TaskProgress {
                task_id,
                state,
                error,
                ..
            }) = rx.recv().await
            {
                if task_id == task && state != "running" {
                    return (state, error);
                }
            }
        }
    })
    .await
    .unwrap();
    assert_eq!((state.as_str(), error), ("done", None));
    let files: Vec<_> = std::fs::read_dir(out.path())
        .unwrap()
        .map(|d| d.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(files, ["collage.png"]);
}
