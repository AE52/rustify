//! Terminal WebSocket integration tests: the upgrade is gated on team role
//! (member denied, admin accepted) and the `{ping:true}` → `pong` protocol
//! works over a live socket.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use sqlx::PgPool;
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use rustify_db::repos::SettingsRepo;
use rustify_server::auth::sha256_hex;
use rustify_server::build_router;

mod common;
use common::{seed_user, state};

/// Bind an ephemeral server and return the `/terminal/ws` base URL.
async fn serve(pool: PgPool) -> String {
    let app = build_router(state(pool));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("ws://{addr}/terminal/ws")
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn member_role_is_denied(pool: PgPool) {
    let (team_id, _) = seed_user(&pool).await;
    // A read-only token resolves to the MEMBER role.
    let raw = "term-member-token";
    let token = SettingsRepo::new(pool.clone())
        .create_api_token(team_id, "term", &sha256_hex(raw))
        .await
        .unwrap();
    sqlx::query("UPDATE api_tokens SET abilities = '{read}' WHERE id = $1")
        .bind(token.id)
        .execute(&pool)
        .await
        .unwrap();

    let url = serve(pool).await;
    let result = connect_async(format!("{url}?token={raw}")).await;
    assert!(
        result.is_err(),
        "member role must be denied the terminal upgrade"
    );
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn admin_role_is_accepted_and_ping_pongs(pool: PgPool) {
    let (team_id, _) = seed_user(&pool).await;
    // Default abilities include `write` → ADMIN role.
    let raw = "term-admin-token";
    SettingsRepo::new(pool.clone())
        .create_api_token(team_id, "term", &sha256_hex(raw))
        .await
        .unwrap();

    let url = serve(pool).await;
    let (mut socket, _resp) = connect_async(format!("{url}?token={raw}"))
        .await
        .expect("admin role is accepted for the terminal upgrade");

    // `{ping:true}` must be answered with the literal text `pong` (not a WS
    // pong control frame).
    socket
        .send(Message::Text(json!({ "ping": true }).to_string()))
        .await
        .unwrap();

    // Read frames until the text `pong` arrives, skipping any transport
    // control frames (ping/pong) the server may interleave.
    let mut pong = None;
    for _ in 0..5 {
        let msg = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .expect("a reply arrives")
            .expect("stream open")
            .expect("valid frame");
        match msg {
            Message::Text(t) => {
                pong = Some(t.to_string());
                break;
            }
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("expected text pong, got {other:?}"),
        }
    }
    assert_eq!(pong.as_deref(), Some("pong"));
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn unauthenticated_upgrade_is_rejected(pool: PgPool) {
    seed_user(&pool).await;
    let url = serve(pool).await;
    let result = connect_async(url).await;
    assert!(
        result.is_err(),
        "unauthenticated terminal upgrade must be rejected"
    );
}
