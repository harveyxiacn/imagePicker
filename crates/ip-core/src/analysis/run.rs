//! The analysis task and the `Core` API around it (worker, models, groups, people).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use ip_worker_client::{
    AnalyzeItem, AnalyzeRequest, AnalyzeRequestItem, CancelToken, WorkerError, WorkerState,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::incremental;
use super::store::{self, IngestItem};
use super::types::*;
use super::vecs::{decode_f16, normalize, parse_npy};
use crate::catalog;
use crate::catalog::now_ms;
use crate::error::{CoreError, Result};
use crate::events::Event;
use crate::model::PhotoUpdate;
use crate::Core;

/// Photos per `analyze.batch` request.
pub const BATCH_SIZE: usize = 32;
/// Long edge handed to the worker for analysis.
pub const ANALYSIS_SIZE: u32 = 1024;
const MAX_BATCH_RETRIES: u32 = 2;

/// Error detail of a 409 `models_missing`.
#[derive(Debug, Clone)]
pub struct ModelsMissing(pub Vec<String>);

/// A run that keeps ending with the process is picked up this many times, then left alone.
const MAX_RESUMES: u32 = 3;

/// Per-session run bookkeeping.
pub struct RunInfo {
    pub status: AnalysisStatus,
    pub cancel: CancelToken,
    pub task_id: String,
    /// Set by [`Core::analysis_pause`]: the run stops before its next batch.
    pub pause: Arc<AtomicBool>,
    pub(crate) spec: RunSpec,
}

/// What a run was asked to do. It is the `params` of the run's task row, so a run that is
/// paused or cut off by an exit can be taken up again with the photos that are left.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RunSpec {
    pub session_id: i64,
    /// The profile that was asked for.
    pub profile: Profile,
    /// The profile in use when it differs (`lite` after a fallback).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective: Option<Profile>,
    /// Photos of the run.
    #[serde(default)]
    pub count: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub photo_ids: Option<Vec<i64>>,
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub fallback_lite: bool,
    /// Photos analysed at or after this time (ms) were done by this run already.
    #[serde(default)]
    pub started_at: i64,
    /// How often the run was picked up after an exit.
    #[serde(default)]
    pub resumes: u32,
}

impl RunSpec {
    fn new(
        session_id: i64,
        profile: Profile,
        photo_ids: Option<Vec<i64>>,
        force: bool,
        fallback_lite: bool,
    ) -> Self {
        Self {
            session_id,
            profile,
            effective: None,
            count: 0,
            photo_ids,
            force,
            fallback_lite,
            started_at: now_ms(),
            resumes: 0,
        }
    }

    fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// Photos handed to the worker with the same steps.
struct Job {
    /// `None` = the profile's own steps; otherwise single steps for already analysed photos.
    steps: Option<Vec<String>>,
    refs: Vec<catalog::PhotoRef>,
}

#[derive(Default)]
struct RunPlan {
    jobs: Vec<Job>,
    /// Fingerprint of the installed models, when the worker was asked.
    fp: Option<String>,
}

impl RunPlan {
    fn total(&self) -> i64 {
        self.jobs.iter().map(|j| j.refs.len() as i64).sum()
    }
}

/// Everything `run_analysis` needs.
struct RunStart {
    spec: RunSpec,
    profile: Profile,
    allow_download: bool,
    plan: RunPlan,
    task_id: String,
    cancel: CancelToken,
    pause: Arc<AtomicBool>,
    /// Photos done before (a continued run).
    done0: i64,
}

/// Why an AI analysis cannot run, when the `lite` profile can stand in for it.
fn error_code(e: &CoreError) -> Option<String> {
    match e {
        CoreError::ModelsMissing(_) => Some("models_missing".into()),
        CoreError::WorkerUnavailable(_) | CoreError::WorkerTimeout(_) => {
            Some("worker_unavailable".into())
        }
        _ => None,
    }
}

pub(crate) fn map_worker_err(e: WorkerError) -> CoreError {
    if let Some(models) = e.model_unavailable() {
        return CoreError::ModelsMissing(models);
    }
    match e {
        WorkerError::Unavailable(m) => CoreError::WorkerUnavailable(m),
        WorkerError::Timeout(m) => CoreError::WorkerUnavailable(m),
        e @ WorkerError::CallTimeout { .. } => CoreError::WorkerTimeout(e.to_string()),
        other => CoreError::Internal(anyhow::anyhow!("{other}")),
    }
}

fn read_f32_file(path: &str) -> Option<Vec<f32>> {
    let bytes = std::fs::read(path).ok()?;
    if path.ends_with(".npy") {
        parse_npy(&bytes).ok().map(|n| n.data)
    } else {
        Some(decode_f16(&bytes))
    }
}

/// Reads the artifact files a worker result points to.
fn load_ingest(item: AnalyzeItem) -> IngestItem {
    let embedding = item
        .embedding_file
        .as_deref()
        .and_then(read_f32_file)
        .map(|mut v| {
            normalize(&mut v);
            v
        });
    let mut face_embs: Vec<Option<Vec<f32>>> = vec![None; item.faces.len()];
    if let Some(path) = item.identity_file.as_deref() {
        if let Some(npy) = std::fs::read(path).ok().and_then(|b| parse_npy(&b).ok()) {
            let rows = npy.rows();
            for (i, f) in item.faces.iter().enumerate() {
                if let Some(r) = rows.get(f.identity_index.unwrap_or(i)) {
                    face_embs[i] = Some(r.to_vec());
                }
            }
        }
    }
    IngestItem {
        item,
        embedding,
        face_embs,
    }
}

impl Core {
    // ------------------------------------------------------------ worker passthrough

    /// Current worker state; `probe` starts the worker (if needed) to learn tier and hardware.
    pub async fn hardware(&self, probe: bool) -> Result<WorkerOut> {
        if probe {
            // "unavailable" (Android, no runtime) is a state to report, not an error
            match self.worker.system_info().await {
                Ok(_) | Err(WorkerError::Unavailable(_)) => {}
                Err(e) => return Err(map_worker_err(e)),
            }
        }
        let st = self.worker.status();
        let info = st.info.clone();
        Ok(WorkerOut {
            state: st.state.as_str().to_string(),
            tier: st.tier.clone(),
            device: info
                .as_ref()
                .map(|i| i.hardware.device.clone())
                .filter(|d| !d.is_empty()),
            providers: info
                .as_ref()
                .map(|i| i.providers.clone())
                .unwrap_or_default(),
            gpu: info
                .as_ref()
                .and_then(|i| i.hardware.gpus.first())
                .map(|g| GpuOut {
                    name: g.name.clone(),
                    vram_mb: g.vram_mb,
                }),
            error: st.error,
        })
    }

