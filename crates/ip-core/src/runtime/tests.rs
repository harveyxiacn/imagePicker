use std::time::Duration;

use super::*;
use crate::fake_worker::FakeWorker;
use crate::testutil::{FakeImaging, FakeRenderer};
use crate::CoreConfig;
use ip_render::Backend;

fn hw(os: &str, arch: &str, driver: Option<&str>) -> Hardware {
    Hardware {
        os: os.into(),
        arch: arch.into(),
        nvidia: driver.map(|d| Nvidia {
            name: "NVIDIA GeForce RTX 4090".into(),
            driver: d.into(),
        }),
    }
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

#[test]
fn parses_nvidia_smi() {
    let n = parse_nvidia_smi("NVIDIA GeForce RTX 4090, 616.00\n").unwrap();
    assert_eq!(n.name, "NVIDIA GeForce RTX 4090");
    assert_eq!(n.driver, "616.00");
    // multi GPU: the first one; blank lines ignored
    let n = parse_nvidia_smi("\nNVIDIA A, 580.65.06\nNVIDIA B, 550.1\n").unwrap();
    assert_eq!(n.name, "NVIDIA A");
    assert!(parse_nvidia_smi("").is_none());
    assert!(parse_nvidia_smi("NVIDIA-SMI has failed because it couldn't communicate").is_none());
    assert_eq!(driver_major("616.00"), Some(616));
    assert_eq!(driver_major("580.65.06"), Some(580));
    assert_eq!(driver_major("x"), None);
}

#[test]
fn hardware_decides_extras() {
    // new enough driver -> cuda
    assert_eq!(
        decide_extras(&hw("windows", "x86_64", Some("616.00"))),
        s(&["cuda", "mediapipe"])
    );
    assert_eq!(
        decide_extras(&hw("linux", "x86_64", Some("580.00"))),
        s(&["cuda", "mediapipe"])
    );
    // driver too old for the CUDA 13 build in uv.lock
    assert_eq!(
        decide_extras(&hw("windows", "x86_64", Some("551.23"))),
        s(&["cpu", "mediapipe"])
    );
    // no NVIDIA (AMD / Intel / none), macOS (CoreML ships in the cpu wheel), unsupported arch
    assert_eq!(
        decide_extras(&hw("windows", "x86_64", None)),
        s(&["cpu", "mediapipe"])
    );
    assert_eq!(
        decide_extras(&hw("macos", "aarch64", None)),
        s(&["cpu", "mediapipe"])
    );
    assert_eq!(
        decide_extras(&hw("linux", "aarch64", Some("600.0"))),
        s(&["cpu", "mediapipe"])
    );
    // unparsable driver is not trusted
    assert_eq!(
        decide_extras(&hw("windows", "x86_64", Some("weird"))),
        s(&["cpu", "mediapipe"])
    );
}

#[test]
fn extras_validation() {
    assert_eq!(
        validate_extras(&s(&["mediapipe", "cuda"])).unwrap(),
        s(&["cuda", "mediapipe"])
    );
    assert!(validate_extras(&s(&["cpu", "cuda"])).is_err());
    assert!(validate_extras(&s(&["mediapipe"])).is_err());
    assert!(validate_extras(&s(&["cpu", "torch;rm"])).is_err());
    assert!(validate_extras(&s(&["cuda", "llm"])).is_err());
    assert!(validate_extras(&s(&["cpu", "llm-cuda"])).is_err());
    assert!(validate_extras(&s(&["cuda", "llm-cuda"])).is_ok());
    assert_eq!(validate_extras(&s(&["cpu", "cpu"])).unwrap(), s(&["cpu"]));
}

#[test]
fn builds_uv_commands() {
    assert_eq!(python_install_args(), s(&["python", "install", "3.12"]));
    assert_eq!(
        sync_args(&s(&["cuda", "mediapipe"])),
        s(&[
            "sync",
            "--frozen",
            "--no-dev",
            "--no-editable",
            "--python",
            "3.12",
            "--extra",
            "cuda",
            "--extra",
            "mediapipe"
        ])
    );
}

#[test]
fn uv_env_points_into_the_data_dir_and_mirrors_are_opt_in() {
    let root = Path::new("/data/runtime");
    let none = |_: &str| None;
    let env = uv_env(root, Mirror::Off, &none);
    let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    assert!(get("UV_PROJECT_ENVIRONMENT").unwrap().ends_with("venv"));
    assert!(get("UV_CACHE_DIR").unwrap().ends_with("uv-cache"));
    assert!(get("UV_PYTHON_INSTALL_DIR").unwrap().ends_with("python"));
    assert_eq!(get("UV_PYTHON_PREFERENCE").unwrap(), "only-managed");
    assert!(get("UV_INDEX_URL").is_none());
    assert!(get("UV_PYTHON_INSTALL_MIRROR").is_none());

    let env = uv_env(root, Mirror::Cn, &none);
    let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    assert!(get("UV_INDEX_URL").unwrap().contains("tuna"));
    assert!(get("UV_PYTHON_INSTALL_MIRROR")
        .unwrap()
        .contains("python-build-standalone"));

    // explicit environment wins, even with the mirror off
    let custom = |k: &str| (k == ENV_UV_INDEX_URL).then(|| "https://example.test/simple".into());
    let env = uv_env(root, Mirror::Off, &custom);
    assert!(env
        .iter()
        .any(|(k, v)| k == "UV_INDEX_URL" && v == "https://example.test/simple"));
}

#[test]
fn parses_uv_progress() {
    let mut p = ProgressParser::default();
    let u = p
        .feed(
            Step::Python,
            "Downloading cpython-3.12.9-windows-x86_64 (20.1MiB)",
        )
        .unwrap();
    assert!(u.detail.starts_with("Downloading cpython"));
    let python_done = p
        .feed(Step::Python, "Installed Python 3.12.9 in 2.3s")
        .unwrap();
    assert!(python_done.percent >= u.percent);
    assert!(p.feed(Step::Python, "some noise").is_none());

    let a = p.feed(Step::Sync, "Resolved 83 packages in 1ms").unwrap();
    let mut last = a.percent;
    // all downloads are announced first, then finish one by one: progress follows the bytes
    for pkg in [
        "onnxruntime-gpu (300MiB)",
        "nvidia-cudnn-cu13 (0.7GiB)",
        "numpy (12MiB)",
    ] {
        p.feed(Step::Sync, &format!("Downloading {pkg}")).unwrap();
    }
    assert_eq!(p.percent(), 25);
    for done in ["numpy", "onnxruntime-gpu", "nvidia-cudnn-cu13"] {
        let u = p.feed(Step::Sync, &format!(" Downloaded {done}")).unwrap();
        assert!(u.percent >= last, "monotonic: {} >= {last}", u.percent);
        last = u.percent;
    }
    assert_eq!(last, 80);
    assert_eq!(
        parse_download("numpy (12.0MiB)"),
        ("numpy".to_string(), 12.0)
    );
    assert_eq!(parse_download("x (2GiB)").1, 2048.0);
    assert_eq!(parse_download("x").1, 1.0);
    let prepared = p.feed(Step::Sync, "Prepared 83 packages in 40s").unwrap();
    assert!(prepared.percent >= last);
    let plus = p.feed(Step::Sync, " + numpy==2.2.1").unwrap();
    assert_eq!(plus.detail, "Installing numpy==2.2.1");
    let done = p.feed(Step::Sync, "Installed 83 packages in 5s").unwrap();
    assert_eq!(done.percent, 95);
    // never goes backwards, never reaches 100 by itself
    let again = p.feed(Step::Sync, "Resolved 83 packages in 1ms").unwrap();
    assert_eq!(again.percent, 95);
    assert!(p.feed(Step::Verify, "anything").is_none());
}

fn worker_fixture() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("pyproject.toml"), "[project]\nname='x'\n").unwrap();
    std::fs::write(d.path().join("uv.lock"), "version = 1\n").unwrap();
    std::fs::write(d.path().join("README.md"), "hi").unwrap();
    std::fs::create_dir_all(d.path().join("imagepicker_ai/__pycache__")).unwrap();
    std::fs::write(d.path().join("imagepicker_ai/__init__.py"), "").unwrap();
    std::fs::write(d.path().join("imagepicker_ai/__pycache__/x.pyc"), "").unwrap();
    std::fs::create_dir_all(d.path().join("tests")).unwrap();
    std::fs::write(d.path().join("tests/test_x.py"), "").unwrap();
    std::fs::create_dir_all(d.path().join("models")).unwrap();
    std::fs::write(d.path().join("models/w.onnx"), "").unwrap();
    d
}

