//! Cached image serving (validators, Range, cache policy), response compression and error-body
//! hygiene of `ip-server`. FakeImaging + FakeWorker + FakeRenderer; no real photo library.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::response::IntoResponse;
use axum::Router;
use http_body_util::BodyExt;
use ip_core::ip_render::Backend;
use ip_core::testutil::{make_photos, FakeImaging, FakeRenderer, FakeWorker};
use ip_core::{Core, CoreConfig, CoreError};
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
        renderer: Some(Arc::new(FakeRenderer::new(Backend::Gpu))),
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
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("not json ({e}): {}", String::from_utf8_lossy(&self.body)))
    }
    /// All values of the header joined with ", " (empty when absent).
    fn header(&self, name: impl header::AsHeaderName) -> String {
        self.headers
            .get_all(name)
            .iter()
            .map(|v| v.to_str().unwrap())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

async fn call(
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

async fn get(app: &Router, uri: &str) -> Resp {
    call(app, Method::GET, uri, None, &[]).await
}

/// Imports `n` generated photos; returns (session id, the photo rows).
async fn import(e: &Env, n: usize) -> (i64, Vec<Value>) {
    make_photos(e.src.path(), n);
    let r = call(
        &e.app,
        Method::POST,
        "/api/import",
        Some(json!({"path": e.src.path().to_string_lossy()})),
        &[],
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let sid = r.json()["session"]["id"].as_i64().unwrap();
    e.core
        .wait_session_ready(sid, Duration::from_secs(600))
        .await
        .unwrap();
    let r = get(&e.app, &format!("/api/photos?session_id={sid}&limit=5000")).await;
    (sid, r.json()["photos"].as_array().unwrap().clone())
}

fn pid(p: &Value) -> i64 {
    p["id"].as_i64().unwrap()
}

fn urls(p: &Value) -> [String; 3] {
    let id = pid(p);
    let v = p["thumb_version"].as_str().unwrap();
    [
        format!("/api/thumb/{id}?s=256&v={v}"),
        format!("/api/preview/{id}?s=1024&v={v}"),
        format!("/api/full/{id}?v={v}"),
    ]
}

#[tokio::test]
async fn cached_images_revalidate_with_304_and_support_range() {
    let e = env();
    let (_, photos) = import(&e, 2).await;
    for url in urls(&photos[0]) {
        let r = get(&e.app, &url).await;
        assert_eq!(r.status, StatusCode::OK, "{url}");
        assert_eq!(r.header(header::CONTENT_TYPE), "image/jpeg", "{url}");
        assert_eq!(
            r.header(header::CONTENT_LENGTH),
            r.body.len().to_string(),
            "{url}"
        );
        assert_eq!(r.header(header::ACCEPT_RANGES), "bytes", "{url}");
        let (etag, lm) = (r.header(header::ETAG), r.header(header::LAST_MODIFIED));
        assert!(
            etag.starts_with('"') && etag.ends_with('"'),
            "{url}: {etag}"
        );
        assert!(!lm.is_empty(), "{url}: Last-Modified");

        // repeated conditional request: 304 without a body, validators and policy repeated
        let r2 = call(
            &e.app,
            Method::GET,
            &url,
            None,
            &[("if-none-match", etag.as_str())],
        )
        .await;
        assert_eq!(r2.status, StatusCode::NOT_MODIFIED, "{url}");
        assert!(r2.body.is_empty(), "{url}");
        assert_eq!(r2.header(header::ETAG), etag, "{url}");
        assert!(!r2.header(header::CACHE_CONTROL).is_empty(), "{url}");
        let r3 = call(
            &e.app,
            Method::GET,
            &url,
            None,
            &[("if-modified-since", lm.as_str())],
        )
        .await;
        assert_eq!(r3.status, StatusCode::NOT_MODIFIED, "{url}: IMS");
        // another validator never matches
        let r4 = call(
            &e.app,
            Method::GET,
            &url,
            None,
            &[("if-none-match", "\"nope\"")],
        )
        .await;
        assert_eq!(r4.status, StatusCode::OK, "{url}");

        // Range: partial body, then If-Range with a stale validator gets the whole file
        let r5 = call(&e.app, Method::GET, &url, None, &[("range", "bytes=0-9")]).await;
        assert_eq!(r5.status, StatusCode::PARTIAL_CONTENT, "{url}");
        assert_eq!(r5.body, &r.body[..10], "{url}");
        assert!(r5.header(header::CONTENT_RANGE).starts_with("bytes 0-9/"));
        let r6 = call(
            &e.app,
            Method::GET,
            &url,
            None,
            &[("range", "bytes=0-9"), ("if-range", "\"stale\"")],
        )
        .await;
        assert_eq!(r6.status, StatusCode::OK, "{url}: If-Range");
        assert_eq!(r6.body, r.body, "{url}");
    }
}

#[tokio::test]
async fn cache_policy_is_private_and_immutable_only_with_a_version() {
    let e = env();
    let (_, photos) = import(&e, 1).await;
    let id = pid(&photos[0]);
    let v = photos[0]["thumb_version"].as_str().unwrap();
    for (bare, versioned) in [
        (
            format!("/api/thumb/{id}?s=256"),
            format!("/api/thumb/{id}?s=256&v={v}"),
        ),
        (
            format!("/api/preview/{id}?s=1024"),
            format!("/api/preview/{id}?s=1024&v={v}"),
        ),
        (format!("/api/full/{id}"), format!("/api/full/{id}?v={v}")),
    ] {
        let cc = get(&e.app, &versioned).await.header(header::CACHE_CONTROL);
        assert!(cc.starts_with("private,"), "{versioned}: {cc}");
        assert!(cc.contains("immutable"), "{versioned}: {cc}");
        assert!(cc.contains("max-age=31536000"), "{versioned}: {cc}");
        let cc = get(&e.app, &bare).await.header(header::CACHE_CONTROL);
        assert!(cc.starts_with("private,"), "{bare}: {cc}");
        assert!(!cc.contains("immutable"), "{bare}: {cc}");
        assert!(!cc.contains("max-age=31536000"), "{bare}: {cc}");
        assert!(cc.contains("max-age="), "{bare}: {cc}");
    }
    // an empty `v=` is no version
    let cc = get(&e.app, &format!("/api/thumb/{id}?s=256&v="))
        .await
        .header(header::CACHE_CONTROL);
    assert!(!cc.contains("immutable"), "{cc}");
}

fn solid(dir: &std::path::Path, name: &str, rgb: [u8; 3]) {
    image::RgbImage::from_pixel(160, 120, image::Rgb(rgb))
        .save_with_format(dir.join(name), image::ImageFormat::Jpeg)
        .unwrap();
}

/// A 304 must never pin a stale image: the validator follows the edit state, so clearing an
/// edit (which switches back to an OLDER cache file) is not answered with 304 for the edited one.
#[tokio::test]
async fn validators_follow_edits_including_a_revert() {
    let e = env();
    solid(e.src.path(), "a.jpg", [100, 100, 100]);
    let r = call(
        &e.app,
        Method::POST,
        "/api/import",
        Some(json!({"path": e.src.path().to_string_lossy()})),
        &[],
    )
    .await;
    let sid = r.json()["session"]["id"].as_i64().unwrap();
    e.core
        .wait_session_ready(sid, Duration::from_secs(60))
        .await
        .unwrap();
    let r = get(&e.app, &format!("/api/photos?session_id={sid}")).await;
    let id = pid(&r.json()["photos"][0]);
    let url = format!("/api/thumb/{id}?s=256");
    let url_full = format!("/api/full/{id}");

    let before = get(&e.app, &url).await;
    let before_full = get(&e.app, &url_full).await;
    // let the file clock tick so an edited file is strictly newer than the original one
    tokio::time::sleep(Duration::from_millis(1200)).await;
    call(
        &e.app,
        Method::PUT,
        &format!("/api/edits/{id}"),
        Some(json!({"stack": {"version":1,"ops":[{"type":"global","exposure":1.5}]}})),
        &[],
    )
    .await;
    let edited = get(&e.app, &url).await;
    let edited_full = get(&e.app, &url_full).await;
    assert_ne!(edited.body, before.body);
    assert_ne!(edited.header(header::ETAG), before.header(header::ETAG));
    assert_ne!(
        edited_full.header(header::ETAG),
        before_full.header(header::ETAG)
    );
    let old = before.header(header::ETAG);
    let r = call(&e.app, Method::GET, &url, None, &[("if-none-match", &old)]).await;
    assert_eq!(r.status, StatusCode::OK, "old validator vs edited image");

    call(
        &e.app,
        Method::DELETE,
        &format!("/api/edits/{id}"),
        None,
        &[],
    )
    .await;
    // browser holds the edited thumbnail: neither validator may answer 304 now
    let (etag, lm) = (
        edited.header(header::ETAG),
        edited.header(header::LAST_MODIFIED),
    );
    for hdr in [("if-none-match", etag.as_str()), ("if-modified-since", &lm)] {
        let r = call(&e.app, Method::GET, &url, None, &[hdr]).await;
        assert_eq!(
            r.status,
            StatusCode::OK,
            "{hdr:?} after the edit was cleared"
        );
        assert_eq!(r.body, before.body, "{hdr:?}");
    }
    // the full image is touched on use, yet its validator stays stable between requests
    let a = get(&e.app, &url_full).await;
    let b = get(&e.app, &url_full).await;
    assert_eq!(a.header(header::ETAG), b.header(header::ETAG));
    assert_eq!(a.header(header::ETAG), before_full.header(header::ETAG));
}

/// Gzip on large JSON, none on images or small bodies. Prints the wire sizes (`--nocapture`).
#[tokio::test]
async fn large_json_is_gzipped_images_are_not() {
    let n: usize = std::env::var("SERVING_TEST_PHOTOS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1500);
    let e = env();
    let (sid, photos) = import(&e, n).await;
    assert_eq!(photos.len(), n);
    let uri = format!("/api/photos?session_id={sid}&limit=5000");

    let t_plain = Instant::now();
    let plain = call(
        &e.app,
        Method::GET,
        &uri,
        None,
        &[("accept-encoding", "identity")],
    )
    .await;
    let took_plain = t_plain.elapsed();
    assert_eq!(plain.status, StatusCode::OK);
    assert_eq!(plain.header(header::CONTENT_ENCODING), "");
    let t = Instant::now();
    let gz = call(
        &e.app,
        Method::GET,
        &uri,
        None,
        &[("accept-encoding", "gzip")],
    )
    .await;
    let took = t.elapsed();
    assert_eq!(gz.status, StatusCode::OK);
    assert_eq!(gz.header(header::CONTENT_ENCODING), "gzip");
    assert!(gz
        .header(header::VARY)
        .to_lowercase()
        .contains("accept-encoding"));
    assert_eq!(&gz.body[..2], &[0x1f, 0x8b], "gzip magic");
    assert!(
        gz.body.len() * 3 < plain.body.len(),
        "gzip must shrink JSON a lot"
    );
    eprintln!(
        "WIRE photos={} identity={} bytes, gzip={} bytes ({:.1}%), request {:?} (identity {:?})",
        photos.len(),
        plain.body.len(),
        gz.body.len(),
        100.0 * gz.body.len() as f64 / plain.body.len() as f64,
        took,
        took_plain
    );

    // images keep their bytes whatever the client accepts
    let [thumb, _, _] = urls(&photos[0]);
    let img = call(
        &e.app,
        Method::GET,
        &thumb,
        None,
        &[("accept-encoding", "gzip, br")],
    )
    .await;
    assert_eq!(img.header(header::CONTENT_ENCODING), "");
    assert_eq!(&img.body[..3], &[0xFF, 0xD8, 0xFF]);
    // small JSON stays plain, and so do errors (their shape is the plain JSON error)
    let h = call(
        &e.app,
        Method::GET,
        "/api/health",
        None,
        &[("accept-encoding", "gzip")],
    )
    .await;
    assert_eq!(h.header(header::CONTENT_ENCODING), "");
    let nf = call(
        &e.app,
        Method::GET,
        "/api/photos/424242",
        None,
        &[("accept-encoding", "gzip")],
    )
    .await;
    assert_eq!(nf.status, StatusCode::NOT_FOUND);
    assert_eq!(nf.json()["error"]["code"], "not_found");
}

fn assert_path_free(what: &str, text: &str) {
    assert!(
        !text.contains('/') && !text.contains('\\'),
        "{what}: {text}"
    );
    let b = text.as_bytes();
    assert!(
        !b.windows(2)
            .any(|w| w[0].is_ascii_alphabetic() && w[1] == b':'),
        "{what}: drive letter in {text}"
    );
}

#[tokio::test]
async fn error_bodies_do_not_leak_server_paths() {
    let e = env();
    let (_, photos) = import(&e, 2).await;
    // a missing original: via the original route and via a not-yet-cached preview
    let p = &photos[1];
    let id = pid(p);
    std::fs::remove_file(p["path"].as_str().unwrap()).unwrap();
    for uri in [
        format!("/api/original/{id}"),
        format!("/api/preview/{id}?s=4096"),
    ] {
        let r = get(&e.app, &uri).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{uri}");
        assert_eq!(r.json()["error"]["code"], "not_found");
        let msg = r.json()["error"]["message"].as_str().unwrap().to_string();
        assert!(msg.contains("missing"), "{msg}");
        assert_path_free(&uri, &String::from_utf8_lossy(&r.body));
    }

    // an internal error carries the full anyhow chain in the log, not in the body
    let err = CoreError::Internal(
        anyhow::anyhow!("open C:\\Users\\ann\\AppData\\catalog.db")
            .context("read /home/ann/photos/a.jpg"),
    );
    let resp = ip_server::error::ApiError::from(err).into_response();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["error"]["code"], "internal");
    assert_path_free("internal", &String::from_utf8_lossy(&body));
}
