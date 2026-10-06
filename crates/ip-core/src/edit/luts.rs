//! LUT library: built-in procedural looks, imported `.cube` files in `<data_dir>/luts/` and
//! absolute `.cube` paths (docs/api-contract-m3.md section B, `POST /api/luts/import`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ip_render::{Lut3d, LutProvider};

use crate::error::{CoreError, Result};

/// Grid size of the procedural built-in LUTs.
const BUILTIN_SIZE: u32 = 17;
/// Largest `.cube` file accepted for import / path references.
const MAX_CUBE_BYTES: u64 = 64 * 1024 * 1024;

/// Ids of the procedural looks shipped with the core.
pub const BUILTIN_LUTS: [&str; 6] = [
    "film_warm",
    "film_cool",
    "bw_classic",
    "teal_orange",
    "fade",
    "vivid",
];

fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Gentle S curve around mid grey, `k` in 0..1.
fn s_curve(x: f32, k: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x + k * (smoothstep(x) - x)
}

fn saturate(c: [f32; 3], s: f32) -> [f32; 3] {
    let l = luma(c);
    [l + (c[0] - l) * s, l + (c[1] - l) * s, l + (c[2] - l) * s]
}

fn curve3(c: [f32; 3], k: f32) -> [f32; 3] {
    [s_curve(c[0], k), s_curve(c[1], k), s_curve(c[2], k)]
}

fn builtin_fn(id: &str) -> Option<fn([f32; 3]) -> [f32; 3]> {
    Some(match id {
        "film_warm" => |c| {
            let c = [c[0] * 1.05 + 0.02, c[1] + 0.01, c[2] * 0.90 + 0.005];
            curve3(saturate(c, 0.92), 0.35)
        },
        "film_cool" => |c| {
            let c = [c[0] * 0.95, c[1] + 0.005, c[2] * 1.06 + 0.02];
            curve3(saturate(c, 0.9), 0.3)
        },
        "bw_classic" => |c| {
            let l = s_curve(luma(c), 0.45);
            [l, l, l]
        },
        "teal_orange" => |c| {
            let hi = smoothstep(luma(c));
            // shadows toward teal, highlights toward orange
            let t = [0.0, 0.05, 0.08];
            let o = [0.08, 0.02, -0.06];
            [
                c[0] + t[0] * (1.0 - hi) + o[0] * hi,
                c[1] + t[1] * (1.0 - hi) + o[1] * hi,
                c[2] + t[2] * (1.0 - hi) + o[2] * hi,
            ]
        },
        "fade" => |c| {
            let c = saturate(c, 0.85);
            [0.06 + c[0] * 0.9, 0.06 + c[1] * 0.9, 0.06 + c[2] * 0.9]
        },
        "vivid" => |c| curve3(saturate(c, 1.25), 0.25),
        _ => return None,
    })
}

fn build_builtin(id: &str) -> Option<Lut3d> {
    let f = builtin_fn(id)?;
    let n = BUILTIN_SIZE;
    let mut data = Vec::with_capacity((n * n * n) as usize);
    let step = 1.0 / (n - 1) as f32;
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let o = f([r as f32 * step, g as f32 * step, b as f32 * step]);
                data.push([
                    o[0].clamp(0.0, 1.0),
                    o[1].clamp(0.0, 1.0),
                    o[2].clamp(0.0, 1.0),
                ]);
            }
        }
    }
    Some(Lut3d { size: n, data })
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
        if BUILTIN_LUTS.contains(&file) {
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
            if BUILTIN_LUTS.contains(&id.as_str()) {
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
        for id in BUILTIN_LUTS {
            let l = build_builtin(id).unwrap();
            assert_eq!(l.data.len(), (l.size * l.size * l.size) as usize);
            assert!(l.data.iter().flatten().all(|v| (0.0..=1.0).contains(v)));
        }
        let bw = build_builtin("bw_classic").unwrap();
        assert!(bw.data.iter().all(|c| (c[0] - c[1]).abs() < 1e-6));
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
