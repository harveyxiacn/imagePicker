//! Filesystem whitelist: every client-supplied path (folder browser, import, export destination,
//! LUT import ...) must resolve inside one of the allowed roots.
//!
//! Resolution is lexical first (no `..`, no NUL, no Windows UNC / device / verbatim / drive
//! relative paths, no alternate data streams or reserved device names) and then physical: the
//! longest existing prefix is canonicalised (symlinks and junctions resolved) and the result must
//! be inside a canonicalised root. The returned path is the canonical one, so callers operate on
//! exactly what was checked.

use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct PathError(pub String);

fn forbidden<T>(why: impl Into<String>) -> Result<T, PathError> {
    Err(PathError(why.into()))
}

/// Windows reserved device names (with or without an extension).
pub fn is_reserved_windows_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").trim_end();
    let up = stem.to_ascii_uppercase();
    matches!(
        up.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ((up.starts_with("COM") || up.starts_with("LPT"))
        && up.len() == 4
        && up.as_bytes()[3].is_ascii_digit()
        && up.as_bytes()[3] != b'0')
}

/// Checks that need no filesystem access. `windows` selects the Windows-specific rules (it is
/// `cfg!(windows)` in production; a parameter so the rules are testable on every OS).
pub fn lexical_check(raw: &str, windows: bool) -> Result<(), PathError> {
    if raw.trim().is_empty() {
        return forbidden("empty path");
    }
    if raw.contains('\0') {
        return forbidden("path contains a NUL byte");
    }
    // UNC (`\\server\share`), device (`\\.\`), verbatim (`\\?\`) and `//` forms.
    if raw.starts_with("\\\\")
        || raw.starts_with("//")
        || raw.starts_with("\\/")
        || raw.starts_with("/\\")
    {
        return forbidden("network and device paths are not allowed");
    }
    // `..` is rejected on both separators regardless of platform.
    if raw.split(['/', '\\']).any(|seg| seg.trim() == "..") {
        return forbidden("path traversal (..) is not allowed");
    }
    if windows {
        let b = raw.as_bytes();
        let drive = b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && (b[2] == b'\\' || b[2] == b'/');
        if !drive {
            return forbidden("path must be absolute with a drive letter");
        }
        for seg in raw[3..].split(['/', '\\']) {
            if seg.contains(':') {
                return forbidden("alternate data streams are not allowed");
            }
            if is_reserved_windows_name(seg) {
                return forbidden("reserved device names are not allowed");
            }
            if seg.contains(['<', '>', '"', '|', '?', '*']) {
                return forbidden("invalid characters in path");
            }
        }
    } else if !raw.starts_with('/') {
        return forbidden("path must be absolute");
    }
    Ok(())
}

/// Canonicalises `p`; the part that does not exist yet (e.g. an export folder to be created) is
/// appended to the canonical form of its longest existing ancestor.
fn canonical_with_tail(p: &Path) -> io::Result<PathBuf> {
    let norm: PathBuf = p.components().collect();
    let mut cur = norm;
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        match std::fs::symlink_metadata(&cur) {
            Ok(_) => {
                // Errors for dangling symlinks / loops: treated as forbidden by the caller.
                let mut c = std::fs::canonicalize(&cur)?;
                for t in tail.iter().rev() {
                    c.push(t);
                }
                return Ok(c);
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let name = cur
                    .file_name()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such root"))?
                    .to_os_string();
                tail.push(name);
                if !cur.pop() {
                    return Err(io::Error::new(io::ErrorKind::NotFound, "no such root"));
                }
            }
            Err(e) => return Err(e),
        }
    }
}

/// `\\?\C:\x` -> `C:\x` (only plain drive paths; `\\?\UNC\...` is left alone).
pub fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        let b = rest.as_bytes();
        if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
            return PathBuf::from(rest);
        }
    }
    p
}

