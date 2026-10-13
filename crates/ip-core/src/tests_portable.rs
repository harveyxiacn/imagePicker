//! Portable mode (docs/09 §5.3 P2): with `imagepicker.portable` next to the executable every
//! directory of a Core lands next to it. The executable folder and the environment are injected.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ip_render::Backend;

use crate::fake_worker::FakeWorker;
use crate::paths::{DataDirSource, PORTABLE_MARKER};
use crate::testutil::{FakeImaging, FakeRenderer};
use crate::*;

fn cfg(data_dir: Option<PathBuf>) -> CoreConfig {
    CoreConfig {
        data_dir,
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(1),
        worker: Some(Arc::new(FakeWorker::new())),
        renderer: Some(Arc::new(FakeRenderer::new(Backend::Cpu))),
        force_cpu: true,
    }
}

fn inside(p: &Path, root: &Path) -> bool {
    p.starts_with(root)
}

#[tokio::test]
async fn marker_next_to_the_exe_keeps_every_dir_inside_it() {
    let exe = tempfile::tempdir().unwrap();
    std::fs::write(exe.path().join(PORTABLE_MARKER), b"").unwrap();
    let core = Core::open_with(cfg(None), None, Some(exe.path().to_path_buf())).unwrap();
    assert_eq!(core.data_dir_source, DataDirSource::Portable);
    let root = exe.path().join("data");
    assert_eq!(core.data_dir(), root);
    let d = &core.dirs;
    for p in [
        &d.root,
        &d.catalog,
        &d.thumbs,
        &d.previews,
        &d.full,
        &d.logs,
        &d.luts,
        &d.masks,
        &d.beauty,
        &d.edited_thumbs,
        &d.edited_previews,
        &d.edits,
        &d.gen,
        &d.backups,
    ] {
        assert!(inside(p, &root), "{} escapes {}", p.display(), root.display());
    }
    assert!(d.catalog.is_file(), "catalog must be created in the portable folder");
    assert!(inside(core.runtime.root(), &root), "AI runtime location");
    let s = core.settings();
    assert!(inside(Path::new(&s.models.dir), &root), "default models dir {}", s.models.dir);
    // nothing but the data folder and the marker was created next to the exe
    let mut names: Vec<_> = std::fs::read_dir(exe.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, vec!["data".to_string(), PORTABLE_MARKER.to_string()]);
    let info = core.cache_info().await.unwrap();
    assert!(info.portable);
    assert_eq!(info.data_dir_source, "portable");
    assert_eq!(Path::new(&info.data_dir), root);
}

#[tokio::test]
async fn explicit_dir_and_env_beat_the_marker() {
    let exe = tempfile::tempdir().unwrap();
    std::fs::write(exe.path().join(PORTABLE_MARKER), b"").unwrap();
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let c = Core::open_with(
        cfg(Some(a.path().to_path_buf())),
        Some(b.path().into()),
        Some(exe.path().to_path_buf()),
    )
    .unwrap();
    assert_eq!(c.data_dir_source, DataDirSource::Explicit);
    assert_eq!(c.data_dir(), a.path());
    drop(c);
    let c = Core::open_with(cfg(None), Some(b.path().into()), Some(exe.path().to_path_buf()))
        .unwrap();
    assert_eq!(c.data_dir_source, DataDirSource::Env);
    assert_eq!(c.data_dir(), b.path());
    assert!(!c.cache_info().await.unwrap().portable);
    assert!(!exe.path().join("data").exists());
}

#[tokio::test]
async fn default_models_dir_is_not_persisted_as_an_absolute_path() {
    let exe = tempfile::tempdir().unwrap();
    std::fs::write(exe.path().join(PORTABLE_MARKER), b"").unwrap();
    let core = Core::open_with(cfg(None), None, Some(exe.path().to_path_buf())).unwrap();
    core.settings
        .patch(&serde_json::json!({"cache": {"max_gb": 3}}))
        .unwrap();
    let text = std::fs::read_to_string(exe.path().join("data").join("settings.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["models"]["dir"], "");
    assert_eq!(v["cache"]["max_gb"].as_f64(), Some(3.0));
    // a copy of the settings file in another folder (the disk got another letter) follows it
    let moved = tempfile::tempdir().unwrap();
    std::fs::write(moved.path().join("settings.json"), text).unwrap();
    let again = crate::settings::SettingsStore::load(moved.path());
    assert!(inside(Path::new(&again.get().models.dir), moved.path()));
}

#[tokio::test]
async fn open_follows_the_app_drive_for_portable_roots() {
    // fake drive mapping at Core level: a catalog written while the disk was `E:`, the exe now
    // reports a `G:` folder. (The folder does not exist; only the string drive logic is used.)
    let exe = tempfile::tempdir().unwrap();
    std::fs::write(exe.path().join(PORTABLE_MARKER), b"").unwrap();
    let data = exe.path().join("data");
    {
        let core = Core::open_with(cfg(None), None, Some(exe.path().to_path_buf())).unwrap();
        core.db
            .with(|c| {
                c.execute(
                    "INSERT INTO root_folder(path, portable, added_at) VALUES('E:\\Photos', 1, 0)",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
    }
    assert!(data.join("catalog.db").is_file());
    // the real drive of a tempdir is not a letter on Linux, so exercise the same step directly
    let core = Core::open_with(cfg(None), None, Some(exe.path().to_path_buf())).unwrap();
    let rep = core
        .db
        .with(|c| portable::rebase_roots(c, "G:"))
        .unwrap();
    assert_eq!(rep.moved, 1);
    let p: String = core
        .db
        .with(|c| Ok(c.query_row("SELECT path FROM root_folder", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(p, "G:\\Photos");
}
