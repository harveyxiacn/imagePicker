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
    // Only in debug builds: a release build must not depend on the machine it was built on.
    if cfg!(debug_assertions) {
        let built_from = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ai-worker");
        return has_worker(&built_from).then_some(built_from);
    }
    None
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
    if cfg!(target_os = "android") {
        // no Python worker on phones (see `UnavailableWorker`)
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "the AI worker cannot run on Android",
        ));
    }
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
    #[cfg(all(unix, not(target_os = "android")))]
    cmd.process_group(0);
    cmd.spawn()
}

/// Kills `pid` and all its descendants (the `uv` launcher shim spawns the real Python child).
pub fn kill_tree(pid: u32) {
    #[cfg(target_os = "android")]
    let _ = pid; // no worker process is ever spawned on Android
    #[cfg(windows)]
    {
        // `taskkill` / `tasklist` answer "access denied" after ~5 s for some restricted accounts
        // (seen over OpenSSH), even for their own children, and the worker lives on. Kill
        // natively; `taskkill` stays as the fallback when the root survives.
        if !win::kill_tree(pid) {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW)
                .status();
        }
    }
    #[cfg(all(unix, not(target_os = "android")))]
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
        win::pid_alive(pid)
    }
    #[cfg(target_os = "android")]
    {
        let _ = pid;
        false
    }
    #[cfg(all(unix, not(target_os = "android")))]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

/// Process liveness and tree kill through kernel32 directly (no `tasklist` / `taskkill`).
#[cfg(windows)]
mod win {
    use std::collections::HashMap;
    use std::ffi::c_void;

    type Handle = *mut c_void;

    const PROCESS_TERMINATE: u32 = 0x0001;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    const WAIT_TIMEOUT: u32 = 0x0000_0102;
    const ERROR_ACCESS_DENIED: u32 = 5;
    const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;

