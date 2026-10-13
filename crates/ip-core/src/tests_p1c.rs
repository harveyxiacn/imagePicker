//! P1-1 (event layer) and P1-3 (duplicate detection) through the whole pipeline with
//! `FakeImaging` + `FakeWorker`; the SQL rules themselves are tested in `analysis::layers`.
//! All images are synthetic.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::analysis::*;
use crate::fake_worker::{FakeSpec, FakeWorker};
use crate::testutil::{write_jpeg, FakeImaging};
use crate::*;

/// 2023-11-14T22:13:20Z
const BASE: u64 = 1_700_000_000;
const MIN: u64 = 60;
const H: u64 = 3_600;

struct Env {
    core: Arc<Core>,
    worker: Arc<FakeWorker>,
    _data: tempfile::TempDir,
    one: tempfile::TempDir,
    two: tempfile::TempDir,
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
    Env {
        core,
        worker,
        _data: data,
        one: tempfile::tempdir().unwrap(),
        two: tempfile::tempdir().unwrap(),
    }
}

/// A 320x240 JPEG whose bytes depend on `shade`, taken `t_s` seconds after BASE (FakeImaging
/// reads the capture time from the mtime).
fn photo(dir: &Path, name: &str, t_s: u64, shade: u8) {
    let p = dir.join(name);
    write_jpeg(&p, 320, 240, shade);
    let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
    f.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(BASE + t_s))
        .unwrap();
}

async fn import(e: &Env, dir: &Path, title: &str) -> i64 {
    let s = e
        .core
        .import(ImportRequest {
            path: dir.to_string_lossy().into_owned(),
            recursive: true,
            title: Some(title.into()),
        })
        .await
        .unwrap();
    e.core
        .wait_session_ready(s.id, Duration::from_secs(60))
        .await
        .unwrap();
    s.id
}

