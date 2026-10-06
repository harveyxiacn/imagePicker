//! Running a plan: every step goes through the Core method behind the matching REST endpoint,
//! the state before each change is recorded so the client can offer one undo.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::keep_top::KeepScope;
use super::{PlanContext, PlanStep, StoredPlan};
use crate::catalog::{self, PhotoRef};
use crate::edit::sync::{self, SyncKind};
use crate::edit::{ops_of, store};
use crate::error::{CoreError, Result};
use crate::events::Event;
use crate::model::{ExportRequest, PatchRequest, PhotoUpdate};
use crate::Core;

/// Longest a single step may take (best take / inpainting / export run in the background).
/// Error of an `export` step without `dest`: only that step fails, the plan goes on.
pub const DEST_REQUIRED: &str = "dest_required";

const STEP_WAIT: Duration = Duration::from_secs(60 * 60);

/// Before-state of everything the plan changed: first snapshot wins.
#[derive(Default)]
struct Undo {
    photos: BTreeMap<i64, PhotoUpdate>,
    edits: BTreeMap<i64, Value>,
}

/// Export presets (`docs/api-contract-m6.md` A.1): `(long edge, crop-fit size, JPEG quality)`.
/// wechat: long edge 2048, q85; xiaohongshu: 3:4 crop-fit 1440x1920, q90; instagram: 4:5
/// crop-fit 1080x1350, q90; original: no resize.
pub fn export_preset(name: &str) -> (Option<u32>, Option<(u32, u32)>, u8) {
    match name {
        "wechat" => (Some(2048), None, 85),
        "xiaohongshu" => (None, Some((1440, 1920)), 90),
        "instagram" => (None, Some((1080, 1350)), 90),
        _ => (None, None, 95),
    }
}

struct StepOk {
    affected: usize,
    data: Option<Value>,
}

impl Core {
    /// `POST /api/assistant/execute`: runs a stored plan as a task; progress is
    /// `task.progress kind:"assistant"`, the end is `assistant.done`.
    pub async fn assistant_execute(self: &Arc<Self>, plan_id: &str) -> Result<String> {
        let plan = self.assistant.take(plan_id).ok_or_else(|| {
            CoreError::not_found(format!(
                "plan {plan_id} not found (plans expire after 30 minutes and run once)"
            ))
        })?;
        let task_id = format!("assistant-{}", self.next_task_seq());
        let total = plan.steps.len() as i64;
        self.assistant_progress(&task_id, 0, total, "running", None);
        let (core, tid, pid) = (self.clone(), task_id.clone(), plan_id.to_string());
        tokio::spawn(async move { core.run_plan(tid, pid, plan).await });
        Ok(task_id)
    }

    fn assistant_progress(
        &self,
        task_id: &str,
        done: i64,
        total: i64,
        state: &str,
        error: Option<String>,
    ) {
        self.events.emit(Event::TaskProgress {
            task_id: task_id.to_string(),
            kind: "assistant".into(),
            done,
            total,
            state: state.into(),
            error,
        });
    }

    async fn run_plan(self: Arc<Self>, task_id: String, plan_id: String, plan: StoredPlan) {
        let total = plan.steps.len() as i64;
        let mut undo = Undo::default();
        let mut results: Vec<Value> = Vec::new();
        let mut failed: Option<String> = None;
        let mut any_failed = false;
        for (i, step) in plan.steps.iter().enumerate() {
            if let Some(why) = &failed {
                results.push(json!({
                    "tool": step.tool, "ok": false, "affected": 0,
                    "error": format!("skipped: {why}")
                }));
                continue;
            }
            match self.run_step(&plan, step, &mut undo).await {
                Ok(ok) => {
                    let mut r = json!({
                        "tool": step.tool, "ok": true, "affected": ok.affected, "error": Value::Null
                    });
                    if let Some(d) = ok.data {
                        r["data"] = d;
                    }
                    if step.tool == "filter" {
                        r["args"] = step.args.clone();
                    }
                    results.push(r);
                }
                Err(e) => {
                    let msg = e.to_string();
                    results.push(json!({
                        "tool": step.tool, "ok": false, "affected": 0, "error": msg
                    }));
                    if msg == DEST_REQUIRED {
                        any_failed = true;
                    } else {
                        failed = Some(msg);
                    }
                }
            }
            self.assistant_progress(&task_id, i as i64 + 1, total, "running", None);
        }
        let undo_json = self.finish_undo(undo).await;
        let ok = failed.is_none() && !any_failed;
        self.events.emit(Event::AssistantDone {
            plan_id,
            ok,
            results,
            undo: undo_json,
        });
        let state = if ok { "done" } else { "failed" };
        self.assistant_progress(&task_id, total, total, state, failed);
    }

