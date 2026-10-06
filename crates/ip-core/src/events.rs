//! Typed event bus (`tokio::sync::broadcast`) and the per-connection coalescer used by the WebSocket.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::model::{PhotoUpdate, Session, ThumbItem};

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
    added: HashMap<i64, i64>,
    sessions: HashMap<i64, Session>,
    tasks: HashMap<String, Event>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Key {
    Thumbs,
    Updates,
    Added(i64),
    Session(i64),
    Task(String),
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
                    if let Some(e) = self.updates.iter_mut().find(|u| u.id == it.id) {
                        *e = it;
                    } else {
                        self.updates.push(it);
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
                Key::Updates => out.push(Event::PhotosUpdated {
                    items: std::mem::take(&mut self.updates),
                }),
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
