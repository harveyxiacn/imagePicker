//! M6 AI assistant (`docs/api-contract-m6.md` section A): plans from natural language, either by
//! the rules engine or by a language model behind strict validation, executed with the same
//! Core methods the REST endpoints use.

pub mod exec;
pub mod keep_top;
pub mod rules;
pub mod target;
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
use self::target::Target;
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

/// A step that changes more photos than this is always a large change.
pub const LARGE_ABS: usize = 500;
/// A step that changes more than half of the session's photos is a large change once it changes
/// at least this many (small sessions are not nagged).
pub const LARGE_MIN: usize = 20;

/// Large changes need `confirm: true` on execution (`docs/api-contract-m6.md` A.4).
pub fn is_large(affects: usize, session_photos: usize) -> bool {
    affects > LARGE_ABS || (affects >= LARGE_MIN && affects.saturating_mul(2) > session_photos)
}

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
    /// Which photos the step works on, in words (`None` for steps that change no photo).
    pub criteria: Option<String>,
    /// See [`is_large`].
    pub large_impact: bool,
}

/// Why the plan does not come from the language model although one was requested or tried.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Fallback {
    /// `llm_failed` (the model call failed or its output was invalid), `llm_no_steps`,
    /// `llm_invalid` (unknown preset / person ...), `llm_unsafe_target` (a step's photos were not
    /// justified, see [`target`]), `llm_tool_mismatch` / `llm_arg_mismatch` (the rules engine's
    /// certain reading disagrees, see `rules_disagree`).
    pub code: String,
    pub message: String,
}

impl Fallback {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanOut {
    pub plan_id: String,
    pub reply: String,
    pub steps: Vec<PlanStep>,
    pub needs_confirmation: bool,
    pub engine: String,
    pub unsupported: Option<String>,
    /// A step is a large change ([`is_large`]): execution needs `confirm: true`.
    pub large_impact: bool,
    /// What the confirmation shows: every step that needs it, with its count and criteria.
    pub confirm_text: Option<String>,
    /// Photos in the session (the base of `large_impact`).
    pub session_photos: usize,
    /// Set when a language model was requested or tried but the plan is the rules engine's.
    pub fallback: Option<Fallback>,
}

#[derive(Debug, Clone)]
pub(crate) struct StoredPlan {
    pub session_id: i64,
    pub steps: Vec<PlanStep>,
    pub ctx: PlanContext,
    pub created: Instant,
    pub large_impact: bool,
    pub confirm_text: Option<String>,
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

