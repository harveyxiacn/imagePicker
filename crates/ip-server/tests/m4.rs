//! HTTP-level tests of the M4 routes (FakeImaging + FakeWorker + FakeRenderer).

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use ip_core::ip_render::Backend;
use ip_core::testutil::{
    person, write_jpeg, FakeFace, FakeImaging, FakeRenderer, FakeSpec, FakeWorker, BEAUTY_MODEL,
};
use ip_core::{AnalysisRunRequest, Core, CoreConfig, Event, Profile, RunState};
use ip_server::build_router;
use serde_json::{json, Value};
use tower::ServiceExt;

const BASE: u64 = 1_700_000_000;

struct Env {
    core: Arc<Core>,
    worker: Arc<FakeWorker>,
    app: Router,
    src: tempfile::TempDir,
    _data: tempfile::TempDir,
}

fn env() -> Env {
    let data = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let worker = Arc::new(FakeWorker::new());
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(2),
        worker: Some(worker.clone()),
        renderer: Some(Arc::new(FakeRenderer::new(Backend::Cpu))),
        force_cpu: true,
    })
    .unwrap();
    let app = build_router(core.clone(), None);
    Env {
        core,
        worker,
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

async fn send(app: &Router, req: Request<Body>) -> Resp {
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

async fn call(app: &Router, method: Method, uri: &str, body: Option<Value>) -> Resp {
    let b = Request::builder().method(method).uri(uri);
    let req = match body {
        Some(v) => b
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    send(app, req).await
}

fn multipart(fields: &[(&str, Option<&str>, Vec<u8>)]) -> Request<Body> {
    let boundary = "XBOUNDARYX";
    let mut body = Vec::new();
    for (name, filename, data) in fields {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match filename {
            Some(f) => body.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"{name}\"; filename=\"{f}\"\r\nContent-Type: image/jpeg\r\n\r\n"
                )
                .as_bytes(),
            ),
            None => body.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
            ),
        }
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Request::builder()
        .method(Method::POST)
        .uri("/api/faces/search")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap()
}

fn photo(dir: &std::path::Path, name: &str, t_s: u64) {
    let p = dir.join(name);
    write_jpeg(&p, 320, 240, (t_s % 200) as u8);
    let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
    f.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(BASE + t_s))
        .unwrap();
}

struct Scene {
    sid: i64,
    ids: std::collections::HashMap<String, i64>,
    a: i64,
    b: i64,
}

/// a1 a2 (burst) with persons A and B, b1 with A, c1 without faces; a2 has closed-eye B.
async fn scene(e: &Env) -> Scene {
    for (n, t) in [
        ("a1.jpg", 0),
        ("a2.jpg", 1),
        ("b1.jpg", 3600),
        ("c1.jpg", 7200),
    ] {
        photo(e.src.path(), n, t);
    }
    let faces = |eyes_b: f64| {
        vec![
            FakeFace::new([0.2, 0.3, 0.2, 0.3], person(0, 1.0)),
            FakeFace::new([0.6, 0.3, 0.1, 0.15], person(1, 1.0)).eyes(eyes_b),
        ]
    };
    e.worker
        .set("a1.jpg", FakeSpec::at(0.0).with_faces(faces(0.9)));
    e.worker
        .set("a2.jpg", FakeSpec::at(2.0).with_faces(faces(0.1)));
    e.worker.set(
        "b1.jpg",
        FakeSpec::at(90.0).with_faces(vec![FakeFace::new([0.2, 0.3, 0.2, 0.3], person(0, 2.0))]),
    );
    e.worker.set("c1.jpg", FakeSpec::at(200.0));
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
    e.core
        .analysis_run(AnalysisRunRequest {
            session_id: sid,
            profile: Profile::Standard,
            photo_ids: None,
            force: false,
            allow_download: false,
        })
        .await
        .unwrap();
    for _ in 0..1200 {
        if e.core.analysis_status(sid).state != RunState::Running {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/photos?session_id={sid}"),
        None,
    )
    .await;
    let ids: std::collections::HashMap<String, i64> = r.json()["photos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["file_name"].as_str().unwrap().to_string(),
                p["id"].as_i64().unwrap(),
            )
        })
        .collect();
    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/photos/{}/analysis", ids["a1.jpg"]),
        None,
    )
    .await;
    let faces = r.json()["faces"].clone();
    Scene {
        sid,
        a: faces[0]["person_id"].as_i64().unwrap(),
        b: faces[1]["person_id"].as_i64().unwrap(),
        ids,
    }
}

