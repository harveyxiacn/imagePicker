//! M3 editing tests (FakeImaging + FakeWorker + FakeRenderer). Tests that need the real
//! `ip-render` implementation are `#[ignore]`d until it is merged.

use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use ip_render::Backend;
use serde_json::{json, Value};
use tokio::sync::broadcast::Receiver;

use crate::edit::{
    stack_hash, validate_stack, AutoRequest, PresetCreate, PreviewRequest, SyncRequest,
};
use crate::testutil::{FakeImaging, FakeRenderer, FakeWorker};
use crate::*;

const WAIT: Duration = Duration::from_secs(30);

struct Env {
    core: Arc<Core>,
    worker: Arc<FakeWorker>,
    renderer: Arc<FakeRenderer>,
    _data: tempfile::TempDir,
    src: tempfile::TempDir,
}

fn env_with(renderer: Option<Arc<dyn ip_render::Renderer>>, fake: Arc<FakeRenderer>) -> Env {
    let data = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let worker = Arc::new(FakeWorker::new());
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(2),
        worker: Some(worker.clone()),
        renderer,
        force_cpu: true,
    })
    .unwrap();
    Env {
        core,
        worker,
        renderer: fake,
        _data: data,
        src,
    }
}

fn env() -> Env {
    let fake = Arc::new(FakeRenderer::new(Backend::Gpu));
    env_with(Some(fake.clone()), fake)
}

fn solid(dir: &Path, name: &str, rgb: [u8; 3]) {
    let img = image::RgbImage::from_pixel(160, 120, image::Rgb(rgb));
    img.save_with_format(dir.join(name), image::ImageFormat::Jpeg)
        .unwrap();
}

/// Imports `files` (name, colour) and returns the photo ids in the order given.
async fn setup(e: &Env, files: &[(&str, [u8; 3])]) -> Vec<i64> {
    for (n, c) in files {
        solid(e.src.path(), n, *c);
    }
    let s = e
        .core
        .import(ImportRequest {
            path: e.src.path().to_string_lossy().into_owned(),
            recursive: true,
            title: None,
        })
        .await
        .unwrap();
    let s = e.core.wait_session_ready(s.id, WAIT).await.unwrap();
    let page = e
        .core
        .photos(PhotoQuery {
            session_id: s.id,
            ..Default::default()
        })
        .await
        .unwrap();
    files
        .iter()
        .map(|(n, _)| page.photos.iter().find(|p| p.file_name == *n).unwrap().id)
        .collect()
}

fn exposure_stack(ev: f64) -> Value {
    json!({"version":1,"ops":[{"type":"global","exposure":ev}]})
}

async fn next_edit_event(rx: &mut Receiver<Event>, id: i64) -> EditUpdate {
    tokio::time::timeout(WAIT, async {
        loop {
            match rx.recv().await {
                Ok(Event::EditsUpdated { items }) => {
                    if let Some(u) = items.into_iter().find(|u| u.id == id) {
                        return u;
                    }
                }
                Ok(_) => {}
                Err(e) => panic!("event bus: {e}"),
            }
        }
    })
    .await
    .expect("edits.updated")
}

fn mean(img: &image::RgbImage) -> f64 {
    let s: u64 = img.as_raw().iter().map(|v| *v as u64).sum();
    s as f64 / img.as_raw().len() as f64
}

fn mean_of_file(p: &Path) -> f64 {
    mean(&image::open(p).unwrap().to_rgb8())
}

fn mean_of_jpeg(bytes: &[u8]) -> f64 {
    mean(&image::load_from_memory(bytes).unwrap().to_rgb8())
}

async fn preview(e: &Env, id: i64, stack: Option<Value>, long_edge: u32) -> Preview {
    e.core
        .render_preview(PreviewRequest {
            photo_id: id,
            stack,
            long_edge: Some(long_edge),
            original: None,
        })
        .await
        .unwrap()
}

// ------------------------------------------------------------------ storage

#[test]
fn migration_v5_creates_edit_tables() {
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    assert_eq!(crate::db::migrate_to(&mut conn, 4).unwrap(), 4);
    assert_eq!(
        crate::db::migrate(&mut conn).unwrap(),
        crate::db::MIGRATIONS.len()
    );
    for t in ["edit_version", "preset"] {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [t],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "{t}");
    }
    conn.prepare("SELECT has_edits, edit_hash FROM photo")
        .unwrap();
}

