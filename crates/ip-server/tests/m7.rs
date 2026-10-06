//! HTTP-level tests of the M7 runtime routes and the `runtime: "missing"` hint.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::response::IntoResponse;
use axum::Router;
use http_body_util::BodyExt;
use ip_core::ip_render::Backend;
use ip_core::testutil::{FakeImaging, FakeRenderer, FakeWorker};
use ip_core::{Core, CoreConfig, CoreError};
use ip_server::build_router;
use serde_json::{json, Value};
use tower::ServiceExt;

fn env() -> (Router, tempfile::TempDir) {
    let data = tempfile::tempdir().unwrap();
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(1),
        worker: Some(Arc::new(FakeWorker::new())),
        renderer: Some(Arc::new(FakeRenderer::new(Backend::Cpu))),
        force_cpu: true,
    })
    .unwrap();
    (build_router(core, None), data)
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

#[tokio::test]
async fn runtime_status_and_guards() {
    let (app, _d) = env();
    let (st, v) = call(&app, Method::GET, "/api/runtime", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["state"], "missing");
    assert!(v["recommended_extras"].is_array());
    assert!(v["hardware"]["os"].is_string());

    // nothing is installing: cancel is a no-op, remove of a missing runtime is fine
    let (st, _) = call(&app, Method::POST, "/api/runtime/cancel", None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = call(&app, Method::DELETE, "/api/runtime", None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    // unknown extras: 422 (when the build could install at all) or 409 (no uv / worker source)
    let (st, v) = call(
        &app,
        Method::POST,
        "/api/runtime/install",
        Some(json!({"extras": ["cpu", "cuda"]})),
    )
    .await;
    assert!(
        st == StatusCode::UNPROCESSABLE_ENTITY || st == StatusCode::CONFLICT,
        "{st} {v}"
    );
}

#[tokio::test]
async fn install_is_refused_offline() {
    let (app, _d) = env();
    let (st, _) = call(
        &app,
        Method::PATCH,
        "/api/settings",
        Some(json!({"privacy": {"allow_network": false}})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, v) = call(&app, Method::POST, "/api/runtime/install", Some(json!({}))).await;
    assert_eq!(st, StatusCode::CONFLICT);
    assert!(v["error"]["message"].as_str().unwrap().contains("network"));
    let (_, v) = call(&app, Method::GET, "/api/runtime", None).await;
    assert_eq!(v["can_install"], false);
}

#[tokio::test]
async fn missing_runtime_is_a_503_with_a_hint() {
    let tagged = format!(
        "{}AI worker unavailable: the AI components are not installed (not found)",
        ip_worker_client::RUNTIME_MISSING_TAG
    );
    let resp =
        ip_server::error::ApiError::from(CoreError::WorkerUnavailable(tagged)).into_response();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["error"]["code"], "worker_unavailable");
    assert_eq!(v["runtime"], "missing");
    assert!(!v["error"]["message"].as_str().unwrap().contains("[runtime"));

    // other worker failures carry no hint
    let resp = ip_server::error::ApiError::from(CoreError::WorkerUnavailable("down".into()))
        .into_response();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(v.get("runtime").is_none());
}
