//! M6 tests: assistant plans / execution / undo, keep-top, settings, cache, models, face data
//! and XMP. FakeImaging + FakeWorker + FakeRenderer.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use ip_render::Backend;
use ip_worker_client::{LlmCall, LlmPlanResponse};
use serde_json::{json, Value};

use crate::assistant::{PlanContext, PlanOut, PlanRequest};
use crate::fake_worker::{person, FakeFace, FakeSpec, FakeWorker, VLM_MODEL};
use crate::testutil::{write_jpeg, FakeImaging, FakeRenderer};
use crate::xmp::XmpSyncRequest;
use crate::*;

const BASE: u64 = 1_700_000_000;
const WAIT: Duration = Duration::from_secs(30);

struct Env {
    core: Arc<Core>,
    worker: Arc<FakeWorker>,
    data: tempfile::TempDir,
    src: tempfile::TempDir,
}

fn env() -> Env {
    env_with(Backend::Cpu)
}

fn env_with(backend: Backend) -> Env {
    let data = tempfile::tempdir().unwrap();
    open_in(data, backend)
}

fn open_in(data: tempfile::TempDir, backend: Backend) -> Env {
    let src = tempfile::tempdir().unwrap();
    let worker = Arc::new(FakeWorker::new());
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(2),
        worker: Some(worker.clone()),
        renderer: Some(Arc::new(FakeRenderer::new(backend))),
        force_cpu: true,
    })
    .unwrap();
    crate::testutil::grant_face_consent(&core);
    Env {
        core,
        worker,
        data,
        src,
    }
}

fn photo(dir: &Path, name: &str, t_s: u64) {
    let p = dir.join(name);
    write_jpeg(&p, 320, 240, (t_s % 200) as u8);
    let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
    f.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(BASE + t_s))
        .unwrap();
}

async fn import_dir(e: &Env, dir: &Path) -> i64 {
    let s = e
        .core
        .import(ImportRequest {
            path: dir.to_string_lossy().into_owned(),
            recursive: true,
            title: None,
        })
        .await
        .unwrap();
    e.core.wait_session_ready(s.id, WAIT).await.unwrap();
    s.id
}

