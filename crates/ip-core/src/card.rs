//! Import from a memory card or camera (PRD F1.8, docs/09 §5.3 P2; `docs/api-contract-m1.md`
//! "从存储卡导入").
//!
//! Three parts:
//!
//! * **Detection**: [`card_sources`] lists the removable volumes that carry a `DCIM` folder
//!   (Windows drive types, macOS `/Volumes`, Linux `/media` and `/run/media`). The classification
//!   ([`classify_volumes`]) is a pure function over a drive list. Nothing is ever imported on
//!   its own.
//! * **Copy engine**: [`copy_sets`] copies files (never moves, the source is only read) into
//!   their destination folders: each copy goes to a hidden temporary file, is verified (size and
//!   blake3 of what was read from the card against a re-read of the copy) and only then claims its
//!   final name, which is never an existing file (a numeric suffix is added instead, shared by
//!   the RAW and the JPEG of a pair so they keep pairing). A file that is already at the
//!   destination with the same content is reused instead of duplicated.
//! * **Task** ([`Core::card_import`]): scan the card (the import exclusions apply), skip what the
//!   catalog already knows (name + size + mtime, or content key), derive the destination folders
//!   from the capture dates, copy with progress and cancellation (task kind `card_import`), then
//!   import the verified copies as a session. A failure or cancellation in the middle keeps the
//!   copies already verified and imports them; the report says what was left.

use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ip_imaging::{ImageFormat, ScannedFile};
use ip_worker_client::CancelToken;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{CoreError, Result};
use crate::tasks::{TaskHandle, CANCELLED, DONE, FAILED};
use crate::Core;

/// Folder name patterns are made of: `{yyyy}`, `{mm}`, `{dd}`, `{yyyy-mm-dd}`, `{yyyymmdd}`.
pub const DEFAULT_PATTERN: &str = "{yyyy}/{yyyy-mm-dd}";
/// Task kind of the card import.
pub const TASK_KIND: &str = "card_import";
/// The copy is read and written in chunks of this size (cancellation is checked per chunk).
const CHUNK: usize = 1 << 20;
/// mtimes of FAT cards have a 2 second resolution.
const MTIME_TOLERANCE_MS: i64 = 2000;
/// Entries looked at when estimating the photo count of a card.
const ESTIMATE_LIMIT: usize = 100_000;
/// Failed files listed in a report (the count is always exact).
const REPORT_FAILURES: usize = 100;

// ------------------------------------------------------------------ detection

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DriveKind {
    Removable,
    Fixed,
    Remote,
    Optical,
    /// The platform does not say (mount points on macOS and Linux).
    Unknown,
}

/// One mounted volume, as far as card detection cares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    pub path: PathBuf,
    pub label: Option<String>,
    pub kind: DriveKind,
    pub has_dcim: bool,
    pub total_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
}

