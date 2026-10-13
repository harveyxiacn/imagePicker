//! Diagnostics bundle (`POST /api/system/diagnostics`, `docs/api-contract-m7.md` "Diagnostics").
//!
//! A zip a user can attach to a bug report. It holds versions, an OS / hardware summary, the
//! settings (secrets removed), recent logs and crash records (paths and secrets scrubbed) and
//! catalog statistics. It never holds photos, thumbnails, previews, face crops, embeddings, the
//! catalog database itself or the path of any photo (root folders appear by name only).

use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::SystemTime;

use rusqlite::Connection;
use serde_json::{json, Value};

use crate::db::MIGRATIONS;
use crate::error::{CoreError, Result};
use crate::logging::{base_name, iso_utc, scrub, stamp_utc, utc_parts, LOG_PREFIX, LOG_SUFFIX};
use crate::Core;

/// Per file and in total, how much log / crash text goes into the bundle (the newest part).
const LOG_FILE_TAIL: u64 = 1024 * 1024;
const LOG_TOTAL_TAIL: u64 = 4 * 1024 * 1024;
const CRASH_FILE_TAIL: u64 = 256 * 1024;
const CRASH_TOTAL_TAIL: u64 = 2 * 1024 * 1024;
const RECENT_TASKS: i64 = 20;

/// Key fragments whose values never leave the machine.
const SECRET_KEYS: [&str; 11] = [
    "password",
    "passwd",
    "secret",
    "token",
    "api_key",
    "apikey",
    "credential",
    "authorization",
    "cookie",
    "hash",
    "salt",
];

#[derive(Debug, Clone)]
pub struct Bundle {
    /// `imagepicker-diagnostics-YYYYMMDD-HHMMSS.zip`
    pub file_name: String,
    pub bytes: Vec<u8>,
    /// Entry names in the zip.
    pub entries: Vec<String>,
}

// ------------------------------------------------------------------ redaction

/// A copy of `v` that is safe to share: values under secret-looking keys are replaced, URLs
/// keep only their presence (`[set]`), root lists shrink to folder names and every remaining
/// string goes through [`scrub`].
pub fn redact_settings(v: &Value) -> Value {
    fn walk(key: &str, v: &Value) -> Value {
        let k = key.to_ascii_lowercase();
        if SECRET_KEYS.iter().any(|s| k.contains(s)) {
            return if v.is_null() || v.as_str() == Some("") {
                v.clone()
            } else {
                json!("[redacted]")
            };
        }
        if k == "url" || k.ends_with("_url") {
            return match v.as_str() {
                Some("") | None => v.clone(),
                Some(_) => json!("[set]"),
            };
        }
        if k == "roots" {
            if let Value::Array(a) = v {
                let names: Vec<String> = a
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|p| scrub(&base_name(p)))
                    .collect();
                return json!({"count": a.len(), "names": names});
            }
        }
        match v {
            Value::Object(m) => {
                Value::Object(m.iter().map(|(k, x)| (k.clone(), walk(k, x))).collect())
            }
            Value::Array(a) => Value::Array(a.iter().map(|x| walk(key, x)).collect()),
            Value::String(s) => Value::String(scrub(s)),
            other => other.clone(),
        }
    }
    walk("", v)
}

// ------------------------------------------------------------------ zip

/// A minimal zip writer (deflate, no zip64, UTF-8 names): enough for a few dozen small files.
pub struct ZipWriter {
    out: Vec<u8>,
    central: Vec<u8>,
    count: u16,
    dos: (u16, u16),
}

fn dos_time(t: SystemTime) -> (u16, u16) {
    let (y, mo, d, h, mi, s) = utc_parts(t);
    let date = (((y.clamp(1980, 2107) - 1980) as u16) << 9) | ((mo as u16) << 5) | d as u16;
    let time = ((h as u16) << 11) | ((mi as u16) << 5) | (s as u16 / 2);
    (time, date)
}