    /// Registry with install state (starts the worker).
    pub async fn models(&self) -> Result<Vec<ModelInfo>> {
        let l = self.worker.models_list().await.map_err(map_worker_err)?;
        let fast_steps = ["phash", "quality", "faces"];
        Ok(l.models
            .iter()
            .map(|m| {
                let mut req = Vec::new();
                for p in ["fast", "standard"] {
                    let needed = match l.profiles.get(p) {
                        Some(pi) => pi.models.contains(&m.id),
                        None => {
                            let any = !m.required_for.is_empty();
                            if p == "fast" {
                                m.required_for
                                    .iter()
                                    .any(|s| fast_steps.contains(&s.as_str()))
                            } else {
                                any
                            }
                        }
                    };
                    if needed {
                        req.push(p.to_string());
                    }
                }
                ModelInfo {
                    id: m.id.clone(),
                    task: m.task.clone(),
                    size_mb: m.size_mb,
                    license: m.license.clone(),
                    noncommercial: m.noncommercial,
                    installed: m.installed,
                    required_for: req,
                }
            })
            .collect())
    }

    /// Downloads models in the background; progress arrives as `task.progress` `kind:"model_download"`
    /// (bytes). Returns the task id.
    pub async fn models_ensure(self: &Arc<Self>, ids: Vec<String>) -> Result<String> {
        if ids.is_empty() {
            return Err(CoreError::bad_request("ids must not be empty"));
        }
        let listing = self.worker.models_list().await.map_err(map_worker_err)?;
        let mut size: HashMap<String, f64> = HashMap::new();
        for id in &ids {
            let m = listing
                .models
                .iter()
                .find(|m| &m.id == id)
                .ok_or_else(|| CoreError::bad_request(format!("unknown model id {id:?}")))?;
            size.insert(id.clone(), m.size_mb * 1024.0 * 1024.0);
        }
        self.require_network("downloading models")?;
        let task_id = format!("models-{}", self.next_task_seq());
        let core = self.clone();
        let tid = task_id.clone();
        tokio::spawn(async move {
            let ev =
                |done: i64, total: i64, state: &str, error: Option<String>| Event::TaskProgress {
                    task_id: tid.clone(),
                    kind: "model_download".into(),
                    done,
                    total,
                    state: state.into(),
                    error,
                };
            let mut totals: HashMap<String, (i64, i64)> = ids
                .iter()
                .map(|i| (i.clone(), (0, size[i] as i64)))
                .collect();
            let sum = |t: &HashMap<String, (i64, i64)>| {
                (
                    t.values().map(|v| v.0).sum::<i64>(),
                    t.values().map(|v| v.1).sum::<i64>(),
                )
            };
            let (d, t) = sum(&totals);
            core.events.emit(ev(d, t, "running", None));
            let (ptx, mut prx) = tokio::sync::mpsc::unbounded_channel::<Value>();
            let worker = core.worker.clone();
            let ids2 = ids.clone();
            let call = tokio::spawn(async move {
                worker
                    .models_ensure(&ids2, Some(ptx), &CancelToken::new())
                    .await
            });
            while let Some(p) = prx.recv().await {
                if let (Some(m), Some(done)) = (
                    p.get("model").and_then(Value::as_str),
                    p.get("done").and_then(Value::as_i64),
                ) {
                    if let Some(e) = totals.get_mut(m) {
                        e.0 = done;
                        if let Some(t) = p.get("total").and_then(Value::as_i64) {
                            e.1 = t.max(done);
                        }
                        if p.get("phase").and_then(Value::as_str) == Some("done") {
                            e.0 = e.1;
                        }
                    }
                    let (d, t) = sum(&totals);
                    core.events.emit(ev(d, t, "running", None));
                }
            }
            let res = call.await;
            let (d, t) = sum(&totals);
            match res {
                Ok(Ok(())) => {
                    core.assistant.invalidate_info();
                    // portrait geometry built while a model was missing is stale now
                    if let Err(e) = core.refresh_partial_beauty(None).await {
                        tracing::warn!(error = %e, "refreshing partial portrait geometry failed");
                    }
                    core.events.emit(ev(t, t, "done", None))
                }
                Ok(Err(e)) => core.events.emit(ev(d, t, "failed", Some(e.to_string()))),
                Err(e) => core
                    .events
                    .emit(ev(d, t, "failed", Some(format!("task crashed: {e}")))),
            }
        });
        Ok(task_id)
    }

    // ------------------------------------------------------------ analysis run

    pub fn analysis_status(&self, session_id: i64) -> AnalysisStatus {
        self.runs
            .lock()
            .unwrap()
            .get(&session_id)
            .map(|r| r.status.clone())
            .unwrap_or_else(AnalysisStatus::idle)
    }

    /// Asks the running analysis of the session to stop after the batch in flight; it then ends
    /// `paused` and [`Core::analysis_continue`] takes it up again. 409 when none is running.
    pub fn analysis_pause(&self, session_id: i64) -> Result<()> {
        match self.runs.lock().unwrap().get(&session_id) {
            Some(r) if r.status.state == RunState::Running => {
                r.pause.store(true, Ordering::SeqCst);
                Ok(())
            }
            _ => Err(CoreError::Conflict(format!(
                "no analysis of session {session_id} is running"
            ))),
        }
    }

    /// Stops the analysis of the session. A paused one is wound up right away (what was
    /// analysed so far is grouped and scored); no-op when nothing is active.
    pub fn analysis_cancel(self: &Arc<Self>, session_id: i64) {
        let paused = match self.runs.lock().unwrap().get(&session_id) {
            Some(r) if r.status.state == RunState::Running => {
                r.cancel.cancel();
                false
            }
            Some(r) => r.status.state == RunState::Paused,
            None => false,
        };
        if paused {
            let core = self.clone();
            tokio::spawn(async move {
                if let Err(e) = core.continue_run(session_id, true).await {
                    tracing::warn!(session = session_id, error = %e, "cancelling a paused analysis failed");
                }
            });
        }
    }

    /// Takes a paused analysis up again with the photos that are left; returns its task id.
    /// 409 when the session has no paused analysis.
    pub async fn analysis_continue(self: &Arc<Self>, session_id: i64) -> Result<String> {
        self.session(session_id).await?;
        self.continue_run(session_id, false).await
    }

    /// What a run of `profile` would do now: new photos and single steps that installed models
    /// have made possible.
    pub async fn analysis_plan(&self, session_id: i64, profile: Profile) -> Result<AnalysisPlan> {
        self.session(session_id).await?;
        let spec = RunSpec::new(session_id, profile, None, false, false);
        let plan = self.plan_jobs(&spec, profile, None).await?;
        let mut steps: Vec<String> = Vec::new();
        let (mut new_photos, mut incremental_photos) = (0, 0);
        for j in &plan.jobs {
            match &j.steps {
                None => new_photos += j.refs.len() as i64,
                Some(s) => {
                    incremental_photos += j.refs.len() as i64;
                    for x in s {
                        if !steps.contains(x) {
                            steps.push(x.clone());
                        }
                    }
                }
            }
        }
        steps.sort_by_key(|s| incremental::INCREMENTAL_STEPS.iter().position(|x| x == s));
        Ok(AnalysisPlan {
            new_photos,
            incremental_photos,
            incremental_steps: steps,
        })
    }

