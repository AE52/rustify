//! Application stop/restart handlers against the scripted [`FakeExecutor`].

use std::sync::Arc;

use sqlx::PgPool;

use rustify_db::repos::ApplicationRepo;
use rustify_deploy::{restart_application, stop_application};

mod common;
mod fake;

use common::{new_app, setup};
use fake::FakeExecutor;

async fn app_status(pool: &PgPool, uuid: &str) -> String {
    ApplicationRepo::new(pool.clone())
        .get_by_uuid(uuid)
        .await
        .unwrap()
        .unwrap()
        .status
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn stop_removes_labelled_containers_and_marks_exited(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let (_id, uuid) = new_app(&pool, &fx, "nixpacks").await;

    let fake = Arc::new(FakeExecutor::new());
    let d = common::deps(&pool, fake.clone());
    stop_application(&d.deps, &uuid).await.unwrap();

    // The stop targets exactly this application's production containers.
    let stop = fake
        .index_of(&format!("--filter label=rustify.applicationUuid={uuid}"))
        .expect("stop targeted the app label");
    let script = &fake.scripts()[stop];
    assert!(script.contains("--filter label=rustify.pullRequestId=0"));
    assert!(script.contains("docker stop -t 30"));
    assert!(script.contains("docker rm -f"));
    // No compose-up on a plain stop.
    assert!(
        !fake.ran("docker compose"),
        "stop must not bring the app up"
    );

    assert_eq!(app_status(&pool, &uuid).await, "exited");
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn restart_stops_then_ups_and_marks_running(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let (_id, uuid) = new_app(&pool, &fx, "nixpacks").await;

    let fake = Arc::new(FakeExecutor::new());
    let d = common::deps(&pool, fake.clone());
    restart_application(&d.deps, &uuid).await.unwrap();

    let stop = fake
        .index_of(&format!("--filter label=rustify.applicationUuid={uuid}"))
        .expect("restart stops the old container");
    let up = fake
        .index_of("docker compose -f docker-compose.yml up -d")
        .expect("restart brings the stored compose up");
    assert!(
        stop < up,
        "must stop the old container before bringing it up"
    );
    // The rolling-outage fix must not leak into the restart path either.
    assert!(!fake.scripts()[up].contains("--remove-orphans"));

    assert_eq!(app_status(&pool, &uuid).await, "running");
}