async fn analyse(e: &Env, sid: i64) {
    e.core
        .analysis_run(AnalysisRunRequest {
            session_id: sid,
            profile: Profile::Standard,
            photo_ids: None,
            force: false,
            allow_download: false,
            fallback_lite: false,
        })
        .await
        .unwrap();
    // wall-clock deadline, not a fixed number of polls
    let t = Instant::now();
    loop {
        let s = e.core.analysis_status(sid);
        if s.state != RunState::Running {
            assert_eq!(s.state, RunState::Done, "{s:?}");
            return;
        }
        assert!(
            t.elapsed() < Duration::from_secs(300),
            "analysis did not finish"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn page(e: &Env, q: PhotoQuery) -> Vec<Photo> {
    e.core
        .photos(PhotoQuery {
            limit: Some(5000),
            ..q
        })
        .await
        .unwrap()
        .photos
}

fn sorted_names(photos: &[Photo]) -> Vec<String> {
    let mut v: Vec<String> = photos.iter().map(|p| p.file_name.clone()).collect();
    v.sort();
    v
}

/// ```text
/// event 1  22:13-22:43   scene A: a1 a2 (a burst)      scene B: b1, 30 min later
/// event 2  01:43         scene C: geoA_c1              (3 h after B: a pause > 2 h)
/// event 3  01:58-02:13   scene D: geoB_d1 (11 km from C: a move > 5 km)   scene E: e1, no GPS
/// ```
fn build_day(e: &Env) {
    let d = e.one.path();
    photo(d, "a1.jpg", 0, 10);
    photo(d, "a2.jpg", 1, 20);
    photo(d, "b1.jpg", 30 * MIN, 30);
    photo(d, "geoA_c1.jpg", 30 * MIN + 3 * H, 40);
    photo(d, "geoB_d1.jpg", 30 * MIN + 3 * H + 15 * MIN, 50);
    photo(d, "e1.jpg", 30 * MIN + 3 * H + 30 * MIN, 60);
    for (n, angle) in [
        ("a1.jpg", 0.0),
        ("a2.jpg", 1.0),
        ("b1.jpg", 90.0),
        ("geoA_c1.jpg", 180.0),
        ("geoB_d1.jpg", 270.0),
        ("e1.jpg", 271.0),
    ] {
        e.worker.set(n, FakeSpec::at(angle));
    }
}

#[tokio::test]
async fn events_group_scenes_by_pause_and_distance() {
    let e = env();
    build_day(&e);
    let sid = import(&e, e.one.path(), "Day").await;
    analyse(&e, sid).await;

    let t = e.core.timeline(sid).await.unwrap();
    let shape: Vec<(Vec<usize>, i64)> = t
        .events
        .iter()
        .map(|ev| {
            (
                ev.scenes.iter().map(|s| s.bursts.len()).collect(),
                ev.photo_count,
            )
        })
        .collect();
    assert_eq!(
        shape,
        vec![(vec![1, 1], 3), (vec![1], 1), (vec![1, 1], 2)],
        "{t:#?}"
    );
    let names: Vec<&str> = t.events.iter().map(|ev| ev.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "2023-11-14 22:13-22:43",
            "2023-11-15 01:43",
            "2023-11-15 01:58-02:13"
        ]
    );
    // the event's position: only where photos carry GPS
    assert!(t.events[0].gps_lat.is_none());
    assert!((t.events[1].gps_lat.unwrap() - 35.6762).abs() < 1e-6);
    assert!(t.events[2].gps_lon.is_some());
    // events -> scenes -> bursts agree with GET /groups
    let groups = e.core.groups(sid).await.unwrap();
    let mut from_events: Vec<i64> = t
        .events
        .iter()
        .flat_map(|ev| ev.scenes.iter().map(|s| s.id))
        .collect();
    let mut from_groups: Vec<i64> = groups.scenes.iter().map(|s| s.id).collect();
    from_events.sort_unstable();
    from_groups.sort_unstable();
    assert_eq!(from_events, from_groups);
    assert!(
        groups.scenes.iter().all(|s| s.event_id.is_none()),
        "/groups keeps its shape"
    );

    // browse by event
    for (ev, want) in t.events.iter().zip([
        vec!["a1.jpg", "a2.jpg", "b1.jpg"],
        vec!["geoA_c1.jpg"],
        vec!["e1.jpg", "geoB_d1.jpg"],
    ]) {
        let ph = page(
            &e,
            PhotoQuery {
                session_id: sid,
                event_id: Some(ev.id),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(sorted_names(&ph), want, "{}", ev.name);
    }
    assert!(page(
        &e,
        PhotoQuery {
            session_id: sid,
            event_id: Some(999_999),
            ..Default::default()
        }
    )
    .await
    .is_empty());
}

#[tokio::test]
async fn a_manual_merge_recomputes_the_events() {
    let e = env();
    build_day(&e);
    let sid = import(&e, e.one.path(), "Day").await;
    analyse(&e, sid).await;
    let before = e.core.timeline(sid).await.unwrap();
    assert_eq!(before.events.len(), 3);
    let burst_of = |t: &TimelineOut, ev: usize| t.events[ev].scenes[0].bursts[0].id;
    // pull the lone burst of event 2 into the first scene of event 1: its scene is gone and the
    // merged scene spans the pause, so the first two events become one
    let merged = e
        .core
        .merge_bursts(MergeBurstsRequest {
            burst_ids: vec![burst_of(&before, 0), burst_of(&before, 1)],
        })
        .await
        .unwrap();
    let after = e.core.timeline(sid).await.unwrap();
    let groups = e.core.groups(sid).await.unwrap();
    // every scene belongs to exactly one event, and nothing was lost
    let scene_ids: Vec<i64> = after
        .events
        .iter()
        .flat_map(|ev| ev.scenes.iter().map(|s| s.id))
        .collect();
    assert_eq!(scene_ids.len(), groups.scenes.len());
    assert!(after
        .events
        .iter()
        .all(|ev| ev.scenes.iter().all(|s| s.event_id == Some(ev.id))));
    assert_eq!(after.events.iter().map(|ev| ev.photo_count).sum::<i64>(), 6);
    let merged_event = after
        .events
        .iter()
        .find(|ev| {
            ev.scenes
                .iter()
                .any(|s| s.bursts.iter().any(|b| b.id == merged))
        })
        .unwrap();
    assert!(
        merged_event.end_at - merged_event.start_at >= 3 * 3_600_000,
        "{merged_event:#?}"
    );
    // the merged scene (a1 a2 + c1) ends in C's GPS position, 11 km from D: the move still cuts
    assert_eq!(after.events.len(), 2);
}

#[tokio::test]
async fn timelines_of_unknown_sessions_are_not_found() {
    let e = env();
    assert!(matches!(
        e.core.timeline(12345).await,
        Err(CoreError::NotFound(_))
    ));
    assert!(matches!(
        e.core.duplicates_of(12345).await,
        Err(CoreError::NotFound(_))
    ));
}

const P: u64 = 0x0F0F_3C3C_A5A5_5A5A;

fn spec_hash(e: &Env, name: &str, phash: u64) {
    e.worker.set(
        name,
        FakeSpec {
            phash,
            ..FakeSpec::at(0.0)
        },
    );
}

#[tokio::test]
async fn duplicates_exact_across_sessions_and_stripped_copies() {
    let e = env();
    // session one: the original, a burst mate with EXIF and an unrelated photo
    photo(e.one.path(), "orig.jpg", 0, 10);
    photo(e.one.path(), "burst_mate.jpg", 5, 12);
    photo(e.one.path(), "unique.jpg", 10 * MIN, 50);
    // session two: a byte-identical copy, the same picture saved again without metadata
    // (different bytes, same pixels as far as the hash can tell), and a different picture
    // without metadata that is 5 bits away
    std::fs::copy(e.one.path().join("orig.jpg"), e.two.path().join("copy.jpg")).unwrap();
    photo(e.two.path(), "noexif_orig.jpg", 20, 11);
    photo(e.two.path(), "noexif_other.jpg", 30, 90);
    spec_hash(&e, "orig.jpg", P);
    spec_hash(&e, "copy.jpg", P);
    spec_hash(&e, "noexif_orig.jpg", P ^ 0b11); // 2 bits
    spec_hash(&e, "burst_mate.jpg", P ^ (0b11111 << 40)); // it has EXIF: not a copy
    spec_hash(&e, "noexif_other.jpg", P ^ (0b11111 << 8)); // 5 bits
    spec_hash(&e, "unique.jpg", !P);

    let s1 = import(&e, e.one.path(), "One").await;
    let s2 = import(&e, e.two.path(), "Two").await;
    let dups = |sid| PhotoQuery {
        session_id: sid,
        duplicates: Some(true),
        ..Default::default()
    };

    // exact duplicates are known right after the import, before any analysis
    let d1 = page(&e, dups(s1)).await;
    let d2 = page(&e, dups(s2)).await;
    assert_eq!(sorted_names(&d1), ["orig.jpg"]);
    assert_eq!(sorted_names(&d2), ["copy.jpg"]);
    assert_eq!(
        d1[0].duplicate_group_id,
        Some(d1[0].id),
        "smallest id names the group"
    );
    assert_eq!(d2[0].duplicate_group_id, d1[0].duplicate_group_id);
    assert_eq!(d2[0].duplicate_kind.as_deref(), Some("exact"));

    // the hashes of the analysis add the re-saved copy; a second run changes nothing
    analyse(&e, s1).await;
    analyse(&e, s2).await;
    let d2 = page(&e, dups(s2)).await;
    assert_eq!(sorted_names(&d2), ["copy.jpg", "noexif_orig.jpg"]);
    let near = d2
        .iter()
        .find(|p| p.file_name == "noexif_orig.jpg")
        .unwrap();
    assert_eq!(near.duplicate_kind.as_deref(), Some("near"));
    assert_eq!(near.duplicate_group_id, d1[0].duplicate_group_id);
    assert_eq!(sorted_names(&page(&e, dups(s1)).await), ["orig.jpg"]);

    // everything else is no duplicate
    let rest = page(
        &e,
        PhotoQuery {
            session_id: s2,
            duplicates: Some(false),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(sorted_names(&rest), ["noexif_other.jpg"]);
    let rest = page(
        &e,
        PhotoQuery {
            session_id: s1,
            duplicates: Some(false),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(sorted_names(&rest), ["burst_mate.jpg", "unique.jpg"]);
    assert!(rest
        .iter()
        .all(|p| p.duplicate_group_id.is_none() && p.duplicate_kind.is_none()));

    // the group, catalog wide
    let out = e.core.duplicates_of(near.id).await.unwrap();
    assert_eq!(out.group_id, d1[0].duplicate_group_id);
    assert_eq!(out.photo_ids.len(), 3);
    assert_eq!(out.exact_ids.len(), 2);
    let none = e.core.duplicates_of(rest[0].id).await.unwrap();
    assert!(none.group_id.is_none() && none.photo_ids.is_empty());

    // one query decorates a single photo as well
    let one = e.core.photo(near.id).await.unwrap();
    assert_eq!(one.duplicate_kind.as_deref(), Some("near"));
    let v = serde_json::to_value(&one).unwrap();
    assert_eq!(v["duplicate_group_id"], out.group_id.unwrap());
    let v = serde_json::to_value(&rest[0]).unwrap();
    assert!(v["duplicate_group_id"].is_null());
    assert!(v.get("duplicate_kind").is_none());
}

#[tokio::test]
async fn duplicates_is_a_built_in_smart_collection() {
    let e = env();
    let c = e.core.collections().await.unwrap();
    let d = c.iter().find(|c| c.id == "duplicates").expect("built-in");
    assert_eq!((d.query.as_str(), d.builtin), ("duplicates=1", true));
}
