//! Log files, crash records and the scrubbing of both (gap P1-13, `docs/api-contract-m6.md`
//! section G).
//!
//! * [`init`] sets up `tracing`: stderr (optional) plus rotating files under `<data>/logs`.
//! * [`install_panic_hook`] writes a crash file under `<data>/crashes` *before* the default hook
//!   runs. The AI worker writes its own files there (Python exceptions, `faulthandler`), and the
//!   worker client records the stderr tail when the process dies ([`write_crash`]).
//! * [`scrub`] removes photo paths and secrets from text; the diagnostics bundle applies it to
//!   everything it copies out of the data directory.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use regex::Regex;

use crate::paths::{resolve_data_dir, DataDirs};

pub const LOG_PREFIX: &str = "imagepicker-";
pub const LOG_SUFFIX: &str = ".log";
/// A log file is closed and a new one started once it reaches this size.
pub const LOG_FILE_BYTES: u64 = 5 * 1024 * 1024;
/// All log files together are trimmed (oldest first) to this size.
pub const LOG_TOTAL_BYTES: u64 = 40 * 1024 * 1024;
/// Log files older than this are deleted.
pub const LOG_MAX_AGE: Duration = Duration::from_secs(14 * 24 * 3600);
/// Number of crash records kept (oldest are deleted).
pub const CRASH_KEEP: usize = 30;

// ------------------------------------------------------------------ time

/// `(year, month, day)` of a count of days since 1970-01-01 (proleptic Gregorian, UTC).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// UTC broken-down time of `t`: `(year, month, day, hour, minute, second)`.
pub fn utc_parts(t: SystemTime) -> (i64, u32, u32, u32, u32, u32) {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    (
        y,
        m,
        d,
        (rem / 3600) as u32,
        (rem % 3600 / 60) as u32,
        (rem % 60) as u32,
    )
}

/// `2026-10-13T12:00:00Z`.
pub fn iso_utc(t: SystemTime) -> String {
    let (y, mo, d, h, mi, s) = utc_parts(t);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// `20261013-120000`.
pub fn stamp_utc(t: SystemTime) -> String {
    let (y, mo, d, h, mi, s) = utc_parts(t);
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}")
}

fn day_utc(t: SystemTime) -> String {
    let (y, mo, d, ..) = utc_parts(t);
    format!("{y:04}{mo:02}{d:02}")
}

// ------------------------------------------------------------------ rotating log files

#[derive(Debug, Clone, Copy)]
pub struct LogLimits {
    pub file_bytes: u64,
    pub total_bytes: u64,
    pub max_age: Duration,
}

impl Default for LogLimits {
    fn default() -> Self {
        Self {
            file_bytes: LOG_FILE_BYTES,
            total_bytes: LOG_TOTAL_BYTES,
            max_age: LOG_MAX_AGE,
        }
    }
}

type Clock = Box<dyn Fn() -> SystemTime + Send + Sync>;

struct Current {
    file: File,
    name: String,
    day: String,
    seq: u32,
    size: u64,
}

/// `<dir>/imagepicker-YYYYMMDD.N.log`: a new file per UTC day and whenever one reaches
/// [`LogLimits::file_bytes`]; old files are deleted by age and by total size. Write errors never
/// surface (logging must not take the application down).
pub struct RotatingLog {
    dir: PathBuf,
    limits: LogLimits,
    now: Clock,
    cur: Mutex<Option<Current>>,
}

fn log_name(day: &str, seq: u32) -> String {
    format!("{LOG_PREFIX}{day}.{seq}{LOG_SUFFIX}")
}

/// `(day, seq)` of a log file name.
fn parse_log_name(name: &str) -> Option<(String, u32)> {
    let rest = name.strip_prefix(LOG_PREFIX)?.strip_suffix(LOG_SUFFIX)?;
    let (day, seq) = rest.split_once('.')?;
    (day.len() == 8 && day.bytes().all(|b| b.is_ascii_digit())).then_some(())?;
    Some((day.to_string(), seq.parse().ok()?))
}

impl RotatingLog {
    pub fn new(dir: PathBuf) -> Self {
        Self::with(dir, LogLimits::default(), Box::new(SystemTime::now))
    }

    pub fn with(dir: PathBuf, limits: LogLimits, now: Clock) -> Self {
        Self {
            dir,
            limits,
            now,
            cur: Mutex::new(None),
        }
    }

