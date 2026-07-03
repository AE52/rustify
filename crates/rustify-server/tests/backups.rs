//! Backup + S3 storage HTTP routes over `tower::ServiceExt::oneshot`.

use axum::http::StatusCode;
use serde_json::json;
use sqlx::PgPool;

use rustify_server::build_router;

mod common;
use common::{Req, login, seed_user, send, state};

/// Private key + server + project + a postgres database; returns the db uuid.
async fn scaffold_database(app: &axum::Router, cookie: &str) -> String {
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
            .json(json!({ "name": "s", "ip": "10.0.0.9", "private_key_uuid": key_uuid }))
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

    let (status, db) = send(
        app,
        Req::post("/api/v1/databases")
            .cookie(cookie)
            .json(json!({
                "project_uuid": project_uuid,
                "environment_name": "production",
                "server_uuid": server_uuid,
                "engine": "postgresql",
                "name": "pg",
            }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    db["uuid"].as_str().unwrap().to_string()
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn s3_storage_crud_and_secrets_hidden(pool: PgPool) {
    seed_user(&pool).await;
    let app = build_router(state(pool.clone()));
    let cookie = login(&app).await;

    let (status, created) = send(
        &app,
        Req::post("/api/v1/s3-storages")
            .cookie(&cookie)
            .json(json!({
                "name": "b",
                "endpoint": "https://s3.example.com",
                "bucket": "mybucket",
                "key": "AKIA",
                "secret": "sk",
            }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let uuid = created["uuid"].as_str().unwrap().to_string();
    assert_eq!(created["region"], "us-east-1");
    // Secrets never leak.
    assert!(created.get("key").is_none());
    assert!(created.get("secret").is_none());

    // Encrypted at rest.
    let has: bool = sqlx::query_scalar(
        "SELECT octet_length(key_enc) > 0 AND octet_length(secret_enc) > 0 FROM s3_storages WHERE uuid = $1",
    )
    .bind(&uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(has);

    // Test endpoint validates and marks usable.
    let (status, res) = send(
        &app,
        Req::post(format!("/api/v1/s3-storages/{uuid}/test"))
            .cookie(&cookie)
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(res["usable"], true);

    // Delete.
    let (status, _) = send(
        &app,
        Req::delete(format!("/api/v1/s3-storages/{uuid}"))
            .cookie(&cookie)
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn backup_schedule_crud_and_trigger(pool: PgPool) {
    seed_user(&pool).await;
    let app = build_router(state(pool.clone()));
    let cookie = login(&app).await;
    let db_uuid = scaffold_database(&app, &cookie).await;

    // Create a schedule.
    let (status, created) = send(
        &app,
        Req::post(format!("/api/v1/databases/{db_uuid}/backups"))
            .cookie(&cookie)
            .json(json!({
                "frequency": "0 2 * * *",
                "retention_amount_local": 5,
            }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let backup_uuid = created["uuid"].as_str().unwrap().to_string();
    assert_eq!(created["frequency"], "0 2 * * *");
    assert_eq!(created["retention_amount_local"], 5);
    assert_eq!(created["database_uuid"], db_uuid);

    // List for the database.
    let (status, list) = send(
        &app,
        Req::get(format!("/api/v1/databases/{db_uuid}/backups"))
            .cookie(&cookie)
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 1);

    // Patch it.
    let (status, patched) = send(
        &app,
        Req::patch(format!("/api/v1/backups/{backup_uuid}"))
            .cookie(&cookie)
            .json(json!({ "frequency": "*/30 * * * *", "enabled": false }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(patched["frequency"], "*/30 * * * *");
    assert_eq!(patched["enabled"], false);

    // Trigger enqueues a database_backup job with an execution uuid.
    let (status, res) = send(
        &app,
        Req::post(format!("/api/v1/backups/{backup_uuid}/trigger"))
            .cookie(&cookie)
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let exec_uuid = res["execution_uuid"].as_str().unwrap();
    let payload: serde_json::Value =
        sqlx::query_scalar("SELECT payload FROM jobs WHERE kind = 'database_backup' LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(payload["execution_uuid"], exec_uuid);

    // Execution history now has the running row.
    let (status, execs) = send(
        &app,
        Req::get(format!("/api/v1/backups/{backup_uuid}/executions"))
            .cookie(&cookie)
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(execs.as_array().unwrap().len(), 1);
    assert_eq!(execs[0]["status"], "running");

    // Delete.
    let (status, _) = send(
        &app,
        Req::delete(format!("/api/v1/backups/{backup_uuid}"))
            .cookie(&cookie)
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn save_s3_requires_storage(pool: PgPool) {
    seed_user(&pool).await;
    let app = build_router(state(pool.clone()));
    let cookie = login(&app).await;
    let db_uuid = scaffold_database(&app, &cookie).await;

    let (status, _) = send(
        &app,
        Req::post(format!("/api/v1/databases/{db_uuid}/backups"))
            .cookie(&cookie)
            .json(json!({ "frequency": "0 2 * * *", "save_s3": true }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}
