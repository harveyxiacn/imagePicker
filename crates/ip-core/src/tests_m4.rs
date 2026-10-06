//! M4 tests: portrait geometry, beauty profiles, M2 remainder (best of person, face search,
//! collections, export folders) and personalised scoring. FakeImaging + FakeWorker + FakeRenderer.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use ip_render::Backend;
use serde_json::{json, Value};

use crate::analysis::*;
use crate::edit::sync::SyncKind;
use crate::edit::{validate_stack, SyncRequest};
use crate::fake_worker::{person, FakeFace, FakeSpec, FakeWorker, BEAUTY_MODEL};
use crate::testutil::{write_jpeg, FakeImaging, FakeRenderer};
use crate::*;

const BASE: u64 = 1_700_000_000;
const WAIT: Duration = Duration::from_secs(30);

struct Env {
    core: Arc<Core>,
    worker: Arc<FakeWorker>,
    renderer: Arc<FakeRenderer>,
    _data: tempfile::TempDir,
    src: tempfile::TempDir,
}

fn env() -> Env {
    let data = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let worker = Arc::new(FakeWorker::new());
    let renderer = Arc::new(FakeRenderer::new(Backend::Cpu));
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(2),
        worker: Some(worker.clone()),
        renderer: Some(renderer.clone()),
        force_cpu: true,
    })
    .unwrap();
    Env {
        core,
        worker,
        renderer,
        _data: data,
        src,
    }
}

fn photo(dir: &Path, name: &str, t_s: u64) {
    let p = dir.join(name);
    write_jpeg(&p, 320, 240, (t_s % 200) as u8);
    let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
    f.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(BASE + t_s))
        .unwrap();
}

async fn import(e: &Env) -> i64 {
    let s = e
        .core
        .import(ImportRequest {
            path: e.src.path().to_string_lossy().into_owned(),
            recursive: true,
            title: None,
        })
        .await
        .unwrap();
    e.core.wait_session_ready(s.id, WAIT).await.unwrap();
    s.id
}

