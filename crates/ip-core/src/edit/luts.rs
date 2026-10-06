//! LUT library: built-in procedural looks, imported `.cube` files in `<data_dir>/luts/` and
//! absolute `.cube` paths (docs/api-contract-m3.md section B, `POST /api/luts/import`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ip_render::{Lut3d, LutProvider};

use crate::error::{CoreError, Result};

/// Largest `.cube` file accepted for import / path references.
const MAX_CUBE_BYTES: u64 = 64 * 1024 * 1024;

/// Built-in looks are owned by ip-render (single source of truth for ids and maths).
fn is_builtin(id: &str) -> bool {
    ip_render::builtin_lut_ids().contains(&id)
}

fn build_builtin(id: &str) -> Option<Lut3d> {
    ip_render::builtin_lut(id)
}

/// A library id is a file stem made of `[A-Za-z0-9._-]`.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

fn is_cube(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("cube"))
}

pub struct LutLibrary {
    dir: PathBuf,
    cache: Mutex<HashMap<String, Arc<Lut3d>>>,
}

impl LutLibrary {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn cached(&self, key: &str) -> Option<Arc<Lut3d>> {
        self.cache.lock().unwrap().get(key).cloned()
    }

    fn store(&self, key: String, lut: Arc<Lut3d>) {
        let mut c = self.cache.lock().unwrap();
        if c.len() >= 32 {
            c.clear();
        }
        c.insert(key, lut);
    }

    fn load_file(&self, key: String, path: &Path) -> anyhow::Result<Option<Arc<Lut3d>>> {
        if let Some(l) = self.cached(&key) {
            return Ok(Some(l));
        }
        let md = match std::fs::metadata(path) {
            Ok(m) if m.is_file() => m,
            _ => return Ok(None),
        };
        if md.len() > MAX_CUBE_BYTES {
            anyhow::bail!("LUT file is too large: {}", path.display());
        }
        let text = std::fs::read_to_string(path)?;
        let lut = Arc::new(ip_render::parse_cube(&text)?);
        self.store(key, lut.clone());
        Ok(Some(lut))
    }

    /// Resolves a built-in id, an imported id or an absolute `.cube` path.
    pub fn resolve(&self, file: &str) -> anyhow::Result<Option<Arc<Lut3d>>> {
        if is_builtin(file) {
            let key = format!("builtin:{file}");
            if let Some(l) = self.cached(&key) {
                return Ok(Some(l));
            }
            if let Some(l) = build_builtin(file) {
                let l = Arc::new(l);
                self.store(key, l.clone());
                return Ok(Some(l));
            }
        }
        if valid_id(file) {
            let p = self.dir.join(format!("{file}.cube"));
            if p.is_file() {
                return self.load_file(format!("lib:{file}"), &p);
            }
        }
        let p = Path::new(file);
        if p.is_absolute() && is_cube(p) {
            let mtime = std::fs::metadata(p)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis())
                .unwrap_or(0);
            return self.load_file(format!("path:{file}:{mtime}"), p);
        }
        Ok(None)
    }

    /// Built-in ids (name = i18n key `lut.<id>`) followed by imported files, as
    /// `(id, name, builtin)`; imported entries are sorted by id.
    pub fn list(&self) -> Vec<(String, String, bool)> {
        let mut out: Vec<(String, String, bool)> = ip_render::builtin_lut_ids()
            .iter()
            .map(|id| (id.to_string(), format!("lut.{id}"), true))
            .collect();
        let mut imported: Vec<(String, String, bool)> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| is_cube(p))
            .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .filter(|id| valid_id(id) && !is_builtin(id))
            .map(|id| (id.clone(), id, false))
            .collect();
        imported.sort();
        out.extend(imported);
        out
    }

    /// Copies a `.cube` file into the library and returns `(id, name)`.
    pub fn import(&self, src: &str) -> Result<(String, String)> {
        let src = src.trim();
        if src.is_empty() {
            return Err(CoreError::bad_request("path must not be empty"));
        }
        let path = Path::new(src);
        if !is_cube(path) {
            return Err(CoreError::bad_request(
                "only .cube LUT files can be imported",
            ));
        }
        let md = std::fs::metadata(path)
            .map_err(|e| CoreError::bad_request(format!("cannot read {src}: {e}")))?;
        if !md.is_file() {
            return Err(CoreError::bad_request(format!("not a file: {src}")));
        }
        if md.len() > MAX_CUBE_BYTES {
            return Err(CoreError::bad_request("LUT file is larger than 64 MiB"));
        }
        let bytes = std::fs::read(path)
            .map_err(|e| CoreError::bad_request(format!("cannot read {src}: {e}")))?;
        let text = String::from_utf8(bytes.clone())
            .map_err(|_| CoreError::bad_request("LUT file is not valid UTF-8 text"))?;
        ip_render::parse_cube(&text)
            .map_err(|e| CoreError::bad_request(format!("invalid .cube file: {e:#}")))?;

        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "lut".into());
        let mut base: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect();
        base.truncate(60);
        let base = base.trim_matches('_').to_string();
        let base = if base.is_empty() {
            "lut".to_string()
        } else {
            base
        };
        std::fs::create_dir_all(&self.dir)?;
        let mut n = 0u32;
        loop {
            let id = if n == 0 {
                base.clone()
            } else {
                format!("{base}-{n}")
            };
            if is_builtin(&id) {
                n += 1;
                continue;
            }
            let dest = self.dir.join(format!("{id}.cube"));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&dest)
            {
                Ok(mut f) => {
                    use std::io::Write;
                    f.write_all(&bytes)?;
                    return Ok((id, name));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // the same file imported twice maps to the same id
                    if std::fs::read(&dest).map(|b| b == bytes).unwrap_or(false) {
                        return Ok((id, name));
                    }
                    n += 1;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
}

impl LutProvider for LutLibrary {
    fn lut(&self, file: &str) -> ip_render::Result<Option<Lut3d>> {
        Ok(self.resolve(file)?.map(|l| (*l).clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_well_formed() {
        for &id in ip_render::builtin_lut_ids() {
            let l = build_builtin(id).unwrap();
            assert_eq!(l.data.len(), (l.size * l.size * l.size) as usize);
            assert!(l.data.iter().flatten().all(|v| (0.0..=1.0).contains(v)));
        }
        // Monochrome; ip-render adds a hint of warm tone, so allow a slight tint.
        let bw = build_builtin("bw_classic").unwrap();
        assert!(bw
            .data
            .iter()
            .all(|c| (c[0] - c[1]).abs() < 0.02 && (c[1] - c[2]).abs() < 0.02));
    }

    #[test]
    fn id_validation() {
        assert!(valid_id("film_warm"));
        assert!(!valid_id("../x"));
        assert!(!valid_id(""));
        assert!(!valid_id(".hidden"));
    }

    #[test]
    fn builtin_resolves_without_files() {
        let d = tempfile::tempdir().unwrap();
        let lib = LutLibrary::new(d.path().join("luts"));
        assert!(lib.resolve("film_warm").unwrap().is_some());
        assert!(lib.resolve("no_such_look").unwrap().is_none());
    }

    #[test]
    fn import_rejects_bad_input() {
        let d = tempfile::tempdir().unwrap();
        let lib = LutLibrary::new(d.path().join("luts"));
        assert!(matches!(lib.import(""), Err(CoreError::BadRequest(_))));
        assert!(matches!(lib.import("x.txt"), Err(CoreError::BadRequest(_))));
        let missing = d.path().join("nope.cube");
        assert!(matches!(
            lib.import(missing.to_str().unwrap()),
            Err(CoreError::BadRequest(_))
        ));
    }
}
