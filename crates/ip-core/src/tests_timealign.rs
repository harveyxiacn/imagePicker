//! P1-2 (multi-device time alignment) through the whole pipeline with `FakeImaging` +
//! `FakeWorker`: the estimate, accepting / typing / resetting an offset, and that grouping and
//! events use the corrected time while the stored EXIF time stays as it was. The estimator itself
//! is tested on synthetic data in `analysis::time_align`. All images are synthetic.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::analysis::time_align::*;
use crate::analysis::*;
use crate::fake_worker::{FakeSpec, FakeWorker};
use crate::testutil::{write_jpeg, FakeImaging};
use crate::*;

/// 2023-11-14T22:13:20Z
const BASE: u64 = 1_700_000_000;
/// The phone's clock runs 5 h 3 min fast.
const ERR_S: u64 = 5 * 3_600 + 3 * 60;
const SCENES: u64 = 12;

struct Env {
    core: Arc<Core>,
    worker: Arc<FakeWorker>,
    _data: tempfile::TempDir,
    dir: tempfile::TempDir,
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
        dir: tempfile::tempdir().unwrap(),
    }
}

/// `name` taken `t_s` seconds after BASE by the file clock (FakeImaging reads the mtime).
fn photo(dir: &Path, name: &str, t_s: u64, shade: u8) {
    let p = dir.join(name);
    write_jpeg(&p, 320, 240, shade);
    let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
    f.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(BASE + t_s))
        .unwrap();
}

/// A camera (`cam_*`, 13 photos) and a phone (`phone_*`, 12 photos) shoot the same twelve scenes
/// ten minutes apart (the scene angle is the embedding); the phone's clock is `ERR_S` fast and
/// its shots lag the camera's by 0-2 s.
fn build(e: &Env) {
    for k in 0..SCENES {
        let angle = 30.0 * k as f32;
        let at = 600 * k;
        let (cam, phone) = (format!("cam_{k:02}.jpg"), format!("phone_{k:02}.jpg"));
        photo(e.dir.path(), &cam, at, (k * 10) as u8);
        photo(e.dir.path(), &phone, at + k % 3 + ERR_S, (k * 10 + 5) as u8);
        e.worker.set(&cam, FakeSpec::at(angle));
        e.worker.set(&phone, FakeSpec::at(angle));
    }
    // a second camera shot of scene 0, a second apart
    photo(e.dir.path(), "cam_00b.jpg", 1, 3);
    e.worker.set("cam_00b.jpg", FakeSpec::at(0.0));
}

async fn import(e: &Env) -> i64 {
    let s = e
        .core
        .import(ImportRequest {
            path: e.dir.path().to_string_lossy().into_owned(),
            recursive: true,
            title: Some("Two devices".into()),
        })
        .await
        .unwrap();
    e.core
        .wait_session_ready(s.id, Duration::from_secs(60))
        .await
        .unwrap();
    s.id
}

async fn analyse(e: &Env, sid: i64, force: bool) {
    e.core
        .analysis_run(AnalysisRunRequest {
            session_id: sid,
            profile: Profile::Standard,
            photo_ids: None,
            force,
            allow_download: false,
            fallback_lite: false,
        })
        .await
        .unwrap();
    let t = Instant::now();
    loop {
        let s = e.core.analysis_status(sid);
        if s.state != RunState::Running {
            assert_eq!(s.state, RunState::Done, "{s:?}");
            return;
        }
        assert!(t.elapsed() < Duration::from_secs(300), "analysis timed out");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn setup() -> (Env, i64) {
    let e = env();
    build(&e);
    let sid = import(&e).await;
    analyse(&e, sid, false).await;
    (e, sid)
}

async fn times(e: &Env, sid: i64) -> Vec<(String, Option<i64>)> {
    let mut v: Vec<_> = e
        .core
        .photos(PhotoQuery {
            session_id: sid,
            limit: Some(500),
            ..Default::default()
        })
        .await
        .unwrap()
        .photos
        .into_iter()
        .map(|p| (p.file_name, p.taken_at))
        .collect();
    v.sort();
    v
}

async fn bursts(e: &Env, sid: i64) -> Vec<Vec<String>> {
    let names: std::collections::HashMap<i64, String> = e
        .core
        .photos(PhotoQuery {
            session_id: sid,
            limit: Some(500),
            ..Default::default()
        })
        .await
        .unwrap()
        .photos
        .into_iter()
        .map(|p| (p.id, p.file_name))
        .collect();
    let mut out: Vec<Vec<String>> = e
        .core
        .groups(sid)
        .await
        .unwrap()
        .scenes
        .iter()
        .flat_map(|s| &s.bursts)
        .map(|b| {
            let mut v: Vec<String> = b.photo_ids.iter().map(|i| names[i].clone()).collect();
            v.sort();
            v
        })
        .collect();
    out.sort();
    out
}

fn device<'a>(o: &'a DeviceOffsetsOut, prefix: &str) -> &'a DeviceOffset {
    o.devices
        .iter()
        .find(|d| d.name.starts_with(prefix))
        .unwrap_or_else(|| panic!("no device {prefix} in {o:?}"))
}