    /// `PROCESSENTRY32W`; most fields only exist for the layout.
    #[repr(C)]
    #[allow(dead_code)]
    struct ProcessEntry32W {
        dw_size: u32,
        cnt_usage: u32,
        th32_process_id: u32,
        th32_default_heap_id: usize,
        th32_module_id: u32,
        cnt_threads: u32,
        th32_parent_process_id: u32,
        pc_pri_class_base: i32,
        dw_flags: u32,
        sz_exe_file: [u16; 260],
    }

    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
        fn CloseHandle(h: Handle) -> i32;
        fn GetLastError() -> u32;
        fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
        fn TerminateProcess(h: Handle, code: u32) -> i32;
        fn GetProcessTimes(
            h: Handle,
            created: *mut u64,
            exited: *mut u64,
            kernel: *mut u64,
            user: *mut u64,
        ) -> i32;
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
        fn Process32FirstW(snap: Handle, entry: *mut ProcessEntry32W) -> i32;
        fn Process32NextW(snap: Handle, entry: *mut ProcessEntry32W) -> i32;
    }

    /// Closes the handle on drop.
    struct Owned(Handle);

    impl Drop for Owned {
        fn drop(&mut self) {
            // SAFETY: `self.0` is a valid handle owned by this value and closed exactly once.
            unsafe { CloseHandle(self.0) };
        }
    }

    fn open(access: u32, pid: u32) -> Option<Owned> {
        // SAFETY: plain call; a null result means failure and is never wrapped.
        let h = unsafe { OpenProcess(access, 0, pid) };
        (!h.is_null()).then_some(Owned(h))
    }

    pub fn pid_alive(pid: u32) -> bool {
        match open(SYNCHRONIZE, pid) {
            // A process handle is signalled once the process has exited.
            Some(h) => {
                // SAFETY: `h` is a valid process handle opened with SYNCHRONIZE.
                let r = unsafe { WaitForSingleObject(h.0, 0) };
                r == WAIT_TIMEOUT
            }
            // It exists, it just belongs to someone we may not open.
            None => {
                // SAFETY: plain call, reads this thread's last error from the failed OpenProcess.
                let e = unsafe { GetLastError() };
                e == ERROR_ACCESS_DENIED
            }
        }
    }

    /// Creation time (100 ns ticks) of a running process.
    fn created(pid: u32) -> Option<u64> {
        let h = open(PROCESS_QUERY_LIMITED_INFORMATION, pid)?;
        let (mut c, mut e, mut k, mut u) = (0u64, 0u64, 0u64, 0u64);
        // SAFETY: `h` is valid; the four out-pointers are live, 8-byte aligned FILETIME-sized slots.
        let ok = unsafe { GetProcessTimes(h.0, &mut c, &mut e, &mut k, &mut u) };
        (ok != 0).then_some(c)
    }

    /// `(pid, parent pid)` of every process.
    fn snapshot() -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        // SAFETY: plain call; the result is checked before use.
        let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snap == INVALID_HANDLE_VALUE || snap.is_null() {
            return out;
        }
        let snap = Owned(snap);
        // SAFETY: all-zero is a valid bit pattern for this plain-data struct.
        let mut e: ProcessEntry32W = unsafe { std::mem::zeroed() };
        e.dw_size = std::mem::size_of::<ProcessEntry32W>() as u32;
        // SAFETY: `snap` is a valid snapshot handle and `e` a live entry with `dw_size` set.
        let mut more = unsafe { Process32FirstW(snap.0, &mut e) } != 0;
        while more {
            out.push((e.th32_process_id, e.th32_parent_process_id));
            // SAFETY: as above.
            more = unsafe { Process32NextW(snap.0, &mut e) } != 0;
        }
        out
    }

    #[cfg(test)]
    pub fn children_of(pid: u32) -> Vec<u32> {
        snapshot()
            .into_iter()
            .filter(|&(p, parent)| parent == pid && p != pid)
            .map(|(p, _)| p)
            .collect()
    }

    fn terminate(pid: u32) -> bool {
        match open(PROCESS_TERMINATE, pid) {
            Some(h) => {
                // SAFETY: `h` is a valid process handle opened with PROCESS_TERMINATE.
                let ok = unsafe { TerminateProcess(h.0, 1) };
                ok != 0 || !pid_alive(pid)
            }
            None => !pid_alive(pid),
        }
    }

    /// Terminates `pid` and its descendants, children first. False when `pid` itself survives.
    pub fn kill_tree(pid: u32) -> bool {
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for (p, parent) in snapshot() {
            if p != parent {
                children.entry(parent).or_default().push(p);
            }
        }
        // Windows keeps the parent pid of orphans and reuses pids, so a "child" that started
        // before its parent is a stranger that inherited a recycled pid.
        let mut tree = vec![pid];
        let mut i = 0;
        while i < tree.len() {
            let born = created(tree[i]);
            for &c in children.get(&tree[i]).into_iter().flatten() {
                let stranger = matches!((born, created(c)), (Some(b), Some(cb)) if cb < b);
                if !stranger && !tree.contains(&c) {
                    tree.push(c);
                }
            }
            i += 1;
        }
        let mut root_gone = false;
        for &p in tree.iter().rev() {
            let gone = terminate(p);
            if p == pid {
                root_gone = gone;
            }
        }
        root_gone
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

    #[cfg(windows)]
    #[test]
    fn native_liveness_and_tree_kill() {
        use std::os::windows::process::CommandExt;
        assert!(pid_alive(std::process::id()));
        // cmd -> ping: a parent with a child, like the uv shim and its Python
        let mut parent = std::process::Command::new("cmd")
            .args(["/C", "ping -n 30 127.0.0.1 >NUL"])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .unwrap();
        let pid = parent.id();
        let child = (0..100)
            .find_map(|_| {
                std::thread::sleep(std::time::Duration::from_millis(50));
                win::children_of(pid).first().copied()
            })
            .expect("cmd never started ping");
        assert!(pid_alive(pid) && pid_alive(child));
        kill_tree(pid);
        parent.wait().unwrap();
        assert!(!pid_alive(pid));
        let gone = (0..100).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(50));
            !pid_alive(child)
        });
        assert!(gone, "child {child} survived kill_tree");
    }
}
