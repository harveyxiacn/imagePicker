//! WP-F1 (docs/09 §5.1 P0-7, §5.2 P1-6): RAW+JPEG pairing, import exclusions, file watching,
//! missing files + relocate, safe reject. FakeImaging + FakeWorker + FakeRenderer with synthetic
//! files only (a "RAW" is any file with a RAW extension: companions are never decoded).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ip_render::Backend;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::fake_worker::FakeWorker;
use crate::files::{pair_all, Trasher};
use crate::testutil::{write_jpeg, FakeImaging, FakeRenderer};
use crate::*;

const WAIT: Duration = Duration::from_secs(30);

fn open(data: &Path) -> Arc<Core> {
    Core::open(CoreConfig {
        data_dir: Some(data.to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(2),
        worker: Some(Arc::new(FakeWorker::new())),
        renderer: Some(Arc::new(FakeRenderer::new(Backend::Cpu))),
        force_cpu: true,
    })
    .unwrap()
}

struct Env {
    core: Arc<Core>,
    _data: tempfile::TempDir,
    src: tempfile::TempDir,
}

fn env() -> Env {
    let (data, src) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    Env {
        core: open(data.path()),
        _data: data,
        src,
    }
}

fn write_raw(path: &Path, seed: u8) {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).unwrap();
    }
    std::fs::write(path, vec![seed; 4096]).unwrap();
}

/// a.jpg + a.CR2 (a pair), b.jpg (alone), c.dng (RAW alone), sub/d.JPG + sub/d.nef (a pair,
/// other case), plus a.jpg's neighbour `a2.jpg` that must not pair with `a.CR2`.
fn make_library(dir: &Path) {
    write_jpeg(&dir.join("a.jpg"), 64, 48, 10);
    write_raw(&dir.join("a.CR2"), 1);
    write_jpeg(&dir.join("a2.jpg"), 64, 48, 20);
    write_jpeg(&dir.join("b.jpg"), 48, 64, 30);
    write_raw(&dir.join("c.dng"), 2);
    write_jpeg(&dir.join("sub/d.JPG"), 64, 48, 40);
    write_raw(&dir.join("sub/d.nef"), 3);
}

async fn import_dir(core: &Arc<Core>, dir: &Path) -> i64 {
    import_with(core, dir, ImportOptions::default()).await
}

async fn import_with(core: &Arc<Core>, dir: &Path, opts: ImportOptions) -> i64 {
    let s = core
        .import_with(
            ImportRequest {
                path: dir.to_string_lossy().into_owned(),
                recursive: true,
                title: None,
            },
            opts,
        )
        .await
        .unwrap();
    core.wait_session_ready(s.id, WAIT).await.unwrap();
    s.id
}

