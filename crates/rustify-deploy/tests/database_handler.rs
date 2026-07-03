//! Database lifecycle handlers against the scripted [`FakeExecutor`]: the
//! start/stop command sequence and the public-proxy path.

use std::sync::Arc;

use sqlx::PgPool;

use rustify_core::DatabaseEngine;
use rustify_db::repos::databases::{DatabaseRepo, NewDatabase};
use rustify_deploy::{start_database, stop_database};

mod common;
mod fake;

use common::{deps, init_secret_key, setup};
use fake::FakeExecutor;

async fn make_db(
    pool: &PgPool,
    fx: &common::Fixture,
    engine: DatabaseEngine,
    is_public: bool,
    public_port: Option<i32>,
) -> String {
    DatabaseRepo::new(pool.clone())
        .create(NewDatabase {
            environment_id: fx.environment_id,
            destination_id: fx.destination_id,
            name: "db".into(),
            engine: engine.as_str().to_string(),
            image: engine.descriptor().default_image.to_string(),
            credentials: engine.default_credentials(),
            is_public,
            public_port,
        })
        .await
        .unwrap()
        .uuid
}

/// The first recorded script index containing `needle` (panics if absent).
fn idx(fake: &FakeExecutor, needle: &str) -> usize {
    fake.index_of(needle).unwrap_or_else(|| {
        panic!(
            "no script contained {needle:?}; scripts: {:?}",
            fake.scripts()
        )
    })
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn start_writes_compose_pulls_and_ups(pool: PgPool) {
    init_secret_key();
    let fx = setup(&pool, 2).await;
    let uuid = make_db(&pool, &fx, DatabaseEngine::Postgresql, false, None).await;

    let fake = Arc::new(FakeExecutor::new());
    let d = deps(&pool, fake.clone());
    start_database(&d.deps, &uuid).await.unwrap();

    let dir = format!("/data/rustify/databases/{uuid}");
    // Ordered: mkdir -> write compose -> pull -> stop -> rm -> up.
    let mkdir = idx(&fake, &format!("mkdir -p {dir}"));
    let write = idx(&fake, &format!("tee {dir}/docker-compose.yml"));
    let pull = idx(
        &fake,
        &format!("docker compose -f {dir}/docker-compose.yml pull"),
    );
    let stop = idx(&fake, &format!("docker stop -t 10 {uuid}"));
    let rm = idx(&fake, &format!("docker rm -f {uuid}"));
    let up = idx(
        &fake,
        &format!("docker compose -f {dir}/docker-compose.yml up -d"),
    );
    assert!(mkdir < write && write < pull && pull < stop && stop < rm && rm < up);

    // The compose is written base64-encoded (secrets never on the command line).
    let write_script = &fake.scripts()[write];
    assert!(write_script.contains("| base64 -d | tee"));

    // No proxy work for a private database.
    assert!(!fake.ran(&format!("{uuid}-proxy")));

    // Status persisted + started_at stamped.
    let db = DatabaseRepo::new(pool.clone())
        .get_by_uuid(&uuid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(db.status, "running");
    assert!(db.started_at.is_some());
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn public_database_brings_up_proxy(pool: PgPool) {
    init_secret_key();
    let fx = setup(&pool, 2).await;
    let uuid = make_db(&pool, &fx, DatabaseEngine::Postgresql, true, Some(5433)).await;

    let fake = Arc::new(FakeExecutor::new());
    let d = deps(&pool, fake.clone());
    start_database(&d.deps, &uuid).await.unwrap();

    let proxy_dir = format!("/data/rustify/databases/{uuid}/proxy");
    // Proxy nginx.conf + compose written and brought up.
    assert!(fake.ran(&format!("tee {proxy_dir}/nginx.conf")));
    assert!(fake.ran(&format!("tee {proxy_dir}/docker-compose.yml")));
    let up = idx(
        &fake,
        &format!("docker compose -f {proxy_dir}/docker-compose.yml up -d"),
    );
    let write_nginx = idx(&fake, &format!("tee {proxy_dir}/nginx.conf"));
    assert!(write_nginx < up);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn stop_removes_container_and_proxy(pool: PgPool) {
    init_secret_key();
    let fx = setup(&pool, 2).await;
    let uuid = make_db(&pool, &fx, DatabaseEngine::Postgresql, true, Some(5433)).await;
    DatabaseRepo::new(pool.clone())
        .set_status(fx_db_id(&pool, &uuid).await, "running")
        .await
        .unwrap();

    let fake = Arc::new(FakeExecutor::new());
    let d = deps(&pool, fake.clone());
    stop_database(&d.deps, &uuid).await.unwrap();

    assert!(fake.ran(&format!("docker stop -t 30 {uuid}")));
    assert!(fake.ran(&format!("docker rm -f {uuid}")));
    assert!(fake.ran(&format!("docker stop -t 30 {uuid}-proxy")));

    let db = DatabaseRepo::new(pool.clone())
        .get_by_uuid(&uuid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(db.status, "exited");
}

async fn fx_db_id(pool: &PgPool, uuid: &str) -> i64 {
    DatabaseRepo::new(pool.clone())
        .get_by_uuid(uuid)
        .await
        .unwrap()
        .unwrap()
        .id
}