    fn update_run(&self, session_id: i64, f: impl FnOnce(&mut AnalysisStatus)) -> AnalysisStatus {
        let mut g = self.runs.lock().unwrap();
        match g.get_mut(&session_id) {
            Some(r) => {
                f(&mut r.status);
                r.status.clone()
            }
            None => AnalysisStatus::idle(),
        }
    }

    fn emit_run(&self, session_id: i64, task_id: &str, st: &AnalysisStatus) {
        self.tasks.progress(task_id, st.done, st.total);
        self.events.emit(Event::AnalysisProgress {
            session_id,
            state: st.state,
            stage: st.stage.clone(),
            done: st.done,
            total: st.total,
        });
        self.events.emit(Event::TaskProgress {
            task_id: task_id.to_string(),
            kind: "analysis".into(),
            done: st.done,
            total: st.total,
            state: match st.state {
                RunState::Running => "running",
                RunState::Paused => "paused",
                RunState::Failed => "failed",
                // a run only ends idle when it was cancelled
                RunState::Idle => "cancelled",
                _ => "done",
            }
            .into(),
            error: st.error.clone(),
        });
    }

    /// The state of the session's run if it blocks a new one (running or paused).
    fn active_run(&self, sid: i64) -> Option<RunState> {
        self.runs
            .lock()
            .unwrap()
            .get(&sid)
            .map(|r| r.status.state)
            .filter(|s| matches!(s, RunState::Running | RunState::Paused))
    }

    fn busy_error(sid: i64, state: RunState) -> CoreError {
        CoreError::Conflict(match state {
            RunState::Paused => {
                format!("the analysis of session {sid} is paused; continue or cancel it")
            }
            _ => format!("an analysis of session {sid} is already running"),
        })
    }

    /// Photos of a run of `profile` that still need every step. `since`: photos analysed at or
    /// after that time (ms) were done by this run already.
    async fn run_refs(
        &self,
        spec: &RunSpec,
        profile: Profile,
        since: Option<i64>,
    ) -> Result<Vec<catalog::PhotoRef>> {
        let sid = spec.session_id;
        let level = profile.level();
        let (explicit, force) = (spec.photo_ids.clone(), spec.force);
        self.db
            .call(move |c| {
                let ids: Vec<i64> = match explicit {
                    Some(ids) => {
                        let mut st = c.prepare(
                            "SELECT photo_id FROM session_photo sp WHERE session_id=?1 AND photo_id=?2
                             AND (?3 IS NULL OR NOT EXISTS(SELECT 1 FROM analysis a
                                                           WHERE a.photo_id=sp.photo_id AND a.analyzed_at >= ?3))",
                        )?;
                        let mut keep = Vec::new();
                        for id in ids {
                            if st.exists(rusqlite::params![sid, id, since])? {
                                keep.push(id);
                            }
                        }
                        keep
                    }
                    None => {
                        let mut st = c.prepare(
                            "SELECT p.id FROM photo p JOIN session_photo sp ON sp.photo_id=p.id AND sp.session_id=?1
                             WHERE (?3 OR COALESCE(p.analysis_version,0) < ?2
                                    OR (?4 AND EXISTS(SELECT 1 FROM analysis a
                                                      WHERE a.photo_id=p.id AND a.profile='lite')))
                               AND (?5 IS NULL OR NOT EXISTS(SELECT 1 FROM analysis a
                                                             WHERE a.photo_id=p.id AND a.analyzed_at >= ?5))
                             ORDER BY COALESCE(p.taken_at, p.mtime), p.id",
                        )?;
                        // a lite result does not count as `fast`/`standard` analysis
                        let upgrade = profile != Profile::Lite;
                        let v = st
                            .query_map(
                                rusqlite::params![sid, level, force, upgrade, since],
                                |r| r.get(0),
                            )?
                            .collect::<rusqlite::Result<Vec<i64>>>()?;
                        v
                    }
                };
                catalog::photo_refs(c, &ids)
            })
            .await
    }

    /// Everything a run of `profile` has to do: the photos without analysis (all steps), then
    /// the photos that lack single steps an installed model enables since (`incremental`).
    async fn plan_jobs(
        &self,
        spec: &RunSpec,
        profile: Profile,
        since: Option<i64>,
    ) -> Result<RunPlan> {
        let mut plan = RunPlan::default();
        let refs = self.run_refs(spec, profile, since).await?;
        if !refs.is_empty() {
            plan.jobs.push(Job { steps: None, refs });
        }
        // only a standard run has model-backed steps that can be added later; the check is a
        // cheap query so an idle run never starts the worker just to look
        let sid = spec.session_id;
        let candidates = profile == Profile::Standard
            && !spec.force
            && spec.photo_ids.is_none()
            && self
                .db
                .call(move |c| incremental::any_candidates(c, sid, profile))
                .await?;
        if !candidates {
            return Ok(plan);
        }
        let Ok(listing) = self.worker.models_list().await else {
            return Ok(plan);
        };
        let fp = incremental::fingerprint(&listing);
        let ready = incremental::ready_steps(&listing, profile);
        plan.fp = Some(fp.clone());
        if ready.is_empty() {
            return Ok(plan);
        }
        let groups = self
            .db
            .call(move |c| incremental::stale_groups(c, sid, profile, &ready, &fp))
            .await?;
        for (steps, ids) in groups {
            let refs = self.db.call(move |c| catalog::photo_refs(c, &ids)).await?;
            if !refs.is_empty() {
                plan.jobs.push(Job {
                    steps: Some(steps),
                    refs,
                });
            }
        }
        Ok(plan)
    }

    /// The profile a run really uses and its plan: the asked-for profile, or `lite` when its
    /// models / worker are unavailable and the run allows the fallback.
    async fn resolve_plan(
        &self,
        spec: &RunSpec,
        since: Option<i64>,
        allow_download: bool,
    ) -> Result<(Profile, RunPlan)> {
        let mut profile = spec.effective.unwrap_or(spec.profile);
        let mut plan = self.plan_jobs(spec, profile, since).await?;
        if profile != Profile::Lite && !plan.jobs.is_empty() {
            if let Err(e) = self.check_models(profile, allow_download).await {
                if !(spec.fallback_lite && error_code(&e).is_some()) {
                    return Err(e);
                }
                tracing::info!(error = %e, "AI analysis unavailable; falling back to lite");
                profile = Profile::Lite;
                plan = self.plan_jobs(spec, profile, since).await?;
            }
        }
        Ok((profile, plan))
    }

