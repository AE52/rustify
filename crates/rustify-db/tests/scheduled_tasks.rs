//! ScheduledTaskRepo: create/read/update/delete, the app/service XOR
//! constraint, and execution lifecycle.

use sqlx::PgPool;

use rustify_db::repos::scheduled_tasks::{NewScheduledTask, ScheduledTaskPatch, ScheduledTaskRepo};

mod common;
use common::{new_app, setup};

fn new_task(application_id: Option<i64>, service_id: Option<i64>) -> NewScheduledTask {
    NewScheduledTask {
        name: "nightly".into(),
        command: "php artisan backup:run".into(),
        frequency: "daily".into(),
        container: None,
        timeout: None,
        team_id: None,
        application_id,
        service_id,
    }
}

#[sqlx::test]
async fn create_defaults_and_roundtrip(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let app_id = new_app(&pool, &fx).await;
    let repo = ScheduledTaskRepo::new(pool.clone());

    let task = repo.create(new_task(Some(app_id), None)).await.unwrap();
    assert!(task.enabled, "enabled defaults true");
    assert_eq!(task.timeout, 300, "timeout defaults to 300");
    assert_eq!(task.application_id, Some(app_id));
    assert!(task.service_id.is_none());

    let got = repo.get_by_uuid(&task.uuid).await.unwrap().unwrap();
    assert_eq!(got.id, task.id);
    assert_eq!(
        repo.get_by_id(task.id).await.unwrap().unwrap().uuid,
        task.uuid
    );

    let listed = repo.list_by_application(app_id).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(
        repo.list_enabled()
            .await
            .unwrap()
            .iter()
            .any(|t| t.id == task.id)
    );
}

#[sqlx::test]
async fn xor_constraint_rejects_orphan_task(pool: PgPool) {
    common::init_secret_key();
    setup(&pool, 2).await;
    let repo = ScheduledTaskRepo::new(pool.clone());
    // Neither application_id nor service_id set violates the CHECK constraint.
    let err = repo.create(new_task(None, None)).await;
    assert!(err.is_err(), "task with no resource must be rejected");
}

#[sqlx::test]
async fn update_and_delete(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let app_id = new_app(&pool, &fx).await;
    let repo = ScheduledTaskRepo::new(pool.clone());
    let task = repo.create(new_task(Some(app_id), None)).await.unwrap();

    let patched = repo
        .update(
            &task.uuid,
            &ScheduledTaskPatch {
                enabled: Some(false),
                frequency: Some("hourly".into()),
                container: Some("web".into()),
                timeout: Some(120),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert!(!patched.enabled);
    assert_eq!(patched.frequency, "hourly");
    assert_eq!(patched.container.as_deref(), Some("web"));
    assert_eq!(patched.timeout, 120);
    assert_eq!(patched.name, "nightly", "COALESCE leaves name unchanged");

    // Disabled tasks drop out of the dispatcher candidate set.
    assert!(repo.list_enabled().await.unwrap().is_empty());

    assert!(repo.delete(&task.uuid).await.unwrap());
    assert!(repo.get_by_uuid(&task.uuid).await.unwrap().is_none());
}

#[sqlx::test]
async fn execution_lifecycle(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let app_id = new_app(&pool, &fx).await;
    let repo = ScheduledTaskRepo::new(pool.clone());
    let task = repo.create(new_task(Some(app_id), None)).await.unwrap();

    let exec = repo.create_execution(task.id).await.unwrap();
    assert_eq!(exec.status, "running", "opens as running");
    assert!(exec.finished_at.is_none());

    let looked_up = repo
        .get_execution_by_uuid(&exec.uuid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(looked_up.id, exec.id);

    repo.finish_execution(exec.id, "success", Some("ok"), None, 3)
        .await
        .unwrap();
    let done = repo
        .get_execution_by_uuid(&exec.uuid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(done.status, "success");
    assert_eq!(done.message.as_deref(), Some("ok"));
    assert_eq!(done.duration, Some(3));
    assert!(done.finished_at.is_some());

    let list = repo.executions(task.id, 10).await.unwrap();
    assert_eq!(list.len(), 1);

    // Deleting the task cascades to its executions.
    assert!(repo.delete(&task.uuid).await.unwrap());
    assert!(
        repo.get_execution_by_uuid(&exec.uuid)
            .await
            .unwrap()
            .is_none()
    );
}

#[sqlx::test]
async fn has_execution_since_dedup_window(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let app_id = new_app(&pool, &fx).await;
    let repo = ScheduledTaskRepo::new(pool.clone());
    let task = repo.create(new_task(Some(app_id), None)).await.unwrap();

    let before = chrono::Utc::now();
    assert!(!repo.has_execution_since(task.id, before).await.unwrap());
    repo.create_execution(task.id).await.unwrap();
    assert!(repo.has_execution_since(task.id, before).await.unwrap());
    // A window strictly in the future sees no executions.
    let future = chrono::Utc::now() + chrono::Duration::minutes(5);
    assert!(!repo.has_execution_since(task.id, future).await.unwrap());
}
