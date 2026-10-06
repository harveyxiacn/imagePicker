//! The analysis task and the `Core` API around it (worker, models, groups, people).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use ip_worker_client::{
    AnalyzeItem, AnalyzeRequest, AnalyzeRequestItem, CancelToken, WorkerError, WorkerState,
};
use serde_json::{json, Value};

use super::grouping::GroupParams;
use super::store::{self, IngestItem};
use super::types::*;
use super::vecs::{decode_f16, normalize, parse_npy};
use crate::catalog::{self, now_ms};
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

/// Per-session run bookkeeping.
pub struct RunInfo {
    pub status: AnalysisStatus,
    pub cancel: CancelToken,
    pub task_id: String,
}

pub(crate) fn map_worker_err(e: WorkerError) -> CoreError {
    if let Some(models) = e.model_unavailable() {
        return CoreError::ModelsMissing(models);
    }
    match e {
        WorkerError::Unavailable(m) => CoreError::WorkerUnavailable(m),
        WorkerError::Timeout(m) => CoreError::WorkerUnavailable(m),
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
            self.worker.system_info().await.map_err(map_worker_err)?;
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
                Ok(Ok(())) => core.events.emit(ev(t, t, "done", None)),
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

    pub fn analysis_cancel(&self, session_id: i64) {
        if let Some(r) = self.runs.lock().unwrap().get(&session_id) {
            if r.status.state == RunState::Running {
                r.cancel.cancel();
            }
        }
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
                RunState::Failed => "failed",
                _ => "done",
            }
            .into(),
            error: st.error.clone(),
        });
    }

    /// Validates, checks the models and starts the analysis in the background; returns the task id.
    pub async fn analysis_run(self: &Arc<Self>, req: AnalysisRunRequest) -> Result<String> {
        let sid = req.session_id;
        self.session(sid).await?; // 404
        if self
            .runs
            .lock()
            .unwrap()
            .get(&sid)
            .map(|r| r.status.state == RunState::Running)
            .unwrap_or(false)
        {
            return Err(CoreError::Conflict(format!(
                "an analysis of session {sid} is already running"
            )));
        }
        let level = req.profile.level();
        let (explicit, force) = (req.photo_ids.clone(), req.force);
        let refs = self
            .db
            .call(move |c| {
                let ids: Vec<i64> = match explicit {
                    Some(ids) => {
                        let mut st = c.prepare(
                            "SELECT photo_id FROM session_photo WHERE session_id=?1 AND photo_id=?2",
                        )?;
                        let mut keep = Vec::new();
                        for id in ids {
                            if st.exists(rusqlite::params![sid, id])? {
                                keep.push(id);
                            }
                        }
                        keep
                    }
                    None => {
                        let mut st = c.prepare(
                            "SELECT p.id FROM photo p JOIN session_photo sp ON sp.photo_id=p.id AND sp.session_id=?1
                             WHERE (?3 OR COALESCE(p.analysis_version,0) < ?2)
                             ORDER BY COALESCE(p.taken_at, p.mtime), p.id",
                        )?;
                        let v = st
                            .query_map(rusqlite::params![sid, level, force], |r| r.get(0))?
                            .collect::<rusqlite::Result<Vec<i64>>>()?;
                        v
                    }
                };
                catalog::photo_refs(c, &ids)
            })
            .await?;

        if !refs.is_empty() {
            self.check_models(req.profile, req.allow_download).await?;
        }

        let task_id = format!("analysis-{}", self.next_task_seq());
        let cancel = CancelToken::new();
        let status = AnalysisStatus {
            state: RunState::Running,
            profile: Some(req.profile),
            done: 0,
            total: refs.len() as i64,
            stage: Some("analyzing".into()),
            error: None,
            skipped_steps: vec![],
        };
        {
            let mut g = self.runs.lock().unwrap();
            if g.get(&sid)
                .map(|r| r.status.state == RunState::Running)
                .unwrap_or(false)
            {
                return Err(CoreError::Conflict(format!(
                    "an analysis of session {sid} is already running"
                )));
            }
            g.insert(
                sid,
                RunInfo {
                    status: status.clone(),
                    cancel: cancel.clone(),
                    task_id: task_id.clone(),
                },
            );
        }
        {
            let (tid, params) = (
                task_id.clone(),
                json!({"session_id": sid, "profile": req.profile.as_str(), "count": refs.len()})
                    .to_string(),
            );
            self.db
                .call(move |c| {
                    c.execute(
                        "INSERT INTO task(id, kind, status, priority, params, progress, created_at, updated_at)
                         VALUES(?1,'analysis','running',0,?2,0,?3,?3)",
                        rusqlite::params![tid, params, now_ms()],
                    )?;
                    Ok(())
                })
                .await?;
        }
        self.emit_run(sid, &task_id, &status);
        let core = self.clone();
        let tid = task_id.clone();
        tokio::spawn(async move {
            core.run_analysis(req, refs, tid, cancel).await;
        });
        Ok(task_id)
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

    async fn run_analysis(
        self: &Arc<Self>,
        req: AnalysisRunRequest,
        refs: Vec<catalog::PhotoRef>,
        task_id: String,
        cancel: CancelToken,
    ) {
        let sid = req.session_id;
        // one analysis at a time (the worker owns the GPU); others wait here
        let _permit = self.analysis_gate.acquire().await.ok();
        let started = Instant::now();
        let out_dir = self.dirs.root.join("cache").join("analysis").join(&task_id);
        let _ = std::fs::create_dir_all(&out_dir);
        let total = refs.len() as i64;
        let mut done: i64 = 0;
        let mut failed: Vec<String> = Vec::new();
        let mut fatal: Option<String> = None;
        let mut skipped: Vec<String> = Vec::new();
        let mut ingested_any = false;

        for chunk in refs.chunks(BATCH_SIZE) {
            if cancel.is_cancelled() {
                break;
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
                profile: Some(req.profile.as_str().to_string()),
                steps: None,
                analysis_size: ANALYSIS_SIZE,
                out_dir: out_dir.to_string_lossy().into_owned(),
                allow_download: req.allow_download,
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
                                let st = core.update_run(sid, |s| s.done = (base + d).min(s.total));
                                core.emit_run(sid, &tid, &st);
                            }
                        }
                    }
                });
                let r = self.worker.analyze_batch(&areq, Some(ptx), &cancel).await;
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
            let resp = match resp {
                Ok(r) => r,
                Err(WorkerError::Cancelled) => break,
                Err(e) => {
                    fatal = Some(match map_worker_err(e) {
                        CoreError::ModelsMissing(m) => format!("models missing: {}", m.join(", ")),
                        other => other.to_string(),
                    });
                    break;
                }
            };
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
            let versions = resp.models.clone();
            let profile = req.profile;
            let dir = out_dir.to_string_lossy().into_owned();
            let res = tokio::task::spawn_blocking({
                let db = self.db.clone();
                move || {
                    let items: Vec<IngestItem> = ok_items.into_iter().map(load_ingest).collect();
                    db.with(|c| store::ingest(c, &items, profile, &versions, &dir))
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
                    break;
                }
                Err(e) => {
                    fatal = Some(format!("ingest task crashed: {e}"));
                    break;
                }
            }
            done += chunk.len() as i64;
            let st = self.update_run(sid, |s| {
                s.done = done.min(s.total);
                s.skipped_steps = skipped.clone();
            });
            self.emit_run(sid, &task_id, &st);
        }
        let t_analyze = started.elapsed();
        let _ = std::fs::remove_dir_all(&out_dir);

        // ---- finishing stages over the whole session
        let had_data = ingested_any
            || refs.is_empty()
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
            s.skipped_steps = skipped.clone();
            if state == RunState::Done {
                s.done = s.total;
            }
        });
        let (tid, status, err) = (
            task_id.clone(),
            match state {
                RunState::Failed => "failed",
                RunState::Idle => "cancelled",
                _ => "done",
            },
            error,
        );
        let _ = self
            .db
            .call(move |c| {
                c.execute(
                    "UPDATE task SET status=?2, error=?3, progress=1, updated_at=?4 WHERE id=?1",
                    rusqlite::params![tid, status, err, now_ms()],
                )?;
                Ok(())
            })
            .await;
        self.emit_run(sid, &task_id, &st);
    }

    /// Grouping, scoring, clustering and the final refresh events.
    async fn finish_stages(self: &Arc<Self>, sid: i64, task_id: &str) -> Result<()> {
        let stage = |name: &str| {
            let st = self.update_run(sid, |s| s.stage = Some(name.to_string()));
            self.emit_run(sid, task_id, &st);
        };
        stage("grouping");
        let t = Instant::now();
        let burst_ids = self
            .db
            .call(move |c| store::regroup_session(c, sid, &GroupParams::default()))
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

    pub async fn people(&self, session_id: Option<i64>) -> Result<Vec<Person>> {
        if let Some(s) = session_id {
            self.session(s).await?;
        }
        self.db
            .call(move |c| store::list_people(c, session_id))
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
    pub async fn accept_ai(&self, ids: Vec<i64>) -> Result<usize> {
        if ids.is_empty() {
            return Err(CoreError::bad_request("ids must not be empty"));
        }
        let updates: Vec<PhotoUpdate> = self.db.call(move |c| store::accept_ai(c, &ids)).await?;
        let n = updates.len();
        if n > 0 {
            self.events.emit(Event::PhotosUpdated { items: updates });
        }
        Ok(n)
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