#[test]
fn copies_only_the_worker_source() {
    let src = worker_fixture();
    let dst = tempfile::tempdir().unwrap();
    let to = dst.path().join("worker-src/1");
    copy_worker_source(src.path(), &to).unwrap();
    assert!(to.join("pyproject.toml").is_file());
    assert!(to.join("uv.lock").is_file());
    assert!(to.join("README.md").is_file());
    assert!(to.join("imagepicker_ai/__init__.py").is_file());
    assert!(!to.join("imagepicker_ai/__pycache__").exists());
    assert!(!to.join("tests").exists());
    assert!(!to.join("models").exists());
}

/// A scripted stand-in for `uv`. `fail-sync` / `slow` marker files next to it select behaviour;
/// every invocation is appended to `args.log` together with the venv/cache env it saw.
#[cfg(unix)]
fn set_exec(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}
#[cfg(not(unix))]
fn set_exec(_: &Path) {}

fn fake_uv(dir: &Path) -> PathBuf {
    if cfg!(windows) {
        let p = dir.join("uv.cmd");
        std::fs::write(
            &p,
            "@echo off\r\n\
             echo %* [%UV_PROJECT_ENVIRONMENT%] [%UV_CACHE_DIR%] >> \"%~dp0args.log\"\r\n\
             if \"%1\"==\"python\" goto py\r\n\
             if \"%1\"==\"sync\" goto sync\r\n\
             exit /b 2\r\n\
             :py\r\n\
             echo Downloading cpython-3.12.9 (20MiB)\r\n\
             echo Installed Python 3.12.9 in 1s\r\n\
             exit /b 0\r\n\
             :sync\r\n\
             if exist \"%~dp0fail-sync\" (echo error: Failed to download onnxruntime-gpu 1>&2 & exit /b 1)\r\n\
             if exist \"%~dp0slow\" (ping -n 60 127.0.0.1 >nul)\r\n\
             echo Resolved 5 packages in 10ms\r\n\
             echo Downloading numpy (12MiB)\r\n\
             echo Prepared 3 packages in 2s\r\n\
             echo Installed 3 packages in 100ms\r\n\
             mkdir \"%UV_PROJECT_ENVIRONMENT%\" 2>nul\r\n\
             (echo @echo Python 3.12.9)> \"%UV_PROJECT_ENVIRONMENT%\\python-fake.cmd\"\r\n\
             exit /b 0\r\n",
        )
        .unwrap();
        p
    } else {
        let p = dir.join("uv");
        std::fs::write(
            &p,
            "#!/bin/sh\n\
             d=$(dirname \"$0\")\n\
             echo \"$* [$UV_PROJECT_ENVIRONMENT] [$UV_CACHE_DIR]\" >> \"$d/args.log\"\n\
             if [ \"$1\" = python ]; then echo 'Downloading cpython-3.12.9 (20MiB)'; echo 'Installed Python 3.12.9 in 1s'; exit 0; fi\n\
             if [ \"$1\" = sync ]; then\n\
               if [ -e \"$d/fail-sync\" ]; then echo 'error: Failed to download onnxruntime-gpu' >&2; exit 1; fi\n\
               if [ -e \"$d/slow\" ]; then sleep 60; fi\n\
               echo 'Resolved 5 packages in 10ms'; echo 'Downloading numpy (12MiB)'\n\
               echo 'Prepared 3 packages in 2s'; echo 'Installed 3 packages in 100ms'\n\
               mkdir -p \"$UV_PROJECT_ENVIRONMENT\"\n\
               printf '#!/bin/sh\\necho Python 3.12.9\\n' > \"$UV_PROJECT_ENVIRONMENT/python-fake\"\n\
               chmod +x \"$UV_PROJECT_ENVIRONMENT/python-fake\"\n\
               exit 0\n\
             fi\n\
             exit 2\n",
        )
        .unwrap();
        set_exec(&p);
        p
    }
}

