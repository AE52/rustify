//! WebSocket fan-out test (contract C4): authenticate at upgrade, subscribe to
//! a channel, and receive only events on that channel.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use sqlx::PgPool;
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use rustify_core::WsEvent;
use rustify_db::repos::SettingsRepo;
use rustify_server::auth::sha256_hex;
use rustify_server::build_router;

mod common;
use common::{seed_user, state};

/// Bind an ephemeral server and return `(base_ws_url, event_sender)`.
async fn serve(pool: PgPool) -> (String, tokio::sync::broadcast::Sender<WsEvent>) {
    let st = state(pool);
    let events = st.events.clone();
    let app = build_router(st);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("ws://{addr}/ws"), events)
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn subscribe_receives_only_matching_channel(pool: PgPool) {
    let (team_id, _) = seed_user(&pool).await;
    // Mint an API token for `?token=` upgrade auth.
    let raw = "ws-test-token-abc";
    SettingsRepo::new(pool.clone())
        .create_api_token(team_id, "ws", &sha256_hex(raw))
        .await
        .unwrap();

    let (url, events) = serve(pool).await;

    let (mut socket, _resp) = connect_async(format!("{url}?token={raw}"))
        .await
        .expect("authenticated upgrade succeeds");

    // Subscribe to one deployment channel.
    socket
        .send(Message::Text(
            json!({ "action": "subscribe", "channel": "deployment:abc" }).to_string(),
        ))
        .await
        .unwrap();

    // Give the server loop time to register the subscription.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Push one matching and one non-matching event.
    events
        .send(WsEvent::new(
            "deployment:abc",
            "deployment_log_appended",
            json!({ "line": "hello" }),
        ))
        .unwrap();
    events
        .send(WsEvent::new(
            "deployment:other",
            "deployment_log_appended",
            json!({ "line": "ignored" }),
        ))
        .unwrap();

    // Exactly one message arrives, on the subscribed channel.
    let msg = tokio::time::timeout(Duration::from_millis(500), socket.next())
        .await
        .expect("a message arrives")
        .expect("stream open")
        .expect("valid frame");
    let text = match msg {
        Message::Text(t) => t.to_string(),
        other => panic!("expected text frame, got {other:?}"),
    };
    let envelope: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(envelope["channel"], "deployment:abc");
    assert_eq!(envelope["event"], "deployment_log_appended");
    assert_eq!(envelope["data"]["line"], "hello");

    // No further (non-matching) message is delivered.
    let next = tokio::time::timeout(Duration::from_millis(300), socket.next()).await;
    assert!(
        next.is_err(),
        "non-matching channel event must not be forwarded"
    );
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn unauthenticated_upgrade_is_rejected(pool: PgPool) {
    seed_user(&pool).await;
    let (url, _events) = serve(pool).await;
    // No token / cookie → the upgrade must fail.
    let result = connect_async(url).await;
    assert!(result.is_err(), "unauthenticated upgrade must be rejected");
}