/// The volumes that are import candidates: removable (or of unknown kind, for mount points) and
/// carrying a `DCIM` folder. Fixed, network and optical drives never are, whatever they hold.
/// Sorted by path, one entry per path.
pub fn classify_volumes(volumes: &[Volume]) -> Vec<Volume> {
    let mut out: Vec<Volume> = Vec::new();
    for v in volumes {
        let kind_ok = matches!(v.kind, DriveKind::Removable | DriveKind::Unknown);
        if v.has_dcim && kind_ok && !out.iter().any(|o| o.path == v.path) {
            out.push(v.clone());
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// The `DCIM` folder of a volume root (any letter case), if there is one.
pub fn dcim_dir(root: &Path) -> Option<PathBuf> {
    std::fs::read_dir(root)
        .ok()?
        .flatten()
        .find(|e| {
            e.file_name().to_string_lossy().eq_ignore_ascii_case("DCIM")
                && e.file_type().map(|t| t.is_dir()).unwrap_or(false)
        })
        .map(|e| e.path())
}

#[cfg(windows)]
fn enumerate_volumes() -> Vec<Volume> {
    extern "system" {
        fn GetDriveTypeW(root: *const u16) -> u32;
        fn GetVolumeInformationW(
            root: *const u16,
            name: *mut u16,
            name_len: u32,
            serial: *mut u32,
            max_component: *mut u32,
            flags: *mut u32,
            fs_name: *mut u16,
            fs_name_len: u32,
        ) -> i32;
        fn GetDiskFreeSpaceExW(
            dir: *const u16,
            available: *mut u64,
            total: *mut u64,
            free: *mut u64,
        ) -> i32;
    }
    let mut out = Vec::new();
    for letter in b'A'..=b'Z' {
        let root = format!("{}:\\", letter as char);
        let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: `wide` is a valid NUL-terminated UTF-16 string for the call's duration.
        let kind = match unsafe { GetDriveTypeW(wide.as_ptr()) } {
            2 => DriveKind::Removable,
            3 => DriveKind::Fixed,
            4 => DriveKind::Remote,
            5 => DriveKind::Optical,
            0 | 1 => continue,
            _ => DriveKind::Unknown,
        };
        let path = PathBuf::from(&root);
        // fixed, network and optical drives are never candidates: do not even touch them
        let has_dcim =
            matches!(kind, DriveKind::Removable | DriveKind::Unknown) && dcim_dir(&path).is_some();
        let (mut label, mut total_bytes, mut free_bytes) = (None, None, None);
        if has_dcim {
            let mut name = [0u16; 261];
            // SAFETY: the buffers are valid for the lengths given; unused outputs are null.
            let ok = unsafe {
                GetVolumeInformationW(
                    wide.as_ptr(),
                    name.as_mut_ptr(),
                    name.len() as u32,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    0,
                )
            };
            if ok != 0 {
                let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
                label = Some(String::from_utf16_lossy(&name[..len])).filter(|l| !l.is_empty());
            }
            let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
            // SAFETY: the three out pointers are valid for the call's duration.
            let ok =
                unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut avail, &mut total, &mut free) };
            if ok != 0 {
                total_bytes = Some(total);
                free_bytes = Some(avail);
            }
        }
        out.push(Volume {
            path,
            label,
            kind,
            has_dcim,
            total_bytes,
            free_bytes,
        });
    }
    out
}

#[cfg(all(unix, not(any(target_os = "android", target_os = "ios"))))]
fn enumerate_volumes() -> Vec<Volume> {
    fn children(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                    .map(|e| e.path())
                    .filter(|p| p.is_dir())
                    .collect()
            })
            .unwrap_or_default()
    }
    // macOS: /Volumes/<name>; Linux: /media/<name>, /media/<user>/<name>, /run/media/<user>/<name>
    let mut candidates: Vec<PathBuf> = children(Path::new("/Volumes"));
    for base in ["/media", "/run/media"] {
        for c in children(Path::new(base)) {
            candidates.extend(children(&c));
            candidates.push(c);
        }
    }
    candidates
        .into_iter()
        .map(|path| {
            let has_dcim = dcim_dir(&path).is_some();
            let (total_bytes, free_bytes) = if has_dcim {
                disk_space(&path).map_or((None, None), |(t, f)| (Some(t), Some(f)))
            } else {
                (None, None)
            };
            Volume {
                label: path.file_name().map(|n| n.to_string_lossy().into_owned()),
                path,
                kind: DriveKind::Unknown,
                has_dcim,
                total_bytes,
                free_bytes,
            }
        })
        .collect()
}

/// Android and iOS: cards are reached through the platform's media APIs, not mount points.
#[cfg(any(target_os = "android", target_os = "ios"))]
fn enumerate_volumes() -> Vec<Volume> {
    Vec::new()
}

/// (total, available to the user) bytes of the filesystem holding `path`.
#[cfg(all(unix, not(any(target_os = "android", target_os = "ios"))))]
#[allow(clippy::unnecessary_cast, clippy::useless_conversion)]
fn disk_space(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `s` is a zeroed plain-old-data struct that statvfs fills in; `c` is NUL-terminated.
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    let frs = s.f_frsize as u64;
    Some((frs * s.f_blocks as u64, frs * s.f_bavail as u64))
}

/// One card in `GET /api/import/sources`.
#[derive(Debug, Clone, Serialize)]
pub struct CardSource {
    pub label: String,
    pub path: String,
    pub kind: DriveKind,
    pub total_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
    /// Photos on the card (RAW+JPEG pairs count once); a lower bound when `estimate_truncated`.
    pub photo_estimate: usize,
    pub estimate_truncated: bool,
}

/// Number of photos below `dir` (distinct folder + base name of supported image files) and
/// whether the walk stopped early.
pub fn estimate_photos(dir: &Path) -> (usize, bool) {
    let mut stems: HashSet<String> = HashSet::new();
    let mut todo = vec![dir.to_path_buf()];
    let mut seen = 0usize;
    while let Some(d) = todo.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            seen += 1;
            if seen > ESTIMATE_LIMIT {
                return (stems.len(), true);
            }
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let Ok(ft) = e.file_type() else { continue };
            let p = e.path();
            if ft.is_dir() {
                todo.push(p);
            } else if ImageFormat::from_path(&p).is_some() {
                let stem = p.file_stem().map(|s| s.to_string_lossy().to_lowercase());
                stems.insert(format!("{}/{}", d.display(), stem.unwrap_or_default()));
            }
        }
    }
    (stems.len(), false)
}