async fn analyze(e: &Env, sid: i64) {
    e.core
        .analysis_run(AnalysisRunRequest {
            session_id: sid,
            profile: Profile::Standard,
            photo_ids: None,
            force: false,
            allow_download: false,
        })
        .await
        .unwrap();
    for _ in 0..1200 {
        let s = e.core.analysis_status(sid);
        if s.state != RunState::Running {
            assert_eq!(s.state, RunState::Done, "{s:?}");
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("analysis did not finish");
}

async fn by_name(e: &Env, sid: i64) -> HashMap<String, Photo> {
    e.core
        .photos(PhotoQuery {
            session_id: sid,
            limit: Some(5000),
            ..Default::default()
        })
        .await
        .unwrap()
        .photos
        .into_iter()
        .map(|p| (p.file_name.clone(), p))
        .collect()
}

/// Two scenes: scene 1 has bursts (a1 a2 a3) and (b1 b2); scene 2 has (c1 c2).
async fn scene_session() -> (Env, i64) {
    let e = env();
    for (name, t, angle, sharp) in [
        ("a1.jpg", 0, 0.0, 0.9),
        ("a2.jpg", 1, 0.0, 0.5),
        ("a3.jpg", 2, 0.0, 0.3),
        ("b1.jpg", 100, 30.0, 0.8),
        ("b2.jpg", 101, 30.0, 0.4),
        ("c1.jpg", 3000, 0.0, 0.6),
        ("c2.jpg", 3001, 0.0, 0.95),
    ] {
        photo(e.src.path(), name, t);
        e.worker.set(name, FakeSpec::at(angle).sharp(sharp));
    }
    let sid = import_dir(&e, e.src.path()).await;
    analyze(&e, sid).await;
    (e, sid)
}

async fn flags(e: &Env, sid: i64) -> HashMap<String, i64> {
    by_name(e, sid)
        .await
        .into_iter()
        .map(|(k, p)| (k, p.flag))
        .collect()
}

fn plan_req(sid: i64, msg: &str) -> PlanRequest {
    PlanRequest {
        session_id: sid,
        message: msg.into(),
        context: PlanContext::default(),
        engine: None,
    }
}

async fn run_plan(e: &Env, plan: &PlanOut) -> (bool, Vec<Value>, Value) {
    let mut rx = e.core.events.subscribe();
    e.core.assistant_execute(&plan.plan_id).await.unwrap();
    let wait = async {
        loop {
            if let Ok(Event::AssistantDone {
                plan_id,
                ok,
                results,
                undo,
            }) = rx.recv().await
            {
                if plan_id == plan.plan_id {
                    return (ok, results, undo);
                }
            }
        }
    };
    tokio::time::timeout(WAIT, wait)
        .await
        .expect("assistant.done")
}

// ------------------------------------------------------------------ keep top

#[tokio::test]
async fn keep_top_per_burst_and_per_scene() {
    let (e, sid) = scene_session().await;
    let groups = e.core.groups(sid).await.unwrap();
    assert_eq!(groups.scenes.len(), 2, "{groups:?}");
    let ph = by_name(&e, sid).await;
    let score = |id: i64| ph.values().find(|p| p.id == id).unwrap().ai_score.unwrap();

    // best frame of every burst is picked, the rest rejected
    let out = e.core.group_keep_top(sid, None, 1, true).await.unwrap();
    let bursts: Vec<&crate::BurstOut> = groups.scenes.iter().flat_map(|s| &s.bursts).collect();
    assert_eq!(out.kept.len(), bursts.len());
    for b in &bursts {
        let best = b
            .photo_ids
            .iter()
            .copied()
            .max_by(|x, y| score(*x).partial_cmp(&score(*y)).unwrap())
            .unwrap();
        assert!(out.kept.contains(&best), "{b:?}");
    }
    let f = flags(&e, sid).await;
    assert_eq!(f.values().filter(|v| **v == 1).count(), bursts.len());
    assert_eq!(f.values().filter(|v| **v == -1).count(), 7 - bursts.len());

    // reset; per scene (spreads over bursts), without rejecting
    let all: Vec<i64> = ph.values().map(|p| p.id).collect();
    e.core
        .patch_photos(PatchRequest {
            ids: all,
            flag: Some(0),
            ..Default::default()
        })
        .await
        .unwrap();
    let out = e.core.scene_keep_top(sid, None, 1, false).await.unwrap();
    assert_eq!(out.kept.len(), 2, "one per scene");
    let f = flags(&e, sid).await;
    assert_eq!(f.values().filter(|v| **v == 1).count(), 2);
    assert!(
        f.values().all(|v| *v != -1),
        "reject_rest=false rejects nothing"
    );
    // a selection limits who competes
    let only: Vec<i64> = ["a2.jpg", "a3.jpg"].iter().map(|n| ph[*n].id).collect();
    let plan = e
        .core
        .keep_top_plan(sid, crate::assistant::KeepScope::Group, Some(&only), 1)
        .await
        .unwrap();
    assert_eq!(plan.kept.len(), 1);
    assert!(only.contains(&plan.kept[0]));
    assert_eq!(plan.rest.len(), 1);
    // rejected photos neither compete nor get touched
    e.core
        .patch_photos(PatchRequest {
            ids: vec![ph["a1.jpg"].id],
            flag: Some(-1),
            ..Default::default()
        })
        .await
        .unwrap();
    let plan = e
        .core
        .keep_top_plan(sid, crate::assistant::KeepScope::Group, None, 1)
        .await
        .unwrap();
    assert!(!plan.kept.contains(&ph["a1.jpg"].id) && !plan.rest.contains(&ph["a1.jpg"].id));
}

// ------------------------------------------------------------------ plan / execute / undo

#[tokio::test]
async fn plan_execute_and_undo_ratings_and_flags() {
    let (e, sid) = scene_session().await;
    let plan = e
        .core
        .assistant_plan(plan_req(sid, "每个场景只留1张，其他淘汰"))
        .await
        .unwrap();
    assert_eq!(plan.engine, "rules");
    assert!(plan.unsupported.is_none());
    assert_eq!(plan.steps.len(), 1);
    let step = &plan.steps[0];
    assert_eq!(step.tool, "scene_keep_top");
    assert!(step.destructive && plan.needs_confirmation);
    assert_eq!(step.affects, 7, "2 kept + 5 rejected");
    assert!(!step.summary.is_empty() && plan.reply.contains("好的"));
    // nothing happened yet
    assert!(flags(&e, sid).await.values().all(|v| *v == 0));

    let (ok, results, undo) = run_plan(&e, &plan).await;
    assert!(ok, "{results:?}");
    assert_eq!(results[0]["tool"], "scene_keep_top");
    assert_eq!(results[0]["affected"], 7);
    let f = flags(&e, sid).await;
    assert_eq!(f.values().filter(|v| **v == 1).count(), 2);
    assert_eq!(f.values().filter(|v| **v == -1).count(), 5);

    // the undo payload restores everything
    let photos = undo["photos"].as_array().unwrap();
    assert_eq!(photos.len(), 7);
    assert!(photos
        .iter()
        .all(|p| p["flag"] == 0 && p["user_rating"].is_null() && p["color_label"].is_null()));
    assert!(undo["edits"].as_array().unwrap().is_empty());
    for p in photos {
        e.core
            .patch_photos(PatchRequest {
                ids: vec![p["id"].as_i64().unwrap()],
                flag: p["flag"].as_i64(),
                ..Default::default()
            })
            .await
            .unwrap();
    }
    assert!(flags(&e, sid).await.values().all(|v| *v == 0));

    // a plan runs once
    assert!(matches!(
        e.core.assistant_execute(&plan.plan_id).await,
        Err(CoreError::NotFound(_))
    ));
    assert!(matches!(
        e.core.assistant_execute("plan-nope").await,
        Err(CoreError::NotFound(_))
    ));
}

#[tokio::test]
async fn plan_with_filter_and_action_and_ratings() {
    let (e, sid) = scene_session().await;
    // sharp photos are blurry-free; rate the first two of the filter (by AI) 5 stars
    let plan = e
        .core
        .assistant_plan(plan_req(sid, "把前2张最好的评5星"))
        .await
        .unwrap();
    assert_eq!(plan.steps.len(), 1, "{plan:?}");
    assert_eq!(plan.steps[0].affects, 2);
    assert!(
        plan.steps[0].destructive,
        "rating more than one photo is a bulk change"
    );
    let (ok, results, undo) = run_plan(&e, &plan).await;
    assert!(ok, "{results:?}");
    let ph = by_name(&e, sid).await;
    let rated: Vec<&Photo> = ph.values().filter(|p| p.user_rating == Some(5)).collect();
    assert_eq!(rated.len(), 2);
    let best = ph
        .values()
        .map(|p| p.ai_score.unwrap())
        .fold(f64::MIN, f64::max);
    assert!(
        rated.iter().any(|p| p.ai_score == Some(best)),
        "best photo included"
    );
    assert_eq!(undo["photos"].as_array().unwrap().len(), 2);

    // bulk rating is destructive and filter steps are not
    let plan = e
        .core
        .assistant_plan(plan_req(sid, "给所有照片打3星"))
        .await
        .unwrap();
    assert!(plan.steps[0].destructive && plan.needs_confirmation);
    assert_eq!(plan.steps[0].affects, 7);
    let plan = e
        .core
        .assistant_plan(plan_req(sid, "只看5星"))
        .await
        .unwrap();
    assert_eq!(plan.steps[0].tool, "filter");
    assert_eq!(plan.steps[0].affects, 2);
    assert!(!plan.needs_confirmation);
    let (ok, results, _) = run_plan(&e, &plan).await;
    assert!(ok);
    assert_eq!(results[0]["args"]["rating_gte"], 5);
}

#[tokio::test]
async fn execute_preset_and_undo_edit_stacks() {
    let (e, sid) = scene_session().await;
    let plan = e
        .core
        .assistant_plan(plan_req(sid, "给所有照片应用胶片暖调"))
        .await
        .unwrap();
    assert_eq!(plan.steps[0].tool, "apply_preset");
    assert_eq!(plan.steps[0].args["preset_id"], "film_warm");
    assert_eq!(plan.steps[0].affects, 7);
    // batch edits on many photos ask for confirmation (still undoable)
    assert!(plan.steps[0].destructive && plan.needs_confirmation);
    let (ok, results, undo) = run_plan(&e, &plan).await;
    assert!(ok, "{results:?}");
    assert_eq!(results[0]["affected"], 7);
    let ph = by_name(&e, sid).await;
    assert!(ph.values().all(|p| p.has_edits));
    let id = ph["a1.jpg"].id;
    let stack = e.core.get_edit(id).await.unwrap().stack;
    assert!(stack["ops"]
        .as_array()
        .unwrap()
        .iter()
        .any(|o| o["type"] == "lut" || o["type"] == "global"));
    // undo = put every `before` stack back
    let edits = undo["edits"].as_array().unwrap();
    assert_eq!(edits.len(), 7);
    for ed in edits {
        assert_eq!(ed["before"]["ops"], json!([]));
        e.core
            .put_edit(ed["photo_id"].as_i64().unwrap(), ed["before"].clone())
            .await
            .unwrap();
    }
    assert!(by_name(&e, sid).await.values().all(|p| !p.has_edits));

    // auto adjust saves a global op
    let plan = e
        .core
        .assistant_plan(plan_req(sid, "一键修图"))
        .await
        .unwrap();
    let (ok, results, undo) = run_plan(&e, &plan).await;
    assert!(ok, "{results:?}");
    let stack = e.core.get_edit(id).await.unwrap().stack;
    assert!(stack["ops"]
        .as_array()
        .unwrap()
        .iter()
        .any(|o| o["type"] == "global" && o["source"] == "ai_auto@1"));
    assert!(!undo["edits"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn export_steps_and_dest_required() {
    let (e, sid) = scene_session().await;
    let out = tempfile::tempdir().unwrap();
    // mixed plan: the export without dest fails alone, the other step still runs
    // via the LLM path: the model proposes both steps
    e.core.hardware(true).await.unwrap();
    e.worker.enable_llm(LlmPlanResponse {
        reply: "ok".into(),
        calls: vec![
            LlmCall {
                tool: "accept_ai".into(),
                args: json!({"selection":{"query":""}}),
            },
            LlmCall {
                tool: "export".into(),
                args: json!({"selection":{"query":""},"preset":"xiaohongshu"}),
            },
        ],
    });
    let plan = e
        .core
        .assistant_plan(plan_req(sid, "accept ratings then export"))
        .await
        .unwrap();
    assert_eq!(plan.engine, "llm");
    assert_eq!(plan.steps.len(), 2);
    assert!(plan.steps[1].destructive);
    let (ok, results, _) = run_plan(&e, &plan).await;
    assert!(!ok);
    assert_eq!(results[0]["ok"], true);
    assert_eq!(results[1]["ok"], false);
    assert_eq!(results[1]["error"], "dest_required");

    // with a destination: xiaohongshu is a 3:4 crop-fit of 1440x1920 at most (never upscaled)
    e.worker.enable_llm(LlmPlanResponse {
        reply: "export".into(),
        calls: vec![LlmCall {
            tool: "export".into(),
            args: json!({"selection":{"query":"limit=2"},"preset":"xiaohongshu","dest":out.path().to_string_lossy()}),
        }],
    });
    let plan = e
        .core
        .assistant_plan(plan_req(sid, "export two"))
        .await
        .unwrap();
    let (ok, results, _) = run_plan(&e, &plan).await;
    assert!(ok, "{results:?}");
    let files: Vec<PathBuf> = std::fs::read_dir(out.path())
        .unwrap()
        .map(|d| d.unwrap().path())
        .collect();
    assert_eq!(files.len(), 2);
    for f in files {
        let (w, h) = image::image_dimensions(&f).unwrap();
        // 320x240 source: the 3:4 centre crop is 180x240
        assert_eq!((w, h), (180, 240), "{}", f.display());
    }
}

#[test]
fn fit_crop_keeps_aspect_and_never_upscales() {
    let img = ip_render::RgbImage {
        width: 400,
        height: 200,
        data: vec![7; 400 * 200 * 3],
    };
    let o = crate::export::fit_crop(&img, 100, 200);
    assert_eq!(
        (o.width, o.height),
        (100, 200),
        "crop only, already small enough"
    );
    let big = ip_render::RgbImage {
        width: 4000,
        height: 3000,
        data: vec![1; 4000 * 3000 * 3],
    };
    let o = crate::export::fit_crop(&big, 1080, 1350);
    assert_eq!((o.width, o.height), (1080, 1350));
    let o = crate::export::fit_crop(&big, 4000, 3000);
    assert_eq!((o.width, o.height), (4000, 3000));
}

#[tokio::test]
async fn plans_expire_after_thirty_minutes() {
    let (e, sid) = scene_session().await;
    let plan = e
        .core
        .assistant_plan(plan_req(sid, "一键修图"))
        .await
        .unwrap();
    assert_eq!(e.core.assistant.plan_count(), 1);
    e.core.assistant.age_plan(
        &plan.plan_id,
        crate::assistant::PLAN_TTL + Duration::from_secs(1),
    );
    assert!(matches!(
        e.core.assistant_execute(&plan.plan_id).await,
        Err(CoreError::NotFound(_))
    ));
    assert_eq!(e.core.assistant.plan_count(), 0);
    // unsupported messages store nothing and explain themselves
    let p = e
        .core
        .assistant_plan(plan_req(sid, "make the sky pink"))
        .await
        .unwrap();
    assert!(p.unsupported.is_some() && p.steps.is_empty());
    assert_eq!(e.core.assistant.plan_count(), 0);
    assert!(matches!(
        e.core.assistant_plan(plan_req(sid, "  ")).await,
        Err(CoreError::BadRequest(_))
    ));
    assert!(matches!(
        e.core.assistant_plan(plan_req(999, "一键修图")).await,
        Err(CoreError::NotFound(_))
    ));
    let mut bad = plan_req(sid, "x");
    bad.engine = Some("magic".into());
    assert!(matches!(
        e.core.assistant_plan(bad).await,
        Err(CoreError::BadRequest(_))
    ));
}

#[tokio::test]
async fn people_names_resolve_in_plans() {
    let e = env();
    for (name, t) in [("p1.jpg", 0), ("p2.jpg", 500), ("p3.jpg", 1000)] {
        photo(e.src.path(), name, t);
    }
    let face = |k| FakeFace::new([0.2, 0.2, 0.3, 0.4], person(k, 0.0));
    e.worker
        .set("p1.jpg", FakeSpec::at(0.0).with_faces(vec![face(1)]));
    e.worker
        .set("p2.jpg", FakeSpec::at(90.0).with_faces(vec![face(1)]));
    e.worker
        .set("p3.jpg", FakeSpec::at(180.0).with_faces(vec![face(2)]));
    let sid = import_dir(&e, e.src.path()).await;
    analyze(&e, sid).await;
    let people = e.core.people(Some(sid)).await.unwrap();
    let ming = people.iter().max_by_key(|p| p.photo_count).unwrap();
    e.core
        .patch_person(
            ming.id,
            PersonPatch {
                name: Some(Some("小明".into())),
                hidden: None,
            },
        )
        .await
        .unwrap();
    let plan = e
        .core
        .assistant_plan(plan_req(sid, "只看小明的照片"))
        .await
        .unwrap();
    assert_eq!(plan.steps[0].tool, "filter");
    assert_eq!(plan.steps[0].args["persons"], json!([ming.id]));
    assert_eq!(plan.steps[0].affects, 2);
    assert!(plan.steps[0].summary.contains("小明"));
    // an LLM that invents a person id is rejected (and the rules answer instead)
    e.core.hardware(true).await.unwrap();
    e.worker.enable_llm(LlmPlanResponse {
        reply: "x".into(),
        calls: vec![LlmCall {
            tool: "filter".into(),
            args: json!({"persons":[424242]}),
        }],
    });
    let plan = e
        .core
        .assistant_plan(plan_req(sid, "只看小明的照片"))
        .await
        .unwrap();
    assert_eq!(plan.engine, "rules");
    assert_eq!(plan.steps[0].args["persons"], json!([ming.id]));
    // faces disabled: names are not offered any more
    e.core
        .patch_settings(json!({"faces":{"enabled":false}}))
        .await
        .unwrap();
    let plan = e
        .core
        .assistant_plan(plan_req(sid, "只看小明的照片"))
        .await
        .unwrap();
    assert!(plan.unsupported.is_some());
    assert!(e.core.people(Some(sid)).await.unwrap().is_empty());
}

// ------------------------------------------------------------------ LLM engine

#[tokio::test]
async fn llm_engine_is_validated_and_falls_back() {
    let (e, sid) = scene_session().await;
    e.core.hardware(true).await.unwrap();
    let status = e.core.assistant_status(false).await;
    assert!(!status.llm_available && status.engine == "rules");

    e.worker.enable_llm(LlmPlanResponse {
        reply: "已为你淘汰 4 星以下".into(),
        calls: vec![LlmCall {
            tool: "set_flag".into(),
            args: json!({"selection":"current_filter","flag":-1}),
        }],
    });
    e.core.assistant.invalidate_info();
    let status = e.core.assistant_status(false).await;
    assert!(status.llm_available && status.engine == "llm");
    assert_eq!(
        status.llm_model.as_deref(),
        Some(crate::testutil::LLM_MODEL)
    );
    assert!(!status.vlm_available);

    let plan = e
        .core
        .assistant_plan(plan_req(sid, "please reject the weak ones"))
        .await
        .unwrap();
    assert_eq!(plan.engine, "llm");
    assert_eq!(plan.reply, "已为你淘汰 4 星以下");
    assert_eq!(plan.steps[0].tool, "set_flag");
    assert!(plan.steps[0].destructive);
    let req = e.worker.last_llm_request.lock().unwrap().clone().unwrap();
    assert_eq!(req.tools.len(), 14, "all tool schemas are offered");
    assert!(req
        .tools
        .iter()
        .all(|t| t["parameters"]["type"] == "object"));
    assert_eq!(req.locale, "en");

    // the rules engine can be forced
    let calls = e.worker.llm_calls.load(Ordering::SeqCst);
    let mut r = plan_req(sid, "一键修图");
    r.engine = Some("rules".into());
    let p = e.core.assistant_plan(r).await.unwrap();
    assert_eq!(p.engine, "rules");
    assert_eq!(e.worker.llm_calls.load(Ordering::SeqCst), calls);

    // invalid model output never becomes a plan: the rules take over
    for bad in [
        json!({"tool":"drop_table","args":{}}),
        json!({"tool":"set_rating","args":{"selection":"current_filter","rating":9}}),
        json!({"tool":"set_flag","args":{"selection":"everything","flag":1}}),
        json!({"tool":"apply_preset","args":{"selection":"current_filter","preset_id":"does_not_exist"}}),
        json!({"tool":"filter","args":{"rating_gte":"lots"}}),
    ] {
        e.worker.enable_llm(LlmPlanResponse {
            reply: "trust me".into(),
            calls: vec![LlmCall {
                tool: bad["tool"].as_str().unwrap().into(),
                args: bad["args"].clone(),
            }],
        });
        let p = e
            .core
            .assistant_plan(plan_req(sid, "一键修图"))
            .await
            .unwrap();
        assert_eq!(p.engine, "rules", "{bad}");
        assert_eq!(p.steps[0].tool, "auto_adjust", "{bad}");
        assert_ne!(p.reply, "trust me");
    }
    // too many steps
    e.worker.enable_llm(LlmPlanResponse {
        reply: "x".into(),
        calls: (0..20)
            .map(|_| LlmCall {
                tool: "filter".into(),
                args: json!({}),
            })
            .collect(),
    });
    let p = e
        .core
        .assistant_plan(plan_req(sid, "一键修图"))
        .await
        .unwrap();
    assert_eq!(p.engine, "rules");
    // a model that has nothing to say
    e.worker.enable_llm(LlmPlanResponse::default());
    let p = e
        .core
        .assistant_plan(plan_req(sid, "一键修图"))
        .await
        .unwrap();
    assert_eq!(p.engine, "rules");
}

#[tokio::test]
async fn vlm_describe_and_suggest() {
    let (e, sid) = scene_session().await;
    let id = by_name(&e, sid).await["a1.jpg"].id;
    // no VLM at all
    assert!(matches!(
        e.core.assistant_describe(id).await,
        Err(CoreError::WorkerUnavailable(_))
    ));
    e.worker.enable_vlm(
        "a red photo",
        &["red", "  red ", "test"],
        &["underexposed"],
        json!({"exposure": 0.7, "shadows": 20}),
        "lift the shadows",
    );
    let d = e.core.assistant_describe(id).await.unwrap();
    assert_eq!(d.caption, "a red photo");
    assert_eq!(d.keywords, ["red", "test"]);
    let s = e.core.assistant_suggest(id).await.unwrap();
    assert!((s.adjust["exposure"].as_f64().unwrap() - 0.7).abs() < 1e-6);
    assert_eq!(s.adjust["source"], "vlm_suggest@1");
    assert_eq!(s.problems, ["underexposed"]);
    // nothing was saved
    assert!(!e.core.photo(id).await.unwrap().has_edits);
    // an out-of-range suggestion is refused
    e.worker
        .enable_vlm("c", &[], &[], json!({"exposure": 9.0}), "bad");
    assert!(matches!(
        e.core.assistant_suggest(id).await,
        Err(CoreError::Unprocessable(_))
    ));
    // as plan steps the results travel in `data`
    e.worker
        .enable_vlm("c", &["k"], &[], json!({"contrast": 10}), "r");
    let mut r = plan_req(sid, "describe this photo");
    r.context.current_photo_id = Some(id);
    let plan = e.core.assistant_plan(r).await.unwrap();
    assert_eq!(plan.steps[0].tool, "describe");
    let (ok, results, _) = run_plan(&e, &plan).await;
    assert!(ok);
    assert_eq!(results[0]["data"]["caption"], "c");
    let mut r = plan_req(sid, "给我一些修图建议");
    r.context.current_photo_id = Some(id);
    let plan = e.core.assistant_plan(r).await.unwrap();
    let (_, results, _) = run_plan(&e, &plan).await;
    assert_eq!(results[0]["data"]["adjust"]["contrast"], 10.0);
    assert_eq!(results[0]["data"]["reason"], "r");
    // a VLM that is registered but not installed: 409
    e.worker.set_missing(&[VLM_MODEL]);
    assert!(matches!(
        e.core.assistant_describe(id).await,
        Err(CoreError::ModelsMissing(_))
    ));
}

// ------------------------------------------------------------------ settings

#[tokio::test]
async fn settings_persist_and_apply_live() {
    let e = env_with(Backend::Gpu);
    assert_eq!(e.core.render.backend(), Backend::Gpu);
    let mut rx = e.core.events.subscribe();
    // render backend
    e.core
        .patch_settings(json!({"render":{"backend":"cpu"}}))
        .await
        .unwrap();
    assert_eq!(e.core.render.backend(), Backend::Cpu);
    // models source and offline mode go to the worker's next start
    e.core
        .patch_settings(json!({"models":{"source":"hf-mirror"}}))
        .await
        .unwrap();
    let opts = e.worker.last_options.lock().unwrap().clone().unwrap();
    assert!(opts
        .env
        .contains(&("HF_ENDPOINT".into(), "https://hf-mirror.com".into())));
    assert_eq!(
        opts.models_dir.unwrap(),
        PathBuf::from(&e.core.settings().models.dir)
    );
    // network disabled: downloads are refused with a clear error
    e.core
        .patch_settings(json!({"privacy":{"allow_network":false}}))
        .await
        .unwrap();
    let opts = e.worker.last_options.lock().unwrap().clone().unwrap();
    assert!(opts.env.contains(&("HF_HUB_OFFLINE".into(), "1".into())));
    let err = e
        .core
        .models_ensure(vec!["yunet".into()])
        .await
        .unwrap_err();
    assert!(
        matches!(&err, CoreError::Conflict(m) if m.contains("network")),
        "{err:?}"
    );
    let sid = {
        photo(e.src.path(), "n.jpg", 0);
        import_dir(&e, e.src.path()).await
    };
    let err = e
        .core
        .analysis_run(AnalysisRunRequest {
            session_id: sid,
            profile: Profile::Fast,
            photo_ids: None,
            force: false,
            allow_download: true,
        })
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Conflict(_)));
    e.core
        .patch_settings(json!({"privacy":{"allow_network":true}}))
        .await
        .unwrap();
    assert!(e.core.models_ensure(vec!["yunet".into()]).await.is_ok());
    // validation
    assert!(matches!(
        e.core.patch_settings(json!({"cache":{"max_gb":-1}})).await,
        Err(CoreError::Unprocessable(_))
    ));
    assert!(matches!(
        e.core.patch_settings(json!({"bogus":1})).await,
        Err(CoreError::BadRequest(_))
    ));
    assert_eq!(e.core.settings().cache.max_gb, 20.0);
    // persistence: a new core on the same data dir sees it
    e.core
        .patch_settings(json!({"theme":"dark","analysis":{"group_strictness":"strict"}}))
        .await
        .unwrap();
    while rx.try_recv().is_ok() {}
    // Close the first core but keep the data dir alive: dropping the whole `Env` would delete
    // the temp dir, and the reopened core would then start from defaults.
    let Env { core, data, .. } = e;
    drop(core);
    let worker = Arc::new(FakeWorker::new());
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(1),
        worker: Some(worker.clone()),
        renderer: Some(Arc::new(FakeRenderer::new(Backend::Gpu))),
        force_cpu: true,
    })
    .unwrap();
    let s = core.settings();
    assert_eq!(s.theme, "dark");
    assert_eq!(s.analysis.group_strictness, "strict");
    assert_eq!(s.models.source, "hf-mirror");
    // the worker is configured at start-up
    assert!(worker.last_options.lock().unwrap().is_some());
}

#[tokio::test]
async fn faces_disabled_skips_identity_steps() {
    let e = env();
    photo(e.src.path(), "f1.jpg", 0);
    let face = FakeFace::new([0.2, 0.2, 0.3, 0.4], person(1, 0.0));
    e.worker
        .set("f1.jpg", FakeSpec::at(0.0).with_faces(vec![face]));
    e.core
        .patch_settings(json!({"faces":{"enabled":false}}))
        .await
        .unwrap();
    let sid = import_dir(&e, e.src.path()).await;
    analyze(&e, sid).await;
    let steps = e.worker.last_steps.lock().unwrap().clone().unwrap();
    assert!(
        !steps.iter().any(|s| s == "faces" || s == "identity"),
        "{steps:?}"
    );
    assert!(steps.iter().any(|s| s == "phash"));
    assert!(e.core.people(Some(sid)).await.unwrap().is_empty());
    // enabled again: the worker's own profile is used
    e.core
        .patch_settings(json!({"faces":{"enabled":true}}))
        .await
        .unwrap();
    e.core
        .analysis_run(AnalysisRunRequest {
            session_id: sid,
            profile: Profile::Standard,
            photo_ids: None,
            force: true,
            allow_download: false,
        })
        .await
        .unwrap();
    for _ in 0..400 {
        if e.core.analysis_status(sid).state != RunState::Running {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(e.worker.last_steps.lock().unwrap().is_none());
}

#[tokio::test]
async fn clearing_face_data() {
    let e = env();
    photo(e.src.path(), "f1.jpg", 0);
    let face = FakeFace::new([0.2, 0.2, 0.3, 0.4], person(1, 0.0));
    e.worker
        .set("f1.jpg", FakeSpec::at(0.0).with_faces(vec![face]));
    let sid = import_dir(&e, e.src.path()).await;
    analyze(&e, sid).await;
    assert_eq!(
        e.core
            .photo(by_name(&e, sid).await["f1.jpg"].id)
            .await
            .unwrap()
            .face_count,
        Some(1)
    );
    let out = e.core.clear_faces().await.unwrap();
    assert_eq!(out["faces"], 1);
    let p = by_name(&e, sid).await.remove("f1.jpg").unwrap();
    assert_eq!(p.face_count, None);
    assert!(p.analyzed, "the rest of the analysis stays");
    e.core
        .patch_settings(json!({"faces":{"enabled":true}}))
        .await
        .unwrap();
    assert!(e.core.people(Some(sid)).await.unwrap().is_empty());
}

// ------------------------------------------------------------------ cache / models / onboarding

fn put(dir: &Path, rel: &str, bytes: usize, age_s: u64) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, vec![1u8; bytes]).unwrap();
    let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
    let t = std::time::SystemTime::now() - Duration::from_secs(age_s);
    f.set_modified(t).unwrap();
    // accessed times may not be tracked; the cache uses the newer of both, so keep them old
    let _ = f.set_times(std::fs::FileTimes::new().set_modified(t).set_accessed(t));
}

#[tokio::test]
async fn cache_accounting_clear_and_eviction() {
    let e = env();
    let d = e.core.dirs.clone();
    put(&d.thumbs, "ab/cd/t1.jpg", 100, 50);
    put(&d.thumbs, "ab/cd/t2.jpg", 100, 40);
    put(&d.previews, "ab/cd/p1.jpg", 1000, 3000);
    put(&d.masks, "m1.png", 500, 10);
    put(&d.edited_thumbs, "ab/cd/e1.jpg", 50, 10);
    put(&d.gen, "task/x.bin", 2000, 5);
    let info = e.core.cache_info().await.unwrap();
    assert_eq!(info.items.thumbs, 2);
    assert_eq!(info.items.previews, 1);
    assert_eq!(info.items.masks, 1);
    assert_eq!(info.items.edits, 1);
    assert_eq!(info.items.gen, 1);
    assert_eq!(info.sizes.thumbs, 200);
    assert_eq!(info.bytes, 200 + 1000 + 500 + 50 + 2000);
    assert_eq!(info.max_bytes, 20 * 1024 * 1024 * 1024);

    // eviction: scratch first, then rendered/previews, oldest first; thumbs and masks last
    let freed = e.core.evict_cache_to(1000);
    assert!(freed > 0);
    let info = e.core.cache_info().await.unwrap();
    assert!(info.bytes <= 1000, "{info:?}");
    assert_eq!(info.items.gen, 0, "scratch went first");
    assert_eq!(info.items.previews, 0);
    assert_eq!(info.items.masks, 1, "masks are the last to go");
    assert_eq!(
        e.core.evict_cache_to(10_000_000),
        0,
        "within budget: nothing happens"
    );

    // clearing thumbs also resets the catalog's thumbnail state
    photo(e.src.path(), "x.jpg", 0);
    let sid = import_dir(&e, e.src.path()).await;
    assert!(by_name(&e, sid).await["x.jpg"].thumb_ready);
    let freed = e.core.cache_clear(vec!["thumbs".into()]).await.unwrap();
    assert!(freed >= 200);
    assert!(!by_name(&e, sid).await["x.jpg"].thumb_ready);
    assert_eq!(e.core.cache_info().await.unwrap().items.thumbs, 0);
    e.core.cache_clear(vec!["all".into()]).await.unwrap();
    assert_eq!(e.core.cache_info().await.unwrap().bytes, 0);
    assert!(matches!(
        e.core.cache_clear(vec!["everything".into()]).await,
        Err(CoreError::BadRequest(_))
    ));
    assert!(matches!(
        e.core.cache_clear(vec![]).await,
        Err(CoreError::BadRequest(_))
    ));
    // thumbnails regenerate on request
    assert!(e
        .core
        .thumb_path(by_name(&e, sid).await["x.jpg"].id, 256)
        .await
        .unwrap()
        .is_file());
}

#[tokio::test]
async fn deleting_models() {
    let e = env();
    e.core.delete_model("siglip2-base").await.unwrap();
    assert_eq!(*e.worker.deleted_models.lock().unwrap(), ["siglip2-base"]);
    assert!(e
        .core
        .models()
        .await
        .unwrap()
        .iter()
        .any(|m| m.id == "siglip2-base" && !m.installed));
    assert!(matches!(
        e.core.delete_model("no-such-model").await,
        Err(CoreError::NotFound(_))
    ));
    for bad in ["", "../x", "a/b", ".hidden", "_cache", "a b"] {
        assert!(
            matches!(
                e.core.delete_model(bad).await,
                Err(CoreError::BadRequest(_))
            ),
            "{bad:?}"
        );
    }
    // a worker without models.delete: the directory under the models dir goes
    e.worker.delete_unsupported.store(true, Ordering::SeqCst);
    let dir = PathBuf::from(e.core.settings().models.dir.clone()).join("old-model");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("w.onnx"), b"x").unwrap();
    e.core.delete_model("old-model").await.unwrap();
    assert!(!dir.exists());
    assert!(matches!(
        e.core.delete_model("old-model").await,
        Err(CoreError::NotFound(_))
    ));
}

