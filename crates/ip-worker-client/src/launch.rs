//! Which command starts the AI worker. Resolution order:
//!
//! 1. `IMAGEPICKER_WORKER_CMD` (explicit override, used by tests and power users),
//! 2. the dev repo: an `ai-worker/` directory next to the working directory / executable (or
//!    `IMAGEPICKER_WORKER_DIR`) when `uv` is on `PATH`  ->  `uv run imagepicker-ai serve`,
//! 3. the installed runtime (`<data>/runtime/venv`, see `ip_core::runtime`),
//! 4. unavailable: an error tagged with [`RUNTIME_MISSING_TAG`] so the API can answer
//!    `503 worker_unavailable` with `runtime: "missing"` and the UI can offer the installation.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::process;

/// Marks a `WorkerError::Unavailable` message as "the AI runtime is not installed".
pub const RUNTIME_MISSING_TAG: &str = "[runtime:missing] ";

/// Written by the runtime installer after a verified install.
pub const MARKER_FILE: &str = "installed.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeMarker {
    /// Runtime version (app version + hash of the worker lock) the venv was synced for.
    pub version: String,
    pub extras: Vec<String>,
    /// `python --version` of the venv.
    pub python: String,
    /// Seconds since the Unix epoch.
    pub installed_at: u64,
}

/// Where the installed runtime lives and which version this build expects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeLaunch {
    /// `<data>/runtime`
    pub root: PathBuf,
    pub version: String,
}

/// `<venv>/Scripts/python.exe` or `<venv>/bin/python`.
pub fn venv_python(venv: &Path) -> PathBuf {
    if cfg!(windows) {
        venv.join("Scripts").join("python.exe")
    } else {
        venv.join("bin").join("python")
    }
}

pub fn venv_dir(root: &Path) -> PathBuf {
    root.join("venv")
}

