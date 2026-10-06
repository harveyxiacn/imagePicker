//! `GET /api/events`: WebSocket stream of core events, coalesced every 100 ms per event type.

use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use ip_core::events::Coalescer;
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;

use crate::routes::AppState;

pub const COALESCE_MS: u64 = 100;

pub async fn events(ws: WebSocketUpgrade, State(st): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle(socket, st))
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ClientMessage {
    #[serde(rename = "viewport")]
    Viewport { ids: Vec<i64> },
}

async fn handle(mut socket: WebSocket, st: AppState) {
    let mut rx = st.core.events.subscribe();
    let mut co = Coalescer::default();
    let mut tick = tokio::time::interval(Duration::from_millis(COALESCE_MS));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(e) => co.push(e),
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
