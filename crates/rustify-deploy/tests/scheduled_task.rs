//! ScheduledTaskHandler + dispatcher against the scripted [`FakeExecutor`]:
//! the exact `docker exec <c> sh -c '...'` command with quote escaping, the
//! recorded execution status, retry-on-failure, and dispatcher due-selection.

use std::sync::Arc;

use sqlx::PgPool;

use rustify_db::repos::scheduled_tasks::{NewScheduledTask, ScheduledTaskRepo};
use rustify_deploy::scheduled_task::{SCHEDULED_TASK_KIND, run_scheduled_task};
use rustify_deploy::{dispatch_due_tasks, task_dispatcher_task};
use rustify_jobs::JobQueue;

mod common;
mod fake;

use common::{Fixture, deps, init_secret_key, new_app, setup};
use fake::FakeExecutor;

/// The docker-ps JSON line the fake returns for an application container lookup.
fn ps_line(app_id: i64, name: &str) -> String {
    format!(
        r#"{{"ID":"abc123","Names":"{name}","Image":"img:tag","State":"running","Labels":"rustify.managed=true,rustify.applicationId={app_id}"}}"#
    )
}

async fn run_app(pool: &PgPool, fx: &Fixture) -> (i64, String) {
    let (id, uuid) = new_app(pool, fx, "nixpacks").await;
    sqlx::query("UPDATE applications SET status = 'running' WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    (id, uuid)
}

async fn make_task(
    pool: &PgPool,
    app_id: i64,
    command: &str,
    frequency: &str,
) -> ScheduledTaskRepo {
    let repo = ScheduledTaskRepo::new(pool.clone());
    repo.create(NewScheduledTask {
        name: "job".into(),
        command: command.into(),
        frequency: frequency.into(),
        container: None,
        timeout: Some(30),
        team_id: None,
        application_id: Some(app_id),
        service_id: None,
    })
    .await
    .unwrap();
    repo
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn handler_runs_exact_docker_exec_with_quote_escaping(pool: PgPool) {
    init_secret_key();
    let fx = setup(&pool, 2).await;
    let (app_id, _uuid) = run_app(&pool, &fx).await;
    let repo = make_task(&pool, app_id, "echo 'hello world'", "every_minute").await;
    let task = &repo.list_by_application(app_id).await.unwrap()[0];
    let execution = repo.create_execution(task.id).await.unwrap();

    let fake = Arc::new(FakeExecutor::new().respond(
        "docker ps -a --filter label=rustify.applicationId",
        &ps_line(app_id, "myapp-container"),
    ));
    let d = deps(&pool, fake.clone());
    run_scheduled_task(&d.deps, &execution.uuid, &[])
        .await
        .unwrap();

    // The exact command Coolify builds: `docker exec c sh -c '<escaped>'`, with
    // the single quotes in `echo 'hello world'` escaped as `'\''`.
    let expected = "docker exec myapp-container sh -c 'echo '\\''hello world'\\'''";
    assert!(
        fake.ran(expected),
        "expected exact command not run; scripts: {:?}",
        fake.scripts()
    );

    let done = repo
        .get_execution_by_uuid(&execution.uuid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(done.status, "success");
    assert!(done.finished_at.is_some());
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn handler_records_failure_and_retries(pool: PgPool) {
    init_secret_key();
    let fx = setup(&pool, 2).await;
    let (app_id, _uuid) = run_app(&pool, &fx).await;
    let repo = make_task(&pool, app_id, "false", "every_minute").await;
    let task = &repo.list_by_application(app_id).await.unwrap()[0];
    let execution = repo.create_execution(task.id).await.unwrap();

    let fake = Arc::new(
        FakeExecutor::new()
            .respond(
                "docker ps -a --filter label=rustify.applicationId",
                &ps_line(app_id, "myapp-container"),
            )
            .respond_full("docker exec", "", "boom", 1),
    );
    let d = deps(&pool, fake.clone());
    // Zero-delay backoff so three attempts run instantly.
    run_scheduled_task(&d.deps, &execution.uuid, &[])
        .await
        .unwrap();

    let exec_calls = fake
        .scripts()
        .iter()
        .filter(|s| s.contains("docker exec"))
        .count();
    assert_eq!(exec_calls, 3, "three attempts (tries = 3)");

    let done = repo
        .get_execution_by_uuid(&execution.uuid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(done.status, "failed");
    assert_eq!(done.message.as_deref(), Some("boom"));
    assert!(done.error_details.is_some());
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn handler_skips_when_resource_not_running(pool: PgPool) {
    init_secret_key();
    let fx = setup(&pool, 2).await;
    let (app_id, _uuid) = new_app(&pool, &fx, "nixpacks").await; // status defaults to 'exited'
    let repo = make_task(&pool, app_id, "echo hi", "every_minute").await;
    let task = &repo.list_by_application(app_id).await.unwrap()[0];
    let execution = repo.create_execution(task.id).await.unwrap();

    let fake = Arc::new(FakeExecutor::new());
    let d = deps(&pool, fake.clone());
    run_scheduled_task(&d.deps, &execution.uuid, &[])
        .await
        .unwrap();

    assert!(
        !fake.scripts().iter().any(|s| s.contains("docker exec")),
        "must not exec when the resource is not running"
    );
    let done = repo
        .get_execution_by_uuid(&execution.uuid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(done.status, "failed");
    assert!(done.message.as_deref().unwrap().contains("not running"));
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn dispatcher_selects_due_running_tasks(pool: PgPool) {
    init_secret_key();
    let fx = setup(&pool, 2).await;
    let (running_id, _u1) = run_app(&pool, &fx).await;
    let (stopped_id, _u2) = new_app(&pool, &fx, "nixpacks").await; // 'exited'

    let repo = ScheduledTaskRepo::new(pool.clone());
    // Due + running → dispatched.
    make_task(&pool, running_id, "echo a", "every_minute").await;
    // Due but resource stopped → skipped.
    make_task(&pool, stopped_id, "echo b", "every_minute").await;
    // Running but not due (yearly, and today is not Jan 1) → skipped.
    make_task(&pool, running_id, "echo c", "yearly").await;
    // Disabled → skipped.
    let disabled = repo
        .create(NewScheduledTask {
            name: "off".into(),
            command: "echo d".into(),
            frequency: "every_minute".into(),
            container: None,
            timeout: None,
            team_id: None,
            application_id: Some(running_id),
            service_id: None,
        })
        .await
        .unwrap();
    repo.update(
        &disabled.uuid,
        &rustify_db::repos::ScheduledTaskPatch {
            enabled: Some(false),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let fake = Arc::new(FakeExecutor::new());
    let d = deps(&pool, fake);
    let queue = JobQueue::new(pool.clone());
    // A wall-clock instant that is not Jan 1 midnight so `yearly` is not due.
    let now = chrono::Utc::now();
    let dispatched = dispatch_due_tasks(&d.deps, &queue, now).await.unwrap();
    assert_eq!(dispatched, 1, "only the due, running, enabled task fires");

    // One `scheduled_task` job was enqueued and one execution opened.
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = $1")
        .bind(SCHEDULED_TASK_KIND)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(jobs, 1);
    let execs: i64 = sqlx::query_scalar("SELECT count(*) FROM scheduled_task_executions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(execs, 1);

    // A second sweep in the same minute does not double-fire.
    let again = dispatch_due_tasks(&d.deps, &queue, now).await.unwrap();
    assert_eq!(again, 0, "dedup within the minute");

    // The dispatcher closure factory is wired the same way as status_sync.
    let _closure = task_dispatcher_task(d.deps.clone(), queue);
}
