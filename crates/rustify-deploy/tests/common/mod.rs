//! Shared engine-test fixtures: a fixed encryption key, an event bus, and the
//! minimal team/server/environment/application/deployment rows the engine reads.
#![allow(dead_code)]

use std::sync::{Arc, Once};

use base64::Engine as _;
use sqlx::PgPool;
use tokio::sync::broadcast;

use rustify_core::CommandExecutor;
use rustify_core::events::WsEvent;
use rustify_db::repos::deployments::{Deployment, DeploymentRepo, NewDeployment};
use rustify_deploy::DeployEngineDeps;

static KEY_INIT: Once = Once::new();

/// Install a fixed 32-byte base64 `RUSTIFY_SECRET_KEY` (once per process) so
/// `rustify_core::crypto` can encrypt/decrypt env vars deterministically.
pub fn init_secret_key() {
    KEY_INIT.call_once(|| {
        let key = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
        // SAFETY: set exactly once, before any crypto call, with a constant.
        unsafe {
            std::env::set_var("RUSTIFY_SECRET_KEY", key);
        }
    });
}

/// Parent ids the engine needs.
pub struct Fixture {
    pub team_id: i64,
    pub server_id: i64,
    pub environment_id: i64,
    pub destination_id: i64,
}

/// Deps wired to a `FakeExecutor` plus a receiver kept alive so `send` succeeds.
pub struct Deps {
    pub deps: DeployEngineDeps,
    pub events_rx: broadcast::Receiver<WsEvent>,
}

/// Build [`DeployEngineDeps`] from a pool and a fake executor.
pub fn deps(pool: &PgPool, executor: Arc<dyn CommandExecutor>) -> Deps {
    let (tx, events_rx) = broadcast::channel(256);
    Deps {
        deps: DeployEngineDeps::new(executor, pool.clone(), tx),
        events_rx,
    }
}

/// Create a team, private key, server (+settings), project, production
/// environment and a default `rustify`-network destination.
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
        "INSERT INTO servers (uuid, team_id, name, ip, port, ssh_user, private_key_id, reachable, usable)
         VALUES ($1, $2, 'srv', '10.0.0.1', 22, 'root', $3, true, true) RETURNING id",
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

/// Insert an application with the given build pack; returns `(id, uuid)`.
pub async fn new_app(pool: &PgPool, fx: &Fixture, build_pack: &str) -> (i64, String) {
    let uuid = uid();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO applications
           (uuid, environment_id, destination_id, name, git_repository, git_branch, build_pack, ports_exposes)
         VALUES ($1, $2, $3, 'app', 'https://example.com/r.git', 'main', $4, '3000') RETURNING id",
    )
    .bind(&uuid)
    .bind(fx.environment_id)
    .bind(fx.destination_id)
    .bind(build_pack)
    .fetch_one(pool)
    .await
    .unwrap();
    (id, uuid)
}

/// Create a queued deployment for `(app, server)`.
pub async fn queue(pool: &PgPool, app_id: i64, server_id: i64, force_rebuild: bool) -> Deployment {
    DeploymentRepo::new(pool.clone())
        .create_queued(NewDeployment {
            application_id: app_id,
            server_id,
            commit_sha: None,
            commit_message: None,
            force_rebuild,
            rollback: false,
            config_snapshot: None,
        })
        .await
        .unwrap()
}

/// A canned `git ls-remote` response resolving `main` to a fixed 40-hex sha.
pub const LS_REMOTE_OK: &str = "9f1c2d3e4b5a69788091a2b3c4d5e6f708192a3b\trefs/heads/main";

pub fn uid() -> String {
    rustify_core::ids::new_uuid()
}
