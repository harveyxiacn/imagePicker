//! Card import (PRD F1.8): volume classification, the destination pattern, the copy engine and
//! the whole task against a fake card (a DCIM tree in a temp dir). Generated files only.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use ip_render::Backend;

use super::*;
use crate::fake_worker::FakeWorker;
use crate::testutil::{write_jpeg, FakeImaging, FakeRenderer};
use crate::{CoreConfig, ImportRequest, PhotoQuery};

const WAIT: Duration = Duration::from_secs(60);
/// 2024-03-01 12:00:00 UTC
const DAY1_MS: i64 = 1_709_294_400_000;
const DAY_MS: i64 = 86_400_000;

// ------------------------------------------------------------------ pure logic

fn vol(path: &str, kind: DriveKind, has_dcim: bool) -> Volume {
    Volume {
        path: PathBuf::from(path),
        label: Some(format!("label of {path}")),
        kind,
        has_dcim,
        total_bytes: Some(64),
        free_bytes: Some(32),
    }
}

#[test]
fn only_removable_volumes_with_dcim_are_cards() {
    let list = vec![
        vol("G:\\", DriveKind::Removable, true),
        vol("C:\\", DriveKind::Fixed, true),
        vol("D:\\", DriveKind::Removable, false),
        vol("E:\\", DriveKind::Remote, true),
        vol("F:\\", DriveKind::Optical, true),
        vol("/media/u/CARD", DriveKind::Unknown, true),
        vol("/media/u/usb", DriveKind::Unknown, false),
        vol("G:\\", DriveKind::Removable, true),
        vol("A:\\", DriveKind::Removable, true),
    ];
    let out = classify_volumes(&list);
    let paths: Vec<String> = out
        .iter()
        .map(|v| v.path.to_string_lossy().into_owned())
        .collect();
    let mut expected = ["/media/u/CARD", "A:\\", "G:\\"];
    expected.sort_by_key(|p| PathBuf::from(p));
    assert_eq!(paths, expected, "sorted, deduplicated");
    assert!(classify_volumes(&[]).is_empty());
}

#[test]
fn sources_carry_label_and_photo_estimate() {
    let d = tempfile::tempdir().unwrap();
    let dcim = d.path().join("dcim/100CANON");
    write_jpeg(&dcim.join("IMG_1.JPG"), 8, 8, 1);
    std::fs::write(dcim.join("IMG_1.CR2"), b"raw").unwrap();
    write_jpeg(&dcim.join("IMG_2.JPG"), 8, 8, 2);
    write_jpeg(&dcim.join(".hidden.jpg"), 8, 8, 3);
    std::fs::write(dcim.join("notes.txt"), b"x").unwrap();
    // `dcim` is lower case on purpose: the folder is found whatever its letter case
    let mut v = vol(&d.path().to_string_lossy(), DriveKind::Removable, true);
    v.label = None;
    let list = sources_from(&[v]);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].photo_estimate, 2, "a RAW+JPEG pair is one photo");
    assert!(!list[0].estimate_truncated);
    assert_eq!(list[0].label, d.path().to_string_lossy());
    assert_eq!(list[0].free_bytes, Some(32));
    assert!(dcim_dir(d.path()).is_some());
    assert!(dcim_dir(&dcim).is_none());
}

#[test]
fn patterns_are_validated_and_expanded() {
    for ok in [
        DEFAULT_PATTERN,
        "{yyyy-mm-dd}",
        "{yyyy}/{mm}",
        "Trip {yyyymmdd}/day",
        "",
        "  ",
    ] {
        assert!(validate_pattern(ok).is_ok(), "{ok:?}");
    }
    for bad in [
        "..",
        "a/../b",
        "{year}",
        "{yyyy",
        "a\\b",
        "a//b",
        "/abs",
        "C:/x",
        "con",
        "x/{dd}/*",
        "dot.",
        "a/ lead",
        "trail /x",
        "a/b/c/d/e/f/g",
        "tab\t{yyyy}",
    ] {
        assert!(validate_pattern(bad).is_err(), "{bad:?}");
    }
    assert_eq!(
        expand_pattern(DEFAULT_PATTERN, 2024, 3, 9),
        ["2024", "2024-03-09"]
    );
    assert_eq!(
        expand_pattern("{yyyymmdd} x/{mm}{dd}", 2024, 3, 9),
        ["20240309 x", "0309"]
    );
    assert!(expand_pattern("", 2024, 3, 9).is_empty());
}

