//! Diagnostics bundle (gap P1-13): contents, and what must never be in it.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ip_render::Backend;
use serde_json::{json, Value};

use crate::diagnostics::read_zip;
use crate::fake_worker::{person, FakeFace, FakeSpec, FakeWorker};
use crate::testutil::{write_jpeg, FakeImaging, FakeRenderer};
use crate::*;

const WAIT: Duration = Duration::from_secs(30);
const PASSWORD: &str = "hunter2-canary-pw";
const TOKEN: &str = "tok-canary-9d41";

struct Env {
    core: Arc<Core>,
    worker: Arc<FakeWorker>,
    _data: tempfile::TempDir,
    src: tempfile::TempDir,
}

fn env() -> Env {
    let data = tempfile::tempdir().unwrap();
    let worker = Arc::new(FakeWorker::new());
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(2),
        worker: Some(worker.clone()),
        renderer: Some(Arc::new(FakeRenderer::new(Backend::Cpu))),
        force_cpu: true,
    })
    .unwrap();
    crate::testutil::grant_face_consent(&core);
    Env {
        core,
        worker,
        _data: data,
        src: tempfile::tempdir().unwrap(),
    }
}

fn photo(dir: &Path, name: &str, shade: u8) {
    write_jpeg(&dir.join(name), 320, 240, shade);
}

async fn import_and_analyze(e: &Env) -> i64 {
    let s = e
        .core
        .import(ImportRequest {
            path: e.src.path().to_string_lossy().into_owned(),
            recursive: true,
            title: None,
        })
        .await
        .unwrap();
    e.core.wait_session_ready(s.id, WAIT).await.unwrap();
    e.core
        .analysis_run(AnalysisRunRequest {
            session_id: s.id,
            profile: Profile::Standard,
            photo_ids: None,
            force: false,
            allow_download: false,
            fallback_lite: false,
        })
        .await
        .unwrap();
    for _ in 0..1200 {
        if e.core.analysis_status(s.id).state != RunState::Running {
            return s.id;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("analysis did not finish");
}

fn find<'a>(entries: &'a [(String, Vec<u8>)], name: &str) -> &'a [u8] {
    &entries
        .iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("{name} missing from the bundle"))
        .1
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

