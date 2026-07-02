//! End-to-end engine integration: log persistence + queue drain.

use std::sync::Arc;

use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

use rustify_core::DeploymentStatus;
use rustify_db::repos::deployments::DeploymentRepo;
use rustify_deploy::run_deployment;

mod common;
mod fake;

use common::{LS_REMOTE_OK, new_app, queue, setup};
use fake::FakeExecutor;

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn full_deploy_writes_ordered_logs_and_finishes(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let (app_id, _uuid) = new_app(&pool, &fx, "nixpacks").await;
    let dep = queue(&pool, app_id, fx.server_id, false).await;

    let fake = Arc::new(FakeExecutor::new().respond("git ls-remote", LS_REMOTE_OK));
    let d = common::deps(&pool, fake.clone());
    run_deployment(&d.deps, &CancellationToken::new(), &dep.uuid)
        .await
        .unwrap();

    let logs = DeploymentRepo::new(pool.clone())
        .logs(dep.id)
        .await
        .unwrap();
    assert!(!logs.is_empty(), "a real deploy writes log lines");
    let orders: Vec<i64> = logs.iter().map(|l| l.order).collect();
    let expected: Vec<i64> = (0..orders.len() as i64).collect();
    assert_eq!(orders, expected, "log order is a dense monotonic sequence");
    assert_eq!(
        sqlx::query_scalar::<_, DeploymentStatus>("SELECT status FROM deployments WHERE id = $1")
            .bind(dep.id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        DeploymentStatus::Finished
    );
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn queue_drain_triggers_next_queued_deployment(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let (app_id, _uuid) = new_app(&pool, &fx, "nixpacks").await;
    // Two queued deployments for the same application; the second cannot run
    // until the first finishes (admission: one in-flight per application).
    let first = queue(&pool, app_id, fx.server_id, false).await;
    let second = queue(&pool, app_id, fx.server_id, false).await;

    let fake = Arc::new(FakeExecutor::new().respond("git ls-remote", LS_REMOTE_OK));
    let d = common::deps(&pool, fake.clone());
    run_deployment(&d.deps, &CancellationToken::new(), &first.uuid)
        .await
        .unwrap();

    // The queue drain claimed the second deployment and enqueued a deploy job.
    let second_status: DeploymentStatus =
        sqlx::query_scalar("SELECT status FROM deployments WHERE id = $1")
            .bind(second.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        second_status,
        DeploymentStatus::InProgress,
        "next queued deployment is claimed after the first finishes"
    );

    let enqueued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE kind = 'deploy' AND payload->>'deployment_uuid' = $1",
    )
    .bind(&second.uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        enqueued, 1,
        "a deploy job was enqueued for the next deployment"
    );
}