#[tokio::test]
async fn onboarding_state() {
    let e = env();
    let o = e.core.onboarding().await.unwrap();
    assert_eq!(o["first_run"], true);
    assert_eq!(o["recommended_tier"], "T3");
    assert!(o["hardware"]["device"].is_string());
    assert_eq!(o["recommended_models"], json!([]));
    e.worker.set_missing(&["siglip2-base"]);
    let o = e.core.onboarding().await.unwrap();
    assert_eq!(o["recommended_models"], json!(["siglip2-base"]));
    assert_eq!(o["recommended_download_mb"], 178);
    e.core.onboarding_done().await.unwrap();
    assert_eq!(e.core.onboarding().await.unwrap()["first_run"], false);
}

// ------------------------------------------------------------------ XMP

const LR_SIDECAR: &str = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    xmp:Rating="4" xmp:Label="Red" crs:Exposure2012="+0.35">
   <dc:subject><rdf:Bag><rdf:li>kyoto</rdf:li><rdf:li>temple</rdf:li></rdf:Bag></dc:subject>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;

const DT_SIDECAR: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:darktable="http://darktable.sf.net/" xmp:Rating="-1" darktable:xmp_version="5">
<darktable:history><rdf:Seq><rdf:li darktable:operation="exposure" darktable:enabled="1"/></rdf:Seq></darktable:history>
<darktable:colorlabels><rdf:Seq><rdf:li>3</rdf:li></rdf:Seq></darktable:colorlabels>
</rdf:Description></rdf:RDF></x:xmpmeta>"#;

