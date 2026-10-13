//! HTTP-level tests of the event layer and the duplicate detection (docs/api-contract-m2.md C.6).
//! FakeImaging + FakeWorker, synthetic images only.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use ip_core::testutil::{write_jpeg, FakeImaging, FakeSpec, FakeWorker};
use ip_core::{Core, CoreConfig};
use ip_server::build_router;
use serde_json::{json, Value};
use tower::ServiceExt;

/// 2023-11-14T22:13:20Z
const BASE: u64 = 1_700_000_000;
const MIN: u64 = 60;
const H: u64 = 3_600;
const P: u64 = 0x0F0F_3C3C_A5A5_5A5A;

struct Env {
    worker: Arc<FakeWorker>,
    app: Router,
    core: Arc<Core>,
    one: tempfile::TempDir,
    two: tempfile::TempDir,
    _data: tempfile::TempDir,
}

fn env() -> Env {
    let data = tempfile::tempdir().unwrap();
    let worker = Arc::new(FakeWorker::new());
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(2),
        worker: Some(worker.clone()),
        renderer: None,
        force_cpu: false,
    })
    .unwrap();
    let app = build_router(core.clone(), None);
    Env {
        worker,
        app,
        core,
        one: tempfile::tempdir().unwrap(),
        two: tempfile::tempdir().unwrap(),
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

fn photo(dir: &std::path::Path, name: &str, t_s: u64, shade: u8) {
    let p = dir.join(name);
    write_jpeg(&p, 320, 240, shade);
    let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
    f.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(BASE + t_s))
        .unwrap();
}

