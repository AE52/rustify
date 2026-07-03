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
use serde_json::json;
use sqlx::PgPool;
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

/// `GET /ws`: authenticate, then upgrade and fan out matching events. The
/// authenticated principal's team id is threaded into the socket so channel
/// subscriptions can be team-scoped (a client must never receive another team's
/// deployment logs, which carry secrets).
pub async fn ws_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<WsQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    // `?token=` is a bearer token; otherwise fall back to header/cookie auth.
    let team_id = if let Some(token) = query.token.as_deref() {
        resolve_bearer(&state, token).await?.team_id
    } else {
        authenticate(&state, &headers).await?.team_id()
    };

    let rx = state.events.subscribe();
    let pool = state.pool.clone();
    Ok(ws.on_upgrade(move |socket| handle_socket(socket, rx, pool, team_id)))
}

/// Resolve the team that owns the resource a channel `<kind>:<uuid>` addresses
/// (or, for `team:<uuid>`, the team's own id). `None` when the kind is unknown,
/// the resource does not exist, or the lookup fails — all of which deny the
/// subscription. Mirrors the ownership chains used elsewhere (deployment →
/// application → environment → project.team_id, etc.).
async fn resolve_channel_team(pool: &PgPool, kind: &str, uuid: &str) -> Option<i64> {
    let result: Result<Option<i64>, sqlx::Error> = match kind {
        "deployment" => {
            sqlx::query_scalar(
                "SELECT p.team_id FROM deployments d
                   JOIN applications a ON a.id = d.application_id
                   JOIN environments e ON e.id = a.environment_id
                   JOIN projects p ON p.id = e.project_id
                 WHERE d.uuid = $1",
            )
            .bind(uuid)
            .fetch_optional(pool)
            .await
        }
        "application" => {
            sqlx::query_scalar(
                "SELECT p.team_id FROM applications a
                   JOIN environments e ON e.id = a.environment_id
                   JOIN projects p ON p.id = e.project_id
                 WHERE a.uuid = $1",
            )
            .bind(uuid)
            .fetch_optional(pool)
            .await
        }
        "database" => {
            sqlx::query_scalar(
                "SELECT p.team_id FROM standalone_databases sd
                   JOIN environments e ON e.id = sd.environment_id
                   JOIN projects p ON p.id = e.project_id
                 WHERE sd.uuid = $1",
            )
            .bind(uuid)
            .fetch_optional(pool)
            .await
        }
        "service" => {
            sqlx::query_scalar(
                "SELECT p.team_id FROM services s
                   JOIN environments e ON e.id = s.environment_id
                   JOIN projects p ON p.id = e.project_id
                 WHERE s.uuid = $1",
            )
            .bind(uuid)
            .fetch_optional(pool)
            .await
        }
        "backup" => {
            sqlx::query_scalar(
                "SELECT p.team_id FROM scheduled_database_backups b
                   JOIN standalone_databases sd ON sd.id = b.database_id
                   JOIN environments e ON e.id = sd.environment_id
                   JOIN projects p ON p.id = e.project_id
                 WHERE b.uuid = $1",
            )
            .bind(uuid)
            .fetch_optional(pool)
            .await
        }
        "scheduled-task" => {
            sqlx::query_scalar(
                "SELECT COALESCE(st.team_id, pa.team_id, ps.team_id) FROM scheduled_tasks st
                   LEFT JOIN applications a ON a.id = st.application_id
                   LEFT JOIN environments ea ON ea.id = a.environment_id
                   LEFT JOIN projects pa ON pa.id = ea.project_id
                   LEFT JOIN services s ON s.id = st.service_id
                   LEFT JOIN environments es ON es.id = s.environment_id
                   LEFT JOIN projects ps ON ps.id = es.project_id
                 WHERE st.uuid = $1",
            )
            .bind(uuid)
            .fetch_optional(pool)
            .await
        }
        "server" => {
            sqlx::query_scalar("SELECT team_id FROM servers WHERE uuid = $1")
                .bind(uuid)
                .fetch_optional(pool)
                .await
        }
        "team" => {
            sqlx::query_scalar("SELECT id FROM teams WHERE uuid = $1")
                .bind(uuid)
                .fetch_optional(pool)
                .await
        }
        _ => return None,
    };
    match result {
        Ok(team) => team,
        Err(e) => {
            tracing::warn!(error = %e, kind, "ws: failed to resolve channel owner");
            None
        }
    }
}

/// Whether `team_id` may subscribe to `channel`. The channel must be
/// `<kind>:<uuid>` and its owning team must equal the caller's team.
async fn channel_allowed(pool: &PgPool, team_id: i64, channel: &str) -> bool {
    let Some((kind, uuid)) = channel.split_once(':') else {
        return false;
    };
    resolve_channel_team(pool, kind, uuid).await == Some(team_id)
}

async fn handle_socket(
    socket: WebSocket,
    mut rx: broadcast::Receiver<WsEvent>,
    pool: PgPool,
    team_id: i64,
) {
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
                                // Team-scope every subscription: reject (and never
                                // insert) a channel whose resource belongs to
                                // another team, so a client can't receive another
                                // tenant's logs/events.
                                "subscribe" => {
                                    if channel_allowed(&pool, team_id, &msg.channel).await {
                                        channels.insert(msg.channel);
                                    } else {
                                        let frame = json!({
                                            "event": "subscription_rejected",
                                            "channel": msg.channel,
                                            "error": "not authorized for channel",
                                        });
                                        if let Ok(text) = serde_json::to_string(&frame)
                                            && sink.send(Message::Text(text.into())).await.is_err()
                                        {
                                            break;
                                        }
                                    }
                                }
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
