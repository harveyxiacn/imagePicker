//! Lifecycle tests of `ManagedWorker` against the `ip-fake-worker` stand-in binary.

use std::time::Duration;

use ip_worker_client::process::pid_alive;
use ip_worker_client::{
    AiWorker, AnalyzeRequest, AnalyzeRequestItem, CancelToken, ManagedWorker, WorkerConfig,
    WorkerError, WorkerState,
};
use serde_json::json;

fn fake_cmd() -> String {
    format!(
        "\"{}\" --token {{token}}",
        env!("CARGO_BIN_EXE_ip-fake-worker")
    )
}

fn cfg() -> WorkerConfig {
    WorkerConfig {
        cmd: Some(fake_cmd()),
        start_timeout: Duration::from_secs(20),
        backoff_base: Duration::from_millis(50),
        cooldown: Duration::from_millis(600),
        ..WorkerConfig::default()
    }
}

fn req(n: usize) -> AnalyzeRequest {
    AnalyzeRequest {
        items: (0..n as i64)
            .map(|i| AnalyzeRequestItem {
                photo_id: i,
                path: "x.jpg".into(),
                orientation: 1,
            })
            .collect(),
        profile: Some("fast".into()),
        steps: None,
        analysis_size: 1024,
        out_dir: "out".into(),
        allow_download: false,
    }
}

async fn wait_dead(pid: u32) {
    for _ in 0..100 {
        if !pid_alive(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("process {pid} is still alive");
}

#[tokio::test]
async fn lazy_start_info_and_analyze() {
    let w = ManagedWorker::new(cfg());
    assert_eq!(w.status().state, WorkerState::Stopped);
    let info = w.system_info().await.unwrap();
    assert_eq!(info.tier.as_deref(), Some("T9"));
    assert_eq!(info.hardware.gpus[0].vram_mb, Some(1234));
    let st = w.status();
    assert_eq!(st.state, WorkerState::Ready);
    assert_eq!(st.tier.as_deref(), Some("T9"));

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let r = w
        .analyze_batch(&req(3), Some(tx), &CancelToken::new())
        .await
        .unwrap();
    assert!(r.items.is_empty());
    assert_eq!(rx.try_recv().unwrap()["done"], 3);
    w.shutdown().await;
    assert_eq!(w.status().state, WorkerState::Stopped);
}

#[tokio::test]
async fn crash_is_reported_and_next_call_restarts() {
    let w = ManagedWorker::new(cfg());
    let pid1 = w.raw_call("pid", json!({})).await.unwrap().as_u64().unwrap() as u32;
    let r = w.raw_call("crash", json!({})).await;
    assert!(matches!(r, Err(WorkerError::Disconnected)), "{r:?}");
    for _ in 0..50 {
        if w.status().state == WorkerState::Crashed {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(w.status().state, WorkerState::Crashed);
    // the next call restarts with a fresh process
    let pid2 = w.raw_call("pid", json!({})).await.unwrap().as_u64().unwrap() as u32;
    assert_ne!(pid1, pid2);
    assert_eq!(w.status().state, WorkerState::Ready);
    w.shutdown().await;
    wait_dead(pid2).await;
}

#[tokio::test]
async fn process_tree_is_killed_on_drop() {
    let mut c = cfg();
    c.cmd = Some(format!("{} --grandchild", fake_cmd()));
    let w = ManagedWorker::new(c);
    let info = w.raw_call("system.info", json!({})).await.unwrap();
    let pid = info["pid"].as_u64().unwrap() as u32;
    let grand = info["grandchild_pid"].as_u64().unwrap() as u32;
    assert!(pid_alive(pid) && pid_alive(grand));
    drop(w);
    wait_dead(pid).await;
    wait_dead(grand).await;
}

#[tokio::test]
async fn unavailable_when_launcher_is_missing() {
    let w = ManagedWorker::new(WorkerConfig {
        cmd: Some("definitely-not-a-real-program-xyz".into()),
        ..cfg()
    });
    let e = w.system_info().await.unwrap_err();
    assert!(matches!(e, WorkerError::Unavailable(_)), "{e:?}");
    let st = w.status();
    assert_eq!(st.state, WorkerState::Unavailable);
    assert!(st.error.unwrap().contains("not found"));
}

#[tokio::test]
async fn repeated_start_failures_become_unavailable() {
    // Starts, but dies before printing the ready line (no --token => the fake worker panics).
    let w = ManagedWorker::new(WorkerConfig {
        cmd: Some(format!("\"{}\" --nope {{token}}", env!("CARGO_BIN_EXE_ip-fake-worker"))),
        ..cfg()
    });
    let e = w.system_info().await.unwrap_err();
    assert!(matches!(e, WorkerError::Unavailable(_)), "{e:?}");
    assert_eq!(w.status().state, WorkerState::Unavailable);
    // within the cool-off the next call fails fast
    let t = std::time::Instant::now();
    assert!(w.system_info().await.is_err());
    assert!(t.elapsed() < Duration::from_millis(400));
}