async fn import(e: &Env, dir: &std::path::Path) -> i64 {
    let r = call(
        &e.app,
        Method::POST,
        "/api/import",
        Some(json!({"path": dir.to_string_lossy()})),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let sid = r.json()["session"]["id"].as_i64().unwrap();
    e.core
        .wait_session_ready(sid, Duration::from_secs(60))
        .await
        .unwrap();
    sid
}

async fn analyse(e: &Env, sid: i64) {
    let r = call(
        &e.app,
        Method::POST,
        "/api/analysis/run",
        Some(json!({"session_id": sid, "profile": "standard"})),
    )
    .await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    let t = Instant::now();
    loop {
        let s = call(
            &e.app,
            Method::GET,
            &format!("/api/analysis/status?session_id={sid}"),
            None,
        )
        .await
        .json();
        if s["state"] != "running" {
            assert_eq!(s["state"], "done", "{s}");
            return;
        }
        assert!(
            t.elapsed() < Duration::from_secs(300),
            "analysis did not finish"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn photos(e: &Env, sid: i64, extra: &str) -> Vec<Value> {
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/photos?session_id={sid}&limit=500{extra}"),
        None,
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    r.json()["photos"].as_array().unwrap().clone()
}

fn names(photos: &[Value]) -> Vec<String> {
    let mut v: Vec<String> = photos
        .iter()
        .map(|p| p["file_name"].as_str().unwrap().to_string())
        .collect();
    v.sort();
    v
}

#[tokio::test]
async fn timeline_endpoint_returns_events_scenes_bursts() {
    let e = env();
    let d = e.one.path();
    // event 1: a burst and a scene 30 min later; event 2: 3 h later (pause > 2 h);
    // event 3: 11 km away (move > 5 km)
    photo(d, "a1.jpg", 0, 10);
    photo(d, "a2.jpg", 1, 20);
    photo(d, "b1.jpg", 30 * MIN, 30);
    photo(d, "geoA_c1.jpg", 30 * MIN + 3 * H, 40);
    photo(d, "geoB_d1.jpg", 30 * MIN + 3 * H + 15 * MIN, 50);
    for (n, angle) in [
        ("a1.jpg", 0.0),
        ("a2.jpg", 1.0),
        ("b1.jpg", 90.0),
        ("geoA_c1.jpg", 180.0),
        ("geoB_d1.jpg", 270.0),
    ] {
        e.worker.set(n, FakeSpec::at(angle));
    }
    let sid = import(&e, d).await;
    analyse(&e, sid).await;

    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/sessions/{sid}/timeline"),
        None,
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let v = r.json();
    let events = v["events"].as_array().unwrap();
    assert_eq!(events.len(), 3, "{v:#}");
    for ev in events {
        for k in [
            "id",
            "name",
            "title",
            "place",
            "start_at",
            "end_at",
            "offset_min",
            "gps_lat",
            "gps_lon",
            "photo_count",
            "scenes",
        ] {
            assert!(ev.get(k).is_some(), "event field {k}");
        }
        for s in ev["scenes"].as_array().unwrap() {
            assert_eq!(s["event_id"], ev["id"]);
            for k in ["id", "start_at", "end_at", "bursts"] {
                assert!(s.get(k).is_some(), "scene field {k}");
            }
            for b in s["bursts"].as_array().unwrap() {
                for k in ["id", "best_photo_id", "photo_ids", "size", "start_at"] {
                    assert!(b.get(k).is_some(), "burst field {k}");
                }
            }
        }
    }
    let counts: Vec<i64> = events
        .iter()
        .map(|ev| ev["photo_count"].as_i64().unwrap())
        .collect();
    assert_eq!(counts, [3, 1, 1]);
    assert_eq!(events[0]["name"], "2023-11-14 22:13-22:43");
    assert_eq!(events[0]["scenes"].as_array().unwrap().len(), 2);
    assert_eq!(events[0]["scenes"][0]["bursts"][0]["size"], 2);
    assert!(events[0]["title"].is_null() && events[0]["gps_lat"].is_null());
    assert!(events[1]["gps_lat"].is_number() && events[1]["gps_lon"].is_number());
    // start/end are ordered and the events do not overlap
    for w in events.windows(2) {
        assert!(w[0]["end_at"].as_i64() < w[1]["start_at"].as_i64());
    }

    // /api/groups keeps its shape: scenes without an event id
    let g = call(
        &e.app,
        Method::GET,
        &format!("/api/groups?session_id={sid}"),
        None,
    )
    .await
    .json();
    assert_eq!(g["scenes"].as_array().unwrap().len(), 4);
    assert!(g["scenes"][0].get("event_id").is_none());

    // browse by event
    let ev2 = events[1]["id"].as_i64().unwrap();
    let ph = photos(&e, sid, &format!("&event_id={ev2}")).await;
    assert_eq!(names(&ph), ["geoA_c1.jpg"]);
    let ev1 = events[0]["id"].as_i64().unwrap();
    assert_eq!(photos(&e, sid, &format!("&event_id={ev1}")).await.len(), 3);
    assert!(photos(&e, sid, "&event_id=424242").await.is_empty());
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/photos?session_id={sid}&event_id=abc"),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // errors
    let r = call(&e.app, Method::GET, "/api/sessions/999/timeline", None).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.json()["error"]["code"], "not_found");
    let r = call(&e.app, Method::GET, "/api/sessions/abc/timeline", None).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn duplicates_over_http() {
    let e = env();
    photo(e.one.path(), "orig.jpg", 0, 10);
    photo(e.one.path(), "unique.jpg", 10 * MIN, 50);
    std::fs::copy(e.one.path().join("orig.jpg"), e.two.path().join("copy.jpg")).unwrap();
    photo(e.two.path(), "noexif_orig.jpg", 20, 11);
    for (n, h) in [
        ("orig.jpg", P),
        ("copy.jpg", P),
        ("noexif_orig.jpg", P ^ 0b11),
        ("unique.jpg", !P),
    ] {
        e.worker.set(
            n,
            FakeSpec {
                phash: h,
                ..FakeSpec::at(0.0)
            },
        );
    }
    let s1 = import(&e, e.one.path()).await;
    let s2 = import(&e, e.two.path()).await;
    analyse(&e, s1).await;
    analyse(&e, s2).await;

    // filter and photo fields
    let d1 = photos(&e, s1, "&duplicates=1").await;
    let d2 = photos(&e, s2, "&duplicates=1").await;
    assert_eq!(names(&d1), ["orig.jpg"]);
    assert_eq!(names(&d2), ["copy.jpg", "noexif_orig.jpg"]);
    let orig = &d1[0];
    assert_eq!(orig["duplicate_group_id"], orig["id"]);
    assert_eq!(orig["duplicate_kind"], "exact");
    let near = d2
        .iter()
        .find(|p| p["file_name"] == "noexif_orig.jpg")
        .unwrap();
    assert_eq!(near["duplicate_group_id"], orig["id"], "across sessions");
    assert_eq!(near["duplicate_kind"], "near");
    let rest = photos(&e, s1, "&duplicates=0").await;
    assert_eq!(names(&rest), ["unique.jpg"]);
    assert!(rest[0]["duplicate_group_id"].is_null());
    assert!(rest[0].get("duplicate_kind").is_none());
    assert_eq!(photos(&e, s1, "").await.len(), 2);
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/photos?session_id={s1}&duplicates=maybe"),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // GET /api/photos/{id}/duplicates
    let id = near["id"].as_i64().unwrap();
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/photos/{id}/duplicates"),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let v = r.json();
    assert_eq!(v["group_id"], orig["id"]);
    assert_eq!(v["photo_ids"].as_array().unwrap().len(), 3);
    assert_eq!(v["exact_ids"].as_array().unwrap().len(), 2);
    let uid = rest[0]["id"].as_i64().unwrap();
    let v = call(
        &e.app,
        Method::GET,
        &format!("/api/photos/{uid}/duplicates"),
        None,
    )
    .await
    .json();
    assert!(v["group_id"].is_null() && v["photo_ids"].as_array().unwrap().is_empty());
    let r = call(&e.app, Method::GET, "/api/photos/99999/duplicates", None).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);

    // the built-in smart collection and the saved queries
    let list = call(&e.app, Method::GET, "/api/collections", None)
        .await
        .json();
    let c = list["collections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "duplicates")
        .expect("built-in collection");
    assert_eq!(
        (c["query"].as_str(), c["builtin"].as_bool()),
        (Some("duplicates=1"), Some(true))
    );
    let r = call(
        &e.app,
        Method::POST,
        "/api/collections",
        Some(json!({"name": "Dups", "query": "duplicates=1&flag=picked"})),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);
    for bad in ["duplicates=maybe", "event_id=3"] {
        let r = call(
            &e.app,
            Method::POST,
            "/api/collections",
            Some(json!({"name": "x", "query": bad})),
        )
        .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}");
    }
}