    /// Validates, checks the models and starts the analysis in the background; returns the task id.
    pub async fn analysis_run(self: &Arc<Self>, req: AnalysisRunRequest) -> Result<String> {
        let sid = req.session_id;
        self.session(sid).await?; // 404
        if let Some(state) = self.active_run(sid) {
            return Err(Self::busy_error(sid, state));
        }
        if req.allow_download {
            self.require_network("downloading models")?;
        }
        let mut spec = RunSpec::new(
            sid,
            req.profile,
            req.photo_ids.clone(),
            req.force,
            req.fallback_lite,
        );
        let (profile, plan) = self.resolve_plan(&spec, None, req.allow_download).await?;
        let total = plan.total();
        spec.count = total;
        spec.effective = (profile != req.profile).then_some(profile);

        let task_id = format!("analysis-{}", self.next_task_seq());
        let cancel = CancelToken::new();
        let pause = Arc::new(AtomicBool::new(false));
        let status = AnalysisStatus {
            state: RunState::Running,
            profile: Some(profile),
            total,
            stage: Some("analyzing".into()),
            fallback_from: spec.effective.map(|_| req.profile),
            ..AnalysisStatus::idle()
        };
        self.start_run(&spec, &task_id, status.clone(), &cancel, &pause, false)
            .await?;
        let core = self.clone();
        let start = RunStart {
            spec,
            profile,
            allow_download: req.allow_download,
            plan,
            task_id: task_id.clone(),
            cancel,
            pause,
            done0: 0,
        };
        tokio::spawn(async move {
            core.run_analysis(start).await;
        });
        Ok(task_id)
    }

    /// Registers a run (in memory and in the task history) and announces it. `reopen`: the task
    /// row exists already (a resumed run) and is put back to `running`.
    async fn start_run(
        &self,
        spec: &RunSpec,
        task_id: &str,
        status: AnalysisStatus,
        cancel: &CancelToken,
        pause: &Arc<AtomicBool>,
        reopen: bool,
    ) -> Result<()> {
        let sid = spec.session_id;
        {
            let mut g = self.runs.lock().unwrap();
            if let Some(state) = g
                .get(&sid)
                .map(|r| r.status.state)
                .filter(|s| matches!(s, RunState::Running | RunState::Paused))
            {
                return Err(Self::busy_error(sid, state));
            }
            g.insert(
                sid,
                RunInfo {
                    status: status.clone(),
                    cancel: cancel.clone(),
                    task_id: task_id.to_string(),
                    pause: pause.clone(),
                    spec: spec.clone(),
                },
            );
        }
        let (tid, params, total) = (task_id.to_string(), spec.to_json(), status.total);
        let recorded = self
            .db
            .call(move |c| {
                if reopen {
                    crate::tasks::reopen(c, &tid, &params)
                } else {
                    crate::tasks::insert(c, &tid, "analysis", &params, total)
                }
            })
            .await;
        if let Err(e) = recorded {
            self.runs.lock().unwrap().remove(&sid);
            return Err(e);
        }
        // `POST /api/tasks/{id}/cancel` stops the run like `POST /api/analysis/cancel`
        self.tasks.register(task_id, cancel.clone(), status.total);
        self.tasks.progress(task_id, status.done, status.total);
        self.emit_run(sid, task_id, &status);
        Ok(())
    }

    /// Takes the paused run of a session up again; with `cancel_now` only winds it up (grouping
    /// and scoring of what was analysed).
    async fn continue_run(self: &Arc<Self>, sid: i64, cancel_now: bool) -> Result<String> {
        let (mut spec, task_id, done0, old_total) = match self.runs.lock().unwrap().get(&sid) {
            Some(r) if r.status.state == RunState::Paused => (
                r.spec.clone(),
                r.task_id.clone(),
                r.status.done,
                r.status.total,
            ),
            _ => {
                return Err(CoreError::Conflict(format!(
                    "no analysis of session {sid} is paused"
                )))
            }
        };
        let asked = spec.effective.unwrap_or(spec.profile);
        let (profile, plan) = if cancel_now {
            (asked, RunPlan::default())
        } else {
            self.resolve_plan(&spec, Some(spec.started_at), false)
                .await?
        };
        if profile != asked {
            spec.effective = Some(profile);
        }
        let total = if cancel_now {
            old_total
        } else {
            done0 + plan.total()
        };
        let cancel = CancelToken::new();
        if cancel_now {
            cancel.cancel();
        }
        let pause = Arc::new(AtomicBool::new(false));
        let status = {
            let mut g = self.runs.lock().unwrap();
            let Some(r) = g
                .get_mut(&sid)
                .filter(|r| r.status.state == RunState::Paused && r.task_id == task_id)
            else {
                return Err(CoreError::Conflict(format!(
                    "no analysis of session {sid} is paused"
                )));
            };
            r.status.state = RunState::Running;
            r.status.stage = Some("analyzing".into());
            r.status.error = None;
            r.status.error_code = None;
            r.status.profile = Some(profile);
            r.status.total = total;
            if profile != spec.profile {
                r.status.fallback_from = Some(spec.profile);
            }
            r.cancel = cancel.clone();
            r.pause = pause.clone();
            r.spec = spec.clone();
            r.status.clone()
        };
        // the entry is claimed (state `running`); the task row follows
        let (tid, params) = (task_id.clone(), spec.to_json());
        if let Err(e) = self
            .db
            .call(move |c| crate::tasks::reopen(c, &tid, &params))
            .await
        {
            tracing::warn!(task = %task_id, error = %e, "recording the continued analysis failed");
        }
        self.tasks.register(&task_id, cancel.clone(), total);
        self.emit_run(sid, &task_id, &status);
        let core = self.clone();
        let start = RunStart {
            spec,
            profile,
            allow_download: false,
            plan,
            task_id: task_id.clone(),
            cancel,
            pause,
            done0,
        };
        tokio::spawn(async move {
            core.run_analysis(start).await;
        });
        Ok(task_id)
    }

    /// Picks up the analysis runs the previous process left behind (call once the application
    /// is up; the server does when it builds its router): a run that was cut off by an exit is
    /// resumed with the photos that are left, a paused one comes back paused. Needs a Tokio
    /// runtime; without one the runs stay pending.
    pub fn resume_analyses(self: &Arc<Self>) {
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let open = std::mem::take(&mut *self.open_analyses.lock().unwrap());
        for o in open {
            let core = self.clone();
            rt.spawn(async move { core.restore_run(o).await });
        }
    }

