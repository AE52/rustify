//! GitHub App source routes: CRUD (secrets elided) + application create wiring.

use axum::http::StatusCode;
use serde_json::json;
use sqlx::PgPool;

use rustify_server::build_router;

mod common;
use common::{Req, login, seed_user, send, state};

async fn make_key(app: &axum::Router, cookie: &str, name: &str) -> String {
    let (status, key) = send(
        app,
        Req::post("/api/v1/private-keys/generate")
            .cookie(cookie)
            .json(json!({ "name": name }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    key["uuid"].as_str().unwrap().to_string()
}

/// Create a server + project (reusing a fresh private key); returns
/// `(server_uuid, project_uuid)`.
async fn scaffold(app: &axum::Router, cookie: &str) -> (String, String) {
    let key_uuid = make_key(app, cookie, "server-key").await;
    let (status, server) = send(
        app,
        Req::post("/api/v1/servers")
            .cookie(cookie)
            .json(json!({
                "name": "s1", "ip": "10.0.0.1", "private_key_uuid": key_uuid
            }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "server: {server}");
    let server_uuid = server["uuid"].as_str().unwrap().to_string();

    let (status, project) = send(
        app,
        Req::post("/api/v1/projects")
            .cookie(cookie)
            .json(json!({ "name": "p1" }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let project_uuid = project["uuid"].as_str().unwrap().to_string();
    (server_uuid, project_uuid)
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn crud_hides_secrets(pool: PgPool) {
    seed_user(&pool).await;
    let app = build_router(state(pool));
    let cookie = login(&app).await;
    let key_uuid = make_key(&app, &cookie, "gh-key").await;

    // create
    let (status, gh) = send(
        &app,
        Req::post("/api/v1/github-apps")
            .cookie(&cookie)
            .json(json!({
                "name": "acme-app",
                "app_id": 12345,
                "installation_id": 67890,
                "client_id": "Iv1.abc",
                "client_secret": "top-secret",
                "webhook_secret": "hook-secret",
                "private_key_uuid": key_uuid,
            }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create: {gh}");
    let uuid = gh["uuid"].as_str().unwrap().to_string();
    assert_eq!(gh["name"], "acme-app");
    assert_eq!(gh["app_id"], 12345);
    assert_eq!(gh["api_url"], "https://api.github.com");
    // secrets are never serialised
    assert!(gh.get("client_secret").is_none(), "client_secret leaked");
    assert!(gh.get("webhook_secret").is_none(), "webhook_secret leaked");

    // list
    let (status, list) = send(
        &app,
        Req::get("/api/v1/github-apps").cookie(&cookie).build(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 1);

    // get
    let (status, one) = send(
        &app,
        Req::get(format!("/api/v1/github-apps/{uuid}"))
            .cookie(&cookie)
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(one["uuid"], uuid);

    // patch
    let (status, patched) = send(
        &app,
        Req::patch(format!("/api/v1/github-apps/{uuid}"))
            .cookie(&cookie)
            .json(json!({ "name": "acme-renamed", "is_public": true }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(patched["name"], "acme-renamed");
    assert_eq!(patched["is_public"], true);

    // delete
    let (status, _) = send(
        &app,
        Req::delete(format!("/api/v1/github-apps/{uuid}"))
            .cookie(&cookie)
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn create_application_with_github_source(pool: PgPool) {
    seed_user(&pool).await;
    let app = build_router(state(pool.clone()));
    let cookie = login(&app).await;

    let gh_key = make_key(&app, &cookie, "gh-key").await;
    let (status, gh) = send(
        &app,
        Req::post("/api/v1/github-apps")
            .cookie(&cookie)
            .json(json!({
                "name": "src", "app_id": 1, "installation_id": 2,
                "private_key_uuid": gh_key,
            }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let gh_uuid = gh["uuid"].as_str().unwrap().to_string();

    let (server_uuid, project_uuid) = scaffold(&app, &cookie).await;

    // Create an application pointing at the GitHub App source. The repository is
    // stored as `owner/repo` (not a URL), so URL validation must be skipped.
    let (status, created) = send(
        &app,
        Req::post("/api/v1/applications")
            .cookie(&cookie)
            .json(json!({
                "project_uuid": project_uuid,
                "environment_name": "production",
                "server_uuid": server_uuid,
                "name": "gh-app",
                "git_repository": "acme/widgets",
                "git_branch": "main",
                "build_pack": "nixpacks",
                "source": "github_app",
                "github_app_uuid": gh_uuid,
                "is_private": true,
            }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create app: {created}");
    let app_uuid = created["uuid"].as_str().unwrap().to_string();

    // The source wiring is persisted.
    let (source_type, source_id): (Option<String>, Option<i64>) =
        sqlx::query_as("SELECT source_type, source_id FROM applications WHERE uuid = $1")
            .bind(&app_uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(source_type.as_deref(), Some("github_app"));
    assert!(source_id.is_some());
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn github_source_requires_app_uuid(pool: PgPool) {
    seed_user(&pool).await;
    let app = build_router(state(pool));
    let cookie = login(&app).await;
    let (server_uuid, project_uuid) = scaffold(&app, &cookie).await;

    let (status, _) = send(
        &app,
        Req::post("/api/v1/applications")
            .cookie(&cookie)
            .json(json!({
                "project_uuid": project_uuid,
                "environment_name": "production",
                "server_uuid": server_uuid,
                "name": "bad",
                "git_repository": "acme/widgets",
                "source": "github_app",
            }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn create_application_with_deploy_key(pool: PgPool) {
    seed_user(&pool).await;
    let app = build_router(state(pool.clone()));
    let cookie = login(&app).await;
    let (server_uuid, project_uuid) = scaffold(&app, &cookie).await;
    let deploy_key = make_key(&app, &cookie, "deploy-key").await;

    let (status, created) = send(
        &app,
        Req::post("/api/v1/applications")
            .cookie(&cookie)
            .json(json!({
                "project_uuid": project_uuid,
                "environment_name": "production",
                "server_uuid": server_uuid,
                "name": "dk-app",
                "git_repository": "git@github.com:acme/widgets.git",
                "private_key_uuid": deploy_key,
            }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create app: {created}");
    let app_uuid = created["uuid"].as_str().unwrap();

    let private_key_id: Option<i64> =
        sqlx::query_scalar("SELECT private_key_id FROM applications WHERE uuid = $1")
            .bind(app_uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(private_key_id.is_some(), "deploy key wired to application");
}