fn req(action: OffsetAction, device_id: Option<i64>, offset_ms: Option<i64>) -> OffsetsRequest {
    OffsetsRequest {
        action,
        device_id,
        device_ids: None,
        offset_ms,
    }
}

#[tokio::test]
async fn estimate_accept_regroup_keeps_exif_times() {
    let (e, sid) = setup().await;
    let before = times(&e, sid).await;
    assert_eq!(before.len(), 25);
    // two clocks 5 h apart: nothing groups across devices, and the day splits into two events
    assert_eq!(bursts(&e, sid).await.len(), 24);
    assert_eq!(e.core.timeline(sid).await.unwrap().events.len(), 2);

    let o = e.core.device_offsets(sid, false).await.unwrap();
    assert_eq!(o.devices.len(), 2);
    assert!(o.estimated_at.is_some());
    let (cam, phone) = (device(&o, "Fake"), device(&o, "Apple"));
    assert_eq!(o.reference_device_id, Some(cam.device_id), "more photos");
    assert_eq!(cam.estimate.as_ref().unwrap().source, EstSource::Reference);
    let est = phone.estimate.as_ref().expect("phone estimate");
    assert_eq!(est.source, EstSource::Visual);
    let err = (est.offset_ms + ERR_S as i64 * 1000).abs();
    assert!(err <= 5_000, "{est:?} off by {err} ms");
    assert!(
        est.confidence >= ACCEPT_MIN_CONFIDENCE && est.evidence >= 10,
        "{est:?}"
    );
    // an estimate is not yet applied
    assert_eq!((phone.offset_ms, phone.source), (0, OffsetSource::None));
    assert_eq!(bursts(&e, sid).await.len(), 24);

    // accept every confident estimate
    let o = e
        .core
        .update_device_offsets(sid, req(OffsetAction::Accept, None, None))
        .await
        .unwrap();
    let phone = device(&o, "Apple");
    assert_eq!(phone.source, OffsetSource::Auto);
    assert_eq!(phone.offset_ms, est.offset_ms);
    assert_eq!(
        (device(&o, "Fake").offset_ms, device(&o, "Fake").source),
        (0, OffsetSource::None)
    );

    // the same scene of both devices is one burst now, and the day is one event
    let b = bursts(&e, sid).await;
    assert_eq!(b.len(), SCENES as usize, "{b:?}");
    assert!(b
        .iter()
        .all(|g| g.iter().any(|n| n.starts_with("phone_"))
            && g.iter().any(|n| n.starts_with("cam_"))));
    assert_eq!(b.iter().map(Vec::len).sum::<usize>(), 25);
    let t = e.core.timeline(sid).await.unwrap();
    assert_eq!(t.events.len(), 1, "{t:#?}");
    // burst times are corrected times: the phone's 5 h are gone
    let g = e.core.groups(sid).await.unwrap();
    let last = g
        .scenes
        .iter()
        .flat_map(|s| &s.bursts)
        .map(|b| b.start_at)
        .max()
        .unwrap();
    assert!(
        (last - (BASE as i64 + 600 * (SCENES as i64 - 1)) * 1000).abs() <= 5_000,
        "{last}"
    );
    // the EXIF times of the photos did not change
    assert_eq!(times(&e, sid).await, before);

    // reset: back to two clocks
    let o = e
        .core
        .update_device_offsets(sid, req(OffsetAction::Reset, None, None))
        .await
        .unwrap();
    assert!(o
        .devices
        .iter()
        .all(|d| d.offset_ms == 0 && d.source == OffsetSource::None));
    assert_eq!(bursts(&e, sid).await.len(), 24);
    assert_eq!(e.core.timeline(sid).await.unwrap().events.len(), 2);
    assert_eq!(times(&e, sid).await, before);
    // the estimate is still there to accept again
    assert!(device(&o, "Apple").estimate.is_some());
}

