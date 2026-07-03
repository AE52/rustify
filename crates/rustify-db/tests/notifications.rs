//! Notification-settings persistence: secret encryption roundtrip, PATCH-merge
//! semantics, and the team auto-provision on `seed_default`, against a real
//! Postgres via `#[sqlx::test]`.

use base64::Engine as _;
use serde_json::json;
use sqlx::PgPool;

use rustify_db::repos::{NotificationSettingsPatch, NotificationsRepo, seed_default};

fn init_key() {
    let key = base64::engine::general_purpose::STANDARD.encode([11u8; 32]);
    // SAFETY: set once per test binary before any crypto call.
    unsafe {
        std::env::set_var("RUSTIFY_SECRET_KEY", key);
    }
}

async fn seed_team(pool: &PgPool) -> i64 {
    sqlx::query_scalar("INSERT INTO teams (uuid, name) VALUES ($1, 't') RETURNING id")
        .bind(rustify_core::ids::new_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test]
async fn ensure_provisions_default_matrix(pool: PgPool) {
    init_key();
    let team_id = seed_team(&pool).await;
    let repo = NotificationsRepo::new(pool.clone());

    assert!(repo.get(team_id).await.unwrap().is_none());
    let s = repo.ensure(team_id).await.unwrap();

    // Default matrix opts the four critical events into every channel.
    assert_eq!(s.event_matrix["deployment_failure"]["discord"], json!(true));
    assert_eq!(s.event_matrix["backup_failure"]["email"], json!(true));
    assert_eq!(s.event_matrix["server_unreachable"]["slack"], json!(true));
    // Non-critical events are absent (default off).
    assert!(s.event_matrix.get("deployment_success").is_none());
    // No channel is enabled or has secrets yet.
    assert!(!s.discord_enabled);
    assert!(s.discord_webhook_url.is_none());
}

#[sqlx::test]
async fn secret_roundtrip_and_at_rest_encryption(pool: PgPool) {
    init_key();
    let team_id = seed_team(&pool).await;
    let repo = NotificationsRepo::new(pool.clone());

    let updated = repo
        .upsert(
            team_id,
            NotificationSettingsPatch {
                discord_enabled: Some(true),
                discord_webhook_url: Some("https://discord.com/api/webhooks/abc".into()),
                telegram_enabled: Some(true),
                telegram_token: Some("123:TOKEN".into()),
                telegram_chat_id: Some("-1001".into()),
                smtp_password: Some("hunter2".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert!(updated.discord_enabled);
    assert_eq!(
        updated.discord_webhook_url.as_deref(),
        Some("https://discord.com/api/webhooks/abc")
    );

    // Re-read decrypts the same plaintext.
    let read = repo.get(team_id).await.unwrap().unwrap();
    assert_eq!(read.telegram_token.as_deref(), Some("123:TOKEN"));
    assert_eq!(read.smtp_password.as_deref(), Some("hunter2"));

    // The columns hold ciphertext, not plaintext, on disk.
    let raw: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT telegram_token_enc FROM notification_settings WHERE team_id = $1",
    )
    .bind(team_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let bytes = raw.unwrap();
    assert!(!bytes.is_empty());
    assert!(
        !bytes.windows(8).any(|w| w == b"123:TOKE"),
        "plaintext token must not appear in the stored column"
    );
}

#[sqlx::test]
async fn patch_merges_leaving_unset_fields_and_clears_on_empty(pool: PgPool) {
    init_key();
    let team_id = seed_team(&pool).await;
    let repo = NotificationsRepo::new(pool.clone());

    repo.upsert(
        team_id,
        NotificationSettingsPatch {
            slack_enabled: Some(true),
            slack_webhook_url: Some("https://hooks.slack.com/services/x".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // A patch that omits slack_webhook_url leaves it intact; toggling another
    // field does not disturb the stored secret.
    let after = repo
        .upsert(
            team_id,
            NotificationSettingsPatch {
                discord_ping_enabled: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        after.slack_webhook_url.as_deref(),
        Some("https://hooks.slack.com/services/x")
    );
    assert!(after.discord_ping_enabled);

    // An explicit empty string clears the secret.
    let cleared = repo
        .upsert(
            team_id,
            NotificationSettingsPatch {
                slack_webhook_url: Some(String::new()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(cleared.slack_webhook_url.is_none());
}

#[sqlx::test]
async fn matrix_can_be_replaced(pool: PgPool) {
    init_key();
    let team_id = seed_team(&pool).await;
    let repo = NotificationsRepo::new(pool.clone());

    let s = repo
        .upsert(
            team_id,
            NotificationSettingsPatch {
                event_matrix: Some(json!({ "deployment_success": { "discord": true } })),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(s.event_matrix["deployment_success"]["discord"], json!(true));
    assert!(s.event_matrix.get("deployment_failure").is_none());
}

#[sqlx::test]
async fn seed_default_auto_provisions_the_root_team(pool: PgPool) {
    init_key();
    // SAFETY: single-threaded test setup before any concurrent access.
    unsafe {
        std::env::set_var("RUSTIFY_ADMIN_EMAIL", "seed-admin@test.local");
        std::env::set_var("RUSTIFY_ADMIN_PASSWORD", "correct horse battery");
    }
    seed_default(&pool).await.unwrap();

    let team_id: i64 = sqlx::query_scalar("SELECT id FROM teams ORDER BY id LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    let s = NotificationsRepo::new(pool.clone())
        .get(team_id)
        .await
        .unwrap()
        .expect("seed_default provisions notification_settings");
    assert_eq!(s.event_matrix["deployment_failure"]["webhook"], json!(true));

    // Re-seeding is a no-op (idempotent), not a unique-constraint failure.
    seed_default(&pool).await.unwrap();
}
