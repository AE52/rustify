//! Database HTTP route tests (create + lifecycle enqueue) over
//! `tower::ServiceExt::oneshot` + `#[sqlx::test]`.

use axum::http::StatusCode;
use serde_json::json;
use sqlx::PgPool;

use rustify_server::build_router;

mod common;
use common::{Req, login, seed_user, send, state};

/// Create a private key, server and project; return `(server_uuid, project_uuid)`.
async fn scaffold(app: &axum::Router, cookie: &str) -> (String, String) {
    let (_, key) = send(
        app,
        Req::post("/api/v1/private-keys/generate")
            .cookie(cookie)
            .json(json!({ "name": "k" }))
            .build(),
    )
    .await;
    let key_uuid = key["uuid"].as_str().unwrap().to_string();

    let (status, server) = send(
        app,
        Req::post("/api/v1/servers")
            .cookie(cookie)
            .json(json!({ "name": "s", "ip": "10.0.0.9", "private_key_uuid": key_uuid }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let server_uuid = server["uuid"].as_str().unwrap().to_string();

    let (status, project) = send(
        app,
        Req::post("/api/v1/projects")
            .cookie(cookie)
            .json(json!({ "name": "p" }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let project_uuid = project["uuid"].as_str().unwrap().to_string();

    (server_uuid, project_uuid)
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn create_defaults_image_and_hides_credentials(pool: PgPool) {
    seed_user(&pool).await;
    let app = build_router(state(pool.clone()));
    let cookie = login(&app).await;
    let (server_uuid, project_uuid) = scaffold(&app, &cookie).await;

    let (status, created) = send(
        &app,
        Req::post("/api/v1/databases")
            .cookie(&cookie)
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
    assert_eq!(status, StatusCode::CREATED, "create: {created:?}");
    assert_eq!(created["engine"], "postgresql");
    // Image defaulted from the engine descriptor.
    assert_eq!(created["image"], "postgres:16-alpine");
    assert_eq!(created["status"], "exited");
    // Credentials never leak into the response.
    assert!(created.get("credentials").is_none());
    assert!(created.get("credentials_enc").is_none());

    let uuid = created["uuid"].as_str().unwrap();
    // Encrypted credentials exist in the DB.
    let has_creds: bool = sqlx::query_scalar(
        "SELECT octet_length(credentials_enc) > 0 FROM standalone_databases WHERE uuid = $1",
    )
    .bind(uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(has_creds);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn unknown_engine_is_rejected(pool: PgPool) {
    seed_user(&pool).await;
    let app = build_router(state(pool));
    let cookie = login(&app).await;
    let (server_uuid, project_uuid) = scaffold(&app, &cookie).await;

    let (status, _) = send(
        &app,
        Req::post("/api/v1/databases")
            .cookie(&cookie)
            .json(json!({
                "project_uuid": project_uuid,
                "environment_name": "production",
                "server_uuid": server_uuid,
                "engine": "cockroach",
                "name": "x",
            }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn start_enqueues_database_start_job(pool: PgPool) {
    seed_user(&pool).await;
    let app = build_router(state(pool.clone()));
    let cookie = login(&app).await;
    let (server_uuid, project_uuid) = scaffold(&app, &cookie).await;

    let (_, created) = send(
        &app,
        Req::post("/api/v1/databases")
            .cookie(&cookie)
            .json(json!({
                "project_uuid": project_uuid,
                "environment_name": "production",
                "server_uuid": server_uuid,
                "engine": "redis",
                "name": "cache",
            }))
            .build(),
    )
    .await;
    let uuid = created["uuid"].as_str().unwrap().to_string();

    let (status, _) = send(
        &app,
        Req::post(format!("/api/v1/databases/{uuid}/start"))
            .cookie(&cookie)
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);

    // A `database_start` job was enqueued with this uuid.
    let payload: serde_json::Value =
        sqlx::query_scalar("SELECT payload FROM jobs WHERE kind = 'database_start' LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(payload["database_uuid"], uuid);

    // Restart reuses the start job kind.
    let (status, _) = send(
        &app,
        Req::post(format!("/api/v1/databases/{uuid}/restart"))
            .cookie(&cookie)
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let start_jobs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'database_start'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(start_jobs, 2);
}