async fn photos(core: &Arc<Core>, sid: i64) -> HashMap<String, Photo> {
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

async fn count(core: &Arc<Core>, sql: &'static str) -> i64 {
    core.db
        .call(move |c| Ok(c.query_row(sql, [], |r| r.get::<_, i64>(0))?))
        .await
        .unwrap()
}

fn sha256(p: &Path) -> String {
    format!("{:x}", Sha256::digest(std::fs::read(p).unwrap()))
}

fn names(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                todo.push(p);
            } else {
                out.push(
                    p.strip_prefix(dir)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    out.sort();
    out
}

async fn wait_for<F, Fut>(what: &str, secs: u64, mut f: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(secs) {
        if f().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for: {what}");
}

async fn export_to(core: &Arc<Core>, ids: Vec<i64>, dest: &Path, member: PairMember) {
    let mut rx = core.events.subscribe();
    let req: ExportRequest =
        serde_json::from_value(json!({"ids": ids, "dest": dest.to_string_lossy()})).unwrap();
    let task = core.export_with(req, member).await.unwrap();
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

// ---------------------------------------------------------------- P0-7 pairing

#[tokio::test(flavor = "multi_thread")]
async fn raw_and_jpeg_with_the_same_name_are_one_grid_item() {
    let e = env();
    make_library(e.src.path());
    let sid = import_dir(&e.core, e.src.path()).await;
    let ph = photos(&e.core, sid).await;
    let mut got: Vec<_> = ph.keys().cloned().collect();
    got.sort();
    assert_eq!(got, ["a.jpg", "a2.jpg", "b.jpg", "c.dng", "d.JPG"]);
    assert_eq!(e.core.session(sid).await.unwrap().photo_count, 5);

    let a = &ph["a.jpg"];
    assert_eq!(a.companions.len(), 1);
    assert_eq!(a.companions[0].file_name, "a.CR2");
    assert_eq!(a.companions[0].format, "raw");
    assert!(!a.companions[0].missing);
    assert_eq!(ph["d.JPG"].companions[0].file_name, "d.nef");
    assert!(ph["a2.jpg"].companions.is_empty(), "a2 is another shot");
    assert!(ph["b.jpg"].companions.is_empty());
    assert!(
        ph["c.dng"].companions.is_empty(),
        "a RAW alone stays a grid item"
    );
    assert_eq!(a.format, "jpeg");
    // single-photo endpoint carries the companions too
    assert_eq!(e.core.photo(a.id).await.unwrap().companions.len(), 1);

    // the companion row exists (for sync/export) but belongs to no session
    assert_eq!(
        count(
            &e.core,
            "SELECT COUNT(*) FROM photo WHERE pair_id IS NOT NULL"
        )
        .await,
        2
    );
    assert_eq!(count(&e.core, "SELECT COUNT(*) FROM photo").await, 7);
    assert_eq!(
        count(&e.core, "SELECT COUNT(*) FROM session_photo").await,
        5
    );

    // removing the session removes the companions with it (catalog rows only)
    let tmp = tempfile::tempdir().unwrap();
    write_jpeg(&tmp.path().join("z.jpg"), 32, 32, 9);
    write_raw(&tmp.path().join("z.cr2"), 9);
    let zsid = import_dir(&e.core, tmp.path()).await;
    assert_eq!(count(&e.core, "SELECT COUNT(*) FROM photo").await, 9);
    e.core.delete_session(zsid).await.unwrap();
    assert_eq!(count(&e.core, "SELECT COUNT(*) FROM photo").await, 7);
    assert!(
        tmp.path().join("z.cr2").is_file(),
        "originals are never touched"
    );

    // importing again changes nothing
    let sid2 = import_dir(&e.core, e.src.path()).await;
    assert_eq!(photos(&e.core, sid2).await.len(), 5);
    assert_eq!(count(&e.core, "SELECT COUNT(*) FROM photo").await, 7);
    assert_eq!(
        count(&e.core, "SELECT COUNT(*) FROM session_photo sp JOIN photo p ON p.id=sp.photo_id WHERE p.pair_id IS NOT NULL").await,
        0
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn rating_flag_and_label_reach_both_files_and_both_sidecars() {
    let e = env();
    make_library(e.src.path());
    e.core
        .settings
        .patch(&json!({"xmp_mode": "sidecar"}))
        .unwrap();
    let before = (
        sha256(&e.src.path().join("a.jpg")),
        sha256(&e.src.path().join("a.CR2")),
    );
    let sid = import_dir(&e.core, e.src.path()).await;
    let ph = photos(&e.core, sid).await;
    let (a, b) = (ph["a.jpg"].id, ph["b.jpg"].id);
    e.core
        .patch_photos(PatchRequest {
            ids: vec![a, b],
            user_rating: Some(Some(4)),
            flag: Some(1),
            color_label: Some(Some("red".into())),
        })
        .await
        .unwrap();
    // catalog: the companion row mirrors the JPEG
    let comp = ph["a.jpg"].companions[0].id;
    let row: (Option<i64>, i64, Option<String>) = e
        .core
        .db
        .call(move |c| {
            Ok(c.query_row(
                "SELECT user_rating, flag, color_label FROM photo WHERE id=?1",
                [comp],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(row, (Some(4), 1, Some("red".into())));
    // sidecars of both files (the RAW keeps `<file>.<ext>.xmp`, the JPEG `<stem>.xmp`)
    e.core.xmp_flush().await.unwrap();
    let dir = e.src.path();
    for sc in ["a.xmp", "a.CR2.xmp", "b.xmp"] {
        let text = std::fs::read_to_string(dir.join(sc)).unwrap_or_else(|_| panic!("{sc} written"));
        let x = crate::xmp::Xmp::parse(&text).unwrap();
        assert_eq!(x.rating(), Some(4), "{sc}");
        assert_eq!(x.label().as_deref(), Some("Red"), "{sc}");
    }
    // clearing it clears both
    e.core
        .patch_photos(PatchRequest {
            ids: vec![a],
            user_rating: Some(None),
            flag: Some(0),
            color_label: Some(None),
        })
        .await
        .unwrap();
    e.core.xmp_flush().await.unwrap();
    for sc in ["a.xmp", "a.CR2.xmp"] {
        let x = crate::xmp::Xmp::parse(&std::fs::read_to_string(dir.join(sc)).unwrap()).unwrap();
        assert_eq!(x.rating().unwrap_or(0), 0, "{sc}");
    }
    // the originals were not touched
    assert_eq!(before.0, sha256(&dir.join("a.jpg")));
    assert_eq!(before.1, sha256(&dir.join("a.CR2")));
}

#[tokio::test(flavor = "multi_thread")]
async fn analysis_and_groups_count_a_pair_once() {
    let e = env();
    make_library(e.src.path());
    let sid = import_dir(&e.core, e.src.path()).await;
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
    wait_for("analysis", 30, || async {
        e.core.analysis_status(sid).state != RunState::Running
    })
    .await;
    // five grid items analysed, the two RAW companions are not
    assert_eq!(count(&e.core, "SELECT COUNT(*) FROM analysis").await, 5);
    let g = e.core.groups(sid).await.unwrap();
    let in_groups: usize = g
        .scenes
        .iter()
        .flat_map(|s| s.bursts.iter())
        .map(|b| b.photo_ids.len())
        .sum();
    assert!(in_groups <= 5, "no companion in any burst: {in_groups}");
    assert_eq!(
        count(
            &e.core,
            "SELECT COUNT(*) FROM photo WHERE pair_id IS NOT NULL AND burst_id IS NOT NULL"
        )
        .await,
        0
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn export_chooses_which_member_of_a_pair() {
    let e = env();
    make_library(e.src.path());
    let sid = import_dir(&e.core, e.src.path()).await;
    let ph = photos(&e.core, sid).await;
    let ids = vec![ph["a.jpg"].id, ph["b.jpg"].id];

    let out = tempfile::tempdir().unwrap();
    export_to(
        &e.core,
        ids.clone(),
        &out.path().join("jpeg"),
        PairMember::default(),
    )
    .await;
    assert_eq!(names(&out.path().join("jpeg")), ["a.jpg", "b.jpg"]);

    export_to(
        &e.core,
        ids.clone(),
        &out.path().join("raw"),
        PairMember::Raw,
    )
    .await;
    // the RAW instead of the JPEG for the pair; the unpaired JPEG is exported as it is
    assert_eq!(names(&out.path().join("raw")), ["a.CR2", "b.jpg"]);
    assert_eq!(
        sha256(&out.path().join("raw/a.CR2")),
        sha256(&e.src.path().join("a.CR2"))
    );

    export_to(&e.core, ids, &out.path().join("both"), PairMember::Both).await;
    assert_eq!(names(&out.path().join("both")), ["a.CR2", "a.jpg", "b.jpg"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn catalogs_from_before_pairing_migrate_and_pair() {
    // a v11 catalog where the RAW was imported as its own photo, rated, and the JPEG was not
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    {
        let mut conn = rusqlite::Connection::open(&path).unwrap();
        assert_eq!(crate::db::migrate_to(&mut conn, 11).unwrap(), 11);
        conn.execute_batch(
            "INSERT INTO root_folder(id, path, added_at) VALUES(1, '/photos', 0);
             INSERT INTO session(id, title, created_at, root_id, import_state) VALUES(1,'old',0,1,'ready');
             INSERT INTO photo(id, root_id, rel_path, file_name, fast_key, format, meta_done) VALUES
               (1,1,'IMG_1.jpg','IMG_1.jpg','k1','jpeg',1),
               (2,1,'IMG_1.CR2','IMG_1.CR2','k2','raw',1),
               (3,1,'IMG_2.CR2','IMG_2.CR2','k3','raw',1);
             UPDATE photo SET user_rating=5, flag=1, color_label='blue' WHERE id=2;
             INSERT INTO session_photo VALUES (1,1),(1,2),(1,3);
             UPDATE session SET cover_photo_id=2 WHERE id=1;",
        )
        .unwrap();
    }
    let core = open(dir.path());
    let page = core
        .photos(PhotoQuery {
            session_id: 1,
            ..Default::default()
        })
        .await
        .unwrap();
    let mut got: Vec<_> = page.photos.iter().map(|p| p.file_name.as_str()).collect();
    got.sort();
    assert_eq!(got, ["IMG_1.jpg", "IMG_2.CR2"]);
    let jpg = page
        .photos
        .iter()
        .find(|p| p.file_name == "IMG_1.jpg")
        .unwrap();
    assert_eq!(jpg.companions[0].file_name, "IMG_1.CR2");
    // the JPEG had no decision of its own and adopts the RAW's; the RAW mirrors it
    assert_eq!(
        (jpg.user_rating, jpg.flag, jpg.color_label.as_deref()),
        (Some(5), 1, Some("blue"))
    );
    assert_eq!(core.session(1).await.unwrap().cover_photo_id, Some(1));
    assert_eq!(core.session(1).await.unwrap().photo_count, 2);
    // idempotent
    let n = core.db.with(pair_all).unwrap();
    assert_eq!(n, 0);
    assert_eq!(
        count(&core, "SELECT recursive FROM session WHERE id=1").await,
        1
    );
}

// ---------------------------------------------------------------- P1-6 exclude

#[tokio::test(flavor = "multi_thread")]
async fn import_exclude_patterns_and_subfolders() {
    let e = env();
    let d = e.src.path();
    write_jpeg(&d.join("keep.jpg"), 32, 32, 1);
    write_jpeg(&d.join("c1.jpg"), 32, 32, 2);
    write_jpeg(&d.join("skip/a.jpg"), 32, 32, 3);
    write_jpeg(&d.join("backup/2020/b.jpg"), 32, 32, 4);
    write_jpeg(&d.join("trip/ok.jpg"), 32, 32, 5);
    write_jpeg(&d.join("trip/rejects/x.jpg"), 32, 32, 6);
    let abs = d.join("skip").to_string_lossy().into_owned();
    let sid = import_with(
        &e.core,
        d,
        ImportOptions {
            exclude: vec![
                "c1.*".into(),
                "backup/**".into(),
                "trip/rejects".into(),
                abs,
            ],
        },
    )
    .await;
    let mut got: Vec<String> = photos(&e.core, sid).await.into_keys().collect();
    got.sort();
    assert_eq!(got, ["keep.jpg", "ok.jpg"]);

    // the same exclusions apply to rescans (file watching)
    write_jpeg(&d.join("skip/new.jpg"), 32, 32, 7);
    write_jpeg(&d.join("c1x.jpg"), 32, 32, 8);
    e.core.rescan_session(sid).await.unwrap();
    let mut got: Vec<String> = photos(&e.core, sid).await.into_keys().collect();
    got.sort();
    assert_eq!(got, ["c1x.jpg", "keep.jpg", "ok.jpg"]);
}

// ---------------------------------------------------------------- P1-6 watching

#[tokio::test(flavor = "multi_thread")]
async fn watching_picks_up_added_and_removed_files_within_five_seconds() {
    let e = env();
    let d = e.src.path();
    write_jpeg(&d.join("one.jpg"), 32, 32, 1);
    write_jpeg(&d.join("two.jpg"), 32, 32, 2);
    e.core.set_watch_debounce(Duration::from_millis(150));
    e.core.start_watching();
    let sid = import_dir(&e.core, d).await;
    assert_eq!(e.core.session(sid).await.unwrap().photo_count, 2);

    // a new file (and a RAW+JPEG pair) shows up
    write_jpeg(&d.join("three.jpg"), 32, 32, 3);
    write_jpeg(&d.join("sub/four.jpg"), 32, 32, 4);
    write_raw(&d.join("sub/four.cr3"), 4);
    let core = e.core.clone();
    wait_for("new files in the grid", 5, || async {
        core.session(sid).await.unwrap().photo_count == 4
            && photos(&core, sid)
                .await
                .get("four.jpg")
                .is_some_and(|p| p.companions.len() == 1)
    })
    .await;

    // a removed file is flagged missing, the rest is fine
    std::fs::remove_file(d.join("one.jpg")).unwrap();
    let core = e.core.clone();
    wait_for("removed file flagged", 5, || async {
        photos(&core, sid).await["one.jpg"].missing
    })
    .await;
    let ph = photos(&e.core, sid).await;
    assert!(!ph["two.jpg"].missing && !ph["three.jpg"].missing);
    assert_eq!(ph.len(), 4, "the row stays: ratings and edits are kept");

    // and it comes back when the file does
    write_jpeg(&d.join("one.jpg"), 32, 32, 1);
    let core = e.core.clone();
    wait_for("restored file clears the flag", 5, || async {
        !photos(&core, sid).await["one.jpg"].missing
    })
    .await;
}

// ---------------------------------------------------------------- P1-6 missing / relocate

#[tokio::test(flavor = "multi_thread")]
async fn moved_folder_shows_as_missing_and_relocates_by_relative_path() {
    let e = env();
    let parent = tempfile::tempdir().unwrap();
    let old = parent.path().join("trip");
    make_library(&old);
    let sid = import_dir(&e.core, &old).await;
    let ph = photos(&e.core, sid).await;
    e.core
        .patch_photos(PatchRequest {
            ids: vec![ph["b.jpg"].id],
            user_rating: Some(Some(3)),
            flag: None,
            color_label: None,
        })
        .await
        .unwrap();

    // the user renames the folder
    let new = parent.path().join("trip-2020");
    std::fs::rename(&old, &new).unwrap();
    e.core.rescan_session(sid).await.unwrap();
    let ph = photos(&e.core, sid).await;
    assert_eq!(ph.len(), 5, "nothing is dropped from the catalog");
    assert!(
        ph.values().all(|p| p.missing),
        "{:?}",
        ph.values().map(|p| p.missing).collect::<Vec<_>>()
    );
    assert!(ph["a.jpg"].companions[0].missing);
    assert!(e.core.original(ph["b.jpg"].id).await.is_err());

    // preview: nothing found in an unrelated folder
    let other = parent.path().join("other");
    write_jpeg(&other.join("zzz.jpg"), 32, 32, 1);
    let err = e
        .core
        .relocate_session(
            sid,
            RelocateRequest {
                path: other.to_string_lossy().into(),
                dry_run: false,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Unprocessable(_)), "{err:?}");

    // one file was changed elsewhere: it must not count as matched
    std::fs::write(new.join("a2.jpg"), b"not the same size").unwrap();
    let dry = e
        .core
        .relocate_session(
            sid,
            RelocateRequest {
                path: new.to_string_lossy().into(),
                dry_run: true,
            },
        )
        .await
        .unwrap();
    assert!(!dry.applied);
    assert_eq!(
        (dry.total, dry.matched, dry.changed, dry.absent),
        (7, 6, 1, 0)
    );
    assert!(
        photos(&e.core, sid).await["b.jpg"].missing,
        "a dry run changes nothing"
    );

    let rep = e
        .core
        .relocate_session(
            sid,
            RelocateRequest {
                path: new.to_string_lossy().into(),
                dry_run: false,
            },
        )
        .await
        .unwrap();
    assert!(rep.applied);
    let core = e.core.clone();
    wait_for("re-linked", 10, || async {
        photos(&core, sid).await.values().all(|p| !p.missing)
    })
    .await;
    let ph = photos(&e.core, sid).await;
    assert_eq!(ph["b.jpg"].user_rating, Some(3), "ratings survive");
    assert!(Path::new(&ph["b.jpg"].path).starts_with(&new));
    assert!(e.core.original(ph["b.jpg"].id).await.is_ok());
    assert_eq!(
        e.core.session(sid).await.unwrap().root_path,
        new.to_string_lossy()
    );
    assert_eq!(e.core.session(sid).await.unwrap().photo_count, 5);
}

// ---------------------------------------------------------------- P1-6 safe reject

#[tokio::test(flavor = "multi_thread")]
async fn reject_into_a_rejected_folder_moves_files_and_never_deletes() {
    let e = env();
    make_library(e.src.path());
    e.core
        .settings
        .patch(&json!({"xmp_mode": "sidecar"}))
        .unwrap();
    let d = e.src.path();
    let sid = import_dir(&e.core, d).await;
    let ph = photos(&e.core, sid).await;
    let (a, b) = (ph["a.jpg"].id, ph["b.jpg"].id);
    e.core
        .patch_photos(PatchRequest {
            ids: vec![a, b],
            user_rating: None,
            flag: Some(-1),
            color_label: None,
        })
        .await
        .unwrap();
    e.core
        .patch_photos(PatchRequest {
            ids: vec![ph["a2.jpg"].id],
            user_rating: None,
            flag: Some(1),
            color_label: None,
        })
        .await
        .unwrap();
    e.core.xmp_flush().await.unwrap();
    let before: Vec<String> = names(d);
    let sha_a = sha256(&d.join("a.jpg"));
    let sha_raw = sha256(&d.join("a.CR2"));

    // no confirmation, nothing happens
    let req = |mode: &str, confirm: bool| RejectFilesRequest {
        session_id: sid,
        ids: None,
        mode: mode.into(),
        confirm,
    };
    assert!(e.core.reject_files(req("folder", false)).await.is_err());
    assert!(e.core.reject_files(req("shred", true)).await.is_err());
    assert_eq!(names(d), before);

    let rep = e.core.reject_files(req("folder", true)).await.unwrap();
    assert_eq!((rep.photos, rep.skipped), (2, 0));
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);
    // moved, byte for byte, next to where they were; the picked photo stays
    assert_eq!(sha256(&d.join("_rejected/a.jpg")), sha_a);
    assert_eq!(sha256(&d.join("_rejected/a.CR2")), sha_raw);
    assert!(d.join("_rejected/b.jpg").is_file());
    assert!(d.join("_rejected/a.xmp").is_file() && d.join("_rejected/a.CR2.xmp").is_file());
    assert!(d.join("a2.jpg").is_file());
    assert!(!d.join("a.jpg").exists() && !d.join("a.CR2").exists());
    // nothing was deleted: same number of files, just relocated
    let after = names(d);
    assert_eq!(after.len(), before.len());
    assert!(after
        .iter()
        .all(|n| n.starts_with("_rejected/") || before.contains(n)));

    // the catalog follows: same photos, new paths, not missing
    let ph2 = photos(&e.core, sid).await;
    assert_eq!(ph2.len(), 5);
    assert!(Path::new(&ph2["a.jpg"].path).starts_with(d.join("_rejected")));
    assert!(!ph2["a.jpg"].missing && !ph2["a.jpg"].companions[0].missing);
    assert_eq!(ph2["a.jpg"].flag, -1);
    // a rescan does not bring the moved files back as new photos
    e.core.rescan_session(sid).await.unwrap();
    assert_eq!(photos(&e.core, sid).await.len(), 5);
    assert_eq!(count(&e.core, "SELECT COUNT(*) FROM photo").await, 7);
}

/// A recycle bin that moves into a folder, so the test can see that nothing is deleted.
struct FakeBin {
    dir: PathBuf,
    seen: Mutex<Vec<PathBuf>>,
}

impl Trasher for FakeBin {
    fn trash(&self, path: &Path) -> anyhow::Result<()> {
        self.seen.lock().unwrap().push(path.to_path_buf());
        let n = self.seen.lock().unwrap().len();
        std::fs::rename(
            path,
            self.dir.join(format!(
                "{n}-{}",
                path.file_name().unwrap().to_string_lossy()
            )),
        )?;
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn reject_to_the_recycle_bin_goes_through_the_trait() {
    let e = env();
    make_library(e.src.path());
    let d = e.src.path();
    let bin = Arc::new(FakeBin {
        dir: tempfile::tempdir().unwrap().keep(),
        seen: Mutex::new(Vec::new()),
    });
    e.core.set_trasher(bin.clone());
    let sid = import_dir(&e.core, d).await;
    let ph = photos(&e.core, sid).await;
    let a = ph["a.jpg"].id;
    e.core
        .patch_photos(PatchRequest {
            ids: vec![a],
            user_rating: None,
            flag: Some(-1),
            color_label: None,
        })
        .await
        .unwrap();
    // only rejected photos are ever touched, even when asked for others
    let rep = e
        .core
        .reject_files(RejectFilesRequest {
            session_id: sid,
            ids: Some(vec![a, ph["b.jpg"].id]),
            mode: "trash".into(),
            confirm: true,
        })
        .await
        .unwrap();
    assert_eq!(rep.photos, 1);
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);
    let seen: Vec<String> = bin
        .seen
        .lock()
        .unwrap()
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(seen, ["a.jpg", "a.CR2"]);
    assert!(d.join("b.jpg").is_file());
    assert_eq!(
        std::fs::read_dir(&bin.dir).unwrap().count(),
        2,
        "moved to the bin, not deleted"
    );
    // the catalog keeps the rows and shows them as missing; restoring + rescan brings them back
    let ph = photos(&e.core, sid).await;
    assert!(ph["a.jpg"].missing && ph["a.jpg"].companions[0].missing);
    assert_eq!(ph.len(), 5);
    for f in std::fs::read_dir(&bin.dir).unwrap().flatten() {
        let name = f.file_name().to_string_lossy().into_owned();
        std::fs::rename(f.path(), d.join(name.split_once('-').unwrap().1)).unwrap();
    }
    e.core.rescan_session(sid).await.unwrap();
    let ph = photos(&e.core, sid).await;
    assert!(!ph["a.jpg"].missing && !ph["a.jpg"].companions[0].missing);
    assert_eq!(ph["a.jpg"].flag, -1);
}