#[tokio::test]
async fn people_prepare_profiles_and_apply_over_http() {
    let e = env();
    let s = scene(&e).await;
    let a1 = s.ids["a1.jpg"];

    let r = call(
        &e.app,
        Method::GET,
        &format!("/api/photos/{a1}/people"),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let j = r.json();
    assert_eq!(j["ready"], false);
    assert_eq!(j["people"].as_array().unwrap().len(), 2);
    for k in [
        "face_id",
        "person_id",
        "person_name",
        "face_box",
        "is_subject",
        "has_pose",
        "has_profile",
    ] {
        assert!(j["people"][0].get(k).is_some(), "missing {k}");
    }
    assert_eq!(
        call(&e.app, Method::GET, "/api/photos/999999/people", None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );

    let mut rx = e.core.events.subscribe();
    let r = call(
        &e.app,
        Method::POST,
        &format!("/api/photos/{a1}/beauty/prepare"),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    assert!(r.json()["task_id"].as_str().unwrap().starts_with("beauty-"));
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Ok(Event::BeautyReady { photo_id }) = rx.recv().await {
                if photo_id == a1 {
                    return;
                }
            }
        }
    })
    .await
    .unwrap();
    let j = call(
        &e.app,
        Method::GET,
        &format!("/api/photos/{a1}/people"),
        None,
    )
    .await
    .json();
    assert_eq!(j["ready"], true);
    assert_eq!(j["people"][0]["has_pose"], true);
    assert_eq!(j["people"][0]["person_id"], s.a);

    // 409 models_missing carries the ids
    e.worker.set_missing(&[BEAUTY_MODEL]);
    let r = call(
        &e.app,
        Method::POST,
        &format!("/api/photos/{}/beauty/prepare", s.ids["a2.jpg"]),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.code(), "models_missing");
    assert_eq!(r.json()["models"], json!([BEAUTY_MODEL]));
    e.worker.set_missing(&[]);

    // profiles
    let uri = format!("/api/people/{}/beauty-profile", s.a);
    assert_eq!(
        call(&e.app, Method::GET, &uri, None).await.json(),
        json!({"profile": null})
    );
    let r = call(
        &e.app,
        Method::PUT,
        &uri,
        Some(json!({"profile":{"beauty":{"smooth":35},"body":{"legs":10}}})),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["profile"]["beauty"]["smooth"], 35.0);
    assert_eq!(r.json()["profile"]["body"]["legs"], 10.0);
    let r = call(
        &e.app,
        Method::PUT,
        &uri,
        Some(json!({"profile":{"beauty":{"smooth":500}}})),
    )
    .await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, "unprocessable")
    );
    assert_eq!(
        call(
            &e.app,
            Method::GET,
            "/api/people/987654/beauty-profile",
            None
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );

    let r = call(
        &e.app,
        Method::POST,
        "/api/edits/apply-profiles",
        Some(json!({"photo_ids":[a1, s.ids["b1.jpg"], s.ids["c1.jpg"]]})),
    )
    .await;
    assert_eq!(r.json(), json!({"updated": 2}));
    let st = call(&e.app, Method::GET, &format!("/api/edits/{a1}"), None)
        .await
        .json();
    let ops = st["stack"]["ops"].as_array().unwrap();
    assert_eq!(ops.len(), 2);
    assert!(ops.iter().all(|o| o["person_id"] == s.a));
    let r = call(&e.app, Method::PUT, &uri, Some(json!({"profile": null}))).await;
    assert_eq!(r.json(), json!({"profile": null}));

    // validation of the new ops through PUT /api/edits
    let bad = json!({"stack":{"version":1,"ops":[{"type":"warp","kind":"body","legs":-4}]}});
    let r = call(&e.app, Method::PUT, &format!("/api/edits/{a1}"), Some(bad)).await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn best_of_people_over_http() {
    let e = env();
    let s = scene(&e).await;
    let r = call(
        &e.app,
        Method::GET,
        &format!(
            "/api/people/best?session_id={}&ids={},{}&n=2",
            s.sid, s.a, s.b
        ),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let people = r.json()["people"].as_array().unwrap().clone();
    assert_eq!(people.len(), 2);
    assert_eq!(people[0]["person_id"], s.a);
    let photos = people[0]["photos"].as_array().unwrap();
    assert_eq!(photos.len(), 2, "burst a1/a2 counts once, plus b1");
    assert!(photos[0]["score"].as_f64().unwrap() >= photos[1]["score"].as_f64().unwrap());
    assert!(photos[0].get("photo_id").is_some());
    let all = call(&e.app, Method::GET, "/api/people/best", None)
        .await
        .json();
    assert!(all["people"].as_array().unwrap().len() >= 2);
    for q in ["n=0", "n=99", "ids=x"] {
        assert_eq!(
            call(&e.app, Method::GET, &format!("/api/people/best?{q}"), None)
                .await
                .status,
            StatusCode::BAD_REQUEST,
            "{q}"
        );
    }
}

fn jpeg() -> Vec<u8> {
    let img = image::RgbImage::from_pixel(40, 30, image::Rgb([90, 120, 200]));
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut out)
        .encode_image(&img)
        .unwrap();
    out
}

#[tokio::test]
async fn face_search_multipart_and_json() {
    let e = env();
    let s = scene(&e).await;
    e.worker.set_embed_faces(vec![
        ([0.1, 0.1, 0.1, 0.1], person(1, 0.0)),
        ([0.4, 0.1, 0.3, 0.3], person(0, 0.0)),
    ]);
    let r = send(
        &e.app,
        multipart(&[
            ("image", Some("q.jpg"), jpeg()),
            ("session_id", None, s.sid.to_string().into_bytes()),
        ]),
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let j = r.json();
    assert_eq!(j["faces_detected"].as_array().unwrap().len(), 2);
    assert_eq!(j["query_face"], 1);
    assert_eq!(j["candidates"][0]["person_id"], s.a);
    assert!(j["candidates"][0]["similarity"].as_f64().unwrap() > 0.95);
    assert!(
        j["similar_faces"][0].get("face_id").is_some()
            && j["similar_faces"][0].get("photo_id").is_some()
    );

    let r = send(
        &e.app,
        multipart(&[
            ("image", Some("q.jpg"), jpeg()),
            ("face_index", None, b"0".to_vec()),
        ]),
    )
    .await;
    let j = r.json();
    assert_eq!(
        (
            j["query_face"].as_i64(),
            j["candidates"][0]["person_id"].as_i64()
        ),
        (Some(0), Some(s.b))
    );
    let r = send(
        &e.app,
        multipart(&[
            ("image", Some("q.jpg"), jpeg()),
            ("face_index", None, b"9".to_vec()),
        ]),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = send(&e.app, multipart(&[("session_id", None, b"1".to_vec())])).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = send(
        &e.app,
        multipart(&[("image", Some("q.txt"), b"hello".to_vec())]),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // by face id
    let face_id = call(
        &e.app,
        Method::GET,
        &format!("/api/photos/{}/analysis", s.ids["a1.jpg"]),
        None,
    )
    .await
    .json()["faces"][0]["id"]
        .as_i64()
        .unwrap();
    let r = call(
        &e.app,
        Method::POST,
        "/api/faces/search",
        Some(json!({"face_id": face_id, "session_id": s.sid})),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["candidates"][0]["person_id"], s.a);
    assert_eq!(r.json()["query_face"], 0);
    let r = call(
        &e.app,
        Method::POST,
        "/api/faces/search",
        Some(json!({"face_id": 99_999_999})),
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn new_person_from_face_search_over_http() {
    let e = env();
    let s = scene(&e).await;
    let analysis = |name: &str| format!("/api/photos/{}/analysis", s.ids[name]);
    let a1_faces = call(&e.app, Method::GET, &analysis("a1.jpg"), None)
        .await
        .json()["faces"]
        .clone();
    let a1_face = a1_faces[0]["id"].as_i64().unwrap();
    let b1_face = call(&e.app, Method::GET, &analysis("b1.jpg"), None)
        .await
        .json()["faces"][0]["id"]
        .as_i64()
        .unwrap();

    // similar faces carry their current person
    let r = call(
        &e.app,
        Method::POST,
        "/api/faces/search",
        Some(json!({"face_id": a1_face, "session_id": s.sid})),
    )
    .await;
    let similar = r.json()["similar_faces"].as_array().unwrap().clone();
    let b1 = similar
        .iter()
        .find(|f| f["face_id"] == b1_face)
        .expect("b1's face is similar");
    assert_eq!(b1["person_id"], s.a);
    assert!(b1.get("person_name").is_some());

    // the query face and b1's face are somebody new
    let mut rx = e.core.events.subscribe();
    let r = call(
        &e.app,
        Method::POST,
        "/api/people",
        Some(json!({"face_ids": [a1_face, b1_face], "name": " Ann "})),
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let p = r.json()["person"].clone();
    let ann = p["id"].as_i64().unwrap();
    assert_ne!(ann, s.a);
    assert_eq!(p["name"], "Ann");
    assert_eq!(p["photo_count"], 2);
    assert!(p["cover_face_id"] == a1_face || p["cover_face_id"] == b1_face);
    for name in ["a1.jpg", "b1.jpg"] {
        let f = call(&e.app, Method::GET, &analysis(name), None)
            .await
            .json()["faces"][0]
            .clone();
        assert_eq!(f["person_id"], ann, "{name}");
        assert_eq!(f["person_name"], "Ann", "{name}");
    }
    let people = call(
        &e.app,
        Method::GET,
        &format!("/api/people?session_id={}", s.sid),
        None,
    )
    .await
    .json()["people"]
        .as_array()
        .unwrap()
        .clone();
    assert!(people
        .iter()
        .any(|p| p["id"] == ann && p["photo_count"] == 2));
    let mut seen = false;
    while let Ok(ev) = rx.try_recv() {
        seen |= matches!(ev, Event::PeopleUpdated { session_id } if session_id == s.sid);
    }
    assert!(seen, "people.updated");

    // validation (two faces of one photo cannot be the same person)
    let (left, right) = (a1_faces[0]["id"].clone(), a1_faces[1]["id"].clone());
    for (body, status) in [
        (json!({"face_ids": []}), StatusCode::BAD_REQUEST),
        (json!({"face_ids": [99_999_999]}), StatusCode::NOT_FOUND),
        (json!({"face_ids": [left, right]}), StatusCode::BAD_REQUEST),
    ] {
        let r = call(&e.app, Method::POST, "/api/people", Some(body.clone())).await;
        assert_eq!(r.status, status, "{body}");
    }
}

#[tokio::test]
async fn collections_over_http_and_queries_filter() {
    let e = env();
    let s = scene(&e).await;
    let r = call(&e.app, Method::GET, "/api/collections", None).await;
    let list = r.json()["collections"].as_array().unwrap().clone();
    let ids: Vec<&str> = list.iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(
        ids,
        ["best_per_group", "has_closed_eyes", "undecided", "edited"]
    );
    assert!(list.iter().all(|c| c["builtin"] == true));

    // every built-in query is a valid /api/photos query and really filters
    e.core
        .put_edit(
            s.ids["b1.jpg"],
            json!({"version":1,"ops":[{"type":"global","exposure":0.3}]}),
        )
        .await
        .unwrap();
    call(
        &e.app,
        Method::PATCH,
        "/api/photos",
        Some(json!({"ids":[s.ids["a1.jpg"]],"flag":1})),
    )
    .await;
    let total = |q: String| {
        let app = e.app.clone();
        let sid = s.sid;
        async move {
            let r = call(
                &app,
                Method::GET,
                &format!("/api/photos?session_id={sid}&{q}&limit=1"),
                None,
            )
            .await;
            assert_eq!(
                r.status,
                StatusCode::OK,
                "{}",
                String::from_utf8_lossy(&r.body)
            );
            r.json()["total"].as_i64().unwrap()
        }
    };
    assert_eq!(total(String::new()).await, 4);
    for c in &list {
        let q = c["query"].as_str().unwrap().to_string();
        let t = total(q.clone()).await;
        match c["id"].as_str().unwrap() {
            "best_per_group" => assert_eq!(t, 3, "{q}"),
            "has_closed_eyes" => assert_eq!(t, 1, "{q}"),
            "undecided" => assert_eq!(t, 3, "{q}"),
            "edited" => assert_eq!(t, 1, "{q}"),
            _ => unreachable!(),
        }
    }
    assert_eq!(total("has_edits=0".into()).await, 3);

    // CRUD
    let r = call(
        &e.app,
        Method::POST,
        "/api/collections",
        Some(json!({"name":"Picked","query":"flag=picked"})),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let c = r.json()["collection"].clone();
    assert_eq!(
        (
            c["name"].as_str(),
            c["query"].as_str(),
            c["builtin"].as_bool()
        ),
        (Some("Picked"), Some("flag=picked"), Some(false))
    );
    let id = c["id"].as_str().unwrap().to_string();
    assert_eq!(total("flag=picked".into()).await, 1);
    let r = call(
        &e.app,
        Method::PATCH,
        &format!("/api/collections/{id}"),
        Some(json!({"name":"Faves","query":"rating_gte=4"})),
    )
    .await;
    assert_eq!(r.json()["collection"]["name"], "Faves");
    for bad in [
        json!({"name":"x","query":"flag=bogus"}),
        json!({"name":"x","query":"session_id=1"}),
        json!({"name":"x","query":"limit=5"}),
        json!({"name":"x","query":"nonsense=1"}),
        json!({"name":" ","query":"flag=picked"}),
    ] {
        let r = call(&e.app, Method::POST, "/api/collections", Some(bad.clone())).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}");
    }
    let r = call(
        &e.app,
        Method::PATCH,
        &format!("/api/collections/{id}"),
        Some(json!({"query":"flag=nope"})),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    // built-ins are protected
    let r = call(
        &e.app,
        Method::PATCH,
        "/api/collections/edited",
        Some(json!({"name":"x"})),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = call(&e.app, Method::DELETE, "/api/collections/undecided", None).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = call(
        &e.app,
        Method::DELETE,
        &format!("/api/collections/{id}"),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let r = call(
        &e.app,
        Method::DELETE,
        &format!("/api/collections/{id}"),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn taste_endpoints_and_export_folders() {
    let e = env();
    let s = scene(&e).await;
    let r = call(&e.app, Method::GET, "/api/taste", None).await;
    assert_eq!(r.status, StatusCode::OK);
    let j = r.json();
    assert_eq!(
        (
            j["labels"].as_i64(),
            j["active"].as_bool(),
            j["alpha"].as_f64()
        ),
        (Some(0), Some(false), Some(0.0))
    );
    assert!(j["holdout_accuracy"].is_null() && j["traits"].as_array().unwrap().is_empty());
    assert!(j.get("updated_at").is_some());
    call(
        &e.app,
        Method::PATCH,
        "/api/photos",
        Some(json!({"ids":[s.ids["a1.jpg"]],"user_rating":5})),
    )
    .await;
    assert_eq!(
        call(&e.app, Method::GET, "/api/taste", None).await.json()["labels"],
        1
    );
    let r = call(&e.app, Method::POST, "/api/taste/reset", None).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(
        call(&e.app, Method::GET, "/api/taste", None).await.json()["labels"],
        0
    );

    // export with folders and no ids
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("o");
    let r = call(
        &e.app,
        Method::POST,
        "/api/export",
        Some(json!({"dest": dest.to_string_lossy(), "folders": {"Anna": [s.ids["a1.jpg"], s.ids["b1.jpg"]], "Ben": [s.ids["a1.jpg"]]}})),
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    for _ in 0..200 {
        if dest.join("Ben").join("a1.jpg").is_file() && dest.join("Anna").join("b1.jpg").is_file() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(dest.join("Anna").join("a1.jpg").is_file());
    assert!(dest.join("Anna").join("b1.jpg").is_file());
    assert!(dest.join("Ben").join("a1.jpg").is_file());
    let r = call(
        &e.app,
        Method::POST,
        "/api/export",
        Some(json!({"dest": dest.to_string_lossy()})),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}
