//! Task history and cancellation (docs/api-contract-m5.md section F).
//!
//! Generation tasks (best take, inpainting, enhancement), exports and analysis runs write a row
//! into the `task` table when they start and update it when they end; only the newest
//! [`KEEP_TASKS`] rows are kept. While a task runs it is also registered in memory with its
//! [`CancelToken`], so `POST /api/tasks/{id}/cancel` can stop it. Rows still `running` when the
//! catalog is opened belong to a process that exited mid-task and become `interrupted`.

use std::collections::HashMap;
use std::sync::Mutex;

use ip_worker_client::CancelToken;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;
use serde_json::Value;

use crate::catalog::now_ms;
use crate::error::{CoreError, Result};
use crate::events::Event;
use crate::Core;

/// Rows kept in the `task` table; older finished rows are deleted when a task starts.
pub const KEEP_TASKS: i64 = 200;
/// `limit` of `GET /api/tasks` when none is given.
pub const DEFAULT_LIST: i64 = 50;

pub const RUNNING: &str = "running";
/// Listed while a cancelled task is still winding down (never stored).
pub const CANCELLING: &str = "cancelling";
pub const DONE: &str = "done";
pub const FAILED: &str = "failed";
pub const CANCELLED: &str = "cancelled";
/// The app exited while the task was running.
pub const INTERRUPTED: &str = "interrupted";

/// One row of `GET /api/tasks`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TaskRecord {
    pub id: String,
    /// `besttake` | `inpaint` | `enhance` | `export` | `analysis`
    pub kind: String,
    /// `running` | `cancelling` | `done` | `failed` | `cancelled` | `interrupted`
    pub status: String,
    /// What was asked, per kind (e.g. `{"photo_id": 12, "op": "denoise"}`).
    pub params: Value,
    pub done: i64,
    pub total: i64,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
    /// `POST /api/tasks/{id}/cancel` would stop it.
    pub cancellable: bool,
}

// ------------------------------------------------------------------ SQL

/// Writes the row of a task that just started and prunes old rows.
pub(crate) fn insert(c: &Connection, id: &str, kind: &str, spec: &Value, total: i64) -> Result<()> {
    c.execute(
        "INSERT INTO task(id, kind, status, priority, params, progress, done, total, created_at, updated_at)
         VALUES(?1, ?2, 'running', 0, ?3, 0, 0, ?4, ?5, ?5)",
        params![id, kind, spec.to_string(), total, now_ms()],
    )?;
    prune(c)?;
    Ok(())
}

