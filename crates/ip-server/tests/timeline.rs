//! HTTP-level tests of the library-wide timeline `GET /api/timeline` (docs/api-contract-m2.md
//! C.6). FakeImaging, synthetic images only: the capture time of a test photo is its mtime.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use ip_core::testutil::{write_jpeg, FakeImaging, FakeWorker};
use ip_core::{Core, CoreConfig};
use ip_server::build_router;
use serde_json::{json, Value};
use tower::ServiceExt;

/// 2023-11-14T22:13:20Z
const BASE: u64 = 1_700_000_000;
const H: u64 = 3_600;
const DAY: u64 = 86_400;

struct Env {
    app: Router,
    core: Arc<Core>,
    one: tempfile::TempDir,
    two: tempfile::TempDir,
    _data: tempfile::TempDir,
}

fn env() -> Env {
    let data = tempfile::tempdir().unwrap();
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(2),
        worker: Some(Arc::new(FakeWorker::new())),
        renderer: None,
        force_cpu: false,
    })
    .unwrap();
    Env {
        app: build_router(core.clone(), None),
        core,
        one: tempfile::tempdir().unwrap(),
        two: tempfile::tempdir().unwrap(),
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

fn photo(dir: &std::path::Path, name: &str, t_s: u64, shade: u8) {
    let p = dir.join(name);
    write_jpeg(&p, 320, 240, shade);
    let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
    f.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(BASE + t_s))
        .unwrap();
}

async fn import(e: &Env, dir: &std::path::Path) -> i64 {
    let (st, v) = call(
        &e.app,
        Method::POST,
        "/api/import",
        Some(json!({"path": dir.to_string_lossy()})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let sid = v["session"]["id"].as_i64().unwrap();
    e.core
        .wait_session_ready(sid, Duration::from_secs(60))
        .await
        .unwrap();
    sid
}

async fn get(e: &Env, q: &str) -> (StatusCode, Value) {
    call(&e.app, Method::GET, &format!("/api/timeline{q}"), None).await
}

#[tokio::test]
async fn timeline_shape_spans_sessions_and_combines_filters() {
    let e = env();
    // session one: two photos on 2023-11-14 and one 3 h later (11-15 UTC); session two: 40 days later
    photo(e.one.path(), "a.jpg", 0, 10);
    photo(e.one.path(), "b.jpg", 600, 20);
    photo(e.one.path(), "c.jpg", 3 * H, 30);
    photo(e.two.path(), "d.jpg", 40 * DAY, 40);
    let s1 = import(&e, e.one.path()).await;
    let s2 = import(&e, e.two.path()).await;

    let (st, v) = get(&e, "?granularity=month").await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["granularity"], "month");
    assert_eq!(v["total"], 4);
    let buckets = v["buckets"].as_array().unwrap();
    let got: Vec<(String, i64)> = buckets
        .iter()
        .map(|b| {
            (
                b["key"].as_str().unwrap().into(),
                b["count"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![("2023-11".to_string(), 3), ("2023-12".to_string(), 1)]
    );
    let b0 = &buckets[0];
    assert_eq!(
        (b0["from"].as_str(), b0["to"].as_str()),
        (Some("2023-11-01"), Some("2023-11-30"))
    );
    assert_eq!(b0["photo_ids"].as_array().unwrap().len(), 3);
    assert_eq!(v["undated"], json!({"count": 0, "photo_ids": []}));
    assert_eq!(v["events"], json!([]));

    // the default granularity is month; day buckets tile 3 + 1 photos
    let (_, d) = get(&e, "?granularity=day").await;
    assert_eq!(d["buckets"].as_array().unwrap().len(), 3);
    let (st, dflt) = get(&e, "").await;
    assert_eq!(
        (st, dflt["granularity"].as_str()),
        (StatusCode::OK, Some("month"))
    );
    let (_, y) = get(&e, "?granularity=year").await;
    assert_eq!(y["buckets"].as_array().unwrap().len(), 1);
    assert_eq!(y["buckets"][0]["count"], 4);

    // one session
    let (_, one) = get(&e, &format!("?granularity=month&session_id={s2}")).await;
    assert_eq!(one["total"], 1);
    let (_, one) = get(&e, &format!("?granularity=month&session_id={s1}")).await;
    assert_eq!(one["total"], 3);

    // a window in local dates, with the photo filters of the grid
    let (_, w) = get(&e, "?granularity=day&from=2023-11-15&to=2023-12-31").await;
    assert_eq!(w["total"], 2);
    assert_eq!(
        (w["from"].as_str(), w["to"].as_str()),
        (Some("2023-11-15"), Some("2023-12-31"))
    );

    let ids: Vec<i64> = b0["photo_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i.as_i64().unwrap())
        .collect();
    let (st, _) = call(
        &e.app,
        Method::PATCH,
        "/api/photos",
        Some(json!({"ids": [ids[0]], "user_rating": 5})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (_, f) = get(&e, "?granularity=month&rating_gte=5").await;
    assert_eq!(f["total"], 1);
    let (_, f) = get(&e, "?granularity=month&rating_gte=5&taken_from=2023-12-01").await;
    assert_eq!(f["total"], 0);
    // the best-rated photo is the first representative of its bucket
    let (_, again) = get(&e, "?granularity=month").await;
    assert_eq!(again["buckets"][0]["photo_ids"][0], ids[0]);
}

#[tokio::test]
async fn timeline_rejects_bad_parameters() {
    let e = env();
    photo(e.one.path(), "a.jpg", 0, 10);
    import(&e, e.one.path()).await;
    for q in [
        "?granularity=week",
        "?granularity=",
        "?from=2023-13-01",
        "?from=2023-1-1",
        "?to=yesterday",
        "?from=2023-11-15&to=2023-11-14",
        "?rating_gte=9",
        "?flag=maybe",
        "?session_id=abc",
        "?taken_from=2023-11-15&taken_to=2023-11-14",
    ] {
        let (st, v) = get(&e, q).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{q}: {v}");
        assert!(
            v["error"].is_object() || v["code"].is_string() || v.is_object(),
            "{q}: {v}"
        );
    }
    let (st, _) = get(&e, "?session_id=9999").await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = call(&e.app, Method::POST, "/api/timeline", None).await;
    assert_eq!(st, StatusCode::METHOD_NOT_ALLOWED);
}
