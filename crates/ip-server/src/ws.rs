//! `GET /api/events`: WebSocket stream of core events, coalesced every 100 ms per event type.
//!
//! Owners get every event. Guests (read-only, 02 §8) get only the events whose payload the REST
//! API would also show them; everything else, including event types added later, is withheld.

use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Extension;
use ip_core::events::{Coalescer, Event};
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;

use crate::auth::{MaybeIdentity, Role};
use crate::routes::AppState;

pub const COALESCE_MS: u64 = 100;

pub async fn events(
    ws: WebSocketUpgrade,
    State(st): State<AppState>,
    Extension(MaybeIdentity(ident)): Extension<MaybeIdentity>,
) -> Response {
    // Only the owner and the guest have a use for the stream. A paired device is refused by the
    // auth middleware already; refuse here too rather than fall through to "everything".
    let role = match ident.map(|i| i.role) {
        Some(r @ (Role::Owner | Role::Guest)) => r,
        _ => return StatusCode::FORBIDDEN.into_response(),
    };
    ws.on_upgrade(move |socket| handle(socket, st, role))
}

/// Allow-list of what a guest may receive. Each of these carries data the guest can also read
/// through `GET /api/sessions`, `/api/photos`, `/api/groups` or `/api/collections`. Everything not
/// named here is denied: settings (paths, LAN config), people / taste / analysis / runtime /
/// worker state, task progress (error text may contain paths), XMP conflicts, and the results of
/// edit, generation and assistant actions.
fn guest_may_receive(e: &Event) -> bool {
    matches!(
        e,
        Event::SessionUpdated { .. }
            | Event::PhotosAdded { .. }
            | Event::ThumbsReady { .. }
            | Event::PhotosUpdated { .. }
            | Event::EditsUpdated { .. }
            | Event::GroupsUpdated { .. }
            | Event::CollectionsUpdated { .. }
    )
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ClientMessage {
    #[serde(rename = "viewport")]
    Viewport { ids: Vec<i64> },
}

async fn handle(mut socket: WebSocket, st: AppState, role: Role) {
    let mut rx = st.core.events.subscribe();
    let mut co = Coalescer::default();
    let mut tick = tokio::time::interval(Duration::from_millis(COALESCE_MS));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(e) => {
                    if role == Role::Owner || guest_may_receive(&e) {
                        co.push(e);
                    }
                }
                Err(RecvError::Lagged(n)) => tracing::warn!(skipped = n, "websocket client lagged"),
                Err(RecvError::Closed) => break,
            },
            _ = tick.tick() => {
                for ev in co.drain() {
                    let Ok(text) = ip_core::jsonfix::to_tidy_string(&ev) else { continue };
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        return;
                    }
                }
            }
            msg = socket.recv() => match msg {
                Some(Ok(Message::Text(t))) => {
                    // `viewport` reorders thumbnail work: owner only (guests have no write access)
                    if role != Role::Owner {
                        continue;
                    }
                    if let Ok(ClientMessage::Viewport { ids }) = serde_json::from_str(&t) {
                        if let Err(e) = st.core.viewport(ids).await {
                            tracing::warn!(error = %e, "viewport message failed");
                        }
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
        }
    }
}
