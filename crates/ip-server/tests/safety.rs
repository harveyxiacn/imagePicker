//! HTTP-level tests of the data-safety routes and settings (docs/09 §5.1 P0-1, P0-3, P0-4):
//! the confirmed `modify_originals` XMP mode, the face-recognition consent flag and the catalog
//! backup / restore API.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use ip_core::ip_render::Backend;
use ip_core::testutil::{write_jpeg, FakeImaging, FakeRenderer, FakeWorker};
use ip_core::{Core, CoreConfig};
use ip_server::build_router;
use serde_json::{json, Value};
use tower::ServiceExt;

struct Env {
    core: Arc<Core>,
    app: Router,
    src: tempfile::TempDir,
    _data: tempfile::TempDir,
}

fn env() -> Env {
    let data = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
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

#[tokio::test]
async fn settings_need_consent_for_faces_and_confirmation_for_modifying_originals() {
    let e = env();
    let s = call(&e.app, Method::GET, "/api/settings", None)
        .await
        .json();
    assert_eq!(s["faces"], json!({"enabled": true, "consented": false}));
    assert_eq!(s["xmp_mode"], "off");

    let r = call(
        &e.app,
        Method::PATCH,
        "/api/settings",
        Some(json!({"xmp_mode": "modify_originals"})),
    )
    .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(r.code(), "unprocessable");
    let r = call(
        &e.app,
        Method::PATCH,
        "/api/settings",
        Some(json!({"xmp_mode": "sidecar_and_embedded"})),
    )
    .await;
    assert!(r.status.is_client_error(), "the old mode name is gone");
    let r = call(
        &e.app,
        Method::PATCH,
        "/api/settings",
        Some(json!({"xmp_mode": "modify_originals", "confirm_modify_originals": true})),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let s = r.json();
    assert_eq!(s["xmp_mode"], "modify_originals");
    assert!(s.get("confirm_modify_originals").is_none(), "never stored");

    let r = call(
        &e.app,
        Method::PATCH,
        "/api/settings",
        Some(json!({"faces": {"consented": true}})),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["faces"]["consented"], true);
    assert!(e.core.settings().faces.allowed());
}

#[tokio::test]
async fn catalog_backup_and_restore_routes() {
    let e = env();
    write_jpeg(&e.src.path().join("a.jpg"), 320, 240, 10);
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
    e.core.catalog_check().await.unwrap();

    let st = call(&e.app, Method::GET, "/api/catalog", None).await;
    assert_eq!(st.status, StatusCode::OK);
    let st = st.json();
    assert_eq!(st["state"], "ok");
    assert_eq!(st["backups"], json!([]));
    assert!(st["backup_dir"].as_str().unwrap().ends_with("backups"));

    let b = call(&e.app, Method::POST, "/api/catalog/backup", None).await;
    assert_eq!(b.status, StatusCode::OK);
    let b = b.json();
    let name = b["name"].as_str().unwrap().to_string();
    assert!(
        name.starts_with("catalog-") && name.ends_with(".db"),
        "{name}"
    );
    assert_eq!(b["kind"], "auto");
    let st = call(&e.app, Method::GET, "/api/catalog", None).await.json();
    assert_eq!(st["backups"][0]["name"], name.as_str());
    assert_eq!(st["last_backup_at"], b["created_at"]);

    // a session made after the backup disappears with the restore
    let r = call(
        &e.app,
        Method::DELETE,
        &format!("/api/sessions/{sid}"),
        None,
    )
    .await;
    assert!(r.status.is_success());
    for (bad, status) in [
        ("../catalog.db", StatusCode::BAD_REQUEST),
        ("catalog-20000101-000000-000.db", StatusCode::NOT_FOUND),
    ] {
        let r = call(
            &e.app,
            Method::POST,
            "/api/catalog/restore",
            Some(json!({"name": bad})),
        )
        .await;
        assert_eq!(r.status, status, "{bad}");
    }
    let r = call(
        &e.app,
        Method::POST,
        "/api/catalog/restore",
        Some(json!({"name": name})),
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let st = r.json();
    assert_eq!(st["state"], "ok");
    assert_eq!(st["restored_from"], name.as_str());
    let sessions = call(&e.app, Method::GET, "/api/sessions", None)
        .await
        .json();
    assert_eq!(sessions["sessions"].as_array().unwrap().len(), 1);
}