impl ZipWriter {
    pub fn new() -> Self {
        Self {
            out: Vec::new(),
            central: Vec::new(),
            count: 0,
            dos: dos_time(SystemTime::now()),
        }
    }

    pub fn add(&mut self, name: &str, data: &[u8]) -> Result<()> {
        let mut enc =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(data)?;
        let packed = enc.finish()?;
        let (method, body): (u16, &[u8]) = if packed.len() < data.len() {
            (8, &packed)
        } else {
            (0, data)
        };
        if self.count == u16::MAX || self.out.len() as u64 + body.len() as u64 > u32::MAX as u64 / 2
        {
            return Err(CoreError::Internal(anyhow::anyhow!(
                "diagnostics bundle too large"
            )));
        }
        let crc = crc32fast::hash(data);
        let offset = self.out.len() as u32;
        let mut common = Vec::new();
        common.extend_from_slice(&20u16.to_le_bytes()); // version needed
        common.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 names
        common.extend_from_slice(&method.to_le_bytes());
        common.extend_from_slice(&self.dos.0.to_le_bytes());
        common.extend_from_slice(&self.dos.1.to_le_bytes());
        common.extend_from_slice(&crc.to_le_bytes());
        common.extend_from_slice(&(body.len() as u32).to_le_bytes());
        common.extend_from_slice(&(data.len() as u32).to_le_bytes());
        common.extend_from_slice(&(name.len() as u16).to_le_bytes());
        common.extend_from_slice(&0u16.to_le_bytes()); // extra
        self.out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        self.out.extend_from_slice(&common);
        self.out.extend_from_slice(name.as_bytes());
        self.out.extend_from_slice(body);

        self.central
            .extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        self.central.extend_from_slice(&20u16.to_le_bytes()); // made by
        self.central.extend_from_slice(&common);
        self.central.extend_from_slice(&0u16.to_le_bytes()); // comment
        self.central.extend_from_slice(&0u16.to_le_bytes()); // disk
        self.central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        self.central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        self.central.extend_from_slice(&offset.to_le_bytes());
        self.central.extend_from_slice(name.as_bytes());
        self.count += 1;
        Ok(())
    }

    pub fn finish(mut self) -> Vec<u8> {
        let cd_offset = self.out.len() as u32;
        let cd_len = self.central.len() as u32;
        self.out.extend_from_slice(&self.central);
        self.out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        self.out.extend_from_slice(&[0, 0, 0, 0]); // disk numbers
        self.out.extend_from_slice(&self.count.to_le_bytes());
        self.out.extend_from_slice(&self.count.to_le_bytes());
        self.out.extend_from_slice(&cd_len.to_le_bytes());
        self.out.extend_from_slice(&cd_offset.to_le_bytes());
        self.out.extend_from_slice(&0u16.to_le_bytes());
        self.out
    }
}

impl Default for ZipWriter {
    fn default() -> Self {
        Self::new()
    }
}

// ------------------------------------------------------------------ catalog statistics

fn count(c: &Connection, sql: &str) -> i64 {
    c.query_row(sql, [], |r| r.get(0)).unwrap_or(-1)
}