/// Removable drives / mount points (USB sticks, SD cards).
pub fn removable_roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    #[cfg(windows)]
    {
        extern "system" {
            fn GetDriveTypeW(root: *const u16) -> u32;
        }
        const DRIVE_REMOVABLE: u32 = 2;
        for letter in b'A'..=b'Z' {
            let root = format!("{}:\\", letter as char);
            let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
            // SAFETY: `wide` is a valid NUL-terminated UTF-16 string for the call's duration.
            let kind = unsafe { GetDriveTypeW(wide.as_ptr()) };
            if kind == DRIVE_REMOVABLE {
                out.push(PathBuf::from(root));
            }
        }
    }
    #[cfg(not(windows))]
    {
        let mut parents: Vec<PathBuf> = vec![PathBuf::from("/Volumes"), PathBuf::from("/mnt")];
        if let Some(user) = std::env::var_os("USER") {
            parents.push(PathBuf::from("/media").join(&user));
            parents.push(PathBuf::from("/run/media").join(&user));
        }
        for parent in parents {
            if let Ok(rd) = std::fs::read_dir(&parent) {
                out.extend(rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
            }
        }
    }
    out
}

/// Built-in roots: user home, Pictures and removable drives.
pub fn default_roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    out.extend(dirs::home_dir());
    out.extend(dirs::picture_dir().filter(|p| p.is_dir()));
    out.extend(removable_roots());
    out
}

/// A resolved set of allowed roots (canonical paths).
#[derive(Debug, Clone, Default)]
pub struct RootSet {
    canon: Vec<PathBuf>,
}

impl RootSet {
    /// `builtin` roots that resolve to a filesystem root (e.g. macOS `/Volumes/Macintosh HD`
    /// -> `/`) are dropped; `explicit` ones (user configured, imported) are kept as given.
    pub fn new(builtin: &[PathBuf], explicit: &[PathBuf]) -> Self {
        let mut canon: Vec<PathBuf> = Vec::new();
        let mut add = |p: &Path, allow_fs_root: bool| {
            if let Ok(c) = std::fs::canonicalize(p) {
                if (allow_fs_root || c.parent().is_some()) && !canon.contains(&c) {
                    canon.push(c);
                }
            }
        };
        for p in builtin {
            add(p, false);
        }
        for p in explicit {
            add(p, true);
        }
        Self { canon }
    }

    /// Roots for display (verbatim prefixes removed).
    pub fn display(&self) -> Vec<PathBuf> {
        self.canon.iter().cloned().map(strip_verbatim).collect()
    }

    pub fn contains_canonical(&self, p: &Path) -> bool {
        self.canon.iter().any(|r| p.starts_with(r))
    }

    /// Resolves `raw` to a canonical path inside the roots.
    pub fn check(&self, raw: &str) -> Result<PathBuf, PathError> {
        lexical_check(raw, cfg!(windows))?;
        let canon = match canonical_with_tail(Path::new(raw)) {
            Ok(c) => c,
            Err(_) => return forbidden("path cannot be resolved"),
        };
        if !self.contains_canonical(&canon) {
            return forbidden("path is outside the allowed folders");
        }
        Ok(strip_verbatim(canon))
    }