#[tokio::test]
async fn edits_crud_thumb_version_and_event() {
    let e = env();
    let ids = setup(&e, &[("a.jpg", [100, 100, 100])]).await;
    let id = ids[0];
    let mut rx = e.core.events.subscribe();

    let p0 = e.core.photo(id).await.unwrap();
    assert!(!p0.has_edits);
    let doc = e.core.get_edit(id).await.unwrap();
    assert_eq!(doc.stack, json!({"version":1,"ops":[]}));
    assert_eq!(doc.updated_at, None);

    // unknown ops survive verbatim
    let stack = json!({"version":1,"ops":[
        {"type":"global","exposure":0.5},
        {"type":"future_op","mesh":[1,2,3]}
    ]});
    let put = e.core.put_edit(id, stack.clone()).await.unwrap();
    assert_ne!(put.thumb_version, p0.thumb_version);
    assert_eq!(put.stack, stack);
    let p1 = e.core.photo(id).await.unwrap();
    assert!(p1.has_edits);
    assert_eq!(p1.thumb_version, put.thumb_version);
    let ev = next_edit_event(&mut rx, id).await;
    assert!(ev.has_edits);
    assert_eq!(ev.thumb_version, put.thumb_version);
    let doc = e.core.get_edit(id).await.unwrap();
    assert_eq!(doc.stack, stack);
    assert!(doc.updated_at.is_some());

    // the same stack again is a no-op (no new version, same timestamp)
    let again = e.core.put_edit(id, stack.clone()).await.unwrap();
    assert_eq!(again.updated_at, put.updated_at);
    assert_eq!(again.thumb_version, put.thumb_version);

    // a different edit changes thumb_version
    let put2 = e.core.put_edit(id, exposure_stack(1.0)).await.unwrap();
    assert_ne!(put2.thumb_version, put.thumb_version);
    let ev = next_edit_event(&mut rx, id).await;
    assert_eq!(ev.thumb_version, put2.thumb_version);

    // an empty stack resets
    let reset = e
        .core
        .put_edit(id, json!({"version":1,"ops":[]}))
        .await
        .unwrap();
    assert_eq!(reset.thumb_version, p0.thumb_version);
    let ev = next_edit_event(&mut rx, id).await;
    assert!(!ev.has_edits);
    assert_eq!(ev.thumb_version, p0.thumb_version);
    assert!(!e.core.photo(id).await.unwrap().has_edits);

    // DELETE
    e.core.put_edit(id, exposure_stack(1.0)).await.unwrap();
    e.core.delete_edit(id).await.unwrap();
    let p = e.core.photo(id).await.unwrap();
    assert!(!p.has_edits);
    assert_eq!(p.thumb_version, p0.thumb_version);
    assert_eq!(e.core.get_edit(id).await.unwrap().updated_at, None);

    // unknown photo
    assert!(matches!(
        e.core.get_edit(9999).await,
        Err(CoreError::NotFound(_))
    ));
    assert!(matches!(
        e.core.put_edit(9999, exposure_stack(1.0)).await,
        Err(CoreError::NotFound(_))
    ));
    assert!(matches!(
        e.core.delete_edit(9999).await,
        Err(CoreError::NotFound(_))
    ));
}

