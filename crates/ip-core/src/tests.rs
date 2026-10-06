//! End-to-end tests of the core with `FakeImaging`.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast::Receiver;

use crate::testutil::{make_photos, write_jpeg, FakeImaging};
use crate::*;

const WAIT: Duration = Duration::from_secs(30);

struct Env {
    core: Arc<Core>,
    imaging: Arc<FakeImaging>,
    _data: tempfile::TempDir,
    src: tempfile::TempDir,
}

fn env() -> Env {
    let data = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let imaging = Arc::new(FakeImaging::new());
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: imaging.clone(),
        thumb_workers: Some(3),
        worker: None,
        renderer: None,
        force_cpu: false,
    })
    .unwrap();
    Env {
        core,
        imaging,
        _data: data,
        src,
    }
}

async fn import(env: &Env, dir: &Path) -> Session {
    let s = env
        .core
        .import(ImportRequest {
            path: dir.to_string_lossy().into_owned(),
            recursive: true,
            title: None,
        })
        .await
        .unwrap();
    env.core.wait_session_ready(s.id, WAIT).await.unwrap()
}

fn drain(rx: &mut Receiver<Event>) -> Vec<Event> {
    let mut v = Vec::new();
    while let Ok(e) = rx.try_recv() {
        v.push(e);
    }
    v
}

fn q(session_id: i64) -> PhotoQuery {
    PhotoQuery {
        session_id,
        ..Default::default()
    }
}

