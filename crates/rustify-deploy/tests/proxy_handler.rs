//! Proxy start/stop/restart handlers against the scripted [`FakeExecutor`].

use std::sync::Arc;

use sqlx::PgPool;

use rustify_db::repos::ServerRepo;
use rustify_deploy::{restart_proxy, start_proxy, stop_proxy};

mod common;
mod fake;

use common::setup;
use fake::FakeExecutor;

async fn server_uuid(pool: &PgPool, server_id: i64) -> String {
    sqlx::query_scalar("SELECT uuid FROM servers WHERE id = $1")
        .bind(server_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn proxy_status(pool: &PgPool, server_id: i64) -> String {
    ServerRepo::new(pool.clone())
        .settings(server_id)
        .await
        .unwrap()
        .unwrap()
        .proxy_status
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn start_brings_proxy_up_and_marks_running(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let uuid = server_uuid(&pool, fx.server_id).await;

    let fake = Arc::new(FakeExecutor::new());
    let d = common::deps(&pool, fake.clone());
    start_proxy(&d.deps, &uuid).await.unwrap();

    assert!(fake.ran("docker network create --attachable rustify"));
    assert!(fake.ran(
        "docker compose -f /data/rustify/proxy/docker-compose.yml up -d --wait --remove-orphans"
    ));
    // The compose body carries the Traefik image (written via heredoc).
    assert!(fake.ran("image: traefik:v3.6"));

    assert_eq!(proxy_status(&pool, fx.server_id).await, "running");
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn stop_removes_proxy_and_marks_exited(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let uuid = server_uuid(&pool, fx.server_id).await;

    let fake = Arc::new(FakeExecutor::new());
    let d = common::deps(&pool, fake.clone());
    stop_proxy(&d.deps, &uuid).await.unwrap();

    assert!(fake.ran("docker stop -t=30 rustify-proxy"));
    assert!(fake.ran("docker rm -f rustify-proxy"));
    assert_eq!(proxy_status(&pool, fx.server_id).await, "exited");
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn restart_stops_then_starts(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let uuid = server_uuid(&pool, fx.server_id).await;

    let fake = Arc::new(FakeExecutor::new());
    let d = common::deps(&pool, fake.clone());
    restart_proxy(&d.deps, &uuid).await.unwrap();

    let stop = fake
        .index_of("docker rm -f rustify-proxy")
        .expect("restart stops the proxy");
    let start = fake
        .index_of("docker compose -f /data/rustify/proxy/docker-compose.yml up -d")
        .expect("restart brings the proxy up");
    assert!(stop < start, "stop must precede start on restart");
    assert_eq!(proxy_status(&pool, fx.server_id).await, "running");
}
