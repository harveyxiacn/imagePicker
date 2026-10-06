//! Typed event bus (`tokio::sync::broadcast`) and the per-connection coalescer used by the WebSocket.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::analysis::types::RunState;
use crate::model::{EditUpdate, PhotoUpdate, Session, ThumbItem};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum Event {
    #[serde(rename = "session.updated")]
    SessionUpdated { session: Session },
    #[serde(rename = "photos.added")]
    PhotosAdded { session_id: i64, count: i64 },
    #[serde(rename = "thumbs.ready")]
    ThumbsReady { items: Vec<ThumbItem> },
    #[serde(rename = "photos.updated")]
    PhotosUpdated { items: Vec<PhotoUpdate> },
    #[serde(rename = "edits.updated")]
    EditsUpdated { items: Vec<EditUpdate> },
    #[serde(rename = "task.progress")]
    TaskProgress {
        task_id: String,
        kind: String,
        done: i64,
        total: i64,
        state: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    #[serde(rename = "analysis.progress")]
    AnalysisProgress {
        session_id: i64,
        state: RunState,
        stage: Option<String>,
        done: i64,
        total: i64,
    },
    #[serde(rename = "analysis.updated")]
    AnalysisUpdated { session_id: i64, ids: Vec<i64> },
    #[serde(rename = "groups.updated")]
    GroupsUpdated { session_id: i64 },
    #[serde(rename = "people.updated")]
    PeopleUpdated { session_id: i64 },
    #[serde(rename = "beauty.ready")]
    BeautyReady { photo_id: i64 },
    #[serde(rename = "taste.updated")]
    TasteUpdated {
        labels: i64,
        active: bool,
        alpha: f64,
    },
    #[serde(rename = "collections.updated")]
    CollectionsUpdated {},
    #[serde(rename = "besttake.done")]
    BestTakeDone {
        photo_id: i64,
        results: Vec<crate::generate::BestTakeResult>,
    },
    #[serde(rename = "inpaint.done")]
    InpaintDone {
        photo_id: i64,
        ok: bool,
        reason: Option<String>,
    },
    #[serde(rename = "enhance.done")]
    EnhanceDone {
        photo_id: i64,
        op: String,
        ok: bool,
        reason: Option<String>,
    },
    /// M6: an assistant plan finished (`results`, and `undo` for one history entry).
    #[serde(rename = "assistant.done")]
    AssistantDone {
        plan_id: String,
        ok: bool,
        results: Vec<serde_json::Value>,
        undo: serde_json::Value,
    },
    /// M6: settings changed.
    #[serde(rename = "settings.updated")]
    SettingsUpdated { settings: serde_json::Value },
    /// M6: a sidecar changed outside the app and differs from the catalog.
    #[serde(rename = "xmp.conflict")]
    XmpConflict {
        session_id: i64,
        photo_id: i64,
        sidecar: serde_json::Value,
        catalog: serde_json::Value,
    },
    /// M7: the AI runtime install state changed (payload = `GET /api/runtime` without the hardware).
    #[serde(rename = "runtime.updated")]
    RuntimeUpdated { runtime: serde_json::Value },
    #[serde(rename = "worker.status")]
    WorkerStatus {
        state: String,
        tier: Option<String>,
        error: Option<String>,
    },
}

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Event>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(16384);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    /// Fire and forget; having no subscribers is fine.
    pub fn emit(&self, ev: Event) {
        let _ = self.tx.send(ev);
    }
}

/// Merges events of the same type so a flush emits at most one frame per type
/// (per session / task for the keyed ones). Order of first appearance is kept.
#[derive(Default)]
pub struct Coalescer {
    order: Vec<Key>,
    thumbs: Vec<ThumbItem>,
    updates: Vec<PhotoUpdate>,
    /// photo id -> position in `updates` (merging N items must stay O(N)).
    updates_idx: HashMap<i64, usize>,
    edits: Vec<EditUpdate>,
    edits_idx: HashMap<i64, usize>,
    added: HashMap<i64, i64>,
    sessions: HashMap<i64, Session>,
    tasks: HashMap<String, Event>,
    analysis_progress: HashMap<i64, Event>,
    analysis_ids: HashMap<i64, Vec<i64>>,
    analysis_seen: HashMap<i64, HashSet<i64>>,
    groups: HashSet<i64>,
    people: HashSet<i64>,
    beauty: Vec<i64>,
    taste: Option<Event>,
    collections: bool,
    worker: Option<Event>,
    settings: Option<Event>,
    runtime: Option<Event>,
    /// `*.done` events: delivered one by one, in order.
    done: Vec<Event>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Key {
    Thumbs,
    Updates,
    Edits,
    Added(i64),
    Session(i64),
    Task(String),
    AnalysisProgress(i64),
    AnalysisIds(i64),
    Groups(i64),
    People(i64),
    Beauty,
    Taste,
    Collections,
    Worker,
    Settings,
    Runtime,
    Done,
}

impl Coalescer {
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    fn note(&mut self, k: Key) {
        if !self.order.contains(&k) {
            self.order.push(k);
        }
    }

