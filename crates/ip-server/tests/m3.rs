//! HTTP-level tests of the M3 routes (FakeImaging + FakeWorker + FakeRenderer).
//! Tests marked `#[ignore]` need the real `ip-render` implementation.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use futures_util::StreamExt;
use http_body_util::BodyExt;
use ip_core::ip_render::Backend;
use ip_core::testutil::{FakeImaging, FakeRenderer, FakeWorker};
use ip_core::{Core, CoreConfig};
use ip_server::{build_router, spawn_with_core, ServerConfig};
use serde_json::{json, Value};
use tower::ServiceExt;

struct Env {
    core: Arc<Core>,
    worker: Arc<FakeWorker>,
    renderer: Arc<FakeRenderer>,
    app: Router,
    src: tempfile::TempDir,
    _data: tempfile::TempDir,
}

fn env() -> Env {
    let data = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let worker = Arc::new(FakeWorker::new());
    let renderer = Arc::new(FakeRenderer::new(Backend::Gpu));
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(2),
        worker: Some(worker.clone()),
        renderer: Some(renderer.clone()),
        force_cpu: true,
    })
    .unwrap();
    let app = build_router(core.clone(), None);
    Env {
        core,
        worker,
        renderer,
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
    fn code(&self) -> String {
        self.json()["error"]["code"].as_str().unwrap_or("").into()
    }
}

