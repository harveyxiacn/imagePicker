//! Parallel directory scan (rayon over sub-directories; `DirEntry::metadata` is free on Windows).
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use rayon::prelude::*;

use crate::{ImageFormat, Result, ScannedFile};

struct Ctx<'a> {
    recursive: bool,
    exclude: &'a [Exclude],
}

/// One normalised exclude pattern (see [`scan_dir`]).
struct Exclude {
    pat: String,
    glob: bool,
    has_slash: bool,
}

impl Exclude {
    fn parse(raw: &str, root_lc: &str) -> Option<Self> {
        let mut p = raw.trim().to_lowercase().replace('\\', "/");
        // an absolute path inside the scanned folder is the same as its relative form
        if !root_lc.is_empty() {
            if let Some(rest) = p.strip_prefix(root_lc) {
                if rest.is_empty() || rest.starts_with('/') {
                    p = rest.to_string();
                }
            }
        }
        while let Some(rest) = p.strip_prefix("./") {
            p = rest.to_string();
        }
        let p = p.trim_matches('/').to_string();
        if p.is_empty() {
            return None;
        }
        Some(Self {
            glob: p.contains(['*', '?']),
            has_slash: p.contains('/'),
            pat: p,
        })
    }

    /// `rel` is the lowercase `/`-separated path of the entry below the scanned root.
    fn matches(&self, rel: &str) -> bool {
        if self.glob {
            if self.has_slash {
                glob_match(&self.pat, rel)
            } else {
                // a pattern without `/` applies to the entry's own name at any depth
                let name = rel.rsplit('/').next().unwrap_or(rel);
                glob_match(&self.pat, name)
            }
        } else if self.has_slash {
            rel == self.pat
                || rel
                    .strip_prefix(self.pat.as_str())
                    .is_some_and(|r| r.starts_with('/'))
        } else {
            rel == self.pat || rel.rsplit('/').next() == Some(self.pat.as_str())
        }
    }
}

/// `*` and `?` stay inside one path segment, `**` crosses `/`.
pub(crate) fn glob_match(pat: &str, text: &str) -> bool {
    fn go(p: &[u8], t: &[u8]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some(b'*') => {
                let double = p.get(1) == Some(&b'*');
                let rest = if double { &p[2..] } else { &p[1..] };
                // `**/` also matches zero directories
                if double && rest.first() == Some(&b'/') && go(&rest[1..], t) {
                    return true;
                }
                let mut i = 0;
                loop {
                    if go(rest, &t[i..]) {
                        return true;
                    }
                    if i == t.len() || (!double && t[i] == b'/') {
                        return false;
                    }
                    i += 1;
                }
            }
            Some(b'?') => !t.is_empty() && t[0] != b'/' && go(&p[1..], &t[1..]),
            Some(c) => t.first() == Some(c) && go(&p[1..], &t[1..]),
        }
    }
    go(pat.as_bytes(), text.as_bytes())
}

fn is_hidden(name: &str, _md: Option<&fs::Metadata>) -> bool {
    if name.starts_with('.') {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if let Some(md) = _md {
            // FILE_ATTRIBUTE_HIDDEN
            return md.file_attributes() & 0x2 != 0;
        }
    }
    false
}