    async fn restore_run(self: &Arc<Self>, o: crate::tasks::OpenAnalysis) {
        let Ok(mut spec) = serde_json::from_value::<RunSpec>(o.params.clone()) else {
            return;
        };
        let sid = spec.session_id;
        if self.session(sid).await.is_err() || self.active_run(sid).is_some() {
            return;
        }
        let asked = spec.effective.unwrap_or(spec.profile);
        if o.paused {
            let status = AnalysisStatus {
                state: RunState::Paused,
                profile: Some(asked),
                done: o.done,
                total: o.total,
                fallback_from: spec.effective.map(|_| spec.profile),
                resumed: true,
                ..AnalysisStatus::idle()
            };
            self.runs.lock().unwrap().insert(
                sid,
                RunInfo {
                    status: status.clone(),
                    cancel: CancelToken::new(),
                    task_id: o.id.clone(),
                    pause: Arc::new(AtomicBool::new(false)),
                    spec,
                },
            );
            self.emit_run(sid, &o.id, &status);
            return;
        }
        // a run that keeps taking the process down is not started again and again
        if spec.resumes >= MAX_RESUMES {
            tracing::warn!(task = %o.id, "analysis interrupted too often; not resuming");
            return;
        }
        spec.resumes += 1;
        let resolved = self.resolve_plan(&spec, Some(spec.started_at), false).await;
        let (profile, plan) = match resolved {
            Ok(r) => r,
            Err(e) => {
                // e.g. the AI components are gone: a failed run the UI can offer `lite` for
                self.fail_restored(&o, &spec, &e).await;
                return;
            }
        };
        if profile != asked {
            spec.effective = Some(profile);
        }
        let remaining = plan.total();
        let done0 = (o.total - remaining).max(0);
        let status = AnalysisStatus {
            state: RunState::Running,
            profile: Some(profile),
            done: done0,
            total: done0 + remaining,
            stage: Some("analyzing".into()),
            fallback_from: spec.effective.map(|_| spec.profile),
            resumed: true,
            ..AnalysisStatus::idle()
        };
        let cancel = CancelToken::new();
        let pause = Arc::new(AtomicBool::new(false));
        if let Err(e) = self
            .start_run(&spec, &o.id, status, &cancel, &pause, true)
            .await
        {
            tracing::warn!(task = %o.id, error = %e, "resuming the analysis failed");
            return;
        }
        tracing::info!(task = %o.id, session = sid, remaining, "resuming an interrupted analysis");
        self.run_analysis(RunStart {
            spec,
            profile,
            allow_download: false,
            plan,
            task_id: o.id.clone(),
            cancel,
            pause,
            done0,
        })
        .await;
    }

    /// Records a run that could not be resumed as failed.
    async fn fail_restored(&self, o: &crate::tasks::OpenAnalysis, spec: &RunSpec, e: &CoreError) {
        let sid = spec.session_id;
        let msg = e.to_string();
        let status = AnalysisStatus {
            state: RunState::Failed,
            profile: Some(spec.effective.unwrap_or(spec.profile)),
            done: o.done,
            total: o.total,
            error: Some(msg.clone()),
            error_code: error_code(e),
            resumed: true,
            ..AnalysisStatus::idle()
        };
        self.runs.lock().unwrap().insert(
            sid,
            RunInfo {
                status: status.clone(),
                cancel: CancelToken::new(),
                task_id: o.id.clone(),
                pause: Arc::new(AtomicBool::new(false)),
                spec: spec.clone(),
            },
        );
        let (tid, done, total) = (o.id.clone(), o.done, o.total);
        let _ = self
            .db
            .call(move |c| crate::tasks::finish(c, &tid, "failed", done, total, Some(&msg)))
            .await;
        self.emit_run(sid, &o.id, &status);
    }

    /// 409 `models_missing` unless every model the profile needs is installed (or downloads are allowed).
    async fn check_models(&self, profile: Profile, allow_download: bool) -> Result<()> {
        let listing = self.worker.models_list().await.map_err(map_worker_err)?;
        let needed: Vec<String> = match listing.profiles.get(profile.as_str()) {
            Some(p) => p.models.clone(),
            None => listing
                .models
                .iter()
                .filter(|m| !m.optional && !m.required_for.is_empty() && m.recommended)
                .map(|m| m.id.clone())
                .collect(),
        };
        let installed: HashSet<&str> = listing
            .models
            .iter()
            .filter(|m| m.installed)
            .map(|m| m.id.as_str())
            .collect();
        let missing: Vec<String> = needed
            .into_iter()
            .filter(|m| !installed.contains(m.as_str()))
            .collect();
        if !missing.is_empty() && !allow_download {
            return Err(CoreError::ModelsMissing(missing));
        }
        Ok(())
    }