#[tokio::test]
async fn import_pipeline_end_to_end() {
    let env = env();
    let mut rx = env.core.events.subscribe();
    make_photos(env.src.path(), 25);
    write_jpeg(&env.src.path().join("sub").join("nested.jpg"), 100, 50, 3);
    std::fs::write(env.src.path().join("notes.txt"), "not an image").unwrap();
    std::fs::write(env.src.path().join("corrupt.jpg"), "garbage").unwrap();

    let first = env
        .core
        .import(ImportRequest {
            path: env.src.path().to_string_lossy().into_owned(),
            recursive: true,
            title: Some("trip".into()),
        })
        .await
        .unwrap();
    // The session exists immediately, before scanning finished.
    assert_eq!(first.title, "trip");
    assert_eq!(first.import_state, ImportState::Scanning);
    let s = env.core.wait_session_ready(first.id, WAIT).await.unwrap();
    assert_eq!(s.photo_count, 27); // 25 + nested + corrupt, not notes.txt
    assert!(s.cover_photo_id.is_some());

    let page = env.core.photos(q(s.id)).await.unwrap();
    assert_eq!(page.total, 27);
    let good: Vec<_> = page
        .photos
        .iter()
        .filter(|p| p.file_name != "corrupt.jpg")
        .collect();
    assert_eq!(good.len(), 26);
    assert!(
        good.iter().all(|p| p.thumb_ready),
        "all decodable photos have grid thumbs"
    );
    let corrupt = page
        .photos
        .iter()
        .find(|p| p.file_name == "corrupt.jpg")
        .unwrap();
    assert!(!corrupt.thumb_ready);
    assert!(corrupt.width.is_none());

    let p0 = page
        .photos
        .iter()
        .find(|p| p.file_name == "img_000.jpg")
        .unwrap();
    assert_eq!((p0.width, p0.height), (Some(640), Some(480)));
    assert_eq!(p0.format, "jpeg");
    assert_eq!(p0.camera.as_deref(), Some("FAKE Cam 1"));
    assert_eq!(p0.session_id, s.id);
    assert!(p0.taken_at.is_some());
    assert_eq!(p0.thumb_version.len(), 8);

    // thumbnails really are on disk and small
    let path = env.core.thumb_path(p0.id, 256).await.unwrap();
    let img = image::open(&path).unwrap();
    assert!(img.width().max(img.height()) <= 256);

    // events
    let evs = drain(&mut rx);
    let added: i64 = evs
        .iter()
        .filter_map(|e| match e {
            Event::PhotosAdded { count, .. } => Some(*count),
            _ => None,
        })
        .sum();
    assert!(added >= 27);
    let thumbs: HashSet<i64> = evs
        .iter()
        .flat_map(|e| match e {
            Event::ThumbsReady { items } => items.iter().map(|i| i.id).collect(),
            _ => vec![],
        })
        .collect();
    assert_eq!(thumbs.len(), 26);
    assert!(matches!(evs.first(), Some(Event::SessionUpdated { .. })));
    let last_session = evs
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::SessionUpdated { session } => Some(session.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(last_session.import_state, ImportState::Ready);
}

#[tokio::test]
async fn reimport_does_not_duplicate_and_delete_keeps_originals() {
    let env = env();
    let files = make_photos(env.src.path(), 5);
    let s1 = import(&env, env.src.path()).await;
    let s2 = import(&env, env.src.path()).await;
    assert_ne!(s1.id, s2.id);
    assert_eq!(s1.photo_count, 5);
    assert_eq!(s2.photo_count, 5);
    let a = env.core.photos(q(s1.id)).await.unwrap();
    let b = env.core.photos(q(s2.id)).await.unwrap();
    let ids_a: Vec<_> = a.photos.iter().map(|p| p.id).collect();
    let ids_b: Vec<_> = b.photos.iter().map(|p| p.id).collect();
    assert_eq!(ids_a, ids_b, "same files map to the same photo rows");
    assert_eq!(env.core.sessions().await.unwrap().len(), 2);
    assert_eq!(
        env.core.sessions().await.unwrap()[0].id,
        s2.id,
        "newest first"
    );

    // deleting one session keeps shared photos
    env.core.delete_session(s1.id).await.unwrap();
    assert_eq!(env.core.photos(q(s2.id)).await.unwrap().total, 5);
    // deleting the last one removes rows and cache, never originals
    let thumb = env.core.thumb_path(ids_b[0], 256).await.unwrap();
    assert!(thumb.exists());
    env.core.delete_session(s2.id).await.unwrap();
    assert!(!thumb.exists());
    assert!(files.iter().all(|f| f.exists()));
    assert!(matches!(
        env.core.photo(ids_b[0]).await,
        Err(CoreError::NotFound(_))
    ));
    assert!(matches!(
        env.core.delete_session(s2.id).await,
        Err(CoreError::NotFound(_))
    ));
}

#[tokio::test]
async fn filters_sorting_and_cursor_pagination() {
    let env = env();
    make_photos(env.src.path(), 23);
    let s = import(&env, env.src.path()).await;
    let all = env.core.photos(q(s.id)).await.unwrap();
    assert_eq!(all.total, 23);
    // default sort: taken_at ascending (mtime grows with index)
    let names: Vec<_> = all.photos.iter().map(|p| p.file_name.clone()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
    let ids: Vec<i64> = all.photos.iter().map(|p| p.id).collect();

    // ratings: ids[0..3] -> 5,4,4 ; ids[3..6] -> 1 ; ids[6] -> 0 ; flags and colours
    let patch = |ids: &[i64], r: Option<Option<i64>>, f: Option<i64>, c: Option<Option<&str>>| {
        PatchRequest {
            ids: ids.to_vec(),
            user_rating: r,
            flag: f,
            color_label: c.map(|c| c.map(String::from)),
        }
    };
    env.core
        .patch_photos(patch(&ids[0..1], Some(Some(5)), Some(1), Some(Some("red"))))
        .await
        .unwrap();
    env.core
        .patch_photos(patch(&ids[1..3], Some(Some(4)), None, Some(Some("blue"))))
        .await
        .unwrap();
    env.core
        .patch_photos(patch(&ids[3..6], Some(Some(1)), Some(-1), None))
        .await
        .unwrap();
    env.core
        .patch_photos(patch(&ids[6..7], Some(Some(0)), None, None))
        .await
        .unwrap();

    let count = |f: PhotoQuery| {
        let core = env.core.clone();
        async move { core.photos(f).await.unwrap().total }
    };
    let mut f = q(s.id);
    f.rating_gte = Some(4);
    assert_eq!(count(f.clone()).await, 3);
    f.rating_gte = Some(1);
    assert_eq!(count(f.clone()).await, 6);
    f.rating_gte = Some(0);
    assert_eq!(count(f.clone()).await, 23, "N=0 keeps unrated photos");
    let mut f = q(s.id);
    f.flag = FlagFilter::Picked;
    assert_eq!(count(f.clone()).await, 1);
    f.flag = FlagFilter::Rejected;
    assert_eq!(count(f.clone()).await, 3);
    f.flag = FlagFilter::Unflagged;
    assert_eq!(count(f.clone()).await, 19);
    f.flag = FlagFilter::NotRejected;
    assert_eq!(count(f.clone()).await, 20);
    let mut f = q(s.id);
    f.color_label = Some("blue".into());
    assert_eq!(count(f.clone()).await, 2);
    f.rating_gte = Some(5);
    assert_eq!(count(f).await, 0);

    // session stats
    let s = env.core.session(s.id).await.unwrap();
    assert_eq!((s.picked_count, s.rejected_count, s.rated_count), (1, 3, 7));

    // pagination for every sort order: pages are disjoint, complete and consistently ordered
    for sort in [
        SortKey::TakenAt,
        SortKey::TakenAtDesc,
        SortKey::Name,
        SortKey::Rating,
    ] {
        let mut cursor = None;
        let mut seen: Vec<Photo> = Vec::new();
        let mut pages = 0;
        loop {
            let page = env
                .core
                .photos(PhotoQuery {
                    session_id: s.id,
                    sort,
                    limit: Some(7),
                    cursor: cursor.clone(),
                    ..Default::default()
                })
                .await
                .unwrap();
            assert_eq!(page.total, 23);
            seen.extend(page.photos);
            pages += 1;
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(pages, 4, "{sort:?}");
        let uniq: HashSet<i64> = seen.iter().map(|p| p.id).collect();
        assert_eq!(uniq.len(), 23, "{sort:?} has duplicates or gaps");
        match sort {
            SortKey::TakenAt => assert!(seen.windows(2).all(|w| w[0].taken_at <= w[1].taken_at)),
            SortKey::TakenAtDesc => {
                assert!(seen.windows(2).all(|w| w[0].taken_at >= w[1].taken_at))
            }
            SortKey::Name => assert!(seen.windows(2).all(|w| w[0].file_name <= w[1].file_name)),
            SortKey::Rating => {
                let r = |p: &Photo| p.user_rating.unwrap_or(-1);
                assert!(seen.windows(2).all(|w| r(&w[0]) >= r(&w[1])));
                assert_eq!(seen[0].user_rating, Some(5));
            }
            SortKey::Ai => {}
        }
    }

    // bad inputs
    let mut bad = q(s.id);
    bad.cursor = Some("zz".into());
    assert!(matches!(
        env.core.photos(bad).await,
        Err(CoreError::BadRequest(_))
    ));
    assert!(matches!(
        env.core.photos(q(9999)).await,
        Err(CoreError::NotFound(_))
    ));
    // limit is clamped
    let mut big = q(s.id);
    big.limit = Some(1_000_000);
    assert_eq!(env.core.photos(big).await.unwrap().photos.len(), 23);
}

#[tokio::test]
async fn patch_semantics_and_events() {
    let env = env();
    make_photos(env.src.path(), 3);
    let s = import(&env, env.src.path()).await;
    let ids: Vec<i64> = env
        .core
        .photos(q(s.id))
        .await
        .unwrap()
        .photos
        .iter()
        .map(|p| p.id)
        .collect();
    let mut rx = env.core.events.subscribe();

    let n = env
        .core
        .patch_photos(PatchRequest {
            ids: vec![ids[0], 424242],
            user_rating: Some(Some(3)),
            flag: Some(1),
            color_label: Some(Some("green".into())),
        })
        .await
        .unwrap();
    assert_eq!(n, 1, "unknown ids are ignored");
    match drain(&mut rx).as_slice() {
        [Event::PhotosUpdated { items }] => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].id, ids[0]);
            assert_eq!(items[0].user_rating, Some(3));
            assert_eq!(items[0].flag, 1);
            assert_eq!(items[0].color_label.as_deref(), Some("green"));
        }
        other => panic!("unexpected events {other:?}"),
    }

    // absent field = untouched, explicit null = cleared
    let body: PatchRequest =
        serde_json::from_str(&format!(r#"{{"ids":[{}],"user_rating":null}}"#, ids[0])).unwrap();
    env.core.patch_photos(body).await.unwrap();
    let p = env.core.photo(ids[0]).await.unwrap();
    assert_eq!(p.user_rating, None);
    assert_eq!(p.flag, 1);
    assert_eq!(p.color_label.as_deref(), Some("green"));
    let body: PatchRequest =
        serde_json::from_str(&format!(r#"{{"ids":[{}],"color_label":null}}"#, ids[0])).unwrap();
    env.core.patch_photos(body).await.unwrap();
    assert_eq!(env.core.photo(ids[0]).await.unwrap().color_label, None);

    for bad in [
        r#"{"ids":[1],"user_rating":6}"#,
        r#"{"ids":[1],"flag":2}"#,
        r#"{"ids":[1],"color_label":"pink"}"#,
        r#"{"ids":[1]}"#,
    ] {
        let req: PatchRequest = serde_json::from_str(bad).unwrap();
        assert!(
            matches!(
                env.core.patch_photos(req).await,
                Err(CoreError::BadRequest(_))
            ),
            "{bad}"
        );
    }
}

#[tokio::test]
async fn on_demand_thumbs_are_deduplicated() {
    let env = env();
    make_photos(env.src.path(), 2);
    let s = import(&env, env.src.path()).await;
    let ids: Vec<i64> = env
        .core
        .photos(q(s.id))
        .await
        .unwrap()
        .photos
        .iter()
        .map(|p| p.id)
        .collect();

    let before = env
        .imaging
        .thumbnails_generated
        .load(std::sync::atomic::Ordering::SeqCst);
    let mut tasks = Vec::new();
    for _ in 0..16 {
        let core = env.core.clone();
        let id = ids[0];
        tasks.push(tokio::spawn(async move {
            core.thumb_path(id, 512).await.unwrap()
        }));
    }
    let paths: HashSet<_> = futures_collect(tasks).await;
    assert_eq!(paths.len(), 1);
    let after = env
        .imaging
        .thumbnails_generated
        .load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        after - before,
        1,
        "16 concurrent requests share one generation"
    );

    // previews, and cache-miss regeneration after the file vanished
    let prev = env.core.preview_path(ids[1], 1024).await.unwrap();
    assert!(prev.exists());
    let grid = env.core.thumb_path(ids[1], 256).await.unwrap();
    std::fs::remove_file(&grid).unwrap();
    assert!(env.core.thumb_path(ids[1], 256).await.unwrap().exists());

    assert!(matches!(
        env.core.thumb_path(ids[0], 300).await,
        Err(CoreError::BadRequest(_))
    ));
    assert!(matches!(
        env.core.preview_path(ids[0], 256).await,
        Err(CoreError::BadRequest(_))
    ));
    assert!(matches!(
        env.core.thumb_path(99999, 256).await,
        Err(CoreError::NotFound(_))
    ));
}

async fn futures_collect(
    tasks: Vec<tokio::task::JoinHandle<std::path::PathBuf>>,
) -> HashSet<std::path::PathBuf> {
    let mut out = HashSet::new();
    for t in tasks {
        out.insert(t.await.unwrap());
    }
    out
}

#[tokio::test]
async fn viewport_requeues_missing_thumbnails() {
    let env = env();
    make_photos(env.src.path(), 4);
    let s = import(&env, env.src.path()).await;
    let p = env.core.photos(q(s.id)).await.unwrap().photos[2].clone();
    // simulate a lost cache + state
    let path = env.core.thumb_path(p.id, 256).await.unwrap();
    std::fs::remove_file(&path).unwrap();
    env.core
        .db
        .call(move |c| {
            c.execute("UPDATE photo SET thumb_state=0 WHERE id=?1", [p.id])?;
            Ok(())
        })
        .await
        .unwrap();
    let mut rx = env.core.events.subscribe();
    env.core.viewport(vec![p.id]).await.unwrap();
    let ev = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(Event::ThumbsReady { items }) = rx.recv().await {
                break items;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(ev[0].id, p.id);
    assert!(path.exists());
    assert!(env.core.photo(p.id).await.unwrap().thumb_ready);
}

#[tokio::test]
async fn export_copies_resizes_and_never_overwrites() {
    let env = env();
    let files = make_photos(env.src.path(), 3);
    let s = import(&env, env.src.path()).await;
    let photos = env.core.photos(q(s.id)).await.unwrap().photos;
    let ids: Vec<i64> = photos.iter().map(|p| p.id).collect();
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("export");

    async fn run(env: &Env, req: ExportRequest) -> (String, Vec<Event>) {
        let mut rx = env.core.events.subscribe();
        let id = env.core.export(req).await.unwrap();
        let mut evs = Vec::new();
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let e = rx.recv().await.unwrap();
                if let Event::TaskProgress { task_id, state, .. } = &e {
                    if *task_id == id && state != "running" {
                        evs.push(e);
                        break;
                    }
                    if *task_id == id {
                        evs.push(e);
                    }
                }
            }
        })
        .await
        .unwrap();
        (id, evs)
    }

    let req = |long_edge: Option<u32>, tpl: &str| ExportRequest {
        ids: ids.clone(),
        dest: dest.to_string_lossy().into_owned(),
        long_edge,
        quality: 85,
        name_template: tpl.into(),
        apply_edits: true,
        upscale: None,
        strip_gps: false,
        folders: None,
    };

    // 1. originals
    let (id1, evs) = run(&env, req(None, "{name}")).await;
    assert_eq!(id1, "export-1");
    match evs.last().unwrap() {
        Event::TaskProgress {
            done,
            total,
            state,
            error,
            ..
        } => {
            assert_eq!((*done, *total, state.as_str()), (3, 3, "done"));
            assert!(error.is_none());
        }
        _ => unreachable!(),
    }
    for f in &files {
        let copied = dest.join(f.file_name().unwrap());
        assert_eq!(std::fs::read(&copied).unwrap(), std::fs::read(f).unwrap());
    }

    // 2. same names again -> suffixes, nothing overwritten
    let (_, _) = run(&env, req(None, "{name}")).await;
    for f in &files {
        let stem = f.file_stem().unwrap().to_string_lossy();
        assert!(dest.join(format!("{stem}_1.jpg")).exists());
    }

    // 3. resized with template
    let (id3, evs) = run(&env, req(Some(100), "{date}_{seq}")).await;
    assert_eq!(id3, "export-3");
    assert!(matches!(evs.last().unwrap(), Event::TaskProgress { state, .. } if state == "done"));
    let mut resized: Vec<_> = std::fs::read_dir(&dest)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("2023"))
        .collect();
    resized.sort();
    assert_eq!(resized.len(), 3);
    assert!(resized[0].ends_with("_0001.jpg"), "{resized:?}");
    let img = image::open(dest.join(&resized[0])).unwrap();
    assert!(img.width().max(img.height()) <= 100);

    // 4. validation
    let mut bad = req(None, "{name}");
    bad.ids.clear();
    assert!(matches!(
        env.core.export(bad).await,
        Err(CoreError::BadRequest(_))
    ));
    let mut bad = req(Some(0), "{name}");
    bad.quality = 90;
    assert!(matches!(
        env.core.export(bad).await,
        Err(CoreError::BadRequest(_))
    ));
}