    fn open(&self, day: &str, seq_after: Option<u32>) -> Option<Current> {
        fs::create_dir_all(&self.dir).ok()?;
        // continue the newest file of the day (a restart) unless it is full
        let newest = fs::read_dir(&self.dir)
            .ok()?
            .flatten()
            .filter_map(|e| parse_log_name(&e.file_name().to_string_lossy()))
            .filter(|(d, _)| d == day)
            .map(|(_, s)| s)
            .max();
        let mut seq = match (newest, seq_after) {
            (Some(n), Some(a)) => n.max(a + 1),
            (Some(n), None) => n,
            (None, Some(a)) => a + 1,
            (None, None) => 0,
        };
        loop {
            let name = log_name(day, seq);
            let path = self.dir.join(&name);
            let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if size >= self.limits.file_bytes {
                seq += 1;
                continue;
            }
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .ok()?;
            return Some(Current {
                file,
                name,
                day: day.to_string(),
                seq,
                size,
            });
        }
    }

    /// Deletes files older than `max_age`, then the oldest until the total fits.
    pub fn prune(&self, keep: &str) {
        let now = (self.now)();
        let Ok(rd) = fs::read_dir(&self.dir) else {
            return;
        };
        let mut files: Vec<(SystemTime, u64, PathBuf, String)> = rd
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                parse_log_name(&name)?;
                let md = e.metadata().ok()?;
                Some((md.modified().ok()?, md.len(), e.path(), name))
            })
            .collect();
        files.sort_by_key(|f| f.0);
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        for (mtime, size, path, name) in files {
            if name == keep {
                continue;
            }
            let old = now
                .duration_since(mtime)
                .is_ok_and(|age| age > self.limits.max_age);
            if (old || total > self.limits.total_bytes) && fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(size);
            }
        }
    }

    fn write_inner(&self, buf: &[u8]) {
        let now = (self.now)();
        let day = day_utc(now);
        let mut guard = self.cur.lock().unwrap_or_else(|e| e.into_inner());
        let rotate = match guard.as_ref() {
            None => Some(None),
            Some(c) if c.day != day => Some(None),
            Some(c) if c.size > 0 && c.size + buf.len() as u64 > self.limits.file_bytes => {
                Some(Some(c.seq))
            }
            Some(_) => None,
        };
        if let Some(after) = rotate {
            *guard = self.open(&day, after);
            if let Some(c) = guard.as_ref() {
                self.prune(&c.name);
            }
        }
        if let Some(c) = guard.as_mut() {
            if c.file.write_all(buf).is_ok() {
                c.size += buf.len() as u64;
            }
        }
    }

    /// Name of the file currently written to.
    pub fn current_name(&self) -> Option<String> {
        self.cur
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|c| c.name.clone())
    }
}

impl Write for &RotatingLog {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.write_inner(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(c) = self.cur.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            let _ = c.file.flush();
        }
        Ok(())
    }
}

/// Starts logging: `RUST_LOG` (or `default_filter`) to stderr when `console` and to rotating
/// files under `<data dir>/logs`; installs the crash hook for `<data dir>/crashes`. Safe to call
/// when a subscriber already exists (the existing one stays). Returns the log directory.
pub fn init(data_dir: Option<&Path>, default_filter: &str, console: bool) -> PathBuf {
    use tracing_subscriber::prelude::*;
    let dirs = DataDirs::new(resolve_data_dir(data_dir));
    let _ = fs::create_dir_all(&dirs.logs);
    let _ = fs::create_dir_all(&dirs.crashes);
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_filter));
    let file = Arc::new(RotatingLog::new(dirs.logs.clone()));
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(console.then(tracing_subscriber::fmt::layer))
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(file),
        )
        .try_init();
    install_panic_hook(dirs.crashes);
    dirs.logs
}

// ------------------------------------------------------------------ crash records

#[derive(Debug, Clone, Default)]
pub struct CrashReport {
    /// `rust-panic`, `worker-exit`, `worker-start-failed`.
    pub kind: String,
    pub message: String,
    pub location: Option<String>,
    pub thread: Option<String>,
    /// Backtrace, or the stderr tail of the worker.
    pub detail: String,
}

static CRASH_SEQ: AtomicU64 = AtomicU64::new(0);

