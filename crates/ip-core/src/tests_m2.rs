//! End-to-end tests of the M2 analysis pipeline with `FakeImaging` + `FakeWorker`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast::Receiver;

use crate::analysis::scoring::Issue;
use crate::analysis::*;
use crate::fake_worker::{person, FakeFace, FakeSpec, FakeWorker};
use crate::testutil::{write_jpeg, FakeImaging};
use crate::*;

const BASE: u64 = 1_700_000_000;

struct Env {
    core: Arc<Core>,
    worker: Arc<FakeWorker>,
    _data: tempfile::TempDir,
    src: tempfile::TempDir,
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
        renderer: None,
        force_cpu: false,
    })
    .unwrap();
    Env {
        core,
        worker,
        _data: data,
        src,
    }
}

/// Creates `name` with capture time BASE + `t_s` seconds (FakeImaging uses the mtime).
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
    e.core
        .wait_session_ready(s.id, Duration::from_secs(30))
        .await
        .unwrap();
    s.id
}

fn run_req(sid: i64, profile: Profile) -> AnalysisRunRequest {
    AnalysisRunRequest {
        session_id: sid,
        profile,
        photo_ids: None,
        force: false,
        allow_download: false,
    }
}

async fn wait_done(e: &Env, sid: i64) -> AnalysisStatus {
    for _ in 0..600 {
        let s = e.core.analysis_status(sid);
        if s.state != RunState::Running {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("analysis did not finish");
}

async fn run(e: &Env, sid: i64, profile: Profile) -> AnalysisStatus {
    e.core.analysis_run(run_req(sid, profile)).await.unwrap();
    wait_done(e, sid).await
}

async fn all(e: &Env, sid: i64) -> HashMap<String, Photo> {
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

fn names(page: &PhotosPage) -> Vec<String> {
    let mut v: Vec<String> = page.photos.iter().map(|p| p.file_name.clone()).collect();
    v.sort();
    v
}

fn drain(rx: &mut Receiver<Event>) -> Vec<Event> {
    let mut v = Vec::new();
    while let Ok(e) = rx.try_recv() {
        v.push(e);
    }
    v
}

fn face_a() -> [f64; 4] {
    [0.2, 0.3, 0.2, 0.3]
}
fn face_b() -> [f64; 4] {
    [0.6, 0.3, 0.2, 0.3]
}

/// Scene used by several tests:
///  burst 1: a1 a2 a3 (t=0,1,2) similar frames, sharpness .5 / .95 / .7
///  burst 2: b1 b2 (t=1h) with the same two people, b2 has person B with closed eyes
///  burst 3: c1 (t=2h) alone, a stranger
fn build_scene(e: &Env) {
    let d = e.src.path();
    for (n, t) in [("a1.jpg", 0), ("a2.jpg", 1), ("a3.jpg", 2)] {
        photo(d, n, t);
    }
    for (n, t) in [("b1.jpg", 3600), ("b2.jpg", 3601)] {
        photo(d, n, t);
    }
    photo(d, "c1.jpg", 7200);
    let faces = |ja: f32, jb: f32, eyes_b: f64| {
        vec![
            FakeFace::new(face_a(), person(0, ja)),
            FakeFace::new(face_b(), person(1, jb)).eyes(eyes_b),
        ]
    };
    e.worker.set(
        "a1.jpg",
        FakeSpec::at(0.0)
            .sharp(0.5)
            .with_faces(faces(40.0, 1.0, 0.9)),
    );
    e.worker.set(
        "a2.jpg",
        FakeSpec::at(2.0)
            .sharp(0.95)
            .with_faces(faces(2.0, 0.0, 0.9)),
    );
    e.worker.set(
        "a3.jpg",
        FakeSpec::at(3.0)
            .sharp(0.7)
            .with_faces(faces(0.0, 2.0, 0.9)),
    );
    e.worker.set(
        "b1.jpg",
        FakeSpec::at(90.0)
            .sharp(0.8)
            .with_faces(faces(3.0, 1.0, 0.9)),
    );
    e.worker.set(
        "b2.jpg",
        FakeSpec::at(91.0)
            .sharp(0.8)
            .with_faces(faces(1.0, 2.0, 0.1)),
    );
    e.worker.set(
        "c1.jpg",
        FakeSpec::at(200.0)
            .scene("landscape")
            .with_faces(vec![FakeFace::new([0.4, 0.4, 0.2, 0.3], person(5, 0.0))]),
    );
}

#[tokio::test]
async fn full_pipeline_groups_scores_people() {
    let e = env();
    build_scene(&e);
    let sid = import(&e).await;
    let mut rx = e.core.events.subscribe();

    let st = run(&e, sid, Profile::Standard).await;
    assert_eq!(st.state, RunState::Done, "{st:?}");
    assert_eq!((st.done, st.total), (6, 6));
    assert!(st.error.is_none());
    assert_eq!(
        e.worker.last_profile.lock().unwrap().as_deref(),
        Some("standard")
    );
    assert_eq!(*e.worker.last_allow_download.lock().unwrap(), Some(false));

    let ph = all(&e, sid).await;
    assert!(ph.values().all(|p| p.analyzed));
    // ---- bursts
    let a: Vec<&Photo> = ["a1.jpg", "a2.jpg", "a3.jpg"]
        .iter()
        .map(|n| &ph[*n])
        .collect();
    assert!(a
        .iter()
        .all(|p| p.burst_id == a[0].burst_id && p.burst_size == Some(3)));
    assert_eq!(ph["a2.jpg"].rank_in_burst, Some(0), "sharpest frame wins");
    assert_eq!(ph["a3.jpg"].rank_in_burst, Some(1));
    assert_eq!(ph["a1.jpg"].rank_in_burst, Some(2));
    assert_ne!(ph["b1.jpg"].burst_id, a[0].burst_id);
    assert_eq!(ph["b1.jpg"].burst_id, ph["b2.jpg"].burst_id);
    assert_eq!(ph["c1.jpg"].burst_size, Some(1));
    assert_eq!(ph["c1.jpg"].scene_type.as_deref(), Some("landscape"));
    // ---- ratings
    let r = |n: &str| ph[n].ai_rating.unwrap();
    assert!(r("a2.jpg") >= r("a3.jpg") && r("a3.jpg") >= r("a1.jpg"));
    assert!(ph["a2.jpg"].ai_score.unwrap() > ph["a1.jpg"].ai_score.unwrap());
    assert_eq!((r("a2.jpg") * 2.0).fract(), 0.0, "0.5 steps");
    // closed eyes: b2 (person B) is flagged and capped at 2 stars; b1 is the better one
    assert_eq!(ph["b2.jpg"].issues, vec![Issue::ClosedEyes]);
    assert!(r("b2.jpg") <= 2.0);
    assert!(ph["b1.jpg"].issues.is_empty());
    assert_eq!(ph["b1.jpg"].rank_in_burst, Some(0));
    // face counts
    assert_eq!(
        (ph["a1.jpg"].face_count, ph["a1.jpg"].subject_face_count),
        (Some(2), Some(2))
    );

    // ---- groups API: time ordered scenes -> bursts, photos best first
    let g = e.core.groups(sid).await.unwrap();
    assert_eq!(g.scenes.len(), 3);
    let first = &g.scenes[0].bursts[0];
    assert_eq!(first.size, 3);
    assert_eq!(first.best_photo_id, Some(ph["a2.jpg"].id));
    assert_eq!(first.photo_ids[0], ph["a2.jpg"].id);
    assert_eq!(first.photo_ids.len(), 3);
    assert!(g.scenes[0].start_at <= g.scenes[1].start_at);
    assert_eq!(g.scenes[0].start_at, (BASE as i64) * 1000);
    assert_eq!(g.scenes[0].end_at, (BASE as i64 + 2) * 1000);

    // ---- analysis detail
    let d = e.core.photo_analysis(ph["b2.jpg"].id).await.unwrap();
    assert!(d.analyzed);
    assert_eq!(d.profile.as_deref(), Some("standard"));
    assert_eq!(d.faces.len(), 2);
    let sc = d.scores.unwrap();
    assert!(sc.sharpness.is_some() && sc.iqa.is_some() && sc.aesthetic.is_some());
    assert!(sc.face.is_some());
    assert!(sc.composition.is_none());
    assert!(d
        .contributions
        .iter()
        .any(|c| c.key == "closed_eyes" && c.delta < 0.0));
    assert!(d.contributions.iter().all(|c| !c.label_key.is_empty()));
    assert!(d.reasons.iter().any(|r| r.key == "closed_eyes"));
    let un = e.core.photo_analysis(ph["a1.jpg"].id).await.unwrap();
    assert!(un.analyzed);

    // ---- people: A and B across both bursts; the stranger (one photo) is not a person
    let people = e.core.people(Some(sid)).await.unwrap();
    assert_eq!(people.len(), 2, "{people:?}");
    assert!(people
        .iter()
        .all(|p| p.photo_count == 5 && p.name.is_none() && !p.hidden));
    assert!(people.iter().all(|p| p.cover_face_id.is_some()));
    let stranger = &d.faces;
    let _ = stranger;
    // ... but as a single-photo subject it still has a (singleton) person of its own
    let c1 = e.core.photo_analysis(ph["c1.jpg"].id).await.unwrap();
    let single = c1.faces[0].person_id.expect("singleton person");
    assert!(!people.iter().any(|p| p.id == single));
    let all = e.core.people_with(Some(sid), true).await.unwrap();
    assert_eq!(all.len(), 3);
    let sp = all.iter().find(|p| p.id == single).unwrap();
    assert!(sp.singleton && sp.photo_count == 1);
    // tracks within the burst: both people, one cell per photo
    let bf = e
        .core
        .burst_faces(ph["a1.jpg"].burst_id.unwrap())
        .await
        .unwrap();
    assert_eq!(bf.photo_ids.len(), 3);
    assert_eq!(bf.tracks.len(), 2);
    for t in &bf.tracks {
        assert_eq!(t.cells.len(), 3);
        assert!(t.cells.values().all(|c| c.is_some()));
        assert_eq!(t.best_photo_ids.len(), 3);
        assert!(t.person_id.is_some());
    }

    // ---- events
    let evs = drain(&mut rx);
    assert!(evs.iter().any(|e| matches!(e, Event::AnalysisProgress { state: RunState::Running, stage: Some(s), .. } if s == "analyzing")));
    assert!(evs.iter().any(|e| matches!(
        e,
        Event::AnalysisProgress {
            state: RunState::Done,
            stage: None,
            ..
        }
    )));
    assert!(evs
        .iter()
        .any(|e| matches!(e, Event::GroupsUpdated { session_id } if *session_id == sid)));
    assert!(evs
        .iter()
        .any(|e| matches!(e, Event::PeopleUpdated { session_id } if *session_id == sid)));
    let updated: usize = evs
        .iter()
        .filter_map(|e| match e {
            Event::AnalysisUpdated { ids, .. } => Some(ids.len()),
            _ => None,
        })
        .sum();
    assert!(updated >= 6);
    assert!(evs.iter().any(|e| matches!(e, Event::TaskProgress { kind, state, .. } if kind == "analysis" && state == "done")));
}

#[tokio::test]
async fn filters_by_people_state_counts_and_ai() {
    let e = env();
    build_scene(&e);
    let sid = import(&e).await;
    run(&e, sid, Profile::Standard).await;
    let ph = all(&e, sid).await;
    let people = e.core.people(Some(sid)).await.unwrap();
    // identify A and B through their faces in a1 (A is the left face)
    let d = e.core.photo_analysis(ph["a1.jpg"].id).await.unwrap();
    let pa = d
        .faces
        .iter()
        .find(|f| f.bbox[0] < 0.4)
        .unwrap()
        .person_id
        .unwrap();
    let pb = d
        .faces
        .iter()
        .find(|f| f.bbox[0] > 0.4)
        .unwrap()
        .person_id
        .unwrap();
    assert_ne!(pa, pb);
    assert_eq!(people.len(), 2);

    let q = |f: &dyn Fn(&mut PhotoQuery)| {
        let mut q = PhotoQuery {
            session_id: sid,
            ..Default::default()
        };
        f(&mut q);
        q
    };
    let page = |q: PhotoQuery| {
        let core = e.core.clone();
        async move { core.photos(q).await.unwrap() }
    };

    // AND (default): both in the same photo
    let both = page(q(&|q| q.persons = vec![pa, pb])).await;
    assert_eq!(both.total, 5);
    // OR
    let any = page(q(&|q| {
        q.persons = vec![pa, pb];
        q.person_mode = PersonMode::Any;
    }))
    .await;
    assert_eq!(any.total, 5);
    // NOT: exclude B -> only the stranger photo remains
    let not_b = page(q(&|q| q.exclude_persons = vec![pb])).await;
    assert_eq!(names(&not_b), vec!["c1.jpg"]);
    // person_state: A with eyes open; B with eyes open excludes b2
    let a_open = page(q(&|q| {
        q.persons = vec![pa];
        q.person_state = vec![PersonState::EyesOpen];
    }))
    .await;
    assert_eq!(a_open.total, 5);
    let b_open = page(q(&|q| {
        q.persons = vec![pb];
        q.person_state = vec![PersonState::EyesOpen];
    }))
    .await;
    assert_eq!(names(&b_open), vec!["a1.jpg", "a2.jpg", "a3.jpg", "b1.jpg"]);
    // AND with states: both people, both with eyes open -> b2 drops out
    let both_open = page(q(&|q| {
        q.persons = vec![pa, pb];
        q.person_state = vec![PersonState::EyesOpen];
    }))
    .await;
    assert_eq!(both_open.total, 4);
    // smiling (smile 0.7) and looking (gaze 0.9) hold for everyone
    let smiling = page(q(&|q| {
        q.persons = vec![pa];
        q.person_state = vec![
            PersonState::Smiling,
            PersonState::Looking,
            PersonState::Subject,
        ];
    }))
    .await;
    assert_eq!(smiling.total, 5);
    // faces_min / faces_max count subjects
    assert_eq!(page(q(&|q| q.faces_min = Some(2))).await.total, 5);
    assert_eq!(page(q(&|q| q.faces_max = Some(1))).await.total, 1);
    assert_eq!(page(q(&|q| q.faces_max = Some(0))).await.total, 0);
    assert_eq!(
        page(q(&|q| {
            q.faces_min = Some(1);
            q.faces_max = Some(1);
        }))
        .await
        .total,
        1
    );
    // issues
    let none = page(q(&|q| q.issues_none = true)).await;
    assert_eq!(none.total, 5);
    let closed = page(q(&|q| {
        q.issues_any = vec![Issue::ClosedEyes, Issue::Blurry]
    }))
    .await;
    assert_eq!(names(&closed), vec!["b2.jpg"]);
    // burst filters
    let best = page(q(&|q| q.burst_best_only = true)).await;
    assert_eq!(best.total, 3); // one per burst
    assert!(best.photos.iter().all(|p| p.rank_in_burst == Some(0)));
    let in_burst = page(q(&|q| q.burst_id = ph["b1.jpg"].burst_id)).await;
    assert_eq!(names(&in_burst), vec!["b1.jpg", "b2.jpg"]);
    // scene / ai rating / sort
    assert_eq!(
        names(&page(q(&|q| q.scene_type = Some("landscape".into()))).await),
        vec!["c1.jpg"]
    );
    let ai = page(q(&|q| q.ai_rating_gte = Some(4.0))).await;
    assert!(ai.photos.iter().all(|p| p.ai_rating.unwrap() >= 4.0));
    let sorted = page(q(&|q| q.sort = SortKey::Ai)).await;
    let scores: Vec<f64> = sorted.photos.iter().map(|p| p.ai_score.unwrap()).collect();
    assert!(scores.windows(2).all(|w| w[0] >= w[1]));
}

#[tokio::test]
async fn sort_by_ai_puts_unanalysed_last_and_paginates() {
    let e = env();
    build_scene(&e);
    let sid = import(&e).await;
    let ph = all(&e, sid).await;
    // analyse only some photos
    let ids = vec![ph["a1.jpg"].id, ph["a2.jpg"].id, ph["b1.jpg"].id];
    e.core
        .analysis_run(AnalysisRunRequest {
            photo_ids: Some(ids.clone()),
            ..run_req(sid, Profile::Standard)
        })
        .await
        .unwrap();
    assert_eq!(wait_done(&e, sid).await.total, 3);
    let mut cursor = None;
    let mut seen = Vec::new();
    loop {
        let p = e
            .core
            .photos(PhotoQuery {
                session_id: sid,
                sort: SortKey::Ai,
                limit: Some(2),
                cursor: cursor.take(),
                ..Default::default()
            })
            .await
            .unwrap();
        seen.extend(p.photos);
        match p.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(seen.len(), 6);
    assert!(seen[..3].iter().all(|p| p.analyzed));
    assert!(seen[3..]
        .iter()
        .all(|p| !p.analyzed && p.ai_score.is_none()));
    let scores: Vec<f64> = seen[..3].iter().map(|p| p.ai_score.unwrap()).collect();
    assert!(scores.windows(2).all(|w| w[0] >= w[1]));
}

#[tokio::test]
async fn people_naming_merging_and_corrections() {
    let e = env();
    build_scene(&e);
    let sid = import(&e).await;
    run(&e, sid, Profile::Standard).await;
    let ph = all(&e, sid).await;
    let d = e.core.photo_analysis(ph["b2.jpg"].id).await.unwrap();
    let pb = d
        .faces
        .iter()
        .find(|f| f.bbox[0] > 0.4)
        .unwrap()
        .person_id
        .unwrap();
    let pa = d
        .faces
        .iter()
        .find(|f| f.bbox[0] < 0.4)
        .unwrap()
        .person_id
        .unwrap();
    let mut rx = e.core.events.subscribe();

    // naming shows up in faces, tracks and the closed-eyes reason
    let p = e
        .core
        .patch_person(
            pb,
            PersonPatch {
                name: Some(Some("小红".into())),
                hidden: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(p.name.as_deref(), Some("小红"));
    let d = e.core.photo_analysis(ph["b2.jpg"].id).await.unwrap();
    assert!(d
        .faces
        .iter()
        .any(|f| f.person_name.as_deref() == Some("小红")));
    let r = d.reasons.iter().find(|r| r.key == "closed_eyes").unwrap();
    assert_eq!(r.params["person"], "小红");
    let bf = e
        .core
        .burst_faces(ph["b1.jpg"].burst_id.unwrap())
        .await
        .unwrap();
    assert!(bf
        .tracks
        .iter()
        .any(|t| t.person_name.as_deref() == Some("小红")));
    assert!(drain(&mut rx)
        .iter()
        .any(|e| matches!(e, Event::PeopleUpdated { session_id } if *session_id == sid)));
    // empty names clear; hidden is independent
    let p = e
        .core
        .patch_person(
            pb,
            PersonPatch {
                name: Some(None),
                hidden: Some(true),
            },
        )
        .await
        .unwrap();
    assert!(p.name.is_none() && p.hidden);
    assert!(matches!(
        e.core
            .patch_person(
                pb,
                PersonPatch {
                    name: None,
                    hidden: None
                }
            )
            .await,
        Err(CoreError::BadRequest(_))
    ));
    assert!(matches!(
        e.core
            .patch_person(
                9999,
                PersonPatch {
                    name: Some(None),
                    hidden: None
                }
            )
            .await,
        Err(CoreError::NotFound(_))
    ));

    // "not this person": the face gets a fresh single-face person and stays there
    let face_b1 = d.faces.iter().find(|f| f.person_id == Some(pb)).unwrap().id;
    let f = e.core.set_face_person(face_b1, None).await.unwrap();
    assert!(f.person_id.is_some() && f.person_id != Some(pb));
    let people = e.core.people(Some(sid)).await.unwrap();
    assert_eq!(people.len(), 3);
    // assigning it back to B
    let f = e.core.set_face_person(face_b1, Some(pb)).await.unwrap();
    assert_eq!(f.person_id, Some(pb));
    assert_eq!(
        e.core.people(Some(sid)).await.unwrap().len(),
        2,
        "emptied person is removed"
    );
    // a second face of the same photo cannot join the same person
    let other = d.faces.iter().find(|f| f.person_id == Some(pa)).unwrap().id;
    assert!(matches!(
        e.core.set_face_person(other, Some(pb)).await,
        Err(CoreError::BadRequest(_))
    ));

    // merge B into A: one person, all faces
    e.core
        .patch_person(
            pa,
            PersonPatch {
                name: Some(Some("小明".into())),
                hidden: None,
            },
        )
        .await
        .unwrap();
    // merging two people that appear together is allowed by the user (constraint only for faces)
    let m = e
        .core
        .merge_people(MergePeopleRequest {
            ids: vec![pb],
            into: pa,
        })
        .await
        .unwrap();
    assert_eq!(m.id, pa);
    assert_eq!(m.name.as_deref(), Some("小明"));
    assert!(matches!(
        e.core
            .merge_people(MergePeopleRequest {
                ids: vec![pa],
                into: pa
            })
            .await,
        Err(CoreError::BadRequest(_))
    ));
    assert_eq!(e.core.people(Some(sid)).await.unwrap().len(), 1);
    assert!(matches!(
        e.core.people(Some(9999)).await,
        Err(CoreError::NotFound(_))
    ));
    // whole-library listing
    assert_eq!(e.core.people(None).await.unwrap().len(), 1);
}

#[tokio::test]
async fn user_constraints_survive_reanalysis() {
    let e = env();
    build_scene(&e);
    let sid = import(&e).await;
    run(&e, sid, Profile::Standard).await;
    let ph = all(&e, sid).await;
    let d = e.core.photo_analysis(ph["a1.jpg"].id).await.unwrap();
    let pa = d
        .faces
        .iter()
        .find(|f| f.bbox[0] < 0.4)
        .unwrap()
        .person_id
        .unwrap();
    e.core
        .patch_person(
            pa,
            PersonPatch {
                name: Some(Some("Ann".into())),
                hidden: None,
            },
        )
        .await
        .unwrap();
    // the user declares a1's left face is somebody else
    let face = d.faces.iter().find(|f| f.person_id == Some(pa)).unwrap().id;
    let lone = e
        .core
        .set_face_person(face, None)
        .await
        .unwrap()
        .person_id
        .unwrap();

    // re-analyse everything
    e.core
        .analysis_run(AnalysisRunRequest {
            force: true,
            ..run_req(sid, Profile::Standard)
        })
        .await
        .unwrap();
    assert_eq!(wait_done(&e, sid).await.state, RunState::Done);
    let d = e.core.photo_analysis(ph["a1.jpg"].id).await.unwrap();
    let left = d.faces.iter().find(|f| f.bbox[0] < 0.4).unwrap();
    assert_eq!(
        left.person_id,
        Some(lone),
        "locked face keeps its person (new face id, same bbox)"
    );
    // the named person survived the re-run
    let people = e.core.people(Some(sid)).await.unwrap();
    let ann = people
        .iter()
        .find(|p| p.name.as_deref() == Some("Ann"))
        .unwrap();
    assert_eq!(ann.id, pa);
    assert!(ann.photo_count >= 1);

    // must-link: the user says the stranger of c1 is Ann, although the embedding is far away
    let c1 = e.core.photo_analysis(ph["c1.jpg"].id).await.unwrap();
    assert!(c1.faces[0].person_id.is_some_and(|p| p != pa)); // its own singleton person
    e.core
        .set_face_person(c1.faces[0].id, Some(pa))
        .await
        .unwrap();
    let before = e
        .core
        .people(Some(sid))
        .await
        .unwrap()
        .iter()
        .find(|p| p.id == pa)
        .unwrap()
        .photo_count;
    e.core
        .analysis_run(AnalysisRunRequest {
            force: true,
            ..run_req(sid, Profile::Standard)
        })
        .await
        .unwrap();
    wait_done(&e, sid).await;
    let c1 = e.core.photo_analysis(ph["c1.jpg"].id).await.unwrap();
    assert_eq!(
        c1.faces[0].person_id,
        Some(pa),
        "user assignment survives re-analysis"
    );
    let after = e
        .core
        .people(Some(sid))
        .await
        .unwrap()
        .iter()
        .find(|p| p.id == pa)
        .unwrap()
        .photo_count;
    assert_eq!(after, before);
}

#[tokio::test]
async fn manual_split_and_merge_rerank_and_persist() {
    let e = env();
    build_scene(&e);
    let sid = import(&e).await;
    run(&e, sid, Profile::Standard).await;
    let ph = all(&e, sid).await;
    let burst = ph["a1.jpg"].burst_id.unwrap();
    let mut rx = e.core.events.subscribe();

    let [a, b] = e
        .core
        .split_burst(SplitRequest {
            burst_id: burst,
            at_photo_id: ph["a3.jpg"].id,
        })
        .await
        .unwrap();
    assert_eq!(a, burst);
    assert_ne!(a, b);
    let now = all(&e, sid).await;
    assert_eq!(now["a1.jpg"].burst_id, Some(a));
    assert_eq!(now["a2.jpg"].burst_id, Some(a));
    assert_eq!(now["a3.jpg"].burst_id, Some(b));
    assert_eq!(now["a3.jpg"].burst_size, Some(1));
    assert_eq!(now["a2.jpg"].rank_in_burst, Some(0));
    assert_eq!(now["a1.jpg"].rank_in_burst, Some(1));
    assert_eq!(now["a3.jpg"].rank_in_burst, Some(0));
    let evs = drain(&mut rx);
    assert!(evs.iter().any(|e| matches!(e, Event::GroupsUpdated { .. })));
    assert!(evs.iter().any(
        |e| matches!(e, Event::AnalysisUpdated { ids, .. } if ids.contains(&ph["a3.jpg"].id))
    ));
    // groups API shows two bursts in the same scene
    let g = e.core.groups(sid).await.unwrap();
    assert_eq!(g.scenes[0].bursts.len(), 2);
    assert_eq!(g.scenes[0].bursts[1].id, b);
    // expression matrix recomputed for the new burst
    let bf = e.core.burst_faces(b).await.unwrap();
    assert_eq!(bf.photo_ids, vec![ph["a3.jpg"].id]);
    assert_eq!(bf.tracks.len(), 2);

    // bad splits
    assert!(matches!(
        e.core
            .split_burst(SplitRequest {
                burst_id: a,
                at_photo_id: ph["a1.jpg"].id
            })
            .await,
        Err(CoreError::BadRequest(_))
    ));
    assert!(matches!(
        e.core
            .split_burst(SplitRequest {
                burst_id: a,
                at_photo_id: ph["c1.jpg"].id
            })
            .await,
        Err(CoreError::BadRequest(_))
    ));
    assert!(matches!(
        e.core
            .split_burst(SplitRequest {
                burst_id: 99999,
                at_photo_id: 1
            })
            .await,
        Err(CoreError::NotFound(_))
    ));

    // manual bursts survive a re-run (auto grouping would have glued them together again)
    e.core
        .analysis_run(AnalysisRunRequest {
            force: true,
            ..run_req(sid, Profile::Standard)
        })
        .await
        .unwrap();
    wait_done(&e, sid).await;
    let now = all(&e, sid).await;
    assert_ne!(now["a3.jpg"].burst_id, now["a1.jpg"].burst_id);
    assert_eq!(now["a1.jpg"].burst_id, now["a2.jpg"].burst_id);

    // merge the two halves and the burst of another scene
    let merged = e
        .core
        .merge_bursts(MergeBurstsRequest {
            burst_ids: vec![
                now["a1.jpg"].burst_id.unwrap(),
                now["a3.jpg"].burst_id.unwrap(),
            ],
        })
        .await
        .unwrap();
    let now = all(&e, sid).await;
    assert!(["a1.jpg", "a2.jpg", "a3.jpg"]
        .iter()
        .all(|n| now[*n].burst_id == Some(merged)));
    assert_eq!(now["a2.jpg"].rank_in_burst, Some(0));
    assert_eq!(now["a2.jpg"].burst_size, Some(3));
    let merged2 = e
        .core
        .merge_bursts(MergeBurstsRequest {
            burst_ids: vec![merged, now["c1.jpg"].burst_id.unwrap()],
        })
        .await
        .unwrap();
    let g = e.core.groups(sid).await.unwrap();
    let b = g
        .scenes
        .iter()
        .flat_map(|s| &s.bursts)
        .find(|b| b.id == merged2)
        .unwrap();
    assert_eq!(b.size, 4);
    assert!(
        g.scenes
            .iter()
            .flat_map(|s| &s.bursts)
            .map(|b| b.size)
            .sum::<i64>()
            == 6
    );
    assert!(matches!(
        e.core
            .merge_bursts(MergeBurstsRequest {
                burst_ids: vec![merged2]
            })
            .await,
        Err(CoreError::BadRequest(_))
    ));
}

#[tokio::test]
async fn models_missing_blocks_standard_but_not_fast() {
    let e = env();
    build_scene(&e);
    let sid = import(&e).await;
    e.worker.set_missing(&["siglip2-base"]);
    match e.core.analysis_run(run_req(sid, Profile::Standard)).await {
        Err(CoreError::ModelsMissing(m)) => assert_eq!(m, vec!["siglip2-base".to_string()]),
        other => panic!("expected models_missing, got {other:?}"),
    }
    assert_eq!(e.core.analysis_status(sid).state, RunState::Idle);
    assert_eq!(e.worker.analyze_calls.load(Ordering::SeqCst), 0);
    // fast does not need the embedding model: grouping falls back to the perceptual hash
    let st = run(&e, sid, Profile::Fast).await;
    assert_eq!(st.state, RunState::Done);
    assert_eq!(
        e.worker.last_profile.lock().unwrap().as_deref(),
        Some("fast")
    );
    let ph = all(&e, sid).await;
    assert!(ph.values().all(|p| p.analyzed && p.ai_rating.is_some()));
    // the scene is the same for every photo in fast mode, so everything within 10 minutes and
    // identical hashes groups together; faces still work without identity
    let d = e.core.photo_analysis(ph["a1.jpg"].id).await.unwrap();
    assert_eq!(d.profile.as_deref(), Some("fast"));
    assert!(d.scores.unwrap().iqa.is_none());
    assert!(
        e.core.people(Some(sid)).await.unwrap().is_empty(),
        "no identity, no people"
    );
    let bf = e
        .core
        .burst_faces(ph["a1.jpg"].burst_id.unwrap())
        .await
        .unwrap();
    assert_eq!(bf.tracks.len(), 2, "tracks come from box overlap");
    // models endpoints
    let models = e.core.models().await.unwrap();
    let sig = models.iter().find(|m| m.id == "siglip2-base").unwrap();
    assert!(!sig.installed);
    assert_eq!(sig.required_for, vec!["standard".to_string()]);
    let yunet = models.iter().find(|m| m.id == "yunet").unwrap();
    assert_eq!(
        yunet.required_for,
        vec!["fast".to_string(), "standard".to_string()]
    );
    let mut rx = e.core.events.subscribe();
    let tid = e
        .core
        .models_ensure(vec!["siglip2-base".into()])
        .await
        .unwrap();
    assert!(tid.starts_with("models-"));
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        if e.core.models().await.unwrap().iter().all(|m| m.installed) {
            break;
        }
    }
    let evs = drain(&mut rx);
    assert!(evs.iter().any(
        |ev| matches!(ev, Event::TaskProgress { kind, state, done, total, .. }
        if kind == "model_download" && state == "done" && done == total && *total > 0)
    ));
    assert!(matches!(
        e.core.models_ensure(vec!["nope".into()]).await,
        Err(CoreError::BadRequest(_))
    ));
    assert!(matches!(
        e.core.models_ensure(vec![]).await,
        Err(CoreError::BadRequest(_))
    ));
    // now standard works
    assert_eq!(run(&e, sid, Profile::Standard).await.state, RunState::Done);
}

#[tokio::test]
async fn skipped_steps_degrade_gracefully() {
    let e = env();
    build_scene(&e);
    let sid = import(&e).await;
    e.worker.set_skipped(&["aesthetic", "scene"]);
    let st = run(&e, sid, Profile::Standard).await;
    assert_eq!(st.state, RunState::Done);
    assert_eq!(
        st.skipped_steps,
        vec!["aesthetic".to_string(), "scene".to_string()]
    );
}

#[tokio::test]
async fn batching_cancel_and_status() {
    let e = env();
    let d = e.src.path();
    for i in 0..70u64 {
        photo(d, &format!("p{i:03}.jpg"), i * 100);
    }
    let sid = import(&e).await;
    e.core
        .analysis_run(run_req(sid, Profile::Fast))
        .await
        .unwrap();
    // a second run while one is active is rejected
    assert!(matches!(
        e.core.analysis_run(run_req(sid, Profile::Fast)).await,
        Err(CoreError::Conflict(_))
    ));
    let st = wait_done(&e, sid).await;
    assert_eq!(st.state, RunState::Done);
    assert_eq!(*e.worker.batch_sizes.lock().unwrap(), vec![32, 32, 6]);
    // nothing left to do: a second run only regroups (no worker call)
    let calls = e.worker.analyze_calls.load(Ordering::SeqCst);
    let st = run(&e, sid, Profile::Fast).await;
    assert_eq!((st.state, st.total), (RunState::Done, 0));
    assert_eq!(e.worker.analyze_calls.load(Ordering::SeqCst), calls);

    // cancellation mid-run: finished batches are kept and grouped, state returns to idle
    let e2 = env();
    for i in 0..70u64 {
        photo(e2.src.path(), &format!("q{i:03}.jpg"), i * 100);
    }
    let sid2 = import(&e2).await;
    e2.worker.delay_ms.store(300, Ordering::SeqCst);
    e2.core
        .analysis_run(run_req(sid2, Profile::Fast))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(450)).await; // first batch done, second running
    e2.core.analysis_cancel(sid2);
    let st = wait_done(&e2, sid2).await;
    assert_eq!(st.state, RunState::Idle);
    let analysed = all(&e2, sid2).await.values().filter(|p| p.analyzed).count();
    assert!((32..70).contains(&analysed), "{analysed}");
    assert!(all(&e2, sid2)
        .await
        .values()
        .filter(|p| p.analyzed)
        .all(|p| p.ai_rating.is_some()));
    // and it can be resumed: only the rest is analysed
    e2.worker.delay_ms.store(0, Ordering::SeqCst);
    let st = run(&e2, sid2, Profile::Fast).await;
    assert_eq!(st.total as usize, 70 - analysed);
    assert!(all(&e2, sid2).await.values().all(|p| p.analyzed));
    e2.core.analysis_cancel(sid2); // no-op when idle
    assert!(matches!(
        e.core.analysis_run(run_req(9999, Profile::Fast)).await,
        Err(CoreError::NotFound(_))
    ));
}

#[tokio::test]
async fn worker_crash_is_retried_then_reported() {
    let e = env();
    build_scene(&e);
    let sid = import(&e).await;
    e.worker.crash_batches.store(1, Ordering::SeqCst);
    let st = run(&e, sid, Profile::Standard).await;
    assert_eq!(st.state, RunState::Done, "{st:?}");
    assert_eq!(e.worker.analyze_calls.load(Ordering::SeqCst), 2);

    let e = env();
    build_scene(&e);
    let sid = import(&e).await;
    e.worker.crash_batches.store(10, Ordering::SeqCst);
    let st = run(&e, sid, Profile::Standard).await;
    assert_eq!(st.state, RunState::Failed);
    assert!(st.error.unwrap().contains("lost"));
    assert_eq!(e.worker.analyze_calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn failed_photos_are_reported_but_do_not_abort() {
    let e = env();
    build_scene(&e);
    let sid = import(&e).await;
    let bad = FakeSpec {
        fail: true,
        ..Default::default()
    };
    e.worker.set("c1.jpg", bad);
    let st = run(&e, sid, Profile::Standard).await;
    assert_eq!(st.state, RunState::Done);
    assert!(st.error.as_deref().unwrap().contains("1 of 6"));
    let ph = all(&e, sid).await;
    assert!(!ph["c1.jpg"].analyzed && ph["c1.jpg"].ai_rating.is_none());
    assert!(ph["a1.jpg"].analyzed);
    let d = e.core.photo_analysis(ph["c1.jpg"].id).await.unwrap();
    assert!(!d.analyzed && d.scores.is_none() && d.faces.is_empty());
    assert!(matches!(
        e.core.photo_analysis(999_999).await,
        Err(CoreError::NotFound(_))
    ));
}

#[tokio::test]
async fn accept_ai_writes_rounded_user_rating() {
    let e = env();
    build_scene(&e);
    let sid = import(&e).await;
    run(&e, sid, Profile::Standard).await;
    let ph = all(&e, sid).await;
    let unanalysed = {
        let e2 = &e;
        // add a new, not yet analysed photo
        photo(e2.src.path(), "z.jpg", 9000);
        import(e2).await
    };
    let z = all(&e, unanalysed).await["z.jpg"].id;
    let mut rx = e.core.events.subscribe();
    let ids = vec![ph["a2.jpg"].id, ph["b2.jpg"].id, z];
    let n = e.core.accept_ai(ids).await.unwrap();
    assert_eq!(n, 2, "unanalysed photos are skipped");
    let after = all(&e, sid).await;
    for name in ["a2.jpg", "b2.jpg"] {
        assert_eq!(
            after[name].user_rating,
            Some(ph[name].ai_rating.unwrap().round() as i64),
            "{name}"
        );
    }
    assert_eq!(after["a1.jpg"].user_rating, None);
    assert!(drain(&mut rx)
        .iter()
        .any(|e| matches!(e, Event::PhotosUpdated { items } if items.len() == 2)));
    assert!(matches!(
        e.core.accept_ai(vec![]).await,
        Err(CoreError::BadRequest(_))
    ));
}

#[tokio::test]
async fn face_crop_is_square_and_cached() {
    let e = env();
    build_scene(&e);
    let sid = import(&e).await;
    run(&e, sid, Profile::Standard).await;
    let ph = all(&e, sid).await;
    let d = e.core.photo_analysis(ph["a1.jpg"].id).await.unwrap();
    let fid = d.faces[0].id;
    for s in [128u32, 256] {
        let p = e.core.face_crop(fid, s).await.unwrap();
        let img = image::open(&p).unwrap();
        assert_eq!((img.width(), img.height()), (s, s));
        assert_eq!(
            e.core.face_crop(fid, s).await.unwrap(),
            p,
            "cached path is stable"
        );
    }
    assert!(matches!(
        e.core.face_crop(fid, 100).await,
        Err(CoreError::BadRequest(_))
    ));
    assert!(matches!(
        e.core.face_crop(987_654, 128).await,
        Err(CoreError::NotFound(_))
    ));
}

#[tokio::test]
async fn hardware_reports_worker_state_and_events() {
    let e = env();
    let mut rx = e.core.events.subscribe();
    let h = e.core.hardware(false).await.unwrap();
    assert_eq!(h.state, "stopped");
    assert!(h.tier.is_none() && h.gpu.is_none());
    let h = e.core.hardware(true).await.unwrap();
    assert_eq!(h.tier.as_deref(), Some("T3"));
    assert_eq!(h.device.as_deref(), Some("cuda"));
    assert_eq!(h.gpu.as_ref().unwrap().vram_mb, Some(24564));
    assert!(h.providers.contains(&"CUDAExecutionProvider".to_string()));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(drain(&mut rx)
        .iter()
        .any(|e| matches!(e, Event::WorkerStatus { state, tier, .. }
        if state == "ready" && tier.as_deref() == Some("T3"))));
}

#[tokio::test]
async fn deleting_a_session_cleans_groups_and_people() {
    let e = env();
    build_scene(&e);
    let sid = import(&e).await;
    run(&e, sid, Profile::Standard).await;
    assert_eq!(e.core.people(None).await.unwrap().len(), 2);
    e.core.delete_session(sid).await.unwrap();
    assert!(e.core.people(None).await.unwrap().is_empty());
    let n: i64 = e
        .core
        .db
        .call(|c| Ok(c.query_row("SELECT (SELECT COUNT(*) FROM burst)+(SELECT COUNT(*) FROM scene)+(SELECT COUNT(*) FROM face)+(SELECT COUNT(*) FROM analysis)", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn migration_v4_upgrades_a_v3_catalog() {
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    db::migrate_to(&mut conn, 3).unwrap();
    conn.execute_batch(
        "INSERT INTO root_folder(path, added_at) VALUES('/x', 0);
         INSERT INTO photo(root_id, rel_path, file_name, fast_key) VALUES(1,'a.jpg','a.jpg','k');",
    )
    .unwrap();
    assert_eq!(db::migrate(&mut conn).unwrap(), db::MIGRATIONS.len());
    let (fc, n): (Option<i64>, i64) = conn
        .query_row(
            "SELECT p.face_count, (SELECT COUNT(*) FROM analysis) FROM photo p",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((fc, n), (None, 0));
}
