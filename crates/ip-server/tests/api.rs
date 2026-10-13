//! Integration tests: router via `oneshot`, WebSocket over an ephemeral port.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use futures_util::StreamExt;
use http_body_util::BodyExt;
use ip_core::testutil::{make_photos, FakeImaging};
use ip_core::{Core, CoreConfig};
use ip_server::{build_router, spawn_with_core, ServerConfig};
use serde_json::{json, Value};
use tower::ServiceExt;

struct Env {
    core: Arc<Core>,
    app: Router,
    src: tempfile::TempDir,
    _data: tempfile::TempDir,
}

fn env_with_web(web_dir: Option<std::path::PathBuf>) -> Env {
    let data = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(2),
        worker: None,
        renderer: None,
        force_cpu: false,
    })
    .unwrap();
    let app = build_router(core.clone(), web_dir);
    Env {
        core,
        app,
        src,
        _data: data,
    }
}

fn env() -> Env {
    env_with_web(None)
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
    call_with(app, method, uri, body, &[]).await
}

async fn call_with(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
    headers: &[(&str, &str)],
) -> Resp {
    let mut b = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let req = match body {
        Some(v) => b
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
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

async fn import(env: &Env, n: usize) -> i64 {
    make_photos(env.src.path(), n);
    let r = call(
        &env.app,
        Method::POST,
        "/api/import",
        Some(json!({"path": env.src.path().to_string_lossy()})),
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let sid = r.json()["session"]["id"].as_i64().unwrap();
    env.core
        .wait_session_ready(sid, Duration::from_secs(30))
        .await
        .unwrap();
    sid
}

#[tokio::test]
async fn health() {
    let e = env();
    let r = call(&e.app, Method::GET, "/api/health", None).await;
    assert_eq!(r.status, StatusCode::OK);
    let v = r.json();
    assert_eq!(v["ok"], true);
    assert!(v["version"].is_string());
}

#[tokio::test]
async fn import_then_list() {
    let e = env();
    let sid = import(&e, 12).await;

    let r = call(&e.app, Method::GET, &format!("/api/sessions/{sid}"), None).await;
    let s = &r.json()["session"];
    assert_eq!(s["photo_count"], 12);
    assert_eq!(s["import_state"], "ready");
    assert_eq!(s["picked_count"], 0);
    assert!(s["cover_photo_id"].is_i64());

    let r = call(&e.app, Method::GET, "/api/sessions", None).await;
    assert_eq!(r.json()["sessions"].as_array().unwrap().len(), 1);

    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/photos?session_id={sid}&limit=5"),
        None,
    )
    .await;
    let v = r.json();
    assert_eq!(v["total"], 12);
    assert_eq!(v["photos"].as_array().unwrap().len(), 5);
    let cursor = v["next_cursor"].as_str().unwrap().to_string();
    let p = &v["photos"][0];
    for key in [
        "id",
        "session_id",
        "path",
        "file_name",
        "format",
        "file_size",
        "width",
        "height",
        "taken_at",
        "camera",
        "lens",
        "focal_mm",
        "aperture",
        "shutter_s",
        "iso",
        "user_rating",
        "ai_rating",
        "flag",
        "color_label",
        "burst_id",
        "thumb_ready",
        "thumb_version",
    ] {
        assert!(p.get(key).is_some(), "photo is missing {key}");
    }
    assert_eq!(p["thumb_ready"], true);
    assert_eq!(p["flag"], 0);
    assert!(p["user_rating"].is_null());

    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/photos?session_id={sid}&limit=100&cursor={cursor}&sort=taken_at"),
        None,
    )
    .await;
    let v = r.json();
    assert_eq!(v["photos"].as_array().unwrap().len(), 7);
    assert!(v["next_cursor"].is_null());

    let id = p["id"].as_i64().unwrap();
    let r = call(&e.app, Method::GET, &format!("/api/photos/{id}"), None).await;
    assert_eq!(r.json()["photo"]["id"], id);

    // filters and validation
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/photos?session_id={sid}&flag=picked"),
        None,
    )
    .await;
    assert_eq!(r.json()["total"], 0);
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/photos?session_id={sid}&flag=bogus"),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.json()["error"]["code"], "bad_request");
    let r = call(&e.app, Method::GET, "/api/photos", None).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // delete session
    let r = call(
        &e.app,
        Method::DELETE,
        &format!("/api/sessions/{sid}"),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let r = call(&e.app, Method::GET, &format!("/api/sessions/{sid}"), None).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn json_errors() {
    let e = env();
    for (method, uri) in [
        (Method::GET, "/api/nope"),
        (Method::GET, "/api/photos/424242"),
        (Method::GET, "/api/sessions/424242"),
        (Method::GET, "/api/thumb/424242"),
        (Method::GET, "/api/original/424242"),
    ] {
        let r = call(&e.app, method, uri, None).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{uri}");
        let v = r.json();
        assert_eq!(v["error"]["code"], "not_found", "{uri}");
        assert!(v["error"]["message"].is_string());
    }
    let r = call(&e.app, Method::GET, "/api/photos/abc", None).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.json()["error"]["code"], "bad_request");

    let r = call(
        &e.app,
        Method::POST,
        "/api/import",
        Some(json!({"path": e.src.path().join("definitely-not-here").to_string_lossy()})),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.json()["error"]["code"], "bad_request");

    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/import")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{not json"))
        .unwrap();
    let resp = e.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["error"]["code"],
        "bad_request"
    );

    let r = call(&e.app, Method::DELETE, "/api/health", None).await;
    assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(r.json()["error"]["code"], "method_not_allowed");
}

