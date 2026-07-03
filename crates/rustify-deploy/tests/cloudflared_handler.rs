//! Cloudflare-tunnel configure/disable handler against the scripted
//! [`FakeExecutor`]: the enable path must repoint `servers.ip` at the
//! operator-supplied SSH hostname (NOT the server uuid) and stash the direct IP
//! in `ip_previous`; the disable path must restore it.

use std::sync::Arc;

use sqlx::PgPool;

use rustify_db::repos::ServerRepo;
use rustify_deploy::{configure_cloudflared, disable_cloudflared};

mod common;
mod fake;

use common::{deps, init_secret_key, setup};
use fake::FakeExecutor;

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn enable_repoints_ip_at_supplied_ssh_hostname(pool: PgPool) {
    init_secret_key();
    let fx = setup(&pool, 1).await;
    let repo = ServerRepo::new(pool.clone());
    let server = repo.get_by_id(fx.server_id).await.unwrap().unwrap();
    let uuid = server.uuid.clone();
    assert_eq!(server.ip, "10.0.0.1");

    let exec = Arc::new(FakeExecutor::new().respond(".State.Health.Status", "healthy"));
    let d = deps(&pool, exec.clone());

    configure_cloudflared(&d.deps, &uuid, "tok", "ssh.example.com")
        .await
        .unwrap();

    let after = repo.get_by_id(fx.server_id).await.unwrap().unwrap();
    assert_eq!(after.ip, "ssh.example.com", "ip must be the ssh hostname");
    assert_ne!(after.ip, uuid, "ip must NOT be the server uuid");
    assert_eq!(after.ip_previous.as_deref(), Some("10.0.0.1"));

    let settings = repo.settings(fx.server_id).await.unwrap().unwrap();
    assert!(settings.is_cloudflare_tunnel);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn disable_restores_previous_ip(pool: PgPool) {
    init_secret_key();
    let fx = setup(&pool, 1).await;
    let repo = ServerRepo::new(pool.clone());
    let uuid = repo.get_by_id(fx.server_id).await.unwrap().unwrap().uuid;

    let exec = Arc::new(FakeExecutor::new().respond(".State.Health.Status", "healthy"));
    let d = deps(&pool, exec.clone());
    configure_cloudflared(&d.deps, &uuid, "tok", "ssh.example.com")
        .await
        .unwrap();

    disable_cloudflared(&d.deps, &uuid).await.unwrap();

    let after = repo.get_by_id(fx.server_id).await.unwrap().unwrap();
    assert_eq!(after.ip, "10.0.0.1", "ip restored to the direct address");
    assert_eq!(after.ip_previous, None);
    let settings = repo.settings(fx.server_id).await.unwrap().unwrap();
    assert!(!settings.is_cloudflare_tunnel);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn enable_fails_when_container_never_healthy(pool: PgPool) {
    init_secret_key();
    let fx = setup(&pool, 1).await;
    let repo = ServerRepo::new(pool.clone());
    let uuid = repo.get_by_id(fx.server_id).await.unwrap().unwrap().uuid;

    // Health inspect returns empty (container absent) on every probe.
    let exec = Arc::new(FakeExecutor::new());
    let d = deps(&pool, exec.clone());

    let err = configure_cloudflared(&d.deps, &uuid, "tok", "ssh.example.com")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("healthy"));

    let after = repo.get_by_id(fx.server_id).await.unwrap().unwrap();
    assert_eq!(after.ip, "10.0.0.1", "ip untouched when enable fails");
    let settings = repo.settings(fx.server_id).await.unwrap().unwrap();
    assert!(!settings.is_cloudflare_tunnel);
}