    /// Drops the entries nothing changed for and renders the `undo` payload.
    async fn finish_undo(&self, undo: Undo) -> Value {
        let photo_ids: Vec<i64> = undo.photos.keys().copied().collect();
        let now = self
            .db
            .call(move |c| photo_states(c, &photo_ids))
            .await
            .unwrap_or_default();
        let photos: Vec<&PhotoUpdate> = undo
            .photos
            .values()
            .filter(|b| now.get(&b.id).is_none_or(|n| n != *b))
            .collect();
        let mut edits = Vec::new();
        for (id, before) in &undo.edits {
            let cur = self
                .get_edit(*id)
                .await
                .map(|d| d.stack)
                .unwrap_or_else(|_| crate::edit::empty_stack_json());
            if cur != *before {
                edits.push(json!({"photo_id": id, "before": before}));
            }
        }
        json!({"edits": edits, "photos": photos})
    }

    async fn snapshot_photos(&self, ids: &[i64], undo: &mut Undo) -> Result<()> {
        let ids2 = ids.to_vec();
        let now = self.db.call(move |c| photo_states(c, &ids2)).await?;
        for (id, st) in now {
            undo.photos.entry(id).or_insert(st);
        }
        Ok(())
    }

    async fn snapshot_edits(&self, ids: &[i64], undo: &mut Undo) -> Result<()> {
        for id in ids {
            if undo.edits.contains_key(id) {
                continue;
            }
            let stack = self.get_edit(*id).await?.stack;
            undo.edits.insert(*id, stack);
        }
        Ok(())
    }

