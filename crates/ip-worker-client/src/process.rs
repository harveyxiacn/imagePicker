//! Spawning the worker process and killing its whole process tree.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::{Child, Command};

pub const ENV_WORKER_CMD: &str = "IMAGEPICKER_WORKER_CMD";
pub const ENV_WORKER_DIR: &str = "IMAGEPICKER_WORKER_DIR";

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Splits a command line into arguments (double/single quotes group, no escapes except `\"` in
/// double quotes). Enough for `IMAGEPICKER_WORKER_CMD`.
pub fn split_command_line(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') if chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            (Some(_), c) => cur.push(c),
            (None, '"') | (None, '\'') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            (None, c) => {
                cur.push(c);
                started = true;
            }
        }
    }
    if started || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Arguments appended to a command that has no `{token}` placeholder.
pub fn serve_args(token: &str, parent_pid: u32, models_dir: Option<&Path>) -> Vec<String> {
    let mut a: Vec<String> = [
        "serve",
        "--host",
        "127.0.0.1",
        "--port",
        "0",
        "--token",
        token,
        "--parent-pid",
        &parent_pid.to_string(),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if let Some(d) = models_dir {
        a.push("--models-dir".into());
        a.push(d.to_string_lossy().into_owned());
    }
    a
}

/// Resolves the argv of the worker. `cmd_override` is `IMAGEPICKER_WORKER_CMD`: if it contains
/// `{token}` it is used verbatim (`{token}` and `{parent_pid}` substituted), otherwise the
/// `serve ...` arguments are appended to it. Without an override: `uv run imagepicker-ai serve ...`.
pub fn build_argv(
    cmd_override: Option<&str>,
    token: &str,
    parent_pid: u32,
    models_dir: Option<&Path>,
) -> Vec<String> {
    match cmd_override.filter(|s| !s.trim().is_empty()) {
        Some(cmd) => {
            let mut argv = split_command_line(cmd);
            if cmd.contains("{token}") {
                for a in &mut argv {
                    *a = a
                        .replace("{token}", token)
                        .replace("{parent_pid}", &parent_pid.to_string());
                }
            } else {
                argv.extend(serve_args(token, parent_pid, models_dir));
            }
            argv
        }
        None => {
            let mut argv: Vec<String> = ["uv", "run", "imagepicker-ai"]
                .iter()
                .map(|s| s.to_string())
                .collect();
            argv.extend(serve_args(token, parent_pid, models_dir));
            argv
        }
    }
}

fn has_worker(dir: &Path) -> bool {
    dir.join("pyproject.toml").is_file() && dir.join("imagepicker_ai").is_dir()
}

/// Finds the `ai-worker` directory: `IMAGEPICKER_WORKER_DIR`, then `ai-worker/` next to the
/// current directory or the executable (searching upwards), then the source tree this was built from.
pub fn find_worker_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os(ENV_WORKER_DIR).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(d));
    }
    let mut starts: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        starts.push(cwd);
    }
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        starts.push(exe_dir);
    }
    for s in starts {
        for anc in s.ancestors() {
            let cand = anc.join("ai-worker");
            if has_worker(&cand) {
                return Some(cand);
            }
        }
    }
    let built_from = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ai-worker");
    has_worker(&built_from).then_some(built_from)
}

/// Spawns the worker with piped stdout/stderr.
pub fn spawn(argv: &[String], dir: Option<&Path>, token: &str) -> std::io::Result<Child> {
    spawn_with_env(argv, dir, token, &[])
}

/// [`spawn`] with extra environment variables (model source, offline mode).
pub fn spawn_with_env(
    argv: &[String],
    dir: Option<&Path>,
    token: &str,
    env: &[(String, String)],
) -> std::io::Result<Child> {
    let (prog, args) = argv
        .split_first()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty command"))?;
    let mut cmd = Command::new(prog);
    cmd.args(args)
        .env("IP_WORKER_TOKEN", token)
        .env("PYTHONUNBUFFERED", "1")
        .env("PYTHONUTF8", "1")
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(d) = dir {
        cmd.current_dir(d);
    }
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    #[cfg(unix)]
    cmd.process_group(0);
    cmd.spawn()
}

/// Kills `pid` and all its descendants (the `uv` launcher shim spawns the real Python child).
pub fn kill_tree(pid: u32) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status();
    }
    #[cfg(unix)]
    {
        // The worker leads its own process group (see `spawn`).
        let _ = std::process::Command::new("kill")
            .args(["-KILL", "--", &format!("-{pid}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// True while a process with this pid exists.
pub fn pid_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let out = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .output();
        out.map(|o| String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\"")))
            .unwrap_or(false)
    }
    #[cfg(unix)]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_quotes() {
        assert_eq!(
            split_command_line(r#"python -m "my mod" 'a b' x"#),
            vec!["python", "-m", "my mod", "a b", "x"]
        );
        assert_eq!(split_command_line("  "), Vec::<String>::new());
        assert_eq!(split_command_line(r#"a "" b"#), vec!["a", "", "b"]);
    }

    #[test]
    fn default_argv_uses_uv() {
        let a = build_argv(None, "T", 42, None);
        assert_eq!(&a[..3], ["uv", "run", "imagepicker-ai"]);
        assert!(a.windows(2).any(|w| w == ["--token", "T"]));
        assert!(a.windows(2).any(|w| w == ["--parent-pid", "42"]));
        assert!(a.windows(2).any(|w| w == ["--port", "0"]));
    }

    #[test]
    fn override_appends_serve_args_unless_placeholder() {
        let a = build_argv(Some("python -m imagepicker_ai"), "T", 1, None);
        assert_eq!(&a[..3], ["python", "-m", "imagepicker_ai"]);
        assert_eq!(a[3], "serve");
        let b = build_argv(
            Some("fake --token {token} --parent {parent_pid}"),
            "SECRET",
            9,
            None,
        );
        assert_eq!(b, ["fake", "--token", "SECRET", "--parent", "9"]);
    }

    #[test]
    fn models_dir_is_forwarded() {
        let a = build_argv(None, "T", 1, Some(Path::new("/m")));
        assert!(a.windows(2).any(|w| w[0] == "--models-dir"));
    }
}