    /// Removes and returns a live plan (a plan runs once). A large plan stays stored (and is not
    /// run) until it is taken with `confirm`.
    pub(crate) fn take(&self, id: &str, confirm: bool) -> Result<StoredPlan> {
        self.purge(Instant::now());
        let mut plans = self.plans.lock().unwrap();
        let gone = || {
            CoreError::not_found(format!(
                "plan {id} not found (plans expire after 30 minutes and run once)"
            ))
        };
        match plans.get(id) {
            None => return Err(gone()),
            Some(p) if p.large_impact && !confirm => {
                return Err(CoreError::ConfirmationRequired(format!(
                    "this plan is a large change; execute it with confirm=true. {}",
                    p.confirm_text.clone().unwrap_or_default()
                )))
            }
            Some(_) => {}
        }
        plans.remove(id).ok_or_else(gone)
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

/// Whether a model whose registry `task` list is `task` serves the assistant role `kind`
/// (`llm` / `vlm`). The worker's registry names the tasks by method (`llm_plan`, `vlm_suggest`,
/// `vlm_describe`), so a bare `llm` / `vlm` and any `<kind>_...` task count.
pub(crate) fn serves(task: &[String], kind: &str) -> bool {
    task.iter().any(|t| {
        let t = t.to_ascii_lowercase();
        t == kind
            || t.strip_prefix(kind)
                .is_some_and(|rest| rest.starts_with('_'))
    })
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
                .find(|m| m.installed && serves(&m.task, kind))
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
            if !start_worker {
                // `auto` only uses a model that is already there
                return Err(CoreError::Conflict("no language model is loaded".into()));
            }
            return Err(self.llm_missing().await);
        }
        let ctx = json!({
            "filter": req.context.filter,
            "selection_count": req.context.selection.len(),
            "selection": req.context.selection.iter().take(200).collect::<Vec<_>>(),
            "current_photo_id": req.context.current_photo_id,
            "people": people.iter().map(|(id, n)| json!({"id": id, "name": n})).collect::<Vec<_>>(),
            "presets": presets.iter().map(|p| json!({"id": p.id, "names": p.names})).collect::<Vec<_>>(),
        });
        let resp = self
            .worker
            .llm_plan(&LlmPlanRequest {
                message: req.message.clone(),
                tools: llm_tools_for(presets),
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

    /// Why `engine: "llm"` cannot be served: 409 `models_missing` when the worker knows a language
    /// model that is not installed, else 503 `worker_unavailable`.
    async fn llm_missing(&self) -> CoreError {
        match self.worker.models_list().await {
            Ok(listing) => {
                let ids: Vec<String> = listing
                    .models
                    .iter()
                    .filter(|m| serves(&m.task, "llm"))
                    .map(|m| m.id.clone())
                    .collect();
                if ids.is_empty() {
                    CoreError::WorkerUnavailable(
                        "this AI worker has no language model (engine \"llm\" is unavailable)"
                            .into(),
                    )
                } else {
                    CoreError::ModelsMissing(ids)
                }
            }
            Err(e) => map_worker_err(e),
        }
    }

    async fn session_photo_count(&self, session_id: i64) -> Result<usize> {
        let q = tools::photo_query_of(session_id, &json!({}), Some(1));
        Ok(self.photos(q).await?.total as usize)
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
        let names = Names {
            people: &people,
            presets: &presets,
        };
        // the rules engine's reading: the plan when no model is used, and the check of a
        // model plan that acts without a condition of its own
        let rules = rules::parse(
            &req.message,
            &RulesCtx {
                people: &people,
                presets: &presets,
                selection: &req.context.selection,
                current_photo_id: req.context.current_photo_id,
                locale,
            },
        );
        let env = PlanEnv {
            session_id: sid,
            ctx: &req.context,
            session_photos: self.session_photo_count(sid).await?,
            locale,
            names: &names,
        };

        let mut fallback: Option<Fallback> = None;
        let mut from_llm: Option<(String, Vec<PlanStep>)> = None;
        if engine_req != "rules" {
            // `auto` only uses a model that is already loaded; `llm` starts the worker
            match self
                .llm_calls(&req, locale, &people, &presets, engine_req == "llm")
                .await
            {
                Ok((reply, mut calls)) if !calls.is_empty() => {
                    let targets = target::bind(&mut calls, &req.context);
                    let agree = target::rules_unscoped(
                        &rules.calls,
                        rules.unsupported.is_some() || !rules.hints.is_empty(),
                        &req.context,
                    );
                    match target::llm_problem(&calls, &targets, agree, locale) {
                        Some(why) => fallback = Some(Fallback::new("llm_unsafe_target", why)),
                        None => match rules_disagree(&req.message, &rules, &calls, locale) {
                            Some((code, why)) => fallback = Some(Fallback::new(code, why)),
                            None => match self.build_steps(&env, &calls, &targets).await {
                                Ok(steps) => from_llm = Some((reply, steps)),
                                Err(why) => fallback = Some(Fallback::new("llm_invalid", why)),
                            },
                        },
                    }
                }
                Ok(_) => {
                    fallback = Some(Fallback::new("llm_no_steps", "the model proposed no steps"));
                }
                Err(e) => {
                    let unavailable = matches!(
                        e,
                        CoreError::ModelsMissing(_)
                            | CoreError::WorkerUnavailable(_)
                            | CoreError::WorkerTimeout(_)
                    );
                    if engine_req == "llm" && unavailable {
                        return Err(e);
                    }
                    // `auto` without a loaded model is the normal case, not worth a note
                    if !matches!(e, CoreError::Conflict(_)) {
                        fallback = Some(Fallback::new("llm_failed", e.to_string()));
                    }
                }
            }
            if let Some(f) = &fallback {
                tracing::info!(code = %f.code, reason = %f.message, "assistant: using the rules engine");
            }
        }

        let (engine, steps, llm_reply, hints, unsupported) = match from_llm {
            Some((reply, steps)) => ("llm", steps, Some(reply), Vec::new(), None),
            None => {
                let mut unsupported = rules.unsupported.clone();
                let mut steps = Vec::new();
                if unsupported.is_none() {
                    let mut calls = rules.calls.clone();
                    let targets = target::bind(&mut calls, &req.context);
                    match self.build_steps(&env, &calls, &targets).await {
                        Ok(s) => steps = s,
                        Err(msg) => unsupported = Some(msg),
                    }
                    if steps.is_empty() && unsupported.is_none() {
                        unsupported = Some(rules::unsupported_help(locale));
                    }
                }
                ("rules", steps, None, rules.hints.clone(), unsupported)
            }
        };
        let large_impact = steps.iter().any(|s| s.large_impact);
        let needs_confirmation = large_impact || steps.iter().any(|s| s.destructive);
        let confirm_text =
            needs_confirmation.then(|| confirm_text(&steps, env.session_photos, locale));
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
                    large_impact,
                    confirm_text: confirm_text.clone(),
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
            large_impact,
            confirm_text,
            session_photos: env.session_photos,
            fallback,
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

    /// Every call as a step, in order; the first that fails explains why.
    async fn build_steps(
        &self,
        env: &PlanEnv<'_>,
        calls: &[Call],
        targets: &[Target],
    ) -> std::result::Result<Vec<PlanStep>, String> {
        let mut steps = Vec::with_capacity(calls.len());
        for (c, t) in calls.iter().zip(targets) {
            steps.push(self.build_step(env, c, t).await?);
        }
        Ok(steps)
    }

    /// Validates a call and computes what the UI shows: summary, affected count, destructive,
    /// criteria and whether it is a large change.
    async fn build_step(
        &self,
        env: &PlanEnv<'_>,
        call: &Call,
        target: &Target,
    ) -> std::result::Result<PlanStep, String> {
        let (session_id, ctx, locale, names) = (env.session_id, env.ctx, env.locale, env.names);
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
        let changes = target::changes_photos(tool);
        Ok(PlanStep {
            tool: tool.to_string(),
            summary: summarize(tool, &args, affects, locale, names),
            criteria: criteria_of(target, &args, ctx, locale, names),
            large_impact: changes && is_large(affects, env.session_photos),
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
            .filter(|m| serves(&m.task, "vlm"))
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

/// What building the steps of one plan needs.
struct PlanEnv<'a> {
    session_id: i64,
    ctx: &'a PlanContext,
    session_photos: usize,
    locale: Locale,
    names: &'a Names<'a>,
}

/// The tool schemas offered to the model, with `apply_preset.preset_id` limited to the presets
/// that exist (their names are in the context): with a free string a model can invent an id.
fn llm_tools_for(presets: &[PresetName]) -> Vec<Value> {
    let mut tools = llm_tools();
    let ids: Vec<&str> = presets.iter().map(|p| p.id.as_str()).collect();
    if ids.is_empty() {
        return tools;
    }
    for t in tools.iter_mut().filter(|t| t["name"] == "apply_preset") {
        t["parameters"]["properties"]["preset_id"] = json!({"type": "string", "enum": ids});
    }
    tools
}

/// A condition of the rules' `filter` step that the model's `filter` step leaves out or changes
/// ("show photos with closed eyes" answered with `person_state: eyes_open`, a capture day with
/// `issues_none`). Extra model conditions are allowed; plans without a `filter` step on either
/// side are checked through their targets instead.
fn filter_dropped(rules_calls: &[Call], llm_calls: &[Call], l: Locale) -> Option<String> {
    let last = |cs: &[Call]| {
        cs.iter()
            .rev()
            .find(|c| c.tool == "filter")
            .map(|c| c.args.clone())
    };
    let (want, got) = (last(rules_calls)?, last(llm_calls)?);
    let (want, got) = (
        want.as_object()?,
        got.as_object().cloned().unwrap_or_default(),
    );
    for (k, v) in want {
        if matches!(k.as_str(), "sort" | "person_mode" | "include_background") {
            continue;
        }
        let ok = match (v, got.get(k)) {
            // a list condition: the model may list more values, not fewer
            (Value::Array(w), Some(Value::Array(g))) => w.iter().all(|x| g.contains(x)),
            (w, g) => same_value(Some(w), g),
        };
        if !ok {
            let have = got.get(k).map_or("-".to_string(), Value::to_string);
            return Some(match l {
                Locale::Zh => format!("模型的筛选条件 {k} = {have}，与请求不符（应为 {v}）"),
                Locale::En => {
                    format!("the model's filter has {k} = {have}, but the request says {v}")
                }
            });
        }
    }
    None
}

/// Tools whose steps change photos, as the rules engine and a model must agree on them.
fn changing_calls(calls: &[Call]) -> Vec<&Call> {
    calls
        .iter()
        .filter(|c| target::changes_photos(&c.tool))
        .collect()
}

/// The arguments that carry the user's intent: a model step must not change them.
const INTENT_ARGS: [(&str, &[&str]); 7] = [
    ("set_flag", &["flag"]),
    ("set_rating", &["rating"]),
    ("apply_preset", &["preset_id"]),
    ("adjust_sliders", &["values", "relative"]),
    ("group_keep_top", &["n", "reject_rest"]),
    ("scene_keep_top", &["n", "reject_rest"]),
    ("export", &["preset"]),
];

fn same_value(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (Some(Value::Number(x)), Some(Value::Number(y))) => {
            (x.as_f64().unwrap_or(f64::NAN) - y.as_f64().unwrap_or(f64::NAN)).abs() < 1e-6
        }
        (Some(Value::Object(x)), Some(Value::Object(y))) => {
            x.len() == y.len() && x.iter().all(|(k, v)| same_value(Some(v), y.get(k)))
        }
        (a, b) => a == b,
    }
}

/// An intent argument of a step, with its default (`relative` is false when left out).
fn intent_arg<'a>(args: &'a Value, key: &str) -> Option<&'a Value> {
    static FALSE: Value = Value::Bool(false);
    match (key, args.get(key)) {
        ("relative", None) => Some(&FALSE),
        (_, v) => v,
    }
}

/// The rules engine as a check of a model plan (`docs/api-contract-m6.md` A.4). When the rules
/// read the request with certainty (no hint, not unsupported), every photo-changing step of their
/// reading must be in the model plan, in order, with the same intent arguments; a view request
/// must not change photos. `llm_tool_mismatch`: "暖调胶片预设" answered with `apply_profiles`,
/// "把曝光提高 0.5" with `auto_adjust`, "只看5星" with an edit; `llm_arg_mismatch`: another flag,
/// star count, preset, slider value or keep-top count. Extra model steps are allowed (they are
/// validated and confirmed like every step), and requests the rules do not understand are left
/// to the model.
fn rules_disagree(
    msg: &str,
    rules: &rules::Parsed,
    llm_calls: &[Call],
    l: Locale,
) -> Option<(&'static str, String)> {
    // a capture day the request does not name is invented (seen: the date of a prompt example),
    // whether or not the rules understood the rest
    let named = rules::named_days(msg);
    for c in llm_calls.iter().filter(|c| c.tool == "filter") {
        for k in ["taken_from", "taken_to"] {
            if let Some(d) = c.args.get(k).and_then(Value::as_str) {
                if !named.iter().any(|n| n == d) {
                    return Some((
                        "llm_arg_mismatch",
                        match l {
                            Locale::Zh => format!("模型的筛选用了请求里没有的日期 {k} = {d}"),
                            Locale::En => format!(
                                "the model's filter has {k} = {d}, a day the request does not name"
                            ),
                        },
                    ));
                }
            }
        }
    }
    if rules.unsupported.is_some() || !rules.hints.is_empty() || rules.calls.is_empty() {
        return None;
    }
    if let Some(why) = filter_dropped(&rules.calls, llm_calls, l) {
        return Some(("llm_arg_mismatch", why));
    }
    let (want, got) = (changing_calls(&rules.calls), changing_calls(llm_calls));
    let tool_mismatch = |what: String| -> Option<(&'static str, String)> {
        let has = |tool: &str| want.iter().any(|c| c.tool == tool);
        Some((
            "llm_tool_mismatch",
            match l {
                Locale::Zh if has("apply_preset") => "模型没有套用请求里点名的预设".into(),
                Locale::Zh if has("adjust_sliders") => "模型没有按请求里的数值调整滑块".into(),
                Locale::En if has("apply_preset") => {
                    "the model did not apply the preset the request names".into()
                }
                Locale::En if has("adjust_sliders") => {
                    "the model did not set the slider values the request names".into()
                }
                _ => what,
            },
        ))
    };
    if want.is_empty() {
        // a view request: the model must not change photos
        return match got.first() {
            Some(c) => tool_mismatch(match l {
                Locale::Zh => format!("请求只是筛选，模型的计划却会修改照片（{}）", c.tool),
                Locale::En => format!(
                    "the request only filters, but the model's plan changes photos ({})",
                    c.tool
                ),
            }),
            None => None,
        };
    }
    let mut from = 0;
    for r in &want {
        let Some(pos) = got[from..].iter().position(|m| m.tool == r.tool) else {
            return tool_mismatch(match l {
                Locale::Zh => format!("模型的计划缺少请求里的 {}", r.tool),
                Locale::En => format!("the model's plan has no {} step", r.tool),
            });
        };
        let m = got[from + pos];
        from += pos + 1;
        let keys = INTENT_ARGS
            .iter()
            .find(|(t, _)| *t == r.tool)
            .map(|(_, k)| *k)
            .unwrap_or(&[]);
        for k in keys {
            // an option the request did not state (export without a platform) is the model's
            if *k == "preset" && r.args.get(k).is_none() {
                continue;
            }
            let (a, b) = (intent_arg(&r.args, k), intent_arg(&m.args, k));
            if !same_value(a, b) {
                let shown = |v: Option<&Value>| v.map_or("-".to_string(), Value::to_string);
                let (have, should) = (shown(m.args.get(*k)), shown(r.args.get(*k)));
                return Some((
                    "llm_arg_mismatch",
                    match l {
                        Locale::Zh => format!(
                            "模型的 {} 参数 {k} = {have}，与请求不符（应为 {should}）",
                            r.tool
                        ),
                        Locale::En => format!(
                            "the model's {} step has {k} = {have}, but the request says {should}",
                            r.tool
                        ),
                    },
                ));
            }
        }
    }
    // a step the rules did not read is the model's only where the request names that action
    // ("评5星然后导出"); any other is a change nobody asked for ("淘汰过曝的" + rate the rest)
    let named = |tool: &str| {
        let n = |cs: &[&Call]| cs.iter().filter(|c| c.tool == tool).count();
        let cues = rules.mentioned.iter().filter(|t| **t == tool).count();
        (n(&got), n(&want).max(cues))
    };
    if let Some(m) = got.iter().find(|m| {
        let (have, allowed) = named(&m.tool);
        have > allowed
    }) {
        return Some((
            "llm_tool_mismatch",
            match l {
                Locale::Zh => format!("模型的计划多出请求里没有的 {}", m.tool),
                Locale::En => format!(
                    "the model's plan has a {} step the request does not ask for",
                    m.tool
                ),
            },
        ));
    }
    None
}

fn is_destructive(tool: &str, args: &Value, affects: usize) -> bool {
    match tool {
        "set_flag" => args["flag"].as_i64() == Some(-1),
        "set_rating" | "accept_ai" => affects > 1,
        // batch edits / generative changes: undoable, but confirm before touching many photos
        "auto_adjust" | "adjust_sliders" | "apply_preset" | "apply_profiles" | "besttake_auto"
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
        ("bystander", Locale::Zh) => "路人入镜",
        ("bad_expression", Locale::Zh) => "表情欠佳",
        ("closed_eyes", Locale::En) => "closed eyes",
        ("blurry", Locale::En) => "blurry",
        ("overexposed", Locale::En) => "overexposed",
        ("underexposed", Locale::En) => "underexposed",
        ("noisy", Locale::En) => "noisy",
        ("tilted", Locale::En) => "tilted",
        ("bystander", Locale::En) => "bystander",
        ("bad_expression", Locale::En) => "bad expression",
        _ => "?",
    }
}

/// The conditions of canonical filter arguments, in words.
fn filter_parts(args: &Value, l: Locale, names: &Names) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let zh = l == Locale::Zh;
    if let Some(p) = args.get("persons").and_then(Value::as_array) {
        let ids: Vec<i64> = p.iter().filter_map(Value::as_i64).collect();
        parts.push(person_labels(names, &ids));
    }
    if let Some(p) = args.get("exclude_persons").and_then(Value::as_array) {
        let ids: Vec<i64> = p.iter().filter_map(Value::as_i64).collect();
        let who = person_labels(names, &ids);
        parts.push(if zh {
            format!("不含{who}")
        } else {
            format!("without {who}")
        });
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
    if let Some(a) = args.get("person_state").and_then(Value::as_array) {
        for st in a.iter().filter_map(Value::as_str) {
            parts.push(
                match (st, zh) {
                    ("eyes_open", true) => "睁眼",
                    ("smiling", true) => "微笑",
                    ("looking", true) => "看镜头",
                    ("subject", true) => "主体人物",
                    ("eyes_open", false) => "eyes open",
                    ("smiling", false) => "smiling",
                    ("looking", false) => "looking at the camera",
                    _ => "subject",
                }
                .to_string(),
            );
        }
    }
    let (fmin, fmax) = (
        args.get("faces_min").and_then(Value::as_i64),
        args.get("faces_max").and_then(Value::as_i64),
    );
    match (fmin, fmax) {
        (Some(a), Some(b)) if a == b => parts.push(if zh {
            format!("{a} 张脸")
        } else {
            format!("{a} face(s)")
        }),
        _ => {
            if let Some(a) = fmin {
                parts.push(if zh {
                    format!("至少 {a} 张脸")
                } else {
                    format!("at least {a} face(s)")
                });
            }
            if let Some(b) = fmax {
                parts.push(if zh {
                    format!("最多 {b} 张脸")
                } else {
                    format!("at most {b} face(s)")
                });
            }
        }
    }
    if let Some(b) = args.get("burst_id").and_then(Value::as_i64) {
        parts.push(if zh {
            format!("连拍组 #{b}")
        } else {
            format!("burst #{b}")
        });
    }
    let (from, to) = (
        args.get("taken_from").and_then(Value::as_str),
        args.get("taken_to").and_then(Value::as_str),
    );
    match (from, to) {
        (Some(a), Some(b)) if a == b => parts.push(if zh {
            format!("{a} 拍摄")
        } else {
            format!("taken on {a}")
        }),
        (None, None) => {}
        (a, b) => parts.push(if zh {
            format!("{} 至 {} 拍摄", a.unwrap_or("…"), b.unwrap_or("…"))
        } else {
            format!("taken {} – {}", a.unwrap_or("…"), b.unwrap_or("…"))
        }),
    }
    parts
}

fn filter_summary(args: &Value, l: Locale, names: &Names) -> String {
    let zh = l == Locale::Zh;
    let parts = filter_parts(args, l, names);
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

/// A selection query's conditions in words ("模糊" / "blurry, first 3 by AI score").
fn query_criteria(q: &str, l: Locale, names: &Names) -> String {
    let zh = l == Locale::Zh;
    let Ok((args, limit)) = tools::query_args(q) else {
        return q.to_string();
    };
    let mut parts = filter_parts(&args, l, names);
    if let Some(n) = limit {
        let by = match (args.get("sort").and_then(Value::as_str), zh) {
            (Some("ai"), true) => "按 AI 评分",
            (Some("rating"), true) => "按星级",
            (Some("name"), true) => "按文件名",
            (Some("-taken_at"), true) => "从最新起",
            (_, true) => "按拍摄时间",
            (Some("ai"), false) => " by AI score",
            (Some("rating"), false) => " by rating",
            (Some("name"), false) => " by name",
            (Some("-taken_at"), false) => ", newest first",
            (_, false) => " by capture time",
        };
        parts.push(if zh {
            format!("{by}前 {n} 张")
        } else {
            format!("the first {n}{by}")
        });
    }
    if parts.is_empty() {
        (if zh { "全部照片" } else { "all photos" }).to_string()
    } else {
        parts.join(if zh { "，" } else { ", " })
    }
}

/// Which photos a step works on, in words; `None` for steps that change no photo.
fn criteria_of(
    t: &Target,
    args: &Value,
    ctx: &PlanContext,
    l: Locale,
    names: &Names,
) -> Option<String> {
    let zh = l == Locale::Zh;
    Some(match t {
        Target::NoPhotos => return None,
        Target::Query(q) => query_criteria(q, l, names),
        Target::View(q) => {
            let c = query_criteria(q, l, names);
            if zh {
                format!("当前视图（{c}）")
            } else {
                format!("the current view ({c})")
            }
        }
        Target::Session => (if zh {
            "本会话的全部照片"
        } else {
            "every photo in this session"
        })
        .to_string(),
        Target::Ids { from_context } => {
            let ids: Vec<i64> = args["selection"]["ids"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_i64).collect())
                .unwrap_or_default();
            let n = ids.len();
            let open = n == 1 && ctx.current_photo_id == Some(ids[0]);
            match (open, *from_context, zh) {
                (true, _, true) => "当前打开的照片".to_string(),
                (true, _, false) => "the open photo".to_string(),
                (false, true, true) => format!("选中的 {n} 张照片"),
                (false, true, false) => format!("the {n} selected photo(s)"),
                (false, false, true) => format!("指定的 {n} 张照片"),
                (false, false, false) => format!("{n} listed photo(s)"),
            }
        }
    })
}

/// The confirmation: every destructive or large step with its count (in the summary), its
/// criteria and, for a large change, its share of the session.
fn confirm_text(steps: &[PlanStep], session_photos: usize, l: Locale) -> String {
    let zh = l == Locale::Zh;
    let mut lines = Vec::new();
    for s in steps.iter().filter(|s| s.destructive || s.large_impact) {
        let crit = s.criteria.clone().unwrap_or_else(|| "-".into());
        let mut line = if zh {
            format!("{}（照片：{crit}）", s.summary)
        } else {
            format!("{} (photos: {crit})", s.summary)
        };
        if s.large_impact {
            let total = session_photos.max(s.affects).max(1);
            let pct = (s.affects * 100 + total / 2) / total;
            line.push_str(&if zh {
                format!("，占本会话 {session_photos} 张中的 {pct}%，属于大范围修改")
            } else {
                format!(", {pct}% of the {session_photos} photos in this session: a large change")
            });
        }
        lines.push(line);
    }
    lines.join("\n")
}

fn slider_label(key: &str, zh: bool) -> String {
    let s = match (key, zh) {
        ("exposure", true) => "曝光",
        ("exposure", false) => "exposure",
        ("contrast", true) => "对比度",
        ("contrast", false) => "contrast",
        ("highlights", true) => "高光",
        ("highlights", false) => "highlights",
        ("shadows", true) => "阴影",
        ("shadows", false) => "shadows",
        ("whites", true) => "白色",
        ("whites", false) => "whites",
        ("blacks", true) => "黑色",
        ("blacks", false) => "blacks",
        ("temp", true) => "色温",
        ("temp", false) => "temperature",
        ("tint", true) => "色调",
        ("tint", false) => "tint",
        ("vibrance", true) => "自然饱和度",
        ("vibrance", false) => "vibrance",
        ("saturation", true) => "饱和度",
        ("saturation", false) => "saturation",
        ("clarity", true) => "清晰度",
        ("clarity", false) => "clarity",
        ("dehaze", true) => "去朦胧",
        ("dehaze", false) => "dehaze",
        ("texture", true) => "纹理",
        ("texture", false) => "texture",
        ("dodge", true) => "减淡",
        ("dodge", false) => "dodge",
        ("burn", true) => "加深",
        ("burn", false) => "burn",
        _ => return key.to_string(),
    };
    s.to_string()
}

/// A relative slider number with its sign: 0.5 -> "+0.5", -10 -> "-10".
fn signed_num(x: f64) -> String {
    if x > 0.0 {
        format!("+{}", trim_num(x))
    } else {
        trim_num(x)
    }
}

/// A slider number without trailing zeros: 0.5 -> "0.5", 20 -> "20".
fn trim_num(x: f64) -> String {
    let s = format!("{x:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
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
        "adjust_sliders" => {
            let rel = args["relative"].as_bool() == Some(true);
            let mut parts: Vec<String> = args["values"]
                .as_object()
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| Some((slider_label(k, zh), v.as_f64()?)))
                        .map(|(label, x)| {
                            if rel {
                                format!("{label} {}", signed_num(x))
                            } else {
                                format!("{label} {}", trim_num(x))
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            parts.sort();
            let what = parts.join(if zh { "、" } else { ", " });
            if zh {
                let verb = if rel { "调整" } else { "设置" };
                format!("{verb} {n} 张照片：{what}")
            } else {
                format!(
                    "{} {what} on {n} photo(s)",
                    if rel { "Adjust" } else { "Set" }
                )
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
mod check_tests {
    use serde_json::{json, Value};

    use super::rules::{builtin_preset_names, Call, Locale, Parsed};

    fn call(tool: &str, args: Value) -> Call {
        Call {
            tool: tool.into(),
            args,
        }
    }

    fn sure(calls: Vec<Call>) -> Parsed {
        Parsed {
            calls,
            ..Default::default()
        }
    }

    #[test]
    fn the_rules_check_tools_and_intent_arguments() {
        let sel = json!({"ids": [1, 2]});
        let flag = |f: i64| call("set_flag", json!({"selection": sel, "flag": f}));
        let code =
            |r: &Parsed, m: Vec<Call>| super::rules_disagree("", r, &m, Locale::En).map(|c| c.0);
        // agreement, also with an extra filter step in the model plan
        let r = sure(vec![flag(-1)]);
        assert_eq!(code(&r, vec![flag(-1)]), None);
        assert_eq!(code(&r, vec![call("filter", json!({})), flag(-1)]), None);
        // another value of the intent argument
        assert_eq!(code(&r, vec![flag(1)]), Some("llm_arg_mismatch"));
        // another tool, or a filter where the request changes photos (and the reverse)
        let rate = call("set_rating", json!({"selection": sel, "rating": 3}));
        assert_eq!(code(&r, vec![rate.clone()]), Some("llm_tool_mismatch"));
        // an extra changing step only where the request names that action ("评5星然后导出")
        assert_eq!(
            code(&r, vec![rate.clone(), flag(-1)]),
            Some("llm_tool_mismatch")
        );
        assert_eq!(
            code(&r, vec![flag(-1), flag(-1)]),
            Some("llm_tool_mismatch")
        );
        let named = Parsed {
            calls: vec![flag(-1)],
            mentioned: vec!["set_flag", "set_rating"],
            ..Default::default()
        };
        assert_eq!(code(&named, vec![rate.clone(), flag(-1)]), None);
        assert_eq!(
            code(&named, vec![rate.clone(), rate.clone(), flag(-1)]),
            Some("llm_tool_mismatch")
        );
        assert_eq!(
            code(&r, vec![call("filter", json!({"flag": "rejected"}))]),
            Some("llm_tool_mismatch")
        );
        let view = sure(vec![call("filter", json!({"rating_gte": 5}))]);
        assert_eq!(code(&view, vec![rate]), Some("llm_tool_mismatch"));
        // slider values: sign, size and relative / absolute
        let s = |v: Value, rel: Option<bool>| {
            let mut a = json!({"selection": sel, "values": v});
            if let Some(r) = rel {
                a["relative"] = json!(r);
            }
            call("adjust_sliders", a)
        };
        let r = sure(vec![s(json!({"exposure": 0.5}), Some(true))]);
        assert_eq!(
            code(&r, vec![s(json!({"exposure": 0.5}), Some(true))]),
            None
        );
        for wrong in [
            s(json!({"exposure": -0.5}), Some(true)),
            s(json!({"exposure": 0.5}), Some(false)),
            s(json!({"exposure": 0.5}), None),
            s(json!({"exposure": 0.5, "contrast": 10}), Some(true)),
        ] {
            assert_eq!(code(&r, vec![wrong]), Some("llm_arg_mismatch"));
        }
        // absolute values: a missing `relative` is false
        let r = sure(vec![s(json!({"contrast": 20}), Some(false))]);
        assert_eq!(code(&r, vec![s(json!({"contrast": 20.0}), None)]), None);
        // the preset the request names
        let p = |id: &str| call("apply_preset", json!({"selection": sel, "preset_id": id}));
        let r = sure(vec![p("film_warm")]);
        assert_eq!(code(&r, vec![p("film_cool")]), Some("llm_arg_mismatch"));
        let profiles = call("apply_profiles", json!({"selection": sel}));
        let why = super::rules_disagree("", &r, &[profiles], Locale::Zh).unwrap();
        assert_eq!(
            why,
            ("llm_tool_mismatch", "模型没有套用请求里点名的预设".into())
        );
        // an export platform the request did not name is the model's choice
        let r = sure(vec![call("export", json!({"selection": sel}))]);
        let m = call("export", json!({"selection": sel, "preset": "wechat"}));
        assert_eq!(code(&r, vec![m]), None);
        // the conditions of the rules' filter must be in the model's filter
        let view = sure(vec![call("filter", json!({"issues_any": ["closed_eyes"]}))]);
        let f = |a: Value| vec![call("filter", a)];
        assert_eq!(
            code(
                &view,
                f(json!({"issues_none": false, "person_state": ["eyes_open"]}))
            ),
            Some("llm_arg_mismatch")
        );
        assert_eq!(
            code(
                &view,
                f(json!({"issues_any": ["closed_eyes", "blurry"], "sort": "ai"}))
            ),
            None
        );
        // "not edited" is not "no issues": a `false` condition is a condition
        let view = sure(vec![call("filter", json!({"has_edits": false}))]);
        assert_eq!(
            code(&view, f(json!({"issues_none": true}))),
            Some("llm_arg_mismatch")
        );
        // a capture day the request does not name is refused even when the rules are unsure
        let mut unsure = sure(vec![call("filter", json!({"rating_gte": 4}))]);
        unsure.hints.push("\"beach\" was ignored".into());
        let dated =
            f(json!({"rating_gte": 4, "taken_from": "2024-05-01", "taken_to": "2024-05-01"}));
        let check =
            |msg: &str| super::rules_disagree(msg, &unsure, &dated, Locale::En).map(|c| c.0);
        assert_eq!(
            check("show only 4 stars and up from the beach"),
            Some("llm_arg_mismatch")
        );
        assert_eq!(check("4 stars and up on 2024-05-01"), None);
        // requests the rules do not understand, or only approximate, are the model's
        let mut unsure = sure(vec![flag(-1)]);
        unsure.hints.push("an unknown place".into());
        assert_eq!(code(&unsure, vec![flag(1)]), None);
        let none = Parsed {
            unsupported: Some("?".into()),
            ..Default::default()
        };
        assert_eq!(code(&none, vec![flag(1)]), None);
    }

    #[test]
    fn the_model_only_sees_existing_preset_ids() {
        let tools = super::llm_tools_for(&builtin_preset_names());
        let t = tools.iter().find(|t| t["name"] == "apply_preset").unwrap();
        let ids = t["parameters"]["properties"]["preset_id"]["enum"]
            .as_array()
            .unwrap();
        assert!(ids.contains(&json!("film_warm")) && ids.len() == builtin_preset_names().len());
        let scene = &tools.iter().find(|t| t["name"] == "filter").unwrap()["parameters"]
            ["properties"]["scene_type"]["enum"];
        assert!(scene.as_array().unwrap().contains(&json!("night")));
    }
}

#[cfg(test)]
mod destructive_tests {
    use serde_json::json;

    use super::rules::Locale;

    #[test]
    fn batch_edits_on_many_photos_need_confirmation() {
        let a = json!({});
        for tool in [
            "auto_adjust",
            "adjust_sliders",
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

    #[test]
    fn slider_summary_names_the_values() {
        let names = super::Names {
            people: &[],
            presets: &[],
        };
        let args = json!({"values": {"exposure": 0.5}, "relative": true});
        let zh = super::summarize("adjust_sliders", &args, 3, Locale::Zh, &names);
        assert_eq!(zh, "调整 3 张照片：曝光 +0.5");
        let en = super::summarize("adjust_sliders", &args, 3, Locale::En, &names);
        assert_eq!(en, "Adjust exposure +0.5 on 3 photo(s)");

        let args = json!({"values": {"contrast": 20.0, "temp": -300.0}});
        let zh = super::summarize("adjust_sliders", &args, 1, Locale::Zh, &names);
        assert!(zh.contains("设置 1 张照片"), "{zh}");
        assert!(zh.contains("对比度 20"), "{zh}");
        assert!(zh.contains("色温 -300"), "{zh}");
    }

    #[test]
    fn large_changes() {
        use super::is_large;
        // more than half of the session, from LARGE_MIN photos on
        assert!(is_large(501, 1000));
        assert!(!is_large(500, 1000));
        assert!(!is_large(147, 1000));
        assert!(is_large(20, 30));
        assert!(!is_large(19, 19), "small sessions are not nagged");
        // more than LARGE_ABS photos, whatever the session
        assert!(is_large(501, 100_000));
        assert!(!is_large(499, 100_000));
    }

    #[test]
    fn the_confirmation_states_count_and_criteria() {
        use super::rules::Locale;
        let step = |summary: &str, affects, destructive, large| super::PlanStep {
            tool: "set_flag".into(),
            args: json!({}),
            summary: summary.into(),
            affects,
            destructive,
            criteria: Some("模糊".into()),
            large_impact: large,
        };
        let steps = vec![
            super::PlanStep {
                tool: "filter".into(),
                criteria: None,
                ..step("筛选：模糊", 147, false, false)
            },
            step("淘汰 147 张照片", 147, true, false),
        ];
        let t = super::confirm_text(&steps, 1000, Locale::Zh);
        assert_eq!(t, "淘汰 147 张照片（照片：模糊）");
        let steps = vec![step("Reject 1000 photo(s)", 1000, true, true)];
        let t = super::confirm_text(&steps, 1000, Locale::En);
        assert!(t.starts_with("Reject 1000 photo(s) (photos: 模糊)"), "{t}");
        assert!(t.contains("100% of the 1000 photos"), "{t}");
    }
}