    async fn run_step(
        self: &Arc<Self>,
        plan: &StoredPlan,
        step: &PlanStep,
        undo: &mut Undo,
    ) -> Result<StepOk> {
        let sid = plan.session_id;
        let args = &step.args;
        let ctx: &PlanContext = &plan.ctx;
        let ids = |core: &Arc<Self>| {
            let core = core.clone();
            let sel = args["selection"].clone();
            let ctx = ctx.clone();
            async move { core.resolve_selection(sid, &sel, &ctx).await }
        };
        let done = |affected: usize| {
            Ok(StepOk {
                affected,
                data: None,
            })
        };
        match step.tool.as_str() {
            "filter" => {
                let mut q = super::tools::photo_query_of(sid, args, Some(1));
                q.cursor = None;
                let n = self.photos(q).await?.total as usize;
                done(n)
            }
            "set_rating" => {
                let ids = ids(self).await?;
                if ids.is_empty() {
                    return done(0);
                }
                self.snapshot_photos(&ids, undo).await?;
                let rating = args["rating"].as_i64();
                let n = self
                    .patch_photos(PatchRequest {
                        ids,
                        user_rating: Some(rating),
                        ..Default::default()
                    })
                    .await?;
                done(n)
            }
            "set_flag" => {
                let ids = ids(self).await?;
                if ids.is_empty() {
                    return done(0);
                }
                self.snapshot_photos(&ids, undo).await?;
                let n = self
                    .patch_photos(PatchRequest {
                        ids,
                        flag: args["flag"].as_i64(),
                        ..Default::default()
                    })
                    .await?;
                done(n)
            }
            "accept_ai" => {
                let ids = ids(self).await?;
                if ids.is_empty() {
                    return done(0);
                }
                self.snapshot_photos(&ids, undo).await?;
                let n = self.accept_ai(ids).await?;
                done(n)
            }
            "group_keep_top" | "scene_keep_top" => {
                let sel = match args.get("selection") {
                    Some(s) => Some(self.resolve_selection(sid, s, ctx).await?),
                    None => None,
                };
                let n = args["n"].as_u64().unwrap_or(1) as usize;
                let rr = args["reject_rest"].as_bool().unwrap_or(false);
                let scope = if step.tool == "group_keep_top" {
                    KeepScope::Group
                } else {
                    KeepScope::Scene
                };
                let plan = self.keep_top_plan(sid, scope, sel.as_deref(), n).await?;
                let mut touched = plan.kept.clone();
                if rr {
                    touched.extend(plan.rest.iter().copied());
                }
                self.snapshot_photos(&touched, undo).await?;
                match scope {
                    KeepScope::Group => self.group_keep_top(sid, sel.as_deref(), n, rr).await?,
                    KeepScope::Scene => self.scene_keep_top(sid, sel.as_deref(), n, rr).await?,
                };
                done(touched.len())
            }
            "apply_preset" => {
                let ids = ids(self).await?;
                let preset_id = args["preset_id"].as_str().unwrap_or_default().to_string();
                let n = self.apply_preset_to(&ids, &preset_id, undo).await?;
                done(n)
            }
            "auto_adjust" => {
                let ids = ids(self).await?;
                let mode = match args["mode"].as_str() {
                    Some("portrait") => ip_render::AutoMode::Portrait,
                    Some("landscape") => ip_render::AutoMode::Landscape,
                    _ => ip_render::AutoMode::Auto,
                };
                let n = self.auto_adjust_save(&ids, mode, undo).await?;
                done(n)
            }
            "apply_profiles" => {
                let ids = ids(self).await?;
                self.snapshot_edits(&ids, undo).await?;
                let n = self.apply_profiles(ids).await?;
                done(n)
            }
            "besttake_auto" => {
                let ids = ids(self).await?;
                let n = self.besttake_selected(&ids, undo).await?;
                done(n)
            }
            "remove_bystanders" => {
                let ids = ids(self).await?;
                let n = self.remove_bystanders_selected(&ids, undo).await?;
                done(n)
            }
            "export" => {
                let ids = ids(self).await?;
                if ids.is_empty() {
                    return done(0);
                }
                let dest = args["dest"]
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| CoreError::bad_request(DEST_REQUIRED))?;
                let (long_edge, fit, quality) =
                    export_preset(args["preset"].as_str().unwrap_or("original"));
                let n = ids.len();
                let mut rx = self.events.subscribe();
                let task = self
                    .export_fit(ExportRequest {
                        ids,
                        dest,
                        folders: None,
                        long_edge,
                        quality,
                        name_template: "{name}".into(),
                        apply_edits: true,
                        upscale: None,
                        strip_gps: false,
                    }, fit)
                    .await?;
                wait_task(&mut rx, &task).await?;
                done(n)
            }
            "describe" => {
                let id = args["photo_id"].as_i64().unwrap_or_default();
                let d = self.assistant_describe(id).await?;
                Ok(StepOk {
                    affected: 1,
                    data: Some(json!({"caption": d.caption, "keywords": d.keywords})),
                })
            }
            "suggest_edits" => {
                let id = args["photo_id"].as_i64().unwrap_or_default();
                let s = self.assistant_suggest(id).await?;
                Ok(StepOk {
                    affected: 1,
                    data: Some(json!({"problems": s.problems, "adjust": s.adjust, "reason": s.reason})),
                })
            }
            other => Err(CoreError::bad_request(format!("unknown tool {other}"))),
        }
    }

    /// Replaces global / local / LUT / output-sharpen of every photo with the preset's.
    async fn apply_preset_to(
        self: &Arc<Self>,
        ids: &[i64],
        preset_id: &str,
        undo: &mut Undo,
    ) -> Result<usize> {
        let preset = self
            .presets()
            .await?
            .into_iter()
            .find(|p| p.id == preset_id)
            .ok_or_else(|| CoreError::not_found(format!("preset {preset_id} not found")))?;
        let preset_ops = ops_of(&preset.stack);
        let include = [
            SyncKind::Global,
            SyncKind::Local,
            SyncKind::Lut,
            SyncKind::OutputSharpen,
        ];
        let ids2 = ids.to_vec();
        let work = self
            .db
            .call(move |c| {
                let mut out = Vec::new();
                for r in catalog::photo_refs(c, &ids2)? {
                    let cur = store::current(c, r.id)?
                        .map(|s| ops_of(&s.stack))
                        .unwrap_or_default();
                    out.push((r, cur));
                }
                Ok(out)
            })
            .await?;
        let mut writes: Vec<(PhotoRef, Vec<Value>)> = Vec::new();
        for (r, cur) in work {
            let merged = sync::merge_ops(&cur, &preset_ops, &include);
            if merged != cur {
                writes.push((r, merged));
            }
        }
        let touched: Vec<i64> = writes.iter().map(|(r, _)| r.id).collect();
        self.snapshot_edits(&touched, undo).await?;
        let n = writes.len();
        self.persist_stacks(writes).await?;
        Ok(n)
    }

    /// `auto_adjust` per photo: the suggested sliders replace the global op's, and are saved.
    async fn auto_adjust_save(
        self: &Arc<Self>,
        ids: &[i64],
        mode: ip_render::AutoMode,
        undo: &mut Undo,
    ) -> Result<usize> {
        let mut writes: Vec<(PhotoRef, Vec<Value>)> = Vec::new();
        for id in ids {
            let id = *id;
            let adj = self.auto_adjust(id, mode).await?;
            let mut a = serde_json::to_value(&adj)?;
            if let Value::Object(m) = &mut a {
                m.entry("source").or_insert(json!("ai_auto@1"));
                m.insert("type".into(), json!("global"));
            }
            let (r, cur) = self
                .db
                .call(move |c| {
                    let r = catalog::photo_ref(c, id)?;
                    let cur = store::current(c, id)?
                        .map(|s| ops_of(&s.stack))
                        .unwrap_or_default();
                    Ok((r, cur))
                })
                .await?;
            let mut ops = cur.clone();
            match ops.iter().position(|o| sync::op_type(o) == Some("global")) {
                Some(i) => {
                    if let (Some(dst), Some(src)) = (ops[i].as_object_mut(), a.as_object()) {
                        for (k, v) in src {
                            dst.insert(k.clone(), v.clone());
                        }
                    }
                }
                None => ops.push(a),
            }
            ops.sort_by_key(sync::rank);
            if ops != cur {
                writes.push((r, ops));
            }
        }
        let touched: Vec<i64> = writes.iter().map(|(r, _)| r.id).collect();
        self.snapshot_edits(&touched, undo).await?;
        let n = writes.len();
        self.persist_stacks(writes).await?;
        Ok(n)
    }

    /// Best take for the burst groups of the selected photos (waits for each `besttake.done`).
    async fn besttake_selected(self: &Arc<Self>, ids: &[i64], undo: &mut Undo) -> Result<usize> {
        let ids2 = ids.to_vec();
        let bursts: Vec<i64> = self
            .db
            .call(move |c| {
                let mut out = Vec::new();
                for r in ids2.chunks(500) {
                    let ph = vec!["?"; r.len()].join(",");
                    let mut st = c.prepare(&format!(
                        "SELECT DISTINCT burst_id FROM photo WHERE burst_id IS NOT NULL AND id IN ({ph})"
                    ))?;
                    let v = st
                        .query_map(rusqlite::params_from_iter(r.iter()), |x| x.get::<_, i64>(0))?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    out.extend(v);
                }
                out.sort_unstable();
                out.dedup();
                Ok(out)
            })
            .await?;
        let mut n = 0;
        for b in bursts {
            let plan = self.besttake_plan(b).await?;
            if plan.base_photo_id == 0 || crate::generate::auto_choices(&plan).is_empty() {
                continue;
            }
            self.snapshot_edits(&[plan.base_photo_id], undo).await?;
            let mut rx = self.events.subscribe();
            self.besttake_auto(b).await?;
            let base = plan.base_photo_id;
            let wait = async {
                loop {
                    match rx.recv().await {
                        Ok(Event::BestTakeDone { photo_id, results }) if photo_id == base => {
                            return Ok(results)
                        }
                        Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(e) => {
                            return Err(CoreError::Internal(anyhow::anyhow!(
                                "event stream closed: {e}"
                            )))
                        }
                    }
                }
            };
            let results = tokio::time::timeout(STEP_WAIT, wait)
                .await
                .map_err(|_| CoreError::WorkerTimeout("best take took too long".into()))??;
            if results.iter().any(|r| r.ok) {
                n += 1;
            }
        }
        Ok(n)
    }

    /// Inpaints the bystanders of every selected photo that has some.
    async fn remove_bystanders_selected(
        self: &Arc<Self>,
        ids: &[i64],
        undo: &mut Undo,
    ) -> Result<usize> {
        let mut n = 0;
        for id in ids {
            let id = *id;
            if self.bystanders(id).await?.is_empty() {
                continue;
            }
            self.snapshot_edits(&[id], undo).await?;
            let mut rx = self.events.subscribe();
            self.inpaint_start(
                id,
                crate::generate::InpaintBody {
                    bystanders: true,
                    ..Default::default()
                },
            )
            .await?;
            let wait = async {
                loop {
                    match rx.recv().await {
                        Ok(Event::InpaintDone {
                            photo_id,
                            ok,
                            reason,
                        }) if photo_id == id => return Ok((ok, reason)),
                        Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(e) => {
                            return Err(CoreError::Internal(anyhow::anyhow!(
                                "event stream closed: {e}"
                            )))
                        }
                    }
                }
            };
            let (ok, reason) = tokio::time::timeout(STEP_WAIT, wait)
                .await
                .map_err(|_| CoreError::WorkerTimeout("inpainting took too long".into()))??;
            if !ok {
                return Err(CoreError::Internal(anyhow::anyhow!(
                    "removing bystanders from photo {id} failed: {}",
                    reason.unwrap_or_default()
                )));
            }
            n += 1;
        }
        Ok(n)
    }
}