#[test]
fn collision_suffix_goes_before_the_extension() {
    assert_eq!(suffixed("IMG_1.JPG", 0), "IMG_1.JPG");
    assert_eq!(suffixed("IMG_1.JPG", 2), "IMG_1_2.JPG");
    assert_eq!(suffixed("noext", 1), "noext_1");
    assert_eq!(suffixed(".hidden", 1), ".hidden_1");
}

#[test]
fn common_dir_is_the_deepest_shared_folder() {
    let p = |s: &str| PathBuf::from(s);
    let root = p("/d");
    assert_eq!(
        common_dir(&[p("/d/2024/a/x.jpg"), p("/d/2024/b/y.jpg")], &root),
        p("/d/2024")
    );
    assert_eq!(common_dir(&[p("/d/2024/a/x.jpg")], &root), p("/d/2024/a"));
    assert_eq!(
        common_dir(&[p("/d/2024/a/x.jpg"), p("/d/2025/a/y.jpg")], &root),
        p("/d")
    );
    assert_eq!(common_dir(&[], &root), root);
}

#[test]
fn outcome_states() {
    let r = |f: &dyn Fn(&mut CardReport)| {
        let mut r = CardReport::default();
        f(&mut r);
        card_outcome(&r)
    };
    assert_eq!(r(&|_| {}), (DONE, None));
    assert_eq!(r(&|r| r.cancelled = true), (CANCELLED, None));
    let failure = CardFailure {
        path: "a".into(),
        error: "boom".into(),
    };
    let (s, e) = r(&|r| {
        r.failed_count = 1;
        r.failed = vec![failure.clone()];
    });
    assert_eq!((s, e.as_deref()), (FAILED, Some("boom")));
    let (s, e) = r(&|r| {
        r.copied = 3;
        r.failed_count = 1;
        r.failed = vec![failure.clone()];
    });
    assert_eq!(s, DONE);
    assert!(e.unwrap().contains("1 of 4 failed"));
    let (s, _) = r(&|r| r.aborted = Some("disk full".into()));
    assert_eq!(s, FAILED);
    let (s, e) = r(&|r| {
        r.copied = 2;
        r.not_attempted = 5;
        r.aborted = Some("disk full".into());
    });
    assert_eq!(s, DONE);
    assert!(e.unwrap().contains("disk full"));
}

// ------------------------------------------------------------------ copy engine

/// Reads like the filesystem, except for the files it is told to break.
#[derive(Default)]
struct TestIo {
    /// `open` fails for paths containing one of these.
    unreadable: Vec<&'static str>,
    /// `open` sleeps this long for paths containing the text.
    slow: Option<(&'static str, u64)>,
    /// The first read of a path containing the text requests this cancellation.
    cancel_on_read: Option<(&'static str, CancelToken)>,
}

struct CancelingReader {
    inner: std::fs::File,
    cancel: CancelToken,
    done: AtomicBool,
}

impl Read for CancelingReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if !self.done.swap(true, Ordering::SeqCst) {
            self.cancel.cancel();
        }
        Ok(n)
    }
}

