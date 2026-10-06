//! Parallel directory scan (rayon over sub-directories; `DirEntry::metadata` is free on Windows).
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use rayon::prelude::*;

use crate::{ImageFormat, Result, ScannedFile};

struct Ctx<'a> {
    recursive: bool,
    exclude: &'a [String],
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
        let rel_child = if rel.is_empty() {
            name.to_lowercase()
        } else {
            format!("{rel}/{}", name.to_lowercase())
        };
        if ctx.exclude.iter().any(|x| rel_child.contains(x.as_str())) {
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
    // Excludes match case-insensitively against the path relative to `root` ('/'-separated).
    let ex: Vec<String> = exclude
        .iter()
        .map(|s| s.to_lowercase().replace('\\', "/"))
        .filter(|s| !s.is_empty())
        .collect();
    let ctx = Ctx {
        recursive,
        exclude: &ex,
    };
    let mut v = walk(root, "", &ctx);
    v.par_sort_unstable_by(|a, b| a.path.cmp(&b.path));
    Ok(v)
}