async fn analyze(e: &Env, sid: i64) {
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
        let s = e.core.analysis_status(sid);
        if s.state != RunState::Running {
            assert_eq!(s.state, RunState::Done, "{s:?}");
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("analysis did not finish");
}

async fn by_name(e: &Env, sid: i64) -> HashMap<String, Photo> {
    e.core
        .photos(PhotoQuery {
            session_id: sid,
            limit: Some(5000),
            ..Default::default()
        })
        .await
        .unwrap()
        .photos
        .into_iter()
        .map(|p| (p.file_name.clone(), p))
        .collect()
}

fn face_a() -> [f64; 4] {
    [0.2, 0.3, 0.2, 0.3]
}
fn face_b() -> [f64; 4] {
    [0.6, 0.3, 0.1, 0.15]
}

/// a1 a2 a3 (one burst) and b1 b2 (another) show people A and B (A bigger), c1 a stranger.
/// Expressions: A's smile/eyes get better from a1 to a3; b1 has A with closed eyes.
fn build_scene(e: &Env) {
    let d = e.src.path();
    for (n, t) in [
        ("a1.jpg", 0),
        ("a2.jpg", 1),
        ("a3.jpg", 2),
        ("b1.jpg", 3600),
        ("b2.jpg", 3601),
        ("c1.jpg", 7200),
        ("d1.jpg", 10800),
    ] {
        photo(d, n, t);
    }
    let faces = |ja: f32, eyes_a: f64, smile_a: f64| {
        vec![
            FakeFace::new(face_a(), person(0, ja))
                .eyes(eyes_a)
                .smile(smile_a),
            FakeFace::new(face_b(), person(1, 1.0)),
        ]
    };
    let set = |n: &str, angle: f32, sharp: f64, f: Vec<FakeFace>| {
        e.worker
            .set(n, FakeSpec::at(angle).sharp(sharp).with_faces(f));
    };
    set("a1.jpg", 0.0, 0.8, faces(1.0, 0.95, 0.2));
    set("a2.jpg", 2.0, 0.8, faces(0.0, 0.95, 0.9));
    set("a3.jpg", 3.0, 0.8, faces(2.0, 0.95, 0.5));
    set("b1.jpg", 90.0, 0.8, faces(1.0, 0.3, 0.9));
    set("b2.jpg", 91.0, 0.8, faces(2.0, 0.95, 0.6));
    e.worker.set(
        "c1.jpg",
        FakeSpec::at(200.0).with_faces(vec![FakeFace::new([0.4, 0.4, 0.2, 0.3], person(5, 0.0))]),
    );
    // d1 has no faces at all
    e.worker.set("d1.jpg", FakeSpec::at(300.0));
}

struct Scene {
    e: Env,
    sid: i64,
    ph: HashMap<String, Photo>,
    a: i64,
    b: i64,
    stranger: Option<i64>,
}

async fn scene() -> Scene {
    let e = env();
    build_scene(&e);
    let sid = import(&e).await;
    analyze(&e, sid).await;
    let ph = by_name(&e, sid).await;
    let faces = e.core.photo_analysis(ph["a1.jpg"].id).await.unwrap().faces;
    let a = faces[0].person_id.expect("person A");
    let b = faces[1].person_id.expect("person B");
    let stranger = e.core.photo_analysis(ph["c1.jpg"].id).await.unwrap().faces[0].person_id;
    Scene {
        e,
        sid,
        ph,
        a,
        b,
        stranger,
    }
}

async fn wait_beauty_ready(rx: &mut tokio::sync::broadcast::Receiver<Event>, photo_id: i64) {
    tokio::time::timeout(WAIT, async {
        loop {
            if let Ok(Event::BeautyReady { photo_id: p }) = rx.recv().await {
                if p == photo_id {
                    return;
                }
            }
        }
    })
    .await
    .expect("beauty.ready");
}

fn beauty_stack(person: Option<i64>, smooth: f64) -> Value {
    let mut op = json!({"type":"beauty","level":"natural","smooth":smooth,"whiten":20});
    if let Some(p) = person {
        op["person_id"] = json!(p);
    }
    json!({"version":1,"ops":[op]})
}

// ------------------------------------------------------------------ portrait

#[tokio::test]
async fn people_endpoint_prepare_task_event_and_cache() {
    let s = scene().await;
    let (a1, c1) = (s.ph["a1.jpg"].id, s.ph["c1.jpg"].id);

    // before preparation: faces from the analysis, ready=false
    let out = s.e.core.photo_people(a1).await.unwrap();
    assert!(!out.ready);
    assert_eq!(out.people.len(), 2);
    assert_eq!(out.people[0].person_id, Some(s.a));
    assert!(out.people.iter().all(|p| !p.has_pose && !p.has_profile));
    assert_eq!(s.e.worker.beauty_calls.load(Ordering::SeqCst), 0);
    assert!(matches!(
        s.e.core.photo_people(999_999).await,
        Err(CoreError::NotFound(_))
    ));

    let mut rx = s.e.core.events.subscribe();
    let task = s.e.core.beauty_prepare(a1).await.unwrap();
    assert!(task.starts_with("beauty-"));
    wait_beauty_ready(&mut rx, a1).await;
    assert_eq!(s.e.worker.beauty_calls.load(Ordering::SeqCst), 1);
    let req =
        s.e.worker
            .last_beauty_request
            .lock()
            .unwrap()
            .clone()
            .unwrap();
    assert_eq!(req.photo.photo_id, a1);
    assert_eq!(
        req.faces.len(),
        2,
        "the analysed faces are handed to the worker"
    );
    assert!(!req.allow_download);

    let out = s.e.core.photo_people(a1).await.unwrap();
    assert!(out.ready);
    assert_eq!(out.people.len(), 2);
    assert_eq!(
        out.people.iter().map(|p| p.person_id).collect::<Vec<_>>(),
        vec![Some(s.a), Some(s.b)]
    );
    assert!(out.people.iter().all(|p| p.has_pose && p.face_id.is_some()));
    assert!((out.people[0].face_box[0] - 0.2).abs() < 1e-6);
    // other photos are untouched
    assert!(!s.e.core.photo_people(c1).await.unwrap().ready);

    // a second prepare is served from the cache
    let mut rx = s.e.core.events.subscribe();
    s.e.core.beauty_prepare(a1).await.unwrap();
    wait_beauty_ready(&mut rx, a1).await;
    assert_eq!(s.e.worker.beauty_calls.load(Ordering::SeqCst), 1);

    // a photo without analysed faces: the worker finds them itself, person unknown
    let d1 = s.ph["d1.jpg"].id;
    let mut rx = s.e.core.events.subscribe();
    s.e.core.beauty_prepare(d1).await.unwrap();
    wait_beauty_ready(&mut rx, d1).await;
    let out = s.e.core.photo_people(d1).await.unwrap();
    assert!(out.ready && out.people.len() == 1);
    assert_eq!(out.people[0].person_id, None);
    assert_eq!(out.people[0].face_id, None);
}

#[tokio::test]
async fn beauty_prepare_without_models_is_409() {
    let s = scene().await;
    s.e.worker.set_missing(&[BEAUTY_MODEL]);
    match s.e.core.beauty_prepare(s.ph["a1.jpg"].id).await {
        Err(CoreError::ModelsMissing(m)) => assert_eq!(m, vec![BEAUTY_MODEL.to_string()]),
        other => panic!("expected models_missing, got {other:?}"),
    }
    assert_eq!(s.e.worker.beauty_calls.load(Ordering::SeqCst), 0);
    // a strict render that needs the geometry surfaces the same error
    let r =
        s.e.core
            .render_preview(crate::edit::PreviewRequest {
                photo_id: s.ph["a1.jpg"].id,
                stack: Some(beauty_stack(None, 30.0)),
                long_edge: Some(128),
                original: None,
            })
            .await;
    assert!(matches!(r, Err(CoreError::ModelsMissing(_))), "{r:?}");
}

#[tokio::test]
async fn render_resolves_people_with_person_ids_strict_and_cached_only() {
    let s = scene().await;
    let (a1, a2) = (s.ph["a1.jpg"].id, s.ph["a2.jpg"].id);

    // thumbnails never prepare: CachedOnly sees no people and the worker is not called
    s.e.core
        .put_edit(a2, beauty_stack(Some(s.a), 40.0))
        .await
        .unwrap();
    s.e.core.thumb_path(a2, 256).await.unwrap();
    assert_eq!(s.e.worker.beauty_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        s.e.renderer.people_seen.lock().unwrap().last().cloned(),
        Some(vec![])
    );

    // previews prepare on demand and see the person ids of the photo
    s.e.core
        .render_preview(crate::edit::PreviewRequest {
            photo_id: a1,
            stack: Some(beauty_stack(Some(s.a), 40.0)),
            long_edge: Some(128),
            original: None,
        })
        .await
        .unwrap();
    assert_eq!(s.e.worker.beauty_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        s.e.renderer.people_seen.lock().unwrap().last().cloned(),
        Some(vec![Some(s.a), Some(s.b)])
    );
    // now cached: thumbnails see them too
    s.e.core
        .put_edit(a1, beauty_stack(Some(s.a), 40.0))
        .await
        .unwrap();
    s.e.core.thumb_path(a1, 256).await.unwrap();
    assert_eq!(
        s.e.renderer.people_seen.lock().unwrap().last().cloned(),
        Some(vec![Some(s.a), Some(s.b)])
    );
    assert_eq!(s.e.worker.beauty_calls.load(Ordering::SeqCst), 1);

    // changed faces (re-analysis) invalidate the geometry
    s.e.core
        .db
        .call(move |c| {
            c.execute(
                "UPDATE face SET bbox_x = bbox_x + 0.01 WHERE photo_id=?1",
                [a1],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(!s.e.core.photo_people(a1).await.unwrap().ready);
}

#[test]
fn validator_ranges_for_beauty_and_warp() {
    let ok = |op: Value| validate_stack(json!({"version":1,"ops":[op]})).is_ok();
    assert!(ok(
        json!({"type":"beauty","smooth":100,"whiten":0,"eye_brighten":50})
    ));
    assert!(ok(
        json!({"type":"beauty","person_id":3,"level":"refined","blemish":true})
    ));
    for field in [
        "smooth",
        "whiten",
        "eye_brighten",
        "teeth_whiten",
        "dark_circles",
    ] {
        assert!(!ok(json!({"type":"beauty", field: 100.5})), "{field}");
        assert!(!ok(json!({"type":"beauty", field: -1})), "{field}");
    }
    assert!(!ok(json!({"type":"beauty","person_id":0})));
    assert!(ok(
        json!({"type":"warp","kind":"face","slim":-100,"chin":100,"eyes":0,"nose":50})
    ));
    for field in ["slim", "chin", "eyes", "nose"] {
        assert!(
            !ok(json!({"type":"warp","kind":"face", field: 101})),
            "{field}"
        );
        assert!(
            !ok(json!({"type":"warp","kind":"face", field: -101})),
            "{field}"
        );
    }
    assert!(ok(
        json!({"type":"warp","kind":"body","arms":100,"legs":0,"waist":10,"lengthen_legs":5})
    ));
    for field in ["arms", "legs", "waist", "lengthen_legs"] {
        assert!(
            !ok(json!({"type":"warp","kind":"body", field: -1})),
            "{field}"
        );
        assert!(
            !ok(json!({"type":"warp","kind":"body", field: 101})),
            "{field}"
        );
    }
    assert!(!ok(json!({"type":"beauty","smooth":"a lot"})));
    // unknown kinds / ops still load
    assert!(ok(json!({"type":"warp","kind":"future"})));
    assert!(ok(json!({"type":"future_op"})));
}

#[test]
fn has_edits_semantics_of_portrait_ops() {
    let has = |op: Value| {
        validate_stack(json!({"version":1,"ops":[op]}))
            .unwrap()
            .has_edits
    };
    assert!(!has(json!({"type":"beauty"})), "nothing dialled in");
    assert!(!has(json!({"type":"beauty","smooth":0,"blemish":false})));
    assert!(has(json!({"type":"beauty","smooth":1})));
    assert!(has(json!({"type":"beauty","blemish":true})));
    assert!(!has(json!({"type":"warp","kind":"face"})));
    assert!(has(json!({"type":"warp","kind":"face","nose":-3})));
    assert!(!has(json!({"type":"warp","kind":"body","arms":0})));
    assert!(has(json!({"type":"warp","kind":"body","legs":4})));
    assert!(!has(json!({"type":"warp","kind":"unknown_kind"})));
    assert!(has(json!({"type":"global","exposure":0})));
}

#[tokio::test]
async fn has_edits_flag_follows_portrait_ops() {
    let s = scene().await;
    let id = s.ph["a1.jpg"].id;
    s.e.core
        .put_edit(
            id,
            json!({"version":1,"ops":[{"type":"beauty","level":"natural"}]}),
        )
        .await
        .unwrap();
    assert!(!s.e.core.photo(id).await.unwrap().has_edits);
    s.e.core
        .put_edit(id, beauty_stack(None, 20.0))
        .await
        .unwrap();
    assert!(s.e.core.photo(id).await.unwrap().has_edits);
}

#[tokio::test]
async fn beauty_profile_crud_and_apply() {
    let s = scene().await;
    let (a1, a2, c1) = (s.ph["a1.jpg"].id, s.ph["a2.jpg"].id, s.ph["c1.jpg"].id);
    assert_eq!(s.e.core.beauty_profile(s.a).await.unwrap(), None);
    assert!(matches!(
        s.e.core.beauty_profile(987_654).await,
        Err(CoreError::NotFound(_))
    ));

    let p =
        s.e.core
            .set_beauty_profile(
                s.a,
                Some(json!({
                    "beauty": {"level":"natural","smooth":30,"whiten":10,"person_id":99},
                    "face": {"level":"natural","slim":25}
                })),
            )
            .await
            .unwrap()
            .unwrap();
    assert_eq!(p["beauty"]["smooth"], 30.0);
    assert!(p["beauty"].get("person_id").is_none() && p["beauty"].get("type").is_none());
    assert!(p["face"].get("kind").is_none());
    assert!(p.get("body").is_none());
    assert_eq!(s.e.core.beauty_profile(s.a).await.unwrap(), Some(p.clone()));
    assert!(s.e.core.photo_people(a1).await.unwrap().people[0].has_profile);

    // invalid profiles
    for bad in [
        json!({"beauty":{"smooth":101}}),
        json!({"face":{"slim":-200}}),
        json!({"body":{"arms":-5}}),
        json!({"hair":{}}),
        json!({"beauty":3}),
        json!(5),
    ] {
        assert!(
            matches!(
                s.e.core.set_beauty_profile(s.b, Some(bad.clone())).await,
                Err(CoreError::Unprocessable(_))
            ),
            "{bad}"
        );
    }

    // photo a2 already has a manual beauty op for A and one for B
    let manual = json!({"version":1,"ops":[
        {"type":"beauty","person_id":s.a,"smooth":5},
        {"type":"beauty","person_id":s.b,"smooth":7},
        {"type":"global","exposure":0.5}]});
    s.e.core.put_edit(a2, manual).await.unwrap();

    let n = s.e.core.apply_profiles(vec![a1, a2, c1]).await.unwrap();
    assert_eq!(n, 2, "c1 does not show A");
    let ops = |v: &Value| v["ops"].as_array().unwrap().clone();
    let st1 = s.e.core.get_edit(a1).await.unwrap().stack;
    assert_eq!(ops(&st1).len(), 2);
    let beauty = ops(&st1)
        .into_iter()
        .find(|o| o["type"] == "beauty")
        .unwrap();
    assert_eq!(beauty["person_id"], s.a);
    assert_eq!(beauty["smooth"], 30.0);
    let warp = ops(&st1).into_iter().find(|o| o["type"] == "warp").unwrap();
    assert_eq!(
        (warp["kind"].as_str(), warp["person_id"].as_i64()),
        (Some("face"), Some(s.a))
    );
    assert!(s.e.core.photo(a1).await.unwrap().has_edits);

    let st2 = s.e.core.get_edit(a2).await.unwrap().stack;
    let o2 = ops(&st2);
    let beauties: Vec<&Value> = o2.iter().filter(|o| o["type"] == "beauty").collect();
    assert_eq!(beauties.len(), 2, "A's op replaced, B's untouched");
    assert_eq!(
        beauties.iter().find(|o| o["person_id"] == s.a).unwrap()["smooth"],
        30.0
    );
    assert_eq!(
        beauties.iter().find(|o| o["person_id"] == s.b).unwrap()["smooth"],
        7.0
    );
    assert!(o2.iter().any(|o| o["type"] == "global"));
    assert!(s.e.core.get_edit(c1).await.unwrap().updated_at.is_none());

    // idempotent
    s.e.core.apply_profiles(vec![a1]).await.unwrap();
    assert_eq!(s.e.core.get_edit(a1).await.unwrap().stack, st1);

    // deleting the profile
    assert_eq!(s.e.core.set_beauty_profile(s.a, None).await.unwrap(), None);
    assert_eq!(s.e.core.beauty_profile(s.a).await.unwrap(), None);
    assert_eq!(s.e.core.apply_profiles(vec![a1]).await.unwrap(), 0);
    assert_eq!(
        s.e.core
            .set_beauty_profile(s.a, Some(json!({})))
            .await
            .unwrap(),
        None,
        "an empty profile is no profile"
    );
}

#[tokio::test]
async fn sync_skips_person_ops_for_photos_without_that_person() {
    let s = scene().await;
    let (a1, b1, c1) = (s.ph["a1.jpg"].id, s.ph["b1.jpg"].id, s.ph["c1.jpg"].id);
    let stack = json!({"version":1,"ops":[
        {"type":"global","exposure":0.4},
        {"type":"beauty","person_id":s.b,"smooth":30},
        {"type":"beauty","smooth":10},
        {"type":"warp","kind":"face","person_id":s.a,"slim":20},
        {"type":"local","mask":{"kind":"ai","target":"person","person_id":s.b},"adjust":{"exposure":0.3}}]});
    s.e.core.put_edit(a1, stack).await.unwrap();
    let n =
        s.e.core
            .sync_edits(SyncRequest {
                from_id: a1,
                to_ids: vec![b1, c1],
                include: vec![
                    SyncKind::Global,
                    SyncKind::Beauty,
                    SyncKind::Warp,
                    SyncKind::Local,
                ],
                adaptive: Some(false),
            })
            .await
            .unwrap();
    assert_eq!(n, 2);
    let o_b1 = s.e.core.get_edit(b1).await.unwrap().stack["ops"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(o_b1.len(), 5, "b1 shows A and B: everything is copied");
    let o_c1 = s.e.core.get_edit(c1).await.unwrap().stack["ops"]
        .as_array()
        .unwrap()
        .clone();
    let types: Vec<&str> = o_c1.iter().map(|o| o["type"].as_str().unwrap()).collect();
    assert_eq!(types.iter().filter(|t| **t == "global").count(), 1);
    assert_eq!(
        types.iter().filter(|t| **t == "beauty").count(),
        1,
        "only the unscoped one"
    );
    assert!(!types.contains(&"warp") && !types.contains(&"local"));
    assert!(o_c1
        .iter()
        .all(|o| o.get("person_id").is_none() || o["person_id"].is_null()));
    let _ = s.stranger;
}

// ------------------------------------------------------------------ best of person

#[tokio::test]
async fn best_of_person_one_per_burst_ordered() {
    let s = scene().await;
    let id = |n: &str| s.ph[n].id;
    let best = s.e.core.best_of_people(Some(s.sid), None, 5).await.unwrap();
    let a = best.iter().find(|p| p.person_id == s.a).unwrap();
    // burst a1..a3 -> one photo (a2: best smile); burst b1,b2 -> b2 (b1 has closed eyes)
    let ids: Vec<i64> = a.photos.iter().map(|p| p.photo_id).collect();
    assert_eq!(ids.len(), 2, "{a:?}");
    assert!(ids.contains(&id("a2.jpg")) && ids.contains(&id("b2.jpg")));
    assert!(a.photos[0].score >= a.photos[1].score);
    assert!(a.photos.iter().all(|p| (0.0..=1.0).contains(&p.score)));
    // formula: 0.6 * expression + 0.4 * ai_score
    let face = s.e.core.photo_analysis(id("a2.jpg")).await.unwrap().faces[0].clone();
    let ph = s.e.core.photo(id("a2.jpg")).await.unwrap();
    let want = 0.6 * face.expression_score.unwrap() + 0.4 * ph.ai_score.unwrap();
    let got = a
        .photos
        .iter()
        .find(|p| p.photo_id == id("a2.jpg"))
        .unwrap()
        .score;
    assert!((got - want).abs() < 2e-4, "{got} vs {want}");

    // n limits, ids select people (and keep their order), unknown ids give empty lists
    let one =
        s.e.core
            .best_of_people(None, Some(vec![s.b, s.a, 424_242]), 1)
            .await
            .unwrap();
    assert_eq!(
        one.iter().map(|p| p.person_id).collect::<Vec<_>>(),
        vec![s.b, s.a, 424_242]
    );
    assert!(one.iter().all(|p| p.photos.len() <= 1));
    assert!(one[2].photos.is_empty());
    // rejected photos are left out
    s.e.core
        .patch_photos(PatchRequest {
            ids: vec![id("a2.jpg")],
            flag: Some(-1),
            ..Default::default()
        })
        .await
        .unwrap();
    let again =
        s.e.core
            .best_of_people(Some(s.sid), Some(vec![s.a]), 5)
            .await
            .unwrap();
    assert!(!again[0].photos.iter().any(|p| p.photo_id == id("a2.jpg")));
    assert!(again[0]
        .photos
        .iter()
        .any(|p| p.photo_id == id("a3.jpg") || p.photo_id == id("a1.jpg")));
    for bad in [0usize, 51] {
        assert!(matches!(
            s.e.core.best_of_people(None, None, bad).await,
            Err(CoreError::BadRequest(_))
        ));
    }
}

// ------------------------------------------------------------------ face search

fn jpeg_bytes() -> Vec<u8> {
    let img = image::RgbImage::from_pixel(40, 30, image::Rgb([90, 120, 200]));
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut out)
        .encode_image(&img)
        .unwrap();
    out
}

#[tokio::test]
async fn face_search_by_upload_and_by_face_id() {
    let s = scene().await;
    // one face of person A in the query image
    s.e.worker
        .set_embed_faces(vec![([0.3, 0.3, 0.3, 0.3], person(0, 5.0))]);
    let out =
        s.e.core
            .faces_search_image(jpeg_bytes(), None, None)
            .await
            .unwrap();
    assert_eq!(out.faces_detected.len(), 1);
    assert_eq!(out.query_face, Some(0));
    assert_eq!(out.candidates[0].person_id, s.a, "{:?}", out.candidates);
    assert!(out.candidates[0].similarity > 0.95);
    assert!(out
        .candidates
        .iter()
        .all(|c| Some(c.person_id) != s.stranger));
    assert!(out
        .candidates
        .windows(2)
        .all(|w| w[0].similarity >= w[1].similarity));
    // nearest individual faces are A's, best first, at most 10
    assert!(!out.similar_faces.is_empty() && out.similar_faces.len() <= 10);
    assert!(out
        .similar_faces
        .windows(2)
        .all(|w| w[0].similarity >= w[1].similarity));
    let a_faces: Vec<i64> = {
        let sid = s.sid;
        let a = s.a;
        s.e.core
            .db
            .call(move |c| {
                let mut st = c.prepare("SELECT id FROM face WHERE person_id=?1")?;
                let v = st
                    .query_map([a], |r| r.get(0))?
                    .collect::<rusqlite::Result<Vec<i64>>>()?;
                let _ = sid;
                Ok(v)
            })
            .await
            .unwrap()
    };
    assert!(out
        .similar_faces
        .iter()
        .all(|f| a_faces.contains(&f.face_id)));
    assert_eq!(s.e.worker.embed_calls.load(Ordering::SeqCst), 1);
    // the temporary upload is gone
    let up = s.e.core.dirs.root.join("cache").join("uploads");
    assert_eq!(std::fs::read_dir(up).map(|d| d.count()).unwrap_or(0), 0);

    // two faces: the larger one is searched by default, face_index picks the other
    s.e.worker.set_embed_faces(vec![
        ([0.1, 0.1, 0.1, 0.1], person(1, 0.0)),
        ([0.5, 0.1, 0.3, 0.3], person(0, 0.0)),
    ]);
    let out =
        s.e.core
            .faces_search_image(jpeg_bytes(), None, None)
            .await
            .unwrap();
    assert_eq!((out.faces_detected.len(), out.query_face), (2, Some(1)));
    assert_eq!(out.candidates[0].person_id, s.a);
    let out =
        s.e.core
            .faces_search_image(jpeg_bytes(), None, Some(0))
            .await
            .unwrap();
    assert_eq!(out.query_face, Some(0));
    assert_eq!(out.candidates[0].person_id, s.b);
    assert!(matches!(
        s.e.core
            .faces_search_image(jpeg_bytes(), None, Some(2))
            .await,
        Err(CoreError::BadRequest(_))
    ));

    // no face, bad image, unknown session
    s.e.worker.set_embed_faces(vec![]);
    let out =
        s.e.core
            .faces_search_image(jpeg_bytes(), None, None)
            .await
            .unwrap();
    assert!(out.faces_detected.is_empty() && out.query_face.is_none() && out.candidates.is_empty());
    assert!(matches!(
        s.e.core
            .faces_search_image(b"not an image".to_vec(), None, None)
            .await,
        Err(CoreError::BadRequest(_))
    ));
    assert!(matches!(
        s.e.core
            .faces_search_image(jpeg_bytes(), Some(777), None)
            .await,
        Err(CoreError::NotFound(_))
    ));

    // by stored face
    let a1_face =
        s.e.core
            .photo_analysis(s.ph["a1.jpg"].id)
            .await
            .unwrap()
            .faces[0]
            .id;
    let out =
        s.e.core
            .faces_search_face(a1_face, Some(s.sid))
            .await
            .unwrap();
    assert_eq!(out.query_face, Some(0));
    assert_eq!(out.faces_detected.len(), 1);
    assert_eq!(out.candidates[0].person_id, s.a);
    assert!(out.similar_faces.iter().all(|f| f.face_id != a1_face));
    assert!(out
        .similar_faces
        .iter()
        .all(|f| a_faces.contains(&f.face_id)));
    assert!(matches!(
        s.e.core.faces_search_face(31_337_000, None).await,
        Err(CoreError::NotFound(_))
    ));
}

// ------------------------------------------------------------------ collections

#[tokio::test]
async fn collections_crud_and_builtin_protection() {
    let e = env();
    let mut rx = e.core.events.subscribe();
    let list = e.core.collections().await.unwrap();
    let keys: Vec<&str> = list.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        keys,
        vec!["best_per_group", "has_closed_eyes", "undecided", "edited"]
    );
    assert!(list
        .iter()
        .all(|c| c.builtin && c.name.starts_with("collection.")));

    let c = e
        .core
        .create_collection("  My picks ", "?flag=picked&rating_gte=4")
        .await
        .unwrap();
    assert_eq!(
        (c.name.as_str(), c.query.as_str(), c.builtin),
        ("My picks", "flag=picked&rating_gte=4", false)
    );
    assert!(c.id.starts_with("user_"));
    assert_eq!(e.core.collections().await.unwrap().len(), 5);
    let c2 = e
        .core
        .update_collection(&c.id, Some("Renamed".into()), None)
        .await
        .unwrap();
    assert_eq!(
        (c2.name.as_str(), c2.query.as_str()),
        ("Renamed", "flag=picked&rating_gte=4")
    );
    let c3 = e
        .core
        .update_collection(&c.id, None, Some("has_edits=1".into()))
        .await
        .unwrap();
    assert_eq!(c3.query, "has_edits=1");
    assert!(matches!(
        e.core.create_collection("   ", "x=1").await,
        Err(CoreError::BadRequest(_))
    ));

    for id in ["best_per_group", "edited"] {
        assert!(matches!(
            e.core.update_collection(id, Some("x".into()), None).await,
            Err(CoreError::BadRequest(_))
        ));
        assert!(matches!(
            e.core.delete_collection(id).await,
            Err(CoreError::BadRequest(_))
        ));
    }
    assert!(matches!(
        e.core
            .update_collection("user_999", Some("x".into()), None)
            .await,
        Err(CoreError::NotFound(_))
    ));
    assert!(matches!(
        e.core.delete_collection("nonsense").await,
        Err(CoreError::NotFound(_))
    ));
    e.core.delete_collection(&c.id).await.unwrap();
    assert_eq!(e.core.collections().await.unwrap().len(), 4);
    assert!(matches!(
        e.core.delete_collection(&c.id).await,
        Err(CoreError::NotFound(_))
    ));

    let mut updates = 0;
    while let Ok(ev) = rx.try_recv() {
        if ev == (Event::CollectionsUpdated {}) {
            updates += 1;
        }
    }
    assert_eq!(updates, 4, "create, two updates, delete");
}

// ------------------------------------------------------------------ export folders

#[tokio::test]
async fn export_into_folders() {
    let s = scene().await;
    let id = |n: &str| s.ph[n].id;
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("export");
    let mut rx = s.e.core.events.subscribe();
    let mut folders = std::collections::BTreeMap::new();
    folders.insert("Anna".to_string(), vec![id("a1.jpg"), id("a2.jpg")]);
    folders.insert("Ben/..".to_string(), vec![id("a1.jpg")]);
    folders.insert("Nobody".to_string(), vec![987_654]);
    let task =
        s.e.core
            .export(ExportRequest {
                ids: vec![],
                dest: dest.to_string_lossy().into_owned(),
                long_edge: None,
                quality: 90,
                name_template: "{name}".into(),
                apply_edits: true,
                folders: Some(folders),
            })
            .await
            .unwrap();
    let last = tokio::time::timeout(WAIT, async {
        loop {
            if let Ok(Event::TaskProgress {
                task_id,
                state,
                done,
                total,
                error,
                ..
            }) = rx.recv().await
            {
                if task_id == task && state != "running" {
                    return (state, done, total, error);
                }
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        (last.0.as_str(), last.1, last.2),
        ("done", 3, 3),
        "{last:?}"
    );
    assert!(dest.join("Anna").join("a1.jpg").is_file());
    assert!(dest.join("Anna").join("a2.jpg").is_file());
    let ben: Vec<_> = std::fs::read_dir(&dest)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(ben.iter().any(|n| n.starts_with("Ben")), "{ben:?}");
    assert!(!ben.contains(&"Nobody".to_string()));
    assert!(!out.path().join("a1.jpg").exists(), "no path traversal");

    // ids and folders are mutually exclusive; neither is an error too
    let mk = |ids: Vec<i64>, folders: Option<std::collections::BTreeMap<String, Vec<i64>>>| {
        ExportRequest {
            ids,
            dest: dest.to_string_lossy().into_owned(),
            long_edge: None,
            quality: 90,
            name_template: "{name}".into(),
            apply_edits: true,
            folders,
        }
    };
    let f: std::collections::BTreeMap<String, Vec<i64>> =
        [("X".to_string(), vec![id("a1.jpg")])].into();
    assert!(matches!(
        s.e.core.export(mk(vec![id("a1.jpg")], Some(f))).await,
        Err(CoreError::BadRequest(_))
    ));
    assert!(matches!(
        s.e.core.export(mk(vec![], None)).await,
        Err(CoreError::BadRequest(_))
    ));
}

// ------------------------------------------------------------------ has_edits query

#[tokio::test]
async fn photo_query_filters_edited_and_unflagged() {
    let s = scene().await;
    let a1 = s.ph["a1.jpg"].id;
    s.e.core
        .put_edit(a1, beauty_stack(None, 20.0))
        .await
        .unwrap();
    s.e.core
        .patch_photos(PatchRequest {
            ids: vec![s.ph["a2.jpg"].id],
            flag: Some(1),
            ..Default::default()
        })
        .await
        .unwrap();
    let q = |f: &dyn Fn(&mut PhotoQuery)| {
        let mut q = PhotoQuery {
            session_id: s.sid,
            limit: Some(100),
            ..Default::default()
        };
        f(&mut q);
        q
    };
    let edited =
        s.e.core
            .photos(q(&|q| q.has_edits = Some(true)))
            .await
            .unwrap();
    assert_eq!(
        edited.photos.iter().map(|p| p.id).collect::<Vec<_>>(),
        vec![a1]
    );
    let plain =
        s.e.core
            .photos(q(&|q| q.has_edits = Some(false)))
            .await
            .unwrap();
    assert_eq!(plain.total, 6);
    let undecided =
        s.e.core
            .photos(q(&|q| q.flag = FlagFilter::Unflagged))
            .await
            .unwrap();
    assert_eq!(undecided.total, 6);
    assert!(!undecided.photos.iter().any(|p| p.file_name == "a2.jpg"));
}

// ------------------------------------------------------------------ personalised scoring

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as f64) / ((1u64 << 31) as f64)
    }
}

/// `n` photos an hour apart, each its own burst. The "saturation" of photo i is hidden in its
/// embedding direction; sharpness and aesthetic (the base score) are unrelated noise.
async fn taste_library(e: &Env, n: usize) -> (i64, Vec<(i64, f64)>) {
    let mut rng = Lcg(42);
    let mut sats = Vec::new();
    for i in 0..n {
        let name = format!("t{i:03}.jpg");
        photo(e.src.path(), &name, i as u64 * 3600);
        let sat = rng.next();
        sats.push(sat);
        let mut spec = FakeSpec::at(0.0);
        spec.emb = vec![sat as f32, (1.0 - sat) as f32, 0.3, 0.1];
        spec.sharpness = 0.4 + 0.6 * rng.next();
        spec.aesthetic = Some(0.3 + 0.6 * rng.next());
        spec.phash = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        e.worker.set(&name, spec);
    }
    let sid = import(e).await;
    analyze(e, sid).await;
    let ph = by_name(e, sid).await;
    let list = (0..n)
        .map(|i| (ph[&format!("t{i:03}.jpg")].id, sats[i]))
        .collect();
    (sid, list)
}

async fn rate(e: &Env, ratings: &[(i64, i64)]) {
    for r in 1..=5 {
        let ids: Vec<i64> = ratings
            .iter()
            .filter(|(_, v)| *v == r)
            .map(|(i, _)| *i)
            .collect();
        if !ids.is_empty() {
            e.core
                .patch_photos(PatchRequest {
                    ids,
                    user_rating: Some(Some(r)),
                    ..Default::default()
                })
                .await
                .unwrap();
        }
    }
}

async fn scores(e: &Env) -> Vec<(i64, f64, f64)> {
    e.core
        .db
        .call(|c| {
            let mut st = c.prepare(
                "SELECT id, ai_score, COALESCE(base_score, ai_score) FROM photo ORDER BY id",
            )?;
            let v = st
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(v)
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn taste_learns_a_hidden_preference_and_resets() {
    let e = env();
    let (sid, lib) = taste_library(&e, 120).await;
    let before = scores(&e).await;
    assert!(before.iter().all(|(_, s, b)| (s - b).abs() < 1e-9));
    let t0 = e.core.taste().await.unwrap();
    assert_eq!((t0.labels, t0.active, t0.alpha), (0, false, 0.0));

    // the user prefers saturated photos
    let ratings: Vec<(i64, i64)> = lib
        .iter()
        .map(|(id, sat)| (*id, 1 + (sat * 4.999) as i64))
        .collect();
    let mut rx = e.core.events.subscribe();
    rate(&e, &ratings).await;
    assert_eq!(e.core.taste().await.unwrap().labels, 120);
    let t = e.core.retrain_taste().await.unwrap();
    assert!(t.active, "{t:?}");
    let (hold, base) = (t.holdout_accuracy.unwrap(), t.base_accuracy.unwrap());
    assert!(
        hold > 0.8 && hold > base + 0.2,
        "holdout {hold} vs base {base}"
    );
    assert!(t.alpha > 0.3 && t.alpha <= 0.6, "{t:?}");
    assert_eq!(t.labels, 120);
    assert!(t.updated_at.is_some() && t.pairs > 100);

    // events
    let mut taste_events = 0;
    let mut analysis_events = 0;
    while let Ok(ev) = rx.try_recv() {
        match ev {
            Event::TasteUpdated {
                labels: 120,
                active: true,
                ..
            } => taste_events += 1,
            Event::AnalysisUpdated { session_id, ids } if session_id == sid && !ids.is_empty() => {
                analysis_events += 1
            }
            _ => {}
        }
    }
    assert!(taste_events >= 1 && analysis_events >= 1);

    // fused scores follow the preference more than the base scores did
    let after = scores(&e).await;
    assert!(after.iter().any(|(_, s, b)| (s - b).abs() > 1e-3));
    let sat_of: HashMap<i64, f64> = lib.iter().copied().collect();
    let gap = |pick: &dyn Fn(&(i64, f64, f64)) -> f64| {
        let mean = |f: &dyn Fn(f64) -> bool| {
            let v: Vec<f64> = after
                .iter()
                .filter(|r| f(sat_of[&r.0]))
                .map(&pick)
                .collect();
            v.iter().sum::<f64>() / v.len() as f64
        };
        mean(&|s| s > 0.75) - mean(&|s| s < 0.25)
    };
    assert!(
        gap(&|r| r.1) > gap(&|r| r.2) + 0.03,
        "fused {} base {}",
        gap(&|r| r.1),
        gap(&|r| r.2)
    );
    // stars follow the existing mapping and stay in range
    let ph = by_name(&e, sid).await;
    assert!(ph.values().all(|p| {
        let r = p.ai_rating.unwrap();
        (0.0..=5.0).contains(&r) && (r * 2.0).fract() == 0.0
    }));
    // the detail explains the taste contribution for a photo whose score moved
    let moved = after
        .iter()
        .find(|(_, s, b)| (s - b).abs() > 0.01)
        .unwrap()
        .0;
    let d = e.core.photo_analysis(moved).await.unwrap();
    assert!(d.contributions.iter().any(|c| c.key == "taste"));
    // traits are i18n keys
    assert!(t
        .traits
        .iter()
        .all(|v| v["key"].as_str().unwrap().starts_with("taste.")));

    // a later re-score (e.g. after a group edit) keeps using the model
    // reset: labels gone, base scores back
    e.core.reset_taste().await.unwrap();
    let t = e.core.taste().await.unwrap();
    assert_eq!((t.labels, t.active, t.alpha), (0, false, 0.0));
    assert!(t.holdout_accuracy.is_none());
    let restored = scores(&e).await;
    assert!(restored.iter().all(|(_, s, b)| (s - b).abs() < 1e-9));
    for ((_, _, base_before), (_, s, _)) in before.iter().zip(&restored) {
        assert!((base_before - s).abs() < 1e-3);
    }
}

#[tokio::test]
async fn taste_stays_inactive_without_signal() {
    let e = env();
    let (_, lib) = taste_library(&e, 120).await;
    let mut rng = Lcg(7);
    let ratings: Vec<(i64, i64)> = lib
        .iter()
        .map(|(id, _)| (*id, 1 + (rng.next() * 4.999) as i64))
        .collect();
    rate(&e, &ratings).await;
    let t = e.core.retrain_taste().await.unwrap();
    assert!(!t.active, "{t:?}");
    assert_eq!(t.alpha, 0.0);
    assert!(scores(&e)
        .await
        .iter()
        .all(|(_, s, b)| (s - b).abs() < 1e-9));

    // too few labels: nothing is trained either
    let e2 = env();
    let (_, lib2) = taste_library(&e2, 12).await;
    let r2: Vec<(i64, i64)> = lib2
        .iter()
        .map(|(id, s)| (*id, 1 + (s * 4.999) as i64))
        .collect();
    rate(&e2, &r2).await;
    let t2 = e2.core.retrain_taste().await.unwrap();
    assert!(!t2.active && t2.holdout_accuracy.is_none());
}

#[tokio::test]
async fn labels_come_from_user_actions_only_and_retrain_every_50() {
    let e = env();
    let (_, lib) = taste_library(&e, 60).await;
    let ids: Vec<i64> = lib.iter().map(|l| l.0).collect();
    // a rating, a flag, clearing: one label per (photo, kind)
    let p = |ids: Vec<i64>, r: Option<Option<i64>>, f: Option<i64>| PatchRequest {
        ids,
        user_rating: r,
        flag: f,
        ..Default::default()
    };
    e.core
        .patch_photos(p(vec![ids[0]], Some(Some(4)), Some(1)))
        .await
        .unwrap();
    assert_eq!(e.core.taste().await.unwrap().labels, 2);
    e.core
        .patch_photos(p(vec![ids[0]], Some(Some(5)), None))
        .await
        .unwrap();
    assert_eq!(e.core.taste().await.unwrap().labels, 2);
    e.core
        .patch_photos(p(vec![ids[0]], Some(None), Some(0)))
        .await
        .unwrap();
    assert_eq!(e.core.taste().await.unwrap().labels, 0);
    // colour labels and rating 0 are not preferences
    e.core
        .patch_photos(PatchRequest {
            ids: vec![ids[1]],
            color_label: Some(Some("red".into())),
            ..Default::default()
        })
        .await
        .unwrap();
    e.core
        .patch_photos(p(vec![ids[1]], Some(Some(0)), None))
        .await
        .unwrap();
    assert_eq!(e.core.taste().await.unwrap().labels, 0);
    // accept-ai records the accepted stars; AI-originated analysis writes record nothing
    assert!(e.core.photo(ids[2]).await.unwrap().ai_rating.is_some());
    e.core.accept_ai(vec![ids[2], ids[3]]).await.unwrap();
    let n = e.core.taste().await.unwrap().labels;
    assert!((1..=2).contains(&n), "{n}");

    // 50 labels trigger a background retraining by themselves
    let mut rx = e.core.events.subscribe();
    let ratings: Vec<(i64, i64)> = lib
        .iter()
        .map(|(id, s)| (*id, 1 + (s * 4.999) as i64))
        .collect();
    rate(&e, &ratings).await;
    tokio::time::timeout(WAIT, async {
        loop {
            if let Ok(Event::TasteUpdated { labels, .. }) = rx.recv().await {
                assert!(labels >= 50);
                return;
            }
        }
    })
    .await
    .expect("taste.updated after 50 labels");
    assert!(e.core.taste().await.unwrap().updated_at.is_some());
}

#[test]
fn taste_unit_pieces() {
    use crate::taste::{alpha_for, ALPHA_MAX};
    assert_eq!(alpha_for(0), 0.0);
    assert!(alpha_for(50) > 0.0 && alpha_for(50) < alpha_for(100));
    assert_eq!(alpha_for(10_000), ALPHA_MAX);
}