    async fn run_analysis(self: &Arc<Self>, run: RunStart) {
        let RunStart {
            mut spec,
            mut profile,
            allow_download,
            plan,
            task_id,
            cancel,
            pause,
            done0,
        } = run;
        let sid = spec.session_id;
        // one analysis at a time (the worker owns the GPU); others wait here
        let _permit = self.analysis_gate.acquire().await.ok();
        let started = Instant::now();
        let out_dir = self.dirs.root.join("cache").join("analysis").join(&task_id);
        let _ = std::fs::create_dir_all(&out_dir);
        let total = self.analysis_status(sid).total;
        // faces switched off or not agreed to yet: no detection, no identity embeddings
        let faces_allowed = self.settings().faces.allowed();
        let mut steps = self.analysis_steps(profile).await;
        let mut done: i64 = done0;
        let mut failed: Vec<String> = Vec::new();
        let mut fatal: Option<String> = None;
        let mut fatal_code: Option<String> = None;
        let mut skipped: Vec<String> = Vec::new();
        let mut ingested_any = false;
        let mut paused = false;
        let fp = plan.fp.clone().unwrap_or_default();

        'jobs: for job in &plan.jobs {
            // the AI is gone for this run: single steps cannot be redone by lite
            if job.steps.is_some() && profile == Profile::Lite {
                continue;
            }
            let mut at = 0;
            while at < job.refs.len() {
                let chunk = &job.refs[at..(at + BATCH_SIZE).min(job.refs.len())];
                if cancel.is_cancelled() {
                    break 'jobs;
                }
                if pause.load(Ordering::SeqCst) {
                    paused = true;
                    break 'jobs;
                }
                let areq = AnalyzeRequest {
                    items: chunk
                        .iter()
                        .map(|r| AnalyzeRequestItem {
                            photo_id: r.id,
                            path: r.path.to_string_lossy().into_owned(),
                            orientation: r.orientation,
                        })
                        .collect(),
                    profile: Some(profile.as_str().to_string()),
                    steps: job.steps.clone().or_else(|| steps.clone()),
                    analysis_size: ANALYSIS_SIZE,
                    out_dir: out_dir.to_string_lossy().into_owned(),
                    allow_download,
                };
                let mut attempt = 0;
                let resp = loop {
                    let (ptx, mut prx) = tokio::sync::mpsc::unbounded_channel::<Value>();
                    let (core, base) = (self.clone(), done);
                    let tid = task_id.clone();
                    let fwd = tokio::spawn(async move {
                        while let Some(p) = prx.recv().await {
                            if p.get("kind").and_then(Value::as_str) == Some("analyze") {
                                if let Some(d) = p.get("done").and_then(Value::as_i64) {
                                    let st =
                                        core.update_run(sid, |s| s.done = (base + d).min(s.total));
                                    core.emit_run(sid, &tid, &st);
                                }
                            }
                        }
                    });
                    let r = if profile == Profile::Lite {
                        self.lite.analyze_batch(&areq, Some(ptx), &cancel).await
                    } else {
                        self.worker.analyze_batch(&areq, Some(ptx), &cancel).await
                    };
                    let _ = fwd.await;
                    match r {
                        Err(WorkerError::Disconnected)
                            if attempt < MAX_BATCH_RETRIES && !cancel.is_cancelled() =>
                        {
                            attempt += 1;
                            tracing::warn!(attempt, "worker crashed during a batch; retrying");
                            continue;
                        }
                        other => break other,
                    }
                };
                let mut resp = match resp {
                    Ok(r) => r,
                    Err(WorkerError::Cancelled) => break 'jobs,
                    Err(e) => {
                        let err = map_worker_err(e);
                        if spec.fallback_lite
                            && profile != Profile::Lite
                            && job.steps.is_none()
                            && error_code(&err).is_some()
                        {
                            // the AI components went away mid-run: the rest is done on-device
                            tracing::warn!(error = %err, "AI analysis failed; continuing with lite");
                            profile = Profile::Lite;
                            steps = self.analysis_steps(profile).await;
                            spec.effective = Some(profile);
                            let asked = spec.profile;
                            self.update_run(sid, |s| {
                                s.profile = Some(Profile::Lite);
                                s.fallback_from = Some(asked);
                            });
                            let (tid, params) = (task_id.clone(), spec.to_json());
                            let _ = self
                                .db
                                .call(move |c| crate::tasks::set_params(c, &tid, &params))
                                .await;
                            continue;
                        }
                        fatal_code = error_code(&err);
                        fatal = Some(match err {
                            CoreError::ModelsMissing(m) => {
                                format!("models missing: {}", m.join(", "))
                            }
                            other => other.to_string(),
                        });
                        break 'jobs;
                    }
                };
                if !faces_allowed {
                    // whatever the worker did, nothing about faces is stored without consent
                    for it in &mut resp.items {
                        it.faces.clear();
                        it.identity_file = None;
                    }
                }
                for s in &resp.skipped_steps {
                    if !skipped.contains(s) {
                        skipped.push(s.clone());
                    }
                }
                for w in &resp.warnings {
                    tracing::info!(warning = %w, "worker warning");
                }
                let mut ok_items: Vec<AnalyzeItem> = Vec::new();
                for it in resp.items {
                    match &it.error {
                        Some(e) => failed.push(format!("photo {}: {}", it.photo_id, e.message)),
                        None => ok_items.push(it),
                    }
                }
                let ids: Vec<i64> = ok_items.iter().map(|i| i.photo_id).collect();
                // a skipped step is remembered with the model set it was skipped under, so it
                // is not retried until a model is installed (see `incremental`)
                let mut versions = resp.models.clone();
                if !resp.skipped_steps.is_empty() {
                    if !versions.is_object() {
                        versions = json!({});
                    }
                    versions[incremental::SKIPPED_KEY] = Value::Object(
                        resp.skipped_steps
                            .iter()
                            .map(|s| (s.clone(), Value::String(fp.clone())))
                            .collect(),
                    );
                }
                let partial = job.steps.clone();
                let dir = out_dir.to_string_lossy().into_owned();
                let res = tokio::task::spawn_blocking({
                    let db = self.db.clone();
                    move || {
                        let items: Vec<IngestItem> =
                            ok_items.into_iter().map(load_ingest).collect();
                        db.with(|c| match &partial {
                            Some(st) => store::ingest_partial(c, &items, st, &versions),
                            None => store::ingest(c, &items, profile, &versions, &dir),
                        })
                    }
                })
                .await;
                match res {
                    Ok(Ok(())) => {
                        ingested_any |= !ids.is_empty();
                        self.events.emit(Event::AnalysisUpdated {
                            session_id: sid,
                            ids,
                        });
                    }
                    Ok(Err(e)) => {
                        fatal = Some(format!("storing results failed: {e}"));
                        break 'jobs;
                    }
                    Err(e) => {
                        fatal = Some(format!("ingest task crashed: {e}"));
                        break 'jobs;
                    }
                }
                done += chunk.len() as i64;
                at += chunk.len();
                let st = self.update_run(sid, |s| {
                    s.done = done.min(s.total);
                    s.skipped_steps = skipped.clone();
                    s.incremental += if job.steps.is_some() {
                        chunk.len() as i64
                    } else {
                        0
                    };
                });
                self.emit_run(sid, &task_id, &st);
            }
        }
        let t_analyze = started.elapsed();
        let _ = std::fs::remove_dir_all(&out_dir);

        // ---- paused: stop here, the rest waits for `continue` (no grouping yet)
        if paused && !cancel.is_cancelled() {
            // the task row is `paused` before anybody can see the state: an exit right after
            // the status turns `paused` must still find a paused run
            let cur = self.analysis_status(sid);
            let (tid, params, d, t) = (task_id.clone(), spec.to_json(), cur.done, cur.total);
            let _ = self
                .db
                .call(move |c| {
                    crate::tasks::set_params(c, &tid, &params)?;
                    crate::tasks::pause(c, &tid, d, t)
                })
                .await;
            self.tasks.remove(&task_id);
            // `continue` starts from the profile the run ended up with (after a fallback)
            if let Some(r) = self.runs.lock().unwrap().get_mut(&sid) {
                r.spec = spec.clone();
            }
            let st = self.update_run(sid, |s| {
                s.state = RunState::Paused;
                s.stage = None;
                s.skipped_steps = skipped.clone();
            });
            self.emit_run(sid, &task_id, &st);
            tracing::info!(session = sid, done = d, total = t, "analysis paused");
            return;
        }

        // ---- finishing stages over the whole session
        let had_data = ingested_any
            || plan.jobs.is_empty()
            || {
                let n: i64 = self
                .db
                .call(move |c| {
                    Ok(c.query_row(
                        "SELECT COUNT(*) FROM analysis a JOIN session_photo sp ON sp.photo_id=a.photo_id WHERE sp.session_id=?1",
                        [sid],
                        |r| r.get(0),
                    )?)
                })
                .await
                .unwrap_or(0);
                n > 0
            };
        let mut finish_err: Option<String> = None;
        if had_data {
            if let Err(e) = self.finish_stages(sid, &task_id).await {
                finish_err = Some(format!("{e}"));
            }
        }
        tracing::info!(
            session = sid,
            analyzed = done,
            analyze_ms = t_analyze.as_millis() as u64,
            total_ms = started.elapsed().as_millis() as u64,
            "analysis finished"
        );

        let cancelled = cancel.is_cancelled();
        let error = fatal.clone().or(finish_err).or_else(|| {
            (!failed.is_empty()).then(|| {
                format!(
                    "{} of {} photos could not be analyzed; first: {}",
                    failed.len(),
                    total,
                    failed[0]
                )
            })
        });
        let state = if cancelled {
            RunState::Idle
        } else if fatal.is_some() || (error.is_some() && failed.len() as i64 >= total && total > 0)
        {
            RunState::Failed
        } else {
            RunState::Done
        };
        let st = self.update_run(sid, |s| {
            s.state = state;
            s.stage = None;
            s.error = error.clone();
            s.error_code = if state == RunState::Failed {
                fatal_code.clone()
            } else {
                None
            };
            s.skipped_steps = skipped.clone();
            if state == RunState::Done {
                s.done = s.total;
            }
        });
        let (tid, status, err, done, total) = (
            task_id.clone(),
            match state {
                RunState::Failed => "failed",
                RunState::Idle => "cancelled",
                _ => "done",
            },
            error,
            st.done,
            st.total,
        );
        let _ = self
            .db
            .call(move |c| crate::tasks::finish(c, &tid, status, done, total, err.as_deref()))
            .await;
        self.emit_run(sid, &task_id, &st);
        self.tasks.remove(&task_id);
    }

