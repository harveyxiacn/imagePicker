//! Task history and cancellation (`crate::tasks`): export cancellation, the history list, and
//! tasks left running by a previous process. FakeImaging + FakeWorker + FakeRenderer.

use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use ip_render::Backend;
use serde_json::json;

use crate::fake_worker::FakeWorker;
use crate::testutil::{write_jpeg, FakeImaging, FakeRenderer};
use crate::*;

const WAIT: Duration = Duration::from_secs(30);

fn open(data: &Path, worker: Arc<FakeWorker>) -> Arc<Core> {
    Core::open(CoreConfig {
        data_dir: Some(data.to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(2),
        worker: Some(worker),
        renderer: Some(Arc::new(FakeRenderer::new(Backend::Cpu))),
        force_cpu: true,
    })
    .unwrap()
}

/// A session of `n` plain photos; returns their ids.
async fn photos(core: &Arc<Core>, dir: &Path, n: usize) -> Vec<i64> {
    for i in 0..n {
        write_jpeg(&dir.join(format!("p{i}.jpg")), 320, 240, (i * 40) as u8);
    }
    let s = core
        .import(ImportRequest {
            path: dir.to_string_lossy().into_owned(),
            recursive: true,
            title: None,
        })
        .await
        .unwrap();
    core.wait_session_ready(s.id, WAIT).await.unwrap();
    core.photos(PhotoQuery {
        session_id: s.id,
        limit: Some(100),
        ..Default::default()
    })
    .await
    .unwrap()
    .photos
    .into_iter()
    .map(|p| p.id)
    .collect()
}

async fn until(mut pred: impl FnMut() -> bool) {
    let t = std::time::Instant::now();
    while !pred() {
        assert!(t.elapsed() < WAIT, "condition never held");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// The last `task.progress` of `task` (waits for a terminal state).
async fn last_progress(
    rx: &mut tokio::sync::broadcast::Receiver<Event>,
    task: &str,
) -> (String, i64, i64, Option<String>) {
    tokio::time::timeout(WAIT, async {
        loop {
            match rx.recv().await {
                Ok(Event::TaskProgress {
                    task_id,
                    state,
                    done,
                    total,
                    error,
                    ..
                }) if task_id == task && state != "running" => return (state, done, total, error),
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(e) => panic!("event stream closed: {e}"),
            }
        }
    })
    .await
    .expect("the task ends")
}

#[tokio::test]
async fn an_upscaling_export_can_be_cancelled() {
    let (data, src) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let worker = Arc::new(FakeWorker::new());
    let core = open(data.path(), worker.clone());
    let ids = photos(&core, src.path(), 3).await;
    worker.gen_delay_ms.store(60_000, Ordering::SeqCst);
    let dest = src.path().join("out");
    let mut rx = core.events.subscribe();
    let task = core
        .export(ExportRequest {
            ids: ids.clone(),
            dest: dest.to_string_lossy().into_owned(),
            folders: None,
            long_edge: None,
            quality: 90,
            name_template: "{name}".into(),
            apply_edits: true,
            upscale: Some(2),
            strip_gps: false,
        })
        .await
        .unwrap();
    until(|| worker.enhance_calls.load(Ordering::SeqCst) >= 1).await;
    let rec = core.task_cancel(&task).await.unwrap();
    assert_eq!(
        (rec.kind.as_str(), rec.status.as_str()),
        ("export", "cancelling")
    );
    let (state, done, total, error) = last_progress(&mut rx, &task).await;
    assert_eq!((state.as_str(), total, error), ("cancelled", 3, None));
    let written = std::fs::read_dir(&dest).map(|d| d.count()).unwrap_or(0);
    assert_eq!(written as i64, done, "only finished files stay");
    assert!(done < 3);
    assert!(worker.cancelled_calls.load(Ordering::SeqCst) >= 1);
    let rec = &core.tasks_list(Some(1)).await.unwrap()[0];
    assert_eq!(
        (rec.id.as_str(), rec.status.as_str()),
        (task.as_str(), "cancelled")
    );
    assert_eq!(rec.params["upscale"], 2);
    assert!(matches!(
        core.task_cancel(&task).await,
        Err(CoreError::Conflict(_))
    ));
}

#[tokio::test]
async fn history_survives_a_restart_and_running_tasks_become_interrupted() {
    let (data, src) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let worker = Arc::new(FakeWorker::new());
    let core = open(data.path(), worker.clone());
    let ids = photos(&core, src.path(), 2).await;

    // one finished task, one still running when the process "exits"
    let mut rx = core.events.subscribe();
    let done_task = core
        .enhance_start(
            ids[0],
            EnhanceBody {
                op: "denoise".into(),
                strength: Some(0.4),
            },
        )
        .await
        .unwrap();
    assert_eq!(last_progress(&mut rx, &done_task).await.0, "done");
    worker.gen_delay_ms.store(60_000, Ordering::SeqCst);
    let stuck = core
        .enhance_start(
            ids[1],
            EnhanceBody {
                op: "denoise".into(),
                strength: None,
            },
        )
        .await
        .unwrap();
    until(|| worker.enhance_calls.load(Ordering::SeqCst) == 2).await;
    let scratch = core.dirs.gen.join(&stuck);
    std::fs::create_dir_all(&scratch).unwrap();
    let listed = core.tasks_list(None).await.unwrap();
    assert_eq!(listed[0].id, stuck);
    assert_eq!(
        (listed[0].status.as_str(), listed[0].cancellable),
        ("running", true)
    );
    assert_eq!(listed[1].id, done_task);
    assert_eq!(
        listed[1].params,
        json!({"photo_id": ids[0], "op": "denoise", "strength": 0.4})
    );

    // a second process opening the catalog: the stuck task is over
    let core2 = open(data.path(), Arc::new(FakeWorker::new()));
    let listed = core2.tasks_list(None).await.unwrap();
    assert_eq!(listed[0].id, stuck);
    assert_eq!(
        (listed[0].status.as_str(), listed[0].cancellable),
        ("interrupted", false)
    );
    assert!(listed[0].finished_at.is_some());
    assert_eq!(listed[1].status, "done");
    assert!(
        !scratch.exists(),
        "the interrupted task's scratch is removed"
    );
    // not running here: cannot be cancelled; unknown ids are 404
    assert!(matches!(
        core2.task_cancel(&stuck).await,
        Err(CoreError::Conflict(_))
    ));
    assert!(matches!(
        core2.task_cancel("enhance-999999").await,
        Err(CoreError::NotFound(_))
    ));
    // new ids continue after the stored ones
    let seq = |id: &str| id.rsplit('-').next().unwrap().parse::<u64>().unwrap();
    let next = core2
        .enhance_start(
            ids[0],
            EnhanceBody {
                op: "denoise".into(),
                strength: None,
            },
        )
        .await
        .unwrap();
    assert!(seq(&next) > seq(&stuck), "{next} after {stuck}");
    assert!(matches!(
        core2.tasks_list(Some(0)).await,
        Err(CoreError::BadRequest(_))
    ));
    assert!(matches!(
        core2.tasks_list(Some(tasks::KEEP_TASKS + 1)).await,
        Err(CoreError::BadRequest(_))
    ));
    // the first process' task is still cancellable there
    core.task_cancel(&stuck).await.unwrap();
}