#[tokio::test]
async fn manual_offset_and_validation() {
    let (e, sid) = setup().await;
    let o = e.core.device_offsets(sid, false).await.unwrap();
    let phone = device(&o, "Apple").device_id;

    // typed in: 5 h 3 min back, the phone's shots are 0-2 s after the camera's
    let o = e
        .core
        .update_device_offsets(
            sid,
            req(OffsetAction::Set, Some(phone), Some(-(ERR_S as i64) * 1000)),
        )
        .await
        .unwrap();
    let p = device(&o, "Apple");
    assert_eq!(
        (p.offset_ms, p.source),
        (-(ERR_S as i64) * 1000, OffsetSource::Manual)
    );
    assert_eq!(bursts(&e, sid).await.len(), SCENES as usize);
    // survives reading again and a fresh estimate
    let o = e.core.device_offsets(sid, true).await.unwrap();
    assert_eq!(device(&o, "Apple").offset_ms, -(ERR_S as i64) * 1000);

    // zero is no correction
    let o = e
        .core
        .update_device_offsets(sid, req(OffsetAction::Set, Some(phone), Some(0)))
        .await
        .unwrap();
    assert_eq!(device(&o, "Apple").source, OffsetSource::None);
    assert_eq!(bursts(&e, sid).await.len(), 24);

    let bad = |r: Result<DeviceOffsetsOut>| match r {
        Err(CoreError::BadRequest(_)) => {}
        other => panic!("expected bad request, got {other:?}"),
    };
    bad(e
        .core
        .update_device_offsets(sid, req(OffsetAction::Set, Some(phone), None))
        .await);
    bad(e
        .core
        .update_device_offsets(sid, req(OffsetAction::Set, None, Some(1000)))
        .await);
    bad(e
        .core
        .update_device_offsets(
            sid,
            req(
                OffsetAction::Set,
                Some(phone),
                Some(MAX_MANUAL_OFFSET_MS + 1),
            ),
        )
        .await);
    match e
        .core
        .update_device_offsets(sid, req(OffsetAction::Set, Some(987_654), Some(1000)))
        .await
    {
        Err(CoreError::NotFound(_)) => {}
        other => panic!("expected not found, got {other:?}"),
    }
    match e.core.device_offsets(424_242, false).await {
        Err(CoreError::NotFound(_)) => {}
        other => panic!("expected not found, got {other:?}"),
    }
}

#[tokio::test]
async fn gps_time_anchors_the_phone_and_the_camera_follows() {
    let (e, sid) = setup().await;
    // the phone knows the true time from GPS; its UTC offset tag makes `taken_at` real UTC
    let true_gap = ERR_S as i64 * 1000;
    e.core
        .db
        .call(move |c| {
            c.execute(
                "UPDATE photo SET gps_time_ms = taken_at - ?1, taken_at_offset_min = 0
                 WHERE file_name LIKE 'phone_%'",
                [true_gap],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let o = e.core.device_offsets(sid, true).await.unwrap();
    let phone = device(&o, "Apple").estimate.clone().unwrap();
    assert_eq!(phone.source, EstSource::Gps);
    assert_eq!(phone.offset_ms, -true_gap);
    // the camera is aligned to UTC through the phone: its clock was right
    let cam = device(&o, "Fake").estimate.clone().unwrap();
    assert_eq!(cam.source, EstSource::Visual);
    assert!(cam.offset_ms.abs() <= 5_000, "{cam:?}");
    assert_eq!(o.reference_device_id, None, "GPS time is the reference");
}

#[tokio::test]
async fn a_new_analysis_drops_the_estimates_but_not_the_offsets() {
    let (e, sid) = setup().await;
    let o = e.core.device_offsets(sid, false).await.unwrap();
    let phone = device(&o, "Apple").device_id;
    e.core
        .update_device_offsets(sid, req(OffsetAction::Accept, Some(phone), None))
        .await
        .unwrap();
    assert_eq!(bursts(&e, sid).await.len(), SCENES as usize);

    analyse(&e, sid, true).await;
    let o = e.core.db.call(move |c| read_offsets(c, sid)).await.unwrap();
    assert!(o.estimated_at.is_none(), "estimates are stale after a run");
    assert!(device(&o, "Apple").estimate.is_none());
    // the applied offset is still used by the grouping of the new run
    assert_eq!(device(&o, "Apple").source, OffsetSource::Auto);
    assert_eq!(bursts(&e, sid).await.len(), SCENES as usize);
}

#[tokio::test]
async fn accept_without_confident_estimate_is_refused() {
    let e = env();
    // one device: only a reference, nothing to accept
    for k in 0..4u64 {
        let n = format!("cam_{k}.jpg");
        photo(e.dir.path(), &n, 600 * k, (k * 20) as u8);
        e.worker.set(&n, FakeSpec::at(40.0 * k as f32));
    }
    let sid = import(&e).await;
    analyse(&e, sid, false).await;
    let o = e.core.device_offsets(sid, false).await.unwrap();
    assert_eq!(o.devices.len(), 1);
    assert_eq!(o.reference_device_id, Some(o.devices[0].device_id));
    match e
        .core
        .update_device_offsets(sid, req(OffsetAction::Accept, None, None))
        .await
    {
        Err(CoreError::BadRequest(m)) => assert!(m.contains("confidence"), "{m}"),
        other => panic!("expected bad request, got {other:?}"),
    }
}