/// The card candidates among `volumes`, with their photo estimates (reads the cards).
pub fn sources_from(volumes: &[Volume]) -> Vec<CardSource> {
    classify_volumes(volumes)
        .into_iter()
        .map(|v| {
            let (photo_estimate, estimate_truncated) = dcim_dir(&v.path)
                .map(|d| estimate_photos(&d))
                .unwrap_or((0, false));
            CardSource {
                label: v
                    .label
                    .filter(|l| !l.trim().is_empty())
                    .unwrap_or_else(|| v.path.to_string_lossy().into_owned()),
                path: v.path.to_string_lossy().into_owned(),
                kind: v.kind,
                total_bytes: v.total_bytes,
                free_bytes: v.free_bytes,
                photo_estimate,
                estimate_truncated,
            }
        })
        .collect()
}

/// Memory cards and camera storage mounted right now (blocking: touches the drives).
pub fn card_sources() -> Vec<CardSource> {
    sources_from(&enumerate_volumes())
}

// ------------------------------------------------------------------ destination pattern

const TOKENS: [&str; 5] = ["{yyyy-mm-dd}", "{yyyymmdd}", "{yyyy}", "{mm}", "{dd}"];

fn expand_token_text(component: &str, y: i64, m: u32, d: u32) -> String {
    component
        .replace("{yyyy-mm-dd}", &format!("{y:04}-{m:02}-{d:02}"))
        .replace("{yyyymmdd}", &format!("{y:04}{m:02}{d:02}"))
        .replace("{yyyy}", &format!("{y:04}"))
        .replace("{mm}", &format!("{m:02}"))
        .replace("{dd}", &format!("{d:02}"))
}

/// Checks a destination folder pattern: `/`-separated relative folders made of literal text and
/// the date tokens; an empty pattern puts every file straight into the destination.
pub fn validate_pattern(pattern: &str) -> std::result::Result<(), String> {
    let p = pattern.trim();
    if p.is_empty() {
        return Ok(());
    }
    if p.contains('\\') {
        return Err("pattern folders are separated by /".into());
    }
    let comps: Vec<&str> = p.split('/').collect();
    if comps.len() > 6 {
        return Err("pattern has too many folder levels".into());
    }
    for c in comps {
        let mut rest = c.to_string();
        for t in TOKENS {
            rest = rest.replace(t, "");
        }
        if rest.contains(['{', '}']) {
            return Err(format!(
                "unknown token in pattern folder {c:?} (use {{yyyy}} {{mm}} {{dd}} {{yyyy-mm-dd}} {{yyyymmdd}})"
            ));
        }
        let expanded = expand_token_text(c, 2000, 1, 2);
        let t = expanded.trim();
        if t.is_empty() || t == "." || t == ".." || t != expanded || expanded.ends_with('.') {
            return Err(format!("pattern folder {c:?} is not usable"));
        }
        if expanded.chars().any(|ch| {
            ch.is_control() || matches!(ch, ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\\' | '/')
        }) || crate::roots::is_reserved_windows_name(&expanded)
        {
            return Err(format!("pattern folder {c:?} is not usable"));
        }
    }
    Ok(())
}

/// The folders (below the destination) a photo taken on `y-m-d` goes to.
pub fn expand_pattern(pattern: &str, y: i64, m: u32, d: u32) -> Vec<String> {
    let p = pattern.trim();
    if p.is_empty() {
        return Vec::new();
    }
    p.split('/')
        .map(|c| expand_token_text(c, y, m, d))
        .collect()
}

// ------------------------------------------------------------------ copy engine

/// Reads the source files; behind a trait so tests can make files unreadable or cancel while a
/// copy is running.
pub trait CardIo: Send + Sync {
    fn open(&self, path: &Path) -> io::Result<Box<dyn Read + Send>>;
}

/// The real filesystem.
pub struct FsCardIo;

impl CardIo for FsCardIo {
    fn open(&self, path: &Path) -> io::Result<Box<dyn Read + Send>> {
        Ok(Box::new(std::fs::File::open(path)?))
    }
}

/// One file to copy.
#[derive(Debug, Clone)]
pub struct CopyItem {
    pub src: PathBuf,
    /// Size at scan time; a different number of bytes read is a failure (the card changed).
    pub size: u64,
    pub mtime_ms: i64,
}

/// Files that must get the same collision suffix and the same folder: a RAW and its JPEG.
#[derive(Debug, Clone)]
pub struct CopySet {
    pub dest_dir: PathBuf,
    pub items: Vec<CopyItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopiedFile {
    pub src: PathBuf,
    pub dst: PathBuf,
    pub bytes: u64,
}

#[derive(Debug, Default)]
pub struct CopyOutcome {
    /// Verified copies.
    pub copied: Vec<CopiedFile>,
    /// Destination files that already held the same content (nothing was written).
    pub present: Vec<PathBuf>,
    pub failed: Vec<(PathBuf, String)>,
    /// Files never tried because of a cancellation or an aborting error.
    pub not_attempted: usize,
    pub cancelled: bool,
    /// A destination-side error that stopped the run (disk full, folder not writable, ...).
    pub aborted: Option<String>,
}

enum CopyErr {
    /// The card file cannot be read (or changed while copying): this file fails, the run goes on.
    Source(String),
    /// The copy did not match its source: this file fails, the run goes on.
    Verify(String),
    /// The destination cannot be written: the run stops.
    Dest(String),
    Cancelled,
}

enum Stop {
    Cancelled,
    Abort(String),
}

fn hash_reader(
    r: &mut dyn Read,
    cancel: Option<&CancelToken>,
    mut sink: Option<&mut dyn Write>,
) -> io::Result<(blake3::Hash, u64)> {
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; CHUNK];
    let mut n = 0u64;
    loop {
        if cancel.is_some_and(|c| c.is_cancelled()) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let k = match r.read(&mut buf) {
            Ok(0) => break,
            Ok(k) => k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted && cancel.is_none() => continue,
            Err(e) => return Err(e),
        };
        h.update(&buf[..k]);
        if let Some(w) = sink.as_mut() {
            w.write_all(&buf[..k]).map_err(WriteFailed::wrap)?;
        }
        n += k as u64;
    }
    Ok((h.finalize(), n))
}

/// Marks an error that came from the sink (destination) rather than the reader (source).
#[derive(Debug)]
struct WriteFailed(io::Error);

impl WriteFailed {
    fn wrap(e: io::Error) -> io::Error {
        io::Error::new(e.kind(), WriteFailed(e))
    }
    fn is(e: &io::Error) -> bool {
        e.get_ref().is_some_and(|i| i.is::<WriteFailed>())
    }
}

impl std::fmt::Display for WriteFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for WriteFailed {}

fn hash_file(path: &Path) -> io::Result<(blake3::Hash, u64)> {
    hash_reader(&mut std::fs::File::open(path)?, None, None)
}

fn suffixed(name: &str, n: u32) -> String {
    if n == 0 {
        return name.to_string();
    }
    match name.rfind('.') {
        Some(i) if i > 0 => format!("{}_{n}{}", &name[..i], &name[i..]),
        _ => format!("{name}_{n}"),
    }
}

fn file_name_of(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into())
}

