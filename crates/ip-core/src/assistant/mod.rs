//! M6 AI assistant (`docs/api-contract-m6.md` section A): plans from natural language, either by
//! the rules engine or by a language model behind strict validation, executed with the same
//! Core methods the REST endpoints use.

pub mod exec;
pub mod keep_top;
pub mod rules;
pub mod tools;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ip_worker_client::{
    LlmPlanRequest, VlmDescribeRequest, VlmSuggestRequest, WorkerError, WorkerState,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use self::rules::{Call, Locale, PresetName, RulesCtx};
use self::tools::{llm_tools, validate_call, MAX_STEPS};
use crate::analysis::map_worker_err;
use crate::catalog;
use crate::error::{CoreError, Result};
use crate::model::PhotoQuery;
use crate::Core;

pub use keep_top::{KeepScope, KeepTop};

/// Plans are kept in memory this long.
pub const PLAN_TTL: Duration = Duration::from_secs(30 * 60);
const STATUS_TTL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PlanContext {
    /// The grid's current `/api/photos` filter (query string without session/paging).
    pub filter: Option<String>,
    /// Photos selected in the UI.
    pub selection: Vec<i64>,
    pub current_photo_id: Option<i64>,
    pub locale: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlanRequest {
    pub session_id: i64,
    pub message: String,
    #[serde(default)]
    pub context: PlanContext,
    /// `auto` | `rules` | `llm`; default from the settings.
    #[serde(default)]
    pub engine: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PlanStep {
    pub tool: String,
    pub args: Value,
    pub summary: String,
    /// Photos the step touches.
    pub affects: usize,
    pub destructive: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanOut {
    pub plan_id: String,
    pub reply: String,
    pub steps: Vec<PlanStep>,
    pub needs_confirmation: bool,
    pub engine: String,
    pub unsupported: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct StoredPlan {
    pub session_id: i64,
    pub steps: Vec<PlanStep>,
    pub ctx: PlanContext,
    pub created: Instant,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LlmInfo {
    pub llm_model: Option<String>,
    pub vlm_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AssistantStatus {
    pub engine: String,
    pub llm_model: Option<String>,
    pub vlm_model: Option<String>,
    pub llm_available: bool,
    pub vlm_available: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DescribeOut {
    pub caption: String,
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SuggestOut {
    pub problems: Vec<String>,
    pub adjust: Value,
    pub reason: String,
}

#[derive(Default)]
pub struct AssistantState {
    plans: Mutex<HashMap<String, StoredPlan>>,
    info: Mutex<Option<(Instant, LlmInfo)>>,
}

impl AssistantState {
    fn purge(&self, now: Instant) {
        self.plans
            .lock()
            .unwrap()
            .retain(|_, p| now.duration_since(p.created) < PLAN_TTL);
    }

    pub(crate) fn insert(&self, id: String, plan: StoredPlan) {
        self.purge(Instant::now());
        self.plans.lock().unwrap().insert(id, plan);
    }

    /// Removes and returns a live plan (a plan runs once).
    pub(crate) fn take(&self, id: &str) -> Option<StoredPlan> {
        self.purge(Instant::now());
        self.plans.lock().unwrap().remove(id)
    }

    pub fn plan_count(&self) -> usize {
        self.purge(Instant::now());
        self.plans.lock().unwrap().len()
    }

    /// Test hook: pretends the plan was created `age` ago.
    pub fn age_plan(&self, id: &str, age: Duration) {
        if let Some(p) = self.plans.lock().unwrap().get_mut(id) {
            p.created = Instant::now().checked_sub(age).unwrap_or_else(Instant::now);
        }
    }

    pub fn invalidate_info(&self) {
        *self.info.lock().unwrap() = None;
    }
}

fn detect_locale(msg: &str, hint: Option<&str>, fallback: &str) -> Locale {
    if let Some(l) = hint.and_then(Locale::parse) {
        return l;
    }
    if msg.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)) {
        return Locale::Zh;
    }
    if msg.chars().any(|c| c.is_ascii_alphabetic()) {
        return Locale::En;
    }
    Locale::parse(fallback).unwrap_or(Locale::Zh)
}

impl Core {
    // ------------------------------------------------------------ status

    /// Installed language / vision-language models, from `models.list`. Without `start` the
    /// worker is not launched for this: a cached answer (30 s) or nothing is used.
    pub(crate) async fn llm_info(&self, start: bool) -> Result<LlmInfo> {
        if let Some((at, info)) = self.assistant.info.lock().unwrap().clone() {
            if at.elapsed() < STATUS_TTL {
                return Ok(info);
            }
        }
        let running = matches!(
            self.worker.status().state,
            WorkerState::Ready | WorkerState::Busy
        );
        if !start && !running {
            return Ok(LlmInfo::default());
        }
        let listing = self.worker.models_list().await.map_err(map_worker_err)?;
        let find = |kind: &str| {
            listing
                .models
                .iter()
                .find(|m| m.installed && m.task.iter().any(|t| t.eq_ignore_ascii_case(kind)))
                .map(|m| m.id.clone())
        };
        let info = LlmInfo {
            llm_model: find("llm"),
            vlm_model: find("vlm"),
        };
        *self.assistant.info.lock().unwrap() = Some((Instant::now(), info.clone()));
        Ok(info)
    }

    /// `GET /api/assistant/status`.
    pub async fn assistant_status(&self, probe: bool) -> AssistantStatus {
        let info = self.llm_info(probe).await.unwrap_or_default();
        let llm_available = info.llm_model.is_some();
        let configured = self.settings().assistant.engine.clone();
        let engine = if llm_available && configured != "rules" {
            "llm"
        } else {
            "rules"
        };
        AssistantStatus {
            engine: engine.into(),
            vlm_available: info.vlm_model.is_some(),
            llm_model: info.llm_model,
            vlm_model: info.vlm_model,
            llm_available,
        }
    }

    // ------------------------------------------------------------ plan

    async fn rules_context(
        &self,
        session_id: i64,
    ) -> Result<(Vec<(i64, String)>, Vec<PresetName>)> {
        let people = if self.settings().faces.enabled {
            self.people_with(Some(session_id), true)
                .await?
                .into_iter()
                .filter(|p| !p.hidden)
                .filter_map(|p| p.name.filter(|n| !n.trim().is_empty()).map(|n| (p.id, n)))
                .collect()
        } else {
            Vec::new()
        };
        let mut presets = rules::builtin_preset_names();
        for p in self.presets().await? {
            if !p.builtin {
                presets.push(PresetName {
                    id: p.id.clone(),
                    names: vec![p.name.clone()],
                });
            }
        }
        Ok((people, presets))
    }

    /// Calls the language model; `Err` carries the reason it could not be used.
    async fn llm_calls(
        &self,
        req: &PlanRequest,
        locale: Locale,
        people: &[(i64, String)],
        presets: &[PresetName],
        start_worker: bool,
    ) -> Result<(String, Vec<Call>)> {
        let info = self.llm_info(start_worker).await?;
        if info.llm_model.is_none() {
            return Err(CoreError::Conflict(
                "no language model is installed (assistant engine falls back to rules)".into(),
            ));
        }
        let ctx = json!({
            "filter": req.context.filter,
            "selection_count": req.context.selection.len(),
            "current_photo_id": req.context.current_photo_id,
            "people": people.iter().map(|(id, n)| json!({"id": id, "name": n})).collect::<Vec<_>>(),
            "presets": presets.iter().map(|p| json!({"id": p.id, "names": p.names})).collect::<Vec<_>>(),
        });
        let resp = self
            .worker
            .llm_plan(&LlmPlanRequest {
                message: req.message.clone(),
                tools: llm_tools(),
                context: ctx,
                locale: locale.code().to_string(),
                allow_download: false,
            })
            .await
            .map_err(map_worker_err)?;
        if resp.calls.len() > MAX_STEPS {
            return Err(CoreError::Unprocessable(format!(
                "the model proposed {} steps (at most {MAX_STEPS})",
                resp.calls.len()
            )));
        }
        let mut calls = Vec::new();
        for c in resp.calls {
            let args = validate_call(&c.tool, &c.args).map_err(|e| {
                CoreError::Unprocessable(format!("invalid model output for {}: {e}", c.tool))
            })?;
            calls.push(Call { tool: c.tool, args });
        }
        Ok((resp.reply, calls))
    }

    /// `POST /api/assistant/plan`.
    pub async fn assistant_plan(self: &Arc<Self>, req: PlanRequest) -> Result<PlanOut> {
        let sid = req.session_id;
        self.session(sid).await?;
        if req.message.trim().is_empty() {
            return Err(CoreError::bad_request("message must not be empty"));
        }
        if req.message.chars().count() > 2000 {
            return Err(CoreError::bad_request(
                "message is too long (2000 characters)",
            ));
        }
        let settings = self.settings();
        let locale = detect_locale(
            &req.message,
            req.context.locale.as_deref(),
            &settings.language,
        );
        let engine_req = req
            .engine
            .clone()
            .unwrap_or_else(|| settings.assistant.engine.clone());
        if !matches!(engine_req.as_str(), "auto" | "rules" | "llm") {
            return Err(CoreError::bad_request("engine must be auto, rules or llm"));
        }
        let (people, presets) = self.rules_context(sid).await?;

        let mut engine = "rules";
        let mut llm_reply: Option<String> = None;
        let mut calls: Option<Vec<Call>> = None;
        let mut fallback_note: Option<String> = None;
        if engine_req != "rules" {
            // `auto` only uses a model that is already loaded; `llm` starts the worker
            match self
                .llm_calls(&req, locale, &people, &presets, engine_req == "llm")
                .await
            {
                Ok((reply, c)) if !c.is_empty() => {
                    engine = "llm";
                    llm_reply = Some(reply);
                    calls = Some(c);
                }
                Ok(_) => {
                    fallback_note = Some("the model returned no steps".into());
                }
                Err(e) => {
                    if engine_req == "llm"
                        && matches!(
                            e,
                            CoreError::ModelsMissing(_)
                                | CoreError::WorkerUnavailable(_)
                                | CoreError::WorkerTimeout(_)
                        )
                    {
                        return Err(e);
                    }
                    tracing::info!(error = %e, "assistant: using the rules engine");
                    fallback_note = Some(e.to_string());
                }
            }
        }
        let _ = fallback_note;
        let names = Names {
            people: &people,
            presets: &presets,
        };
        let mut llm_calls = calls;
        let (steps, hints, unsupported) = loop {
            let (calls, hints, mut unsupported) = match llm_calls.take() {
                Some(c) => (c, Vec::new(), None),
                None => {
                    let parsed = rules::parse(
                        &req.message,
                        &RulesCtx {
                            people: &people,
                            presets: &presets,
                            selection: &req.context.selection,
                            current_photo_id: req.context.current_photo_id,
                            locale,
                        },
                    );
                    (parsed.calls, parsed.hints, parsed.unsupported)
                }
            };
            let mut steps = Vec::new();
            if unsupported.is_none() {
                for c in calls {
                    match self.build_step(sid, &req.context, &c, locale, &names).await {
                        Ok(s) => steps.push(s),
                        Err(msg) => {
                            unsupported = Some(msg);
                            steps.clear();
                            break;
                        }
                    }
                }
                if steps.is_empty() && unsupported.is_none() {
                    unsupported = Some(rules::unsupported_help(locale));
                }
            }
            if unsupported.is_some() && engine == "llm" {
                // the model referred to something that does not exist: use the rules instead
                tracing::info!(reason = ?unsupported, "assistant: model plan rejected; using the rules");
                engine = "rules";
                llm_reply = None;
                continue;
            }
            break (steps, hints, unsupported);
        };
        let needs_confirmation = steps.iter().any(|s| s.destructive);
        let reply = match (&unsupported, &llm_reply) {
            (Some(u), _) => u.clone(),
            (None, Some(r)) if !r.trim().is_empty() => r.trim().to_string(),
            _ => reply_for(&steps, &hints, locale, needs_confirmation),
        };
        let plan_id = self.new_plan_id();
        if unsupported.is_none() {
            self.assistant.insert(
                plan_id.clone(),
                StoredPlan {
                    session_id: sid,
                    steps: steps.clone(),
                    ctx: req.context.clone(),
                    created: Instant::now(),
                },
            );
        }
        Ok(PlanOut {
            plan_id,
            reply,
            steps,
            needs_confirmation,
            engine: engine.into(),
            unsupported,
        })
    }

    fn new_plan_id(&self) -> String {
        let seq = self.next_task_seq();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let h = blake3::hash(format!("{seq}:{now}:{}", std::process::id()).as_bytes());
        format!("plan-{seq}-{}", &h.to_hex()[..8])
    }

    /// Validates a call and computes what the UI shows: summary, affected count, destructive.
    async fn build_step(
        &self,
        session_id: i64,
        ctx: &PlanContext,
        call: &Call,
        locale: Locale,
        names: &Names<'_>,
    ) -> std::result::Result<PlanStep, String> {
        let args = validate_call(&call.tool, &call.args)?;
        let tool = call.tool.as_str();
        // references must exist
        if tool == "apply_preset" {
            let id = args["preset_id"].as_str().unwrap_or_default();
            let known = names.presets.iter().any(|p| p.id == id);
            if !known {
                return Err(match locale {
                    Locale::Zh => format!("没有这个预设：{id}"),
                    Locale::En => format!("Unknown preset: {id}"),
                });
            }
        }
        if tool == "filter" {
            for key in ["persons", "exclude_persons"] {
                for id in args
                    .get(key)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_i64)
                {
                    if !names.people.iter().any(|(p, _)| *p == id) {
                        return Err(match locale {
                            Locale::Zh => format!("没有 id 为 {id} 的人物。"),
                            Locale::En => format!("There is no person with id {id}."),
                        });
                    }
                }
            }
        }
        let affects = self
            .affects_of(session_id, ctx, tool, &args)
            .await
            .map_err(|e| e.to_string())?;
        let destructive = is_destructive(tool, &args, affects);
        Ok(PlanStep {
            tool: tool.to_string(),
            summary: summarize(tool, &args, affects, locale, names),
            args,
            affects,
            destructive,
        })
    }

    async fn affects_of(
        &self,
        session_id: i64,
        ctx: &PlanContext,
        tool: &str,
        args: &Value,
    ) -> Result<usize> {
        match tool {
            "filter" => {
                let mut q = tools::photo_query_of(session_id, args, Some(1));
                q.cursor = None;
                Ok(self.photos(q).await?.total as usize)
            }
            "describe" | "suggest_edits" => Ok(1),
            "group_keep_top" | "scene_keep_top" => {
                let sel = match args.get("selection") {
                    Some(s) => Some(self.resolve_selection(session_id, s, ctx).await?),
                    None => None,
                };
                let scope = if tool == "group_keep_top" {
                    KeepScope::Group
                } else {
                    KeepScope::Scene
                };
                let n = args["n"].as_u64().unwrap_or(1) as usize;
                let rr = args["reject_rest"].as_bool().unwrap_or(false);
                let plan = self
                    .keep_top_plan(session_id, scope, sel.as_deref(), n)
                    .await?;
                Ok(plan.kept.len() + if rr { plan.rest.len() } else { 0 })
            }
            _ => Ok(self
                .resolve_selection(session_id, &args["selection"], ctx)
                .await?
                .len()),
        }
    }

    /// Photo ids of a selection, restricted to the session.
    pub(crate) async fn resolve_selection(
        &self,
        session_id: i64,
        sel: &Value,
        ctx: &PlanContext,
    ) -> Result<Vec<i64>> {
        let query = match sel {
            Value::String(s) if s == "current_filter" => {
                Some(ctx.filter.clone().unwrap_or_default())
            }
            Value::Object(o) => o
                .get("query")
                .map(|q| q.as_str().unwrap_or_default().to_string()),
            _ => None,
        };
        if let Some(q) = query {
            let base = tools::parse_photo_query(session_id, &q)
                .map_err(|e| CoreError::bad_request(format!("selection: {e}")))?;
            return self.ids_of_query(base).await;
        }
        let ids: Vec<i64> = sel
            .get("ids")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_i64).collect())
            .ok_or_else(|| CoreError::bad_request("invalid selection"))?;
        self.db
            .call(move |c| {
                let mut out = Vec::new();
                let mut seen = std::collections::HashSet::new();
                let mut st =
                    c.prepare("SELECT 1 FROM session_photo WHERE session_id=?1 AND photo_id=?2")?;
                for id in ids {
                    if seen.insert(id) && st.exists(rusqlite::params![session_id, id])? {
                        out.push(id);
                    }
                }
                Ok(out)
            })
            .await
    }

    async fn ids_of_query(&self, mut q: PhotoQuery) -> Result<Vec<i64>> {
        let want = q.limit;
        let mut out = Vec::new();
        loop {
            q.limit = Some(want.unwrap_or(5000));
            let page = self.photos(q.clone()).await?;
            out.extend(page.photos.iter().map(|p| p.id));
            match page.next_cursor {
                Some(c) if want.is_none() && out.len() < 100_000 => q.cursor = Some(c),
                _ => break,
            }
        }
        Ok(out)
    }

    // ------------------------------------------------------------ VLM

    fn vlm_photo(r: &catalog::PhotoRef) -> ip_worker_client::MaskPhoto {
        ip_worker_client::MaskPhoto {
            photo_id: r.id,
            path: r.path.to_string_lossy().into_owned(),
            orientation: r.orientation,
        }
    }

    /// 409 `models_missing` / 503 when no VLM can answer.
    async fn require_vlm(&self) -> Result<()> {
        let listing = self.worker.models_list().await.map_err(map_worker_err)?;
        let vlms: Vec<_> = listing
            .models
            .iter()
            .filter(|m| m.task.iter().any(|t| t.eq_ignore_ascii_case("vlm")))
            .collect();
        if vlms.is_empty() {
            return Err(CoreError::WorkerUnavailable(
                "this AI worker has no vision-language model".into(),
            ));
        }
        if !vlms.iter().any(|m| m.installed) {
            return Err(CoreError::ModelsMissing(
                vlms.iter().map(|m| m.id.clone()).collect(),
            ));
        }
        Ok(())
    }

    fn vlm_err(e: WorkerError) -> CoreError {
        match &e {
            WorkerError::Rpc { code, .. } if *code == ip_worker_client::CODE_METHOD_NOT_FOUND => {
                CoreError::WorkerUnavailable("this AI worker does not support the VLM".into())
            }
            _ => map_worker_err(e),
        }
    }

    /// `POST /api/assistant/describe`.
    pub async fn assistant_describe(&self, photo_id: i64) -> Result<DescribeOut> {
        let r = self
            .db
            .call(move |c| catalog::photo_ref(c, photo_id))
            .await?;
        self.require_vlm().await?;
        let locale = detect_locale("", None, &self.settings().language);
        let out = self
            .worker
            .vlm_describe(&VlmDescribeRequest {
                photo: Self::vlm_photo(&r),
                locale: locale.code().to_string(),
                allow_download: false,
            })
            .await
            .map_err(Self::vlm_err)?;
        Ok(DescribeOut {
            caption: out.caption,
            keywords: crate::xmp::normalize_tags(out.keywords),
        })
    }

    /// `POST /api/assistant/suggest`: VLM edit suggestion; the `adjust` is validated and
    /// clamped, nothing is saved.
    pub async fn assistant_suggest(&self, photo_id: i64) -> Result<SuggestOut> {
        let r = self
            .db
            .call(move |c| catalog::photo_ref(c, photo_id))
            .await?;
        self.require_vlm().await?;
        let photo = self.photo(photo_id).await?;
        let ctx = json!({
            "scene_type": photo.scene_type,
            "scores": {"ai_score": photo.ai_score, "ai_rating": photo.ai_rating},
            "histogram": Value::Null,
        });
        let out = self
            .worker
            .vlm_suggest(&VlmSuggestRequest {
                photo: Self::vlm_photo(&r),
                context: ctx,
                allow_download: false,
            })
            .await
            .map_err(Self::vlm_err)?;
        let adjust = clean_adjust(&out.adjust)?;
        Ok(SuggestOut {
            problems: out.problems.into_iter().take(12).collect(),
            adjust,
            reason: out.reason,
        })
    }
}

/// A model-suggested `Adjust`: parsed strictly, range-checked, serialised canonically.
fn clean_adjust(v: &Value) -> Result<Value> {
    let adj: ip_render::Adjust = serde_json::from_value(v.clone())
        .map_err(|e| CoreError::Unprocessable(format!("the model's adjustment is invalid: {e}")))?;
    crate::edit::check_adjust_public(&adj)?;
    let mut out = serde_json::to_value(&adj)?;
    if let Value::Object(m) = &mut out {
        m.insert("source".into(), json!("vlm_suggest@1"));
    }
    Ok(out)
}

pub(crate) struct Names<'a> {
    pub people: &'a [(i64, String)],
    pub presets: &'a [PresetName],
}

fn is_destructive(tool: &str, args: &Value, affects: usize) -> bool {
    match tool {
        "set_flag" => args["flag"].as_i64() == Some(-1),
        "set_rating" | "accept_ai" => affects > 1,
        // batch edits / generative changes: undoable, but confirm before touching many photos
        "auto_adjust" | "apply_preset" | "apply_profiles" | "besttake_auto"
        | "remove_bystanders" => affects > 1,
        "group_keep_top" | "scene_keep_top" => args["reject_rest"].as_bool() == Some(true),
        "export" => true,
        _ => false,
    }
}

fn preset_label(names: &Names, id: &str) -> String {
    names
        .presets
        .iter()
        .find(|p| p.id == id)
        .and_then(|p| p.names.first().cloned())
        .unwrap_or_else(|| id.to_string())
}

fn person_labels(names: &Names, ids: &[i64]) -> String {
    ids.iter()
        .map(|id| {
            names
                .people
                .iter()
                .find(|(p, _)| p == id)
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| format!("#{id}"))
        })
        .collect::<Vec<_>>()
        .join("、")
}

fn issue_label(k: &str, l: Locale) -> &'static str {
    match (k, l) {
        ("closed_eyes", Locale::Zh) => "闭眼",
        ("blurry", Locale::Zh) => "模糊",
        ("overexposed", Locale::Zh) => "过曝",
        ("underexposed", Locale::Zh) => "欠曝",
        ("noisy", Locale::Zh) => "噪点",
        ("tilted", Locale::Zh) => "倾斜",
        ("closed_eyes", Locale::En) => "closed eyes",
        ("blurry", Locale::En) => "blurry",
        ("overexposed", Locale::En) => "overexposed",
        ("underexposed", Locale::En) => "underexposed",
        ("noisy", Locale::En) => "noisy",
        ("tilted", Locale::En) => "tilted",
        _ => "?",
    }
}

fn filter_summary(args: &Value, l: Locale, names: &Names) -> String {
    let mut parts: Vec<String> = Vec::new();
    let zh = l == Locale::Zh;
    if let Some(p) = args.get("persons").and_then(Value::as_array) {
        let ids: Vec<i64> = p.iter().filter_map(Value::as_i64).collect();
        parts.push(person_labels(names, &ids));
    }
    if let Some(r) = args.get("rating_gte").and_then(Value::as_i64) {
        parts.push(if zh {
            format!("{r}星以上")
        } else {
            format!("{r}+ stars")
        });
    }
    if let Some(r) = args.get("ai_rating_gte").and_then(Value::as_f64) {
        parts.push(if zh {
            format!("AI评分≥{r}")
        } else {
            format!("AI rating ≥ {r}")
        });
    }
    if let Some(f) = args.get("flag").and_then(Value::as_str) {
        parts.push(
            match (f, zh) {
                ("picked", true) => "已选用",
                ("rejected", true) => "已淘汰",
                ("unflagged", true) => "未标记",
                ("not_rejected", true) => "未淘汰",
                ("picked", false) => "picked",
                ("rejected", false) => "rejected",
                ("unflagged", false) => "unflagged",
                _ => "not rejected",
            }
            .to_string(),
        );
    }
    if let Some(a) = args.get("issues_any").and_then(Value::as_array) {
        let v: Vec<&str> = a
            .iter()
            .filter_map(Value::as_str)
            .map(|k| issue_label(k, l))
            .collect();
        parts.push(v.join(if zh { "/" } else { " / " }));
    }
    if args.get("issues_none").and_then(Value::as_bool) == Some(true) {
        parts.push(if zh { "无问题" } else { "no issues" }.to_string());
    }
    if let Some(s) = args.get("scene_type").and_then(Value::as_str) {
        parts.push(s.to_string());
    }
    if let Some(c) = args.get("color_label").and_then(Value::as_str) {
        parts.push(if zh {
            format!("{c}标签")
        } else {
            format!("{c} label")
        });
    }
    if args.get("burst_best_only").and_then(Value::as_bool) == Some(true) {
        parts.push(
            if zh {
                "每组最佳"
            } else {
                "best of each burst"
            }
            .to_string(),
        );
    }
    if let Some(e) = args.get("has_edits").and_then(Value::as_bool) {
        parts.push(
            match (e, zh) {
                (true, true) => "已修图",
                (false, true) => "未修图",
                (true, false) => "edited",
                (false, false) => "unedited",
            }
            .to_string(),
        );
    }
    if parts.is_empty() {
        if zh {
            "显示全部照片".to_string()
        } else {
            "Show all photos".to_string()
        }
    } else if zh {
        format!("筛选：{}", parts.join("，"))
    } else {
        format!("Filter: {}", parts.join(", "))
    }
}

fn summarize(tool: &str, args: &Value, affects: usize, l: Locale, names: &Names) -> String {
    let zh = l == Locale::Zh;
    let n = affects;
    match tool {
        "filter" => filter_summary(args, l, names),
        "set_rating" => match args["rating"].as_i64() {
            Some(r) if zh => format!("给 {n} 张照片评 {r} 星"),
            Some(r) => format!("Rate {n} photo(s) {r} star(s)"),
            None if zh => format!("清除 {n} 张照片的评分"),
            None => format!("Clear the rating of {n} photo(s)"),
        },
        "set_flag" => match (args["flag"].as_i64(), zh) {
            (Some(-1), true) => format!("淘汰 {n} 张照片"),
            (Some(-1), false) => format!("Reject {n} photo(s)"),
            (Some(1), true) => format!("把 {n} 张照片标为选用"),
            (Some(1), false) => format!("Pick {n} photo(s)"),
            (_, true) => format!("清除 {n} 张照片的旗标"),
            (_, false) => format!("Clear the flag of {n} photo(s)"),
        },
        "accept_ai" => {
            if zh {
                format!("把 {n} 张照片的 AI 评分接受为星级")
            } else {
                format!("Accept the AI rating of {n} photo(s)")
            }
        }
        "group_keep_top" | "scene_keep_top" => {
            let k = args["n"].as_i64().unwrap_or(1);
            let rr = args["reject_rest"].as_bool() == Some(true);
            let (zs, es) = if tool == "group_keep_top" {
                ("每个连拍组", "each burst")
            } else {
                ("每个场景", "each scene")
            };
            match (zh, rr) {
                (true, true) => format!("{zs}保留最好的 {k} 张，其余淘汰（涉及 {n} 张）"),
                (true, false) => format!("{zs}把最好的 {k} 张标为选用（涉及 {n} 张）"),
                (false, true) => {
                    format!("In {es} keep the best {k} and reject the rest ({n} photos)")
                }
                (false, false) => format!("In {es} pick the best {k} ({n} photos)"),
            }
        }
        "apply_preset" => {
            let p = preset_label(names, args["preset_id"].as_str().unwrap_or_default());
            if zh {
                format!("给 {n} 张照片应用「{p}」")
            } else {
                format!("Apply \"{p}\" to {n} photo(s)")
            }
        }
        "auto_adjust" => {
            if zh {
                format!("对 {n} 张照片一键修图")
            } else {
                format!("Auto adjust {n} photo(s)")
            }
        }
        "apply_profiles" => {
            if zh {
                format!("对 {n} 张照片应用人物美颜档案")
            } else {
                format!("Apply beauty profiles to {n} photo(s)")
            }
        }
        "besttake_auto" => {
            if zh {
                format!("对 {n} 张照片所在的连拍组做全员最佳")
            } else {
                format!("Best take for the burst groups of {n} photo(s)")
            }
        }
        "remove_bystanders" => {
            if zh {
                format!("消除 {n} 张照片中的路人")
            } else {
                format!("Remove bystanders from {n} photo(s)")
            }
        }
        "export" => {
            let p = args["preset"].as_str().unwrap_or("original");
            if zh {
                format!("导出 {n} 张照片（{p}）")
            } else {
                format!("Export {n} photo(s) ({p})")
            }
        }
        "describe" => {
            if zh {
                "描述这张照片".to_string()
            } else {
                "Describe this photo".to_string()
            }
        }
        "suggest_edits" => {
            if zh {
                "为这张照片给出修图建议".to_string()
            } else {
                "Suggest edits for this photo".to_string()
            }
        }
        _ => tool.to_string(),
    }
}

fn reply_for(steps: &[PlanStep], hints: &[String], l: Locale, confirm: bool) -> String {
    let zh = l == Locale::Zh;
    let list = steps
        .iter()
        .map(|s| s.summary.clone())
        .collect::<Vec<_>>()
        .join(if zh { "；" } else { "; " });
    let mut r = if zh {
        format!("好的，我会：{list}。")
    } else {
        format!("Okay, I will: {list}.")
    };
    if confirm {
        r.push_str(if zh {
            "其中包含不可轻易还原的操作，请确认后执行（执行后可撤销）。"
        } else {
            " This includes changes that need your confirmation (you can undo afterwards)."
        });
    }
    for h in hints {
        r.push(' ');
        r.push_str(h);
    }
    r
}

#[cfg(test)]
mod destructive_tests {
    use serde_json::json;

    #[test]
    fn batch_edits_on_many_photos_need_confirmation() {
        let a = json!({});
        for tool in [
            "auto_adjust",
            "apply_preset",
            "apply_profiles",
            "besttake_auto",
            "remove_bystanders",
        ] {
            assert!(super::is_destructive(tool, &a, 35), "{tool} on 35 photos");
            assert!(!super::is_destructive(tool, &a, 1), "{tool} on one photo");
        }
        assert!(!super::is_destructive("filter", &a, 100));
        assert!(super::is_destructive("set_flag", &json!({"flag": -1}), 1));
    }
}