#[tokio::test]
async fn only_a_few_versions_are_kept() {
    let e = env();
    let id = setup(&e, &[("a.jpg", [100, 100, 100])]).await[0];
    for i in 0..9 {
        e.core
            .put_edit(id, exposure_stack(i as f64 / 10.0))
            .await
            .unwrap();
    }
    let n: i64 = e
        .core
        .db
        .call(move |c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM edit_version WHERE photo_id=?1",
                [id],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(n, crate::edit::store::KEEP_VERSIONS);
    let cur = e.core.get_edit(id).await.unwrap();
    assert_eq!(cur.stack["ops"][0]["exposure"], 0.8);
}

#[test]
fn validation_rejects_bad_stacks() {
    let bad = |v: Value| matches!(validate_stack(v), Err(CoreError::Unprocessable(_)));
    assert!(!bad(json!({"version":1,"ops":[]})));
    assert!(!bad(json!({"ops":[{"type":"global","exposure":2.0}]}))); // version defaults
    assert!(bad(json!([])));
    assert!(bad(json!({"ops":"nope"})));
    assert!(bad(json!({"version":2,"ops":[]})));
    assert!(bad(
        json!({"version":1,"ops":[{"type":"global","exposure":9}]})
    ));
    assert!(bad(
        json!({"version":1,"ops":[{"type":"global","temp":-4000}]})
    ));
    assert!(bad(
        json!({"version":1,"ops":[{"type":"global","contrast":101}]})
    ));
    assert!(bad(
        json!({"version":1,"ops":[{"type":"global","exposure":"x"}]})
    ));
    assert!(bad(
        json!({"version":1,"ops":[{"type":"crop","rect":[0.5,0.5,0.8,0.8]}]})
    ));
    assert!(bad(
        json!({"version":1,"ops":[{"type":"crop","rect":[0,0,1,1],"angle":60}]})
    ));
    assert!(bad(json!({"version":1,"ops":[{"type":"crop"}]}))); // missing rect
    assert!(bad(
        json!({"version":1,"ops":[{"type":"local","adjust":{}}]})
    )); // missing mask
    assert!(bad(json!({"version":1,"ops":[
        {"type":"local","mask":{"kind":"radial","center":[0.5,0.5],"radius":[0,0]},"adjust":{}}]})));
    assert!(bad(
        json!({"version":1,"ops":[{"type":"lut","file":"x","amount":2}]})
    ));
    assert!(bad(
        json!({"version":1,"ops":[{"type":"output_sharpen","amount":-1}]})
    ));
    assert!(bad(json!({"version":1,"ops":[
        {"type":"global","curve":{"rgb":[[0.5,0],[0.2,1]]}}]}))); // unsorted curve
                                                                  // unknown op types are fine
    assert!(!bad(
        json!({"version":1,"ops":[{"type":"future_op","x":1}]})
    ));
    let ok = validate_stack(json!({"version":1,"ops":[{"type":"future_op"}]})).unwrap();
    assert!(!ok.has_edits, "only unknown ops: nothing visible");
    // hash is stable
    let h = stack_hash(&json!({"version":1,"ops":[]}));
    assert_eq!(h, stack_hash(&json!({"ops":[],"version":1})));
}

// ------------------------------------------------------------------ rendering

#[tokio::test]
async fn edited_thumbs_and_previews_are_rendered_and_restored() {
    let e = env();
    let id = setup(&e, &[("a.jpg", [100, 100, 100])]).await[0];
    let orig = e.core.thumb_path(id, 256).await.unwrap();
    let orig_mean = mean_of_file(&orig);
    let orig_preview = e.core.preview_path(id, 1024).await.unwrap();

    e.core.put_edit(id, exposure_stack(1.5)).await.unwrap();
    let t = e.core.thumb_path(id, 256).await.unwrap();
    assert_ne!(t, orig);
    assert!(mean_of_file(&t) > orig_mean + 20.0);
    let pv = e.core.preview_path(id, 1024).await.unwrap();
    assert_ne!(pv, orig_preview);
    assert!(mean_of_file(&pv) > mean_of_file(&orig_preview) + 20.0);

    // a new edit invalidates the old rendering
    e.core.put_edit(id, exposure_stack(-1.0)).await.unwrap();
    let t2 = e.core.thumb_path(id, 256).await.unwrap();
    assert_ne!(t2, t);
    assert!(mean_of_file(&t2) < orig_mean - 10.0);

    // clearing edits restores the originals
    e.core.delete_edit(id).await.unwrap();
    assert_eq!(e.core.thumb_path(id, 256).await.unwrap(), orig);
    assert_eq!(e.core.preview_path(id, 1024).await.unwrap(), orig_preview);
}

#[tokio::test]
async fn failing_renderer_falls_back_to_the_original_thumbnail() {
    let e = env();
    let id = setup(&e, &[("a.jpg", [100, 100, 100])]).await[0];
    let orig = e.core.thumb_path(id, 256).await.unwrap();
    e.renderer.fail.store(true, Ordering::SeqCst);
    e.core.put_edit(id, exposure_stack(1.0)).await.unwrap();
    assert_eq!(e.core.thumb_path(id, 256).await.unwrap(), orig);
}

#[tokio::test]
async fn preview_uses_saved_stack_override_and_original() {
    let e = env();
    let id = setup(&e, &[("a.jpg", [100, 100, 100])]).await[0];
    let base = mean_of_jpeg(&preview(&e, id, None, 128).await.jpeg);
    assert_eq!(
        e.renderer.calls.load(Ordering::SeqCst),
        0,
        "identity skips the renderer"
    );

    e.core.put_edit(id, exposure_stack(1.0)).await.unwrap();
    // put_edit renders the grid thumbnail in the background; let it finish so it cannot
    // bump the renderer call count while this test is counting
    e.core.thumb_path(id, 256).await.unwrap();
    let saved = preview(&e, id, None, 128).await;
    assert!(mean_of_jpeg(&saved.jpeg) > base + 15.0);
    assert_eq!(saved.backend, Backend::Gpu);

    // explicit stack wins over the saved one
    let dark = preview(&e, id, Some(exposure_stack(-1.5)), 128).await;
    assert!(mean_of_jpeg(&dark.jpeg) < base - 10.0);

    // original:true ignores edits (and does not touch the renderer)
    let calls = e.renderer.calls.load(Ordering::SeqCst);
    let o = e
        .core
        .render_preview(PreviewRequest {
            photo_id: id,
            stack: Some(exposure_stack(2.0)),
            long_edge: Some(128),
            original: Some(true),
        })
        .await
        .unwrap();
    assert!((mean_of_jpeg(&o.jpeg) - base).abs() < 3.0);
    assert_eq!(e.renderer.calls.load(Ordering::SeqCst), calls);

    // bad inputs
    for edge in [0u32, 8, 9000] {
        let r = e
            .core
            .render_preview(PreviewRequest {
                photo_id: id,
                stack: None,
                long_edge: Some(edge),
                original: None,
            })
            .await;
        assert!(matches!(r, Err(CoreError::BadRequest(_))), "{edge}");
    }
    let r = e
        .core
        .render_preview(PreviewRequest {
            photo_id: id,
            stack: Some(exposure_stack(9.0)),
            long_edge: None,
            original: None,
        })
        .await;
    assert!(matches!(r, Err(CoreError::Unprocessable(_))));
    let r = e
        .core
        .render_preview(PreviewRequest {
            photo_id: 777,
            stack: None,
            long_edge: None,
            original: None,
        })
        .await;
    assert!(matches!(r, Err(CoreError::NotFound(_))));
}

#[tokio::test]
async fn slider_dragging_reuses_the_decoded_proxy() {
    let e = env();
    let id = setup(&e, &[("a.jpg", [100, 100, 100])]).await[0];
    let svc = e.core.render_service().clone();
    let before = svc.decodes.load(Ordering::SeqCst);
    for i in 0..5 {
        preview(&e, id, Some(exposure_stack(i as f64 / 10.0 + 0.1)), 100).await;
    }
    assert_eq!(svc.decodes.load(Ordering::SeqCst) - before, 1);
    assert_eq!(svc.renders.load(Ordering::SeqCst), 5);
    // a crop that needs more resolution decodes again (different bucket), once
    let crop = json!({"version":1,"ops":[{"type":"crop","rect":[0,0,0.1,0.1]},{"type":"global","exposure":0.5}]});
    preview(&e, id, Some(crop.clone()), 100).await;
    preview(&e, id, Some(crop), 100).await;
    assert!(svc.decodes.load(Ordering::SeqCst) - before <= 2);
}

#[tokio::test]
async fn crop_is_applied_to_previews() {
    let e = env();
    let id = setup(&e, &[("a.jpg", [100, 100, 100])]).await[0];
    let crop = json!({"version":1,"ops":[{"type":"crop","rect":[0.0,0.0,0.5,1.0]}]});
    let p = preview(&e, id, Some(crop), 1000).await;
    let img = image::load_from_memory(&p.jpeg).unwrap();
    assert_eq!((img.width(), img.height()), (80, 120));
}

#[tokio::test]
async fn luts_are_resolved_through_the_library() {
    let e = env();
    let id = setup(&e, &[("a.jpg", [100, 100, 100])]).await[0];
    let stack = json!({"version":1,"ops":[
        {"type":"lut","file":"film_warm","amount":0.6},
        {"type":"lut","file":"no_such_look","amount":1}]});
    preview(&e, id, Some(stack), 100).await;
    let seen = e.renderer.luts_seen.lock().unwrap().clone();
    assert_eq!(seen, ["film_warm", "no_such_look"]);
    let lib = &e.core.render_service().luts;
    assert!(lib.resolve("film_warm").unwrap().is_some());
    assert!(lib.resolve("no_such_look").unwrap().is_none());
}

#[tokio::test]
async fn import_lut_errors() {
    let e = env();
    let bad = |r: Result<LutOut>| matches!(r, Err(CoreError::BadRequest(_)));
    assert!(bad(e.core.import_lut("".into()).await));
    assert!(bad(e.core.import_lut("look.png".into()).await));
    let missing = e.src.path().join("missing.cube");
    assert!(bad(e
        .core
        .import_lut(missing.to_string_lossy().into())
        .await));
    let dir = e.src.path().join("dir.cube");
    std::fs::create_dir_all(&dir).unwrap();
    assert!(bad(e.core.import_lut(dir.to_string_lossy().into()).await));
    let garbage = e.src.path().join("garbage.cube");
    std::fs::write(&garbage, "this is not a LUT").unwrap();
    assert!(bad(e
        .core
        .import_lut(garbage.to_string_lossy().into())
        .await));
}

// ------------------------------------------------------------------ presets

#[tokio::test]
async fn user_presets_crud_and_filtering() {
    let e = env();
    let stack = json!({"version":1,"ops":[
        {"type":"crop","rect":[0,0,0.5,0.5]},
        {"type":"global","exposure":0.3},
        {"type":"local","mask":{"kind":"ai","target":"sky"},"adjust":{"exposure":-0.3}},
        {"type":"local","mask":{"kind":"ai","target":"person","person_id":3},"adjust":{"exposure":0.3}},
        {"type":"lut","file":"film_warm","amount":0.5},
        {"type":"output_sharpen","amount":10},
        {"type":"future_op","x":1}
    ]});
    let p = e
        .core
        .create_preset(PresetCreate {
            name: "  My look ".into(),
            stack,
        })
        .await
        .unwrap();
    assert!(!p.builtin);
    assert_eq!(p.name, "My look");
    let types: Vec<&str> = p.stack["ops"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["type"].as_str().unwrap())
        .collect();
    assert_eq!(types, ["global", "local", "lut", "output_sharpen"]);
    assert_eq!(p.stack["ops"][1]["mask"]["target"], "sky");

    let list = e.core.presets().await.unwrap();
    assert!(list.iter().any(|x| x.id == p.id && !x.builtin));

    // validation
    for (name, stack) in [
        ("", exposure_stack(0.1)),
        (
            "x",
            json!({"version":1,"ops":[{"type":"crop","rect":[0,0,1,1]}]}),
        ), // nothing savable
        ("x", exposure_stack(9.0)),
    ] {
        let r = e
            .core
            .create_preset(PresetCreate {
                name: name.into(),
                stack,
            })
            .await;
        assert!(matches!(r, Err(CoreError::Unprocessable(_))), "{name:?}");
    }

    e.core.delete_preset(&p.id).await.unwrap();
    assert!(!e.core.presets().await.unwrap().iter().any(|x| x.id == p.id));
    assert!(matches!(
        e.core.delete_preset(&p.id).await,
        Err(CoreError::NotFound(_))
    ));
    assert!(matches!(
        e.core.delete_preset("nonsense").await,
        Err(CoreError::NotFound(_))
    ));
}

// ------------------------------------------------------------------ sync

fn op_types(stack: &Value) -> Vec<String> {
    stack["ops"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["type"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn sync_copies_only_the_included_kinds() {
    let e = env();
    let ids = setup(
        &e,
        &[
            ("a.jpg", [120, 120, 120]),
            ("b.jpg", [120, 120, 120]),
            ("c.jpg", [120, 120, 120]),
        ],
    )
    .await;
    let (a, b, c) = (ids[0], ids[1], ids[2]);
    e.core
        .put_edit(
            a,
            json!({"version":1,"ops":[
                {"type":"crop","rect":[0.1,0.1,0.8,0.8]},
                {"type":"global","exposure":0.7,"contrast":10},
                {"type":"lut","file":"film_warm","amount":0.5}]}),
        )
        .await
        .unwrap();
    e.core
        .put_edit(
            b,
            json!({"version":1,"ops":[
                {"type":"global","exposure":-0.4},
                {"type":"output_sharpen","amount":15}]}),
        )
        .await
        .unwrap();
    let mut rx = e.core.events.subscribe();

    let n = e
        .core
        .sync_edits(SyncRequest {
            from_id: a,
            to_ids: vec![b, c, a, b, 4242],
            include: vec![
                crate::edit::sync::SyncKind::Global,
                crate::edit::sync::SyncKind::Lut,
            ],
            adaptive: Some(false),
        })
        .await
        .unwrap();
    assert_eq!(n, 2, "source, duplicates and unknown ids are skipped");

    let sb = e.core.get_edit(b).await.unwrap().stack;
    assert_eq!(op_types(&sb), ["global", "lut", "output_sharpen"]);
    assert_eq!(sb["ops"][0]["exposure"], 0.7);
    assert_eq!(sb["ops"][0]["contrast"], 10);
    let sc = e.core.get_edit(c).await.unwrap().stack;
    assert_eq!(op_types(&sc), ["global", "lut"], "no crop copied");
    assert!(e.core.photo(c).await.unwrap().has_edits);
    // the source is untouched
    assert_eq!(op_types(&e.core.get_edit(a).await.unwrap().stack).len(), 3);

    // events for both targets
    let (ub, uc) = (
        next_edit_event(&mut rx, b).await,
        next_edit_event(&mut rx, c).await,
    );
    assert!(ub.has_edits && uc.has_edits);
    assert_eq!(
        uc.thumb_version,
        e.core.photo(c).await.unwrap().thumb_version
    );

    // only crop: replaces the target's (absent) crop, leaves the rest
    e.core
        .sync_edits(SyncRequest {
            from_id: a,
            to_ids: vec![b],
            include: vec![crate::edit::sync::SyncKind::Crop],
            adaptive: Some(false),
        })
        .await
        .unwrap();
    let sb = e.core.get_edit(b).await.unwrap().stack;
    assert_eq!(op_types(&sb), ["crop", "global", "lut", "output_sharpen"]);

    // syncing from a photo without edits clears the included kinds
    e.core
        .sync_edits(SyncRequest {
            from_id: c,
            to_ids: vec![b],
            include: vec![crate::edit::sync::SyncKind::Crop],
            adaptive: None,
        })
        .await
        .unwrap();
    let sb = e.core.get_edit(b).await.unwrap().stack;
    assert!(!op_types(&sb).contains(&"crop".to_string()));

    // errors
    let r = e
        .core
        .sync_edits(SyncRequest {
            from_id: a,
            to_ids: vec![b],
            include: vec![],
            adaptive: None,
        })
        .await;
    assert!(matches!(r, Err(CoreError::BadRequest(_))));
    let r = e
        .core
        .sync_edits(SyncRequest {
            from_id: 9999,
            to_ids: vec![b],
            include: vec![crate::edit::sync::SyncKind::Global],
            adaptive: None,
        })
        .await;
    assert!(matches!(r, Err(CoreError::NotFound(_))));
}

#[tokio::test]
async fn adaptive_sync_matches_rendered_brightness_and_white_balance() {
    let e = env();
    let ids = setup(
        &e,
        &[
            ("a_src.jpg", [140, 140, 140]),
            ("b_dark.jpg", [70, 70, 70]),
            ("c_warm.jpg", [145, 140, 132]),
        ],
    )
    .await;
    let (a, b, c) = (ids[0], ids[1], ids[2]);
    let look = json!({"version":1,"ops":[
        {"type":"crop","rect":[0.0,0.0,0.5,0.5]},
        {"type":"global","exposure":0.3,"contrast":15}]});
    e.core.put_edit(a, look.clone()).await.unwrap();

    let render_mean = |id: i64, stack: Value| {
        let e = &e;
        async move {
            // whole frame, crop removed
            let mut s = stack;
            s["ops"]
                .as_array_mut()
                .unwrap()
                .retain(|o| o["type"] != "crop");
            let p = preview(e, id, Some(s), 200).await;
            let img = image::load_from_memory(&p.jpeg).unwrap().to_rgb8();
            let n = (img.width() * img.height()) as f64;
            let mut ch = [0f64; 3];
            for px in img.pixels() {
                for (acc, v) in ch.iter_mut().zip(px.0) {
                    *acc += v as f64;
                }
            }
            [ch[0] / n, ch[1] / n, ch[2] / n]
        }
    };
    let target_lum = {
        let m = render_mean(a, look.clone()).await;
        (m[0] + m[1] + m[2]) / 3.0
    };

    // plain copy: far from the source's brightness / balance
    e.core
        .sync_edits(SyncRequest {
            from_id: a,
            to_ids: vec![b, c],
            include: vec![crate::edit::sync::SyncKind::Global],
            adaptive: Some(false),
        })
        .await
        .unwrap();
    let plain_b = {
        let m = render_mean(b, e.core.get_edit(b).await.unwrap().stack).await;
        (m[0] + m[1] + m[2]) / 3.0
    };
    assert!((plain_b - target_lum).abs() > 25.0);
    let plain_c = render_mean(c, e.core.get_edit(c).await.unwrap().stack).await;

    // adaptive: close to the source
    let n = e
        .core
        .sync_edits(SyncRequest {
            from_id: a,
            to_ids: vec![b, c],
            include: vec![crate::edit::sync::SyncKind::Global],
            adaptive: Some(true),
        })
        .await
        .unwrap();
    assert_eq!(n, 2);
    let sb = e.core.get_edit(b).await.unwrap().stack;
    assert!(
        sb["ops"][0]["exposure"].as_f64().unwrap() > 0.3 + 0.5,
        "dark target is brightened: {sb}"
    );
    assert_eq!(
        sb["ops"][0]["contrast"], 15,
        "other fields are copied as is"
    );
    let m = render_mean(b, sb).await;
    let lum_b = (m[0] + m[1] + m[2]) / 3.0;
    assert!(
        (lum_b - target_lum).abs() < target_lum * 0.06,
        "adaptive b {lum_b} vs source {target_lum}"
    );

    let sc = e.core.get_edit(c).await.unwrap().stack;
    assert!(
        sc["ops"][0]["temp"].as_f64().unwrap() < 0.0,
        "warm target is cooled: {sc}"
    );
    let m = render_mean(c, sc).await;
    let cast = |m: [f64; 3]| (m[0] - m[2]).abs();
    assert!(
        cast(m) < cast(plain_c) * 0.5,
        "colour cast shrinks: {m:?} vs {plain_c:?}"
    );

    // include without `global`: adaptive is a no-op (nothing tonal copied)
    e.core.delete_edit(b).await.unwrap();
    e.core
        .sync_edits(SyncRequest {
            from_id: a,
            to_ids: vec![b],
            include: vec![crate::edit::sync::SyncKind::Crop],
            adaptive: Some(true),
        })
        .await
        .unwrap();
    let sb = e.core.get_edit(b).await.unwrap().stack;
    assert_eq!(op_types(&sb), ["crop"]);
}

// ------------------------------------------------------------------ export

#[tokio::test]
async fn export_applies_saved_edits() {
    let e = env();
    let ids = setup(
        &e,
        &[("a.jpg", [100, 100, 100]), ("b.jpg", [100, 100, 100])],
    )
    .await;
    let (a, b) = (ids[0], ids[1]);
    e.core.put_edit(a, exposure_stack(1.5)).await.unwrap();
    let orig_b = std::fs::read(e.src.path().join("b.jpg")).unwrap();
    let orig_mean = mean_of_file(&e.src.path().join("a.jpg"));

    let run = |dest: std::path::PathBuf, apply: bool, long_edge: Option<u32>| {
        let core = e.core.clone();
        let ids = vec![a, b];
        async move {
            let mut rx = core.events.subscribe();
            core.export(ExportRequest {
                ids,
                dest: dest.to_string_lossy().into_owned(),
                long_edge,
                quality: 90,
                name_template: "{name}".into(),
                apply_edits: apply,
                upscale: None,
                strip_gps: false,
                folders: None,
            })
            .await
            .unwrap();
            tokio::time::timeout(WAIT, async {
                loop {
                    if let Ok(Event::TaskProgress { state, error, .. }) = rx.recv().await {
                        if state != "running" {
                            assert_eq!(state, "done", "{error:?}");
                            return;
                        }
                    }
                }
            })
            .await
            .expect("export finishes");
        }
    };

    // default: edits applied; the unedited photo is still a byte-exact copy
    let d1 = e.src.path().join("out1");
    run(d1.clone(), true, None).await;
    assert!(mean_of_file(&d1.join("a.jpg")) > orig_mean + 20.0);
    assert_eq!(std::fs::read(d1.join("b.jpg")).unwrap(), orig_b);

    // apply_edits=false: both are plain copies
    let d2 = e.src.path().join("out2");
    run(d2.clone(), false, None).await;
    assert_eq!(
        std::fs::read(d2.join("a.jpg")).unwrap(),
        std::fs::read(e.src.path().join("a.jpg")).unwrap()
    );

    // resize: edited photo rendered at the target size
    let d3 = e.src.path().join("out3");
    run(d3.clone(), true, Some(80)).await;
    let img = image::open(d3.join("a.jpg")).unwrap();
    assert_eq!(img.width().max(img.height()), 80);
    assert!(mean_of_file(&d3.join("a.jpg")) > orig_mean + 15.0);
    let rendered_full = e.renderer.last_source.lock().unwrap().is_some();
    assert!(rendered_full);
}

// ------------------------------------------------------------------ masks

#[tokio::test]
async fn masks_are_generated_cached_and_inverted() {
    let e = env();
    let id = setup(&e, &[("a.jpg", [100, 100, 100])]).await[0];
    let png = e.core.mask_png(id, "sky", None).await.unwrap();
    let m = image::load_from_memory(&png).unwrap().to_luma8();
    assert_eq!(m.get_pixel(0, 0).0[0], 255);
    assert_eq!(m.get_pixel(31, 0).0[0], 0);
    assert_eq!(e.worker.mask_calls.load(Ordering::SeqCst), 1);
    let req = e.worker.last_mask_request.lock().unwrap().clone().unwrap();
    assert_eq!(req.targets, ["sky"]);
    assert!(!req.allow_download);
    assert_eq!(req.size, 1024);
    assert_eq!(req.photo.photo_id, id);
    assert!(req.person_bbox.is_none());

    // cached on disk
    e.core.mask_png(id, "sky", None).await.unwrap();
    assert_eq!(e.worker.mask_calls.load(Ordering::SeqCst), 1);

    // background = inverted subject
    let bg = e.core.mask_png(id, "background", None).await.unwrap();
    let bg = image::load_from_memory(&bg).unwrap().to_luma8();
    assert_eq!(bg.get_pixel(0, 0).0[0], 0);
    assert_eq!(bg.get_pixel(31, 0).0[0], 255);
    assert_eq!(e.worker.mask_calls.load(Ordering::SeqCst), 2);
    let last = e.worker.last_mask_request.lock().unwrap().clone().unwrap();
    assert_eq!(last.targets, ["subject"]);

    // request validation
    assert!(matches!(
        e.core.mask_png(id, "dragon", None).await,
        Err(CoreError::BadRequest(_))
    ));
    assert!(matches!(
        e.core.mask_png(id, "person", None).await,
        Err(CoreError::BadRequest(_))
    ));
    assert!(matches!(
        e.core.mask_png(id, "person", Some(5)).await,
        Err(CoreError::NotFound(_))
    ));
    assert!(matches!(
        e.core.mask_png(9999, "sky", None).await,
        Err(CoreError::NotFound(_))
    ));
}

#[tokio::test]
async fn mask_errors_map_to_409_and_503() {
    let e = env();
    let id = setup(&e, &[("a.jpg", [100, 100, 100])]).await[0];
    e.worker.set_mask_missing(&["birefnet", "segformer-sky"]);
    match e.core.mask_png(id, "sky", None).await {
        Err(CoreError::ModelsMissing(m)) => assert_eq!(m, ["birefnet", "segformer-sky"]),
        other => panic!("{other:?}"),
    }
    e.worker.set_mask_missing(&[]);
    e.worker.set_mask_skipped(&["hair"]);
    assert!(matches!(
        e.core.mask_png(id, "hair", None).await,
        Err(CoreError::ModelsMissing(_))
    ));
    assert!(e.core.mask_png(id, "sky", None).await.is_ok());
    e.worker.set_mask_skipped(&[]);
    e.worker.mask_unavailable.store(true, Ordering::SeqCst);
    assert!(matches!(
        e.core.mask_png(id, "skin", None).await,
        Err(CoreError::WorkerUnavailable(_))
    ));
}

#[tokio::test]
async fn ai_masks_feed_the_renderer_and_failures_surface_in_previews_only() {
    let e = env();
    let id = setup(&e, &[("a.jpg", [100, 100, 100])]).await[0];
    let stack = json!({"version":1,"ops":[
        {"type":"local","mask":{"kind":"ai","target":"sky"},"amount":1,"adjust":{"exposure":2.0}}]});

    // strict preview generates the mask; the fake mask brightens the left half
    let p = preview(&e, id, Some(stack.clone()), 160).await;
    assert_eq!(e.worker.mask_calls.load(Ordering::SeqCst), 1);
    let img = image::load_from_memory(&p.jpeg).unwrap().to_rgb8();
    assert!(img.get_pixel(5, 60).0[0] > img.get_pixel(150, 60).0[0] + 40);

    // missing models: preview is a 409-style error, thumbnails silently skip the mask
    let id2 = {
        let ids = setup(&e, &[("b.jpg", [100, 100, 100])]).await;
        ids[0]
    };
    e.worker.set_mask_missing(&["birefnet"]);
    let r = e
        .core
        .render_preview(PreviewRequest {
            photo_id: id2,
            stack: Some(stack.clone()),
            long_edge: Some(100),
            original: None,
        })
        .await;
    assert!(matches!(r, Err(CoreError::ModelsMissing(_))), "{r:?}");
    let calls = e.worker.mask_calls.load(Ordering::SeqCst);
    e.core.put_edit(id2, stack).await.unwrap();
    let t = e.core.thumb_path(id2, 256).await.unwrap();
    assert!(t.is_file());
    assert_eq!(
        e.worker.mask_calls.load(Ordering::SeqCst),
        calls,
        "thumbnails never start the worker"
    );
}

// ------------------------------------------------------------------ auto

#[tokio::test]
async fn auto_endpoint_returns_an_adjust_without_saving() {
    let e = env();
    let id = setup(&e, &[("a.jpg", [60, 60, 60])]).await[0];
    // (ip-render's real auto_adjust is a stub here: it returns the neutral adjust)
    let adj = e
        .core
        .auto_adjust(id, ip_render::AutoMode::Auto)
        .await
        .unwrap();
    assert!(adj.exposure.abs() <= 5.0);
    assert!(!e.core.photo(id).await.unwrap().has_edits);
    let ctx_req: AutoRequest = serde_json::from_value(json!({"mode":"portrait"})).unwrap();
    assert_eq!(ctx_req.mode, ip_render::AutoMode::Portrait);
    assert!(serde_json::from_value::<AutoRequest>(json!({"mode":"bogus"})).is_err());
    assert!(matches!(
        e.core.auto_adjust(999, ip_render::AutoMode::Auto).await,
        Err(CoreError::NotFound(_))
    ));
}

// ------------------------------------------------------------------ real ip-render

fn real_env() -> Env {
    // renderer: None -> ip_render::create_renderer(prefer_gpu = !force_cpu) = the CPU path
    env_with(None, Arc::new(FakeRenderer::default()))
}

#[tokio::test]
async fn real_renderer_exposure_and_auto() {
    let e = real_env();
    let id = setup(&e, &[("a.jpg", [60, 60, 60])]).await[0];
    let base = mean_of_jpeg(&preview(&e, id, None, 128).await.jpeg);
    let up = mean_of_jpeg(&preview(&e, id, Some(exposure_stack(1.0)), 128).await.jpeg);
    assert!(up > base + 20.0, "{up} vs {base}");
    let adj = e
        .core
        .auto_adjust(id, ip_render::AutoMode::Auto)
        .await
        .unwrap();
    assert!(adj.exposure > 0.0, "a dark frame is brightened: {adj:?}");
}

#[tokio::test]
async fn real_builtin_presets_are_protected_and_listed() {
    let e = real_env();
    let list = e.core.presets().await.unwrap();
    let builtin: Vec<_> = list.iter().filter(|p| p.builtin).collect();
    assert!(!builtin.is_empty());
    assert!(builtin[0].name.starts_with("preset."));
    assert!(matches!(
        e.core.delete_preset(&builtin[0].id).await,
        Err(CoreError::BadRequest(_))
    ));
}

#[tokio::test]
async fn real_lut_import_and_use() {
    let e = real_env();
    let id = setup(&e, &[("a.jpg", [100, 100, 100])]).await[0];
    let cube = e.src.path().join("My Look.cube");
    std::fs::write(
        &cube,
        "TITLE \"x\"\nLUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n",
    )
    .unwrap();
    let out = e
        .core
        .import_lut(cube.to_string_lossy().into_owned())
        .await
        .unwrap();
    assert_eq!(out.name, "My Look");
    assert!(e.core.dirs.luts.join(format!("{}.cube", out.id)).is_file());
    // the same file maps to the same id
    let again = e
        .core
        .import_lut(cube.to_string_lossy().into_owned())
        .await
        .unwrap();
    assert_eq!(again.id, out.id);
    let stack = json!({"version":1,"ops":[{"type":"lut","file":out.id,"amount":1}]});
    let p = preview(&e, id, Some(stack), 128).await;
    assert!(mean_of_jpeg(&p.jpeg) > 0.0);
}

#[tokio::test]
async fn real_adaptive_sync() {
    let e = real_env();
    let ids = setup(&e, &[("a.jpg", [140, 140, 140]), ("b.jpg", [70, 70, 70])]).await;
    e.core.put_edit(ids[0], exposure_stack(0.3)).await.unwrap();
    e.core
        .sync_edits(SyncRequest {
            from_id: ids[0],
            to_ids: vec![ids[1]],
            include: vec![crate::edit::sync::SyncKind::Global],
            adaptive: Some(true),
        })
        .await
        .unwrap();
    let sb = e.core.get_edit(ids[1]).await.unwrap().stack;
    assert!(sb["ops"][0]["exposure"].as_f64().unwrap() > 0.8, "{sb}");
}