/// Copies one file to `final_path` (a free name). The bytes go to a hidden temporary file in the
/// destination folder first; `final_path` is claimed with `create_new` (never an overwrite) and
/// replaced by the verified temporary file.
fn copy_one(
    io: &dyn CardIo,
    item: &CopyItem,
    final_path: &Path,
    cancel: &CancelToken,
) -> std::result::Result<u64, CopyErr> {
    let dir = final_path.parent().unwrap_or(Path::new("."));
    let name = file_name_of(final_path);
    let mut tmp_path = None;
    let mut tmp_file = None;
    for k in 0..1000u32 {
        let p = dir.join(format!(".{name}.{k}.ipcopy"));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&p)
        {
            Ok(f) => {
                tmp_path = Some(p);
                tmp_file = Some(f);
                break;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(CopyErr::Dest(format!(
                    "cannot write in {}: {e}",
                    dir.display()
                )))
            }
        }
    }
    let (Some(tmp), Some(mut file)) = (tmp_path, tmp_file) else {
        return Err(CopyErr::Dest(format!(
            "cannot create a temporary file in {}",
            dir.display()
        )));
    };
    let cleanup = |tmp: &Path| {
        let _ = std::fs::remove_file(tmp);
    };

    let mut src = match io.open(&item.src) {
        Ok(r) => r,
        Err(e) => {
            cleanup(&tmp);
            return Err(CopyErr::Source(format!(
                "cannot read {}: {e}",
                item.src.display()
            )));
        }
    };
    let (src_hash, bytes) = match hash_reader(&mut src, Some(cancel), Some(&mut file)) {
        Ok(v) => v,
        Err(e) => {
            cleanup(&tmp);
            return Err(
                if cancel.is_cancelled() && e.kind() == io::ErrorKind::Interrupted {
                    CopyErr::Cancelled
                } else if WriteFailed::is(&e) {
                    CopyErr::Dest(format!("cannot write {}: {e}", final_path.display()))
                } else {
                    CopyErr::Source(format!("cannot read {}: {e}", item.src.display()))
                },
            );
        }
    };
    drop(src);
    if bytes != item.size {
        cleanup(&tmp);
        return Err(CopyErr::Source(format!(
            "{} changed while it was copied ({} bytes expected, {bytes} read)",
            item.src.display(),
            item.size
        )));
    }
    let finish = (|| -> io::Result<()> {
        file.flush()?;
        file.sync_all()?;
        if item.mtime_ms >= 0 {
            // keep the capture time of the card file (the catalog matches on it later)
            let t = std::time::UNIX_EPOCH + std::time::Duration::from_millis(item.mtime_ms as u64);
            let _ = file.set_modified(t);
        }
        Ok(())
    })();
    drop(file);
    if let Err(e) = finish {
        cleanup(&tmp);
        return Err(CopyErr::Dest(format!(
            "cannot write {}: {e}",
            final_path.display()
        )));
    }
    // verify: what is on the destination now is what was read from the card
    match hash_file(&tmp) {
        Ok((h, n)) if h == src_hash && n == bytes => {}
        Ok(_) => {
            cleanup(&tmp);
            return Err(CopyErr::Verify(format!(
                "the copy of {} does not match the original",
                item.src.display()
            )));
        }
        Err(e) => {
            cleanup(&tmp);
            return Err(CopyErr::Verify(format!(
                "cannot verify the copy of {}: {e}",
                item.src.display()
            )));
        }
    }
    // claim the final name without overwriting anything, then put the verified file there
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(final_path)
    {
        Ok(placeholder) => drop(placeholder),
        Err(e) => {
            cleanup(&tmp);
            return Err(CopyErr::Verify(format!(
                "{} is taken by another file now: {e}",
                final_path.display()
            )));
        }
    }
    if let Err(e) = std::fs::rename(&tmp, final_path) {
        cleanup(&tmp);
        let _ = std::fs::remove_file(final_path);
        return Err(CopyErr::Dest(format!(
            "cannot create {}: {e}",
            final_path.display()
        )));
    }
    Ok(bytes)
}

