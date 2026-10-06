//! M5 tests: best take, inpainting, enhancement, patch assets, singleton people, export upscale
//! and metadata, worker timeouts. FakeImaging + FakeWorker + FakeRenderer.

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
use crate::fake_worker::{
    person, FakeFace, FakeSpec, FakeWorker, BESTTAKE_MODEL, ENHANCE_MODEL, INPAINT_MODEL,
    SDXL_MODEL,
};
use crate::generate::{auto_choices, body_box, plan_from_tracks, MaskCanvas, Stroke};
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

async fn import_dir(e: &Env, dir: &Path) -> i64 {
    let s = e
        .core
        .import(ImportRequest {
            path: dir.to_string_lossy().into_owned(),
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

fn face(bbox: [f64; 4], who: usize, jitter: f32) -> FakeFace {
    FakeFace::new(bbox, person(who, jitter))
}

fn at(f: FakeFace, eyes: f64, smile: f64, yaw: f64) -> FakeFace {
    FakeFace {
        yaw: Some(yaw),
        ..f.eyes(eyes).smile(smile)
    }
}

const FA: [f64; 4] = [0.15, 0.3, 0.25, 0.35];
const FB: [f64; 4] = [0.55, 0.3, 0.22, 0.32];

struct Scene {
    e: Env,
    sid: i64,
    ph: HashMap<String, Photo>,
    burst: i64,
    /// a1 is the sharpest frame of the burst (the base).
    a: i64,
    b: i64,
}

/// a1 a2 a3 (one burst, a1 sharpest = base) show A and B. A: a1 weak smile, a2 great, a3 good
/// but turned away (yaw 60). B: closed eyes in a1, fine in a2 and a3 (a3 best). p1 has a main
/// subject and a small bystander; c1 shows a stranger; d1 has no faces.
async fn scene() -> Scene {
    let e = env();
    let d = e.src.path();
    for (n, t) in [
        ("a1.jpg", 0),
        ("a2.jpg", 1),
        ("a3.jpg", 2),
        ("p1.jpg", 3600),
        ("c1.jpg", 7200),
        ("d1.jpg", 10800),
    ] {
        photo(d, n, t);
    }
    let spec = |angle: f32, sharp: f64, iqa: f64, faces: Vec<FakeFace>| {
        let mut s = FakeSpec::at(angle).sharp(sharp).with_faces(faces);
        s.iqa = Some(iqa);
        s.aesthetic = Some(iqa);
        s
    };
    e.worker.set(
        "a1.jpg",
        spec(
            0.0,
            0.99,
            1.0,
            vec![
                at(face(FA, 0, 1.0), 0.95, 0.4, 2.0),
                at(face(FB, 1, 1.0), 0.1, 0.5, 2.0),
            ],
        ),
    );
    e.worker.set(
        "a2.jpg",
        spec(
            2.0,
            0.2,
            0.1,
            vec![
                at(face(FA, 0, 0.0), 0.95, 0.9, 5.0),
                at(face(FB, 1, 0.0), 0.95, 0.7, 4.0),
            ],
        ),
    );
    e.worker.set(
        "a3.jpg",
        spec(
            3.0,
            0.2,
            0.1,
            vec![
                at(face(FA, 0, 2.0), 0.95, 0.6, 60.0),
                at(face(FB, 1, 2.0), 0.95, 0.95, 3.0),
            ],
        ),
    );
    e.worker.set(
        "p1.jpg",
        FakeSpec::at(100.0).with_faces(vec![
            face([0.3, 0.2, 0.3, 0.4], 7, 0.0),
            face([0.8, 0.7, 0.06, 0.08], 6, 0.0),
        ]),
    );
    e.worker.set(
        "c1.jpg",
        FakeSpec::at(200.0).with_faces(vec![face([0.4, 0.4, 0.2, 0.3], 5, 0.0)]),
    );
    e.worker.set("d1.jpg", FakeSpec::at(300.0));
    let sid = import_dir(&e, d).await;
    analyze(&e, sid).await;
    let ph = by_name(&e, sid).await;
    let burst = ph["a1.jpg"].burst_id.expect("a1 is in a burst");
    let faces = e.core.photo_analysis(ph["a1.jpg"].id).await.unwrap().faces;
    let a = faces[0].person_id.expect("person A");
    let b = faces[1].person_id.expect("person B");
    Scene {
        e,
        sid,
        ph,
        burst,
        a,
        b,
    }
}

async fn faces_of(e: &Env, photo: i64) -> Vec<Face> {
    e.core.photo_analysis(photo).await.unwrap().faces
}

async fn patches_of(e: &Env, photo: i64) -> Vec<Value> {
    ops(&e.core.get_edit(photo).await.unwrap().stack)
        .into_iter()
        .filter(|o| o["type"] == "patch")
        .collect()
}

fn ops(stack: &Value) -> Vec<Value> {
    stack["ops"].as_array().cloned().unwrap_or_default()
}

fn op_types(stack: &Value) -> Vec<String> {
    ops(stack)
        .iter()
        .map(|o| o["type"].as_str().unwrap_or("").to_string())
        .collect()
}

/// Receives events until `pred` returns true; panics after [`WAIT`].
async fn wait_for(
    rx: &mut tokio::sync::broadcast::Receiver<Event>,
    mut pred: impl FnMut(&Event) -> bool,
) -> Vec<Event> {
    let mut seen = Vec::new();
    tokio::time::timeout(WAIT, async {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let done = pred(&ev);
                    seen.push(ev);
                    if done {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(e) => panic!("event stream closed: {e}"),
            }
        }
    })
    .await
    .expect("the awaited event arrives");
    seen
}

async fn run_besttake(
    e: &Env,
    base: i64,
    choices: Vec<BestTakeChoice>,
) -> (Vec<BestTakeResult>, Vec<Event>) {
    let mut rx = e.core.events.subscribe();
    e.core
        .besttake_start(BestTakeRequest {
            base_photo_id: base,
            choices,
        })
        .await
        .unwrap();
    let seen = wait_for(&mut rx, |ev| {
        matches!(ev, Event::TaskProgress { kind, state, .. } if kind == "besttake" && state != "running")
    })
    .await;
    let results = seen
        .iter()
        .find_map(|ev| match ev {
            Event::BestTakeDone { results, .. } => Some(results.clone()),
            _ => None,
        })
        .expect("besttake.done precedes the final task progress");
    (results, seen)
}

fn choice(base_face: i64, src_photo: i64, src_face: i64) -> BestTakeChoice {
    BestTakeChoice {
        base_face_id: base_face,
        source_photo_id: src_photo,
        source_face_id: src_face,
    }
}

// ------------------------------------------------------------------ best take plan

#[tokio::test]
async fn plan_lists_candidates_by_expression_with_composability() {
    let s = scene().await;
    let (a1, a2, a3) = (s.ph["a1.jpg"].id, s.ph["a2.jpg"].id, s.ph["a3.jpg"].id);
    let g = s.e.core.groups(s.sid).await.unwrap();
    let burst = g
        .scenes
        .iter()
        .flat_map(|sc| &sc.bursts)
        .find(|b| b.id == s.burst)
        .unwrap();
    assert_eq!(
        burst.best_photo_id,
        Some(a1),
        "a1 is the burst's best frame"
    );

    let plan = s.e.core.besttake_plan(s.burst).await.unwrap();
    assert_eq!(plan.base_photo_id, a1);
    assert_eq!(plan.people.len(), 2);
    let pa = plan
        .people
        .iter()
        .find(|p| p.person_id == Some(s.a))
        .unwrap();
    let pb = plan
        .people
        .iter()
        .find(|p| p.person_id == Some(s.b))
        .unwrap();
    let fa1 = faces_of(&s.e, a1).await;
    assert_eq!(pa.base_face_id, fa1[0].id);
    assert_eq!(pb.base_face_id, fa1[1].id);

    // A: a2 (smile .9) beats a3 (smile .6), but a3 is turned away (60 vs 2 degrees)
    assert_eq!(pa.candidates.len(), 2);
    let c2 = pa.candidates.iter().find(|c| c.photo_id == a2).unwrap();
    let c3 = pa.candidates.iter().find(|c| c.photo_id == a3).unwrap();
    assert!(c2.composable && c2.reason.is_none());
    assert!(!c3.composable);
    assert_eq!(c3.reason.as_deref(), Some("large_pose_change"));
    assert!(c2.expression_score > c3.expression_score);
    assert_eq!(pa.candidates[0].photo_id, a2, "best expression first");
    assert_eq!(pa.best_photo_id, a2);
    // B: eyes closed in the base; a3 has the best expression and is composable for B
    assert_eq!(pb.candidates[0].photo_id, a3);
    assert!(pb.candidates.iter().all(|c| c.composable));
    assert_eq!(pb.best_photo_id, a3);
    // the face ids are the candidates' own faces
    assert_eq!(c2.face_id, faces_of(&s.e, a2).await[0].id);

    // auto choices: one per person whose best take is not the base
    let choices = auto_choices(&plan);
    assert_eq!(choices.len(), 2);
    assert!(choices
        .iter()
        .any(|c| c.base_face_id == pa.base_face_id && c.source_photo_id == a2));

    // unknown burst
    assert!(matches!(
        s.e.core.besttake_plan(999_999).await,
        Err(CoreError::NotFound(_))
    ));
}

#[test]
fn plan_from_tracks_handles_missing_faces_and_unknown_poses() {
    let f = |id: i64, photo: i64, yaw: Option<f64>, expr: Option<f64>| Face {
        id,
        photo_id: photo,
        person_id: Some(1),
        person_name: None,
        bbox: [0.1, 0.1, 0.2, 0.2],
        eyes_open: None,
        smile: None,
        gaze: None,
        yaw,
        pitch: None,
        roll: None,
        sharpness: None,
        expression_score: expr,
        is_subject: true,
    };
    let mut cells = std::collections::BTreeMap::new();
    cells.insert("10".to_string(), Some(f(1, 10, Some(0.0), Some(0.5))));
    cells.insert("11".to_string(), Some(f(2, 11, None, Some(0.9)))); // pose unknown: allowed
    cells.insert("12".to_string(), None); // face absent: no candidate
    let mut gone = std::collections::BTreeMap::new();
    gone.insert("10".to_string(), None); // not in the base: no entry
    gone.insert("11".to_string(), Some(f(3, 11, None, Some(0.2))));
    gone.insert("12".to_string(), None);
    let out = BurstFacesOut {
        photo_ids: vec![10, 11, 12],
        tracks: vec![
            TrackOut {
                track_id: 0,
                person_id: Some(1),
                person_name: None,
                cells,
                best_photo_ids: vec![],
            },
            TrackOut {
                track_id: 1,
                person_id: None,
                person_name: None,
                cells: gone,
                best_photo_ids: vec![],
            },
        ],
    };
    let plan = plan_from_tracks(&out).unwrap();
    assert_eq!(plan.base_photo_id, 10);
    assert_eq!(plan.people.len(), 1);
    assert_eq!(plan.people[0].candidates.len(), 1);
    assert!(plan.people[0].candidates[0].composable);
    assert_eq!(plan.people[0].best_photo_id, 11);
    assert!(plan_from_tracks(&BurstFacesOut {
        photo_ids: vec![],
        tracks: vec![]
    })
    .is_none());
}

// ------------------------------------------------------------------ best take task

#[tokio::test]
async fn besttake_task_replaces_the_persons_patch_and_emits_events() {
    let s = scene().await;
    let (a1, a2, a3) = (s.ph["a1.jpg"].id, s.ph["a2.jpg"].id, s.ph["a3.jpg"].id);
    let (fa1, fa2, fa3) = (
        faces_of(&s.e, a1).await,
        faces_of(&s.e, a2).await,
        faces_of(&s.e, a3).await,
    );
    // an existing edit stays and the patch goes in front of it
    s.e.core
        .put_edit(
            a1,
            json!({"version":1,"ops":[{"type":"global","exposure":0.5}]}),
        )
        .await
        .unwrap();

    let (results, seen) = run_besttake(&s.e, a1, vec![choice(fa1[0].id, a2, fa2[0].id)]).await;
    assert_eq!(results.len(), 1);
    assert!(results[0].ok, "{results:?}");
    assert_eq!(results[0].base_face_id, fa1[0].id);
    assert!(results[0].warnings.is_empty() && results[0].reason.is_none());

    // progress events: running with 0/1 and 1/1, then done
    let states: Vec<(i64, String)> = seen
        .iter()
        .filter_map(|ev| match ev {
            Event::TaskProgress {
                kind,
                done,
                total,
                state,
                ..
            } if kind == "besttake" => {
                assert_eq!(*total, 1);
                Some((*done, state.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(states.first(), Some(&(0, "running".to_string())));
    assert_eq!(states.last(), Some(&(1, "done".to_string())));
    // edits.updated for the base photo arrives before besttake.done
    let i_edit = seen
        .iter()
        .position(|e| matches!(e, Event::EditsUpdated { items } if items.iter().any(|i| i.id == a1 && i.has_edits)))
        .expect("edits.updated");
    let i_done = seen
        .iter()
        .position(|e| matches!(e, Event::BestTakeDone { photo_id, .. } if *photo_id == a1))
        .unwrap();
    assert!(i_edit < i_done);

    // the worker was asked with the right faces
    let req =
        s.e.worker
            .last_compose_request
            .lock()
            .unwrap()
            .clone()
            .unwrap();
    assert_eq!((req.base.photo_id, req.source.photo_id), (a1, a2));
    assert_eq!(req.base_face, fa1[0].bbox);
    assert_eq!(req.source_face, fa2[0].bbox);

    // stack: patch first (before the global op), tracked by person
    let stack = s.e.core.get_edit(a1).await.unwrap().stack;
    assert_eq!(op_types(&stack), ["patch", "global"]);
    let p = &ops(&stack)[0];
    assert_eq!(p["kind"], "best_take");
    assert_eq!(p["person_id"], json!(s.a));
    assert_eq!(p["source_photo_id"], json!(a2));
    assert_eq!(p["base_face_id"], json!(fa1[0].id));
    assert_eq!(p["enabled"], json!(true));
    let asset1 = p["asset"].as_str().unwrap().to_string();
    assert!(s.e.core.render.patches.exists(a1, &asset1));
    assert!(s.e.core.photo(a1).await.unwrap().thumb_version.len() > 4);
    // the asset is a PNG the renderer can read back, and the rect surrounds A's face
    let img = s.e.core.render.patches.load(a1, &asset1).unwrap().unwrap();
    assert_eq!((img.width, img.height), (24, 16));
    let rect: Vec<f64> = p["rect"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    assert!(rect[0] <= FA[0] && rect[0] + rect[2] >= FA[0] + FA[2]);

    // B: a second person is inserted next to A's patch
    let (results, _) = run_besttake(&s.e, a1, vec![choice(fa1[1].id, a3, fa3[1].id)]).await;
    assert!(results[0].ok);
    let ps = patches_of(&s.e, a1).await;
    assert_eq!(ps.len(), 2);
    assert!(ps.iter().any(|p| p["person_id"] == json!(s.a)));
    assert!(ps.iter().any(|p| p["person_id"] == json!(s.b)));

    // a source turned away from the posted base is refused without asking the worker
    let calls = s.e.worker.compose_calls.load(Ordering::SeqCst);
    let (results, _) = run_besttake(&s.e, a1, vec![choice(fa1[0].id, a3, fa3[0].id)]).await;
    assert_eq!(results[0].reason.as_deref(), Some("large_pose_change"));
    assert_eq!(s.e.worker.compose_calls.load(Ordering::SeqCst), calls);
    // replacing A's take (a2 again) swaps A's patch only: still two patches, new asset
    let (results, _) = run_besttake(&s.e, a1, vec![choice(fa1[0].id, a2, fa2[0].id)]).await;
    assert!(results[0].ok);
    let ps = patches_of(&s.e, a1).await;
    assert_eq!(ps.len(), 2);
    let pa = ps.iter().find(|p| p["person_id"] == json!(s.a)).unwrap();
    assert_eq!(pa["source_photo_id"], json!(a2));
    let asset2 = pa["asset"].as_str().unwrap();
    assert_ne!(asset2, asset1);
    assert!(s.e.core.render.patches.exists(a1, asset2));
    // (the previous versions keep their assets until they fall out of the retained history)
    let ph = s.e.core.photo(a1).await.unwrap();
    assert!(ph.thumb_version.contains('-') || !ph.thumb_version.is_empty());
}

#[tokio::test]
async fn besttake_reports_each_choice_and_validates_the_request() {
    let s = scene().await;
    let (a1, a2, a3, c1) = (
        s.ph["a1.jpg"].id,
        s.ph["a2.jpg"].id,
        s.ph["a3.jpg"].id,
        s.ph["c1.jpg"].id,
    );
    let (fa1, fa2, fa3) = (
        faces_of(&s.e, a1).await,
        faces_of(&s.e, a2).await,
        faces_of(&s.e, a3).await,
    );

    // the worker refuses one source: that choice fails with its reason, the other succeeds
    s.e.worker.refuse_compose(a3, "camera_moved");
    s.e.worker
        .compose_warnings
        .lock()
        .unwrap()
        .push("seam".into());
    let (results, _) = run_besttake(
        &s.e,
        a1,
        vec![
            choice(fa1[0].id, a2, fa2[0].id),
            choice(fa1[1].id, a3, fa3[1].id),
        ],
    )
    .await;
    assert!(results[0].ok);
    assert_eq!(results[0].warnings, vec!["seam".to_string()]);
    assert!(!results[1].ok);
    assert_eq!(results[1].reason.as_deref(), Some("camera_moved"));
    let ps = patches_of(&s.e, a1).await;
    assert_eq!(ps.len(), 1, "only the successful choice is stored");

    // all choices fail -> the task fails (and nothing changes)
    s.e.worker.refuse_compose(a2, "occlusion");
    let before = s.e.core.get_edit(a1).await.unwrap().stack;
    let (results, seen) = run_besttake(&s.e, a1, vec![choice(fa1[0].id, a2, fa2[0].id)]).await;
    assert!(!results[0].ok);
    assert!(seen.iter().any(|e| matches!(e,
        Event::TaskProgress { kind, state, error, .. } if kind == "besttake" && state == "failed" && error.as_deref() == Some("occlusion"))));
    assert_eq!(s.e.core.get_edit(a1).await.unwrap().stack, before);

    // request validation
    let start = |base: i64, choices: Vec<BestTakeChoice>| {
        let core = s.e.core.clone();
        async move {
            core.besttake_start(BestTakeRequest {
                base_photo_id: base,
                choices,
            })
            .await
        }
    };
    assert!(matches!(
        start(a1, vec![]).await,
        Err(CoreError::BadRequest(_))
    ));
    assert!(matches!(
        start(999_999, vec![choice(fa1[0].id, a2, fa2[0].id)]).await,
        Err(CoreError::NotFound(_))
    ));
    // a face that is not in the base photo / not in the source photo / same photo
    assert!(matches!(
        start(a1, vec![choice(fa2[0].id, a2, fa2[0].id)]).await,
        Err(CoreError::Unprocessable(_))
    ));
    assert!(matches!(
        start(a1, vec![choice(fa1[0].id, a3, fa2[0].id)]).await,
        Err(CoreError::Unprocessable(_))
    ));
    assert!(matches!(
        start(a1, vec![choice(fa1[0].id, a1, fa1[0].id)]).await,
        Err(CoreError::Unprocessable(_))
    ));
    assert!(matches!(
        start(
            a1,
            vec![
                choice(fa1[0].id, a2, fa2[0].id),
                choice(fa1[0].id, a3, fa3[0].id)
            ]
        )
        .await,
        Err(CoreError::Unprocessable(_))
    ));
    assert!(matches!(
        start(a1, vec![choice(999_999, a2, fa2[0].id)]).await,
        Err(CoreError::NotFound(_))
    ));
    let _ = c1;

    // models missing -> 409 before anything starts; worker down -> 503
    s.e.worker.set_missing(&[BESTTAKE_MODEL]);
    match start(a1, vec![choice(fa1[0].id, a2, fa2[0].id)]).await {
        Err(CoreError::ModelsMissing(m)) => assert_eq!(m, vec![BESTTAKE_MODEL.to_string()]),
        other => panic!("expected models_missing, got {other:?}"),
    }
    s.e.worker.set_missing(&[]);
    s.e.worker.models_unavailable.store(true, Ordering::SeqCst);
    assert!(matches!(
        start(a1, vec![choice(fa1[0].id, a2, fa2[0].id)]).await,
        Err(CoreError::WorkerUnavailable(_))
    ));
}

#[tokio::test]
async fn besttake_auto_composes_the_best_take_of_every_person() {
    let s = scene().await;
    let (a1, a2, a3) = (s.ph["a1.jpg"].id, s.ph["a2.jpg"].id, s.ph["a3.jpg"].id);
    let mut rx = s.e.core.events.subscribe();
    let task = s.e.core.besttake_auto(s.burst).await.unwrap();
    assert!(task.starts_with("besttake-"));
    let seen = wait_for(&mut rx, |e| matches!(e, Event::BestTakeDone { .. })).await;
    let results = match seen.last().unwrap() {
        Event::BestTakeDone { photo_id, results } => {
            assert_eq!(*photo_id, a1);
            results.clone()
        }
        _ => unreachable!(),
    };
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|r| r.ok), "{results:?}");
    let ps = patches_of(&s.e, a1).await;
    assert_eq!(ps.len(), 2);
    let src = |person: i64| {
        ps.iter().find(|p| p["person_id"] == json!(person)).unwrap()["source_photo_id"].clone()
    };
    assert_eq!(src(s.a), json!(a2));
    assert_eq!(src(s.b), json!(a3));

    // a burst with nothing to improve still answers with a (empty) done event
    let g = s.e.core.groups(s.sid).await.unwrap();
    let lonely = g
        .scenes
        .iter()
        .flat_map(|sc| &sc.bursts)
        .find(|b| b.photo_ids == vec![s.ph["p1.jpg"].id])
        .map(|b| b.id);
    if let Some(burst) = lonely {
        let mut rx = s.e.core.events.subscribe();
        s.e.core.besttake_auto(burst).await.unwrap();
        let seen = wait_for(&mut rx, |e| matches!(e, Event::BestTakeDone { .. })).await;
        assert!(
            matches!(seen.last(), Some(Event::BestTakeDone { results, .. }) if results.is_empty())
        );
    }
}

// ------------------------------------------------------------------ inpaint

#[test]
fn mask_canvas_strokes_have_round_caps_and_follow_the_polyline() {
    let mut c = MaskCanvas::new(200, 100);
    // radius 0.05 of the long edge (200) = 10 px
    c.draw_strokes(&[Stroke {
        points: vec![[0.25, 0.5], [0.75, 0.5]],
        radius: 0.05,
    }]);
    let px = |x: u32, y: u32| c.data[(y * 200 + x) as usize];
    assert_eq!(px(100, 50), 255);
    assert_eq!(px(100, 59), 255); // within the radius
    assert_eq!(px(100, 62), 0); // beyond it
    assert_eq!(px(44, 50), 255); // round cap beyond the end point (x=50) by 6 px
    assert_eq!(px(30, 50), 0);
    assert_eq!(px(44, 58), 0); // the cap is round, not square: corner is outside
                               // a single point is a disc
    let mut d = MaskCanvas::new(100, 100);
    d.draw_strokes(&[Stroke {
        points: vec![[0.5, 0.5]],
        radius: 0.1,
    }]);
    assert_eq!(d.data[50 * 100 + 50], 255);
    assert_eq!(d.data[50 * 100 + 59], 255);
    assert_eq!(d.data[50 * 100 + 65], 0);
    assert_eq!(d.data[(50 + 8) * 100 + 58], 0); // diagonal corner of the square
    let b = d.bounds().unwrap();
    assert!(
        (b[0] - 0.4).abs() < 0.02 && (b[2] - 0.2).abs() < 0.02,
        "{b:?}"
    );
}

#[test]
fn mask_canvas_union_dilate_and_fallback_box() {
    let mut c = MaskCanvas::new(100, 100);
    let other = image::GrayImage::from_fn(10, 10, |x, y| {
        image::Luma([if (3..6).contains(&x) && (3..6).contains(&y) {
            255
        } else {
            0
        }])
    });
    c.union(&other);
    assert_eq!(c.data[40 * 100 + 40], 255);
    assert_eq!(c.data[10 * 100 + 10], 0);
    let before = c.count();
    c.dilate(4);
    assert!(c.count() > before);
    assert_eq!(c.data[40 * 100 + 27], 255); // 3 px left of the 30..60 block, within 4
    assert_eq!(c.data[40 * 100 + 24], 0);
    // body box stays inside the image and extends below the face
    let b = body_box([0.8, 0.5, 0.15, 0.2]);
    assert!(b[0] + b[2] <= 1.0001 && b[1] + b[3] <= 1.0001);
    assert!(b[1] + b[3] > 0.5 + 0.2);
    let mut f = MaskCanvas::new(100, 100);
    f.fill_rect([0.1, 0.1, 0.2, 0.2]);
    assert!((400..=441).contains(&f.count()));
}

#[tokio::test]
async fn inpaint_removes_bystanders_with_person_masks_of_their_boxes() {
    let s = scene().await;
    let p1 = s.ph["p1.jpg"].id;
    s.e.worker
        .person_mask_from_bbox
        .store(true, Ordering::SeqCst);

    let by = s.e.core.bystanders(p1).await.unwrap();
    assert_eq!(by.len(), 1, "{by:?}");
    let faces = faces_of(&s.e, p1).await;
    let small = faces.iter().find(|f| !f.is_subject).unwrap();
    assert_eq!(by[0].face_id, small.id);
    assert_eq!(by[0].bbox, small.bbox);
    assert!(matches!(
        s.e.core.bystanders(999_999).await,
        Err(CoreError::NotFound(_))
    ));

    let mut rx = s.e.core.events.subscribe();
    let task =
        s.e.core
            .inpaint_start(
                p1,
                InpaintBody {
                    bystanders: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    assert!(task.starts_with("inpaint-"));
    let seen = wait_for(&mut rx, |e| matches!(e, Event::InpaintDone { .. })).await;
    match seen.last().unwrap() {
        Event::InpaintDone {
            photo_id,
            ok,
            reason,
        } => {
            assert_eq!(*photo_id, p1);
            assert!(*ok, "{reason:?}");
        }
        _ => unreachable!(),
    }
    assert!(seen
        .iter()
        .any(|e| matches!(e, Event::EditsUpdated { items } if items.iter().any(|i| i.id == p1))));

    // the worker asked for a person mask of the bystander's box only
    let mreq =
        s.e.worker
            .last_mask_request
            .lock()
            .unwrap()
            .clone()
            .unwrap();
    assert_eq!(mreq.targets, vec!["person".to_string()]);
    assert_eq!(mreq.person_bbox, Some(small.bbox));
    assert_eq!(s.e.worker.mask_calls.load(Ordering::SeqCst), 1);
    // the removal mask handed to inpaint.run: the box (+ a small dilation), nothing else
    let ireq =
        s.e.worker
            .last_inpaint_request
            .lock()
            .unwrap()
            .clone()
            .unwrap();
    assert_eq!(ireq.model, "lama");
    assert_eq!(ireq.photo.photo_id, p1);
    let m =
        s.e.worker
            .last_inpaint_mask
            .lock()
            .unwrap()
            .clone()
            .unwrap();
    assert_eq!((m.width(), m.height()), (1024, 768));
    let at = |x: u32, y: u32| m.get_pixel(x, y).0[0];
    assert_eq!(at(850, 570), 255, "inside the bystander box");
    assert_eq!(at(815, 570), 255, "grown by the dilation");
    assert_eq!(at(790, 570), 0, "far from it");
    assert_eq!(at(300, 150), 0, "the main subject is untouched");
    // the patch is in the stack as an `inpaint` op around the removed region
    let ps = patches_of(&s.e, p1).await;
    assert_eq!(ps.len(), 1);
    assert_eq!(ps[0]["kind"], "inpaint");
    let r: Vec<f64> = ps[0]["rect"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    assert!(
        r[0] <= 0.8 && r[0] + r[2] >= 0.86 && r[1] <= 0.7 && r[1] + r[3] >= 0.78,
        "{r:?}"
    );
    assert!(s
        .e
        .core
        .render
        .patches
        .exists(p1, ps[0]["asset"].as_str().unwrap()));

    // strokes: appended as a second inpaint patch; rasterised round-capped at analysis size
    let mut rx = s.e.core.events.subscribe();
    s.e.core
        .inpaint_start(
            p1,
            InpaintBody {
                strokes: Some(vec![Stroke {
                    points: vec![[0.1, 0.1], [0.3, 0.1]],
                    radius: 0.02,
                }]),
                model: Some("sdxl".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    wait_for(&mut rx, |e| {
        matches!(e, Event::InpaintDone { ok: true, .. })
    })
    .await;
    let ireq =
        s.e.worker
            .last_inpaint_request
            .lock()
            .unwrap()
            .clone()
            .unwrap();
    assert_eq!(ireq.model, "sdxl");
    let m =
        s.e.worker
            .last_inpaint_mask
            .lock()
            .unwrap()
            .clone()
            .unwrap();
    assert_eq!(m.get_pixel(200, 77).0[0], 255); // on the stroke (y = 0.1 * 768)
    assert_eq!(m.get_pixel(200, 77 + 40).0[0], 0); // beyond the radius (20 px)
    assert_eq!(m.get_pixel(95, 77).0[0], 255); // round cap before the start (x = 102)
    assert_eq!(m.get_pixel(850, 570).0[0], 0, "no bystander mask this time");
    assert_eq!(patches_of(&s.e, p1).await.len(), 2);
    // no mask requests for strokes only
    assert_eq!(s.e.worker.mask_calls.load(Ordering::SeqCst), 1);

    // explicit face_ids may name a subject too
    let big = faces.iter().find(|f| f.is_subject).unwrap();
    let mut rx = s.e.core.events.subscribe();
    s.e.core
        .inpaint_start(
            p1,
            InpaintBody {
                face_ids: Some(vec![big.id]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    wait_for(&mut rx, |e| {
        matches!(e, Event::InpaintDone { ok: true, .. })
    })
    .await;
    assert_eq!(patches_of(&s.e, p1).await.len(), 3);
}

#[tokio::test]
async fn inpaint_validation_and_failures() {
    let s = scene().await;
    let (p1, a1) = (s.ph["p1.jpg"].id, s.ph["a1.jpg"].id);
    let faces_a1 = faces_of(&s.e, a1).await;
    let start = |photo: i64, body: InpaintBody| {
        let core = s.e.core.clone();
        async move { core.inpaint_start(photo, body).await }
    };
    // nothing to remove
    assert!(matches!(
        start(p1, InpaintBody::default()).await,
        Err(CoreError::Unprocessable(_))
    ));
    // a photo whose faces are all subjects has no bystanders
    let d1 = s.ph["d1.jpg"].id;
    assert!(matches!(
        start(
            d1,
            InpaintBody {
                bystanders: true,
                ..Default::default()
            }
        )
        .await,
        Err(CoreError::Unprocessable(_))
    ));
    assert!(matches!(
        start(
            999_999,
            InpaintBody {
                bystanders: true,
                ..Default::default()
            }
        )
        .await,
        Err(CoreError::NotFound(_))
    ));
    // a face of another photo
    assert!(matches!(
        start(
            p1,
            InpaintBody {
                face_ids: Some(vec![faces_a1[0].id]),
                ..Default::default()
            }
        )
        .await,
        Err(CoreError::Unprocessable(_))
    ));
    let stroke = |pts: Vec<[f32; 2]>, r: f32| InpaintBody {
        strokes: Some(vec![Stroke {
            points: pts,
            radius: r,
        }]),
        ..Default::default()
    };
    for bad in [
        stroke(vec![], 0.02),
        stroke(vec![[0.5, 0.5]], 0.0),
        stroke(vec![[0.5, 0.5]], 0.9),
        stroke(vec![[2.0, 0.5]], 0.02),
        stroke(vec![[f32::NAN, 0.5]], 0.02),
        InpaintBody {
            model: Some("dalle".into()),
            ..stroke(vec![[0.5, 0.5]], 0.02)
        },
    ] {
        assert!(
            matches!(start(p1, bad).await, Err(CoreError::Unprocessable(_))),
            "bad request accepted"
        );
    }
    // models missing -> 409; the worker failing mid-task -> inpaint.done with ok=false
    s.e.worker.set_missing(&[INPAINT_MODEL]);
    assert!(matches!(
        start(p1, stroke(vec![[0.5, 0.5]], 0.02)).await,
        Err(CoreError::ModelsMissing(_))
    ));
    s.e.worker.set_missing(&[]);
    s.e.worker.set_mask_missing(&["person"]);
    let mut rx = s.e.core.events.subscribe();
    start(
        p1,
        InpaintBody {
            bystanders: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let seen = wait_for(&mut rx, |e| matches!(e, Event::InpaintDone { .. })).await;
    match seen.last().unwrap() {
        Event::InpaintDone { ok, reason, .. } => {
            assert!(!*ok);
            assert!(
                reason.as_deref().unwrap_or("").contains("not installed"),
                "{reason:?}"
            );
        }
        _ => unreachable!(),
    }
    assert!(patches_of(&s.e, p1).await.is_empty());
    assert_eq!(s.e.worker.inpaint_calls.load(Ordering::SeqCst), 0);
}

// ------------------------------------------------------------------ enhance

async fn enhance(e: &Env, photo: i64, op: &str) -> (bool, Option<String>) {
    let mut rx = e.core.events.subscribe();
    e.core
        .enhance_start(
            photo,
            EnhanceBody {
                op: op.into(),
                strength: Some(0.7),
            },
        )
        .await
        .unwrap();
    let seen = wait_for(&mut rx, |ev| matches!(ev, Event::EnhanceDone { .. })).await;
    match seen.last().unwrap() {
        Event::EnhanceDone {
            photo_id,
            op: o,
            ok,
            reason,
        } => {
            assert_eq!((*photo_id, o.as_str()), (photo, op));
            (*ok, reason.clone())
        }
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn enhance_replaces_patches_of_the_same_kind() {
    let s = scene().await;
    let (a1, p1) = (s.ph["a1.jpg"].id, s.ph["p1.jpg"].id);
    let kinds = |ps: &[Value]| {
        let mut k: Vec<String> = ps
            .iter()
            .map(|p| p["kind"].as_str().unwrap().to_string())
            .collect();
        k.sort();
        k
    };

    // an inpaint patch must survive every enhancement
    s.e.core
        .put_edit(
            a1,
            json!({"version":1,"ops":[{"type":"global","exposure":0.2}]}),
        )
        .await
        .unwrap();
    let mut rx = s.e.core.events.subscribe();
    s.e.core
        .inpaint_start(
            a1,
            InpaintBody {
                strokes: Some(vec![Stroke {
                    points: vec![[0.5, 0.5]],
                    radius: 0.05,
                }]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    wait_for(&mut rx, |e| {
        matches!(e, Event::InpaintDone { ok: true, .. })
    })
    .await;

    assert_eq!(enhance(&s.e, a1, "denoise").await, (true, None));
    let req =
        s.e.worker
            .enhance_requests
            .lock()
            .unwrap()
            .last()
            .cloned()
            .unwrap();
    assert_eq!((req.op.as_str(), req.strength), ("denoise", 0.7));
    assert!(req.faces.is_none());
    let ps = patches_of(&s.e, a1).await;
    assert_eq!(kinds(&ps), ["denoise", "inpaint"]);
    let first = ps.iter().find(|p| p["kind"] == "denoise").unwrap().clone();
    assert_eq!(first["rect"], json!([0.0, 0.0, 1.0, 1.0]));

    // again: replaced, not stacked
    assert_eq!(enhance(&s.e, a1, "denoise").await, (true, None));
    let ps = patches_of(&s.e, a1).await;
    assert_eq!(kinds(&ps), ["denoise", "inpaint"]);
    let second = ps.iter().find(|p| p["kind"] == "denoise").unwrap();
    assert_ne!(second["asset"], first["asset"]);

    // face restoration: one patch per face; running it again replaces them all
    assert_eq!(enhance(&s.e, a1, "face_restore").await, (true, None));
    let req =
        s.e.worker
            .enhance_requests
            .lock()
            .unwrap()
            .last()
            .cloned()
            .unwrap();
    assert_eq!(req.faces.as_ref().map(|f| f.len()), Some(2));
    assert_eq!(
        kinds(&patches_of(&s.e, a1).await),
        ["denoise", "face_restore", "face_restore", "inpaint"]
    );
    assert_eq!(enhance(&s.e, a1, "face_restore").await, (true, None));
    assert_eq!(
        kinds(&patches_of(&s.e, a1).await),
        ["denoise", "face_restore", "face_restore", "inpaint"]
    );
    // the global edit is still there and patches lead the stack
    let stack = s.e.core.get_edit(a1).await.unwrap().stack;
    let t = op_types(&stack);
    assert_eq!(t.last().map(String::as_str), Some("global"));
    assert!(t[..t.len() - 1].iter().all(|x| x == "patch"));

    // validation
    let start = |photo: i64, op: &str, strength: Option<f64>| {
        let core = s.e.core.clone();
        let op = op.to_string();
        async move {
            core.enhance_start(photo, EnhanceBody { op, strength })
                .await
        }
    };
    assert!(matches!(
        start(a1, "upscale", None).await,
        Err(CoreError::Unprocessable(_))
    ));
    assert!(matches!(
        start(a1, "sharpen", None).await,
        Err(CoreError::Unprocessable(_))
    ));
    assert!(matches!(
        start(a1, "denoise", Some(1.5)).await,
        Err(CoreError::Unprocessable(_))
    ));
    assert!(matches!(
        start(999_999, "denoise", None).await,
        Err(CoreError::NotFound(_))
    ));
    let d1 = s.ph["d1.jpg"].id;
    assert!(matches!(
        start(d1, "face_restore", None).await,
        Err(CoreError::Unprocessable(_))
    ));
    s.e.worker.set_missing(&[ENHANCE_MODEL]);
    assert!(matches!(
        start(p1, "denoise", None).await,
        Err(CoreError::ModelsMissing(_))
    ));
}

// ------------------------------------------------------------------ patch assets

fn asset_png(dir: &Path) -> std::path::PathBuf {
    let p = dir.join("asset.png");
    image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 200]))
        .save(&p)
        .unwrap();
    p
}

fn patch_stack(asset: &str, rect: Value, feather: f64, amount: f64) -> Value {
    json!({"version":1,"ops":[{"type":"patch","kind":"inpaint","asset":asset,"rect":rect,
        "feather":feather,"amount":amount,"enabled":true}]})
}

#[tokio::test]
async fn patch_ops_are_validated_and_sync_never_copies_them() {
    let s = scene().await;
    let (a1, a2) = (s.ph["a1.jpg"].id, s.ph["a2.jpg"].id);
    let tmp = tempfile::tempdir().unwrap();
    let png = asset_png(tmp.path());
    let (asset, dims) = s.e.core.render.patches.import(a1, "t", "", &png).unwrap();
    assert_eq!(dims, (4, 4));
    let (other, _) = s.e.core.render.patches.import(a2, "t", "", &png).unwrap();

    let ok = patch_stack(&asset, json!([0.1, 0.1, 0.5, 0.5]), 0.1, 1.0);
    s.e.core.put_edit(a1, ok.clone()).await.unwrap();
    // typed validation without a photo
    assert!(validate_stack(ok).is_ok());
    for (what, st) in [
        (
            "rect outside",
            patch_stack(&asset, json!([0.6, 0.1, 0.5, 0.5]), 0.1, 1.0),
        ),
        (
            "negative x",
            patch_stack(&asset, json!([-0.1, 0.1, 0.5, 0.5]), 0.1, 1.0),
        ),
        (
            "empty rect",
            patch_stack(&asset, json!([0.1, 0.1, 0.0, 0.5]), 0.1, 1.0),
        ),
        (
            "feather too big",
            patch_stack(&asset, json!([0.1, 0.1, 0.5, 0.5]), 0.6, 1.0),
        ),
        (
            "negative feather",
            patch_stack(&asset, json!([0.1, 0.1, 0.5, 0.5]), -0.1, 1.0),
        ),
        (
            "amount too big",
            patch_stack(&asset, json!([0.1, 0.1, 0.5, 0.5]), 0.1, 1.5),
        ),
        (
            "bad asset id",
            patch_stack("../x", json!([0.1, 0.1, 0.5, 0.5]), 0.1, 1.0),
        ),
        (
            "missing asset",
            patch_stack("nope", json!([0.1, 0.1, 0.5, 0.5]), 0.1, 1.0),
        ),
        (
            "asset of another photo",
            patch_stack(&other, json!([0.1, 0.1, 0.5, 0.5]), 0.1, 1.0),
        ),
    ] {
        match s.e.core.put_edit(a1, st).await {
            Err(CoreError::Unprocessable(_)) => {}
            other => panic!("{what}: expected 422, got {other:?}"),
        }
    }
    // preview with a given stack is checked the same way
    let r =
        s.e.core
            .render_preview(crate::edit::PreviewRequest {
                photo_id: a1,
                stack: Some(patch_stack("nope", json!([0.1, 0.1, 0.5, 0.5]), 0.1, 1.0)),
                long_edge: Some(64),
                original: None,
            })
            .await;
    assert!(matches!(r, Err(CoreError::Unprocessable(_))));
    // a disabled patch is not an edit
    let mut off = patch_stack(&asset, json!([0.1, 0.1, 0.5, 0.5]), 0.1, 1.0);
    off["ops"][0]["enabled"] = json!(false);
    s.e.core.put_edit(a1, off).await.unwrap();
    assert!(!s.e.core.photo(a1).await.unwrap().has_edits);

    // sync: a2 keeps its own patch and receives none of a1's
    s.e.core
        .put_edit(a1, json!({"version":1,"ops":[
            {"type":"patch","kind":"inpaint","asset":asset,"rect":[0.1,0.1,0.5,0.5],"feather":0.1,"amount":1,"enabled":true},
            {"type":"global","exposure":0.7}]}))
        .await
        .unwrap();
    s.e.core
        .put_edit(
            a2,
            patch_stack(&other, json!([0.2, 0.2, 0.3, 0.3]), 0.0, 1.0),
        )
        .await
        .unwrap();
    s.e.core
        .sync_edits(SyncRequest {
            from_id: a1,
            to_ids: vec![a2],
            include: vec![
                SyncKind::Crop,
                SyncKind::Global,
                SyncKind::Local,
                SyncKind::Lut,
                SyncKind::OutputSharpen,
                SyncKind::Beauty,
                SyncKind::Warp,
            ],
            adaptive: Some(false),
        })
        .await
        .unwrap();
    let st = s.e.core.get_edit(a2).await.unwrap().stack;
    assert_eq!(
        op_types(&st),
        ["patch", "global"],
        "own patch first, then the synced global"
    );
    assert_eq!(ops(&st)[0]["asset"], json!(other));
    // a2 without any patch of its own gets none either
    s.e.core.delete_edit(a2).await.unwrap();
    s.e.core
        .sync_edits(SyncRequest {
            from_id: a1,
            to_ids: vec![a2],
            include: vec![SyncKind::Global],
            adaptive: Some(false),
        })
        .await
        .unwrap();
    assert_eq!(
        op_types(&s.e.core.get_edit(a2).await.unwrap().stack),
        ["global"]
    );
    // presets never hold patches
    let preset =
        s.e.core
            .create_preset(PresetCreate {
                name: "p".into(),
                stack: s.e.core.get_edit(a1).await.unwrap().stack,
            })
            .await
            .unwrap();
    assert_eq!(op_types(&preset.stack), ["global"]);
}

#[tokio::test]
async fn assets_are_readable_and_cleaned_up_with_their_session() {
    let s = scene().await;
    let a1 = s.ph["a1.jpg"].id;
    let fa1 = faces_of(&s.e, a1).await;
    let a2 = s.ph["a2.jpg"].id;
    let fa2 = faces_of(&s.e, a2).await;
    run_besttake(&s.e, a1, vec![choice(fa1[0].id, a2, fa2[0].id)]).await;
    let asset = patches_of(&s.e, a1).await[0]["asset"]
        .as_str()
        .unwrap()
        .to_string();
    let png = s.e.core.asset_png(a1, &asset).await.unwrap();
    assert_eq!(&png[1..4], b"PNG");
    assert!(matches!(
        s.e.core.asset_png(a1, "nope").await,
        Err(CoreError::NotFound(_))
    ));
    assert!(matches!(
        s.e.core.asset_png(a1, "../x").await,
        Err(CoreError::BadRequest(_))
    ));
    assert!(matches!(
        s.e.core.asset_png(999_999, &asset).await,
        Err(CoreError::NotFound(_))
    ));
    assert!(matches!(
        s.e.core.asset_png(a2, &asset).await,
        Err(CoreError::NotFound(_))
    ));

    // render the edited thumbnails so there is something cached
    s.e.core.thumb_path(a1, 256).await.unwrap();
    let r =
        s.e.core
            .db
            .call(move |c| catalog::photo_ref(c, a1))
            .await
            .unwrap();
    let key = crate::edit::edited_key(&r.fast_key, r.edit_hash.as_deref().unwrap());
    let thumb = s.e.core.render.edited_path(&key, 256);
    assert!(thumb.is_file(), "edited thumbnail is cached");
    let dir = s.e.core.render.patches.dir(a1);
    assert!(dir.is_dir());

    s.e.core.delete_session(s.sid).await.unwrap();
    assert!(!dir.exists(), "assets are removed with the photo");
    assert!(
        !thumb.exists(),
        "edited thumbnail cache is purged with the session"
    );
    assert!(matches!(
        s.e.core.asset_png(a1, &asset).await,
        Err(CoreError::NotFound(_))
    ));
}

#[tokio::test]
async fn old_assets_are_collected_once_no_version_refers_to_them() {
    let s = scene().await;
    let a1 = s.ph["a1.jpg"].id;
    let tmp = tempfile::tempdir().unwrap();
    let png = asset_png(tmp.path());
    let patches = &s.e.core.render.patches;
    let mut assets = Vec::new();
    // more versions than are retained: the oldest ones lose their assets
    for i in 0..8 {
        let (a, _) = patches.import(a1, "t", "", &png).unwrap();
        s.e.core
            .put_edit(
                a1,
                patch_stack(&a, json!([0.1, 0.1, 0.5, 0.5]), 0.0, 1.0 - i as f64 * 0.01),
            )
            .await
            .unwrap();
        assets.push(a);
    }
    assert!(patches.exists(a1, assets.last().unwrap()));
    assert!(
        !patches.exists(a1, &assets[0]),
        "version fell out of the history"
    );
    assert!(
        patches.exists(a1, &assets[assets.len() - 2]),
        "previous version is kept for undo"
    );
}

// ------------------------------------------------------------------ singleton people

#[tokio::test]
async fn single_photo_subjects_become_singleton_people() {
    let s = scene().await;
    let (c1, p1) = (s.ph["c1.jpg"].id, s.ph["p1.jpg"].id);
    let sid = s.sid;

    // hidden from the default list, shown with include_singletons
    let shown = s.e.core.people(Some(sid)).await.unwrap();
    assert_eq!(shown.len(), 2, "{shown:?}");
    assert!(shown.iter().all(|p| !p.singleton));
    let all = s.e.core.people_with(Some(sid), true).await.unwrap();
    // the stranger and p1's main subject; p1's bystander is no subject, so no person
    assert_eq!(all.len(), 4, "{all:?}");
    assert_eq!(all.iter().filter(|p| p.singleton).count(), 2);
    assert_eq!(s.e.core.people_with(None, false).await.unwrap().len(), 2);

    // still found in the portrait panel
    let out = s.e.core.photo_people(c1).await.unwrap();
    assert_eq!(out.people.len(), 1);
    let single = out.people[0].person_id.expect("the stranger has a person");
    assert!(all.iter().any(|p| p.id == single && p.singleton));
    let fp1 = faces_of(&s.e, p1).await;
    assert!(fp1
        .iter()
        .find(|f| f.is_subject)
        .unwrap()
        .person_id
        .is_some());
    assert!(fp1
        .iter()
        .find(|f| !f.is_subject)
        .unwrap()
        .person_id
        .is_none());

    // addressable by portrait ops: validation, profile, apply-profiles and the renderer
    s.e.core
        .put_edit(
            c1,
            json!({"version":1,"ops":[
        {"type":"beauty","person_id":single,"level":"natural","smooth":20}]}),
        )
        .await
        .unwrap();
    s.e.core.delete_edit(c1).await.unwrap();
    s.e.core
        .set_beauty_profile(
            single,
            Some(json!({"beauty": {"smooth": 30.0, "level": "natural"}})),
        )
        .await
        .unwrap();
    assert_eq!(s.e.core.apply_profiles(vec![c1]).await.unwrap(), 1);
    let st = s.e.core.get_edit(c1).await.unwrap().stack;
    assert_eq!(ops(&st)[0]["person_id"], json!(single));
    s.e.core
        .render_preview(crate::edit::PreviewRequest {
            photo_id: c1,
            stack: None,
            long_edge: Some(64),
            original: None,
        })
        .await
        .unwrap();
    assert_eq!(
        s.e.renderer.people_seen.lock().unwrap().last().cloned(),
        Some(vec![Some(single)])
    );
    // best-of-people ignores singletons unless asked for them by id
    let best = s.e.core.best_of_people(Some(sid), None, 3).await.unwrap();
    assert!(best.iter().all(|b| b.person_id != single));
}

#[tokio::test]
async fn a_singleton_joins_the_real_person_when_later_photos_match() {
    let s = scene().await;
    let c1 = s.ph["c1.jpg"].id;
    let single = faces_of(&s.e, c1).await[0].person_id.unwrap();
    assert!(s
        .e
        .core
        .people_with(None, true)
        .await
        .unwrap()
        .iter()
        .any(|p| p.id == single && p.singleton));

    // a second session: the same stranger (identity 5) in two more photos
    let src2 = tempfile::tempdir().unwrap();
    for (n, t) in [("s1.jpg", 20_000), ("s2.jpg", 40_000)] {
        photo(src2.path(), n, t);
        s.e.worker.set(
            n,
            FakeSpec::at(250.0 + t as f32 / 1000.0).with_faces(vec![face(
                [0.35, 0.3, 0.25, 0.35],
                5,
                1.0,
            )]),
        );
    }
    let sid2 = import_dir(&s.e, src2.path()).await;
    analyze(&s.e, sid2).await;

    let people = s.e.core.people(None).await.unwrap();
    let p = people
        .iter()
        .find(|p| p.id == single)
        .expect("the singleton became a real person");
    assert!(!p.singleton);
    assert_eq!(p.photo_count, 3);
    let ph2 = by_name(&s.e, sid2).await;
    for n in ["s1.jpg", "s2.jpg"] {
        assert_eq!(faces_of(&s.e, ph2[n].id).await[0].person_id, Some(single));
    }
    // no duplicate person for the same identity
    let all = s.e.core.people_with(None, true).await.unwrap();
    assert_eq!(
        all.len(),
        people.len() + 1,
        "only p1's singleton is left over: {all:?}"
    );
}

// ------------------------------------------------------------------ export: upscale, EXIF, ICC

fn export_req(ids: Vec<i64>, dest: &Path, long_edge: Option<u32>) -> ExportRequest {
    ExportRequest {
        ids,
        dest: dest.to_string_lossy().into_owned(),
        folders: None,
        long_edge,
        quality: 90,
        name_template: "{name}".into(),
        apply_edits: true,
        upscale: None,
        strip_gps: false,
    }
}

async fn run_export(e: &Env, req: ExportRequest) -> (String, Option<String>) {
    let mut rx = e.core.events.subscribe();
    e.core.export(req).await.unwrap();
    let seen = wait_for(&mut rx, |ev| {
        matches!(ev, Event::TaskProgress { kind, state, .. } if kind == "export" && state != "running")
    })
    .await;
    match seen.last().unwrap() {
        Event::TaskProgress { state, error, .. } => (state.clone(), error.clone()),
        _ => unreachable!(),
    }
}

fn exposure(v: f64) -> Value {
    json!({"version":1,"ops":[{"type":"global","exposure":v}]})
}

#[tokio::test]
async fn export_upscale_renders_then_upscales_then_encodes() {
    let s = scene().await;
    let (a1, d1) = (s.ph["a1.jpg"].id, s.ph["d1.jpg"].id);
    s.e.core.put_edit(a1, exposure(0.5)).await.unwrap();
    let out = s.e.src.path().join("up");

    // 2x of the native render, for edited and unedited photos alike
    let mut req = export_req(vec![a1, d1], &out, None);
    req.upscale = Some(2);
    let (state, err) = run_export(&s.e, req).await;
    assert_eq!((state.as_str(), err), ("done", None));
    for n in ["a1.jpg", "d1.jpg"] {
        let img = image::open(out.join(n)).unwrap();
        assert_eq!((img.width(), img.height()), (640, 480), "{n}");
    }
    let reqs = s.e.worker.enhance_requests.lock().unwrap().clone();
    assert_eq!(reqs.len(), 2);
    assert!(reqs
        .iter()
        .all(|r| r.op == "upscale" && r.scale == Some(2) && r.photo.orientation == 1));
    assert!(reqs.iter().all(|r| r.photo.path.ends_with("render.png")));
    // the scratch files are gone again
    assert!(
        std::fs::read_dir(&s.e.core.dirs.gen)
            .map(|d| d.count())
            .unwrap_or(0)
            == 0
            || !s.e.core.dirs.gen.join("export-0").exists()
    );

    // with long_edge the *result* is that size: rendered at half, then doubled
    let out2 = s.e.src.path().join("up2");
    let mut req = export_req(vec![a1], &out2, Some(200));
    req.upscale = Some(2);
    run_export(&s.e, req).await;
    let img = image::open(out2.join("a1.jpg")).unwrap();
    assert_eq!(img.width().max(img.height()), 200);
    let last =
        s.e.worker
            .enhance_requests
            .lock()
            .unwrap()
            .last()
            .cloned()
            .unwrap();
    assert_eq!(last.scale, Some(2));

    // x4 and bad factors
    let out4 = s.e.src.path().join("up4");
    let mut req = export_req(vec![d1], &out4, None);
    req.upscale = Some(4);
    run_export(&s.e, req).await;
    assert_eq!(image::open(out4.join("d1.jpg")).unwrap().width(), 1280);
    let mut bad = export_req(vec![d1], &s.e.src.path().join("bad"), None);
    bad.upscale = Some(3);
    assert!(matches!(
        s.e.core.export(bad).await,
        Err(CoreError::BadRequest(_))
    ));
    // missing models -> 409 up front
    s.e.worker.set_missing(&[ENHANCE_MODEL]);
    let mut miss = export_req(vec![d1], &s.e.src.path().join("miss"), None);
    miss.upscale = Some(2);
    assert!(matches!(
        s.e.core.export(miss).await,
        Err(CoreError::ModelsMissing(_))
    ));
}

#[tokio::test]
async fn export_failure_of_the_upscaler_is_reported_per_photo() {
    let s = scene().await;
    let d1 = s.ph["d1.jpg"].id;
    s.e.worker.call_delay_ms.store(0, Ordering::SeqCst);
    // the worker answers but cannot read the image: use an unknown scale path by killing models
    // after the preflight is not possible; instead make the worker refuse by timing out
    s.e.core
        .worker_timeouts
        .set("enhance.run", Some(Duration::from_millis(100)));
    s.e.worker.call_delay_ms.store(600, Ordering::SeqCst);
    s.e.core.worker_timeouts.set("models.list", None);
    let mut req = export_req(vec![d1], &s.e.src.path().join("slow"), None);
    req.upscale = Some(2);
    let (state, err) = run_export(&s.e, req).await;
    assert_eq!(state, "failed");
    assert!(
        err.unwrap_or_default().contains("did not answer"),
        "reported as a timeout"
    );
    assert!(s.e.worker.kills.load(Ordering::SeqCst) >= 1);
}

fn jpeg_with_exif(path: &Path, orientation: u16, gps: bool) {
    write_jpeg(path, 320, 240, 90);
    let bytes = std::fs::read(path).unwrap();
    let exif = crate::jpegmeta::build_test_exif(orientation, false, gps);
    std::fs::write(
        path,
        crate::jpegmeta::insert_segments(&bytes, Some(&exif), None),
    )
    .unwrap();
}

fn read_md(path: &Path) -> ip_imaging::Metadata {
    ip_imaging::read_metadata(path, ip_imaging::ImageFormat::Jpeg).unwrap()
}

fn icc_of(path: &Path) -> Option<Vec<u8>> {
    use image::ImageDecoder;
    let f = std::io::BufReader::new(std::fs::File::open(path).unwrap());
    image::codecs::jpeg::JpegDecoder::new(f)
        .unwrap()
        .icc_profile()
        .unwrap()
}

#[tokio::test]
async fn exported_jpegs_keep_exif_and_carry_an_srgb_profile() {
    let e = env();
    jpeg_with_exif(&e.src.path().join("e1.jpg"), 6, true);
    jpeg_with_exif(&e.src.path().join("e2.jpg"), 1, true);
    photo(e.src.path(), "plain.jpg", 5);
    let sid = import_dir(&e, e.src.path()).await;
    let ph = by_name(&e, sid).await;
    let (e1, e2, plain) = (ph["e1.jpg"].id, ph["e2.jpg"].id, ph["plain.jpg"].id);
    e.core.put_edit(e1, exposure(0.5)).await.unwrap();
    e.core.put_edit(e2, exposure(0.5)).await.unwrap();
    let src_md = read_md(&e.src.path().join("e1.jpg"));
    assert_eq!(src_md.orientation, 6);
    assert!(src_md.gps_lat.is_some());

    // edited render: EXIF without orientation, capture data and GPS kept, ICC embedded
    let out = e.src.path().join("o1");
    let (state, err) = run_export(&e, export_req(vec![e1, plain], &out, None)).await;
    assert_eq!((state.as_str(), err), ("done", None));
    let md = read_md(&out.join("e1.jpg"));
    assert_eq!(md.orientation, 1, "orientation is baked in");
    assert_eq!(md.camera_make.as_deref(), Some("ACME Corp"));
    assert_eq!(md.camera_model.as_deref(), Some("Model One"));
    assert_eq!(md.taken_at_ms, src_md.taken_at_ms);
    assert!((md.gps_lat.unwrap() - src_md.gps_lat.unwrap()).abs() < 1e-6);
    assert!((md.gps_lon.unwrap() - src_md.gps_lon.unwrap()).abs() < 1e-6);
    assert_eq!((md.width, md.height), (Some(320), Some(240)));
    assert_eq!(
        icc_of(&out.join("e1.jpg")).as_deref(),
        Some(crate::jpegmeta::srgb_icc())
    );
    // the unedited photo is still a byte-exact copy (nothing re-encoded, nothing added)
    assert_eq!(
        std::fs::read(out.join("plain.jpg")).unwrap(),
        std::fs::read(e.src.path().join("plain.jpg")).unwrap()
    );

    // strip_gps drops the position but keeps the rest
    let out = e.src.path().join("o2");
    let mut req = export_req(vec![e2], &out, None);
    req.strip_gps = true;
    run_export(&e, req).await;
    let md = read_md(&out.join("e2.jpg"));
    assert!(md.gps_lat.is_none() && md.gps_lon.is_none());
    assert_eq!(md.camera_make.as_deref(), Some("ACME Corp"));
    assert!(md.taken_at_ms.is_some());
    assert!(icc_of(&out.join("e2.jpg")).is_some());

    // the resize path (no edits) keeps metadata as well
    let out = e.src.path().join("o3");
    let (state, _) = run_export(
        &e,
        export_req(vec![ph["e1.jpg"].id.min(e1)], &out, Some(100)),
    )
    .await;
    assert_eq!(state, "done");
    let md = read_md(&out.join("e1.jpg"));
    assert_eq!(md.camera_make.as_deref(), Some("ACME Corp"));
    assert_eq!(md.orientation, 1);
}

#[tokio::test]
async fn exports_of_non_jpeg_sources_get_synthesised_metadata() {
    let e = env();
    // the fake imaging reports make/model/lens/exposure for every file and no GPS
    let p = e.src.path().join("x.png");
    image::RgbImage::from_pixel(64, 48, image::Rgb([40, 40, 40]))
        .save(&p)
        .unwrap();
    let sid = import_dir(&e, e.src.path()).await;
    let id = by_name(&e, sid).await["x.png"].id;
    e.core.put_edit(id, exposure(0.3)).await.unwrap();
    let out = e.src.path().join("out");
    let (state, _) = run_export(&e, export_req(vec![id], &out, None)).await;
    assert_eq!(state, "done");
    let md = read_md(&out.join("x.jpg"));
    assert_eq!(md.camera_make.as_deref(), Some("FAKE"));
    assert_eq!(md.iso, Some(100));
    assert!(icc_of(&out.join("x.jpg")).is_some());
}

// ------------------------------------------------------------------ worker timeouts

#[tokio::test]
async fn a_stuck_worker_call_times_out_kills_the_worker_and_recovers() {
    let s = scene().await;
    let p = s.ph["a1.jpg"].id;
    s.e.core
        .worker_timeouts
        .set("mask.generate", Some(Duration::from_millis(150)));
    s.e.worker.call_delay_ms.store(1500, Ordering::SeqCst);
    let t0 = std::time::Instant::now();
    let r = s.e.core.mask_png(p, "subject", None).await;
    assert!(matches!(r, Err(CoreError::WorkerTimeout(_))), "{r:?}");
    assert!(
        t0.elapsed() < Duration::from_millis(1200),
        "the call was abandoned early"
    );
    assert_eq!(s.e.worker.kills.load(Ordering::SeqCst), 1);
    assert_eq!(
        CoreError::WorkerTimeout("x".into()).code(),
        "worker_timeout"
    );
    // a preview that needs the mask surfaces the same error
    let r =
        s.e.core
            .render_preview(crate::edit::PreviewRequest {
                photo_id: p,
                stack: Some(
                    json!({"version":1,"ops":[{"type":"local","mask":{"kind":"ai","target":"sky"},
                "adjust":{"exposure":0.5}}]}),
                ),
                long_edge: Some(64),
                original: None,
            })
            .await;
    assert!(matches!(r, Err(CoreError::WorkerTimeout(_))), "{r:?}");
    // the next request works once the worker answers again
    s.e.worker.call_delay_ms.store(0, Ordering::SeqCst);
    assert!(!s
        .e
        .core
        .mask_png(p, "subject", None)
        .await
        .unwrap()
        .is_empty());
    // limits are per method and can be lifted
    s.e.worker.call_delay_ms.store(300, Ordering::SeqCst);
    s.e.core.worker_timeouts.set("mask.generate", None);
    assert!(s.e.core.mask_png(p, "sky", None).await.is_ok());
}

#[tokio::test]
async fn a_stuck_best_take_fails_the_task_without_hammering_the_worker() {
    let s = scene().await;
    let (a1, a2, a3) = (s.ph["a1.jpg"].id, s.ph["a2.jpg"].id, s.ph["a3.jpg"].id);
    let (fa1, fa2, fa3) = (
        faces_of(&s.e, a1).await,
        faces_of(&s.e, a2).await,
        faces_of(&s.e, a3).await,
    );
    s.e.core
        .worker_timeouts
        .set("besttake.compose", Some(Duration::from_millis(100)));
    s.e.worker.call_delay_ms.store(800, Ordering::SeqCst);
    s.e.core.worker_timeouts.set("models.list", None);
    let (results, seen) = run_besttake(
        &s.e,
        a1,
        vec![
            choice(fa1[0].id, a2, fa2[0].id),
            choice(fa1[1].id, a3, fa3[1].id),
        ],
    )
    .await;
    assert!(results.iter().all(|r| !r.ok));
    assert!(results[0]
        .reason
        .as_deref()
        .unwrap()
        .contains("did not answer"));
    assert_eq!(results[0].reason, results[1].reason);
    assert_eq!(
        s.e.worker.compose_calls.load(Ordering::SeqCst),
        1,
        "no retry storm"
    );
    assert!(seen
        .iter()
        .any(|e| matches!(e, Event::TaskProgress { state, .. } if state == "failed")));
    assert!(patches_of(&s.e, a1).await.is_empty());
}

// ------------------------------------------------------------------ partial portrait geometry

#[tokio::test]
async fn partial_portrait_geometry_is_rebuilt_after_the_models_arrive() {
    let s = scene().await;
    let a1 = s.ph["a1.jpg"].id;
    let partial_rows = |s: &Scene| {
        let core = s.e.core.clone();
        async move {
            core.db
                .call(|c| {
                    Ok(c.query_row(
                        "SELECT COUNT(*) FROM beauty_geometry WHERE partial=1",
                        [],
                        |r| r.get::<_, i64>(0),
                    )?)
                })
                .await
                .unwrap()
        }
    };
    // prepared while a model is missing (the worker skips the pose)
    s.e.worker.beauty_partial.store(true, Ordering::SeqCst);
    let mut rx = s.e.core.events.subscribe();
    s.e.core.beauty_prepare(a1).await.unwrap();
    wait_for(
        &mut rx,
        |e| matches!(e, Event::BeautyReady { photo_id } if *photo_id == a1),
    )
    .await;
    assert_eq!(partial_rows(&s).await, 1);
    assert_eq!(s.e.worker.beauty_calls.load(Ordering::SeqCst), 1);
    assert!(s.e.core.photo_people(a1).await.unwrap().ready);

    // the models get installed: the partial geometry is dropped and rebuilt on demand
    s.e.worker.beauty_partial.store(false, Ordering::SeqCst);
    let mut rx = s.e.core.events.subscribe();
    s.e.core
        .models_ensure(vec![crate::fake_worker::BEAUTY_MODEL.to_string()])
        .await
        .unwrap();
    wait_for(&mut rx, |e| matches!(e, Event::TaskProgress { kind, state, .. } if kind == "model_download" && state == "done")).await;
    assert_eq!(partial_rows(&s).await, 0);
    assert!(
        !s.e.core.photo_people(a1).await.unwrap().ready,
        "needs a new prepare"
    );
    let mut rx = s.e.core.events.subscribe();
    s.e.core.beauty_prepare(a1).await.unwrap();
    wait_for(
        &mut rx,
        |e| matches!(e, Event::BeautyReady { photo_id } if *photo_id == a1),
    )
    .await;
    assert_eq!(s.e.worker.beauty_calls.load(Ordering::SeqCst), 2);
    assert_eq!(partial_rows(&s).await, 0);

    // complete geometry is never invalidated
    s.e.core
        .models_ensure(vec![crate::fake_worker::BEAUTY_MODEL.to_string()])
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(s.e.core.photo_people(a1).await.unwrap().ready);
}

#[tokio::test]
async fn optional_sdxl_pack_never_blocks_lama_inpainting() {
    let e = env();
    // The 6.7 GB SDXL "pro" pack is not installed; LaMa is.
    e.worker.set_missing(&[SDXL_MODEL]);
    e.core.preflight_models("inpaint", &["lama"]).await.unwrap();
    match e.core.preflight_models("inpaint", &["sdxl"]).await {
        Err(CoreError::ModelsMissing(m)) => assert_eq!(m, vec![SDXL_MODEL.to_string()]),
        other => panic!("expected models_missing for sdxl, got {other:?}"),
    }
    // Features without an exact list fall back to tags, still ignoring optional models.
    e.worker.set_missing(&[SDXL_MODEL, ENHANCE_MODEL]);
    match e.core.preflight_models("enhance", &["denoise"]).await {
        Err(CoreError::ModelsMissing(m)) => assert_eq!(m, vec![ENHANCE_MODEL.to_string()]),
        other => panic!("expected models_missing for enhance, got {other:?}"),
    }
}
