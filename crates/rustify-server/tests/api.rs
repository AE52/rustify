//! HTTP handler tests (contract C5) over `tower::ServiceExt::oneshot` +
//! `#[sqlx::test]`.

use axum::http::StatusCode;
use serde_json::{Value, json};
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

async fn create_app(app: &axum::Router, cookie: &str) -> String {
    let (server_uuid, project_uuid) = scaffold(app, cookie).await;
    let (status, created) = send(
        app,
        Req::post("/api/v1/applications")
            .cookie(cookie)
            .json(json!({
                "project_uuid": project_uuid,
                "environment_name": "production",
                "server_uuid": server_uuid,
                "name": "web",
                "git_repository": "https://github.com/x/y.git",
            }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "app create: {created:?}");
    created["uuid"].as_str().unwrap().to_string()
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn login_logout_me_cycle(pool: PgPool) {
    seed_user(&pool).await;
    let app = build_router(state(pool));

    let cookie = login(&app).await;

    // /auth/me returns the user, including team_uuid.
    let (status, me) = send(&app, Req::get("/api/v1/auth/me").cookie(&cookie).build()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["email"], common::ADMIN_EMAIL);
    assert!(me["team_uuid"].as_str().is_some(), "me includes team_uuid");
    assert!(me["id"].as_str().is_some());

    // Logout returns 204.
    let (status, _) = send(
        &app,
        Req::post("/api/v1/auth/logout").cookie(&cookie).build(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // The session is now invalid.
    let (status, _) = send(&app, Req::get("/api/v1/auth/me").cookie(&cookie).build()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn requires_auth(pool: PgPool) {
    seed_user(&pool).await;
    let app = build_router(state(pool));

    // No session cookie / bearer → 401 with the {code,message} envelope.
    let (status, body) = send(&app, Req::get("/api/v1/servers").build()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "unauthorized");
    assert!(body["message"].as_str().is_some());

    let (status, _) = send(&app, Req::get("/api/v1/auth/me").build()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn bad_credentials_are_401(pool: PgPool) {
    seed_user(&pool).await;
    let app = build_router(state(pool));
    let (status, _) = send(
        &app,
        Req::post("/api/v1/auth/login")
            .json(json!({ "email": common::ADMIN_EMAIL, "password": "wrong" }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn health_needs_no_auth(pool: PgPool) {
    let app = build_router(state(pool));
    let (status, body) = send(&app, Req::get("/api/v1/health").build()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn token_auth_hits_servers(pool: PgPool) {
    seed_user(&pool).await;
    let app = build_router(state(pool));
    let cookie = login(&app).await;

    // Mint an API token and use it as a bearer to list servers.
    let (status, created) = send(
        &app,
        Req::post("/api/v1/api-tokens")
            .cookie(&cookie)
            .json(json!({ "name": "ci" }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let token = created["token"].as_str().unwrap().to_string();

    let (status, body) = send(&app, Req::get("/api/v1/servers").bearer(&token).build()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.is_array());

    // A bogus bearer is rejected.
    let (status, _) = send(&app, Req::get("/api/v1/servers").bearer("nope").build()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn app_create_validates_git_and_build_pack(pool: PgPool) {
    seed_user(&pool).await;
    let app = build_router(state(pool));
    let cookie = login(&app).await;
    let (server_uuid, project_uuid) = scaffold(&app, &cookie).await;

    let base = |extra: Value| {
        let mut body = json!({
            "project_uuid": project_uuid,
            "environment_name": "production",
            "server_uuid": server_uuid,
            "name": "web",
            "git_repository": "https://github.com/x/y.git",
        });
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        body
    };

    // Invalid git URL → 422.
    let (status, body) = send(
        &app,
        Req::post("/api/v1/applications")
            .cookie(&cookie)
            .json(base(json!({ "git_repository": "ftp://bad/repo" })))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "validation_error");

    // Invalid build_pack → 422.
    let (status, _) = send(
        &app,
        Req::post("/api/v1/applications")
            .cookie(&cookie)
            .json(base(json!({ "build_pack": "cargo" })))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // git@ prefix is accepted.
    let (status, _) = send(
        &app,
        Req::post("/api/v1/applications")
            .cookie(&cookie)
            .json(base(json!({ "git_repository": "git@github.com:x/y.git" })))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn deploy_enqueues_job_and_creates_deployment(pool: PgPool) {
    seed_user(&pool).await;
    let app = build_router(state(pool.clone()));
    let cookie = login(&app).await;
    let app_uuid = create_app(&app, &cookie).await;

    let (status, body) = send(
        &app,
        Req::post(format!("/api/v1/applications/{app_uuid}/deploy"))
            .cookie(&cookie)
            .json(json!({ "force_rebuild": true }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let deployment_uuid = body["deployment_uuid"].as_str().unwrap();

    // A deployments row was created (queued).
    let deploy_status: String =
        sqlx::query_scalar("SELECT status::text FROM deployments WHERE uuid = $1")
            .bind(deployment_uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(deploy_status, "queued");

    // A `deploy` job was enqueued with the deployment uuid.
    let payload: Value = sqlx::query_scalar(
        "SELECT payload FROM jobs WHERE kind = 'deploy' ORDER BY id DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(payload["deployment_uuid"], deployment_uuid);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn cancel_flips_running_deployment(pool: PgPool) {
    seed_user(&pool).await;
    let app = build_router(state(pool.clone()));
    let cookie = login(&app).await;
    let app_uuid = create_app(&app, &cookie).await;

    let (_, body) = send(
        &app,
        Req::post(format!("/api/v1/applications/{app_uuid}/deploy"))
            .cookie(&cookie)
            .build(),
    )
    .await;
    let deployment_uuid = body["deployment_uuid"].as_str().unwrap().to_string();

    // Move it to in_progress ("running").
    sqlx::query(
        "UPDATE deployments SET status = 'in_progress', started_at = now() WHERE uuid = $1",
    )
    .bind(&deployment_uuid)
    .execute(&pool)
    .await
    .unwrap();

    let (status, _) = send(
        &app,
        Req::post(format!("/api/v1/deployments/{deployment_uuid}/cancel"))
            .cookie(&cookie)
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);

    let now: String = sqlx::query_scalar("SELECT status::text FROM deployments WHERE uuid = $1")
        .bind(&deployment_uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(now, "cancelled");

    // A terminal deployment cannot be cancelled again → 409.
    let (status, body) = send(
        &app,
        Req::post(format!("/api/v1/deployments/{deployment_uuid}/cancel"))
            .cookie(&cookie)
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "conflict");
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn settings_patch_roundtrip(pool: PgPool) {
    seed_user(&pool).await;
    let app = build_router(state(pool));
    let cookie = login(&app).await;

    let (status, body) = send(
        &app,
        Req::patch("/api/v1/settings")
            .cookie(&cookie)
            .json(json!({ "fqdn": "rustify.example.com", "registration_enabled": true }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["fqdn"], "rustify.example.com");
    assert_eq!(body["registration_enabled"], true);

    // GET reflects the update; an omitted field keeps its value.
    let (status, body) = send(&app, Req::get("/api/v1/settings").cookie(&cookie).build()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["fqdn"], "rustify.example.com");
    assert_eq!(body["registration_enabled"], true);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn openapi_contains_all_c5_paths(pool: PgPool) {
    let app = build_router(state(pool));
    let (status, doc) = send(&app, Req::get("/api/v1/openapi.json").build()).await;
    assert_eq!(status, StatusCode::OK);

    let paths = doc["paths"].as_object().expect("openapi has paths");
    assert_eq!(paths.len(), 85, "expected 85 documented C5 paths");

    for expected in [
        "/auth/login",
        "/auth/logout",
        "/auth/me",
        "/health",
        "/private-keys",
        "/private-keys/generate",
        "/private-keys/{uuid}",
        "/servers",
        "/servers/{uuid}",
        "/servers/{uuid}/validate",
        "/servers/{uuid}/proxy",
        "/servers/{uuid}/proxy/start",
        "/servers/{uuid}/proxy/stop",
        "/servers/{uuid}/proxy/restart",
        "/servers/{uuid}/metrics/{metric}",
        "/servers/{uuid}/metrics/status",
        "/containers/{uuid}/metrics/{metric}",
        "/servers/{uuid}/cloudflared",
        "/servers/provision/hetzner",
        "/cloud-tokens",
        "/cloud-tokens/{uuid}",
        "/hetzner/locations",
        "/hetzner/server-types",
        "/hetzner/images",
        "/projects",
        "/projects/{uuid}",
        "/projects/{uuid}/environments",
        "/applications",
        "/applications/{uuid}",
        "/applications/{uuid}/deploy",
        "/applications/{uuid}/stop",
        "/applications/{uuid}/restart",
        "/applications/{uuid}/logs",
        "/applications/{uuid}/envs",
        "/applications/{uuid}/envs/{env_uuid}",
        "/applications/{uuid}/previews",
        "/applications/{uuid}/previews/{pr}/redeploy",
        "/applications/{uuid}/previews/{pr}",
        "/deployments",
        "/deployments/{uuid}",
        "/deployments/{uuid}/cancel",
        "/github-apps",
        "/github-apps/{uuid}",
        "/github-apps/{uuid}/repositories",
        "/github-apps/{uuid}/repositories/{owner}/{repo}/branches",
        "/github-apps/{uuid}/manifest-state",
        "/databases",
        "/databases/{uuid}",
        "/databases/{uuid}/start",
        "/databases/{uuid}/stop",
        "/databases/{uuid}/restart",
        "/databases/{uuid}/backups",
        "/backups/{uuid}",
        "/backups/{uuid}/trigger",
        "/backups/{uuid}/executions",
        "/s3-storages",
        "/s3-storages/{uuid}",
        "/s3-storages/{uuid}/test",
        "/service-templates",
        "/service-templates/{key}",
        "/services",
        "/services/{uuid}",
        "/services/{uuid}/deploy",
        "/services/{uuid}/stop",
        "/services/{uuid}/restart",
        "/applications/{uuid}/scheduled-tasks",
        "/services/{uuid}/scheduled-tasks",
        "/scheduled-tasks/{uuid}",
        "/scheduled-tasks/{uuid}/trigger",
        "/scheduled-tasks/{uuid}/executions",
        "/settings",
        "/notifications/settings",
        "/notifications/test",
        "/api-tokens",
        "/api-tokens/{uuid}",
        "/teams",
        "/teams/current",
        "/teams/current/members",
        "/teams/{id}",
        "/teams/{id}/members",
        "/teams/{id}/members/{user_uuid}",
        "/teams/{id}/invitations",
        "/teams/{id}/switch",
        "/invitations/{uuid}",
    ] {
        assert!(paths.contains_key(expected), "missing path {expected}");
    }
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn env_var_shown_once_is_masked_in_list(pool: PgPool) {
    seed_user(&pool).await;
    let app = build_router(state(pool));
    let cookie = login(&app).await;
    let app_uuid = create_app(&app, &cookie).await;

    // Create returns the value once.
    let (status, created) = send(
        &app,
        Req::post(format!("/api/v1/applications/{app_uuid}/envs"))
            .cookie(&cookie)
            .json(json!({ "key": "SECRET", "value": "s3cr3t", "is_shown_once": true }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["value"], "s3cr3t");

    // Listing masks the shown-once value.
    let (status, list) = send(
        &app,
        Req::get(format!("/api/v1/applications/{app_uuid}/envs"))
            .cookie(&cookie)
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let first = &list.as_array().unwrap()[0];
    assert_eq!(first["key"], "SECRET");
    assert!(first["value"].is_null(), "shown-once value must be masked");
}
