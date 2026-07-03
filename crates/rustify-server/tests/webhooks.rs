//! Git-source webhook route tests: ping→pong, signature rejection, and an
//! App-mode `pull_request` opened event producing a preview deployment.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use hmac::{Hmac, Mac};
use serde_json::json;
use sha2::Sha256;
use sqlx::PgPool;

use rustify_db::repos::{GithubAppRepo, NewGithubApp};
use rustify_server::build_router;

mod common;

fn uid() -> String {
    rustify_core::ids::new_uuid()
}

fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    let bytes = mac.finalize().into_bytes();
    use std::fmt::Write as _;
    let mut s = String::new();
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    format!("sha256={s}")
}

async fn post_bytes(
    app: &Router,
    path: &str,
    headers: &[(&str, String)],
    body: Vec<u8>,
) -> (StatusCode, Vec<u8>) {
    use tower::ServiceExt;
    let mut b = Request::builder().method("POST").uri(path);
    for (k, v) in headers {
        b = b.header(*k, v);
    }
    let req = b.body(Body::from(body)).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, bytes)
}

/// Create team/server/destination/project/environment and return (team_id, environment_id, destination_id, server_id).
async fn infra(pool: &PgPool) -> (i64, i64, i64, i64) {
    let team_id: i64 =
        sqlx::query_scalar("INSERT INTO teams (uuid, name) VALUES ($1, 't') RETURNING id")
            .bind(uid())
            .fetch_one(pool)
            .await
            .unwrap();
    let key_id: i64 = sqlx::query_scalar(
        "INSERT INTO private_keys (uuid, team_id, name, private_key_enc, public_key)
         VALUES ($1, $2, 'k', $3, 'ssh-ed25519 AAAA') RETURNING id",
    )
    .bind(uid())
    .bind(team_id)
    .bind(vec![0u8; 4])
    .fetch_one(pool)
    .await
    .unwrap();
    let server_id: i64 = sqlx::query_scalar(
        "INSERT INTO servers (uuid, team_id, name, ip, port, ssh_user, private_key_id)
         VALUES ($1,$2,'s','10.0.0.1',22,'root',$3) RETURNING id",
    )
    .bind(uid())
    .bind(team_id)
    .bind(key_id)
    .fetch_one(pool)
    .await
    .unwrap();
    let destination_id: i64 = sqlx::query_scalar(
        "INSERT INTO destinations (uuid, server_id, network) VALUES ($1,$2,'rustify') RETURNING id",
    )
    .bind(uid())
    .bind(server_id)
    .fetch_one(pool)
    .await
    .unwrap();
    let project_id: i64 = sqlx::query_scalar(
        "INSERT INTO projects (uuid, team_id, name) VALUES ($1,$2,'p') RETURNING id",
    )
    .bind(uid())
    .bind(team_id)
    .fetch_one(pool)
    .await
    .unwrap();
    let environment_id: i64 = sqlx::query_scalar(
        "INSERT INTO environments (uuid, project_id, name) VALUES ($1,$2,'production') RETURNING id",
    )
    .bind(uid())
    .bind(project_id)
    .fetch_one(pool)
    .await
    .unwrap();
    (team_id, environment_id, destination_id, server_id)
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn ping_returns_pong(pool: PgPool) {
    let app = build_router(common::state(pool));
    let (status, body) = post_bytes(
        &app,
        "/webhooks/source/github/events",
        &[("X-GitHub-Event", "ping".into())],
        b"{}".to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(String::from_utf8_lossy(&body), "pong");
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn app_mode_rejects_invalid_signature(pool: PgPool) {
    let (team_id, _env, _dest, _srv) = infra(&pool).await;
    GithubAppRepo::new(pool.clone())
        .create(NewGithubApp {
            team_id,
            name: "app".into(),
            app_id: Some(999),
            webhook_secret: Some("s3cr3t".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let app = build_router(common::state(pool));
    let body = json!({ "ref": "refs/heads/main", "after": "abc", "repository": { "id": 1 } });
    let bytes = serde_json::to_vec(&body).unwrap();
    let (status, resp) = post_bytes(
        &app,
        "/webhooks/source/github/events",
        &[
            ("X-GitHub-Event", "push".into()),
            ("X-GitHub-Hook-Installation-Target-Id", "999".into()),
            ("X-Hub-Signature-256", "sha256=deadbeef".into()),
        ],
        bytes,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(String::from_utf8_lossy(&resp), "Invalid signature.");
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn app_mode_pr_open_queues_preview(pool: PgPool) {
    common::init_secret_key();
    let (team_id, env_id, dest_id, _srv) = infra(&pool).await;
    let secret = "hooksecret";
    let gh = GithubAppRepo::new(pool.clone())
        .create(NewGithubApp {
            team_id,
            name: "app".into(),
            app_id: Some(555),
            webhook_secret: Some(secret.into()),
            ..Default::default()
        })
        .await
        .unwrap();

    // App wired to the App source, repository id 123, base branch main.
    let app_id: i64 = sqlx::query_scalar(
        "INSERT INTO applications
           (uuid, environment_id, destination_id, name, git_repository, git_branch, build_pack,
            ports_exposes, source_type, source_id, repository_project_id)
         VALUES ($1,$2,$3,'app','owner/repo','main','nixpacks','3000','github_app',$4,123)
         RETURNING id",
    )
    .bind(uid())
    .bind(env_id)
    .bind(dest_id)
    .bind(gh.id)
    .fetch_one(&pool)
    .await
    .unwrap();

    let router = build_router(common::state(pool.clone()));
    let payload = json!({
        "action": "opened",
        "number": 42,
        "repository": { "id": 123, "full_name": "owner/repo" },
        "pull_request": {
            "html_url": "https://github.com/owner/repo/pull/42",
            "title": "Add feature",
            "author_association": "OWNER",
            "head": { "ref": "feature", "sha": "cafebabe", "repo": { "id": 123 } },
            "base": { "ref": "main", "repo": { "id": 123 } }
        }
    });
    let bytes = serde_json::to_vec(&payload).unwrap();
    let sig = sign(secret, &bytes);
    let (status, _resp) = post_bytes(
        &router,
        "/webhooks/source/github/events",
        &[
            ("X-GitHub-Event", "pull_request".into()),
            ("X-GitHub-Hook-Installation-Target-Id", "555".into()),
            ("X-Hub-Signature-256", sig),
        ],
        bytes,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // A preview deployment (pull_request_id=42) was queued.
    let pr_deploys: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM deployments WHERE application_id = $1 AND pull_request_id = 42",
    )
    .bind(app_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(pr_deploys, 1, "a preview deployment should be queued");

    // And an application_previews row was upserted.
    let previews: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM application_previews WHERE application_id = $1 AND pull_request_id = 42",
    )
    .bind(app_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(previews, 1);
}