impl CardIo for TestIo {
    fn open(&self, path: &Path) -> io::Result<Box<dyn Read + Send>> {
        let s = path.to_string_lossy();
        if self.unreadable.iter().any(|u| s.contains(u)) {
            return Err(io::Error::other("read error (bad sector)"));
        }
        if let Some((text, ms)) = self.slow {
            if s.contains(text) {
                std::thread::sleep(Duration::from_millis(ms));
            }
        }
        let f = std::fs::File::open(path)?;
        if let Some((text, cancel)) = &self.cancel_on_read {
            if s.contains(text) {
                return Ok(Box::new(CancelingReader {
                    inner: f,
                    cancel: cancel.clone(),
                    done: AtomicBool::new(false),
                }));
            }
        }
        Ok(Box::new(f))
    }
}

fn set_mtime(path: &Path, ms: i64) {
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(UNIX_EPOCH + Duration::from_millis(ms as u64))
        .unwrap();
}

fn item(path: &Path) -> CopyItem {
    let md = std::fs::metadata(path).unwrap();
    CopyItem {
        src: path.to_path_buf(),
        size: md.len(),
        mtime_ms: md
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64,
    }
}

fn set_of(dir: &Path, files: &[&Path]) -> CopySet {
    CopySet {
        dest_dir: dir.to_path_buf(),
        items: files.iter().map(|p| item(p)).collect(),
    }
}