#[tokio::test]
async fn thumb_preview_original() {
    let e = env();
    let sid = import(&e, 3).await;
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/photos?session_id={sid}"),
        None,
    )
    .await;
    let p = r.json()["photos"][0].clone();
    let id = p["id"].as_i64().unwrap();
    let v = p["thumb_version"].as_str().unwrap();

    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/thumb/{id}?s=256&v={v}"),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.headers[header::CONTENT_TYPE], "image/jpeg");
    assert_eq!(
        r.headers[header::CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );
    assert_eq!(&r.body[..3], &[0xFF, 0xD8, 0xFF]);

    // 512 is generated on demand on a cache miss
    let r = call(&e.app, Method::GET, &format!("/api/thumb/{id}?s=512"), None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(&r.body[..2], &[0xFF, 0xD8]);
    let r = call(&e.app, Method::GET, &format!("/api/thumb/{id}"), None).await;
    assert_eq!(r.status, StatusCode::OK, "s defaults to 256");
    let r = call(&e.app, Method::GET, &format!("/api/thumb/{id}?s=999"), None).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/preview/{id}?s=1024"),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.headers[header::CONTENT_TYPE], "image/jpeg");
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/preview/{id}?s=300"),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // original, with and without Range
    let full = std::fs::read(p["path"].as_str().unwrap()).unwrap();
    let r = call(&e.app, Method::GET, &format!("/api/original/{id}"), None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.headers[header::CONTENT_TYPE], "image/jpeg");
    assert_eq!(r.body, full);
    let r = call_with(
        &e.app,
        Method::GET,
        &format!("/api/original/{id}"),
        None,
        &[("range", "bytes=0-9")],
    )
    .await;
    assert_eq!(r.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(r.body, &full[..10]);
    assert!(r.headers[header::CONTENT_RANGE]
        .to_str()
        .unwrap()
        .starts_with("bytes 0-9/"));

    let r = call(
        &e.app,
        Method::POST,
        "/api/viewport",
        Some(json!({"ids": [id, 999999]})),
    )
    .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn patch_emits_photos_updated_over_websocket() {
    let e = env();
    let sid = import(&e, 4).await;
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

    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/photos?session_id={sid}"),
        None,
    )
    .await;
    let ids: Vec<i64> = r.json()["photos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_i64().unwrap())
        .collect();

    // give the server task a moment to subscribe, then fire several quick patches
    tokio::time::sleep(Duration::from_millis(150)).await;
    for rating in 1..=3 {
        let r = call(
            &e.app,
            Method::PATCH,
            "/api/photos",
            Some(json!({"ids": [ids[0], ids[1]], "user_rating": rating, "flag": 1})),
        )
        .await;
        assert_eq!(r.status, StatusCode::OK);
        assert_eq!(r.json()["updated"], 2);
    }

    let mut frames: Vec<Value> = Vec::new();
    let got = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let msg = ws.next().await.unwrap().unwrap();
            let v: Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
            let done = v["type"] == "photos.updated";
            frames.push(v);
            if done {
                break;
            }
        }
    })
    .await;
    assert!(got.is_ok(), "no photos.updated within 5s: {frames:?}");
    let ev = frames.last().unwrap();
    assert_eq!(ev["type"], "photos.updated");
    let items = ev["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "coalesced to one entry per photo");
    let first = items.iter().find(|i| i["id"] == ids[0]).unwrap();
    assert_eq!(first["flag"], 1);
    assert!(first["color_label"].is_null());
    assert!(first["user_rating"].is_i64());

    // a second event type arrives as its own frame; PATCH response matches DB
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/photos/{}", ids[0]),
        None,
    )
    .await;
    assert_eq!(r.json()["photo"]["user_rating"], 3);

    // WS viewport message is accepted (no reply, connection stays open)
    use futures_util::SinkExt;
    ws.send(tokio_tungstenite::tungstenite::Message::text(
        json!({"type":"viewport","ids":[ids[2]]}).to_string(),
    ))
    .await
    .unwrap();
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn export_over_http_reports_progress() {
    let e = env();
    let sid = import(&e, 3).await;
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/photos?session_id={sid}"),
        None,
    )
    .await;
    let ids: Vec<i64> = r.json()["photos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_i64().unwrap())
        .collect();
    let out = tempfile::tempdir().unwrap();
    let mut rx = e.core.events.subscribe();
    let r = call(
        &e.app,
        Method::POST,
        "/api/export",
        Some(json!({"ids": ids, "dest": out.path().join("o").to_string_lossy(), "long_edge": 64, "name_template": "{seq}_{name}"})),
    )
    .await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    let task_id = r.json()["task_id"].as_str().unwrap().to_string();
    let last = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Ok(ip_core::Event::TaskProgress {
                task_id: t,
                state,
                done,
                total,
                ..
            }) = rx.recv().await
            {
                if t == task_id && state == "done" {
                    break (done, total);
                }
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(last, (3, 3));
    assert_eq!(std::fs::read_dir(out.path().join("o")).unwrap().count(), 3);
    assert!(out.path().join("o").join("0001_img_000.jpg").exists());
}

#[tokio::test]
async fn export_presets_are_the_core_table() {
    let e = env();
    let r = call(&e.app, Method::GET, "/api/export/presets", None).await;
    assert_eq!(r.status, StatusCode::OK);
    let presets = r.json()["presets"].clone();
    // the table the assistant's export tool uses, served as is
    assert_eq!(
        presets,
        serde_json::to_value(ip_core::export::EXPORT_PRESETS).unwrap()
    );
    let ids: Vec<&str> = presets
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ip_core::export::EXPORT_PRESET_IDS);
    assert_eq!(
        presets[2],
        json!({"id": "xiaohongshu", "long_edge": 1920, "aspect": [3, 4], "quality": 90,
               "format": "jpeg", "strip_gps": true})
    );
    assert_eq!(presets[3]["aspect"], json!([4, 5]));
}

#[tokio::test]
async fn fs_browse() {
    let e = env();
    let r = call(&e.app, Method::GET, "/api/fs/roots", None).await;
    assert!(!r.json()["roots"].as_array().unwrap().is_empty());

    make_photos(e.src.path(), 2);
    std::fs::create_dir(e.src.path().join("Zeta")).unwrap();
    std::fs::create_dir(e.src.path().join("alpha")).unwrap();
    std::fs::create_dir(e.src.path().join(".hidden")).unwrap();
    let path = e.src.path().to_string_lossy().into_owned();
    let uri = format!(
        "/api/fs/list?path={}",
        path.replace('\\', "%5C")
            .replace(':', "%3A")
            .replace(' ', "%20")
    );
    let r = call(&e.app, Method::GET, &uri, None).await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let v = r.json();
    assert_eq!(v["image_count"], 2);
    let dirs: Vec<&str> = v["dirs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap())
        .collect();
    assert_eq!(dirs.len(), 2);
    assert!(
        dirs[0].ends_with("alpha") && dirs[1].ends_with("Zeta"),
        "{dirs:?}"
    );
    assert!(v["parent"].is_string());

    let r = call(&e.app, Method::GET, "/api/fs/list?path=relative/dir", None).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn serves_spa_with_index_fallback() {
    let web = tempfile::tempdir().unwrap();
    std::fs::write(web.path().join("index.html"), "<html>spa</html>").unwrap();
    std::fs::create_dir(web.path().join("assets")).unwrap();
    std::fs::write(web.path().join("assets").join("a.js"), "console.log(1)").unwrap();
    let e = env_with_web(Some(web.path().to_path_buf()));

    let r = call(&e.app, Method::GET, "/", None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body, b"<html>spa</html>");
    let r = call(&e.app, Method::GET, "/cull/session/3", None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body, b"<html>spa</html>");
    let r = call(&e.app, Method::GET, "/assets/a.js", None).await;
    assert_eq!(r.body, b"console.log(1)");
    // /api never falls back to the SPA
    let r = call(&e.app, Method::GET, "/api/unknown", None).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.json()["error"]["code"], "not_found");
}

#[tokio::test]
async fn ephemeral_port_is_reported() {
    let e = env();
    let server = spawn_with_core(
        e.core.clone(),
        &ServerConfig {
            port: 0,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_ne!(server.addr.port(), 0);
    // raw HTTP/1.0 request over TCP
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(server.addr).await.unwrap();
    s.write_all(b"GET /api/health HTTP/1.0\r\n\r\n")
        .await
        .unwrap();
    let mut buf = String::new();
    s.read_to_string(&mut buf).await.unwrap();
    assert!(
        buf.starts_with("HTTP/1.0 200") || buf.starts_with("HTTP/1.1 200"),
        "{buf}"
    );
    assert!(buf.contains("\"ok\":true"));
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn devices_endpoint_and_filter() {
    let e = env();
    make_photos(e.src.path(), 3);
    for i in 0..2 {
        ip_core::testutil::write_jpeg(&e.src.path().join(format!("phone_{i}.jpg")), 64, 48, 9);
    }
    ip_core::testutil::write_jpeg(&e.src.path().join("nodev_0.jpg"), 64, 48, 50);
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

    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/sessions/{sid}/devices"),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let devs = r.json()["devices"].as_array().unwrap().clone();
    assert_eq!(devs.len(), 2);
    assert_eq!(devs[0]["name"], "Fake Cam 1");
    assert_eq!(devs[0]["photo_count"], 3);
    assert_eq!(devs[0]["kind"], "unknown");
    assert_eq!(devs[1]["name"], "Apple iPhone 15 Pro");
    assert_eq!(devs[1]["kind"], "phone");
    assert_eq!(devs[1]["make"], "Apple");
    assert_eq!(devs[1]["photo_count"], 2);
    let phone = devs[1]["id"].as_i64().unwrap();

    let r = call(&e.app, Method::GET, "/api/sessions/9999/devices", None).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);

    let list = |q: String| {
        let app = e.app.clone();
        async move {
            call(
                &app,
                Method::GET,
                &format!("/api/photos?session_id={sid}&{q}"),
                None,
            )
            .await
        }
    };
    let v = list(format!("device={phone}")).await.json();
    assert_eq!(v["total"], 2);
    for p in v["photos"].as_array().unwrap() {
        assert_eq!(p["device_id"], phone);
        assert_eq!(p["device_kind"], "phone");
        assert_eq!(p["camera"], "Apple iPhone 15 Pro");
    }
    let v = list("device=none".into()).await.json();
    assert_eq!(v["total"], 1);
    assert!(v["photos"][0]["device_id"].is_null());
    assert!(v["photos"][0]["device_kind"].is_null());
    assert!(v["photos"][0]["camera"].is_null());
    let v = list(format!("device={phone},none&sort=name&flag=not_rejected"))
        .await
        .json();
    assert_eq!(v["total"], 3);
    for bad in ["device=abc", "device=1,x"] {
        assert_eq!(
            list(bad.into()).await.status,
            StatusCode::BAD_REQUEST,
            "{bad}"
        );
    }
}

#[tokio::test]
async fn filters_complete_query_parsing_and_options() {
    let e = env();
    make_photos(e.src.path(), 4); // img_000..003, 2023-11-14 22:13:20 UTC + i minutes, lens 50mm
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
    let ids: Vec<i64> = call(
        &e.app,
        Method::GET,
        &format!("/api/photos?session_id={sid}&sort=name"),
        None,
    )
    .await
    .json()["photos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids.len(), 4);
    for (i, tag) in [(0, "kyoto"), (1, "kyoto"), (1, "a,b"), (2, "food")] {
        let r = call(
            &e.app,
            Method::POST,
            "/api/photos/tags",
            Some(json!({"ids": [ids[i]], "add": [tag]})),
        )
        .await;
        assert_eq!(
            r.status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&r.body)
        );
    }

    let list = |q: &str| {
        let app = e.app.clone();
        let uri = format!("/api/photos?session_id={sid}&{q}");
        async move { call(&app, Method::GET, &uri, None).await }
    };
    let total = |q: &'static str| {
        let l = list(q);
        async move {
            let r = l.await;
            assert_eq!(
                r.status,
                StatusCode::OK,
                "{q}: {}",
                String::from_utf8_lossy(&r.body)
            );
            r.json()["total"].as_i64().unwrap()
        }
    };
    // lens: exact name, comma separated, `\,` is a comma inside a name
    assert_eq!(total("lens=50mm").await, 4);
    assert_eq!(total("lens=nope,50mm").await, 4);
    assert_eq!(total("lens=nope").await, 0);
    assert_eq!(total("lens=a%5C,b").await, 0);
    assert_eq!(total("lens=").await, 4);
    // time range: dates cover whole days (UTC wall clock), numbers are ms
    assert_eq!(total("taken_from=2023-11-14&taken_to=2023-11-14").await, 4);
    assert_eq!(total("taken_from=2023-11-15").await, 0);
    assert_eq!(total("taken_to=2023-11-13").await, 0);
    assert_eq!(
        total("taken_from=1700000060000&taken_to=1700000120000").await,
        2
    );
    assert_eq!(
        total("taken_from=2023-11-14&taken_to=1700000060000").await,
        2
    );
    // tags
    assert_eq!(total("tags=kyoto").await, 2);
    assert_eq!(total("tags=kyoto,food").await, 3);
    assert_eq!(total("tags=kyoto,food&tag_mode=all").await, 0);
    assert_eq!(total("tags=a%5C,b&tag_mode=all").await, 1);
    assert_eq!(
        total("tags=kyoto&tag_mode=any&lens=50mm&sort=-taken_at").await,
        2
    );
    // sharpness state (nothing analysed here)
    assert_eq!(total("sharp=unknown").await, 4);
    assert_eq!(total("sharp=sharp").await, 0);
    assert_eq!(total("sharp=blurry").await, 0);
    // pagination keeps the filter
    let v = list("tags=kyoto,food&limit=2&sort=name").await.json();
    assert_eq!(v["total"], 3);
    assert_eq!(v["photos"].as_array().unwrap().len(), 2);
    let cursor = v["next_cursor"].as_str().unwrap().to_string();
    let v = list(&format!(
        "tags=kyoto,food&limit=2&sort=name&cursor={cursor}"
    ))
    .await
    .json();
    assert_eq!(v["photos"].as_array().unwrap().len(), 1);
    assert!(v["next_cursor"].is_null());

    // validation errors
    for bad in [
        "taken_from=yesterday",
        "taken_from=2023-13-01",
        "taken_to=2023-02-30",
        "taken_from=2023-1-1",
        "taken_from=2023-11-15&taken_to=2023-11-14",
        "tag_mode=some",
        "sharp=crisp",
        "face_ids=abc",
        "face_ids=1,x",
        "face_ids=999999",
        "face_similarity=2",
        "face_similarity=-0.1",
        "face_similarity=high",
    ] {
        let r = list(bad).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}");
        assert_eq!(r.json()["error"]["code"], "bad_request", "{bad}");
    }
    let too_many: Vec<String> = (1..=51).map(|i| i.to_string()).collect();
    assert_eq!(
        list(&format!("face_ids={}", too_many.join(",")))
            .await
            .status,
        StatusCode::BAD_REQUEST
    );

    // filter options for the pickers
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/sessions/{sid}/filter-options"),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let o = r.json();
    assert_eq!(o["lenses"], json!([{"lens": "50mm", "photo_count": 4}]));
    assert_eq!(
        o["tags"],
        json!([
            {"tag": "kyoto", "photo_count": 2},
            {"tag": "a,b", "photo_count": 1},
            {"tag": "food", "photo_count": 1}
        ])
    );
    assert_eq!(o["taken_min"], 1_700_000_000_000_i64);
    assert_eq!(o["taken_max"], 1_700_000_180_000_i64);
    let r = call(
        &e.app,
        Method::GET,
        "/api/sessions/9999/filter-options",
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);

    // smart collections accept the new keys and validate them
    let r = call(
        &e.app,
        Method::POST,
        "/api/collections",
        Some(json!({"name": "kyoto, sharp", "query": "tags=kyoto&sharp=sharp&lens=50mm&taken_from=2023-11-14"})),
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let r = call(
        &e.app,
        Method::POST,
        "/api/collections",
        Some(json!({"name": "bad", "query": "sharp=crisp"})),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // face search: the similarity floor is validated before anything else
    for bad in [json!(2.0), json!(-0.5)] {
        let r = call(
            &e.app,
            Method::POST,
            "/api/faces/search",
            Some(json!({"face_id": 1, "min_similarity": bad})),
        )
        .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn export_watermark_over_http() {
    let e = env();
    let sid = import(&e, 2).await;
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/photos?session_id={sid}"),
        None,
    )
    .await;
    let ids: Vec<i64> = r.json()["photos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_i64().unwrap())
        .collect();
    let out = tempfile::tempdir().unwrap();
    let post = |dest: &str, wm: Option<Value>| {
        let mut body = json!({"ids": ids, "dest": dest, "format": "png"});
        if let Some(wm) = wm {
            body["watermark"] = wm;
        }
        call(&e.app, Method::POST, "/api/export", Some(body))
    };
    let dest = |n: &str| out.path().join(n).to_string_lossy().into_owned();

    // settings are validated before a task starts
    let text = |extra: Value| {
        let mut wm = json!({"kind": "text", "text": "(c) me", "opacity": 0.8});
        wm.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        wm
    };
    for (what, wm) in [
        ("opacity", text(json!({"opacity": 3}))),
        ("empty", text(json!({"text": ""}))),
        ("color", text(json!({"color": "red"}))),
        ("font_path", text(json!({"text": "水印"}))),
        ("image_path", json!({"kind": "image"})),
        (
            "nope.png",
            json!({"kind": "image", "image_path": out.path().join("nope.png").to_string_lossy()}),
        ),
    ] {
        let r = post(&dest("bad"), Some(wm)).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{what}");
        assert_eq!(r.json()["error"]["code"], "bad_request", "{what}");
        assert!(
            r.json()["error"]["message"]
                .as_str()
                .unwrap()
                .contains(what),
            "{what}: {}",
            r.json()
        );
    }
    let r = post(
        &dest("bad"),
        Some(json!({"kind": "image", "image_path": "logo.png"})),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "relative logo path");
    let r = post(&dest("bad"), Some(json!({"kind": "sticker"}))).await;
    assert!(r.status.is_client_error(), "unknown kind: {}", r.status);

    // a valid mark: the task runs and the files differ from the plain export
    let mut rx = e.core.events.subscribe();
    let mut files = Vec::new();
    for (name, wm) in [("plain", None), ("marked", Some(text(json!({}))))] {
        let r = post(&dest(name), wm).await;
        assert_eq!(r.status, StatusCode::ACCEPTED, "{name}");
        let task_id = r.json()["task_id"].as_str().unwrap().to_string();
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if let Ok(ip_core::Event::TaskProgress {
                    task_id: t, state, ..
                }) = rx.recv().await
                {
                    if t == task_id && state == "done" {
                        break;
                    }
                }
            }
        })
        .await
        .unwrap();
        let mut names: Vec<_> = std::fs::read_dir(out.path().join(name))
            .unwrap()
            .map(|d| d.unwrap().path())
            .collect();
        names.sort();
        assert_eq!(names.len(), 2, "{name}");
        files.push(std::fs::read(&names[0]).unwrap());
    }
    assert_ne!(files[0], files[1], "the watermark changed the pixels");
}

#[tokio::test]
async fn fs_list_names_files_with_the_requested_extensions() {
    let e = env();
    let dir = e.src.path().join("assets");
    std::fs::create_dir(&dir).unwrap();
    for n in ["b.PNG", "a.ttf", "c.txt", ".hidden.png"] {
        std::fs::write(dir.join(n), b"x").unwrap();
    }
    let enc = |p: &str| {
        p.replace('\\', "%5C")
            .replace(':', "%3A")
            .replace(' ', "%20")
    };
    let path = enc(&dir.to_string_lossy());
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/fs/list?path={path}&ext=.png,ttf"),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let files: Vec<String> = r.json()["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            std::path::Path::new(f.as_str().unwrap())
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(files, ["a.ttf", "b.PNG"]);
    // without `ext` the key is absent
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/fs/list?path={path}"),
        None,
    )
    .await;
    assert!(r.json().get("files").is_none());
}