    /// Grouping, scoring, clustering and the final refresh events.
    async fn finish_stages(self: &Arc<Self>, sid: i64, task_id: &str) -> Result<()> {
        let stage = |name: &str| {
            let st = self.update_run(sid, |s| s.stage = Some(name.to_string()));
            self.emit_run(sid, task_id, &st);
        };
        stage("grouping");
        let group_params = self.settings().group_params();
        let t = Instant::now();
        let burst_ids = self
            .db
            .call(move |c| store::regroup_session(c, sid, &group_params))
            .await?;
        tracing::info!(
            ms = t.elapsed().as_millis() as u64,
            bursts = burst_ids.len(),
            "grouped"
        );

        stage("clustering");
        // cluster before scoring so closed-eye issues name the right people and subjects are final
        let t = Instant::now();
        let outcome = self
            .db
            .call(move |c| store::cluster_session(c, sid))
            .await?;
        tracing::info!(ms = t.elapsed().as_millis() as u64, "clustered");

        stage("scoring");
        let t = Instant::now();
        let ids2 = burst_ids.clone();
        self.db
            .call(move |c| store::rescore_bursts(c, &ids2).map(|_| ()))
            .await?;
        tracing::info!(ms = t.elapsed().as_millis() as u64, "scored");
        let _ = outcome;

        self.events.emit(Event::GroupsUpdated { session_id: sid });
        self.events.emit(Event::PeopleUpdated { session_id: sid });
        let all_ids: Vec<i64> = self
            .db
            .call(move |c| {
                let mut st = c.prepare(
                    "SELECT photo_id FROM session_photo sp JOIN analysis a USING(photo_id) WHERE sp.session_id=?1",
                )?;
                let v = st
                    .query_map([sid], |r| r.get(0))?
                    .collect::<rusqlite::Result<Vec<i64>>>()?;
                Ok(v)
            })
            .await?;
        for chunk in all_ids.chunks(5000) {
            self.events.emit(Event::AnalysisUpdated {
                session_id: sid,
                ids: chunk.to_vec(),
            });
        }
        Ok(())
    }

    // ------------------------------------------------------------ reads

    pub async fn photo_analysis(&self, photo_id: i64) -> Result<AnalysisDetail> {
        self.db
            .call(move |c| store::analysis_detail(c, photo_id))
            .await
    }

    pub async fn groups(&self, session_id: i64) -> Result<GroupsOut> {
        self.db
            .call(move |c| store::groups_of_session(c, session_id))
            .await
    }

    pub async fn burst_faces(&self, burst_id: i64) -> Result<BurstFacesOut> {
        self.db.call(move |c| store::burst_faces(c, burst_id)).await
    }

    /// People without the single-photo subjects (`singleton`).
    pub async fn people(&self, session_id: Option<i64>) -> Result<Vec<Person>> {
        self.people_with(session_id, false).await
    }

    /// `GET /api/people`; `include_singletons` adds the single-photo subjects.
    pub async fn people_with(
        &self,
        session_id: Option<i64>,
        include_singletons: bool,
    ) -> Result<Vec<Person>> {
        if let Some(s) = session_id {
            self.session(s).await?;
        }
        if !self.settings().faces.enabled {
            return Ok(Vec::new());
        }
        self.db
            .call(move |c| store::list_people(c, session_id, include_singletons))
            .await
    }

    // ------------------------------------------------------------ manual group edits

    async fn after_group_edit(&self, session_id: i64, burst_ids: Vec<i64>) -> Result<()> {
        let ids = burst_ids.clone();
        let touched = self
            .db
            .call(move |c| {
                for b in &ids {
                    store::retrack_burst(c, *b)?;
                }
                store::rescore_bursts(c, &ids)
            })
            .await?;
        self.events.emit(Event::AnalysisUpdated {
            session_id,
            ids: touched,
        });
        self.events.emit(Event::GroupsUpdated { session_id });
        Ok(())
    }

    pub async fn split_burst(&self, req: SplitRequest) -> Result<[i64; 2]> {
        let out = self
            .db
            .call(move |c| store::split_burst(c, req.burst_id, req.at_photo_id))
            .await?;
        self.after_group_edit(out.session_id, out.burst_ids.to_vec())
            .await?;
        Ok(out.burst_ids)
    }

    pub async fn merge_bursts(&self, req: MergeBurstsRequest) -> Result<i64> {
        let out = self
            .db
            .call(move |c| store::merge_bursts(c, &req.burst_ids))
            .await?;
        self.after_group_edit(out.session_id, vec![out.burst_id])
            .await?;
        Ok(out.burst_id)
    }

    // ------------------------------------------------------------ accept AI

    /// `user_rating = round(ai_rating)` for analysed photos among `ids`.
    pub async fn accept_ai(self: &Arc<Self>, ids: Vec<i64>) -> Result<usize> {
        if ids.is_empty() {
            return Err(CoreError::bad_request("ids must not be empty"));
        }
        let (updates, labels): (Vec<PhotoUpdate>, usize) = self
            .db
            .call(move |c| {
                let updates = store::accept_ai(c, &ids)?;
                let labels = crate::taste::record_accept(c, &updates)?;
                Ok((updates, labels))
            })
            .await?;
        self.taste_after_labels(labels);
        let n = updates.len();
        if n > 0 {
            let ids: Vec<i64> = updates.iter().map(|u| u.id).collect();
            self.events.emit(Event::PhotosUpdated { items: updates });
            self.xmp_touch(&ids);
        }
        Ok(n)
    }

    /// Explicit worker steps of `profile` when faces are switched off or not agreed to yet
    /// (`faces.consented`, docs/02 §8); `None` = the worker's own profile.
    pub(crate) async fn analysis_steps(&self, profile: Profile) -> Option<Vec<String>> {
        if self.settings().faces.allowed() {
            return None;
        }
        if profile == Profile::Lite {
            return Some(
                crate::lite::LITE_STEPS
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            );
        }
        let fallback: &[&str] = match profile {
            Profile::Lite | Profile::Fast => &["phash", "quality"],
            Profile::Standard => &["phash", "quality", "embed", "aesthetic", "iqa", "scene"],
        };
        let listed = self
            .worker
            .models_list()
            .await
            .ok()
            .and_then(|l| l.profiles.get(profile.as_str()).map(|p| p.steps.clone()))
            .filter(|s| !s.is_empty());
        let steps: Vec<String> = match listed {
            Some(s) => s,
            None => fallback.iter().map(|s| s.to_string()).collect(),
        };
        Some(
            steps
                .into_iter()
                .filter(|s| s != "faces" && s != "identity")
                .collect(),
        )
    }

    // ------------------------------------------------------------ people

