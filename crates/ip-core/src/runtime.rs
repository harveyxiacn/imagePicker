//! The installable AI runtime (milestone M7, docs/02 section 6 "AI 组件安装").
//!
//! A packaged app has no repo, no Python and no `uv`. On user consent the core installs, into
//! `<data>/runtime`, a private managed Python 3.12 plus the worker with the extras that fit the
//! hardware, using the `uv` binary bundled with the app:
//!
//! ```text
//! <data>/runtime/
//!   python/              UV_PYTHON_INSTALL_DIR (uv-managed CPython)
//!   uv-cache/            UV_CACHE_DIR
//!   venv/                UV_PROJECT_ENVIRONMENT
//!   worker-src/<ver>/    the bundled worker source (pyproject, uv.lock, package), side by side
//!   installed.json       written after a verified install (see [`RuntimeMarker`])
//! ```
//!
//! State machine: `missing -> installing -> ready | failed` (`failed`/`missing` can install
//! again, `ready` is invalidated by an app update because the marker records the app version).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ip_worker_client::{
    find_on_path, process, read_marker, venv_dir, venv_python, CancelToken, RuntimeMarker,
    MARKER_FILE,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::error::{CoreError, Result};
use crate::events::Event;
use crate::Core;

/// Python of the runtime (must satisfy `requires-python` of the worker).
pub const PYTHON_VERSION: &str = "3.12";
/// Minimum NVIDIA driver (major) for the CUDA 13 build of onnxruntime-gpu pinned in `uv.lock`.
pub const MIN_CUDA_DRIVER: u32 = 580;
/// The runtime version: an app update re-syncs the venv.
pub const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const ENV_UV: &str = "IMAGEPICKER_UV";
pub const ENV_WORKER_SRC: &str = "IMAGEPICKER_WORKER_SRC";
pub const ENV_UV_INDEX_URL: &str = "IMAGEPICKER_UV_INDEX_URL";
pub const ENV_PYTHON_MIRROR: &str = "IMAGEPICKER_PYTHON_MIRROR";

const CN_INDEX_URL: &str = "https://pypi.tuna.tsinghua.edu.cn/simple";
const CN_PYTHON_MIRROR: &str = "https://registry.npmmirror.com/-/binary/python-build-standalone";