pub fn read_marker(root: &Path) -> Option<RuntimeMarker> {
    let text = std::fs::read_to_string(root.join(MARKER_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

/// The venv's interpreter when a runtime of exactly `version` is installed.
pub fn installed_python(rt: &RuntimeLaunch) -> Option<PathBuf> {
    let marker = read_marker(&rt.root)?;
    if marker.version != rt.version {
        return None;
    }
    let py = venv_python(&venv_dir(&rt.root));
    py.is_file().then_some(py)
}

/// Absolute path of `name` on `PATH` (also `name.exe` on Windows).
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    which_in(name, &path)
}

pub fn which_in(name: &str, path: &std::ffi::OsStr) -> Option<PathBuf> {
    for dir in std::env::split_paths(path) {
        let plain = dir.join(name);
        if plain.is_file() {
            return Some(plain);
        }
        if cfg!(windows) {
            let exe = dir.join(format!("{name}.exe"));
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchSource {
    Override,
    Dev,
    Runtime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub source: LaunchSource,
    pub argv: Vec<String>,
    pub dir: Option<PathBuf>,
}

/// Everything resolution depends on, so it is testable without touching the environment.
pub struct LaunchInputs<'a> {
    pub cmd_override: Option<&'a str>,
    /// Dev worker project dir (`IMAGEPICKER_WORKER_DIR` or found next to cwd/exe).
    pub dev_dir: Option<PathBuf>,
    pub uv_on_path: bool,
    pub runtime: Option<&'a RuntimeLaunch>,
}

pub fn resolve_launch(
    inputs: &LaunchInputs<'_>,
    token: &str,
    parent_pid: u32,
    models_dir: Option<&Path>,
) -> Result<Launch, String> {
    if let Some(cmd) = inputs.cmd_override.filter(|s| !s.trim().is_empty()) {
        return Ok(Launch {
            source: LaunchSource::Override,
            argv: process::build_argv(Some(cmd), token, parent_pid, models_dir),
            dir: inputs.dev_dir.clone(),
        });
    }
    if let (Some(dir), true) = (&inputs.dev_dir, inputs.uv_on_path) {
        return Ok(Launch {
            source: LaunchSource::Dev,
            argv: process::build_argv(None, token, parent_pid, models_dir),
            dir: Some(dir.clone()),
        });
    }
    if let Some(rt) = inputs.runtime {
        if let Some(py) = installed_python(rt) {
            let mut argv = vec![
                py.to_string_lossy().into_owned(),
                "-m".into(),
                "imagepicker_ai".into(),
            ];
            argv.extend(process::serve_args(token, parent_pid, models_dir));
            return Ok(Launch {
                source: LaunchSource::Runtime,
                argv,
                dir: Some(rt.root.clone()),
            });
        }
    }
    Err(format!(
        "{RUNTIME_MISSING_TAG}the AI components are not installed (not found): install them from \
         Settings > Hardware and models, or set IMAGEPICKER_WORKER_CMD / IMAGEPICKER_WORKER_DIR"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(root: &Path, version: &str) {
        let py = venv_python(&venv_dir(root));
        std::fs::create_dir_all(py.parent().unwrap()).unwrap();
        std::fs::write(&py, b"").unwrap();
        let m = RuntimeMarker {
            version: version.into(),
            extras: vec!["cpu".into()],
            python: "Python 3.12.1".into(),
            installed_at: 1,
        };
        std::fs::write(root.join(MARKER_FILE), serde_json::to_vec(&m).unwrap()).unwrap();
    }

    fn inputs<'a>(
        cmd: Option<&'a str>,
        dev: bool,
        uv: bool,
        rt: Option<&'a RuntimeLaunch>,
    ) -> LaunchInputs<'a> {
        LaunchInputs {
            cmd_override: cmd,
            dev_dir: dev.then(|| PathBuf::from("/repo/ai-worker")),
            uv_on_path: uv,
            runtime: rt,
        }
    }

    #[test]
    fn override_beats_everything() {
        let tmp = tempfile::tempdir().unwrap();
        installed(tmp.path(), "1");
        let rt = RuntimeLaunch {
            root: tmp.path().into(),
            version: "1".into(),
        };
        let l = resolve_launch(
            &inputs(Some("fake-worker"), true, true, Some(&rt)),
            "T",
            1,
            None,
        )
        .unwrap();
        assert_eq!(l.source, LaunchSource::Override);
        assert_eq!(l.argv[0], "fake-worker");
    }

    #[test]
    fn dev_repo_needs_uv_then_runtime() {
        let tmp = tempfile::tempdir().unwrap();
        installed(tmp.path(), "1");
        let rt = RuntimeLaunch {
            root: tmp.path().into(),
            version: "1".into(),
        };
        let l = resolve_launch(&inputs(None, true, true, Some(&rt)), "T", 1, None).unwrap();
        assert_eq!(l.source, LaunchSource::Dev);
        assert_eq!(&l.argv[..3], ["uv", "run", "imagepicker-ai"]);
        // dev repo without uv on PATH falls through to the installed runtime
        let l = resolve_launch(&inputs(None, true, false, Some(&rt)), "T", 1, None).unwrap();
        assert_eq!(l.source, LaunchSource::Runtime);
        assert!(l.argv[0].contains("python"));
        assert_eq!(&l.argv[1..3], ["-m", "imagepicker_ai"]);
        assert_eq!(l.argv[3], "serve");
        assert!(l.argv.windows(2).any(|w| w == ["--token", "T"]));
        assert_eq!(l.dir.as_deref(), Some(tmp.path()));
        // no dev repo at all
        let l = resolve_launch(&inputs(None, false, true, Some(&rt)), "T", 1, None).unwrap();
        assert_eq!(l.source, LaunchSource::Runtime);
    }

    #[test]
    fn outdated_or_missing_runtime_is_unavailable_with_hint() {
        let tmp = tempfile::tempdir().unwrap();
        let rt = RuntimeLaunch {
            root: tmp.path().into(),
            version: "2".into(),
        };
        let e = resolve_launch(&inputs(None, false, true, Some(&rt)), "T", 1, None).unwrap_err();
        assert!(e.starts_with(RUNTIME_MISSING_TAG));
        assert!(e.contains("not found"));
        installed(tmp.path(), "1"); // other version
        assert!(resolve_launch(&inputs(None, false, true, Some(&rt)), "T", 1, None).is_err());
        assert!(resolve_launch(&inputs(None, false, false, None), "T", 1, None).is_err());
    }

    #[test]
    fn which_finds_executables() {
        let tmp = tempfile::tempdir().unwrap();
        let name = if cfg!(windows) { "myuv.exe" } else { "myuv" };
        std::fs::write(tmp.path().join(name), b"").unwrap();
        let path = std::env::join_paths([tmp.path()]).unwrap();
        assert!(which_in("myuv", &path).is_some());
        assert!(which_in("nope", &path).is_none());
    }
}
