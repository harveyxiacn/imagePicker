//! WP-F1 over HTTP: import exclusions, RAW+JPEG pairs in the photo JSON, rescan / relocate,
//! safe reject and the export member choice.

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
    src: tempfile::TempDir,
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
    Env {
        core,
        app,
        src: tempfile::tempdir().unwrap(),
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
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

async fn import(env: &Env, body: Value) -> i64 {
    let (status, v) = call(&env.app, Method::POST, "/api/import", Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{v}");
    let sid = v["session"]["id"].as_i64().unwrap();
    env.core
        .wait_session_ready(sid, Duration::from_secs(30))
        .await
        .unwrap();
    sid
}

async fn names(env: &Env, sid: i64) -> Vec<String> {
    let (_, v) = call(
        &env.app,
        Method::GET,
        &format!("/api/photos?session_id={sid}&sort=name"),
        None,
    )
    .await;
    v["photos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["file_name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn import_accepts_exclude_globs_and_folders() {
    let e = env();
    let d = e.src.path();
    write_jpeg(&d.join("keep.jpg"), 32, 32, 1);
    write_jpeg(&d.join("skip/a.jpg"), 32, 32, 2);
    write_jpeg(&d.join("old/2019/b.jpg"), 32, 32, 3);
    write_jpeg(&d.join("c.tmp.jpg"), 32, 32, 4);
    let path = d.to_string_lossy();
    // `exclude`
    let sid = import(&e, json!({"path": path, "exclude": ["skip", "old/**"]})).await;
    assert_eq!(names(&e, sid).await, ["c.tmp.jpg", "keep.jpg"]);
    // `exclude[]` is the same key
    let sid = import(
        &e,
        json!({"path": path, "exclude[]": ["*.tmp.jpg", "skip"]}),
    )
    .await;
    assert_eq!(names(&e, sid).await, ["b.jpg", "keep.jpg"]);
    // without it everything comes in
    let sid = import(&e, json!({"path": path})).await;
    assert_eq!(names(&e, sid).await.len(), 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn pairs_missing_relocate_reject_and_export_over_http() {
    let e = env();
    let parent = tempfile::tempdir().unwrap();
    let d = parent.path().join("shoot");
    write_jpeg(&d.join("a.jpg"), 32, 32, 1);
    std::fs::write(d.join("a.arw"), vec![7u8; 1000]).unwrap();
    write_jpeg(&d.join("b.jpg"), 32, 32, 2);
    let sid = import(&e, json!({"path": d.to_string_lossy()})).await;

    // one grid item per pair, the RAW listed as its companion
    let (_, v) = call(
        &e.app,
        Method::GET,
        &format!("/api/photos?session_id={sid}&sort=name"),
        None,
    )
    .await;
    assert_eq!(v["total"], 2);
    let a = &v["photos"][0];
    assert_eq!(a["file_name"], "a.jpg");
    assert_eq!(a["companions"][0]["file_name"], "a.arw");
    assert_eq!(a["companions"][0]["format"], "raw");
    assert_eq!(a["missing"], false);
    assert!(
        v["photos"][1].get("companions").is_none(),
        "unpaired photos carry no companions"
    );
    let (a_id, b_id) = (
        a["id"].as_i64().unwrap(),
        v["photos"][1]["id"].as_i64().unwrap(),
    );

    // export the RAW instead of the JPEG
    let out = tempfile::tempdir().unwrap();
    let (status, v) = call(
        &e.app,
        Method::POST,
        "/api/export",
        Some(json!({"ids": [a_id], "dest": out.path().to_string_lossy(), "pair_member": "raw"})),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{v}");
    let task = v["task_id"].as_str().unwrap().to_string();
    for _ in 0..200 {
        let (_, t) = call(&e.app, Method::GET, "/api/tasks", None).await;
        if t["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["id"] == task.as_str() && x["state"] == "done")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(out.path().join("a.arw").is_file() && !out.path().join("a.jpg").exists());
    let (status, _) = call(
        &e.app,
        Method::POST,
        "/api/export",
        Some(json!({"ids": [a_id], "dest": out.path().to_string_lossy(), "pair_member": "bogus"})),
    )
    .await;
    assert!(status.is_client_error(), "{status}");

    // the folder is renamed: everything is flagged missing after a rescan
    let moved = parent.path().join("shoot-renamed");
    std::fs::rename(&d, &moved).unwrap();
    e.core.rescan_session(sid).await.unwrap();
    let (status, v) = call(
        &e.app,
        Method::POST,
        &format!("/api/sessions/{sid}/rescan"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{v}");
    let (_, v) = call(&e.app, Method::GET, &format!("/api/photos/{a_id}"), None).await;
    assert_eq!(v["photo"]["missing"], true);
    assert_eq!(v["photo"]["companions"][0]["missing"], true);

    // relocate: wrong folder is refused, the right one previews, then applies
    let other = parent.path().join("empty");
    std::fs::create_dir_all(&other).unwrap();
    let (status, _) = call(
        &e.app,
        Method::POST,
        &format!("/api/sessions/{sid}/relocate"),
        Some(json!({"path": other.to_string_lossy()})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = call(
        &e.app,
        Method::POST,
        &format!("/api/sessions/{sid}/relocate"),
        Some(json!({"path": "relative/dir"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, v) = call(
        &e.app,
        Method::POST,
        &format!("/api/sessions/{sid}/relocate"),
        Some(json!({"path": moved.to_string_lossy(), "dry_run": true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["relocate"]["matched"], 3);
    assert_eq!(v["relocate"]["applied"], false);
    let (status, v) = call(
        &e.app,
        Method::POST,
        &format!("/api/sessions/{sid}/relocate"),
        Some(json!({"path": moved.to_string_lossy()})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["relocate"]["applied"], true);
    for _ in 0..100 {
        let (_, v) = call(&e.app, Method::GET, &format!("/api/photos/{a_id}"), None).await;
        if v["photo"]["missing"] == false {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let (_, v) = call(&e.app, Method::GET, &format!("/api/photos/{a_id}"), None).await;
    assert_eq!(v["photo"]["missing"], false);
    assert!(v["photo"]["path"]
        .as_str()
        .unwrap()
        .contains("shoot-renamed"));

    // safe reject: needs confirmation, never deletes
    let (status, _) = call(
        &e.app,
        Method::PATCH,
        "/api/photos",
        Some(json!({"ids": [b_id], "flag": -1})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body = |confirm: bool| json!({"session_id": sid, "mode": "folder", "confirm": confirm});
    let (status, _) = call(
        &e.app,
        Method::POST,
        "/api/photos/reject-files",
        Some(body(false)),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(moved.join("b.jpg").is_file());
    let (status, v) = call(
        &e.app,
        Method::POST,
        "/api/photos/reject-files",
        Some(body(true)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["result"]["photos"], 1);
    assert!(moved.join("_rejected/b.jpg").is_file() && !moved.join("b.jpg").exists());
    assert!(moved.join("a.jpg").is_file() && moved.join("a.arw").is_file());
}