// ------------------------------------------------------------------------ hardware -> extras

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Nvidia {
    pub name: String,
    pub driver: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hardware {
    /// `std::env::consts::OS` (`windows`, `linux`, `macos`).
    pub os: String,
    /// `std::env::consts::ARCH`.
    pub arch: String,
    pub nvidia: Option<Nvidia>,
}

/// First GPU of `nvidia-smi --query-gpu=name,driver_version --format=csv,noheader`.
pub fn parse_nvidia_smi(out: &str) -> Option<Nvidia> {
    out.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .and_then(|l| {
            let (name, driver) = l.rsplit_once(',')?;
            let (name, driver) = (name.trim(), driver.trim());
            (!name.is_empty() && driver.chars().next()?.is_ascii_digit()).then(|| Nvidia {
                name: name.to_string(),
                driver: driver.to_string(),
            })
        })
}

/// `"616.00"` -> `616`, `"580.65.06"` -> `580`.
pub fn driver_major(driver: &str) -> Option<u32> {
    driver.split('.').next()?.trim().parse().ok()
}

/// NVIDIA GPU detection via `nvidia-smi` (absent = no NVIDIA driver).
pub async fn detect_hardware() -> Hardware {
    let mut cmd = Command::new("nvidia-smi");
    cmd.args(["--query-gpu=name,driver_version", "--format=csv,noheader"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let nvidia = match tokio::time::timeout(Duration::from_secs(8), cmd.output()).await {
        Ok(Ok(o)) if o.status.success() => parse_nvidia_smi(&String::from_utf8_lossy(&o.stdout)),
        _ => None,
    };
    Hardware {
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        nvidia,
    }
}

/// `cuda` only when an NVIDIA driver new enough for the locked onnxruntime-gpu build is present on
/// x86-64 Windows/Linux; everything else (macOS incl. CoreML, AMD, Intel, old drivers) is `cpu`.
/// `mediapipe` (face blendshapes / head pose) is added on every platform.
pub fn decide_extras(hw: &Hardware) -> Vec<String> {
    let cuda_ok = matches!(hw.os.as_str(), "windows" | "linux")
        && hw.arch == "x86_64"
        && hw
            .nvidia
            .as_ref()
            .and_then(|n| driver_major(&n.driver))
            .is_some_and(|m| m >= MIN_CUDA_DRIVER);
    vec![
        if cuda_ok { "cuda" } else { "cpu" }.to_string(),
        "mediapipe".to_string(),
    ]
}

/// Validates a user-chosen extras list: exactly one of `cpu`/`cuda`, optional `mediapipe`, and
/// optionally the LLM extra matching the accelerator (`llm` with cpu, `llm-cuda` with cuda).
pub fn validate_extras(extras: &[String]) -> std::result::Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for e in extras {
        if !matches!(
            e.as_str(),
            "cpu" | "cuda" | "mediapipe" | "llm" | "llm-cuda"
        ) {
            return Err(format!("unknown extra {e:?}"));
        }
        if !out.contains(e) {
            out.push(e.clone());
        }
    }
    let has = |n: &str| out.iter().any(|e| e == n);
    if has("cpu") == has("cuda") {
        return Err("choose exactly one of the extras cpu / cuda".into());
    }
    if has("llm") && !has("cpu") {
        return Err("extra llm needs cpu (use llm-cuda with cuda)".into());
    }
    if has("llm-cuda") && !has("cuda") {
        return Err("extra llm-cuda needs cuda".into());
    }
    out.sort_by_key(|e| match e.as_str() {
        "cpu" | "cuda" => 0,
        "mediapipe" => 1,
        _ => 2,
    });
    Ok(out)
}

// ------------------------------------------------------------------------ uv commands

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mirror {
    Off,
    Cn,
}

/// Environment of every `uv` invocation (`env_lookup` = process environment in production).
pub fn uv_env(
    root: &Path,
    mirror: Mirror,
    env_lookup: &dyn Fn(&str) -> Option<String>,
) -> Vec<(String, String)> {
    let p = |name: &str| root.join(name).to_string_lossy().into_owned();
    let mut env = vec![
        ("UV_PYTHON_INSTALL_DIR".to_string(), p("python")),
        ("UV_PROJECT_ENVIRONMENT".to_string(), p("venv")),
        ("UV_CACHE_DIR".to_string(), p("uv-cache")),
        ("UV_PYTHON_PREFERENCE".to_string(), "only-managed".into()),
        ("UV_NO_PROGRESS".to_string(), "1".into()),
        ("NO_COLOR".to_string(), "1".into()),
    ];
    let custom_index = env_lookup(ENV_UV_INDEX_URL).filter(|s| !s.trim().is_empty());
    let custom_py = env_lookup(ENV_PYTHON_MIRROR).filter(|s| !s.trim().is_empty());
    let index = custom_index.or_else(|| (mirror == Mirror::Cn).then(|| CN_INDEX_URL.to_string()));
    let py = custom_py.or_else(|| (mirror == Mirror::Cn).then(|| CN_PYTHON_MIRROR.to_string()));
    if let Some(i) = index {
        env.push(("UV_INDEX_URL".into(), i));
    }
    if let Some(m) = py {
        env.push(("UV_PYTHON_INSTALL_MIRROR".into(), m));
    }
    env
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Prepare,
    Python,
    Sync,
    Verify,
}

impl Step {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prepare => "prepare",
            Self::Python => "python",
            Self::Sync => "sync",
            Self::Verify => "verify",
        }
    }
}