    async fn after_people_change(
        &self,
        person_ids: Vec<i64>,
        photo_ids: Option<Vec<i64>>,
    ) -> Result<()> {
        // names change who counts as a subject; rescore what changed
        let pids = person_ids.clone();
        let (sessions, changed) = self
            .db
            .call(move |c| {
                let mut photos = photo_ids.unwrap_or_default();
                for p in &pids {
                    photos.extend(store::photos_of_person(c, *p)?);
                }
                photos.sort_unstable();
                photos.dedup();
                let changed = store::recompute_subjects(c, &photos)?;
                let mut sessions = store::sessions_of_people(c, &pids)?;
                sessions.extend(store::sessions_of_photos(c, &photos)?);
                sessions.sort_unstable();
                sessions.dedup();
                if !changed.is_empty() {
                    let mut bursts = Vec::new();
                    for chunk in changed.chunks(500) {
                        let mut st = c.prepare(&format!(
                            "SELECT DISTINCT burst_id FROM photo WHERE burst_id IS NOT NULL AND id IN ({})",
                            vec!["?"; chunk.len()].join(",")
                        ))?;
                        let v = st
                            .query_map(rusqlite::params_from_iter(chunk.iter()), |r| r.get::<_, i64>(0))?
                            .collect::<rusqlite::Result<Vec<_>>>()?;
                        bursts.extend(v);
                    }
                    store::rescore_bursts(c, &bursts)?;
                }
                Ok((sessions, changed))
            })
            .await?;
        for s in sessions {
            self.events.emit(Event::PeopleUpdated { session_id: s });
            if !changed.is_empty() {
                self.events.emit(Event::AnalysisUpdated {
                    session_id: s,
                    ids: changed.clone(),
                });
            }
        }
        Ok(())
    }

    pub async fn patch_person(&self, id: i64, patch: PersonPatch) -> Result<Person> {
        let renamed = patch.name.is_some();
        let p = self
            .db
            .call(move |c| store::patch_person(c, id, &patch))
            .await?;
        if renamed {
            self.after_people_change(vec![id], None).await?;
        } else {
            let sessions = self
                .db
                .call(move |c| store::sessions_of_people(c, &[id]))
                .await?;
            for s in sessions {
                self.events.emit(Event::PeopleUpdated { session_id: s });
            }
        }
        Ok(p)
    }

    /// `GET /api/people/{id}/faces`: ids of the faces assigned to the person.
    pub async fn person_face_ids(&self, id: i64) -> Result<Vec<i64>> {
        self.db.call(move |c| store::person_face_ids(c, id)).await
    }

    pub async fn merge_people(&self, req: MergePeopleRequest) -> Result<Person> {
        let (ids, into) = (req.ids.clone(), req.into);
        let sessions_before = {
            let mut all = ids.clone();
            all.push(into);
            self.db
                .call(move |c| store::sessions_of_people(c, &all))
                .await?
        };
        let p = self
            .db
            .call(move |c| store::merge_people(c, &ids, into))
            .await?;
        self.after_people_change(vec![into], None).await?;
        for s in sessions_before {
            self.events.emit(Event::PeopleUpdated { session_id: s });
        }
        Ok(p)
    }

    pub async fn set_face_person(&self, face_id: i64, person_id: Option<i64>) -> Result<Face> {
        let (face, photo_id, touched) = self
            .db
            .call(move |c| store::set_face_person(c, face_id, person_id))
            .await?;
        self.after_people_change(touched, Some(vec![photo_id]))
            .await?;
        Ok(face)
    }

    /// `POST /api/people`: "this is a new person" from faces picked in search by face.
    pub async fn create_person(&self, req: CreatePersonRequest) -> Result<Person> {
        let out = self
            .db
            .call(move |c| store::create_person_from_faces(c, &req.face_ids, req.name.as_deref()))
            .await?;
        let mut touched = vec![out.person.id];
        touched.extend(out.left_people);
        self.after_people_change(touched, Some(out.photo_ids))
            .await?;
        Ok(out.person)
    }

    // ------------------------------------------------------------ face crop

    /// JPEG of the face with 40% margin, square, `size` px (128 or 256), cached on disk.
    pub async fn face_crop(&self, face_id: i64, size: u32) -> Result<PathBuf> {
        if size != 128 && size != 256 {
            return Err(CoreError::bad_request("s must be 128 or 256"));
        }
        let (photo_id, bbox) = self
            .db
            .call(move |c| store::face_crop_ref(c, face_id))
            .await?;
        let preview = self.thumbs.ensure(photo_id, 2048).await?;
        let dir = self.dirs.root.join("cache").join("faces");
        let stem = preview
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let key = format!("{face_id}_{size}_{stem}.jpg");
        let out = dir.join(key);
        if out.is_file() {
            return Ok(out);
        }
        let out2 = out.clone();
        tokio::task::spawn_blocking(move || crop_face(&preview, bbox, size, &out2))
            .await
            .map_err(|e| CoreError::Internal(anyhow::anyhow!("crop task failed: {e}")))??;
        Ok(out)
    }

    /// Forwards worker state changes to the event bus (called from `Core::open`).
    pub(crate) fn spawn_worker_status_forwarder(self_: &Arc<Self>) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let events = self_.events.clone();
        let mut rx = self_.worker.subscribe();
        let init = rx.borrow().clone();
        tokio::spawn(async move {
            let mut last: Option<(WorkerState, Option<String>)> = Some((init.state, init.error));
            loop {
                let st = rx.borrow_and_update().clone();
                let key = (st.state, st.error.clone());
                if last.as_ref() != Some(&key) {
                    last = Some(key);
                    events.emit(Event::WorkerStatus {
                        state: st.state.as_str().to_string(),
                        tier: st.tier.clone(),
                        error: st.error.clone(),
                    });
                }
                if rx.changed().await.is_err() {
                    break;
                }
            }
        });
    }
}

fn crop_face(preview: &Path, bbox: [f64; 4], size: u32, out: &Path) -> Result<()> {
    let img = image::open(preview)
        .map_err(|e| CoreError::Internal(anyhow::anyhow!("decode preview: {e}")))?
        .to_rgb8();
    let (w, h) = (img.width() as f64, img.height() as f64);
    let (cx, cy) = ((bbox[0] + bbox[2] / 2.0) * w, (bbox[1] + bbox[3] / 2.0) * h);
    let side = ((bbox[2] * w).max(bbox[3] * h) * 1.4).clamp(8.0, w.min(h));
    let x0 = (cx - side / 2.0).clamp(0.0, w - side);
    let y0 = (cy - side / 2.0).clamp(0.0, h - side);
    let crop =
        image::imageops::crop_imm(&img, x0 as u32, y0 as u32, side as u32, side as u32).to_image();
    let resized = image::imageops::resize(&crop, size, size, image::imageops::FilterType::Triangle);
    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p)?;
    }
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 85)
        .encode_image(&resized)
        .map_err(|e| CoreError::Internal(anyhow::anyhow!("encode crop: {e}")))?;
    let tmp = out.with_extension("tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, out)?;
    Ok(())
}