fn all_files(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                todo.push(p);
            } else {
                out.push(
                    p.strip_prefix(dir)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    out.sort();
    out
}

fn no_progress(_: usize) {}

fn engine_fixture() -> (tempfile::TempDir, tempfile::TempDir, [PathBuf; 3]) {
    let (card, dest) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let files = [
        card.path().join("a.jpg"),
        card.path().join("a.cr2"),
        card.path().join("b.jpg"),
    ];
    write_jpeg(&files[0], 32, 24, 10);
    std::fs::write(&files[1], vec![7u8; 5000]).unwrap();
    write_jpeg(&files[2], 24, 32, 90);
    set_mtime(&files[0], DAY1_MS);
    (card, dest, files)
}

#[test]
fn copies_are_verified_and_keep_the_modification_time() {
    let (card, dest, files) = engine_fixture();
    let sets = vec![
        set_of(&dest.path().join("x/y"), &[&files[0], &files[1]]),
        set_of(&dest.path().join("x/z"), &[&files[2]]),
    ];
    let seen = std::sync::Mutex::new(Vec::new());
    let out = copy_sets(&FsCardIo, dest.path(), &sets, &CancelToken::new(), &|n| {
        seen.lock().unwrap().push(n)
    });
    assert!(out.failed.is_empty() && out.aborted.is_none() && !out.cancelled);
    assert_eq!(out.copied.len(), 3);
    assert_eq!(*seen.lock().unwrap(), [2, 3]);
    assert_eq!(
        all_files(dest.path()),
        ["x/y/a.cr2", "x/y/a.jpg", "x/z/b.jpg"],
        "no temporary files are left"
    );
    for c in &out.copied {
        assert_eq!(
            std::fs::read(&c.src).unwrap(),
            std::fs::read(&c.dst).unwrap()
        );
        assert_eq!(c.bytes, std::fs::metadata(&c.src).unwrap().len());
    }
    let copy_mtime = item(&dest.path().join("x/y/a.jpg")).mtime_ms;
    assert_eq!(copy_mtime, DAY1_MS);
    // the card is untouched
    assert_eq!(all_files(card.path()).len(), 3);
}

#[test]
fn a_different_file_with_the_same_name_is_never_overwritten() {
    let (_card, dest, files) = engine_fixture();
    let dir = dest.path().join("d");
    std::fs::create_dir_all(&dir).unwrap();
    // another card's IMG: same name as the JPEG of the pair, different content
    std::fs::write(dir.join("a.jpg"), b"somebody else's photo").unwrap();
    std::fs::write(dir.join("a_1.cr2"), b"and a stray raw").unwrap();
    let out = copy_sets(
        &FsCardIo,
        dest.path(),
        &[set_of(&dir, &[&files[0], &files[1]])],
        &CancelToken::new(),
        &no_progress,
    );
    assert_eq!(out.copied.len(), 2, "{:?}", out.failed);
    assert_eq!(
        std::fs::read(dir.join("a.jpg")).unwrap(),
        b"somebody else's photo"
    );
    assert_eq!(
        std::fs::read(dir.join("a_1.cr2")).unwrap(),
        b"and a stray raw"
    );
    // both members of the pair got suffix 2 (suffix 1 collides with the stray RAW)
    assert_eq!(
        std::fs::read(dir.join("a_2.jpg")).unwrap(),
        std::fs::read(&files[0]).unwrap()
    );
    assert_eq!(
        std::fs::read(dir.join("a_2.cr2")).unwrap(),
        std::fs::read(&files[1]).unwrap()
    );
}

#[test]
fn a_file_the_destination_already_has_is_not_copied_again() {
    let (_card, dest, files) = engine_fixture();
    let dir = dest.path().join("d");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(&files[2], dir.join("b.jpg")).unwrap();
    let out = copy_sets(
        &FsCardIo,
        dest.path(),
        &[set_of(&dir, &[&files[2]])],
        &CancelToken::new(),
        &no_progress,
    );
    assert!(out.copied.is_empty() && out.failed.is_empty());
    assert_eq!(out.present, [dir.join("b.jpg")]);
    assert_eq!(all_files(&dir), ["b.jpg"]);
}

#[test]
fn an_unreadable_file_fails_alone_and_leaves_nothing_behind() {
    let (_card, dest, files) = engine_fixture();
    let io = TestIo {
        unreadable: vec!["a.jpg"],
        ..Default::default()
    };
    let dir = dest.path().join("d");
    let out = copy_sets(
        &io,
        dest.path(),
        &[
            set_of(&dir, &[&files[0], &files[1]]),
            set_of(&dir, &[&files[2]]),
        ],
        &CancelToken::new(),
        &no_progress,
    );
    assert_eq!(out.failed.len(), 1);
    assert!(out.failed[0].0.ends_with("a.jpg"));
    assert!(out.failed[0].1.contains("bad sector"), "{:?}", out.failed);
    assert_eq!(
        out.copied.len(),
        2,
        "the RAW and the other photo are copied"
    );
    assert!(out.aborted.is_none() && !out.cancelled);
    assert_eq!(all_files(&dir), ["a.cr2", "b.jpg"]);
}

#[test]
fn a_file_that_changed_size_is_not_catalogued() {
    let (_card, dest, files) = engine_fixture();
    let mut it = item(&files[2]);
    it.size += 10; // the card shrank the file after the scan
    let out = copy_sets(
        &FsCardIo,
        dest.path(),
        &[CopySet {
            dest_dir: dest.path().to_path_buf(),
            items: vec![it],
        }],
        &CancelToken::new(),
        &no_progress,
    );
    assert_eq!(out.failed.len(), 1);
    assert!(
        out.failed[0].1.contains("changed while"),
        "{:?}",
        out.failed
    );
    assert!(all_files(dest.path()).is_empty());
}

#[test]
fn cancelling_mid_copy_removes_the_partial_file_and_keeps_the_rest() {
    let (_card, dest, files) = engine_fixture();
    let cancel = CancelToken::new();
    let io = TestIo {
        cancel_on_read: Some(("b.jpg", cancel.clone())),
        ..Default::default()
    };
    let dir = dest.path().join("d");
    let out = copy_sets(
        &io,
        dest.path(),
        &[
            set_of(&dir, &[&files[0], &files[1]]),
            set_of(&dir, &[&files[2]]),
            set_of(&dir, &[&files[0]]),
        ],
        &cancel,
        &no_progress,
    );
    assert!(out.cancelled);
    assert_eq!(out.copied.len(), 2, "the first set was complete");
    assert_eq!(out.not_attempted, 2);
    assert_eq!(
        all_files(&dir),
        ["a.cr2", "a.jpg"],
        "no partial b.jpg, no temp file"
    );
}

#[test]
fn a_destination_error_stops_the_run_but_keeps_the_verified_copies() {
    let (_card, dest, files) = engine_fixture();
    let blocked = dest.path().join("blocked");
    std::fs::write(&blocked, b"i am a file, not a folder").unwrap();
    let out = copy_sets(
        &FsCardIo,
        dest.path(),
        &[
            set_of(&dest.path().join("ok"), &[&files[0]]),
            set_of(&blocked.join("sub"), &[&files[1]]),
            set_of(&dest.path().join("never"), &[&files[2]]),
        ],
        &CancelToken::new(),
        &no_progress,
    );
    assert_eq!(out.copied.len(), 1);
    assert!(out.aborted.is_some());
    assert_eq!(out.not_attempted, 2);
    assert_eq!(all_files(dest.path()), ["blocked", "ok/a.jpg"]);
}

#[cfg(unix)]
#[test]
fn a_destination_folder_cannot_lead_outside_through_a_symlink() {
    let (_card, dest, files) = engine_fixture();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), dest.path().join("link")).unwrap();
    let out = copy_sets(
        &FsCardIo,
        dest.path(),
        &[set_of(&dest.path().join("link/2024"), &[&files[0]])],
        &CancelToken::new(),
        &no_progress,
    );
    assert!(out.copied.is_empty());
    assert!(out.aborted.as_deref().unwrap().contains("outside"));
    assert!(
        all_files(outside.path()).is_empty(),
        "nothing was written outside"
    );
}