fn walk(dir: &Path, rel: &str, ctx: &Ctx) -> Vec<ScannedFile> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    let mut subdirs: Vec<(PathBuf, String)> = Vec::new();
    for ent in rd.flatten() {
        let name_os = ent.file_name();
        let Some(name) = name_os.to_str() else {
            continue;
        };
        let Ok(ft) = ent.file_type() else { continue };
        let mut md = if ft.is_symlink() {
            fs::metadata(ent.path()).ok()
        } else {
            None
        };
        let is_dir = if ft.is_symlink() {
            md.as_ref().is_some_and(|m| m.is_dir())
        } else {
            ft.is_dir()
        };
        let is_file = if ft.is_symlink() {
            md.as_ref().is_some_and(|m| m.is_file())
        } else {
            ft.is_file()
        };
        if !is_dir && !is_file {
            continue;
        }
        if is_dir && (ft.is_symlink() || !ctx.recursive) {
            continue; // never follow directory symlinks (loops)
        }
        if md.is_none() {
            md = ent.metadata().ok();
        }
        if is_hidden(name, md.as_ref()) {
            continue;
        }
        // where "reject -> move to a folder" puts files: not part of the library any more
        if is_dir && name.eq_ignore_ascii_case("_rejected") {
            continue;
        }
        let rel_child = if rel.is_empty() {
            name.to_lowercase()
        } else {
            format!("{rel}/{}", name.to_lowercase())
        };
        if ctx.exclude.iter().any(|x| x.matches(&rel_child)) {
            continue;
        }
        let path = ent.path();
        if is_dir {
            subdirs.push((path, rel_child));
            continue;
        }
        let Some(format) = ImageFormat::from_path(&path) else {
            continue;
        };
        let Some(md) = md else { continue };
        let mtime_ms = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        files.push(ScannedFile {
            path,
            size: md.len(),
            mtime_ms,
            format,
        });
    }
    if !subdirs.is_empty() {
        let sub: Vec<Vec<ScannedFile>> = subdirs.par_iter().map(|(p, r)| walk(p, r, ctx)).collect();
        for s in sub {
            files.extend(s);
        }
    }
    files
}

pub fn scan_dir(root: &Path, recursive: bool, exclude: &[String]) -> Result<Vec<ScannedFile>> {
    let md =
        fs::metadata(root).map_err(|e| anyhow::anyhow!("cannot scan {}: {e}", root.display()))?;
    if !md.is_dir() {
        anyhow::bail!("not a directory: {}", root.display());
    }
    // Excludes match case-insensitively against the path relative to `root` ('/'-separated):
    // a glob (`*`, `?`, `**`), a sub-folder path (also absolute, inside `root`) or a plain
    // name (any file or folder of that name).
    let root_lc = root.to_string_lossy().to_lowercase().replace('\\', "/");
    let root_lc = root_lc.trim_end_matches('/').to_string();
    let ex: Vec<Exclude> = exclude
        .iter()
        .filter_map(|s| Exclude::parse(s, &root_lc))
        .collect();
    let ctx = Ctx {
        recursive,
        exclude: &ex,
    };
    let mut v = walk(root, "", &ctx);
    v.par_sort_unstable_by(|a, b| a.path.cmp(&b.path));
    Ok(v)
}

#[cfg(test)]
mod exclude_tests {
    use super::*;

    fn ex(p: &str) -> Exclude {
        Exclude::parse(p, "c:/photos").unwrap()
    }

    #[test]
    fn globs_and_folders() {
        assert!(ex("*.dng").matches("a/b/x.dng"));
        assert!(!ex("*.dng").matches("a/b/x.jpg"));
        assert!(ex("backup/**").matches("backup/2020/x.jpg"));
        assert!(ex("**/thumbs").matches("a/thumbs"));
        assert!(ex("**/thumbs").matches("thumbs"));
        assert!(ex("2020/*/raw").matches("2020/jan/raw"));
        assert!(!ex("2020/*/raw").matches("2020/jan/feb/raw"));
        assert!(ex("2020/**/raw").matches("2020/jan/feb/raw"));
        // sub-folder paths: prefix on segment boundaries, absolute form allowed
        assert!(ex("Trip/Rejects").matches("trip/rejects"));
        assert!(ex("Trip/Rejects/").matches("trip/rejects/x.jpg"));
        assert!(!ex("trip/rej").matches("trip/rejects"));
        assert!(ex("C:\\Photos\\Trip").matches("trip"));
        // plain name anywhere
        assert!(ex("cache").matches("a/b/cache"));
        assert!(!ex("cache").matches("a/b/cachey"));
        assert!(Exclude::parse("  ", "").is_none());
    }
}