/// Records how a task ended.
pub(crate) fn finish(
    c: &Connection,
    id: &str,
    status: &str,
    done: i64,
    total: i64,
    error: Option<&str>,
) -> Result<()> {
    let progress = if status == DONE {
        1.0
    } else if total > 0 {
        (done as f64 / total as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    c.execute(
        "UPDATE task SET status=?2, done=?3, total=?4, progress=?5, error=?6, updated_at=?7,
                         finished_at=?7
         WHERE id=?1",
        params![id, status, done, total, progress, error, now_ms()],
    )?;
    Ok(())
}

/// Deletes the finished rows beyond the newest [`KEEP_TASKS`].
pub(crate) fn prune(c: &Connection) -> Result<usize> {
    Ok(c.execute(
        "DELETE FROM task WHERE status <> 'running' AND rowid NOT IN
           (SELECT rowid FROM task ORDER BY created_at DESC, rowid DESC LIMIT ?1)",
        [KEEP_TASKS],
    )?)
}

/// Marks the rows a previous process left `running` as `interrupted` (nothing can be running
/// while the catalog is being opened) and prunes. Returns the ids of the interrupted tasks.
pub fn settle_interrupted(c: &Connection) -> Result<Vec<String>> {
    let mut st = c.prepare("SELECT id FROM task WHERE status = 'running'")?;
    let ids = st
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    if !ids.is_empty() {
        let now = now_ms();
        c.execute(
            "UPDATE task SET status='interrupted', updated_at=?1, finished_at=?1
             WHERE status = 'running'",
            [now],
        )?;
    }
    prune(c)?;
    Ok(ids)
}

/// Largest numeric suffix of the stored task ids (`<kind>-<n>`): new ids continue after it, so
/// they never collide with a kept row (pruning makes the row count useless for that).
pub(crate) fn max_seq(c: &Connection) -> Result<u64> {
    let mut st = c.prepare("SELECT id FROM task")?;
    let ids = st.query_map([], |r| r.get::<_, String>(0))?;
    let mut max = 0u64;
    for id in ids {
        if let Some(n) = id?.rsplit('-').next().and_then(|s| s.parse::<u64>().ok()) {
            max = max.max(n);
        }
    }
    Ok(max)
}

const SELECT: &str = "SELECT id, kind, status, params, done, total, error, created_at, updated_at,
                             finished_at
                      FROM task";

fn record(r: &Row) -> rusqlite::Result<TaskRecord> {
    let params: Option<String> = r.get(3)?;
    Ok(TaskRecord {
        id: r.get(0)?,
        kind: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
        status: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
        params: params
            .and_then(|p| serde_json::from_str(&p).ok())
            .unwrap_or(Value::Null),
        done: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
        total: r.get::<_, Option<i64>>(5)?.unwrap_or(0),
        error: r.get(6)?,
        created_at: r.get::<_, Option<i64>>(7)?.unwrap_or(0),
        updated_at: r.get::<_, Option<i64>>(8)?.unwrap_or(0),
        finished_at: r.get(9)?,
        cancellable: false,
    })
}

/// The newest `limit` rows, newest first.
pub(crate) fn list(c: &Connection, limit: i64) -> Result<Vec<TaskRecord>> {
    let mut st = c.prepare(&format!(
        "{SELECT} ORDER BY created_at DESC, rowid DESC LIMIT ?1"
    ))?;
    let v = st
        .query_map([limit], record)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

pub(crate) fn get(c: &Connection, id: &str) -> Result<Option<TaskRecord>> {
    Ok(
        c.query_row(&format!("{SELECT} WHERE id = ?1"), [id], record)
            .optional()?,
    )
}

// ------------------------------------------------------------------ running tasks

struct Live {
    cancel: CancelToken,
    done: i64,
    total: i64,
}

/// The running tasks of this process (cancel tokens and latest progress).
#[derive(Default)]
pub struct TaskRegistry {
    live: Mutex<HashMap<String, Live>>,
}

impl TaskRegistry {
    pub(crate) fn register(&self, id: &str, cancel: CancelToken, total: i64) {
        self.live.lock().unwrap().insert(
            id.to_string(),
            Live {
                cancel,
                done: 0,
                total,
            },
        );
    }

    pub(crate) fn progress(&self, id: &str, done: i64, total: i64) {
        if let Some(t) = self.live.lock().unwrap().get_mut(id) {
            t.done = done;
            t.total = total;
        }
    }

    pub(crate) fn remove(&self, id: &str) {
        self.live.lock().unwrap().remove(id);
    }

    /// Asks the task to stop; `false` when it is not running in this process.
    pub(crate) fn cancel(&self, id: &str) -> bool {
        match self.live.lock().unwrap().get(id) {
            Some(t) => {
                t.cancel.cancel();
                true
            }
            None => false,
        }
    }

    /// Live progress and cancellability of a stored row.
    fn overlay(&self, rec: &mut TaskRecord) {
        if rec.status != RUNNING {
            return;
        }
        if let Some(t) = self.live.lock().unwrap().get(&rec.id) {
            rec.done = t.done;
            rec.total = t.total;
            if t.cancel.is_cancelled() {
                rec.status = CANCELLING.to_string();
            } else {
                rec.cancellable = true;
            }
        }
    }
}

/// A running, recorded, cancellable task.
#[derive(Clone)]
pub(crate) struct TaskHandle {
    pub id: String,
    pub kind: &'static str,
    pub cancel: CancelToken,
}

impl TaskHandle {
    pub fn cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

impl Core {
    /// Starts a recorded task: allocates the id `<kind>-<n>`, writes its row, registers its
    /// cancel token and emits the first `task.progress` (`running`, 0 of `total`).
    pub(crate) async fn task_begin(
        &self,
        kind: &'static str,
        spec: Value,
        total: i64,
    ) -> Result<TaskHandle> {
        let id = format!("{kind}-{}", self.next_task_seq());
        let tid = id.clone();
        self.db
            .call(move |c| insert(c, &tid, kind, &spec, total))
            .await?;
        let cancel = CancelToken::new();
        self.tasks.register(&id, cancel.clone(), total);
        self.emit_task(&id, kind, 0, total, RUNNING, None);
        Ok(TaskHandle { id, kind, cancel })
    }

    /// Progress of a running task (`task.progress`, `running`).
    pub(crate) fn task_step(&self, t: &TaskHandle, done: i64, total: i64) {
        self.tasks.progress(&t.id, done, total);
        self.emit_task(&t.id, t.kind, done, total, RUNNING, None);
    }

    /// Ends a task: records the outcome and unregisters it, then emits `result` (the task's
    /// `*.done` event, if any) and the last `task.progress`, so whoever reacts to either event
    /// already finds the final record.
    pub(crate) async fn task_end(
        &self,
        t: &TaskHandle,
        (done, total): (i64, i64),
        status: &str,
        error: Option<String>,
        result: Option<Event>,
    ) {
        let (id, st, err) = (t.id.clone(), status.to_string(), error.clone());
        if let Err(e) = self
            .db
            .call(move |c| finish(c, &id, &st, done, total, err.as_deref()))
            .await
        {
            tracing::warn!(task = %t.id, error = %e, "recording the end of a task failed");
        }
        self.tasks.remove(&t.id);
        if let Some(ev) = result {
            self.events.emit(ev);
        }
        self.emit_task(&t.id, t.kind, done, total, status, error);
    }

    fn emit_task(
        &self,
        id: &str,
        kind: &str,
        done: i64,
        total: i64,
        state: &str,
        error: Option<String>,
    ) {
        self.events.emit(Event::TaskProgress {
            task_id: id.to_string(),
            kind: kind.to_string(),
            done,
            total,
            state: state.to_string(),
            error,
        });
    }

    /// `GET /api/tasks`: the newest task records first (at most [`KEEP_TASKS`]).
    pub async fn tasks_list(&self, limit: Option<i64>) -> Result<Vec<TaskRecord>> {
        let limit = limit.unwrap_or(DEFAULT_LIST);
        if !(1..=KEEP_TASKS).contains(&limit) {
            return Err(CoreError::bad_request(format!(
                "limit must be within 1..{KEEP_TASKS}"
            )));
        }
        let mut rows = self.db.call(move |c| list(c, limit)).await?;
        for r in &mut rows {
            self.tasks.overlay(r);
        }
        Ok(rows)
    }

    /// `POST /api/tasks/{id}/cancel`: asks a running task to stop and returns its record
    /// (`cancelling` until it has wound down). The task then ends `cancelled` without committing
    /// a result, unless it was already committing one (it ends `done` then). 404 for an unknown
    /// id, 409 when the task is not running.
    pub async fn task_cancel(&self, id: &str) -> Result<TaskRecord> {
        let requested = self.tasks.cancel(id);
        let tid = id.to_string();
        let Some(mut rec) = self.db.call(move |c| get(c, &tid)).await? else {
            return Err(CoreError::not_found(format!("task {id} not found")));
        };
        if !requested {
            return Err(CoreError::Conflict(format!(
                "task {id} is not running ({})",
                rec.status
            )));
        }
        self.tasks.overlay(&mut rec);
        Ok(rec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn db() -> Connection {
        let mut c = Connection::open_in_memory().unwrap();
        crate::db::migrate(&mut c).unwrap();
        c
    }

    #[test]
    fn rows_are_recorded_listed_and_pruned() {
        let c = db();
        insert(&c, "inpaint-1", "inpaint", &json!({"photo_id": 3}), 2).unwrap();
        let r = get(&c, "inpaint-1").unwrap().unwrap();
        assert_eq!((r.status.as_str(), r.done, r.total), (RUNNING, 0, 2));
        assert_eq!(r.params, json!({"photo_id": 3}));
        assert!(r.finished_at.is_none() && !r.cancellable);
        finish(&c, "inpaint-1", FAILED, 1, 2, Some("boom")).unwrap();
        let r = get(&c, "inpaint-1").unwrap().unwrap();
        assert_eq!(
            (r.status.as_str(), r.done, r.error.as_deref()),
            (FAILED, 1, Some("boom"))
        );
        assert!(r.finished_at.is_some());
        assert!(get(&c, "nope-1").unwrap().is_none());

        // only the newest KEEP_TASKS rows survive; running rows are never pruned
        insert(&c, "export-2", "export", &json!({}), 5).unwrap();
        for i in 3..(KEEP_TASKS + 10) {
            let id = format!("enhance-{i}");
            insert(&c, &id, "enhance", &json!({}), 1).unwrap();
            finish(&c, &id, DONE, 1, 1, None).unwrap();
        }
        let n: i64 = c
            .query_row("SELECT COUNT(*) FROM task", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, KEEP_TASKS + 1, "the running export is kept on top");
        assert!(get(&c, "export-2").unwrap().is_some());
        assert!(get(&c, "inpaint-1").unwrap().is_none());
        let rows = list(&c, 3).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].id, format!("enhance-{}", KEEP_TASKS + 9));
        assert_eq!(max_seq(&c).unwrap(), (KEEP_TASKS + 9) as u64);
    }

    #[test]
    fn running_rows_of_a_previous_process_become_interrupted() {
        let c = db();
        insert(&c, "besttake-1", "besttake", &json!({}), 3).unwrap();
        insert(&c, "export-2", "export", &json!({}), 3).unwrap();
        finish(&c, "export-2", DONE, 3, 3, None).unwrap();
        assert_eq!(settle_interrupted(&c).unwrap(), vec!["besttake-1"]);
        let r = get(&c, "besttake-1").unwrap().unwrap();
        assert_eq!(r.status, INTERRUPTED);
        assert!(r.finished_at.is_some());
        assert_eq!(get(&c, "export-2").unwrap().unwrap().status, DONE);
        assert!(settle_interrupted(&c).unwrap().is_empty());
    }

    #[test]
    fn the_registry_overlays_live_progress_and_cancellation() {
        let reg = TaskRegistry::default();
        let tok = CancelToken::new();
        reg.register("inpaint-4", tok.clone(), 3);
        reg.progress("inpaint-4", 2, 3);
        let mut rec = TaskRecord {
            id: "inpaint-4".into(),
            kind: "inpaint".into(),
            status: RUNNING.into(),
            params: Value::Null,
            done: 0,
            total: 0,
            error: None,
            created_at: 1,
            updated_at: 1,
            finished_at: None,
            cancellable: false,
        };
        let mut a = rec.clone();
        reg.overlay(&mut a);
        assert_eq!((a.done, a.total, a.cancellable), (2, 3, true));
        assert!(reg.cancel("inpaint-4") && tok.is_cancelled());
        reg.overlay(&mut rec);
        assert_eq!((rec.status.as_str(), rec.cancellable), (CANCELLING, false));
        reg.remove("inpaint-4");
        assert!(!reg.cancel("inpaint-4"));
    }
}