    pub fn push(&mut self, ev: Event) {
        match ev {
            Event::ThumbsReady { items } => {
                self.note(Key::Thumbs);
                self.thumbs.extend(items);
            }
            Event::PhotosUpdated { items } => {
                self.note(Key::Updates);
                for it in items {
                    match self.updates_idx.get(&it.id) {
                        Some(&i) => self.updates[i] = it,
                        None => {
                            self.updates_idx.insert(it.id, self.updates.len());
                            self.updates.push(it);
                        }
                    }
                }
            }
            Event::EditsUpdated { items } => {
                self.note(Key::Edits);
                for it in items {
                    match self.edits_idx.get(&it.id) {
                        Some(&i) => self.edits[i] = it,
                        None => {
                            self.edits_idx.insert(it.id, self.edits.len());
                            self.edits.push(it);
                        }
                    }
                }
            }
            Event::PhotosAdded { session_id, count } => {
                self.note(Key::Added(session_id));
                *self.added.entry(session_id).or_insert(0) += count;
            }
            Event::SessionUpdated { session } => {
                self.note(Key::Session(session.id));
                self.sessions.insert(session.id, session);
            }
            ev @ Event::AnalysisProgress { .. } => {
                let sid = match &ev {
                    Event::AnalysisProgress { session_id, .. } => *session_id,
                    _ => unreachable!(),
                };
                self.note(Key::AnalysisProgress(sid));
                self.analysis_progress.insert(sid, ev);
            }
            Event::AnalysisUpdated { session_id, ids } => {
                self.note(Key::AnalysisIds(session_id));
                let e = self.analysis_ids.entry(session_id).or_default();
                let seen = self.analysis_seen.entry(session_id).or_default();
                for id in ids {
                    if seen.insert(id) {
                        e.push(id);
                    }
                }
            }
            Event::GroupsUpdated { session_id } => {
                self.note(Key::Groups(session_id));
                self.groups.insert(session_id);
            }
            Event::PeopleUpdated { session_id } => {
                self.note(Key::People(session_id));
                self.people.insert(session_id);
            }
            Event::BeautyReady { photo_id } => {
                self.note(Key::Beauty);
                if !self.beauty.contains(&photo_id) {
                    self.beauty.push(photo_id);
                }
            }
            ev @ Event::TasteUpdated { .. } => {
                self.note(Key::Taste);
                self.taste = Some(ev);
            }
            Event::CollectionsUpdated {} => {
                self.note(Key::Collections);
                self.collections = true;
            }
            ev @ Event::WorkerStatus { .. } => {
                self.note(Key::Worker);
                self.worker = Some(ev);
            }
            ev @ Event::SettingsUpdated { .. } => {
                self.note(Key::Settings);
                self.settings = Some(ev);
            }
            ev @ Event::RuntimeUpdated { .. } => {
                self.note(Key::Runtime);
                self.runtime = Some(ev);
            }
            ev @ (Event::BestTakeDone { .. }
            | Event::InpaintDone { .. }
            | Event::EnhanceDone { .. }
            | Event::AssistantDone { .. }
            | Event::XmpConflict { .. }) => {
                self.note(Key::Done);
                self.done.push(ev);
            }
            ev @ Event::TaskProgress { .. } => {
                let id = match &ev {
                    Event::TaskProgress { task_id, .. } => task_id.clone(),
                    _ => unreachable!(),
                };
                self.note(Key::Task(id.clone()));
                self.tasks.insert(id, ev);
            }
        }
    }

