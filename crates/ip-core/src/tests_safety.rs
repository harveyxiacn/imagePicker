//! Data safety (docs/09 §5.1 P0-1, P0-3, P0-4): originals keep their bytes (SHA-256 before and
//! after every kind of operation) unless the confirmed `modify_originals` XMP mode is on,
//! catalog backups / recovery of an unreadable catalog, and no face data before the user agreed
//! to face recognition. FakeImaging + FakeWorker + FakeRenderer.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use ip_render::Backend;
use serde_json::json;

use crate::backup::{CatalogState, KEEP};
use crate::fake_worker::{person, FakeFace, FakeSpec, FakeWorker};
use crate::tests_common::{count, sha256};
use crate::testutil::{grant_face_consent, write_jpeg, FakeImaging, FakeRenderer};
use crate::xmp::XmpSyncRequest;
use crate::*;

const WAIT: Duration = Duration::from_secs(30);

struct Env {
    core: Arc<Core>,
    worker: Arc<FakeWorker>,
    data: tempfile::TempDir,
    src: tempfile::TempDir,
}

fn open(data: &Path) -> (Arc<Core>, Arc<FakeWorker>) {
    let worker = Arc::new(FakeWorker::new());
    let core = Core::open(CoreConfig {
        data_dir: Some(data.to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(2),
        worker: Some(worker.clone()),
        renderer: Some(Arc::new(FakeRenderer::new(Backend::Cpu))),
        force_cpu: true,
    })
    .unwrap();
    (core, worker)
}

fn env() -> Env {
    let (data, src) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (core, worker) = open(data.path());
    Env {
        core,
        worker,
        data,
        src,
    }
}

async fn import_dir(core: &Arc<Core>, dir: &Path) -> i64 {
    let s = core
        .import(ImportRequest {
            path: dir.to_string_lossy().into_owned(),
            recursive: true,
            title: None,
        })
        .await
        .unwrap();
    core.wait_session_ready(s.id, WAIT).await.unwrap();
    s.id
}

async fn analyze(core: &Arc<Core>, sid: i64, force: bool) {
    run_to_end(core, run_req(sid, Profile::Standard, force)).await;
}

fn run_req(sid: i64, profile: Profile, force: bool) -> AnalysisRunRequest {
    AnalysisRunRequest {
        session_id: sid,
        profile,
        photo_ids: None,
        force,
        allow_download: false,
        fallback_lite: false,
    }
}

/// Starts the run and waits until it is over; it must end `done`.
async fn run_to_end(core: &Arc<Core>, req: AnalysisRunRequest) -> AnalysisStatus {
    let sid = req.session_id;
    core.analysis_run(req).await.unwrap();
    // generous: lite runs are CPU-bound and share a small runner with the whole suite
    let t = std::time::Instant::now();
    while t.elapsed() < Duration::from_secs(300) {
        let s = core.analysis_status(sid);
        if s.state != RunState::Running {
            assert_eq!(s.state, RunState::Done, "{s:?}");
            return s;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("analysis did not finish");
}

async fn by_name(core: &Arc<Core>, sid: i64) -> HashMap<String, Photo> {
    core.photos(PhotoQuery {
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

/// Exports `ids` into `dest` (saved edits applied) and waits for the task.
async fn export_to(core: &Arc<Core>, ids: Vec<i64>, dest: &Path) {
    let mut rx = core.events.subscribe();
    let req: ExportRequest =
        serde_json::from_value(json!({"ids": ids, "dest": dest.to_string_lossy()})).unwrap();
    let task = core.export(req).await.unwrap();
    tokio::time::timeout(WAIT, async {
        loop {
            if let Ok(Event::TaskProgress {
                task_id,
                state,
                error,
                ..
            }) = rx.recv().await
            {
                if task_id == task && state != "running" {
                    assert_eq!(state, "done", "{error:?}");
                    return;
                }
            }
        }
    })
    .await
    .expect("export finishes");
}

/// SHA-256 of every image file under `dir` (not the `.xmp` sidecars the app may write).
fn originals(dir: &Path) -> BTreeMap<PathBuf, String> {
    let mut out = BTreeMap::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                todo.push(p);
            } else if p
                .extension()
                .is_some_and(|x| !x.eq_ignore_ascii_case("xmp"))
            {
                out.insert(p.clone(), sha256(&p));
            }
        }
    }
    out
}

const SIDECAR: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:Rating="3" xmp:Label="Blue"/></rdf:RDF></x:xmpmeta>"#;

// ------------------------------------------------------------------ P0-1 originals

#[tokio::test]
async fn originals_keep_their_bytes_outside_modify_originals() {
    let e = env();
    grant_face_consent(&e.core);
    e.core.set_xmp_debounce(Duration::from_millis(20));
    let sub = e.src.path().join("day 2");
    write_jpeg(&e.src.path().join("a.jpg"), 320, 240, 10);
    write_jpeg(&e.src.path().join("b.jpg"), 240, 320, 90);
    write_jpeg(&sub.join("c.jpg"), 320, 240, 170);
    // b already has a sidecar from another app
    std::fs::write(e.src.path().join("b.xmp"), SIDECAR).unwrap();
    let face = FakeFace::new([0.2, 0.2, 0.3, 0.4], person(1, 0.0));
    e.worker
        .set("a.jpg", FakeSpec::at(0.0).with_faces(vec![face]));
    let before = originals(e.src.path());
    assert_eq!(before.len(), 3);

    e.core
        .patch_settings(json!({"xmp_mode": "sidecar"}))
        .await
        .unwrap();
    // import (reads the sidecar) and analysis
    let sid = import_dir(&e.core, e.src.path()).await;
    analyze(&e.core, sid, false).await;
    let ph = by_name(&e.core, sid).await;
    let (a, b, c) = (ph["a.jpg"].id, ph["b.jpg"].id, ph["c.jpg"].id);
    assert_eq!(ph["b.jpg"].user_rating, Some(3), "the sidecar was read");
    assert!(count(&e.core, "SELECT COUNT(*) FROM face").await > 0);

    // rating / flag / colour / keywords / accepted AI ratings, written to sidecars
    e.core
        .patch_photos(PatchRequest {
            ids: vec![a, c],
            user_rating: Some(Some(5)),
            color_label: Some(Some("red".into())),
            ..Default::default()
        })
        .await
        .unwrap();
    e.core
        .patch_photos(PatchRequest {
            ids: vec![b],
            flag: Some(-1),
            ..Default::default()
        })
        .await
        .unwrap();
    e.core
        .set_tags(vec![a, b], vec!["trip".into()], vec![])
        .await
        .unwrap();
    e.core.accept_ai(vec![c]).await.unwrap();
    let r = e.core.xmp_flush().await.unwrap();
    assert!(r.errors.is_empty(), "{r:?}");
    for direction in ["write", "read"] {
        let r = e
            .core
            .xmp_sync(XmpSyncRequest {
                session_id: sid,
                direction: direction.into(),
                photo_ids: None,
            })
            .await
            .unwrap();
        assert!(r.errors.is_empty(), "{r:?}");
    }
    assert!(e.src.path().join("a.xmp").exists(), "sidecars were written");
    assert!(sub.join("c.xmp").exists());

    // an edit, then an export of the edited photo and an unedited copy into the source
    // folders themselves (same names: the export must pick new ones)
    e.core
        .put_edit(
            a,
            json!({"version": 1, "ops": [{"type": "global", "exposure": 0.4}]}),
        )
        .await
        .unwrap();
    export_to(&e.core, vec![a, b], e.src.path()).await;
    export_to(&e.core, vec![c], &sub).await;
    // removing the session only touches the catalog
    e.core.delete_session(sid).await.unwrap();

    let after = originals(e.src.path());
    for (p, h) in &before {
        assert_eq!(after.get(p), Some(h), "{} changed", p.display());
    }
    assert!(
        after.len() > before.len(),
        "the exports exist next to the originals"
    );

    // `modify_originals` needs a confirmation, and then really rewrites the JPEG (so the check
    // above would have noticed a change)
    assert!(matches!(
        e.core
            .patch_settings(json!({"xmp_mode": "modify_originals"}))
            .await,
        Err(CoreError::Unprocessable(_))
    ));
    assert_eq!(e.core.settings().xmp_mode, "sidecar");
    e.core
        .patch_settings(json!({"xmp_mode": "modify_originals", "confirm_modify_originals": true}))
        .await
        .unwrap();
    let sid = import_dir(&e.core, e.src.path()).await;
    let a = by_name(&e.core, sid).await["a.jpg"].id;
    e.core
        .patch_photos(PatchRequest {
            ids: vec![a],
            user_rating: Some(Some(2)),
            ..Default::default()
        })
        .await
        .unwrap();
    let r = e.core.xmp_flush().await.unwrap();
    assert!(r.errors.is_empty(), "{r:?}");
    let a_path = e.src.path().join("a.jpg");
    assert_ne!(
        sha256(&a_path),
        before[&a_path],
        "modify_originals rewrites"
    );
    assert_eq!(sha256(&sub.join("c.jpg")), before[&sub.join("c.jpg")]);
}

// ------------------------------------------------------------------ P0-3 catalog

#[tokio::test]
async fn unreadable_catalog_is_moved_aside_and_restored_from_a_backup() {
    let e = env();
    for (n, shade) in [("a.jpg", 10), ("b.jpg", 90), ("c.jpg", 170)] {
        write_jpeg(&e.src.path().join(n), 320, 240, shade);
    }
    let sid = import_dir(&e.core, e.src.path()).await;
    let ph = by_name(&e.core, sid).await;
    let (a, b) = (ph["a.jpg"].id, ph["b.jpg"].id);
    let rate = |id: i64, r: i64| PatchRequest {
        ids: vec![id],
        user_rating: Some(Some(r)),
        ..Default::default()
    };
    e.core.patch_photos(rate(a, 5)).await.unwrap();
    assert_eq!(e.core.catalog_check().await.unwrap(), CatalogState::Ok);
    let backup = e.core.catalog_backup().await.unwrap();
    assert_eq!(backup.kind, "auto");
    assert!(backup.bytes > 0);
    // a change after the backup: a restore loses it
    e.core.patch_photos(rate(b, 2)).await.unwrap();
    let st = e.core.catalog_status().await;
    assert_eq!(st.state, CatalogState::Ok);
    assert_eq!(st.backups.len(), 1);
    assert_eq!(st.last_backup_at, Some(backup.created_at));

    // a copy of the data directory whose catalog.db got garbage over its header
    let copy = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(copy.path().join("backups")).unwrap();
    std::fs::copy(
        e.data.path().join("backups").join(&backup.name),
        copy.path().join("backups").join(&backup.name),
    )
    .unwrap();
    let mut bytes = std::fs::read(e.data.path().join("catalog.db")).unwrap();
    // (most of the content may still sit in the WAL, which is not copied)
    bytes.resize(bytes.len().max(8192), 0);
    bytes[..4096].fill(0xA5);
    std::fs::write(copy.path().join("catalog.db"), &bytes).unwrap();

    // it still starts: the bad file is moved aside, an empty catalog is used, backups offered
    let (core, _worker) = open(copy.path());
    let st = core.catalog_status().await;
    assert_eq!(st.state, CatalogState::Recovery, "{st:?}");
    assert!(!st.problems.is_empty());
    let moved = PathBuf::from(st.moved_to.clone().unwrap());
    assert!(moved.is_file() && moved.starts_with(copy.path().join("backups")));
    assert_eq!(std::fs::read(&moved).unwrap(), bytes, "kept as it was");
    assert!(st.backups.iter().any(|x| x.name == backup.name));
    assert!(st.backups.iter().any(|x| x.kind == "corrupt"));
    assert!(core.sessions().await.unwrap().is_empty());
    // recovery stays until a restore; no backup of the empty replacement
    assert_eq!(core.catalog_check().await.unwrap(), CatalogState::Recovery);
    assert!(matches!(
        core.catalog_backup().await,
        Err(CoreError::Conflict(_))
    ));
    core.daily_backup().await;
    assert_eq!(
        crate::backup::list_backups(&copy.path().join("backups"))
            .iter()
            .filter(|x| x.kind == "auto")
            .count(),
        1
    );

    // bad requests leave everything as it is
    assert!(matches!(
        core.catalog_restore("../catalog.db").await,
        Err(CoreError::BadRequest(_))
    ));
    assert!(matches!(
        core.catalog_restore("catalog-20000101-000000-000.db").await,
        Err(CoreError::NotFound(_))
    ));
    let moved_name = moved.file_name().unwrap().to_string_lossy().into_owned();
    assert!(matches!(
        core.catalog_restore(&moved_name).await,
        Err(CoreError::Unprocessable(_))
    ));
    assert_eq!(core.catalog_guard.state(), CatalogState::Recovery);

    // restore: the catalog as of the backup
    let st = core.catalog_restore(&backup.name).await.unwrap();
    assert_eq!(st.state, CatalogState::Ok);
    assert_eq!(st.restored_from.as_deref(), Some(backup.name.as_str()));
    assert!(st.backups.iter().any(|x| x.kind == "replaced"));
    let sessions = core.sessions().await.unwrap();
    assert_eq!(sessions.len(), 1);
    let ph = by_name(&core, sessions[0].id).await;
    assert_eq!(ph.len(), 3);
    assert_eq!(ph["a.jpg"].user_rating, Some(5));
    assert_eq!(ph["b.jpg"].user_rating, None, "made after the backup");
    // the restored catalog is fully usable
    core.patch_photos(rate(ph["c.jpg"].id, 4)).await.unwrap();
    assert_eq!(
        by_name(&core, sessions[0].id).await["c.jpg"].user_rating,
        Some(4)
    );
    assert_eq!(core.catalog_check().await.unwrap(), CatalogState::Ok);
    assert!(core.catalog_backup().await.is_ok());
}

#[tokio::test]
async fn daily_backups_skip_empty_catalogs_and_keep_seven() {
    let e = env();
    let dir = e.data.path().join("backups");
    e.core.catalog_check().await.unwrap();
    e.core.daily_backup().await;
    assert!(
        crate::backup::list_backups(&dir).is_empty(),
        "an empty catalog is not backed up"
    );
    write_jpeg(&e.src.path().join("a.jpg"), 320, 240, 10);
    import_dir(&e.core, e.src.path()).await;
    e.core.daily_backup().await;
    assert_eq!(crate::backup::list_backups(&dir).len(), 1);
    e.core.daily_backup().await;
    assert_eq!(
        crate::backup::list_backups(&dir).len(),
        1,
        "one a day is enough"
    );
    // older backups (names sort by time) are dropped beyond KEEP
    for i in 0..10 {
        std::fs::write(
            dir.join(format!("catalog-2000010{}-000000-000.db", i % 10)),
            b"old",
        )
        .unwrap();
    }
    e.core.catalog_backup().await.unwrap();
    let left = crate::backup::list_backups(&dir);
    assert_eq!(left.len(), KEEP);
    let old: Vec<&str> = left
        .iter()
        .map(|b| b.name.as_str())
        .filter(|n| n.starts_with("catalog-2000"))
        .collect();
    assert_eq!(old.len(), KEEP - 2, "{left:?}");
    assert!(old.iter().all(|n| *n >= "catalog-20000105"), "{old:?}");
}

// ------------------------------------------------------------------ P0-4 face consent

#[tokio::test]
async fn no_face_data_before_consent() {
    let e = env();
    assert!(!e.core.settings().faces.consented, "nobody agreed yet");
    for (name, p) in [("f1.jpg", 1usize), ("f2.jpg", 2)] {
        write_jpeg(&e.src.path().join(name), 320, 240, 40 * p as u8);
        let face = FakeFace::new([0.2, 0.2, 0.3, 0.4], person(p, 0.0));
        e.worker.set(name, FakeSpec::at(0.0).with_faces(vec![face]));
    }
    let sid = import_dir(&e.core, e.src.path()).await;
    analyze(&e.core, sid, false).await;
    // the worker was not asked for faces, and nothing it returned about faces was stored
    let steps = e.worker.last_steps.lock().unwrap().clone().unwrap();
    assert!(
        !steps.iter().any(|s| s == "faces" || s == "identity"),
        "{steps:?}"
    );
    assert_eq!(count(&e.core, "SELECT COUNT(*) FROM face").await, 0);
    assert_eq!(count(&e.core, "SELECT COUNT(*) FROM person").await, 0);
    assert_eq!(
        count(
            &e.core,
            "SELECT COUNT(*) FROM photo WHERE face_count IS NOT NULL AND face_count > 0"
        )
        .await,
        0
    );
    assert_eq!(
        count(&e.core, "SELECT COUNT(*) FROM analysis").await,
        2,
        "the rest of the analysis ran"
    );
    assert_eq!(
        count(
            &e.core,
            "SELECT COUNT(*) FROM photo WHERE face_count IS NOT NULL OR subject_face_count IS NOT NULL"
        )
        .await,
        0,
        "faces were not looked for: the counts are unknown, not zero"
    );
    assert!(e.core.people(Some(sid)).await.unwrap().is_empty());
    // the on-device profile does not detect faces either
    assert_eq!(
        e.core.analysis_steps(Profile::Lite).await,
        Some(vec!["phash".to_string(), "quality".to_string()])
    );

    // declining switches face recognition off: still nothing
    e.core
        .patch_settings(json!({"faces": {"enabled": false}}))
        .await
        .unwrap();
    analyze(&e.core, sid, true).await;
    assert_eq!(count(&e.core, "SELECT COUNT(*) FROM face").await, 0);

    // agreeing: faces and their identity embeddings
    e.core
        .patch_settings(json!({"faces": {"enabled": true, "consented": true}}))
        .await
        .unwrap();
    assert!(e.core.settings().faces.allowed());
    assert_eq!(e.core.analysis_steps(Profile::Lite).await, None);
    analyze(&e.core, sid, true).await;
    assert!(e.worker.last_steps.lock().unwrap().is_none());
    assert_eq!(count(&e.core, "SELECT COUNT(*) FROM face").await, 2);
    assert_eq!(
        count(
            &e.core,
            "SELECT COUNT(*) FROM face WHERE embedding IS NOT NULL"
        )
        .await,
        2
    );
    // withdrawing consent stops new face extraction (stored data stays until cleared)
    e.core
        .patch_settings(json!({"faces": {"consented": false}}))
        .await
        .unwrap();
    assert!(e.core.analysis_steps(Profile::Standard).await.is_some());
}

/// Every stored face (id, photo, person when assigned by hand, locked, has an identity
/// embedding) and the face counts of every photo, to compare before and after a run.
type FaceState = (
    Vec<(i64, i64, Option<i64>, i64, bool)>,
    Vec<(i64, Option<i64>, Option<i64>)>,
);

async fn face_state(core: &Arc<Core>) -> FaceState {
    core.db
        .call(|c| {
            let mut st = c.prepare(
                "SELECT id, photo_id, CASE WHEN person_locked=1 THEN person_id END, person_locked,
                        embedding IS NOT NULL FROM face ORDER BY id",
            )?;
            let faces = st
                .query_map([], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut st =
                c.prepare("SELECT id, face_count, subject_face_count FROM photo ORDER BY id")?;
            let photos = st
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((faces, photos))
        })
        .await
        .unwrap()
}

/// Consent given, `f1.jpg` and `f2.jpg` with one face each analysed, and the face in `f1.jpg`
/// named "Ann" by hand (a locked assignment). Returns the session, the id of `f1.jpg` and Ann.
async fn named_faces(e: &Env) -> (i64, i64, i64) {
    grant_face_consent(&e.core);
    for (name, p) in [("f1.jpg", 1usize), ("f2.jpg", 2)] {
        write_jpeg(&e.src.path().join(name), 320, 240, 40 * p as u8);
        let face = FakeFace::new([0.2, 0.2, 0.3, 0.4], person(p, 0.0));
        e.worker.set(name, FakeSpec::at(0.0).with_faces(vec![face]));
    }
    let sid = import_dir(&e.core, e.src.path()).await;
    analyze(&e.core, sid, false).await;
    let f1 = by_name(&e.core, sid).await["f1.jpg"].id;
    let face: i64 = e
        .core
        .db
        .call(
            move |c| Ok(c.query_row("SELECT id FROM face WHERE photo_id=?1", [f1], |r| r.get(0))?),
        )
        .await
        .unwrap();
    let ann = e
        .core
        .create_person(CreatePersonRequest {
            face_ids: vec![face],
            name: Some("Ann".into()),
        })
        .await
        .unwrap();
    let st = face_state(&e.core).await;
    assert_eq!(st.0.len(), 2, "{st:?}");
    assert!(st.0.iter().all(|f| f.4), "identity embeddings: {st:?}");
    assert!(st.1.iter().all(|p| p.1 == Some(1)), "{st:?}");
    (sid, f1, ann.id)
}

#[tokio::test]
async fn withdrawing_consent_keeps_stored_faces_and_people_on_reanalysis() {
    let e = env();
    let (sid, f1, ann) = named_faces(&e).await;
    let before = face_state(&e.core).await;
    e.core
        .patch_settings(json!({"faces": {"consented": false}}))
        .await
        .unwrap();
    let calls = e.worker.analyze_calls.load(Ordering::SeqCst);
    analyze(&e.core, sid, true).await;
    assert!(
        e.worker.analyze_calls.load(Ordering::SeqCst) > calls,
        "the photos were analysed again"
    );
    let steps = e.worker.last_steps.lock().unwrap().clone().unwrap();
    assert!(!steps.iter().any(|s| s == "faces"), "{steps:?}");
    // no new face data, and the stored faces, Ann and her face stay until cleared
    assert_eq!(face_state(&e.core).await, before);
    assert_eq!(e.core.person_face_ids(ann).await.unwrap().len(), 1);

    // face recognition switched off, the photo re-analysed by id: the same
    e.core
        .patch_settings(json!({"faces": {"enabled": false}}))
        .await
        .unwrap();
    run_to_end(
        &e.core,
        AnalysisRunRequest {
            photo_ids: Some(vec![f1]),
            ..run_req(sid, Profile::Standard, false)
        },
    )
    .await;
    assert_eq!(face_state(&e.core).await, before);
    assert_eq!(e.core.person_face_ids(ann).await.unwrap().len(), 1);

    // clearing is what removes them
    e.core.clear_faces().await.unwrap();
    assert_eq!(count(&e.core, "SELECT COUNT(*) FROM face").await, 0);
}

#[tokio::test]
async fn a_skipped_faces_step_keeps_stored_faces() {
    let e = env();
    let (sid, _f1, ann) = named_faces(&e).await;
    let before = face_state(&e.core).await;
    // no face detection this time: the worker reports the step skipped and finds no faces
    for name in ["f1.jpg", "f2.jpg"] {
        e.worker.set(name, FakeSpec::at(0.0));
    }
    e.worker.set_skipped(&["faces", "identity"]);
    let st = run_to_end(&e.core, run_req(sid, Profile::Standard, true)).await;
    assert!(
        st.skipped_steps.iter().any(|s| s == "faces"),
        "{:?}",
        st.skipped_steps
    );
    assert_eq!(face_state(&e.core).await, before);
    assert_eq!(e.core.person_face_ids(ann).await.unwrap().len(), 1);
}

#[tokio::test]
async fn lite_runs_keep_stored_faces() {
    let e = env();
    let (sid, _f1, ann) = named_faces(&e).await;
    let before = face_state(&e.core).await;
    // the AI is gone at the first batch: the run falls back to lite, which detects no faces
    e.worker.analyze_unavailable.store(true, Ordering::SeqCst);
    let st = run_to_end(
        &e.core,
        AnalysisRunRequest {
            fallback_lite: true,
            ..run_req(sid, Profile::Standard, true)
        },
    )
    .await;
    assert_eq!(
        (st.profile, st.fallback_from),
        (Some(Profile::Lite), Some(Profile::Standard))
    );
    assert_eq!(
        count(
            &e.core,
            "SELECT COUNT(*) FROM analysis WHERE profile='lite'"
        )
        .await,
        2
    );
    assert_eq!(face_state(&e.core).await, before);
    // a lite run asked for directly
    e.worker.analyze_unavailable.store(false, Ordering::SeqCst);
    run_to_end(&e.core, run_req(sid, Profile::Lite, true)).await;
    assert_eq!(face_state(&e.core).await, before);
    assert_eq!(e.core.person_face_ids(ann).await.unwrap().len(), 1);
}

#[tokio::test]
async fn faces_no_longer_found_are_cleared_when_the_faces_step_ran() {
    let e = env();
    let (sid, f1, _ann) = named_faces(&e).await;
    // consent still given; the face in f1 is not found again
    e.worker.set("f1.jpg", FakeSpec::at(0.0));
    analyze(&e.core, sid, true).await;
    let ph = by_name(&e.core, sid).await;
    let n1 = e
        .core
        .db
        .call(move |c| {
            Ok(
                c.query_row("SELECT COUNT(*) FROM face WHERE photo_id=?1", [f1], |r| {
                    r.get::<_, i64>(0)
                })?,
            )
        })
        .await
        .unwrap();
    assert_eq!(n1, 0, "the locked face is gone with its detection");
    assert_eq!(
        (ph["f1.jpg"].face_count, ph["f1.jpg"].subject_face_count),
        (Some(0), Some(0))
    );
    assert_eq!(ph["f2.jpg"].face_count, Some(1));
    assert_eq!(count(&e.core, "SELECT COUNT(*) FROM face").await, 1);
}