/// Does `path` already hold the content of `item`?
fn same_content(io: &dyn CardIo, item: &CopyItem, path: &Path) -> bool {
    let Ok(md) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !md.is_file() || md.len() != item.size {
        return false;
    }
    let Ok(mut src) = io.open(&item.src) else {
        return false;
    };
    let (Ok((a, _)), Ok((b, _))) = (hash_reader(&mut src, None, None), hash_file(path)) else {
        return false;
    };
    a == b
}

/// Copies `sets` below `dest_root` (which must exist). Each set goes to its `dest_dir` (created
/// on demand, and checked to stay inside `dest_root` after symlinks are resolved) under the
/// original names, or under the smallest numeric suffix that collides with nothing. `progress`
/// gets the number of files handled so far.
pub fn copy_sets(
    io: &dyn CardIo,
    dest_root: &Path,
    sets: &[CopySet],
    cancel: &CancelToken,
    progress: &dyn Fn(usize),
) -> CopyOutcome {
    let mut out = CopyOutcome::default();
    let total: usize = sets.iter().map(|s| s.items.len()).sum();
    let root = match std::fs::canonicalize(dest_root) {
        Ok(r) => r,
        Err(e) => {
            out.not_attempted = total;
            out.aborted = Some(format!("cannot use {}: {e}", dest_root.display()));
            return out;
        }
    };
    let mut ok_dirs: HashSet<PathBuf> = HashSet::new();
    let mut handled = 0usize;
    for set in sets {
        match copy_set(io, &root, set, cancel, &mut ok_dirs, &mut out) {
            Ok(()) => {}
            Err(stop) => {
                match stop {
                    Stop::Cancelled => out.cancelled = true,
                    Stop::Abort(msg) => out.aborted = Some(msg),
                }
                out.not_attempted =
                    total.saturating_sub(out.copied.len() + out.present.len() + out.failed.len());
                return out;
            }
        }
        handled += set.items.len();
        progress(handled);
    }
    out
}