async fn call_raw(app: &Router, method: Method, uri: &str, body: Option<String>) -> Resp {
    let b = Request::builder().method(method).uri(uri);
    let req = match body {
        Some(v) => b
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let (status, headers) = (resp.status(), resp.headers().clone());
    let body = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    Resp {
        status,
        headers,
        body,
    }
}

async fn call(app: &Router, method: Method, uri: &str, body: Option<Value>) -> Resp {
    call_raw(app, method, uri, body.map(|v| v.to_string())).await
}

fn solid(dir: &std::path::Path, name: &str, rgb: [u8; 3]) {
    image::RgbImage::from_pixel(160, 120, image::Rgb(rgb))
        .save_with_format(dir.join(name), image::ImageFormat::Jpeg)
        .unwrap();
}

/// Imports the given files; returns (session id, photo ids in the order given).
async fn setup(e: &Env, files: &[(&str, [u8; 3])]) -> (i64, Vec<i64>) {
    for (n, c) in files {
        solid(e.src.path(), n, *c);
    }
    let r = call(
        &e.app,
        Method::POST,
        "/api/import",
        Some(json!({"path": e.src.path().to_string_lossy()})),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let sid = r.json()["session"]["id"].as_i64().unwrap();
    e.core
        .wait_session_ready(sid, Duration::from_secs(30))
        .await
        .unwrap();
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/photos?session_id={sid}"),
        None,
    )
    .await;
    let photos = r.json()["photos"].as_array().unwrap().clone();
    let ids = files
        .iter()
        .map(|(n, _)| {
            photos.iter().find(|p| p["file_name"] == *n).unwrap()["id"]
                .as_i64()
                .unwrap()
        })
        .collect();
    (sid, ids)
}

fn exposure(ev: f64) -> Value {
    json!({"version":1,"ops":[{"type":"global","exposure":ev}]})
}

fn mean_of(bytes: &[u8]) -> f64 {
    let img = image::load_from_memory(bytes).unwrap().to_rgb8();
    img.as_raw().iter().map(|v| *v as f64).sum::<f64>() / img.as_raw().len() as f64
}

#[tokio::test]
async fn edits_crud_shapes_and_validation() {
    let e = env();
    let (sid, ids) = setup(&e, &[("a.jpg", [100, 100, 100])]).await;
    let id = ids[0];

    let r = call(&e.app, Method::GET, &format!("/api/edits/{id}"), None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        r.json(),
        json!({"photo_id": id, "stack": {"version":1,"ops":[]}, "updated_at": null})
    );

    let photo = call(&e.app, Method::GET, &format!("/api/photos/{id}"), None)
        .await
        .json()["photo"]
        .clone();
    assert_eq!(photo["has_edits"], false);

    let stack = json!({"version":1,"ops":[
        {"type":"global","exposure":0.5,"temp":120},
        {"type":"future_op","payload":{"a":1}}]});
    let r = call(
        &e.app,
        Method::PUT,
        &format!("/api/edits/{id}"),
        Some(json!({"stack": stack})),
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let put = r.json();
    assert_eq!(put["photo_id"], id);
    assert_eq!(put["stack"], stack);
    assert!(put["updated_at"].as_i64().unwrap() > 0);
    let tv = put["thumb_version"].as_str().unwrap().to_string();
    assert_ne!(tv, photo["thumb_version"].as_str().unwrap());

    let photo = call(&e.app, Method::GET, &format!("/api/photos/{id}"), None)
        .await
        .json()["photo"]
        .clone();
    assert_eq!(photo["has_edits"], true);
    assert_eq!(photo["thumb_version"], tv);
    let list = call(
        &e.app,
        Method::GET,
        &format!("/api/photos?session_id={sid}"),
        None,
    )
    .await
    .json();
    assert_eq!(list["photos"][0]["has_edits"], true);
    assert_eq!(list["photos"][0]["thumb_version"], tv);

    let r = call(&e.app, Method::GET, &format!("/api/edits/{id}"), None).await;
    assert_eq!(r.json()["stack"], stack);

    // validation: wrong shape / types / ranges are 422, broken JSON is 400
    for body in [
        json!({}),
        json!({"stack": 5}),
        json!({"stack": {"version":1,"ops":"x"}}),
        json!({"stack": {"version":1,"ops":[{"type":"global","exposure":"bright"}]}}),
        json!({"stack": {"version":1,"ops":[{"type":"global","exposure":7}]}}),
        json!({"stack": {"version":1,"ops":[{"type":"crop","rect":[0,0,2,2]}]}}),
        json!({"stack": {"version":3,"ops":[]}}),
    ] {
        let r = call(
            &e.app,
            Method::PUT,
            &format!("/api/edits/{id}"),
            Some(body.clone()),
        )
        .await;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    }
    let r = call_raw(
        &e.app,
        Method::PUT,
        &format!("/api/edits/{id}"),
        Some("{not json".into()),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = call(
        &e.app,
        Method::PUT,
        &format!("/api/edits/{id}"),
        Some(json!({"stack": exposure(8.0)})),
    )
    .await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, "unprocessable")
    );
    // the saved stack survived the failed writes
    let r = call(&e.app, Method::GET, &format!("/api/edits/{id}"), None).await;
    assert_eq!(r.json()["stack"], stack);

    // unknown photo
    assert_eq!(
        call(&e.app, Method::GET, "/api/edits/999", None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &e.app,
            Method::PUT,
            "/api/edits/999",
            Some(json!({"stack": exposure(1.0)}))
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );

    // DELETE resets
    let r = call(&e.app, Method::DELETE, &format!("/api/edits/{id}"), None).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let photo = call(&e.app, Method::GET, &format!("/api/photos/{id}"), None)
        .await
        .json()["photo"]
        .clone();
    assert_eq!(photo["has_edits"], false);
    assert_ne!(photo["thumb_version"], tv);
    let r = call(&e.app, Method::GET, &format!("/api/edits/{id}"), None).await;
    assert_eq!(r.json()["updated_at"], Value::Null);
    assert_eq!(
        call(&e.app, Method::DELETE, "/api/edits/999", None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn thumb_and_preview_serve_rendered_images_for_edited_photos() {
    let e = env();
    let (_, ids) = setup(&e, &[("a.jpg", [100, 100, 100])]).await;
    let id = ids[0];
    let t0 = call(&e.app, Method::GET, &format!("/api/thumb/{id}?s=256"), None).await;
    let p0 = call(
        &e.app,
        Method::GET,
        &format!("/api/preview/{id}?s=1024"),
        None,
    )
    .await;
    assert_eq!(t0.status, StatusCode::OK);

    call(
        &e.app,
        Method::PUT,
        &format!("/api/edits/{id}"),
        Some(json!({"stack": exposure(1.5)})),
    )
    .await;
    let t1 = call(&e.app, Method::GET, &format!("/api/thumb/{id}?s=256"), None).await;
    let p1 = call(
        &e.app,
        Method::GET,
        &format!("/api/preview/{id}?s=1024"),
        None,
    )
    .await;
    assert_eq!(t1.headers[header::CONTENT_TYPE], "image/jpeg");
    assert!(mean_of(&t1.body) > mean_of(&t0.body) + 20.0);
    assert!(mean_of(&p1.body) > mean_of(&p0.body) + 20.0);

    call(&e.app, Method::DELETE, &format!("/api/edits/{id}"), None).await;
    let t2 = call(&e.app, Method::GET, &format!("/api/thumb/{id}?s=256"), None).await;
    assert_eq!(
        t2.body, t0.body,
        "clearing edits restores the original thumbnail"
    );
}

#[tokio::test]
async fn render_preview_headers_stack_override_and_original() {
    let e = env();
    let (_, ids) = setup(&e, &[("a.jpg", [100, 100, 100])]).await;
    let id = ids[0];
    let post = |body: Value| {
        let app = e.app.clone();
        async move { call(&app, Method::POST, "/api/render/preview", Some(body)).await }
    };

    // no stack, nothing saved: plain proxy
    let base = post(json!({"photo_id": id, "long_edge": 100})).await;
    assert_eq!(base.status, StatusCode::OK);
    assert_eq!(base.headers[header::CONTENT_TYPE], "image/jpeg");
    assert_eq!(base.headers["x-render-backend"], "gpu");
    assert!(base.headers["x-render-ms"]
        .to_str()
        .unwrap()
        .parse::<u64>()
        .is_ok());
    let img = image::load_from_memory(&base.body).unwrap();
    assert_eq!(img.width().max(img.height()), 100);
    assert_eq!(e.renderer.calls.load(Ordering::SeqCst), 0);

    // explicit stack
    let r = post(json!({"photo_id": id, "stack": exposure(1.0), "long_edge": 100})).await;
    assert!(mean_of(&r.body) > mean_of(&base.body) + 15.0);
    assert_eq!(e.renderer.calls.load(Ordering::SeqCst), 1);

    // saved stack is used when `stack` is omitted; `original` bypasses it
    call(
        &e.app,
        Method::PUT,
        &format!("/api/edits/{id}"),
        Some(json!({"stack": exposure(-1.0)})),
    )
    .await;
    let saved = post(json!({"photo_id": id, "long_edge": 100})).await;
    assert!(mean_of(&saved.body) < mean_of(&base.body) - 10.0);
    let calls = e.renderer.calls.load(Ordering::SeqCst);
    let orig = post(json!({"photo_id": id, "long_edge": 100, "original": true})).await;
    assert!((mean_of(&orig.body) - mean_of(&base.body)).abs() < 3.0);
    assert_eq!(e.renderer.calls.load(Ordering::SeqCst), calls);

    // default long edge (1600) never upscales the 160 px source
    let r = post(json!({"photo_id": id})).await;
    assert_eq!(image::load_from_memory(&r.body).unwrap().width(), 160);

    // errors
    assert_eq!(
        post(json!({"photo_id": 999})).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        post(json!({"photo_id": id, "long_edge": 0})).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post(
            json!({"photo_id": id, "stack": {"version":1,"ops":[{"type":"global","exposure":99}]}})
        )
        .await
        .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        post(json!({})).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    // CPU backend is reported as such
    let cpu = Arc::new(FakeRenderer::new(Backend::Cpu));
    let data = tempfile::tempdir().unwrap();
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(1),
        worker: Some(Arc::new(FakeWorker::new())),
        renderer: Some(cpu),
        force_cpu: false,
    })
    .unwrap();
    assert_eq!(core.render_service().backend(), Backend::Cpu);
}

#[tokio::test]
async fn presets_http() {
    let e = env();
    let r = call(&e.app, Method::GET, "/api/presets", None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.json()["presets"].is_array());

    let stack = json!({"version":1,"ops":[
        {"type":"crop","rect":[0,0,0.5,0.5]},
        {"type":"global","exposure":0.4},
        {"type":"lut","file":"film_warm","amount":0.5}]});
    let r = call(
        &e.app,
        Method::POST,
        "/api/presets",
        Some(json!({"name": "Mine", "stack": stack})),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let p = r.json()["preset"].clone();
    assert_eq!(p["name"], "Mine");
    assert_eq!(p["builtin"], false);
    assert_eq!(
        p["stack"]["ops"].as_array().unwrap().len(),
        2,
        "crop dropped"
    );
    let id = p["id"].as_str().unwrap().to_string();

    let list = call(&e.app, Method::GET, "/api/presets", None).await.json();
    let mine = list["presets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == id.as_str())
        .unwrap();
    assert_eq!(mine["builtin"], false);

    for body in [
        json!({"name": "", "stack": stack}),
        json!({"name": "x", "stack": {"version":1,"ops":[{"type":"crop","rect":[0,0,1,1]}]}}),
        json!({"name": "x", "stack": {"version":1,"ops":[{"type":"global","exposure":50}]}}),
        json!({"stack": stack}),
    ] {
        let r = call(&e.app, Method::POST, "/api/presets", Some(body.clone())).await;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    }

    let r = call(&e.app, Method::DELETE, &format!("/api/presets/{id}"), None).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(
        call(&e.app, Method::DELETE, &format!("/api/presets/{id}"), None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn builtin_presets_are_listed_and_protected() {
    let data = tempfile::tempdir().unwrap();
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(1),
        worker: Some(Arc::new(FakeWorker::new())),
        renderer: None,
        force_cpu: true,
    })
    .unwrap();
    let app = build_router(core, None);
    let list = call(&app, Method::GET, "/api/presets", None).await.json();
    let b = list["presets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["builtin"] == true)
        .expect("built-in presets")
        .clone();
    assert!(b["name"].as_str().unwrap().starts_with("preset."));
    let r = call(
        &app,
        Method::DELETE,
        &format!("/api/presets/{}", b["id"].as_str().unwrap()),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn lut_import_errors() {
    let e = env();
    let missing = e.src.path().join("nope.cube");
    let garbage = e.src.path().join("bad.cube");
    std::fs::write(&garbage, "not a lut").unwrap();
    for path in [
        "".to_string(),
        "photo.jpg".to_string(),
        missing.to_string_lossy().into_owned(),
        garbage.to_string_lossy().into_owned(),
    ] {
        let r = call(
            &e.app,
            Method::POST,
            "/api/luts/import",
            Some(json!({"path": path})),
        )
        .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{path:?}");
        assert_eq!(r.code(), "bad_request");
    }
    let r = call(&e.app, Method::POST, "/api/luts/import", Some(json!({}))).await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn lut_import_succeeds_and_is_usable() {
    let data = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(1),
        worker: Some(Arc::new(FakeWorker::new())),
        renderer: None,
        force_cpu: true,
    })
    .unwrap();
    let app = build_router(core, None);
    let cube = src.path().join("Teal.cube");
    std::fs::write(
        &cube,
        "LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n",
    )
    .unwrap();
    let r = call(
        &app,
        Method::POST,
        "/api/luts/import",
        Some(json!({"path": cube.to_string_lossy()})),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["name"], "Teal");
    assert!(!r.json()["id"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn sync_and_auto_http() {
    let e = env();
    let (_, ids) = setup(
        &e,
        &[("a.jpg", [120, 120, 120]), ("b.jpg", [120, 120, 120])],
    )
    .await;
    let (a, b) = (ids[0], ids[1]);
    call(
        &e.app,
        Method::PUT,
        &format!("/api/edits/{a}"),
        Some(json!({"stack": {"version":1,"ops":[
            {"type":"global","exposure":0.5},{"type":"lut","file":"film_warm"}]}})),
    )
    .await;
    let r = call(
        &e.app,
        Method::POST,
        "/api/edits/sync",
        Some(json!({"from_id": a, "to_ids": [b], "include": ["lut"], "adaptive": false})),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json(), json!({"updated": 1}));
    let sb = call(&e.app, Method::GET, &format!("/api/edits/{b}"), None)
        .await
        .json();
    assert_eq!(sb["stack"]["ops"].as_array().unwrap().len(), 1);
    assert_eq!(sb["stack"]["ops"][0]["type"], "lut");

    // adaptive defaults to true and works with include ["global"]
    let r = call(
        &e.app,
        Method::POST,
        "/api/edits/sync",
        Some(json!({"from_id": a, "to_ids": [b], "include": ["global"]})),
    )
    .await;
    assert_eq!(r.json(), json!({"updated": 1}));

    for body in [
        json!({"from_id": a, "to_ids": [b], "include": ["bogus"]}),
        json!({"from_id": a, "to_ids": [b]}),
    ] {
        let r = call(&e.app, Method::POST, "/api/edits/sync", Some(body)).await;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    }
    let r = call(
        &e.app,
        Method::POST,
        "/api/edits/sync",
        Some(json!({"from_id": a, "to_ids": [b], "include": []})),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = call(
        &e.app,
        Method::POST,
        "/api/edits/sync",
        Some(json!({"from_id": 999, "to_ids": [b], "include": ["global"]})),
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);

    // auto: shape {"adjust": {...}}, nothing saved
    call(&e.app, Method::DELETE, &format!("/api/edits/{b}"), None).await;
    let r = call(
        &e.app,
        Method::POST,
        &format!("/api/edits/{b}/auto"),
        Some(json!({"mode": "landscape"})),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.json()["adjust"]["exposure"].is_number());
    let doc = call(&e.app, Method::GET, &format!("/api/edits/{b}"), None)
        .await
        .json();
    assert_eq!(doc["updated_at"], Value::Null);
    let r = call(
        &e.app,
        Method::POST,
        &format!("/api/edits/{b}/auto"),
        Some(json!({"mode": "weird"})),
    )
    .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    let r = call(
        &e.app,
        Method::POST,
        "/api/edits/999/auto",
        Some(json!({"mode": "auto"})),
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn masks_http() {
    let e = env();
    let (_, ids) = setup(&e, &[("a.jpg", [100, 100, 100])]).await;
    let id = ids[0];
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/masks/{id}?target=sky"),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.headers[header::CONTENT_TYPE], "image/png");
    let m = image::load_from_memory(&r.body).unwrap();
    assert_eq!(m.color(), image::ColorType::L8);
    assert_eq!(e.worker.mask_calls.load(Ordering::SeqCst), 1);

    for (uri, status) in [
        (format!("/api/masks/{id}"), StatusCode::BAD_REQUEST),
        (
            format!("/api/masks/{id}?target=nope"),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("/api/masks/{id}?target=person"),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("/api/masks/{id}?target=sky&person_id=x"),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/api/masks/999?target=sky".to_string(),
            StatusCode::NOT_FOUND,
        ),
    ] {
        assert_eq!(
            call(&e.app, Method::GET, &uri, None).await.status,
            status,
            "{uri}"
        );
    }

    // models missing -> 409 with the M2 shape
    e.worker.set_mask_missing(&["birefnet"]);
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/masks/{id}?target=skin"),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.code(), "models_missing");
    assert_eq!(r.json()["models"], json!(["birefnet"]));
    // a preview that needs that mask fails the same way
    let r = call(
        &e.app,
        Method::POST,
        "/api/render/preview",
        Some(json!({"photo_id": id, "stack": {"version":1,"ops":[
            {"type":"local","mask":{"kind":"ai","target":"skin"},"adjust":{"exposure":1}}]}})),
    )
    .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.code(), "models_missing");

    // worker down -> 503
    e.worker.set_mask_missing(&[]);
    e.worker.mask_unavailable.store(true, Ordering::SeqCst);
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/masks/{id}?target=hair"),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(r.code(), "worker_unavailable");
}

#[tokio::test]
async fn edits_updated_event_over_websocket() {
    let e = env();
    let (_, ids) = setup(&e, &[("a.jpg", [100, 100, 100]), ("b.jpg", [90, 90, 90])]).await;
    let server = spawn_with_core(
        e.core.clone(),
        &ServerConfig {
            port: 0,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/api/events", server.addr))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    let put = call(
        &e.app,
        Method::PUT,
        &format!("/api/edits/{}", ids[0]),
        Some(json!({"stack": exposure(0.7)})),
    )
    .await
    .json();
    let v = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let msg = ws.next().await.unwrap().unwrap();
            let v: Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
            if v["type"] == "edits.updated" {
                return v;
            }
        }
    })
    .await
    .expect("edits.updated frame");
    let item = &v["items"][0];
    assert_eq!(item["id"], ids[0]);
    assert_eq!(item["has_edits"], true);
    assert_eq!(item["thumb_version"], put["thumb_version"]);
    let _ = ids[1];
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn export_applies_edits_by_default() {
    let e = env();
    let (_, ids) = setup(&e, &[("a.jpg", [100, 100, 100])]).await;
    call(
        &e.app,
        Method::PUT,
        &format!("/api/edits/{}", ids[0]),
        Some(json!({"stack": exposure(1.5)})),
    )
    .await;
    let dest = e.src.path().join("out");
    let r = call(
        &e.app,
        Method::POST,
        "/api/export",
        Some(json!({"ids": ids, "dest": dest.to_string_lossy()})),
    )
    .await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    let out = dest.join("a.jpg");
    for _ in 0..200 {
        if out.is_file() {
            tokio::time::sleep(Duration::from_millis(100)).await;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let exported = std::fs::read(&out).unwrap();
    let original = std::fs::read(e.src.path().join("a.jpg")).unwrap();
    assert_ne!(exported, original);
    assert!(mean_of(&exported) > mean_of(&original) + 20.0);

    // apply_edits:false copies the original
    let dest2 = e.src.path().join("out2");
    call(
        &e.app,
        Method::POST,
        "/api/export",
        Some(json!({"ids": ids, "dest": dest2.to_string_lossy(), "apply_edits": false})),
    )
    .await;
    for _ in 0..200 {
        if dest2.join("a.jpg").is_file() {
            tokio::time::sleep(Duration::from_millis(100)).await;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(std::fs::read(dest2.join("a.jpg")).unwrap(), original);
}
