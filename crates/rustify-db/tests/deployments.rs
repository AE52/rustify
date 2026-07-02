//! DeploymentRepo: state-machine race, admission control, log ordering.

use chrono::Utc;
use sqlx::PgPool;

use rustify_core::{DeploymentStatus, LogLine};
use rustify_db::repos::deployments::{DeploymentRepo, NewDeployment};

mod common;
use common::{new_app, setup};

fn queued(app_id: i64, server_id: i64) -> NewDeployment {
    NewDeployment {
        application_id: app_id,
        server_id,
        commit_sha: Some("HEAD".into()),
        ..Default::default()
    }
}

async fn inflight_count(pool: &PgPool, server_id: i64) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM deployments WHERE server_id = $1 AND status = 'in_progress'",
    )
    .bind(server_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test]
async fn transition_race_exactly_one_winner(pool: PgPool) {
    let fx = setup(&pool, 5).await;
    let app = new_app(&pool, &fx).await;
    let repo = DeploymentRepo::new(pool.clone());
    let dep = repo.create_queued(queued(app, fx.server_id)).await.unwrap();

    // Two tasks race the same Queued -> InProgress transition.
    let a = tokio::spawn({
        let repo = repo.clone();
        async move {
            repo.transition(dep.id, DeploymentStatus::InProgress)
                .await
                .unwrap()
        }
    });
    let b = tokio::spawn({
        let repo = repo.clone();
        async move {
            repo.transition(dep.id, DeploymentStatus::InProgress)
                .await
                .unwrap()
        }
    });
    let (ra, rb) = (a.await.unwrap(), b.await.unwrap());

    assert_ne!(ra, rb, "exactly one transition must win, got {ra}/{rb}");
    assert!(ra || rb, "at least one must win");

    let status: DeploymentStatus =
        sqlx::query_scalar("SELECT status FROM deployments WHERE id = $1")
            .bind(dep.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, DeploymentStatus::InProgress);
}

#[sqlx::test]
async fn illegal_transition_rejected_in_sql(pool: PgPool) {
    let fx = setup(&pool, 5).await;
    let app = new_app(&pool, &fx).await;
    let repo = DeploymentRepo::new(pool.clone());
    let dep = repo.create_queued(queued(app, fx.server_id)).await.unwrap();

    // Queued -> Finished is illegal (contract C2): must not move the row.
    assert!(
        !repo
            .transition(dep.id, DeploymentStatus::Finished)
            .await
            .unwrap()
    );
    let status: DeploymentStatus =
        sqlx::query_scalar("SELECT status FROM deployments WHERE id = $1")
            .bind(dep.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, DeploymentStatus::Queued);
}

#[sqlx::test]
async fn admission_one_inflight_per_application(pool: PgPool) {
    // High server cap so only the per-application rule can bind.
    let fx = setup(&pool, 5).await;
    let app = new_app(&pool, &fx).await;
    let repo = DeploymentRepo::new(pool.clone());
    for _ in 0..3 {
        repo.create_queued(queued(app, fx.server_id)).await.unwrap();
    }

    let first = repo.next_queuable(fx.server_id).await.unwrap();
    assert!(
        first.is_some(),
        "first queued deployment should be admitted"
    );
    // A second in-flight for the same application is forbidden.
    assert!(repo.next_queuable(fx.server_id).await.unwrap().is_none());
    assert!(repo.next_queuable(fx.server_id).await.unwrap().is_none());

    assert_eq!(inflight_count(&pool, fx.server_id).await, 1);
}

#[sqlx::test]
async fn admission_respects_server_concurrent_builds(pool: PgPool) {
    // Cap 2, three distinct applications each with one queued deployment.
    let fx = setup(&pool, 2).await;
    let repo = DeploymentRepo::new(pool.clone());
    for _ in 0..3 {
        let app = new_app(&pool, &fx).await;
        repo.create_queued(queued(app, fx.server_id)).await.unwrap();
    }

    assert!(repo.next_queuable(fx.server_id).await.unwrap().is_some());
    assert!(repo.next_queuable(fx.server_id).await.unwrap().is_some());
    // Cap reached: the third must be refused.
    assert!(repo.next_queuable(fx.server_id).await.unwrap().is_none());

    assert_eq!(inflight_count(&pool, fx.server_id).await, 2);
}

#[sqlx::test]
async fn next_queuable_is_fifo_by_created_at(pool: PgPool) {
    let fx = setup(&pool, 1).await;
    let repo = DeploymentRepo::new(pool.clone());

    let app1 = new_app(&pool, &fx).await;
    let app2 = new_app(&pool, &fx).await;
    let first = repo
        .create_queued(queued(app1, fx.server_id))
        .await
        .unwrap();
    // Force a strictly later created_at so ordering is deterministic.
    sqlx::query("UPDATE deployments SET created_at = now() + interval '1 second' WHERE id <> $1")
        .bind(first.id)
        .execute(&pool)
        .await
        .unwrap();
    let _second = repo
        .create_queued(queued(app2, fx.server_id))
        .await
        .unwrap();

    let claimed = repo.next_queuable(fx.server_id).await.unwrap().unwrap();
    assert_eq!(claimed.id, first.id, "oldest queued deployment runs first");
}

#[sqlx::test]
async fn cancel_requested_reflects_cancelled_status(pool: PgPool) {
    let fx = setup(&pool, 5).await;
    let app = new_app(&pool, &fx).await;
    let repo = DeploymentRepo::new(pool.clone());
    let dep = repo.create_queued(queued(app, fx.server_id)).await.unwrap();

    assert!(!repo.cancel_requested(dep.id).await.unwrap());
    assert!(
        repo.transition(dep.id, DeploymentStatus::Cancelled)
            .await
            .unwrap()
    );
    assert!(repo.cancel_requested(dep.id).await.unwrap());
}

#[sqlx::test]
async fn append_logs_preserves_order_across_batches(pool: PgPool) {
    let fx = setup(&pool, 5).await;
    let app = new_app(&pool, &fx).await;
    let repo = DeploymentRepo::new(pool.clone());
    let dep = repo.create_queued(queued(app, fx.server_id)).await.unwrap();

    let line = |order: i64, content: &str| LogLine {
        order,
        kind: "stdout".into(),
        content: content.into(),
        hidden: false,
        batch: 1,
        timestamp: Utc::now(),
    };

    // Two separate append calls; global order must be preserved on read.
    repo.append_logs(dep.id, &[line(0, "a"), line(1, "b")])
        .await
        .unwrap();
    repo.append_logs(dep.id, &[line(2, "c"), line(3, "d")])
        .await
        .unwrap();
    // Empty batch is a no-op.
    repo.append_logs(dep.id, &[]).await.unwrap();

    let logs = repo.logs(dep.id).await.unwrap();
    let orders: Vec<i64> = logs.iter().map(|l| l.order).collect();
    let contents: Vec<&str> = logs.iter().map(|l| l.content.as_str()).collect();
    assert_eq!(orders, vec![0, 1, 2, 3]);
    assert_eq!(contents, vec!["a", "b", "c", "d"]);
}