/// Counts and health of the catalog; no row content apart from root folder names and task
/// kinds / states.
fn catalog_stats(c: &Connection) -> Result<Value> {
    let user_version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    // the full check, not the quick one: this is what a bug report is for
    let mut problems: Vec<String> = Vec::new();
    match c.prepare("PRAGMA integrity_check") {
        Ok(mut st) => match st.query_map([], |r| r.get::<_, String>(0)) {
            Ok(rows) => {
                for row in rows {
                    match row {
                        Ok(l) if l == "ok" => {}
                        Ok(l) => problems.push(scrub(&l)),
                        Err(e) => problems.push(scrub(&e.to_string())),
                    }
                }
            }
            Err(e) => problems.push(scrub(&e.to_string())),
        },
        Err(e) => problems.push(scrub(&e.to_string())),
    }
    let mut roots = Vec::new();
    if let Ok(mut st) = c.prepare(
        "SELECT r.path, COUNT(p.id) FROM root_folder r LEFT JOIN photo p ON p.root_id = r.id \
         GROUP BY r.id ORDER BY r.id",
    ) {
        if let Ok(rows) = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))) {
            for (path, photos) in rows.flatten() {
                roots.push(json!({"name": scrub(&base_name(&path)), "photos": photos}));
            }
        }
    }
    let mut tasks = Vec::new();
    if let Ok(mut st) = c.prepare(
        "SELECT kind, status, error, updated_at FROM task ORDER BY updated_at DESC LIMIT ?1",
    ) {
        if let Ok(rows) = st.query_map([RECENT_TASKS], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<i64>>(3)?,
            ))
        }) {
            for (kind, status, error, at) in rows.flatten() {
                tasks.push(json!({
                    "kind": kind, "status": status,
                    "error": error.map(|e| scrub(&e)), "updated_at": at,
                }));
            }
        }
    }
    Ok(json!({
        "schema_version": user_version,
        "expected_schema_version": MIGRATIONS.len(),
        "integrity_check": {"ok": problems.is_empty(), "problems": problems},
        "counts": {
            "root_folders": count(c, "SELECT COUNT(*) FROM root_folder"),
            "sessions": count(c, "SELECT COUNT(*) FROM session"),
            "photos": count(c, "SELECT COUNT(*) FROM photo"),
            "photos_missing": count(c, "SELECT COUNT(*) FROM photo WHERE missing<>0"),
            "photos_analyzed": count(c, "SELECT COUNT(*) FROM analysis"),
            "photos_with_edits": count(c, "SELECT COUNT(*) FROM photo WHERE has_edits<>0"),
            "bursts": count(c, "SELECT COUNT(*) FROM burst"),
            "faces": count(c, "SELECT COUNT(*) FROM face"),
            "people": count(c, "SELECT COUNT(*) FROM person"),
            "tags": count(c, "SELECT COUNT(DISTINCT tag) FROM photo_tag"),
            "presets": count(c, "SELECT COUNT(*) FROM preset"),
            "collections": count(c, "SELECT COUNT(*) FROM smart_collection"),
            "tasks": count(c, "SELECT COUNT(*) FROM task"),
        },
        "roots": roots,
        "recent_tasks": tasks,
    }))
}

// ------------------------------------------------------------------ files

/// The last `max` bytes of a text file, starting at a line boundary, scrubbed.
fn tail_scrubbed(path: &Path, max: u64) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let mut start = bytes.len().saturating_sub(max as usize);
    if start > 0 {
        if let Some(nl) = bytes[start..].iter().position(|b| *b == b'\n') {
            start += nl + 1;
        }
    }
    Some(scrub(&String::from_utf8_lossy(&bytes[start..])))
}

/// Newest-first text files of `dir` accepted by `keep`, as `(name, scrubbed tail)` within the
/// per-file and total budgets.
fn recent_texts(
    dir: &Path,
    keep: impl Fn(&str) -> bool,
    per_file: u64,
    total: u64,
) -> Vec<(String, String)> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<(SystemTime, String)> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let md = e.metadata().ok()?;
            (md.is_file() && keep(&name))
                .then(|| (md.modified().unwrap_or(SystemTime::UNIX_EPOCH), name))
        })
        .collect();
    files.sort_by(|a, b| b.cmp(a));
    let mut left = total;
    let mut out = Vec::new();
    for (_, name) in files {
        if left == 0 {
            break;
        }
        if let Some(text) = tail_scrubbed(&dir.join(&name), per_file.min(left)) {
            left = left.saturating_sub(text.len() as u64);
            out.push((name, text));
        }
    }
    out
}

fn is_log_name(n: &str) -> bool {
    n.starts_with(LOG_PREFIX) && n.ends_with(LOG_SUFFIX)
}