/// `uv python install 3.12`
pub fn python_install_args() -> Vec<String> {
    ["python", "install", PYTHON_VERSION]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// `uv sync --frozen --no-dev --no-editable --extra <x> ...` (run with the worker source as cwd).
pub fn sync_args(extras: &[String]) -> Vec<String> {
    let mut a: Vec<String> = [
        "sync",
        "--frozen",
        "--no-dev",
        "--no-editable",
        "--python",
        PYTHON_VERSION,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for e in extras {
        a.push("--extra".into());
        a.push(e.clone());
    }
    a
}

// ------------------------------------------------------------------------ progress parsing

/// Turns `uv` output lines into a monotonic percentage and a short human detail.
#[derive(Debug, Default)]
pub struct ProgressParser {
    percent: u8,
    downloads: u32,
    installs: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressUpdate {
    pub percent: u8,
    pub detail: String,
}

impl ProgressParser {
    pub fn percent(&self) -> u8 {
        self.percent
    }

    fn at_least(&mut self, p: u8) -> u8 {
        self.percent = self.percent.max(p.min(99));
        self.percent
    }

    /// Feeds one line of `step` output; `None` when the line carries no progress.
    pub fn feed(&mut self, step: Step, line: &str) -> Option<ProgressUpdate> {
        let l = line.trim();
        let mk = |s: &mut Self, p: u8, d: String| {
            Some(ProgressUpdate {
                percent: s.at_least(p),
                detail: d,
            })
        };
        match step {
            Step::Python => {
                if let Some(rest) = l.strip_prefix("Downloading ") {
                    return mk(self, 8, format!("Downloading {}", short(rest)));
                }
                if l.starts_with("Installed Python") || l.starts_with("Installed ") {
                    return mk(self, 20, l.to_string());
                }
                None
            }
            Step::Sync => {
                if l.starts_with("Using CPython") || l.starts_with("Creating virtual environment") {
                    return mk(self, 22, "Creating the virtual environment".into());
                }
                if let Some(rest) = l.strip_prefix("Resolved ") {
                    return mk(self, 25, format!("Resolved {}", short(rest)));
                }
                if let Some(rest) = l.strip_prefix("Downloading ") {
                    self.downloads += 1;
                    let p = 25 + (self.downloads * 3).min(55) as u8;
                    return mk(self, p, format!("Downloading {}", short(rest)));
                }
                if let Some(rest) = l.strip_prefix("Prepared ") {
                    return mk(self, 82, format!("Prepared {}", short(rest)));
                }
                if let Some(rest) = l.strip_prefix("Installed ") {
                    return mk(self, 95, format!("Installed {}", short(rest)));
                }
                if let Some(pkg) = l.strip_prefix("+ ") {
                    self.installs += 1;
                    let p = 82 + (self.installs / 4).min(12) as u8;
                    return mk(self, p, format!("Installing {}", pkg.trim()));
                }
                None
            }
            Step::Prepare | Step::Verify => None,
        }
    }
}

fn short(s: &str) -> String {
    let s: String = s.chars().take(80).collect();
    s.trim().to_string()
}

// ------------------------------------------------------------------------ files

/// Copies the worker project (`pyproject.toml`, `uv.lock`, `README.md`, `imagepicker_ai/`) —
/// not tests, scripts, models or caches.
pub fn copy_worker_source(src: &Path, dst: &Path) -> std::io::Result<()> {
    fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            let name = entry.file_name();
            let n = name.to_string_lossy();
            if n == "__pycache__" || n.ends_with(".pyc") {
                continue;
            }
            let (f, t) = (entry.path(), to.join(&name));
            if entry.file_type()?.is_dir() {
                copy_dir(&f, &t)?;
            } else {
                std::fs::copy(&f, &t)?;
            }
        }
        Ok(())
    }
    std::fs::create_dir_all(dst)?;
    for file in ["pyproject.toml", "uv.lock", "README.md"] {
        let from = src.join(file);
        if from.is_file() {
            std::fs::copy(&from, dst.join(file))?;
        }
    }
    copy_dir(&src.join("imagepicker_ai"), &dst.join("imagepicker_ai"))
}

fn dir_size(p: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                stack.push(e.path());
            } else if ft.is_file() {
                total += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    total
}

fn has_worker_src(dir: &Path) -> bool {
    dir.join("pyproject.toml").is_file() && dir.join("imagepicker_ai").is_dir()
}

// ------------------------------------------------------------------------ manager

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeState {
    Missing,
    Installing,
    Ready,
    Failed,
}

impl RuntimeState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Installing => "installing",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }
}

