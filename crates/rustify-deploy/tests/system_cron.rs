//! Daily record-prune (`cleanup_old_records`): old deployment logs and finished
//! scheduled-task executions are deleted; recent ones are kept.

use sqlx::PgPool;

use rustify_db::repos::scheduled_tasks::{NewScheduledTask, ScheduledTaskRepo};
use rustify_deploy::cleanup_old_records;

mod common;

use common::{new_app, queue, setup};

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn prunes_old_logs_and_executions_only(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let (app_id, _uuid) = new_app(&pool, &fx, "nixpacks").await;
    let deployment = queue(&pool, app_id, fx.server_id, false).await;

    // One old deployment log (35 days) and one fresh one.
    sqlx::query(
        "INSERT INTO deployment_logs (deployment_id, ord, kind, content, created_at)
         VALUES ($1, 1, 'stdout', 'old', now() - interval '35 days'),
                ($1, 2, 'stdout', 'new', now())",
    )
    .bind(deployment.id)
    .execute(&pool)
    .await
    .unwrap();

    // One old finished execution, one fresh finished execution, one old but
    // still-running execution (must be kept: only finished ones are pruned).
    let repo = ScheduledTaskRepo::new(pool.clone());
    let task = repo
        .create(NewScheduledTask {
            name: "t".into(),
            command: "echo".into(),
            frequency: "daily".into(),
            container: None,
            timeout: None,
            team_id: None,
            application_id: Some(app_id),
            service_id: None,
        })
        .await
        .unwrap();
    let old = repo.create_execution(task.id).await.unwrap();
    let fresh = repo.create_execution(task.id).await.unwrap();
    let old_running = repo.create_execution(task.id).await.unwrap();
    sqlx::query(
        "UPDATE scheduled_task_executions
           SET started_at = now() - interval '40 days', finished_at = now() - interval '40 days',
               status = 'success'
         WHERE id = $1",
    )
    .bind(old.id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE scheduled_task_executions SET started_at = now() - interval '40 days' WHERE id = $1",
    )
    .bind(old_running.id)
    .execute(&pool)
    .await
    .unwrap();

    let (logs, execs) = cleanup_old_records(&pool, 7).await.unwrap();
    assert_eq!(logs, 1, "only the 35-day log is pruned");
    assert_eq!(execs, 1, "only the old finished execution is pruned");

    assert!(
        repo.get_execution_by_uuid(&old.uuid)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        repo.get_execution_by_uuid(&fresh.uuid)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        repo.get_execution_by_uuid(&old_running.uuid)
            .await
            .unwrap()
            .is_some(),
        "unfinished executions are never pruned"
    );

    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM deployment_logs WHERE deployment_id = $1")
            .bind(deployment.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(remaining, 1, "the fresh log survives");
}