fn copy_set(
    io: &dyn CardIo,
    root: &Path,
    set: &CopySet,
    cancel: &CancelToken,
    ok_dirs: &mut HashSet<PathBuf>,
    out: &mut CopyOutcome,
) -> std::result::Result<(), Stop> {
    if cancel.is_cancelled() {
        return Err(Stop::Cancelled);
    }
    if !ok_dirs.contains(&set.dest_dir) {
        std::fs::create_dir_all(&set.dest_dir)
            .map_err(|e| Stop::Abort(format!("cannot create {}: {e}", set.dest_dir.display())))?;
        let canon = std::fs::canonicalize(&set.dest_dir)
            .map_err(|e| Stop::Abort(format!("cannot use {}: {e}", set.dest_dir.display())))?;
        if !canon.starts_with(root) {
            return Err(Stop::Abort(format!(
                "{} is outside the destination folder",
                set.dest_dir.display()
            )));
        }
        ok_dirs.insert(set.dest_dir.clone());
    }
    // the smallest suffix for which no member collides with a different file
    let names: Vec<String> = set.items.iter().map(|i| file_name_of(&i.src)).collect();
    let mut chosen = None;
    for n in 0..10_000u32 {
        let mut plan: Vec<(PathBuf, bool)> = Vec::new(); // (path, already the same content)
        let mut clash = false;
        for (item, name) in set.items.iter().zip(&names) {
            let path = set.dest_dir.join(suffixed(name, n));
            if std::fs::symlink_metadata(&path).is_ok() {
                if same_content(io, item, &path) {
                    plan.push((path, true));
                } else {
                    clash = true;
                    break;
                }
            } else {
                plan.push((path, false));
            }
        }
        if !clash {
            chosen = Some(plan);
            break;
        }
    }
    let Some(plan) = chosen else {
        for item in &set.items {
            out.failed.push((
                item.src.clone(),
                "no free file name at the destination".into(),
            ));
        }
        return Ok(());
    };
    for (item, (path, present)) in set.items.iter().zip(plan) {
        if present {
            out.present.push(path);
            continue;
        }
        match copy_one(io, item, &path, cancel) {
            Ok(bytes) => out.copied.push(CopiedFile {
                src: item.src.clone(),
                dst: path,
                bytes,
            }),
            Err(CopyErr::Source(m)) | Err(CopyErr::Verify(m)) => {
                out.failed.push((item.src.clone(), m));
            }
            Err(CopyErr::Dest(m)) => {
                out.failed.push((item.src.clone(), m.clone()));
                return Err(Stop::Abort(m));
            }
            Err(CopyErr::Cancelled) => return Err(Stop::Cancelled),
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ the task

/// Body of `POST /api/import/card`.
#[derive(Debug, Clone, Deserialize)]
pub struct CardImportRequest {
    /// The card (volume root, as listed by `GET /api/import/sources`).
    pub source: String,
    /// Destination root, inside the allowed folders.
    pub dest: String,
    /// Folder pattern below `dest` ([`DEFAULT_PATTERN`] when absent).
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    /// Globs / sub-folders / names to leave out, as for `POST /api/import`.
    #[serde(default, alias = "exclude[]")]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CardFailure {
    pub path: String,
    pub error: String,
}

/// What a card import did; stored in the task's `params.report`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CardReport {
    /// Photo files found on the card (after exclusions).
    pub found: usize,
    /// Left alone because the catalog already has them.
    pub skipped_existing: usize,
    /// Verified copies written.
    pub copied: usize,
    /// Files the destination already had with the same content (imported, not written again).
    pub reused: usize,
    pub bytes: u64,
    pub failed_count: usize,
    pub failed: Vec<CardFailure>,
    /// Never tried because of a cancellation or an aborting error.
    pub not_attempted: usize,
    pub cancelled: bool,
    pub aborted: Option<String>,
    /// The session the copies were imported into (none when nothing was copied).
    pub session_id: Option<i64>,
}

struct CardJob {
    dcim: PathBuf,
    source_name: String,
    dest_root: PathBuf,
    pattern: String,
    title: Option<String>,
    exclude: Vec<String>,
}

/// What the catalog knows about its photos: (lower-case name, size) -> mtimes, and content keys.
#[derive(Default)]
struct Known {
    by_name: HashMap<(String, i64), Vec<i64>>,
    keys: HashSet<String>,
}

fn load_known(c: &mut rusqlite::Connection) -> Result<Known> {
    let mut st = c.prepare("SELECT file_name, file_size, mtime, content_key FROM photo")?;
    let mut known = Known::default();
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, Option<String>>(0)?,
            r.get::<_, Option<i64>>(1)?,
            r.get::<_, Option<i64>>(2)?,
            r.get::<_, Option<String>>(3)?,
        ))
    })?;
    for row in rows {
        let (name, size, mtime, key) = row?;
        if let (Some(n), Some(s), Some(m)) = (name, size, mtime) {
            known
                .by_name
                .entry((n.to_lowercase(), s))
                .or_default()
                .push(m);
        }
        known.keys.extend(key);
    }
    Ok(known)
}

fn group_key(p: &Path) -> String {
    let dir = p.parent().map(|d| d.to_string_lossy().to_lowercase());
    let stem = p.file_stem().map(|s| s.to_string_lossy().to_lowercase());
    format!("{}/{}", dir.unwrap_or_default(), stem.unwrap_or_default())
}

fn format_rank(f: ImageFormat) -> u8 {
    match f {
        ImageFormat::Jpeg => 0,
        ImageFormat::Heif => 1,
        ImageFormat::Raw => 3,
        _ => 2,
    }
}

/// Deepest folder containing every path (all are below `fallback`).
fn common_dir(paths: &[PathBuf], fallback: &Path) -> PathBuf {
    let mut common: Option<PathBuf> = None;
    for p in paths {
        let dir = p.parent().unwrap_or(fallback);
        common = Some(match common {
            None => dir.to_path_buf(),
            Some(c) => c
                .components()
                .zip(dir.components())
                .take_while(|(a, b)| a == b)
                .map(|(a, _)| a)
                .collect(),
        });
    }
    common.unwrap_or_else(|| fallback.to_path_buf())
}