    pub fn drain(&mut self) -> Vec<Event> {
        let order = std::mem::take(&mut self.order);
        let mut out = Vec::with_capacity(order.len());
        for k in order {
            match k {
                Key::Thumbs => out.push(Event::ThumbsReady {
                    items: std::mem::take(&mut self.thumbs),
                }),
                Key::Updates => {
                    self.updates_idx.clear();
                    out.push(Event::PhotosUpdated {
                        items: std::mem::take(&mut self.updates),
                    })
                }
                Key::Edits => {
                    self.edits_idx.clear();
                    out.push(Event::EditsUpdated {
                        items: std::mem::take(&mut self.edits),
                    })
                }
                Key::Added(sid) => {
                    if let Some(count) = self.added.remove(&sid) {
                        out.push(Event::PhotosAdded {
                            session_id: sid,
                            count,
                        });
                    }
                }
                Key::Session(sid) => {
                    if let Some(session) = self.sessions.remove(&sid) {
                        out.push(Event::SessionUpdated { session });
                    }
                }
                Key::Task(id) => {
                    if let Some(ev) = self.tasks.remove(&id) {
                        out.push(ev);
                    }
                }
                Key::AnalysisProgress(sid) => {
                    if let Some(ev) = self.analysis_progress.remove(&sid) {
                        out.push(ev);
                    }
                }
                Key::AnalysisIds(sid) => {
                    self.analysis_seen.remove(&sid);
                    if let Some(ids) = self.analysis_ids.remove(&sid) {
                        out.push(Event::AnalysisUpdated {
                            session_id: sid,
                            ids,
                        });
                    }
                }
                Key::Groups(sid) => {
                    if self.groups.remove(&sid) {
                        out.push(Event::GroupsUpdated { session_id: sid });
                    }
                }
                Key::People(sid) => {
                    if self.people.remove(&sid) {
                        out.push(Event::PeopleUpdated { session_id: sid });
                    }
                }
                Key::Beauty => {
                    // one frame per photo: the payload is a single id
                    for photo_id in std::mem::take(&mut self.beauty) {
                        out.push(Event::BeautyReady { photo_id });
                    }
                }
                Key::Taste => {
                    if let Some(ev) = self.taste.take() {
                        out.push(ev);
                    }
                }
                Key::Collections => {
                    if std::mem::take(&mut self.collections) {
                        out.push(Event::CollectionsUpdated {});
                    }
                }
                Key::Worker => {
                    if let Some(ev) = self.worker.take() {
                        out.push(ev);
                    }
                }
                Key::Settings => {
                    if let Some(ev) = self.settings.take() {
                        out.push(ev);
                    }
                }
                Key::Runtime => {
                    if let Some(ev) = self.runtime.take() {
                        out.push(ev);
                    }
                }
                Key::Done => out.append(&mut self.done),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesces_by_type() {
        let mut c = Coalescer::default();
        c.push(Event::ThumbsReady {
            items: vec![ThumbItem {
                id: 1,
                v: "a".into(),
            }],
        });
        c.push(Event::ThumbsReady {
            items: vec![ThumbItem {
                id: 2,
                v: "b".into(),
            }],
        });
        c.push(Event::PhotosAdded {
            session_id: 1,
            count: 3,
        });
        c.push(Event::PhotosAdded {
            session_id: 1,
            count: 4,
        });
        let pu = |r| PhotoUpdate {
            id: 5,
            user_rating: Some(r),
            flag: 0,
            color_label: None,
        };
        c.push(Event::PhotosUpdated { items: vec![pu(1)] });
        c.push(Event::PhotosUpdated { items: vec![pu(4)] });
        let out = c.drain();
        assert_eq!(out.len(), 3);
        assert!(matches!(&out[0], Event::ThumbsReady { items } if items.len() == 2));
        assert_eq!(
            out[1],
            Event::PhotosAdded {
                session_id: 1,
                count: 7
            }
        );
        assert!(
            matches!(&out[2], Event::PhotosUpdated { items } if items.len() == 1 && items[0].user_rating == Some(4))
        );
        assert!(c.is_empty());
    }

    #[test]
    fn m2_events_coalesce() {
        let mut c = Coalescer::default();
        for ids in [vec![1, 2], vec![2, 3]] {
            c.push(Event::AnalysisUpdated { session_id: 1, ids });
        }
        c.push(Event::AnalysisUpdated {
            session_id: 2,
            ids: vec![9],
        });
        for done in [1, 5] {
            c.push(Event::AnalysisProgress {
                session_id: 1,
                state: RunState::Running,
                stage: Some("analyzing".into()),
                done,
                total: 10,
            });
        }
        c.push(Event::GroupsUpdated { session_id: 1 });
        c.push(Event::GroupsUpdated { session_id: 1 });
        c.push(Event::PeopleUpdated { session_id: 1 });
        for st in ["starting", "ready"] {
            c.push(Event::WorkerStatus {
                state: st.into(),
                tier: None,
                error: None,
            });
        }
        let out = c.drain();
        assert_eq!(out.len(), 6);
        assert_eq!(
            out[0],
            Event::AnalysisUpdated {
                session_id: 1,
                ids: vec![1, 2, 3]
            }
        );
        assert!(matches!(&out[2], Event::AnalysisProgress { done: 5, .. }));
        assert!(matches!(&out[5], Event::WorkerStatus { state, .. } if state == "ready"));
        let v = serde_json::to_value(&out[5]).unwrap();
        assert_eq!(v["type"], "worker.status");
        assert!(v["error"].is_null() && v.get("error").is_some());
        let v = serde_json::to_value(&out[2]).unwrap();
        assert_eq!(v["state"], "running");
    }

    #[test]
    fn edits_updated_merges_by_id() {
        let mut c = Coalescer::default();
        let eu = |id, v: &str| EditUpdate {
            id,
            has_edits: true,
            thumb_version: v.into(),
        };
        c.push(Event::EditsUpdated {
            items: vec![eu(1, "a"), eu(2, "b")],
        });
        c.push(Event::EditsUpdated {
            items: vec![eu(1, "c")],
        });
        let out = c.drain();
        assert_eq!(out.len(), 1);
        match &out[0] {
            Event::EditsUpdated { items } => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[0].thumb_version, "c");
            }
            e => panic!("unexpected {e:?}"),
        }
        let v = serde_json::to_value(&out[0]).unwrap();
        assert_eq!(v["type"], "edits.updated");
        assert_eq!(v["items"][1]["has_edits"], true);
    }

    #[test]
    fn json_shape() {
        let ev = Event::TaskProgress {
            task_id: "export-1".into(),
            kind: "export".into(),
            done: 1,
            total: 2,
            state: "running".into(),
            error: None,
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "task.progress");
        assert!(v.get("error").is_none());
    }
}