struct Env {
    core: Arc<Core>,
    tools: tempfile::TempDir,
    _data: tempfile::TempDir,
    _src: tempfile::TempDir,
}

fn env() -> Env {
    let data = tempfile::tempdir().unwrap();
    let src = worker_fixture();
    let tools = tempfile::tempdir().unwrap();
    let uv = fake_uv(tools.path());
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(1),
        worker: Some(Arc::new(FakeWorker::new())),
        renderer: Some(Arc::new(FakeRenderer::new(Backend::Cpu))),
        force_cpu: true,
    })
    .unwrap();
    core.runtime.set_bundled(Bundled {
        uv: Some(uv),
        worker_src: Some(src.path().to_path_buf()),
    });
    core.runtime.set_python_rel(if cfg!(windows) {
        "python-fake.cmd"
    } else {
        "python-fake"
    });
    Env {
        core,
        tools,
        _data: data,
        _src: src,
    }
}

async fn wait_state(core: &Arc<Core>, want: &str) -> Value {
    for _ in 0..400 {
        let v = core.runtime.status(true).await;
        if v["state"] == want {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!(
        "runtime never reached {want}: {}",
        core.runtime.status(true).await
    );
}

fn args_log(e: &Env) -> String {
    std::fs::read_to_string(e.tools.path().join("args.log")).unwrap_or_default()
}

#[tokio::test]
async fn install_runs_python_then_sync_then_verifies() {
    let e = env();
    let st = e.core.runtime.status(true).await;
    assert_eq!(st["state"], "missing");
    assert_eq!(st["can_install"], true);
    let mut rx = e.core.events.subscribe();

    let task = e
        .core
        .runtime_install(InstallRequest {
            extras: Some(s(&["cpu", "mediapipe"])),
            mirror: None,
        })
        .await
        .unwrap();
    assert!(task.starts_with("runtime-"));
    // a second install while running is refused
    assert!(matches!(
        e.core.runtime_install(InstallRequest::default()).await,
        Err(CoreError::Conflict(_))
    ));
    let st = wait_state(&e.core, "ready").await;
    assert_eq!(st["extras"], json!(["cpu", "mediapipe"]));
    assert_eq!(st["version"], RUNTIME_VERSION);
    assert_eq!(st["python"], "Python 3.12.9");
    assert!(st["venv_bytes"].as_u64().is_some());
    assert!(st["error"].is_null());

    let log = args_log(&e);
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 2, "{log}");
    assert!(lines[0].starts_with("python install 3.12"));
    assert!(lines[1].starts_with(
        "sync --frozen --no-dev --no-editable --python 3.12 --extra cpu --extra mediapipe"
    ));
    assert!(lines[1].contains("venv]"), "venv env var set: {}", lines[1]);
    assert!(lines[1].contains("uv-cache]"));
    // the worker source was copied side by side
    let ws = e
        .core
        .runtime
        .root()
        .join("worker-src")
        .join(RUNTIME_VERSION);
    assert!(ws.join("pyproject.toml").is_file());
    // the log file exists
    let logf = e.core.dirs.logs.join("runtime-install.log");
    assert!(std::fs::read_to_string(logf)
        .unwrap()
        .contains("Installed 3 packages"));

    // events: runtime.updated with the final state, task.progress done
    let mut saw_done = false;
    let mut saw_ready = false;
    while let Ok(ev) = rx.try_recv() {
        match ev {
            Event::TaskProgress {
                kind, state, done, ..
            } if kind == "runtime_install" && state == "done" => {
                assert_eq!(done, 100);
                saw_done = true;
            }
            Event::RuntimeUpdated { runtime } if runtime["state"] == "ready" => saw_ready = true,
            _ => {}
        }
    }
    assert!(saw_done && saw_ready);
}

#[tokio::test]
async fn failed_install_reports_the_error_and_can_be_retried() {
    let e = env();
    std::fs::write(e.tools.path().join("fail-sync"), "").unwrap();
    e.core
        .runtime_install(InstallRequest::default_cpu())
        .await
        .unwrap();
    let st = wait_state(&e.core, "failed").await;
    assert!(st["error"].as_str().unwrap().contains("onnxruntime-gpu"));
    assert_eq!(st["can_install"], true);
    // no marker, so the worker will not launch from a half-built venv
    assert!(!e.core.runtime.root().join(MARKER_FILE).exists());

    std::fs::remove_file(e.tools.path().join("fail-sync")).unwrap();
    e.core
        .runtime_install(InstallRequest::default_cpu())
        .await
        .unwrap();
    wait_state(&e.core, "ready").await;
}

#[tokio::test]
async fn cancel_stops_the_install() {
    let e = env();
    std::fs::write(e.tools.path().join("slow"), "").unwrap();
    e.core
        .runtime_install(InstallRequest::default_cpu())
        .await
        .unwrap();
    // wait until the sync step runs
    for _ in 0..200 {
        if e.core.runtime.status(true).await["step"] == "sync" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    e.core.runtime_cancel();
    let st = wait_state(&e.core, "missing").await;
    assert!(st["error"].is_null());
    assert!(!e.core.runtime.root().join(MARKER_FILE).exists());
}

#[tokio::test]
async fn refuses_without_network_consent_or_tools() {
    let e = env();
    e.core
        .patch_settings(json!({"privacy": {"allow_network": false}}))
        .await
        .unwrap();
    assert!(matches!(
        e.core.runtime_install(InstallRequest::default_cpu()).await,
        Err(CoreError::Conflict(_))
    ));
    let st = e.core.runtime.status(false).await;
    assert_eq!(st["can_install"], false);
    assert!(st["reason"].as_str().unwrap().contains("network"));

    // bad extras are a 422
    e.core
        .patch_settings(json!({"privacy": {"allow_network": true}}))
        .await
        .unwrap();
    assert!(matches!(
        e.core
            .runtime_install(InstallRequest {
                extras: Some(s(&["cpu", "cuda"])),
                mirror: None
            })
            .await,
        Err(CoreError::Unprocessable(_))
    ));
}

#[tokio::test]
async fn app_update_invalidates_the_runtime_and_remove_cleans_up() {
    let e = env();
    e.core
        .runtime_install(InstallRequest::default_cpu())
        .await
        .unwrap();
    wait_state(&e.core, "ready").await;
    // a marker of another app version = outdated -> missing (re-sync needed)
    let mut m = read_marker(e.core.runtime.root()).unwrap();
    m.version = "0.0.0-old".into();
    std::fs::write(
        e.core.runtime.root().join(MARKER_FILE),
        serde_json::to_vec(&m).unwrap(),
    )
    .unwrap();
    let st = e.core.runtime.status(true).await;
    assert_eq!(st["state"], "missing");
    assert_eq!(st["outdated"], true);

    e.core.runtime_remove().await.unwrap();
    assert!(!e.core.runtime.root().exists());
    assert_eq!(e.core.runtime.status(true).await["state"], "missing");
}

impl InstallRequest {
    fn default_cpu() -> Self {
        Self {
            extras: Some(vec!["cpu".into()]),
            mirror: None,
        }
    }
}