/// Waits until `task_id` reports `done` / `failed` (progress events of one task).
async fn wait_task(
    rx: &mut tokio::sync::broadcast::Receiver<Event>,
    task_id: &str,
) -> Result<()> {
    let wait = async {
        loop {
            match rx.recv().await {
                Ok(Event::TaskProgress {
                    task_id: t,
                    state,
                    error,
                    ..
                }) if t == task_id && (state == "done" || state == "failed") => {
                    return if state == "done" {
                        Ok(())
                    } else {
                        Err(CoreError::Internal(anyhow::anyhow!(
                            "task {t} failed: {}",
                            error.unwrap_or_default()
                        )))
                    }
                }
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(e) => {
                    return Err(CoreError::Internal(anyhow::anyhow!(
                        "event stream closed: {e}"
                    )))
                }
            }
        }
    };
    tokio::time::timeout(STEP_WAIT, wait)
        .await
        .map_err(|_| CoreError::WorkerTimeout("the task took too long".into()))?
}

fn photo_states(
    c: &rusqlite::Connection,
    ids: &[i64],
) -> Result<std::collections::HashMap<i64, PhotoUpdate>> {
    let mut out = std::collections::HashMap::new();
    for chunk in ids.chunks(500) {
        let ph = vec!["?"; chunk.len()].join(",");
        let mut st = c.prepare(&format!(
            "SELECT id, user_rating, COALESCE(flag,0), color_label FROM photo WHERE id IN ({ph})"
        ))?;
        let rows = st
            .query_map(rusqlite::params_from_iter(chunk.iter()), |r| {
                Ok(PhotoUpdate {
                    id: r.get(0)?,
                    user_rating: r.get(1)?,
                    flag: r.get(2)?,
                    color_label: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        out.extend(rows.into_iter().map(|u| (u.id, u)));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_presets_are_distinct() {
        assert_eq!(export_preset("xiaohongshu"), (None, Some((1440, 1920)), 90));
        assert_eq!(export_preset("instagram"), (None, Some((1080, 1350)), 90));
        assert_eq!(export_preset("wechat"), (Some(2048), None, 85));
        assert_eq!(export_preset("original"), (None, None, 95));
    }
}
