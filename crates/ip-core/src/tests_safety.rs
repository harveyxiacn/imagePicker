//! Data safety (docs/09 §5.1 P0-1, P0-3, P0-4): originals keep their bytes (SHA-256 before and
//! after every kind of operation) unless the confirmed `modify_originals` XMP mode is on,
//! catalog backups / recovery of an unreadable catalog, and no face data before the user agreed
//! to face recognition. FakeImaging + FakeWorker + FakeRenderer.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ip_render::Backend;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::backup::{CatalogState, KEEP};
use crate::fake_worker::{person, FakeFace, FakeSpec, FakeWorker};
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
    core.analysis_run(AnalysisRunRequest {
        session_id: sid,
        profile: Profile::Standard,
        photo_ids: None,
        force,
        allow_download: false,
    })
    .await
    .unwrap();
    for _ in 0..1200 {
        let s = core.analysis_status(sid);
        if s.state != RunState::Running {
            assert_eq!(s.state, RunState::Done, "{s:?}");
            return;
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

async fn count(core: &Arc<Core>, sql: &'static str) -> i64 {
    core.db
        .call(move |c| Ok(c.query_row(sql, [], |r| r.get::<_, i64>(0))?))
        .await
        .unwrap()
}

fn sha256(p: &Path) -> String {
    format!("{:x}", Sha256::digest(std::fs::read(p).unwrap()))
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