    /// Parent directory of an already checked path, if it is still inside the roots (used by the
    /// folder browser so "up" never leaves the whitelist).
    pub fn parent_within(&self, p: &Path) -> Option<PathBuf> {
        let parent = p.parent()?;
        let c = std::fs::canonicalize(parent).ok()?;
        self.contains_canonical(&c).then(|| parent.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(root: &Path) -> RootSet {
        RootSet::new(&[], &[root.to_path_buf()])
    }

    #[test]
    fn inside_and_outside() {
        let d = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("sub")).unwrap();
        let rs = set(d.path());
        assert!(rs.check(&d.path().to_string_lossy()).is_ok());
        assert!(rs.check(&d.path().join("sub").to_string_lossy()).is_ok());
        // not yet existing (export folder)
        assert!(rs
            .check(&d.path().join("new/deeper").to_string_lossy())
            .is_ok());
        assert!(rs.check(&other.path().to_string_lossy()).is_err());
    }

    #[test]
    fn traversal_is_rejected() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("sub")).unwrap();
        let rs = set(d.path());
        let sep = std::path::MAIN_SEPARATOR;
        let p = format!("{}{sep}sub{sep}..{sep}..{sep}x", d.path().display());
        assert!(rs.check(&p).is_err());
        let p = format!("{}{sep}..", d.path().display());
        assert!(rs.check(&p).is_err());
        let p = format!("{}/sub/../sub", d.path().display());
        assert!(rs.check(&p).is_err(), "even harmless .. is refused");
    }

    #[test]
    fn sibling_prefix_is_not_inside() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("photos");
        let b = d.path().join("photos-private");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        let rs = set(&a);
        assert!(rs.check(&b.to_string_lossy()).is_err());
    }

    #[test]
    fn lexical_rules() {
        assert!(lexical_check("", false).is_err());
        assert!(lexical_check("rel/path", false).is_err());
        assert!(lexical_check("/a/\0/b", false).is_err());
        assert!(lexical_check("//server/share", false).is_err());
        assert!(lexical_check("\\\\server\\share\\x", true).is_err());
        assert!(lexical_check("\\\\?\\C:\\Users", true).is_err());
        assert!(lexical_check("\\\\.\\PhysicalDrive0", true).is_err());
        assert!(lexical_check("C:foo", true).is_err());
        assert!(lexical_check("\\Windows", true).is_err());
        assert!(lexical_check("C:\\a\\..\\b", true).is_err());
        assert!(lexical_check("C:\\a/..\\b", true).is_err());
        assert!(lexical_check("C:\\photos\\img.jpg:secret", true).is_err());
        assert!(lexical_check("C:\\photos\\NUL", true).is_err());
        assert!(lexical_check("C:\\photos\\com1.txt", true).is_err());
        assert!(lexical_check("C:\\photos\\a*b", true).is_err());
        assert!(lexical_check("C:\\Users\\me\\Pictures", true).is_ok());
        assert!(lexical_check("D:/Photos/2024", true).is_ok());
        assert!(lexical_check("/home/me/Pictures", false).is_ok());
        assert!(!is_reserved_windows_name("com0"));
        assert!(!is_reserved_windows_name("console"));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_rejected() {
        let d = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), d.path().join("link")).unwrap();
        let rs = set(d.path());
        assert!(rs.check(&d.path().join("link").to_string_lossy()).is_err());
        assert!(rs
            .check(&d.path().join("link/new").to_string_lossy())
            .is_err());
        // dangling symlink
        std::os::unix::fs::symlink(d.path().join("nope"), d.path().join("dangling")).unwrap();
        assert!(rs
            .check(&d.path().join("dangling").to_string_lossy())
            .is_err());
    }

    #[cfg(windows)]
    #[test]
    fn junction_escape_is_rejected_and_verbatim_stripped() {
        let d = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        // directory junction (no admin rights needed)
        let link = d.path().join("junc");
        let ok = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(outside.path())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        let rs = set(d.path());
        let inside = rs.check(&d.path().to_string_lossy()).unwrap();
        assert!(!inside.to_string_lossy().starts_with(r"\\?\"), "{inside:?}");
        if ok {
            assert!(rs.check(&link.to_string_lossy()).is_err());
            assert!(rs.check(&link.join("new").to_string_lossy()).is_err());
        }
        assert!(rs.check(r"\\localhost\c$\Windows").is_err());
        assert!(rs.check(r"\\?\C:\Windows").is_err());
    }

    #[test]
    fn builtin_fs_root_is_dropped() {
        let rs = RootSet::new(
            &[PathBuf::from(if cfg!(windows) { "C:\\" } else { "/" })],
            &[],
        );
        assert!(rs.display().is_empty());
    }

    #[test]
    fn parent_stops_at_the_root_boundary() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("sub")).unwrap();
        let rs = set(d.path());
        let sub = rs.check(&d.path().join("sub").to_string_lossy()).unwrap();
        assert!(rs.parent_within(&sub).is_some());
        let root = rs.check(&d.path().to_string_lossy()).unwrap();
        assert!(rs.parent_within(&root).is_none());
    }
}
