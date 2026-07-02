//! WebSocket endpoint (contract C4).
//!
//! Authenticates at upgrade (session cookie, `Authorization: Bearer`, or
//! `?token=` bearer). Each connection keeps a `HashSet<String>` of subscribed
//! channels and receives the shared `broadcast::Receiver<WsEvent>`; only events
//! whose `channel` is in the set are forwarded. A lagged receiver drops missed
//! events (per C4) instead of disconnecting the client.

use std::collections::HashSet;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::broadcast;

use rustify_core::WsEvent;

use crate::app::AppState;
use crate::auth::{authenticate, resolve_bearer};
use crate::error::ApiError;

/// `?token=` bearer at upgrade time.
#[derive(Debug, Deserialize)]
pub struct WsQuery {
    pub token: Option<String>,
}

/// Client→server control frame (contract C4).
#[derive(Debug, Deserialize)]
struct ClientMessage {
    action: String,
    channel: String,
}

/// Whether an event should be forwarded to a connection given its current
/// channel subscriptions. Extracted so the filtering rule is unit-testable
/// without a live socket.
fn should_forward(channels: &HashSet<String>, event: &WsEvent) -> bool {
    channels.contains(&event.channel)
}

/// `GET /ws`: authenticate, then upgrade and fan out matching events.
pub async fn ws_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<WsQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    // `?token=` is a bearer token; otherwise fall back to header/cookie auth.
    if let Some(token) = query.token.as_deref() {
        resolve_bearer(&state, token).await?;
    } else {
        authenticate(&state, &headers).await?;
    }

    let rx = state.events.subscribe();
    Ok(ws.on_upgrade(move |socket| handle_socket(socket, rx)))
}

async fn handle_socket(socket: WebSocket, mut rx: broadcast::Receiver<WsEvent>) {
    let (mut sink, mut stream) = socket.split();
    let mut channels: HashSet<String> = HashSet::new();

    loop {
        tokio::select! {
            // Client control frames: subscribe / unsubscribe.
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(msg) = serde_json::from_str::<ClientMessage>(text.as_str()) {
                            match msg.action.as_str() {
                                "subscribe" => { channels.insert(msg.channel); }
                                "unsubscribe" => { channels.remove(&msg.channel); }
                                _ => {}
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}       // ignore ping/pong/binary
                    Some(Err(_)) => break,  // transport error
                }
            }
            // Broadcast events: forward those on subscribed channels.
            event = rx.recv() => {
                match event {
                    Ok(ev) => {
                        if should_forward(&channels, &ev) {
                            if let Ok(json) = serde_json::to_string(&ev) {
                                if sink.send(Message::Text(json.into())).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    // Lagged: skip dropped events, keep the connection alive.
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn forwards_only_subscribed_channels() {
        let mut channels = HashSet::new();
        channels.insert("deployment:abc".to_string());

        let match_ev = WsEvent::new("deployment:abc", "deployment_log_appended", json!({}));
        let other_ev = WsEvent::new("deployment:xyz", "deployment_log_appended", json!({}));
        let team_ev = WsEvent::new("team:1", "application_status_changed", json!({}));

        assert!(should_forward(&channels, &match_ev));
        assert!(!should_forward(&channels, &other_ev));
        assert!(!should_forward(&channels, &team_ev));
    }
}