fn is_crash_name(n: &str) -> bool {
    // `crash-*.txt` (Rust), `worker-*` (Python exception reports and faulthandler dumps)
    (n.starts_with("crash-") && n.ends_with(".txt"))
        || (n.starts_with("worker-") && (n.ends_with(".txt") || n.ends_with(".log")))
}

impl Core {
    fn system_summary(&self) -> Value {
        let st = self.worker.status();
        let info = st.info.as_ref();
        json!({
            "app": {"name": "imagePicker", "version": env!("CARGO_PKG_VERSION")},
            "os": {
                "name": std::env::consts::OS,
                "family": std::env::consts::FAMILY,
                "arch": std::env::consts::ARCH,
            },
            "hardware": {
                "logical_cpus": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
                "render_backend": self.settings().render.backend,
                "worker_device": info.map(|i| i.hardware.device.clone()),
                "gpus": info.map(|i| i.hardware.gpus.iter()
                    .map(|g| json!({"name": scrub(&g.name), "vram_mb": g.vram_mb}))
                    .collect::<Vec<_>>()).unwrap_or_default(),
            },
            "worker": {
                "state": st.state.as_str(),
                "tier": st.tier,
                "providers": info.map(|i| i.providers.clone()).unwrap_or_default(),
                "error": st.error.map(|e| scrub(&e)),
                "runtime_version": self.runtime.version(),
                "package_version": env!("CARGO_PKG_VERSION"),
            },
        })
    }

    /// Builds the diagnostics zip. `settings` is the settings document the API serves (including
    /// the LAN block and the roots); it is redacted here.
    pub async fn diagnostics_bundle(
        self: &std::sync::Arc<Self>,
        settings: Value,
    ) -> Result<Bundle> {
        let now = SystemTime::now();
        let catalog = self.db.call(|c| catalog_stats(c)).await?;
        let mut catalog = catalog;
        if let Value::Object(m) = &mut catalog {
            let cache = self.cache_info().await?;
            m.insert("state".into(), json!(self.catalog_guard.state()));
            m.insert(
                "sizes".into(),
                json!({
                    "catalog_bytes": fs::metadata(&self.dirs.catalog).map(|m| m.len()).ok(),
                    "cache_bytes": cache.bytes,
                    "cache_items": cache.items,
                }),
            );
        }
        let system = self.system_summary();
        let settings = redact_settings(&settings);
        let logs_dir = self.dirs.logs.clone();
        let crashes_dir = self.dirs.crashes.clone();
        let (logs, crashes) = tokio::task::spawn_blocking(move || {
            (
                recent_texts(&logs_dir, is_log_name, LOG_FILE_TAIL, LOG_TOTAL_TAIL),
                recent_texts(
                    &crashes_dir,
                    is_crash_name,
                    CRASH_FILE_TAIL,
                    CRASH_TOTAL_TAIL,
                ),
            )
        })
        .await
        .map_err(|e| CoreError::Internal(anyhow::anyhow!("diagnostics failed: {e}")))?;

        let pretty = |v: &Value| serde_json::to_vec_pretty(v).unwrap_or_default();
        let mut files: Vec<(String, Vec<u8>)> = vec![
            ("system.json".into(), pretty(&system)),
            ("settings.json".into(), pretty(&settings)),
            ("catalog.json".into(), pretty(&catalog)),
        ];
        for (n, t) in logs {
            files.push((format!("logs/{n}"), t.into_bytes()));
        }
        for (n, t) in crashes {
            files.push((format!("crashes/{n}"), t.into_bytes()));
        }
        let listing: Vec<Value> = files
            .iter()
            .map(|(n, b)| json!({"name": n, "bytes": b.len()}))
            .collect();
        let manifest = json!({
            "format": 1,
            "generated_at": iso_utc(now),
            "app_version": env!("CARGO_PKG_VERSION"),
            "files": listing,
            "excluded": [
                "photos, thumbnails, previews, face crops and masks",
                "face embeddings and every other catalog row (only counts are included)",
                "the catalog database and its backups",
                "photo paths (root folders appear by name only; paths in logs are masked)",
                "passwords, tokens, device pairings and other secrets",
            ],
        });
        let mut zip = ZipWriter::new();
        let mut entries = Vec::new();
        zip.add("manifest.json", &pretty(&manifest))?;
        entries.push("manifest.json".to_string());
        for (n, b) in files {
            zip.add(&n, &b)?;
            entries.push(n);
        }
        Ok(Bundle {
            file_name: format!("imagepicker-diagnostics-{}.zip", stamp_utc(now)),
            bytes: zip.finish(),
            entries,
        })
    }
}

