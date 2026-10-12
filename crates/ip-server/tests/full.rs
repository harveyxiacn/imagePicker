//! `GET /api/full/{id}`: the full-resolution image of the loupe at 100%.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use ip_core::testutil::write_jpeg;
use ip_core::{Core, CoreConfig, RealImaging};
use ip_server::build_router;
use serde_json::{json, Value};
use tower::ServiceExt;

struct Resp {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: Vec<u8>,
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

/// Width and height from the first SOF marker of a JPEG.
fn jpeg_dims(b: &[u8]) -> (u32, u32) {
    assert_eq!(&b[..2], &[0xFF, 0xD8], "not a JPEG");
    let mut i = 2;
    while i + 9 < b.len() {
        assert_eq!(b[i], 0xFF, "marker expected at {i}");
        let m = b[i + 1];
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        if matches!(m, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
            let h = u16::from_be_bytes([b[i + 5], b[i + 6]]) as u32;
            let w = u16::from_be_bytes([b[i + 7], b[i + 8]]) as u32;
            return (w, h);
        }
        i += 2 + len;
    }
    panic!("no SOF marker");
}

/// Inserts an EXIF block holding only `Orientation` right after the SOI marker.
fn with_orientation(jpeg: &[u8], orientation: u16) -> Vec<u8> {
    let mut tiff = b"II*\0".to_vec();
    tiff.extend_from_slice(&8u32.to_le_bytes());
    tiff.extend_from_slice(&1u16.to_le_bytes());
    tiff.extend_from_slice(&0x0112u16.to_le_bytes());
    tiff.extend_from_slice(&3u16.to_le_bytes());
    tiff.extend_from_slice(&1u32.to_le_bytes());
    tiff.extend_from_slice(&orientation.to_le_bytes());
    tiff.extend_from_slice(&[0, 0]);
    tiff.extend_from_slice(&0u32.to_le_bytes());
    let mut payload = b"Exif\0\0".to_vec();
    payload.extend_from_slice(&tiff);
    let mut out = jpeg[..2].to_vec();
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(&payload);
    out.extend_from_slice(&jpeg[2..]);
    out
}

async fn import(app: &Router, core: &Core, dir: &Path) -> Vec<Value> {
    let r = call(
        app,
        Method::POST,
        "/api/import",
        Some(json!({"path": dir.to_string_lossy()})),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let sid = serde_json::from_slice::<Value>(&r.body).unwrap()["session"]["id"]
        .as_i64()
        .unwrap();
    core.wait_session_ready(sid, Duration::from_secs(30))
        .await
        .unwrap();
    let r = call(
        app,
        Method::GET,
        &format!("/api/photos?session_id={sid}&sort=name"),
        None,
    )
    .await;
    let v: Value = serde_json::from_slice(&r.body).unwrap();
    v["photos"].as_array().unwrap().clone()
}

#[tokio::test]
async fn full_resolution_route() {
    let data = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: Arc::new(RealImaging),
        thumb_workers: Some(2),
        worker: None,
        renderer: None,
        force_cpu: false,
    })
    .unwrap();
    let app = build_router(core.clone(), None);

    // a: larger than the 4096 preview; b: stored 600x400 with EXIF orientation 6;
    // c: damaged after import; d: deleted after import
    write_jpeg(&src.path().join("a_wide.jpg"), 5000, 300, 60);
    write_jpeg(&src.path().join("b_rotated.jpg"), 600, 400, 120);
    let b = src.path().join("b_rotated.jpg");
    std::fs::write(&b, with_orientation(&std::fs::read(&b).unwrap(), 6)).unwrap();
    write_jpeg(&src.path().join("c_damaged.jpg"), 320, 200, 180);
    write_jpeg(&src.path().join("d_missing.jpg"), 320, 200, 240);
    let photos = import(&app, &core, src.path()).await;
    let names: Vec<&str> = photos
        .iter()
        .map(|p| p["file_name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "a_wide.jpg",
            "b_rotated.jpg",
            "c_damaged.jpg",
            "d_missing.jpg"
        ]
    );
    let id = |i: usize| photos[i]["id"].as_i64().unwrap();
    let v = photos[0]["thumb_version"].as_str().unwrap();

    // native size, beyond the 4096 preview
    let r = call(
        &app,
        Method::GET,
        &format!("/api/full/{}?v={v}", id(0)),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.headers[header::CONTENT_TYPE], "image/jpeg");
    assert_eq!(r.headers[header::CACHE_CONTROL], "public, max-age=3600");
    assert_eq!(jpeg_dims(&r.body), (5000, 300));
    let p = call(
        &app,
        Method::GET,
        &format!("/api/preview/{}?s=4096", id(0)),
        None,
    )
    .await;
    assert_eq!(jpeg_dims(&p.body).0, 4096);

    // EXIF orientation applied: upright 400x600, like the photo's reported size
    assert_eq!(
        (photos[1]["width"].as_u64(), photos[1]["height"].as_u64()),
        (Some(400), Some(600))
    );
    let r = call(&app, Method::GET, &format!("/api/full/{}", id(1)), None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(jpeg_dims(&r.body), (400, 600));

    // cache hit: served from the cache even once the original is gone
    let first = r.body;
    std::fs::remove_file(&b).unwrap();
    let r = call(&app, Method::GET, &format!("/api/full/{}", id(1)), None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body, first);

    // 415: the original exists but cannot be decoded
    std::fs::write(src.path().join("c_damaged.jpg"), b"garbage, not a JPEG").unwrap();
    let r = call(&app, Method::GET, &format!("/api/full/{}", id(2)), None).await;
    assert_eq!(r.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let e: Value = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(e["error"]["code"], "unsupported_media");

    // 404: unknown photo, or the original is missing (and nothing cached)
    std::fs::remove_file(src.path().join("d_missing.jpg")).unwrap();
    for uri in [
        "/api/full/424242".to_string(),
        format!("/api/full/{}", id(3)),
    ] {
        let r = call(&app, Method::GET, &uri, None).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{uri}");
        let e: Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(e["error"]["code"], "not_found", "{uri}");
    }
    let r = call(&app, Method::GET, "/api/full/abc", None).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}