#[allow(clippy::type_complexity)]
#[tokio::test]
async fn bundle_has_the_expected_entries_and_leaks_nothing() {
    let e = env();
    photo(e.src.path(), "family_0001.jpg", 10);
    photo(e.src.path(), "family_0002.jpg", 80);
    photo(e.src.path(), "family_0003.jpg", 150);
    for (i, n) in ["family_0001.jpg", "family_0002.jpg"].iter().enumerate() {
        let face = FakeFace::new([0.2, 0.2, 0.3, 0.4], person(1, i as f32 * 0.01));
        e.worker.set(n, FakeSpec::at(0.0).with_faces(vec![face]));
    }
    let sid = import_and_analyze(&e).await;

    // sensitive data present in the data directory: embeddings, thumbnails, a log and a crash
    // file that mention a photo path and secrets
    let (face_embs, photo_embs, photo_paths, file_names): (
        Vec<Vec<u8>>,
        Vec<Vec<u8>>,
        Vec<String>,
        Vec<String>,
    ) = e
        .core
        .db
        .call(|c| {
            let blobs = |sql: &str| -> rusqlite::Result<Vec<Vec<u8>>> {
                c.prepare(sql)?
                    .query_map([], |r| r.get::<_, Vec<u8>>(0))?
                    .collect()
            };
            let paths: Vec<(String, String)> = c
                .prepare(
                    "SELECT r.path, p.rel_path FROM photo p JOIN root_folder r ON r.id=p.root_id",
                )?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            Ok((
                blobs("SELECT embedding FROM face WHERE embedding IS NOT NULL")?,
                blobs("SELECT embedding FROM analysis WHERE embedding IS NOT NULL")?,
                paths
                    .iter()
                    .map(|(r, p)| Path::new(r).join(p).to_string_lossy().into_owned())
                    .collect(),
                paths.iter().map(|(_, p)| p.clone()).collect(),
            ))
        })
        .await
        .unwrap();
    assert_eq!(photo_paths.len(), 3);
    assert!(
        !face_embs.is_empty(),
        "the fixture must contain face embeddings"
    );
    assert!(
        !photo_embs.is_empty(),
        "the fixture must contain photo embeddings"
    );
    // a cached thumbnail exists on disk
    let thumbs: Vec<_> = walk(&e.core.dirs.thumbs);
    assert!(!thumbs.is_empty(), "the fixture must contain thumbnails");

    let first = photo_paths[0].clone();
    std::fs::write(
        e.core.dirs.logs.join("imagepicker-20260101.0.log"),
        format!(
            "2026-01-01T00:00:00Z ERROR original file is missing: {first}\n\
             2026-01-01T00:00:01Z INFO login password={PASSWORD} token={TOKEN}\n\
             2026-01-01T00:00:02Z INFO GET /api/thumb/3 200\n"
        ),
    )
    .unwrap();
    std::fs::write(
        e.core.dirs.crashes.join("crash-20260101-000000-1-0.txt"),
        format!("kind: rust-panic\nmessage: cannot open {first}\n\nbacktrace:\n  at /home/runner/work/x/src/main.rs:1:2\n"),
    )
    .unwrap();
    std::fs::write(
        e.core
            .dirs
            .crashes
            .join("worker-crash-20260101-000000-1-0.txt"),
        "kind: python-unhandled\nmessage: ValueError: boom\n",
    )
    .unwrap();

    let mut settings = serde_json::to_value(&*e.core.settings()).unwrap();
    settings["lan"] =
        json!({"enabled": true, "port": 7878, "password": PASSWORD, "guest_enabled": false});
    settings["remote_ai"]["token"] = json!(TOKEN);
    settings["roots"] = json!([e.src.path().to_string_lossy()]);
    let bundle = e.core.diagnostics_bundle(settings).await.unwrap();
    assert!(bundle.file_name.starts_with("imagepicker-diagnostics-"));
    assert!(bundle.file_name.ends_with(".zip"));
    let entries = read_zip(&bundle.bytes);
    let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        bundle
            .entries
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    );
    for want in [
        "manifest.json",
        "system.json",
        "settings.json",
        "catalog.json",
        "logs/imagepicker-20260101.0.log",
        "crashes/crash-20260101-000000-1-0.txt",
        "crashes/worker-crash-20260101-000000-1-0.txt",
    ] {
        assert!(names.contains(&want), "{want} missing from {names:?}");
    }
    // nothing but these folders: no cache, catalog, edits or backups
    for n in &names {
        assert!(
            !n.contains('/') || n.starts_with("logs/") || n.starts_with("crashes/"),
            "unexpected entry {n}"
        );
        assert!(
            !n.ends_with(".db") && !n.ends_with(".jpg") && !n.ends_with(".png"),
            "{n}"
        );
    }

    // every entry is text: no JPEG / PNG / SQLite bytes
    for (n, data) in &entries {
        assert!(std::str::from_utf8(data).is_ok(), "{n} is not text");
        assert!(!contains(data, b"SQLite format 3"), "{n}");
        assert!(
            !data.starts_with(&[0xFF, 0xD8, 0xFF]) && !data.starts_with(b"\x89PNG"),
            "{n}"
        );
        if n.ends_with(".json") {
            serde_json::from_slice::<Value>(data).unwrap_or_else(|e| panic!("{n}: {e}"));
        }
    }
    // no embeddings, thumbnails, photo paths or secrets in any entry
    for (n, data) in &entries {
        for blob in face_embs.iter().chain(&photo_embs) {
            let head = &blob[..blob.len().min(16)];
            assert!(!contains(data, head), "{n} contains embedding bytes");
        }
        for p in &photo_paths {
            assert!(!contains(data, p.as_bytes()), "{n} contains photo path {p}");
            // JSON strings escape backslashes
            assert!(
                !contains(data, p.replace('\\', "\\\\").as_bytes()),
                "{n} contains {p}"
            );
        }
        for name in &file_names {
            assert!(
                !contains(data, name.as_bytes()),
                "{n} contains the photo name {name}"
            );
        }
        for s in [PASSWORD, TOKEN, "family_0001", "/home/runner"] {
            assert!(!contains(data, s.as_bytes()), "{n} contains {s}");
        }
        let src = e.src.path().to_string_lossy().into_owned();
        assert!(
            !contains(data, src.as_bytes()),
            "{n} contains the root path"
        );
    }

    // what it does contain
    let log =
        String::from_utf8_lossy(find(&entries, "logs/imagepicker-20260101.0.log")).into_owned();
    assert!(
        log.contains("original file is missing") && log.contains("<photo>"),
        "{log}"
    );
    assert!(log.contains("GET /api/thumb/3 200"), "{log}");
    assert!(log.contains("password=[redacted]"), "{log}");
    let crash = String::from_utf8_lossy(find(&entries, "crashes/crash-20260101-000000-1-0.txt"))
        .into_owned();
    assert!(
        crash.contains("kind: rust-panic") && crash.contains("main.rs:1:2"),
        "{crash}"
    );

    let system: Value = serde_json::from_slice(find(&entries, "system.json")).unwrap();
    assert_eq!(system["app"]["name"], "imagePicker");
    assert!(system["app"]["version"].is_string());
    assert!(system["os"]["name"].is_string() && system["os"]["arch"].is_string());
    assert!(system["hardware"]["logical_cpus"].as_u64().unwrap() >= 1);
    assert!(system["worker"]["state"].is_string());

    let settings: Value = serde_json::from_slice(find(&entries, "settings.json")).unwrap();
    assert_eq!(settings["lan"]["password"], "[redacted]");
    assert_eq!(settings["lan"]["port"], 7878);
    assert_eq!(settings["remote_ai"]["token"], "[redacted]");
    assert_eq!(settings["roots"]["count"], 1);
    assert_eq!(settings["language"], e.core.settings().language);

    let cat: Value = serde_json::from_slice(find(&entries, "catalog.json")).unwrap();
    assert_eq!(cat["schema_version"], cat["expected_schema_version"]);
    assert_eq!(cat["integrity_check"]["ok"], true);
    assert_eq!(cat["counts"]["photos"], 3);
    assert_eq!(cat["counts"]["sessions"], 1);
    assert_eq!(cat["counts"]["faces"], 2);
    assert_eq!(cat["counts"]["photos_analyzed"], 3);
    assert_eq!(cat["roots"].as_array().unwrap().len(), 1);
    assert_eq!(cat["roots"][0]["photos"], 3);
    assert_eq!(cat["state"], "ok");
    let _ = sid;

    let manifest: Value = serde_json::from_slice(find(&entries, "manifest.json")).unwrap();
    assert_eq!(manifest["format"], 1);
    assert_eq!(
        manifest["files"].as_array().unwrap().len(),
        entries.len() - 1
    );
    assert!(manifest["excluded"].as_array().unwrap().len() >= 4);
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

