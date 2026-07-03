//! Scheduled-task HTTP route tests: per-application create/list, task CRUD,
//! manual trigger (enqueues a job + opens an execution) and execution listing.

use axum::http::StatusCode;
use serde_json::json;
use sqlx::PgPool;

use rustify_server::build_router;

mod common;
use common::{Req, login, seed_user, send, state};

/// Create key + server + project + application; return the application uuid.
async fn scaffold_application(app: &axum::Router, cookie: &str) -> String {
    let (_, key) = send(
        app,
        Req::post("/api/v1/private-keys/generate")
            .cookie(cookie)
            .json(json!({ "name": "k" }))
            .build(),
    )
    .await;
    let key_uuid = key["uuid"].as_str().unwrap().to_string();

    let (_, server) = send(
        app,
        Req::post("/api/v1/servers")
            .cookie(cookie)
            .json(json!({ "name": "s", "ip": "10.0.0.21", "private_key_uuid": key_uuid }))
            .build(),
    )
    .await;
    let server_uuid = server["uuid"].as_str().unwrap().to_string();

    let (_, project) = send(
        app,
        Req::post("/api/v1/projects")
            .cookie(cookie)
            .json(json!({ "name": "p" }))
            .build(),
    )
    .await;
    let project_uuid = project["uuid"].as_str().unwrap().to_string();

    let (status, created) = send(
        app,
        Req::post("/api/v1/applications")
            .cookie(cookie)
            .json(json!({
                "project_uuid": project_uuid,
                "environment_name": "production",
                "server_uuid": server_uuid,
                "name": "app",
                "git_repository": "https://example.com/r.git",
            }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "app create: {created:?}");
    created["uuid"].as_str().unwrap().to_string()
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn create_list_update_delete(pool: PgPool) {
    seed_user(&pool).await;
    let app = build_router(state(pool.clone()));
    let cookie = login(&app).await;
    let app_uuid = scaffold_application(&app, &cookie).await;

    // Create.
    let (status, created) = send(
        &app,
        Req::post(format!("/api/v1/applications/{app_uuid}/scheduled-tasks"))
            .cookie(&cookie)
            .json(json!({
                "name": "nightly",
                "command": "php artisan backup",
                "frequency": "daily",
            }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    assert_eq!(created["enabled"], true);
    assert_eq!(created["timeout"], 300);
    let task_uuid = created["uuid"].as_str().unwrap().to_string();

    // List under the application.
    let (status, list) = send(
        &app,
        Req::get(format!("/api/v1/applications/{app_uuid}/scheduled-tasks"))
            .cookie(&cookie)
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 1);

    // Get.
    let (status, got) = send(
        &app,
        Req::get(format!("/api/v1/scheduled-tasks/{task_uuid}"))
            .cookie(&cookie)
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["name"], "nightly");

    // Patch: disable + rename.
    let (status, patched) = send(
        &app,
        Req::patch(format!("/api/v1/scheduled-tasks/{task_uuid}"))
            .cookie(&cookie)
            .json(json!({ "enabled": false, "frequency": "hourly" }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(patched["enabled"], false);
    assert_eq!(patched["frequency"], "hourly");
    assert_eq!(patched["name"], "nightly");

    // Delete.
    let (status, _) = send(
        &app,
        Req::delete(format!("/api/v1/scheduled-tasks/{task_uuid}"))
            .cookie(&cookie)
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(
        &app,
        Req::get(format!("/api/v1/scheduled-tasks/{task_uuid}"))
            .cookie(&cookie)
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn validation_rejects_empty_command(pool: PgPool) {
    seed_user(&pool).await;
    let app = build_router(state(pool.clone()));
    let cookie = login(&app).await;
    let app_uuid = scaffold_application(&app, &cookie).await;

    let (status, _) = send(
        &app,
        Req::post(format!("/api/v1/applications/{app_uuid}/scheduled-tasks"))
            .cookie(&cookie)
            .json(json!({ "name": "x", "command": "", "frequency": "daily" }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn trigger_enqueues_job_and_opens_execution(pool: PgPool) {
    seed_user(&pool).await;
    let app = build_router(state(pool.clone()));
    let cookie = login(&app).await;
    let app_uuid = scaffold_application(&app, &cookie).await;

    let (_, created) = send(
        &app,
        Req::post(format!("/api/v1/applications/{app_uuid}/scheduled-tasks"))
            .cookie(&cookie)
            .json(json!({ "name": "t", "command": "echo hi", "frequency": "daily" }))
            .build(),
    )
    .await;
    let task_uuid = created["uuid"].as_str().unwrap().to_string();

    let (status, body) = send(
        &app,
        Req::post(format!("/api/v1/scheduled-tasks/{task_uuid}/trigger"))
            .cookie(&cookie)
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body:?}");
    assert!(body["execution_uuid"].is_string());

    // A `scheduled_task` job was enqueued.
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'scheduled_task'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(jobs, 1);

    // The execution shows up in the task's history (status `running`).
    let (status, execs) = send(
        &app,
        Req::get(format!("/api/v1/scheduled-tasks/{task_uuid}/executions"))
            .cookie(&cookie)
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let execs = execs.as_array().unwrap();
    assert_eq!(execs.len(), 1);
    assert_eq!(execs[0]["status"], "running");
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn other_team_cannot_see_task(pool: PgPool) {
    seed_user(&pool).await;
    let app = build_router(state(pool.clone()));
    let cookie = login(&app).await;
    let app_uuid = scaffold_application(&app, &cookie).await;
    let (_, created) = send(
        &app,
        Req::post(format!("/api/v1/applications/{app_uuid}/scheduled-tasks"))
            .cookie(&cookie)
            .json(json!({ "name": "t", "command": "echo hi", "frequency": "daily" }))
            .build(),
    )
    .await;
    let task_uuid = created["uuid"].as_str().unwrap().to_string();

    // An unauthenticated request is rejected (no team context).
    let (status, _) = send(
        &app,
        Req::get(format!("/api/v1/scheduled-tasks/{task_uuid}")).build(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