// ------------------------------------------------------------------ the task

fn open_core(data: &Path) -> Arc<Core> {
    Core::open(CoreConfig {
        data_dir: Some(data.to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(2),
        worker: Some(Arc::new(FakeWorker::new())),
        renderer: Some(Arc::new(FakeRenderer::new(Backend::Cpu))),
        force_cpu: true,
    })
    .unwrap()
}

fn put_jpeg(path: &Path, shade: u8, day: i64) {
    write_jpeg(path, 64, 48, shade);
    set_mtime(path, DAY1_MS + (day - 1) * DAY_MS);
}

fn put_raw(path: &Path, seed: u8, day: i64) {
    std::fs::write(path, vec![seed; 3000]).unwrap();
    set_mtime(path, DAY1_MS + (day - 1) * DAY_MS);
}

/// 100CANON: IMG_0001 (JPEG+CR2, day 1), IMG_0002 (day 1), IMG_0003 (day 2);
/// 101CANON: IMG_0001 (another JPEG with the same name, day 1).
fn make_card(root: &Path) {
    let a = root.join("DCIM/100CANON");
    let b = root.join("DCIM/101CANON");
    put_jpeg(&a.join("IMG_0001.JPG"), 10, 1);
    put_raw(&a.join("IMG_0001.CR2"), 1, 1);
    put_jpeg(&a.join("IMG_0002.JPG"), 60, 1);
    put_jpeg(&a.join("IMG_0003.JPG"), 110, 2);
    put_jpeg(&b.join("IMG_0001.JPG"), 160, 1);
}

struct Env {
    core: Arc<Core>,
    _data: tempfile::TempDir,
    card: tempfile::TempDir,
    dest: tempfile::TempDir,
}

fn env() -> Env {
    let (data, card, dest) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    make_card(card.path());
    Env {
        core: open_core(data.path()),
        _data: data,
        card,
        dest,
    }
}

impl Env {
    fn request(&self) -> CardImportRequest {
        CardImportRequest {
            source: self.card.path().to_string_lossy().into_owned(),
            dest: self.dest.path().to_string_lossy().into_owned(),
            pattern: None,
            title: None,
            exclude: Vec::new(),
        }
    }
}

/// Waits for the task to end and returns its record and parsed report.
async fn finished(core: &Arc<Core>, id: &str) -> (crate::tasks::TaskRecord, CardReport) {
    let t = std::time::Instant::now();
    loop {
        let rec = core
            .tasks_list(None)
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.id == id)
            .unwrap();
        if rec.status != crate::tasks::RUNNING && rec.status != crate::tasks::CANCELLING {
            let report = serde_json::from_value(rec.params["report"].clone()).unwrap_or_default();
            return (rec, report);
        }
        assert!(t.elapsed() < WAIT, "task {id} never ended");
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

async fn session_files(core: &Arc<Core>, sid: i64) -> Vec<crate::Photo> {
    core.wait_session_ready(sid, WAIT).await.unwrap();
    let mut v = core
        .photos(PhotoQuery {
            session_id: sid,
            limit: Some(1000),
            ..Default::default()
        })
        .await
        .unwrap()
        .photos;
    v.sort_by(|a, b| a.path.cmp(&b.path));
    v
}

#[tokio::test(flavor = "multi_thread")]
async fn imports_a_card_into_dated_folders_and_pairs_raw_and_jpeg() {
    let e = env();
    let before = all_files(e.card.path());
    let id = e.core.card_import(e.request()).await.unwrap();
    assert!(id.starts_with("card_import-"));
    let (rec, rep) = finished(&e.core, &id).await;
    assert_eq!(rec.kind, TASK_KIND);
    assert_eq!((rec.status.as_str(), rec.error.as_deref()), ("done", None));
    assert_eq!((rep.found, rep.copied, rep.skipped_existing), (5, 5, 0));
    assert_eq!(rep.failed_count, 0);
    assert_eq!(rec.done, 5);
    assert_eq!(
        all_files(e.dest.path()),
        [
            "2024/2024-03-01/IMG_0001.CR2",
            "2024/2024-03-01/IMG_0001.JPG",
            "2024/2024-03-01/IMG_0001_1.JPG",
            "2024/2024-03-01/IMG_0002.JPG",
            "2024/2024-03-02/IMG_0003.JPG",
        ],
        "same-name file from the second folder gets a suffix"
    );
    assert_eq!(all_files(e.card.path()), before, "the card is only read");
    // every copy equals its original
    let copy = e.dest.path().join("2024/2024-03-01/IMG_0001_1.JPG");
    let orig = e.card.path().join("DCIM/101CANON/IMG_0001.JPG");
    assert_eq!(std::fs::read(copy).unwrap(), std::fs::read(orig).unwrap());

    // imported as one session; the RAW is the JPEG's companion
    let sid = rep.session_id.expect("a session was created");
    e.core.wait_session_ready(sid, WAIT).await.unwrap();
    let s = e.core.session(sid).await.unwrap();
    assert!(!s.title.is_empty());
    let photos = session_files(&e.core, sid).await;
    let names: Vec<&str> = photos.iter().map(|p| p.file_name.as_str()).collect();
    assert_eq!(
        names,
        [
            "IMG_0001.JPG",
            "IMG_0001_1.JPG",
            "IMG_0002.JPG",
            "IMG_0003.JPG"
        ],
        "the RAW is not a grid item of its own"
    );
    let pair = photos
        .iter()
        .find(|p| p.file_name == "IMG_0001.JPG")
        .unwrap();
    assert_eq!(pair.companions.len(), 1);
    assert_eq!(pair.companions[0].file_name, "IMG_0001.CR2");
    assert_eq!(s.photo_count, 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_import_of_the_same_card_copies_nothing() {
    let e = env();
    let id = e.core.card_import(e.request()).await.unwrap();
    let (_, first) = finished(&e.core, &id).await;
    assert_eq!(first.copied, 5);
    e.core
        .wait_session_ready(first.session_id.unwrap(), WAIT)
        .await
        .unwrap();
    let files = all_files(e.dest.path());
    let id = e.core.card_import(e.request()).await.unwrap();
    let (rec, rep) = finished(&e.core, &id).await;
    assert_eq!(rec.status, "done");
    assert_eq!((rep.copied, rep.skipped_existing, rep.found), (0, 5, 5));
    assert!(rep.session_id.is_none(), "nothing new, no session");
    assert_eq!(all_files(e.dest.path()), files);
}

#[tokio::test(flavor = "multi_thread")]
async fn files_the_catalog_already_has_are_skipped() {
    let e = env();
    // imported earlier from somewhere else: IMG_0002 with the same name, size and mtime ...
    let old = tempfile::tempdir().unwrap();
    let src = e.card.path().join("DCIM/100CANON/IMG_0002.JPG");
    std::fs::copy(&src, old.path().join("IMG_0002.JPG")).unwrap();
    set_mtime(&old.path().join("IMG_0002.JPG"), DAY1_MS);
    // ... and IMG_0003 under another name and time (same bytes)
    std::fs::copy(
        e.card.path().join("DCIM/100CANON/IMG_0003.JPG"),
        old.path().join("renamed.jpg"),
    )
    .unwrap();
    set_mtime(&old.path().join("renamed.jpg"), DAY1_MS + 5 * DAY_MS);
    let s = e
        .core
        .import(ImportRequest {
            path: old.path().to_string_lossy().into_owned(),
            recursive: true,
            title: None,
        })
        .await
        .unwrap();
    e.core.wait_session_ready(s.id, WAIT).await.unwrap();

    let id = e.core.card_import(e.request()).await.unwrap();
    let (_, rep) = finished(&e.core, &id).await;
    assert_eq!((rep.found, rep.skipped_existing, rep.copied), (5, 2, 3));
    assert_eq!(
        all_files(e.dest.path()),
        [
            "2024/2024-03-01/IMG_0001.CR2",
            "2024/2024-03-01/IMG_0001.JPG",
            "2024/2024-03-01/IMG_0001_1.JPG",
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn exclusions_apply_to_the_card() {
    let e = env();
    let mut req = e.request();
    req.exclude = vec!["*.CR2".into(), "101CANON".into()];
    let id = e.core.card_import(req).await.unwrap();
    let (_, rep) = finished(&e.core, &id).await;
    assert_eq!((rep.found, rep.copied), (3, 3));
    assert_eq!(
        all_files(e.dest.path()),
        [
            "2024/2024-03-01/IMG_0001.JPG",
            "2024/2024-03-01/IMG_0002.JPG",
            "2024/2024-03-02/IMG_0003.JPG",
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_pattern_chooses_the_folders() {
    let e = env();
    let mut req = e.request();
    req.pattern = Some("{yyyymmdd}".into());
    req.title = Some("Trip".into());
    let id = e.core.card_import(req).await.unwrap();
    let (_, rep) = finished(&e.core, &id).await;
    assert_eq!(rep.copied, 5);
    let names = all_files(e.dest.path());
    assert!(names
        .iter()
        .all(|n| n.starts_with("20240301/") || n.starts_with("20240302/")));
    let s = e.core.session(rep.session_id.unwrap()).await.unwrap();
    assert_eq!(s.title, "Trip");

    let e2 = env();
    let mut req = e2.request();
    req.pattern = Some(String::new());
    let id = e2.core.card_import(req).await.unwrap();
    let (_, rep) = finished(&e2.core, &id).await;
    assert_eq!(rep.copied, 5);
    assert!(all_files(e2.dest.path()).iter().all(|n| !n.contains('/')));
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_requests_are_refused_before_anything_is_written() {
    let e = env();
    let bad = |f: &dyn Fn(&mut CardImportRequest)| {
        let mut r = e.request();
        f(&mut r);
        r
    };
    let none = tempfile::tempdir().unwrap();
    for (what, req) in [
        ("empty source", bad(&|r| r.source = " ".into())),
        ("empty dest", bad(&|r| r.dest = String::new())),
        (
            "no DCIM",
            bad(&|r| r.source = none.path().to_string_lossy().into_owned()),
        ),
        ("bad pattern", bad(&|r| r.pattern = Some("../x".into()))),
        (
            "destination on the card",
            bad(&|r| r.dest = e.card.path().join("DCIM").to_string_lossy().into_owned()),
        ),
    ] {
        let err = e.core.card_import(req).await.unwrap_err();
        assert!(matches!(err, CoreError::BadRequest(_)), "{what}: {err}");
    }
    assert!(all_files(e.dest.path()).is_empty());
    assert!(e.core.tasks_list(None).await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unreadable_file_is_reported_and_the_rest_is_imported() {
    let e = env();
    put_jpeg(&e.card.path().join("DCIM/100CANON/IMG_0004.JPG"), 200, 2);
    let io = TestIo {
        unreadable: vec!["IMG_0004"],
        ..Default::default()
    };
    let id = e
        .core
        .card_import_with(e.request(), Arc::new(io))
        .await
        .unwrap();
    let (rec, rep) = finished(&e.core, &id).await;
    assert_eq!(rec.status, "done");
    assert!(rec.error.unwrap().contains("1 of 6 failed"));
    assert_eq!((rep.copied, rep.failed_count), (5, 1));
    assert!(rep.failed[0].path.ends_with("IMG_0004.JPG"));
    assert!(rep.failed[0].error.contains("bad sector"));
    assert!(!all_files(e.dest.path())
        .iter()
        .any(|n| n.contains("IMG_0004")));
    let photos = session_files(&e.core, rep.session_id.unwrap()).await;
    assert_eq!(photos.len(), 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failure_midway_keeps_and_imports_what_was_copied() {
    let e = env();
    // day 2 cannot be created: a file is in the way
    std::fs::write(e.dest.path().join("2024-03-02"), b"in the way").unwrap();
    let mut req = e.request();
    req.pattern = Some("{yyyy-mm-dd}".into());
    let id = e.core.card_import(req).await.unwrap();
    let (rec, rep) = finished(&e.core, &id).await;
    assert_eq!(rec.status, "done");
    assert!(rec.error.is_some());
    assert!(rep.aborted.is_some());
    assert_eq!((rep.copied, rep.failed_count, rep.not_attempted), (3, 0, 2));
    let photos = session_files(&e.core, rep.session_id.unwrap()).await;
    let names: Vec<&str> = photos.iter().map(|p| p.file_name.as_str()).collect();
    assert_eq!(names, ["IMG_0001.JPG", "IMG_0002.JPG"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_stops_the_copy_and_imports_the_finished_files() {
    let e = env();
    // IMG_0003 (the third set) is slow to open, which leaves time to cancel
    let io = TestIo {
        slow: Some(("IMG_0003", 1500)),
        ..Default::default()
    };
    let id = e
        .core
        .card_import_with(e.request(), Arc::new(io))
        .await
        .unwrap();
    let t = std::time::Instant::now();
    loop {
        let rec = e.core.tasks_list(None).await.unwrap().remove(0);
        if rec.done >= 2 && rec.cancellable {
            break;
        }
        assert!(t.elapsed() < WAIT, "no progress");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let rec = e.core.task_cancel(&id).await.unwrap();
    assert_eq!(rec.status, "cancelling");
    let (rec, rep) = finished(&e.core, &id).await;
    assert_eq!(rec.status, "cancelled");
    assert!(rep.cancelled);
    assert!(rep.copied >= 2 && rep.copied < 5, "{rep:?}");
    let names = all_files(e.dest.path());
    assert!(!names.iter().any(|n| n.contains("IMG_0003")), "{names:?}");
    assert!(!names.iter().any(|n| n.ends_with(".ipcopy")), "{names:?}");
    let photos = session_files(&e.core, rep.session_id.unwrap()).await;
    assert_eq!(photos.len() + 1, rep.copied, "the pair is one item");
}