/// Paths handed over by the desktop shell (bundled resources).
#[derive(Debug, Clone, Default)]
pub struct Bundled {
    pub uv: Option<PathBuf>,
    pub worker_src: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InstallRequest {
    /// Default: [`decide_extras`] for this machine.
    pub extras: Option<Vec<String>>,
    /// `auto` (default: from `models.source`, otherwise off) | `off` | `cn`.
    pub mirror: Option<String>,
}

struct Active {
    task_id: String,
    cancel: CancelToken,
}

struct Inner {
    state: RuntimeState,
    error: Option<String>,
    step: Option<&'static str>,
    percent: u8,
    detail: String,
    extras_requested: Vec<String>,
    active: Option<Active>,
    venv_bytes: Option<u64>,
}

pub struct RuntimeManager {
    root: PathBuf,
    logs_dir: PathBuf,
    version: String,
    bundled: Mutex<Bundled>,
    /// Test hook: interpreter path relative to the venv (default: the platform's).
    python_rel: Mutex<Option<PathBuf>>,
    inner: Mutex<Inner>,
}

impl RuntimeManager {
    pub fn new(data_dir: &Path, logs_dir: PathBuf) -> Self {
        let env_path = |n: &str| {
            std::env::var_os(n)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        Self {
            root: data_dir.join("runtime"),
            logs_dir,
            version: RUNTIME_VERSION.to_string(),
            bundled: Mutex::new(Bundled {
                uv: env_path(ENV_UV),
                worker_src: env_path(ENV_WORKER_SRC),
            }),
            python_rel: Mutex::new(None),
            inner: Mutex::new(Inner {
                state: RuntimeState::Missing,
                error: None,
                step: None,
                percent: 0,
                detail: String::new(),
                extras_requested: Vec::new(),
                active: None,
                venv_bytes: None,
            }),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    /// Where the desktop shell says `uv` and the worker source are.
    pub fn set_bundled(&self, b: Bundled) {
        let mut cur = self.bundled.lock().unwrap();
        if b.uv.is_some() {
            cur.uv = b.uv;
        }
        if b.worker_src.is_some() {
            cur.worker_src = b.worker_src;
        }
    }

    /// Test hook: interpreter path relative to the venv (lets tests use a fake `python`).
    #[doc(hidden)]
    pub fn set_python_rel(&self, rel: &str) {
        *self.python_rel.lock().unwrap() = Some(PathBuf::from(rel));
    }

    fn python_path(&self) -> PathBuf {
        match self.python_rel.lock().unwrap().as_ref() {
            Some(rel) => venv_dir(&self.root).join(rel),
            None => venv_python(&venv_dir(&self.root)),
        }
    }

    /// The `uv` to use: bundled, `IMAGEPICKER_UV`, else `uv` on `PATH` (dev machines).
    pub fn uv_path(&self) -> Option<PathBuf> {
        let b = self.bundled.lock().unwrap().uv.clone();
        b.filter(|p| p.is_file()).or_else(|| find_on_path("uv"))
    }

    /// The worker source to install from: bundled, `IMAGEPICKER_WORKER_SRC`, else the dev repo.
    pub fn worker_src(&self) -> Option<PathBuf> {
        let b = self.bundled.lock().unwrap().worker_src.clone();
        b.filter(|p| has_worker_src(p))
            .or_else(process::find_worker_dir)
            .filter(|p| has_worker_src(p))
    }

    fn marker_ok(&self) -> Option<RuntimeMarker> {
        let m = read_marker(&self.root)?;
        (m.version == self.version && self.python_path().is_file()).then_some(m)
    }

    fn can_install(&self, allow_network: bool) -> std::result::Result<(), String> {
        if !allow_network {
            return Err("network access is disabled (settings: privacy.allow_network)".into());
        }
        if self.uv_path().is_none() {
            return Err("the uv installer is missing from this build".into());
        }
        if self.worker_src().is_none() {
            return Err("the AI worker source is missing from this build".into());
        }
        Ok(())
    }

    fn effective_state(&self, inner: &Inner) -> RuntimeState {
        match inner.state {
            RuntimeState::Installing => RuntimeState::Installing,
            RuntimeState::Failed => RuntimeState::Failed,
            _ if self.marker_ok().is_some() => RuntimeState::Ready,
            _ => RuntimeState::Missing,
        }
    }

    /// `GET /api/runtime`.
    pub async fn status(&self, allow_network: bool) -> Value {
        let (state, need_size) = {
            let g = self.inner.lock().unwrap();
            let s = self.effective_state(&g);
            (s, s == RuntimeState::Ready && g.venv_bytes.is_none())
        };
        if need_size {
            let root = self.root.clone();
            let bytes = tokio::task::spawn_blocking(move || {
                dir_size(&root.join("venv")) + dir_size(&root.join("python"))
            })
            .await
            .unwrap_or(0);
            self.inner.lock().unwrap().venv_bytes = Some(bytes);
        }
        self.snapshot(allow_network, state)
    }

    fn snapshot(&self, allow_network: bool, state: RuntimeState) -> Value {
        let g = self.inner.lock().unwrap();
        let marker = read_marker(&self.root);
        let installed = marker.as_ref().filter(|m| m.version == self.version);
        let can = self.can_install(allow_network);
        let hw_extras = installed.map(|m| m.extras.clone());
        json!({
            "state": state.as_str(),
            "version": marker.as_ref().map(|m| m.version.clone()),
            "bundled_version": self.version,
            "outdated": marker.as_ref().is_some_and(|m| m.version != self.version),
            "extras": hw_extras.unwrap_or_default(),
            "python": installed.map(|m| m.python.clone()),
            "venv_bytes": if state == RuntimeState::Ready { g.venv_bytes } else { None },
            "error": g.error,
            "can_install": state != RuntimeState::Installing && can.is_ok(),
            "reason": can.err(),
            "step": g.step,
            "percent": g.percent,
            "detail": g.detail,
            "task_id": g.active.as_ref().map(|a| a.task_id.clone()),
        })
    }

    fn log_path(&self) -> PathBuf {
        self.logs_dir.join("runtime-install.log")
    }
}

/// Failure of one install step.
enum StepError {
    Cancelled,
    Failed(String),
}

impl Core {
    /// `GET /api/runtime`.
    pub async fn runtime_status(&self) -> Value {
        let net = self.settings().privacy.allow_network;
        let mut v = self.runtime.status(net).await;
        let hw = detect_hardware().await;
        v["recommended_extras"] = json!(decide_extras(&hw));
        v["hardware"] = json!(hw);
        v
    }

    fn emit_runtime(&self) {
        let net = self.settings().privacy.allow_network;
        let state = {
            let g = self.runtime.inner.lock().unwrap();
            self.runtime.effective_state(&g)
        };
        let snap = self.runtime.snapshot(net, state);
        self.events.emit(Event::RuntimeUpdated { runtime: snap });
    }

    fn runtime_progress(&self, task_id: &str, state: &str, error: Option<String>) {
        let percent = self.runtime.inner.lock().unwrap().percent;
        self.events.emit(Event::TaskProgress {
            task_id: task_id.to_string(),
            kind: "runtime_install".into(),
            done: if state == "done" { 100 } else { percent as i64 },
            total: 100,
            state: state.into(),
            error,
        });
    }

    fn runtime_set(&self, step: Option<&'static str>, update: Option<ProgressUpdate>) {
        {
            let mut g = self.runtime.inner.lock().unwrap();
            g.step = step.or(g.step);
            if let Some(u) = update {
                g.percent = g.percent.max(u.percent);
                g.detail = u.detail;
            }
        }
        self.emit_runtime();
    }

    /// `POST /api/runtime/install`: starts the installation in the background; returns the task id.
    pub async fn runtime_install(self: &Arc<Self>, req: InstallRequest) -> Result<String> {
        self.require_network("installing the AI components")?;
        if let Err(reason) = self.runtime.can_install(true) {
            return Err(CoreError::Conflict(reason));
        }
        let extras = match req.extras {
            Some(e) => validate_extras(&e).map_err(CoreError::Unprocessable)?,
            None => decide_extras(&detect_hardware().await),
        };
        let settings = self.settings();
        let mirror = match req.mirror.as_deref().unwrap_or("auto") {
            "off" => Mirror::Off,
            "cn" => Mirror::Cn,
            "auto" => match settings.models.source.as_str() {
                "hf-mirror" | "modelscope" => Mirror::Cn,
                _ => Mirror::Off,
            },
            other => {
                return Err(CoreError::Unprocessable(format!(
                    "mirror must be auto, off or cn (got {other:?})"
                )))
            }
        };
        let (task_id, cancel) = {
            let mut g = self.runtime.inner.lock().unwrap();
            if g.state == RuntimeState::Installing {
                return Err(CoreError::Conflict(
                    "the AI components are already being installed".into(),
                ));
            }
            let task_id = format!("runtime-{}", self.next_task_seq());
            let cancel = CancelToken::new();
            g.state = RuntimeState::Installing;
            g.error = None;
            g.step = Some(Step::Prepare.as_str());
            g.percent = 0;
            g.detail = "Preparing".into();
            g.extras_requested = extras.clone();
            g.venv_bytes = None;
            g.active = Some(Active {
                task_id: task_id.clone(),
                cancel: cancel.clone(),
            });
            (task_id, cancel)
        };
        self.emit_runtime();
        self.runtime_progress(&task_id, "running", None);
        let core = self.clone();
        let tid = task_id.clone();
        tokio::spawn(async move {
            // the venv is replaced: stop the worker (Windows locks its files) and invalidate it
            core.worker.kill().await;
            let _ = std::fs::remove_file(core.runtime.root.join(MARKER_FILE));
            let res = core.run_install(&extras, mirror, &cancel).await;
            let (state, err, task_state) = match res {
                Ok(()) => (RuntimeState::Ready, None, "done"),
                Err(StepError::Cancelled) => (RuntimeState::Missing, None, "cancelled"),
                Err(StepError::Failed(m)) => (RuntimeState::Failed, Some(m), "failed"),
            };
            {
                let mut g = core.runtime.inner.lock().unwrap();
                g.state = state;
                g.error = err.clone();
                g.active = None;
                if state == RuntimeState::Ready {
                    g.percent = 100;
                    g.step = Some("done");
                    g.detail = "Ready".into();
                }
            }
            // a worker that failed before the runtime existed must retry right away
            core.worker.kill().await;
            core.assistant.invalidate_info();
            core.emit_runtime();
            core.runtime_progress(&tid, task_state, err);
        });
        Ok(task_id)
    }

    /// `POST /api/runtime/cancel` (no-op when nothing is installing).
    pub fn runtime_cancel(&self) {
        if let Some(a) = self.runtime.inner.lock().unwrap().active.as_ref() {
            a.cancel.cancel();
        }
    }

    /// `DELETE /api/runtime`: stops the worker and removes the whole runtime directory.
    pub async fn runtime_remove(&self) -> Result<()> {
        {
            let g = self.runtime.inner.lock().unwrap();
            if g.state == RuntimeState::Installing {
                return Err(CoreError::Conflict(
                    "the AI components are being installed; cancel first".into(),
                ));
            }
        }
        self.worker.kill().await;
        let root = self.runtime.root.clone();
        if root.exists() {
            tokio::task::spawn_blocking(move || remove_dir_all_retry(&root))
                .await
                .map_err(|e| CoreError::Internal(e.into()))?
                .map_err(|e| {
                    CoreError::Internal(anyhow::anyhow!("cannot remove the AI runtime: {e}"))
                })?;
        }
        {
            let mut g = self.runtime.inner.lock().unwrap();
            g.state = RuntimeState::Missing;
            g.error = None;
            g.step = None;
            g.percent = 0;
            g.venv_bytes = None;
        }
        self.assistant.invalidate_info();
        self.emit_runtime();
        Ok(())
    }

    async fn run_install(
        &self,
        extras: &[String],
        mirror: Mirror,
        cancel: &CancelToken,
    ) -> std::result::Result<(), StepError> {
        let rt = &self.runtime;
        let fail = |m: String| StepError::Failed(m);
        let uv = rt
            .uv_path()
            .ok_or_else(|| fail("uv is not available".into()))?;
        let src = rt
            .worker_src()
            .ok_or_else(|| fail("the worker source is not available".into()))?;
        std::fs::create_dir_all(&rt.logs_dir).ok();
        let mut log = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(rt.log_path())
            .await
            .ok();
        if let Some(f) = log.as_mut() {
            let head = format!(
                "==== runtime install {} extras={} mirror={:?} uv={}\n",
                rt.version,
                extras.join(","),
                mirror,
                uv.display()
            );
            let _ = f.write_all(head.as_bytes()).await;
        }

        // 1. the worker source, side by side per version
        self.runtime_set(
            Some(Step::Prepare.as_str()),
            Some(ProgressUpdate {
                percent: 2,
                detail: "Copying the worker".into(),
            }),
        );
        let dst = rt.root.join("worker-src").join(&rt.version);
        {
            let (s, d) = (src.clone(), dst.clone());
            tokio::task::spawn_blocking(move || {
                let _ = std::fs::remove_dir_all(&d);
                copy_worker_source(&s, &d)
            })
            .await
            .map_err(|e| fail(e.to_string()))?
            .map_err(|e| fail(format!("cannot copy the worker source: {e}")))?;
        }

        let env = uv_env(&rt.root, mirror, &|k| std::env::var(k).ok());
        let mut parser = ProgressParser::default();

        // 2. managed Python
        self.runtime_set(
            Some(Step::Python.as_str()),
            Some(ProgressUpdate {
                percent: 5,
                detail: "Installing Python".into(),
            }),
        );
        self.run_logged(
            &uv,
            &python_install_args(),
            &rt.root,
            &env,
            Step::Python,
            &mut parser,
            cancel,
            log.as_mut(),
        )
        .await?;

        // 3. dependencies
        self.runtime_set(
            Some(Step::Sync.as_str()),
            Some(ProgressUpdate {
                percent: 20,
                detail: "Installing the AI libraries".into(),
            }),
        );
        self.run_logged(
            &uv,
            &sync_args(extras),
            &dst,
            &env,
            Step::Sync,
            &mut parser,
            cancel,
            log.as_mut(),
        )
        .await?;

        // 4. verify
        self.runtime_set(
            Some(Step::Verify.as_str()),
            Some(ProgressUpdate {
                percent: 96,
                detail: "Verifying".into(),
            }),
        );
        let py = rt.python_path();
        let python = py.to_string_lossy().into_owned();
        let mut vparser = ProgressParser::default();
        self.run_logged(
            Path::new(&python),
            &["-c".to_string(), "import imagepicker_ai".to_string()],
            &dst,
            &env,
            Step::Verify,
            &mut vparser,
            cancel,
            log.as_mut(),
        )
        .await?;
        let version_out = Command::new(&py)
            .arg("--version")
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .stdout(Stdio::piped())
            .output()
            .await
            .map(|o| {
                let mut s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if s.is_empty() {
                    s = String::from_utf8_lossy(&o.stderr).trim().to_string();
                }
                s
            })
            .unwrap_or_default();

        let marker = RuntimeMarker {
            version: rt.version.clone(),
            extras: extras.to_vec(),
            python: version_out,
            installed_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        };
        let text = serde_json::to_vec_pretty(&marker).map_err(|e| fail(e.to_string()))?;
        std::fs::write(rt.root.join(MARKER_FILE), text)
            .map_err(|e| fail(format!("cannot write the install marker: {e}")))?;
        // older worker sources are no longer needed (the install is not editable)
        if let Ok(rd) = std::fs::read_dir(rt.root.join("worker-src")) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy() != rt.version {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
        if let Some(f) = log.as_mut() {
            let _ = f.write_all(b"==== runtime install finished: ok\n").await;
        }
        Ok(())
    }

    /// Runs one process, streaming its output into the log and the progress parser.
    #[allow(clippy::too_many_arguments)]
    async fn run_logged(
        &self,
        program: &Path,
        args: &[String],
        cwd: &Path,
        env: &[(String, String)],
        step: Step,
        parser: &mut ProgressParser,
        cancel: &CancelToken,
        mut log: Option<&mut tokio::fs::File>,
    ) -> std::result::Result<(), StepError> {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(cwd)
            .envs(env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000);
        let mut child = cmd
            .spawn()
            .map_err(|e| StepError::Failed(format!("cannot start {}: {e}", program.display())))?;
        let pid = child.id();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let pipes: [Option<Box<dyn tokio::io::AsyncRead + Send + Unpin>>; 2] = [
            child.stdout.take().map(|p| Box::new(p) as _),
            child.stderr.take().map(|p| Box::new(p) as _),
        ];
        for pipe in pipes.into_iter().flatten() {
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(pipe).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    if tx.send(l).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        let mut tail: std::collections::VecDeque<String> = Default::default();
        let status = loop {
            tokio::select! {
                line = rx.recv() => match line {
                    Some(l) => {
                        if let Some(f) = log.as_deref_mut() {
                            let _ = f.write_all(format!("{l}\n").as_bytes()).await;
                        }
                        if let Some(u) = parser.feed(step, &l) {
                            self.runtime_set(None, Some(u));
                            let tid = self.runtime.inner.lock().unwrap().active.as_ref().map(|a| a.task_id.clone());
                            if let Some(t) = tid {
                                self.runtime_progress(&t, "running", None);
                            }
                        }
                        if tail.len() >= 12 {
                            tail.pop_front();
                        }
                        tail.push_back(l);
                    }
                    None => break child.wait().await,
                },
                _ = cancel.cancelled() => {
                    if let Some(p) = pid {
                        process::kill_tree(p);
                    }
                    let _ = child.kill().await;
                    if let Some(f) = log.as_deref_mut() {
                        let _ = f.write_all(b"==== cancelled\n").await;
                    }
                    return Err(StepError::Cancelled);
                }
            }
        };
        match status {
            Ok(s) if s.success() => Ok(()),
            Ok(s) => {
                let msg = format!(
                    "{} failed ({s}):\n{}",
                    step.as_str(),
                    tail.iter().cloned().collect::<Vec<_>>().join("\n")
                );
                if let Some(f) = log {
                    let _ = f.write_all(format!("==== {msg}\n").as_bytes()).await;
                }
                Err(StepError::Failed(msg))
            }
            Err(e) => Err(StepError::Failed(format!("{}: {e}", step.as_str()))),
        }
    }
}

/// Windows may keep a file busy for a moment after its process was killed.
fn remove_dir_all_retry(p: &Path) -> std::io::Result<()> {
    let mut last = None;
    for _ in 0..10 {
        match std::fs::remove_dir_all(p) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(Duration::from_millis(300));
            }
        }
    }
    Err(last.unwrap())
}

#[cfg(test)]
mod tests;