fn sidecar_text(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

#[tokio::test]
async fn xmp_is_read_on_import_in_both_naming_schemes() {
    let e = env();
    e.core
        .patch_settings(json!({"xmp_mode":"sidecar"}))
        .await
        .unwrap();
    photo(e.src.path(), "lr.jpg", 0);
    photo(e.src.path(), "dt.jpg", 10);
    photo(e.src.path(), "plain.jpg", 20);
    std::fs::write(e.src.path().join("lr.xmp"), LR_SIDECAR).unwrap();
    std::fs::write(e.src.path().join("dt.jpg.xmp"), DT_SIDECAR).unwrap();
    let sid = import_dir(&e, e.src.path()).await;
    let ph = by_name(&e, sid).await;
    let lr = &ph["lr.jpg"];
    assert_eq!(lr.user_rating, Some(4));
    assert_eq!(lr.color_label.as_deref(), Some("red"));
    assert_eq!(lr.flag, 0);
    assert_eq!(e.core.photo_tags(lr.id).await.unwrap(), ["kyoto", "temple"]);
    let dt = &ph["dt.jpg"];
    assert_eq!(dt.flag, -1, "Rating -1 is rejected");
    assert_eq!(
        dt.color_label.as_deref(),
        Some("blue"),
        "darktable label index 3"
    );
    let plain = &ph["plain.jpg"];
    assert_eq!((plain.user_rating, plain.flag), (None, 0));
    assert!(e.core.photo_tags(plain.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn xmp_off_ignores_sidecars() {
    let e = env();
    photo(e.src.path(), "lr.jpg", 0);
    std::fs::write(e.src.path().join("lr.xmp"), LR_SIDECAR).unwrap();
    let sid = import_dir(&e, e.src.path()).await;
    assert_eq!(by_name(&e, sid).await["lr.jpg"].user_rating, None);
    // and nothing is written
    let id = by_name(&e, sid).await["lr.jpg"].id;
    e.core
        .patch_photos(PatchRequest {
            ids: vec![id],
            user_rating: Some(Some(2)),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(e.core.xmp_flush().await.unwrap().written, 0);
    assert_eq!(sidecar_text(&e.src.path().join("lr.xmp")), LR_SIDECAR);
    assert!(matches!(
        e.core
            .xmp_sync(XmpSyncRequest {
                session_id: sid,
                direction: "read".into(),
                photo_ids: None
            })
            .await,
        Err(CoreError::Conflict(_))
    ));
}

#[tokio::test]
async fn xmp_writes_preserve_unknown_content() {
    let e = env();
    photo(e.src.path(), "a.jpg", 0);
    photo(e.src.path(), "b.jpg", 10);
    // an existing sidecar with develop settings and no rating
    let existing = LR_SIDECAR
        .replace("xmp:Rating=\"4\" xmp:Label=\"Red\"", "")
        .replace(
            "<dc:subject><rdf:Bag><rdf:li>kyoto</rdf:li><rdf:li>temple</rdf:li></rdf:Bag></dc:subject>",
            "",
        );
    std::fs::write(e.src.path().join("a.xmp"), existing).unwrap();
    let sid = import_dir(&e, e.src.path()).await;
    e.core
        .patch_settings(json!({"xmp_mode":"sidecar"}))
        .await
        .unwrap();
    let ph = by_name(&e, sid).await;
    let (a, b) = (ph["a.jpg"].id, ph["b.jpg"].id);
    e.core
        .patch_photos(PatchRequest {
            ids: vec![a, b],
            user_rating: Some(Some(3)),
            color_label: Some(Some("green".into())),
            ..Default::default()
        })
        .await
        .unwrap();
    e.core
        .set_tags(vec![a], vec!["trip".into(), " 京都 ".into()], vec![])
        .await
        .unwrap();
    let r = e.core.xmp_flush().await.unwrap();
    assert!(r.errors.is_empty(), "{r:?}");
    assert_eq!(r.written, 2);
    let text = sidecar_text(&e.src.path().join("a.xmp"));
    for keep in [
        "crs:Exposure2012=\"+0.35\"",
        "<?xpacket end=\"w\"?>",
        "trip",
        "京都",
    ] {
        assert!(text.contains(keep), "{keep} missing in\n{text}");
    }
    let x = crate::xmp::Xmp::parse(&text).unwrap();
    assert_eq!(x.rating(), Some(3));
    assert_eq!(x.label().as_deref(), Some("Green"));
    assert_eq!(x.keywords(), ["trip", "京都"]);
    // the new sidecar of b.jpg is <stem>.xmp
    let y = crate::xmp::Xmp::parse(&sidecar_text(&e.src.path().join("b.xmp"))).unwrap();
    assert_eq!(y.rating(), Some(3));
    // reject = Rating -1; removing the rating of a rejected photo keeps -1
    e.core
        .patch_photos(PatchRequest {
            ids: vec![b],
            flag: Some(-1),
            ..Default::default()
        })
        .await
        .unwrap();
    e.core.xmp_flush().await.unwrap();
    let y = crate::xmp::Xmp::parse(&sidecar_text(&e.src.path().join("b.xmp"))).unwrap();
    assert_eq!(y.rating(), Some(-1));
    // nothing changed: nothing is written again
    e.core
        .patch_photos(PatchRequest {
            ids: vec![b],
            flag: Some(-1),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(e.core.xmp_flush().await.unwrap().written, 0);
    // no temp files are left behind
    for f in std::fs::read_dir(e.src.path()).unwrap() {
        let n = f.unwrap().file_name().to_string_lossy().into_owned();
        assert!(!n.contains("ip-tmp"), "{n}");
    }
}

#[tokio::test]
async fn xmp_writes_are_debounced() {
    let e = env();
    photo(e.src.path(), "a.jpg", 0);
    let sid = import_dir(&e, e.src.path()).await;
    e.core
        .patch_settings(json!({"xmp_mode":"sidecar"}))
        .await
        .unwrap();
    e.core.set_xmp_debounce(Duration::from_millis(150));
    let id = by_name(&e, sid).await["a.jpg"].id;
    for r in 1..=5 {
        e.core
            .patch_photos(PatchRequest {
                ids: vec![id],
                user_rating: Some(Some(r)),
                ..Default::default()
            })
            .await
            .unwrap();
    }
    let sc = e.src.path().join("a.xmp");
    assert!(!sc.exists(), "not written immediately");
    for _ in 0..100 {
        if sc.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(sc.exists(), "written after the quiet period");
    assert_eq!(
        crate::xmp::Xmp::parse(&sidecar_text(&sc)).unwrap().rating(),
        Some(5)
    );
}

#[tokio::test]
async fn xmp_conflicts_let_the_sidecar_win() {
    let e = env();
    photo(e.src.path(), "a.jpg", 0);
    let sid = import_dir(&e, e.src.path()).await;
    e.core
        .patch_settings(json!({"xmp_mode":"sidecar"}))
        .await
        .unwrap();
    let id = by_name(&e, sid).await["a.jpg"].id;
    e.core
        .patch_photos(PatchRequest {
            ids: vec![id],
            user_rating: Some(Some(4)),
            ..Default::default()
        })
        .await
        .unwrap();
    e.core.xmp_flush().await.unwrap();
    let sc = e.src.path().join("a.xmp");
    // another program changes the rating, later
    let edited = sidecar_text(&sc).replace("xmp:Rating=\"4\"", "xmp:Rating=\"2\"");
    assert!(edited.contains("Rating=\"2\""));
    std::fs::write(&sc, edited).unwrap();
    let f = std::fs::OpenOptions::new().write(true).open(&sc).unwrap();
    f.set_modified(std::time::SystemTime::now() + Duration::from_secs(30))
        .unwrap();
    drop(f);
    let mut rx = e.core.events.subscribe();
    e.core
        .patch_photos(PatchRequest {
            ids: vec![id],
            user_rating: Some(Some(5)),
            ..Default::default()
        })
        .await
        .unwrap();
    let report = e.core.xmp_flush().await.unwrap();
    assert_eq!(report.conflicts, 1);
    assert_eq!(report.written, 0, "the sidecar was not overwritten");
    assert_eq!(e.core.photo(id).await.unwrap().user_rating, Some(2));
    let mut seen = None;
    while let Ok(ev) = rx.try_recv() {
        if let Event::XmpConflict {
            session_id,
            photo_id,
            sidecar,
            catalog,
        } = ev
        {
            seen = Some((session_id, photo_id, sidecar, catalog));
        }
    }
    let (s, p, sidecar, catalog) = seen.expect("xmp.conflict");
    assert_eq!((s, p), (sid, id));
    assert_eq!(sidecar["rating"], 2);
    assert_eq!(catalog["rating"], 5);
    // now they agree again: the next change is written normally
    e.core
        .patch_photos(PatchRequest {
            ids: vec![id],
            user_rating: Some(Some(3)),
            ..Default::default()
        })
        .await
        .unwrap();
    let report = e.core.xmp_flush().await.unwrap();
    assert_eq!((report.conflicts, report.written), (0, 1));
    assert_eq!(
        crate::xmp::Xmp::parse(&sidecar_text(&sc)).unwrap().rating(),
        Some(3)
    );
}

#[tokio::test]
async fn manual_xmp_sync_both_directions() {
    let e = env();
    photo(e.src.path(), "a.jpg", 0);
    photo(e.src.path(), "b.jpg", 10);
    let sid = import_dir(&e, e.src.path()).await;
    e.core
        .patch_settings(json!({"xmp_mode":"sidecar"}))
        .await
        .unwrap();
    let ph = by_name(&e, sid).await;
    let (a, b) = (ph["a.jpg"].id, ph["b.jpg"].id);
    // write: every photo of the session gets a sidecar
    e.core
        .patch_photos(PatchRequest {
            ids: vec![a],
            user_rating: Some(Some(5)),
            ..Default::default()
        })
        .await
        .unwrap();
    e.core.xmp.pending.lock().unwrap().clear();
    let r = e
        .core
        .xmp_sync(XmpSyncRequest {
            session_id: sid,
            direction: "write".into(),
            photo_ids: None,
        })
        .await
        .unwrap();
    assert_eq!((r.photos, r.written), (2, 2), "{r:?}");
    // an outside change to b, then read: the catalog follows (scoped to b)
    std::fs::write(e.src.path().join("b.xmp"), LR_SIDECAR).unwrap();
    let r = e
        .core
        .xmp_sync(XmpSyncRequest {
            session_id: sid,
            direction: "read".into(),
            photo_ids: Some(vec![b]),
        })
        .await
        .unwrap();
    assert_eq!((r.photos, r.updated), (1, 1), "{r:?}");
    let ph = by_name(&e, sid).await;
    assert_eq!(ph["b.jpg"].user_rating, Some(4));
    assert_eq!(ph["a.jpg"].user_rating, Some(5));
    assert_eq!(e.core.photo_tags(b).await.unwrap(), ["kyoto", "temple"]);
    assert!(matches!(
        e.core
            .xmp_sync(XmpSyncRequest {
                session_id: sid,
                direction: "sideways".into(),
                photo_ids: None
            })
            .await,
        Err(CoreError::BadRequest(_))
    ));
    assert!(matches!(
        e.core
            .xmp_sync(XmpSyncRequest {
                session_id: 999,
                direction: "read".into(),
                photo_ids: None
            })
            .await,
        Err(CoreError::NotFound(_))
    ));
}

#[tokio::test]
async fn embedded_xmp_rewrite_keeps_the_image_bytes() {
    let e = env();
    photo(e.src.path(), "a.jpg", 0);
    let path = e.src.path().join("a.jpg");
    let before = std::fs::read(&path).unwrap();
    let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
    let sid = import_dir(&e, e.src.path()).await;
    e.core
        .patch_settings(json!({"xmp_mode":"modify_originals","confirm_modify_originals":true}))
        .await
        .unwrap();
    let id = by_name(&e, sid).await["a.jpg"].id;
    e.core
        .patch_photos(PatchRequest {
            ids: vec![id],
            user_rating: Some(Some(4)),
            color_label: Some(Some("purple".into())),
            ..Default::default()
        })
        .await
        .unwrap();
    e.core
        .set_tags(vec![id], vec!["alpha".into()], vec![])
        .await
        .unwrap();
    let r = e.core.xmp_flush().await.unwrap();
    assert!(r.errors.is_empty(), "{r:?}");
    let after = std::fs::read(&path).unwrap();
    assert_ne!(before, after);
    // embedded packet carries the values
    let x = crate::xmp::Xmp::parse(&crate::xmp::jpeg::extract(&after).unwrap()).unwrap();
    assert_eq!(x.rating(), Some(4));
    assert_eq!(x.label().as_deref(), Some("Purple"));
    assert_eq!(x.keywords(), ["alpha"]);
    // everything from the start of scan on is byte identical, so are the other segments
    let scan = |b: &[u8]| {
        let segs = crate::xmp::jpeg::segments(b).unwrap();
        segs.last().unwrap().end
    };
    assert_eq!(before[scan(&before)..], after[scan(&after)..]);
    let strip = |b: &[u8]| {
        let segs = crate::xmp::jpeg::segments(b).unwrap();
        let mut v = Vec::new();
        let mut pos = 2;
        for s in &segs {
            let is_xmp =
                s.marker == 0xE1 && b[s.start + 4..s.end].starts_with(crate::xmp::jpeg::XMP_HEADER);
            if !is_xmp {
                v.extend_from_slice(&b[s.start..s.end]);
            }
            pos = s.end;
        }
        v.extend_from_slice(&b[pos..]);
        v
    };
    assert_eq!(
        strip(&before),
        strip(&after),
        "only the XMP segment changed"
    );
    // still a valid image with the same pixels
    let (a, b) = (
        image::open(&path).unwrap(),
        image::load_from_memory(&before).unwrap(),
    );
    assert_eq!(a.to_rgb8().into_raw(), b.to_rgb8().into_raw());
    // the file time is untouched (the catalog keys caches on it) and the sidecar exists too
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), mtime);
    assert!(e.src.path().join("a.xmp").exists());
    // no temp files
    assert!(!std::fs::read_dir(e.src.path()).unwrap().any(|f| f
        .unwrap()
        .file_name()
        .to_string_lossy()
        .contains("ip-tmp")));
    // a second flush without changes does not touch the file again
    e.core
        .patch_photos(PatchRequest {
            ids: vec![id],
            user_rating: Some(Some(4)),
            ..Default::default()
        })
        .await
        .unwrap();
    e.core.xmp_flush().await.unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), after);
}

#[tokio::test]
async fn unreadable_sidecars_are_never_overwritten() {
    let e = env();
    photo(e.src.path(), "a.jpg", 0);
    std::fs::write(e.src.path().join("a.xmp"), "this is not xml <<<").unwrap();
    let sid = import_dir(&e, e.src.path()).await;
    e.core
        .patch_settings(json!({"xmp_mode":"sidecar"}))
        .await
        .unwrap();
    let id = by_name(&e, sid).await["a.jpg"].id;
    e.core
        .patch_photos(PatchRequest {
            ids: vec![id],
            user_rating: Some(Some(1)),
            ..Default::default()
        })
        .await
        .unwrap();
    let r = e.core.xmp_flush().await.unwrap();
    assert_eq!((r.written, r.errors.len()), (0, 1), "{r:?}");
    assert_eq!(
        sidecar_text(&e.src.path().join("a.xmp")),
        "this is not xml <<<"
    );
}

#[tokio::test]
async fn keywords_api() {
    let e = env();
    photo(e.src.path(), "a.jpg", 0);
    let sid = import_dir(&e, e.src.path()).await;
    let id = by_name(&e, sid).await["a.jpg"].id;
    assert_eq!(
        e.core
            .set_tags(vec![id], vec!["x".into(), "y".into()], vec![])
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        e.core
            .set_tags(vec![id], vec!["x".into()], vec![])
            .await
            .unwrap(),
        0,
        "no change"
    );
    assert_eq!(
        e.core
            .set_tags(vec![id], vec![], vec!["x".into()])
            .await
            .unwrap(),
        1
    );
    assert_eq!(e.core.photo_tags(id).await.unwrap(), ["y"]);
    assert!(matches!(
        e.core.set_tags(vec![id], vec![], vec![]).await,
        Err(CoreError::BadRequest(_))
    ));
    assert!(matches!(
        e.core.photo_tags(999).await,
        Err(CoreError::NotFound(_))
    ));
    // deleting the session removes the keywords with the photo
    e.core.delete_session(sid).await.unwrap();
    let n: i64 = e
        .core
        .db
        .call(|c| Ok(c.query_row("SELECT COUNT(*) FROM photo_tag", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn settings_updated_event_and_new_event_shapes() {
    let e = env();
    let mut rx = e.core.events.subscribe();
    e.core.emit_settings_updated(json!({"theme":"dark"}));
    let ev = rx.recv().await.unwrap();
    let v = serde_json::to_value(&ev).unwrap();
    assert_eq!(v["type"], "settings.updated");
    assert_eq!(v["settings"]["theme"], "dark");
    let v = serde_json::to_value(Event::XmpConflict {
        session_id: 1,
        photo_id: 12,
        sidecar: json!({"rating":3}),
        catalog: json!({"rating":4}),
    })
    .unwrap();
    assert_eq!(v["type"], "xmp.conflict");
    assert_eq!(v["photo_id"], 12);
    assert_eq!(v["session_id"], 1);
    let v = serde_json::to_value(Event::AssistantDone {
        plan_id: "p".into(),
        ok: true,
        results: vec![],
        undo: json!({"edits":[],"photos":[]}),
    })
    .unwrap();
    assert_eq!(v["type"], "assistant.done");
    assert!(v["undo"]["photos"].is_array());
    // coalescing: only the last settings frame, assistant.done frames all survive
    let mut c = crate::events::Coalescer::default();
    c.push(Event::SettingsUpdated { settings: json!(1) });
    c.push(Event::SettingsUpdated { settings: json!(2) });
    for i in 0..2 {
        c.push(Event::AssistantDone {
            plan_id: format!("p{i}"),
            ok: true,
            results: vec![],
            undo: json!({}),
        });
    }
    let out = c.drain();
    assert_eq!(out.len(), 3);
    assert!(matches!(&out[0], Event::SettingsUpdated { settings } if *settings == json!(2)));
}