/// Writes `<dir>/crash-<utc stamp>-<pid>-<n>.txt` and trims the folder to [`CRASH_KEEP`] files.
pub fn write_crash(dir: &Path, r: &CrashReport) -> std::io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let now = SystemTime::now();
    let name = format!(
        "crash-{}-{}-{}.txt",
        stamp_utc(now),
        std::process::id(),
        CRASH_SEQ.fetch_add(1, Ordering::SeqCst)
    );
    let path = dir.join(name);
    let mut text = String::new();
    text.push_str("imagePicker crash record\n");
    text.push_str(&format!("kind: {}\n", r.kind));
    text.push_str(&format!("time: {}\n", iso_utc(now)));
    text.push_str(&format!("version: {}\n", env!("CARGO_PKG_VERSION")));
    text.push_str(&format!(
        "os: {} {}\n",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    text.push_str(&format!("pid: {}\n", std::process::id()));
    if let Some(t) = &r.thread {
        text.push_str(&format!("thread: {t}\n"));
    }
    if let Some(l) = &r.location {
        text.push_str(&format!("location: {l}\n"));
    }
    text.push_str(&format!("message: {}\n\n", r.message));
    text.push_str(&r.detail);
    if !text.ends_with('\n') {
        text.push('\n');
    }
    fs::write(&path, text)?;
    prune_crashes(dir, CRASH_KEEP);
    Ok(path)
}

fn prune_crashes(dir: &Path, keep: usize) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut files: Vec<(SystemTime, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let md = e.metadata().ok()?;
            md.is_file()
                .then(|| (md.modified().unwrap_or(UNIX_EPOCH), e.path()))
        })
        .collect();
    if files.len() <= keep {
        return;
    }
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    for (_, p) in files.into_iter().skip(keep) {
        let _ = fs::remove_file(p);
    }
}

static CRASH_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);
static HOOK: Once = Once::new();

/// Records every panic of the process under `dir` (see [`write_crash`]) and then runs the hook
/// that was installed before (the default one prints the message). Calling it again only
/// changes the directory.
pub fn install_panic_hook(dir: PathBuf) {
    *CRASH_DIR.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir);
    HOOK.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            record_panic(info);
            prev(info);
        }));
    });
}

fn record_panic(info: &std::panic::PanicHookInfo<'_>) {
    let payload = info.payload();
    let message = payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic with a non-text payload".into());
    let thread = std::thread::current();
    let thread = format!(
        "{} ({:?})",
        thread.name().unwrap_or("<unnamed>"),
        thread.id()
    );
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
    tracing::error!(%thread, location = location.as_deref().unwrap_or("?"), "panic: {message}");
    let dir = CRASH_DIR.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(dir) = dir {
        let _ = write_crash(
            &dir,
            &CrashReport {
                kind: "rust-panic".into(),
                message,
                location,
                thread: Some(thread),
                detail: format!("backtrace:\n{}", std::backtrace::Backtrace::force_capture()),
            },
        );
    }
}

// ------------------------------------------------------------------ scrubbing

const IMAGE_EXTS: &str = "jpe?g|png|webp|tiff?|heic|heif|avif|gif|bmp|jxl|xmp|raw|cr2|cr3|crw|nef|nrw|arw|srf|sr2|dng|orf|rw2|raf|pef|srw|x3f|3fr|erf|mef|mrw|kdc|dcr|rwl|iiq";

struct ScrubRules {
    win_path: Regex,
    unc_path: Regex,
    unix_path: Regex,
    photo_name: Regex,
    url_user: Regex,
    secret_kv: Regex,
    bearer: Regex,
}