impl Core {
    /// Validates and starts a card import; progress arrives as `task.progress` events of kind
    /// `card_import` and the outcome is in the task's `params.report` ([`CardReport`]).
    pub async fn card_import(self: &Arc<Self>, req: CardImportRequest) -> Result<String> {
        self.card_import_with(req, Arc::new(FsCardIo)).await
    }

    pub(crate) async fn card_import_with(
        self: &Arc<Self>,
        req: CardImportRequest,
        io: Arc<dyn CardIo>,
    ) -> Result<String> {
        if req.source.trim().is_empty() {
            return Err(CoreError::bad_request("source must not be empty"));
        }
        if req.dest.trim().is_empty() {
            return Err(CoreError::bad_request("dest must not be empty"));
        }
        let source = crate::import::normalize_path(Path::new(&req.source))
            .map_err(|e| CoreError::bad_request(format!("invalid source: {e}")))?;
        let dcim = dcim_dir(&source).ok_or_else(|| {
            CoreError::bad_request(format!("no DCIM folder on {}", source.display()))
        })?;
        let pattern = req
            .pattern
            .clone()
            .unwrap_or_else(|| DEFAULT_PATTERN.to_string());
        validate_pattern(&pattern).map_err(CoreError::bad_request)?;
        let dest_root = crate::import::normalize_path(Path::new(&req.dest))
            .map_err(|e| CoreError::bad_request(format!("invalid dest: {e}")))?;
        std::fs::create_dir_all(&dest_root)
            .map_err(|e| CoreError::bad_request(format!("cannot create destination: {e}")))?;
        // never write onto the card being read
        if let (Ok(s), Ok(d)) = (
            std::fs::canonicalize(&source),
            std::fs::canonicalize(&dest_root),
        ) {
            if d.starts_with(&s) {
                return Err(CoreError::bad_request(
                    "the destination must not be on the card",
                ));
            }
        }
        let exclude: Vec<String> = req
            .exclude
            .iter()
            .map(|e| e.trim().to_string())
            .filter(|e| !e.is_empty())
            .collect();
        let source_name = source
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| source.to_string_lossy().into_owned());
        let spec = json!({
            "source": source.to_string_lossy(),
            "dest": dest_root.to_string_lossy(),
            "pattern": pattern,
        });
        let task = self.task_begin(TASK_KIND, spec.clone(), 0).await?;
        let id = task.id.clone();
        let job = CardJob {
            dcim,
            source_name,
            dest_root,
            pattern,
            title: req.title.filter(|t| !t.trim().is_empty()),
            exclude,
        };
        let core = self.clone();
        tokio::spawn(async move {
            let mut report = CardReport::default();
            let res = core.card_pipeline(&task, &job, io, &mut report).await;
            let (state, error) = match res {
                Err(e) => (FAILED, Some(e.to_string())),
                Ok(()) => card_outcome(&report),
            };
            let mut params = spec;
            params["report"] = serde_json::to_value(&report).unwrap_or_default();
            let tid = task.id.clone();
            let _ = core
                .db
                .call(move |c| crate::tasks::set_params(c, &tid, &params))
                .await;
            let done = (report.copied + report.reused) as i64;
            let total = done + (report.failed_count + report.not_attempted) as i64;
            core.task_end(&task, (done, total), state, error, None)
                .await;
        });
        Ok(id)
    }

    async fn card_pipeline(
        self: &Arc<Self>,
        task: &TaskHandle,
        job: &CardJob,
        io: Arc<dyn CardIo>,
        report: &mut CardReport,
    ) -> Result<()> {
        // 1. what is on the card
        let imaging = self.imaging.clone();
        let (dcim, exclude) = (job.dcim.clone(), job.exclude.clone());
        let files = tokio::task::spawn_blocking(move || imaging.scan_dir(&dcim, true, &exclude))
            .await
            .map_err(|e| CoreError::Internal(e.into()))??;
        report.found = files.len();
        if task.cancelled() {
            report.cancelled = true;
            return Ok(());
        }

        // 2. leave out what the catalog has already
        let known = self.db.call(load_known).await?;
        let imaging = self.imaging.clone();
        let (fresh, skipped) = tokio::task::spawn_blocking(move || {
            let hit = |f: &ScannedFile| -> bool {
                let name = f
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                let by_name = known.by_name.get(&(name, f.size as i64)).is_some_and(|ms| {
                    ms.iter()
                        .any(|m| (m - f.mtime_ms).abs() <= MTIME_TOLERANCE_MS)
                });
                by_name
                    || imaging
                        .content_key(&f.path)
                        .is_ok_and(|k| known.keys.contains(&k))
            };
            let flags: Vec<bool> = files.par_iter().map(hit).collect();
            let mut fresh = Vec::new();
            let mut skipped = 0usize;
            for (f, is_known) in files.into_iter().zip(flags) {
                if is_known {
                    skipped += 1;
                } else {
                    fresh.push(f);
                }
            }
            (fresh, skipped)
        })
        .await
        .map_err(|e| CoreError::Internal(e.into()))?;
        report.skipped_existing = skipped;
        if fresh.is_empty() || task.cancelled() {
            report.cancelled = task.cancelled();
            return Ok(());
        }

        // 3. destination folders from the capture dates (a pair shares one date)
        let imaging = self.imaging.clone();
        let (dest_root, pattern) = (job.dest_root.clone(), job.pattern.clone());
        let sets = tokio::task::spawn_blocking(move || {
            let mut groups: HashMap<String, Vec<ScannedFile>> = HashMap::new();
            for f in fresh {
                groups.entry(group_key(&f.path)).or_default().push(f);
            }
            let mut groups: Vec<Vec<ScannedFile>> = groups.into_values().collect();
            for g in &mut groups {
                g.sort_by(|a, b| {
                    (format_rank(a.format), &a.path).cmp(&(format_rank(b.format), &b.path))
                });
            }
            groups.sort_by(|a, b| a[0].path.cmp(&b[0].path));
            groups
                .par_iter()
                .map(|g| {
                    let taken = g.iter().find_map(|f| {
                        imaging
                            .read_metadata(&f.path, f.format)
                            .ok()
                            .and_then(|m| m.taken_at_ms)
                    });
                    let ms = taken.unwrap_or(g[0].mtime_ms);
                    let (y, m, d) = crate::export::civil_from_days(ms.div_euclid(86_400_000));
                    let mut dir = dest_root.clone();
                    dir.extend(expand_pattern(&pattern, y, m, d));
                    CopySet {
                        dest_dir: dir,
                        items: g
                            .iter()
                            .map(|f| CopyItem {
                                src: f.path.clone(),
                                size: f.size,
                                mtime_ms: f.mtime_ms,
                            })
                            .collect(),
                    }
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| CoreError::Internal(e.into()))?;
        let total: usize = sets.iter().map(|s| s.items.len()).sum();
        self.task_step(task, 0, total as i64);

        // 4. copy + verify
        let (core, t2, cancel, dest_root) = (
            self.clone(),
            task.clone(),
            task.cancel.clone(),
            job.dest_root.clone(),
        );
        let outcome = tokio::task::spawn_blocking(move || {
            copy_sets(&*io, &dest_root, &sets, &cancel, &|done| {
                core.task_step(&t2, done as i64, total as i64)
            })
        })
        .await
        .map_err(|e| CoreError::Internal(e.into()))?;
        report.copied = outcome.copied.len();
        report.reused = outcome.present.len();
        report.bytes = outcome.copied.iter().map(|c| c.bytes).sum();
        report.failed_count = outcome.failed.len();
        report.failed = outcome
            .failed
            .iter()
            .take(REPORT_FAILURES)
            .map(|(p, e)| CardFailure {
                path: p.to_string_lossy().into_owned(),
                error: e.clone(),
            })
            .collect();
        report.not_attempted = outcome.not_attempted;
        report.cancelled = outcome.cancelled;
        report.aborted = outcome.aborted.clone();

        // 5. whatever was verified is imported, whatever happened afterwards
        let mut imported: Vec<PathBuf> = outcome.copied.iter().map(|c| c.dst.clone()).collect();
        imported.extend(outcome.present.iter().cloned());
        if !imported.is_empty() {
            let root = common_dir(&imported, &job.dest_root);
            let title = job.title.clone().unwrap_or_else(|| job.source_name.clone());
            let session = self
                .import_with(
                    crate::ImportRequest {
                        path: root.to_string_lossy().into_owned(),
                        recursive: true,
                        title: Some(title),
                    },
                    crate::ImportOptions::default(),
                )
                .await?;
            report.session_id = Some(session.id);
        }
        Ok(())
    }
}

/// The end state of a task and its error text, from what the report says.
fn card_outcome(r: &CardReport) -> (&'static str, Option<String>) {
    let imported = r.copied + r.reused;
    if r.cancelled {
        return (CANCELLED, None);
    }
    if let Some(msg) = &r.aborted {
        return if imported == 0 {
            (FAILED, Some(msg.clone()))
        } else {
            (
                DONE,
                Some(format!(
                    "stopped after {imported} files, {} not copied: {msg}",
                    r.not_attempted + r.failed_count
                )),
            )
        };
    }
    if r.failed_count > 0 {
        let first = r
            .failed
            .first()
            .map(|f| f.error.clone())
            .unwrap_or_default();
        return if imported == 0 {
            (FAILED, Some(first))
        } else {
            (
                DONE,
                Some(format!(
                    "{} of {} failed; first: {first}",
                    r.failed_count,
                    imported + r.failed_count
                )),
            )
        };
    }
    (DONE, None)
}

#[cfg(test)]
#[path = "card_tests.rs"]
mod tests;
