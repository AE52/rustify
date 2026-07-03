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
use rustify_db::repos::{DeploymentRepo, NewDeployment, SettingsRepo, TeamRepo};
use rustify_server::auth::sha256_hex;
use rustify_server::build_router;

mod common;
use common::{seed_user, state};

fn uid() -> String {
    rustify_core::ids::new_uuid()
}

/// A process-unique IP so each seeded server clears the `(ip, port, ssh_user)`
/// unique constraint.
fn unique_ip() -> String {
    use std::sync::atomic::{AtomicU16, Ordering};
    static N: AtomicU16 = AtomicU16::new(0);
    let n = N.fetch_add(1, Ordering::SeqCst);
    format!("10.0.{}.{}", n / 256, n % 256)
}

/// Seed a full ownership chain (key → server → destination → project →
/// environment → application) for `team_id` and return a queued deployment's
/// uuid, so the WS guard can resolve the channel to this team.
async fn seed_deployment(pool: &PgPool, team_id: i64) -> String {
    let key_id: i64 = sqlx::query_scalar(
        "INSERT INTO private_keys (uuid, team_id, name, private_key_enc, public_key)
         VALUES ($1, $2, 'k', $3, 'ssh-ed25519 AAAA') RETURNING id",
    )
    .bind(uid())
    .bind(team_id)
    .bind(vec![0u8; 4])
    .fetch_one(pool)
    .await
    .unwrap();
    let server_id: i64 = sqlx::query_scalar(
        "INSERT INTO servers (uuid, team_id, name, ip, port, ssh_user, private_key_id, reachable, usable)
         VALUES ($1, $2, 'srv', $4, 22, 'root', $3, true, true) RETURNING id",
    )
    .bind(uid())
    .bind(team_id)
    .bind(key_id)
    .bind(unique_ip())
    .fetch_one(pool)
    .await
    .unwrap();
    let destination_id: i64 = sqlx::query_scalar(
        "INSERT INTO destinations (uuid, server_id, network) VALUES ($1, $2, 'rustify') RETURNING id",
    )
    .bind(uid())
    .bind(server_id)
    .fetch_one(pool)
    .await
    .unwrap();
    let project_id: i64 = sqlx::query_scalar(
        "INSERT INTO projects (uuid, team_id, name) VALUES ($1, $2, 'p') RETURNING id",
    )
    .bind(uid())
    .bind(team_id)
    .fetch_one(pool)
    .await
    .unwrap();
    let environment_id: i64 = sqlx::query_scalar(
        "INSERT INTO environments (uuid, project_id, name) VALUES ($1, $2, 'production') RETURNING id",
    )
    .bind(uid())
    .bind(project_id)
    .fetch_one(pool)
    .await
    .unwrap();
    let app_id: i64 = sqlx::query_scalar(
        "INSERT INTO applications
           (uuid, environment_id, destination_id, name, git_repository, git_branch, build_pack, ports_exposes)
         VALUES ($1, $2, $3, 'app', 'https://example.com/r.git', 'main', 'nixpacks', '3000') RETURNING id",
    )
    .bind(uid())
    .bind(environment_id)
    .bind(destination_id)
    .fetch_one(pool)
    .await
    .unwrap();
    DeploymentRepo::new(pool.clone())
        .create_queued(NewDeployment {
            application_id: app_id,
            server_id,
            commit_sha: None,
            commit_message: None,
            force_rebuild: false,
            rollback: false,
            config_snapshot: None,
            pull_request_id: 0,
            git_type: None,
        })
        .await
        .unwrap()
        .uuid
}

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
    // A real deployment owned by this team (the guard resolves it to `team_id`).
    let dep = seed_deployment(&pool, team_id).await;
    let other = seed_deployment(&pool, team_id).await;
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

    // Subscribe to one (own) deployment channel.
    socket
        .send(Message::Text(
            json!({ "action": "subscribe", "channel": format!("deployment:{dep}") }).to_string(),
        ))
        .await
        .unwrap();

    // Give the server loop time to register the subscription.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Push one matching and one non-matching (but still same-team) event.
    events
        .send(WsEvent::new(
            format!("deployment:{dep}"),
            "deployment_log_appended",
            json!({ "line": "hello" }),
        ))
        .unwrap();
    events
        .send(WsEvent::new(
            format!("deployment:{other}"),
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
    assert_eq!(envelope["channel"], format!("deployment:{dep}"));
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
async fn cross_tenant_subscription_is_rejected(pool: PgPool) {
    // Team A (the caller) and Team B (the victim), each with a deployment.
    let (team_a, _) = seed_user(&pool).await;
    let team_b = TeamRepo::new(pool.clone()).create("team-b").await.unwrap();
    let dep_b = seed_deployment(&pool, team_b.id).await;

    let raw = "ws-test-token-a";
    SettingsRepo::new(pool.clone())
        .create_api_token(team_a, "ws", &sha256_hex(raw))
        .await
        .unwrap();

    let (url, events) = serve(pool).await;
    let (mut socket, _resp) = connect_async(format!("{url}?token={raw}"))
        .await
        .expect("authenticated upgrade succeeds");

    // Team A tries to subscribe to Team B's deployment channel.
    socket
        .send(Message::Text(
            json!({ "action": "subscribe", "channel": format!("deployment:{dep_b}") }).to_string(),
        ))
        .await
        .unwrap();

    // The server replies with an explicit rejection frame (never inserts the
    // channel into the subscription set).
    let msg = tokio::time::timeout(Duration::from_millis(500), socket.next())
        .await
        .expect("a rejection frame arrives")
        .expect("stream open")
        .expect("valid frame");
    let text = match msg {
        Message::Text(t) => t.to_string(),
        other => panic!("expected text frame, got {other:?}"),
    };
    let envelope: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(envelope["event"], "subscription_rejected");
    assert_eq!(envelope["channel"], format!("deployment:{dep_b}"));

    // Broadcasting on Team B's channel must NOT reach Team A.
    events
        .send(WsEvent::new(
            format!("deployment:{dep_b}"),
            "deployment_log_appended",
            json!({ "line": "SECRET=topsecret" }),
        ))
        .unwrap();

    let leaked = tokio::time::timeout(Duration::from_millis(300), socket.next()).await;
    assert!(
        leaked.is_err(),
        "a rejected (cross-tenant) subscription must never receive events"
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
