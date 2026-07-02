//! Shared test setup: a fixed encryption key and minimal row fixtures.
//!
//! `#[sqlx::test]` provisions a fresh migrated database per test, so fixtures
//! here only need to create the parent rows a deployment/env-var references.
//!
//! Not every test binary uses every helper; `#![allow(dead_code)]` keeps the
//! shared module warning-clean under `-D warnings`.
#![allow(dead_code)]

use std::sync::Once;

use base64::Engine as _;
use sqlx::PgPool;

static KEY_INIT: Once = Once::new();

/// Install a fixed 32-byte base64 `RUSTIFY_SECRET_KEY` so `rustify_core::crypto`
/// has a stable key across the whole test process. Set once, before any crypto
/// call, matching the pattern the brief prescribes.
pub fn init_secret_key() {
    KEY_INIT.call_once(|| {
        let key = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
        // SAFETY: run exactly once via `Once`, with the same value; no other
        // code in the test binary mutates the environment.
        unsafe {
            std::env::set_var("RUSTIFY_SECRET_KEY", key);
        }
    });
}

/// Parent ids an application/deployment needs.
#[allow(dead_code)]
pub struct Fixture {
    pub team_id: i64,
    pub server_id: i64,
    pub environment_id: i64,
    pub destination_id: i64,
}

/// Create a team, private key, server (+settings with `concurrent_builds`),
/// project, production environment and a default destination.
pub async fn setup(pool: &PgPool, concurrent_builds: i32) -> Fixture {
    let team_id: i64 =
        sqlx::query_scalar("INSERT INTO teams (uuid, name) VALUES ($1, 'team') RETURNING id")
            .bind(uid())
            .fetch_one(pool)
            .await
            .unwrap();

    let key_id: i64 = sqlx::query_scalar(
        "INSERT INTO private_keys (uuid, team_id, name, private_key_enc, public_key)
         VALUES ($1, $2, 'key', $3, 'ssh-ed25519 AAAA') RETURNING id",
    )
    .bind(uid())
    .bind(team_id)
    .bind(vec![0u8; 4])
    .fetch_one(pool)
    .await
    .unwrap();

    let server_id: i64 = sqlx::query_scalar(
        "INSERT INTO servers (uuid, team_id, name, ip, port, ssh_user, private_key_id)
         VALUES ($1, $2, 'srv', '10.0.0.1', 22, 'root', $3) RETURNING id",
    )
    .bind(uid())
    .bind(team_id)
    .bind(key_id)
    .fetch_one(pool)
    .await
    .unwrap();

    sqlx::query("INSERT INTO server_settings (server_id, concurrent_builds) VALUES ($1, $2)")
        .bind(server_id)
        .bind(concurrent_builds)
        .execute(pool)
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

    Fixture {
        team_id,
        server_id,
        environment_id,
        destination_id,
    }
}

/// Insert an application under the fixture and return its id.
pub async fn new_app(pool: &PgPool, fx: &Fixture) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO applications (uuid, environment_id, destination_id, name, git_repository)
         VALUES ($1, $2, $3, 'app', 'https://example.com/r.git') RETURNING id",
    )
    .bind(uid())
    .bind(fx.environment_id)
    .bind(fx.destination_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

fn uid() -> String {
    rustify_core::ids::new_uuid()
}