fn rules() -> &'static ScrubRules {
    static R: OnceLock<ScrubRules> = OnceLock::new();
    R.get_or_init(|| ScrubRules {
        // `C:\dir\sub\file` or `C:/dir/file`; spaces are allowed inside the path
        win_path: Regex::new(r#"[A-Za-z]:[\\/](?:[^\\/\r\n"'<>|*?]+[\\/])*[^\\/\r\n"'<>|*?]*"#)
            .unwrap(),
        unc_path: Regex::new(r#"\\\\[^\\/\s"'<>|*?]+(?:\\[^\\/\r\n"'<>|*?]+)+"#).unwrap(),
        unix_path: Regex::new(
            r#"/(?:home|Users|root|mnt|media|Volumes|storage|sdcard|data|tmp|var|run|opt|private|srv|net)(?:/[^/\r\n"'<>|*?:,;()\[\]{}]+)+"#,
        )
        .unwrap(),
        photo_name: Regex::new(&format!(
            r#"(?i)[^\\/\r\n"'<>|*?]*?\.(?:{IMAGE_EXTS})\b"#
        ))
        .unwrap(),
        url_user: Regex::new(r"(?i)([a-z][a-z0-9+.-]*://)[^/\s@]+@").unwrap(),
        secret_kv: Regex::new(
            r#"(?i)\b([a-z0-9_.-]*(?:password|passwd|secret|token|api[_-]?key|authorization|cookie|credential)[a-z0-9_.-]*)(["']?\s*[=:]\s*)("[^"]*"|'[^']*'|[^\s,;&}\]]+)"#,
        )
        .unwrap(),
        bearer: Regex::new(r"(?i)\b(bearer|basic)\s+[A-Za-z0-9._~+/=-]{8,}").unwrap(),
    })
}

/// A path found in text: the directories become `<path>`, the last segment is kept unless it
/// names an image / sidecar file (then `<photo>`).
fn mask_path(m: &str) -> String {
    let sep = |c: char| c == '/' || c == '\\';
    let trimmed = m.trim_end_matches(sep);
    let (_, last) = trimmed.rsplit_once(sep).unwrap_or(("", trimmed));
    let last = match rules().photo_name.find(last) {
        Some(f) => format!("<photo>{}", &last[f.end()..]),
        None => last.to_string(),
    };
    format!("<path>/{last}")
}

/// Removes what must not leave the machine from log / crash text: absolute paths (directories
/// are dropped, image file names replaced by `<photo>`), credentials in URLs, `password=...`
/// style values and bearer tokens.
pub fn scrub(text: &str) -> String {
    let r = rules();
    let s = r.bearer.replace_all(text, "${1} [redacted]");
    let s = r.url_user.replace_all(&s, "${1}[redacted]@");
    let s = r.secret_kv.replace_all(&s, "${1}${2}[redacted]");
    let s = r
        .win_path
        .replace_all(&s, |c: &regex::Captures| mask_path(&c[0]));
    let s = r
        .unc_path
        .replace_all(&s, |c: &regex::Captures| mask_path(&c[0]));
    let s = r
        .unix_path
        .replace_all(&s, |c: &regex::Captures| mask_path(&c[0]));
    s.into_owned()
}

/// The final path component of a Windows or POSIX path (a root folder's display name).
pub fn base_name(path: &str) -> String {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_correct() {
        let t = UNIX_EPOCH + Duration::from_secs(1_791_892_245);
        assert_eq!(iso_utc(t), "2026-10-13T11:50:45Z");
        assert_eq!(stamp_utc(UNIX_EPOCH), "19700101-000000");
        // leap day
        let t = UNIX_EPOCH + Duration::from_secs(1_709_164_800);
        assert_eq!(day_utc(t), "20240229");
    }

    #[test]
    fn scrub_removes_paths_and_secrets() {
        let s = scrub(r"open C:\Users\Jo Doe\Pictures\2024\IMG 0001.CR2 failed: code 5");
        assert!(!s.contains("Jo Doe") && !s.contains("IMG 0001"), "{s}");
        assert!(s.contains("<photo>") && s.contains("failed"), "{s}");
        let s = scrub("original file is missing: /home/ann/photos/trip/a.jpg");
        assert!(!s.contains("ann") && !s.contains("a.jpg"), "{s}");
        assert!(s.contains("<path>/<photo>"), "{s}");
        let s = scrub("at /home/runner/work/imagePicker/src/main.rs:12:5");
        assert!(s.contains("<path>/main.rs:12:5"), "{s}");
        // API routes are not file paths
        assert_eq!(scrub("GET /api/thumb/3 200"), "GET /api/thumb/3 200");
        let s = scrub(
            r#"login {"password":"hunter2"} token=abc123 Authorization: Bearer abcdefgh12345678"#,
        );
        assert!(
            !s.contains("hunter2") && !s.contains("abc123") && !s.contains("abcdefgh"),
            "{s}"
        );
        assert!(!scrub("https://user:pw@host/x").contains("pw@"));
        let s = scrub(r"\\nas\share\photos\2024\x.nef");
        assert_eq!(s, "<path>/<photo>");
    }

    #[test]
    fn base_name_handles_both_separators() {
        assert_eq!(base_name(r"D:\Photos\Trip 2024\"), "Trip 2024");
        assert_eq!(base_name("/mnt/photos/trip"), "trip");
    }

    #[test]
    fn rotation_by_size_and_pruning() {
        let dir = tempfile::tempdir().unwrap();
        let limits = LogLimits {
            file_bytes: 100,
            total_bytes: 250,
            max_age: Duration::from_secs(3600),
        };
        let log = RotatingLog::with(dir.path().to_path_buf(), limits, Box::new(SystemTime::now));
        let mut w = &log;
        for _ in 0..20 {
            w.write_all(&[b'x'; 60]).unwrap();
        }
        let mut names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| {
                (
                    e.file_name().to_string_lossy().into_owned(),
                    e.metadata().unwrap().len(),
                )
            })
            .collect();
        names.sort();
        // every file holds at most one 60 byte write (two would exceed 100) ...
        assert!(names.iter().all(|(_, n)| *n <= 100), "{names:?}");
        // ... and the total was trimmed to the cap (the newest file is always kept)
        let total: u64 = names.iter().map(|n| n.1).sum();
        assert!(total <= 250 + 60, "{names:?}");
        assert!(names.len() >= 2, "{names:?}");
        assert_eq!(log.current_name().unwrap(), names.last().unwrap().0);
    }

    #[test]
    fn rotation_by_day_and_age() {
        let dir = tempfile::tempdir().unwrap();
        let t0 = UNIX_EPOCH + Duration::from_secs(1_791_892_245);
        let clock = Arc::new(Mutex::new(t0));
        let c2 = clock.clone();
        let limits = LogLimits {
            max_age: Duration::from_secs(2 * 86_400),
            ..LogLimits::default()
        };
        let log = RotatingLog::with(
            dir.path().to_path_buf(),
            limits,
            Box::new(move || *c2.lock().unwrap()),
        );
        let mut w = &log;
        w.write_all(b"day one\n").unwrap();
        assert_eq!(log.current_name().unwrap(), "imagepicker-20261013.0.log");
        // an old file (mtime far in the past relative to the fake clock) is removed on rotation
        let old = dir.path().join("imagepicker-20200101.0.log");
        fs::write(&old, b"old").unwrap();
        File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_secs(1_577_836_800))
            .unwrap();
        *clock.lock().unwrap() = t0 + Duration::from_secs(86_400);
        w.write_all(b"day two\n").unwrap();
        assert_eq!(log.current_name().unwrap(), "imagepicker-20261014.0.log");
        assert!(!old.exists());
        // a restart continues the newest file of the day
        let again = RotatingLog::new(dir.path().to_path_buf());
        let day = day_utc(SystemTime::now());
        (&again).write_all(b"x").unwrap();
        assert_eq!(again.current_name().unwrap(), log_name(&day, 0));
    }

    /// The only test that installs the process-wide hook.
    #[test]
    fn panic_hook_writes_a_crash_file() {
        let dir = tempfile::tempdir().unwrap();
        install_panic_hook(dir.path().to_path_buf());
        let r = std::thread::Builder::new()
            .name("crash-test-thread".into())
            .spawn(|| {
                panic!("forced panic for the crash test 7f3a");
            })
            .unwrap()
            .join();
        assert!(r.is_err());
        let files: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| fs::read_to_string(e.path()).is_ok_and(|t| t.contains("crash test 7f3a")))
            .collect();
        assert_eq!(files.len(), 1, "one crash file for the forced panic");
        let text = fs::read_to_string(files[0].path()).unwrap();
        for needle in [
            "kind: rust-panic",
            "time: 20",
            "version: ",
            "os: ",
            "thread: crash-test-thread",
            "location: ",
            "message: forced panic for the crash test 7f3a",
            "backtrace:",
        ] {
            assert!(text.contains(needle), "missing {needle:?} in\n{text}");
        }
        // later panics of other tests must not recreate the deleted temp folder
        *CRASH_DIR.lock().unwrap() = None;
    }

    #[test]
    fn crash_folder_is_trimmed() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..5 {
            write_crash(
                dir.path(),
                &CrashReport {
                    kind: "worker-exit".into(),
                    message: format!("m{i}"),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        prune_crashes(dir.path(), 3);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 3);
    }
}
