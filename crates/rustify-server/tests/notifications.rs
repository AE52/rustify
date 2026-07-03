//! Notification route + subscriber integration tests over `#[sqlx::test(migrations = "../rustify-db/migrations")]`.
//!
//! Covers the write-only/masked settings surface, the test-notification
//! endpoint's validation, and the WS→notification subscriber path driven through
//! an injected fake [`Sender`] (no real network).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;

use rustify_core::WsEvent;
use rustify_core::deployment::DeploymentStatus;
use rustify_core::notify::Channel;
use rustify_db::repos::{NotificationSettingsPatch, NotificationsRepo};
use rustify_server::build_router;
use rustify_server::notify::email::EmailDelivery;
use rustify_server::notify::{Sender, handle_event};

mod common;
use common::{Req, login, seed_user, send, state};

// ----- HTTP surface -------------------------------------------------------

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn settings_are_masked_on_read_and_write_only(pool: PgPool) {
    seed_user(&pool).await;
    let app = build_router(state(pool.clone()));
    let cookie = login(&app).await;

    // Write a secret via PATCH.
    let (status, body) = send(
        &app,
        Req::patch("/api/v1/notifications/settings")
            .cookie(&cookie)
            .json(json!({
                "discord_enabled": true,
                "discord_webhook_url": "https://discord.com/api/webhooks/abc",
                "smtp_recipients": "ops@acme.io"
            }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["discord_enabled"], json!(true));
    assert_eq!(body["discord_webhook_url_configured"], json!(true));
    assert_eq!(body["smtp_recipients"], "ops@acme.io");
    // The secret value must never be echoed back under any key.
    assert!(
        !body.to_string().contains("discord.com/api/webhooks/abc"),
        "response must not leak the webhook secret"
    );

    // GET returns the same masked view.
    let (status, body) = send(
        &app,
        Req::get("/api/v1/notifications/settings")
            .cookie(&cookie)
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["discord_webhook_url_configured"], json!(true));
    assert!(body.get("discord_webhook_url").is_none());
    // The auto-provisioned default matrix opts critical events into all channels.
    assert_eq!(
        body["event_matrix"]["deployment_failure"]["discord"],
        json!(true)
    );
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn test_endpoint_validates_channel_and_reports_unconfigured(pool: PgPool) {
    seed_user(&pool).await;
    let app = build_router(state(pool.clone()));
    let cookie = login(&app).await;

    // Unknown channel → 422.
    let (status, _) = send(
        &app,
        Req::post("/api/v1/notifications/test")
            .cookie(&cookie)
            .json(json!({ "channel": "carrierpigeon" }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Known-but-unconfigured channel → 200 with sent:false (no network hit).
    let (status, body) = send(
        &app,
        Req::post("/api/v1/notifications/test")
            .cookie(&cookie)
            .json(json!({ "channel": "discord" }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["sent"], json!(false));
    assert!(
        body["message"].as_str().unwrap().contains("not enabled"),
        "message should explain the channel is not enabled"
    );
}

// ----- Subscriber path (fake sender seam) ---------------------------------

/// Records every webhook POST so the test can assert what was delivered.
#[derive(Clone, Default)]
struct FakeSender {
    posts: Arc<Mutex<Vec<(Channel, String, Value)>>>,
}

#[async_trait::async_trait]
impl Sender for FakeSender {
    async fn post_json(&self, channel: Channel, url: &str, body: Value) -> Result<(), String> {
        self.posts
            .lock()
            .expect("lock")
            .push((channel, url.to_string(), body));
        Ok(())
    }
    async fn send_email(&self, _delivery: EmailDelivery) -> Result<(), String> {
        Ok(())
    }
}

/// Build team → project → env → app → deployment; return `(team_id, deploy_uuid)`.
async fn seed_deployment(pool: &PgPool) -> (i64, String) {
    let uid = rustify_core::ids::new_uuid;
    let team_id: i64 =
        sqlx::query_scalar("INSERT INTO teams (uuid, name) VALUES ($1,'t') RETURNING id")
            .bind(uid())
            .fetch_one(pool)
            .await
            .unwrap();
    let key_id: i64 = sqlx::query_scalar(
        "INSERT INTO private_keys (uuid, team_id, name, private_key_enc, public_key)
         VALUES ($1,$2,'k',$3,'ssh-ed25519 AAAA') RETURNING id",
    )
    .bind(uid())
    .bind(team_id)
    .bind(vec![0u8; 4])
    .fetch_one(pool)
    .await
    .unwrap();
    let server_id: i64 = sqlx::query_scalar(
        "INSERT INTO servers (uuid, team_id, name, ip, private_key_id) VALUES ($1,$2,'s','10.0.0.1',$3) RETURNING id",
    )
    .bind(uid())
    .bind(team_id)
    .bind(key_id)
    .fetch_one(pool)
    .await
    .unwrap();
    let dest_id: i64 = sqlx::query_scalar(
        "INSERT INTO destinations (uuid, server_id, network) VALUES ($1,$2,'rustify') RETURNING id",
    )
    .bind(uid())
    .bind(server_id)
    .fetch_one(pool)
    .await
    .unwrap();
    let project_id: i64 = sqlx::query_scalar(
        "INSERT INTO projects (uuid, team_id, name) VALUES ($1,$2,'p') RETURNING id",
    )
    .bind(uid())
    .bind(team_id)
    .fetch_one(pool)
    .await
    .unwrap();
    let env_id: i64 = sqlx::query_scalar(
        "INSERT INTO environments (uuid, project_id, name) VALUES ($1,$2,'production') RETURNING id",
    )
    .bind(uid())
    .bind(project_id)
    .fetch_one(pool)
    .await
    .unwrap();
    let app_id: i64 = sqlx::query_scalar(
        "INSERT INTO applications (uuid, environment_id, destination_id, name, git_repository)
         VALUES ($1,$2,$3,'web','https://example.com/r.git') RETURNING id",
    )
    .bind(uid())
    .bind(env_id)
    .bind(dest_id)
    .fetch_one(pool)
    .await
    .unwrap();
    let deploy_uuid = uid();
    sqlx::query(
        "INSERT INTO deployments (uuid, application_id, server_id, status) VALUES ($1,$2,$3,'failed')",
    )
    .bind(&deploy_uuid)
    .bind(app_id)
    .bind(server_id)
    .execute(pool)
    .await
    .unwrap();
    (team_id, deploy_uuid)
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn subscriber_delivers_deployment_failure_to_discord(pool: PgPool) {
    common::init_secret_key();
    let (team_id, deploy_uuid) = seed_deployment(&pool).await;

    // Enable Discord with a webhook URL; the default matrix already opts
    // deployment_failure into every channel.
    NotificationsRepo::new(pool.clone())
        .upsert(
            team_id,
            NotificationSettingsPatch {
                discord_enabled: Some(true),
                discord_ping_enabled: Some(true),
                discord_webhook_url: Some("https://discord.example/webhooks/x".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let sender = Arc::new(FakeSender::default());
    let ev = WsEvent::deployment_status_changed(&deploy_uuid, DeploymentStatus::Failed);
    handle_event(sender.clone(), &pool, &ev).await;

    // Discord is fire-and-forget (spawned); wait briefly for the task to run.
    let mut waited = 0;
    while sender.posts.lock().unwrap().is_empty() && waited < 50 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        waited += 1;
    }
    let posts = sender.posts.lock().unwrap();
    assert_eq!(posts.len(), 1, "exactly one Discord delivery expected");
    let (channel, url, body) = &posts[0];
    assert_eq!(*channel, Channel::Discord);
    assert_eq!(url, "https://discord.example/webhooks/x");
    assert_eq!(body["embeds"][0]["title"], "Deployment failed");
    assert_eq!(body["embeds"][0]["color"], json!(0xff_70_5f));
    // Critical + ping enabled ⇒ @here.
    assert_eq!(body["content"], "@here");
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn subscriber_ignores_success_when_matrix_off(pool: PgPool) {
    common::init_secret_key();
    let (team_id, deploy_uuid) = seed_deployment(&pool).await;
    // Discord enabled, but deployment_success is off by default.
    NotificationsRepo::new(pool.clone())
        .upsert(
            team_id,
            NotificationSettingsPatch {
                discord_enabled: Some(true),
                discord_webhook_url: Some("https://discord.example/webhooks/x".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let sender = Arc::new(FakeSender::default());
    let ev = WsEvent::deployment_status_changed(&deploy_uuid, DeploymentStatus::Finished);
    handle_event(sender.clone(), &pool, &ev).await;

    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        sender.posts.lock().unwrap().is_empty(),
        "deployment_success is off by default → no delivery"
    );
}