#[tokio::test]
async fn an_empty_catalog_gives_a_valid_bundle() {
    let e = env();
    let b = e.core.diagnostics_bundle(json!({})).await.unwrap();
    let entries = read_zip(&b.bytes);
    let cat: Value = serde_json::from_slice(find(&entries, "catalog.json")).unwrap();
    assert_eq!(cat["counts"]["photos"], 0);
    assert_eq!(cat["integrity_check"]["ok"], true);
    assert!(entries
        .iter()
        .all(|(n, _)| !n.starts_with("logs/") || n.ends_with(".log")));
}

#[tokio::test]
async fn logs_are_capped_to_the_newest_part() {
    let e = env();
    // 3 MiB log: only the newest ~1 MiB, from a line boundary
    let line = "2026-01-01T00:00:00Z INFO something happened\n";
    let big = line.repeat(3 * 1024 * 1024 / line.len());
    std::fs::write(e.core.dirs.logs.join("imagepicker-20260101.0.log"), &big).unwrap();
    let b = e.core.diagnostics_bundle(json!({})).await.unwrap();
    let entries = read_zip(&b.bytes);
    let log = find(&entries, "logs/imagepicker-20260101.0.log");
    assert!(log.len() <= 1024 * 1024 + 64, "{}", log.len());
    assert!(log.len() > 512 * 1024);
    assert!(log.starts_with(b"2026-"));
}