/// A small zip reader for tests (stored and deflated entries).
#[cfg(any(test, feature = "testutil"))]
pub fn read_zip(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    use std::io::Read;
    let u16at = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]) as usize;
    let u32at = |o: usize| {
        u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]) as usize
    };
    let eocd = (0..=bytes.len() - 22)
        .rev()
        .find(|&o| u32at(o) == 0x0605_4b50)
        .expect("end of central directory");
    let n = u16at(eocd + 10);
    let mut p = u32at(eocd + 16);
    let mut out = Vec::new();
    for _ in 0..n {
        assert_eq!(u32at(p), 0x0201_4b50);
        let (method, crc, csize, usize_) =
            (u16at(p + 10), u32at(p + 16), u32at(p + 20), u32at(p + 24));
        let (nl, el, cl) = (u16at(p + 28), u16at(p + 30), u16at(p + 32));
        let off = u32at(p + 42);
        let name = String::from_utf8(bytes[p + 46..p + 46 + nl].to_vec()).unwrap();
        assert_eq!(u32at(off), 0x0403_4b50);
        let start = off + 30 + u16at(off + 26) + u16at(off + 28);
        let raw = &bytes[start..start + csize];
        let data = match method {
            0 => raw.to_vec(),
            8 => {
                let mut v = Vec::new();
                flate2::read::DeflateDecoder::new(raw)
                    .read_to_end(&mut v)
                    .unwrap();
                v
            }
            m => panic!("method {m}"),
        };
        assert_eq!(data.len(), usize_);
        assert_eq!(crc32fast::hash(&data) as usize, crc, "crc of {name}");
        out.push((name, data));
        p += 46 + nl + el + cl;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip_roundtrip() {
        let mut z = ZipWriter::new();
        z.add("a.txt", b"hello hello hello hello hello hello")
            .unwrap();
        z.add("dir/b.bin", &[1, 2, 3]).unwrap();
        z.add("empty", b"").unwrap();
        let got = read_zip(&z.finish());
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].0, "a.txt");
        assert_eq!(got[1], ("dir/b.bin".to_string(), vec![1, 2, 3]));
        assert!(got[2].1.is_empty());
    }

    #[test]
    fn settings_are_redacted() {
        let v = json!({
            "language": "en",
            "lan": {"enabled": true, "password": "hunter2", "port": 7878},
            "remote_ai": {"enabled": true, "host_url": "http://192.168.1.9:7979", "api_token": "tok-123"},
            "models": {"dir": "D:\\Models\\imagePicker"},
            "roots": ["D:\\Photos\\Trip 2024", "/mnt/pics/family"],
            "empty": {"password": ""},
        });
        let r = redact_settings(&v);
        let text = r.to_string();
        for leak in [
            "hunter2",
            "tok-123",
            "192.168.1.9",
            "D:\\\\Photos",
            "/mnt/pics",
        ] {
            assert!(!text.contains(leak), "{leak} leaked: {text}");
        }
        assert_eq!(r["lan"]["port"], 7878);
        assert_eq!(r["lan"]["password"], "[redacted]");
        assert_eq!(r["empty"]["password"], "");
        assert_eq!(r["remote_ai"]["host_url"], "[set]");
        assert_eq!(r["roots"]["count"], 2);
        assert_eq!(r["roots"]["names"][0], "Trip 2024");
        assert_eq!(r["models"]["dir"], "<path>/imagePicker");
    }
}
