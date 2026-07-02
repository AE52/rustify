//! Engine behaviour against the scripted [`FakeExecutor`]: script ordering,
//! skip-build, static nginx generation, the unhealthy rollback path,
//! cancellation, and secret redaction.

use std::sync::Arc;

use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

use rustify_core::DeploymentStatus;
use rustify_db::repos::deployments::DeploymentRepo;
use rustify_db::repos::env_vars::{EnvVarRepo, NewEnvVar};
use rustify_deploy::run_deployment;

mod common;
mod fake;

use common::{LS_REMOTE_OK, new_app, queue, setup};
use fake::FakeExecutor;

async fn status(pool: &PgPool, id: i64) -> DeploymentStatus {
    sqlx::query_scalar("SELECT status FROM deployments WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn happy_nixpacks_produces_expected_script_order(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let (app_id, _uuid) = new_app(&pool, &fx, "nixpacks").await;
    let dep = queue(&pool, app_id, fx.server_id, false).await;

    let fake = Arc::new(FakeExecutor::new().respond("git ls-remote", LS_REMOTE_OK));
    let d = common::deps(&pool, fake.clone());
    let token = CancellationToken::new();
    run_deployment(&d.deps, &token, &dep.uuid).await.unwrap();

    let helper = fake
        .index_of("docker run -d --rm --name")
        .expect("helper up");
    let ls = fake.index_of("git ls-remote").expect("ls-remote");
    let clone = fake.index_of("git clone").expect("clone");
    let build = fake.index_of("docker build").expect("image build");
    let up = fake.index_of("docker compose").expect("compose up");
    assert!(
        helper < ls && ls < clone && clone < build && build < up,
        "expected helper<ls<clone<build<up, got {helper}/{ls}/{clone}/{build}/{up}"
    );
    assert_eq!(status(&pool, dep.id).await, DeploymentStatus::Finished);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn skip_build_issues_no_clone_or_build(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let (app_id, _uuid) = new_app(&pool, &fx, "nixpacks").await;
    let dep = queue(&pool, app_id, fx.server_id, false).await;

    // Pretend the image already exists for this commit.
    let fake = Arc::new(
        FakeExecutor::new()
            .respond("git ls-remote", LS_REMOTE_OK)
            .respond("docker images -q", "sha256:deadbeef"),
    );
    let d = common::deps(&pool, fake.clone());
    run_deployment(&d.deps, &CancellationToken::new(), &dep.uuid)
        .await
        .unwrap();

    assert!(!fake.ran("git clone"), "skip-build must not clone");
    assert!(!fake.ran("docker build"), "skip-build must not build");
    assert!(
        fake.ran("docker compose"),
        "still deploys the existing image"
    );
    assert_eq!(status(&pool, dep.id).await, DeploymentStatus::Finished);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn static_pack_generates_nginx_dockerfile(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let (app_id, _uuid) = new_app(&pool, &fx, "static").await;
    sqlx::query("UPDATE applications SET publish_directory = '/dist' WHERE id = $1")
        .bind(app_id)
        .execute(&pool)
        .await
        .unwrap();
    let dep = queue(&pool, app_id, fx.server_id, false).await;

    let fake = Arc::new(FakeExecutor::new().respond("git ls-remote", LS_REMOTE_OK));
    let d = common::deps(&pool, fake.clone());
    run_deployment(&d.deps, &CancellationToken::new(), &dep.uuid)
        .await
        .unwrap();

    let has_dockerfile = fake.scripts().iter().any(|s| {
        s.contains("FROM nginx:alpine") && s.contains("COPY ./dist /usr/share/nginx/html")
    });
    assert!(
        has_dockerfile,
        "generated nginx Dockerfile with publish dir"
    );
    assert_eq!(status(&pool, dep.id).await, DeploymentStatus::Finished);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn unhealthy_removes_new_keeps_old_and_fails(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let (app_id, app_uuid) = new_app(&pool, &fx, "nixpacks").await;
    sqlx::query(
        "UPDATE applications SET health_check_enabled = true,
             health_check_start_period = 0, health_check_interval = 0, health_check_retries = 1
         WHERE id = $1",
    )
    .bind(app_id)
    .execute(&pool)
    .await
    .unwrap();
    let dep = queue(&pool, app_id, fx.server_id, false).await;

    let fake = Arc::new(
        FakeExecutor::new()
            .respond("git ls-remote", LS_REMOTE_OK)
            .respond("State.Health.Status", "\"unhealthy\""),
    );
    let d = common::deps(&pool, fake.clone());
    run_deployment(&d.deps, &CancellationToken::new(), &dep.uuid)
        .await
        .unwrap();

    // New container removed; old containers never stopped.
    assert!(
        fake.ran(&format!("docker rm -f {app_uuid}-")),
        "the new container must be removed"
    );
    assert!(
        !fake.ran("docker stop"),
        "old container must not be stopped"
    );
    assert!(fake.ran("docker logs -n 100"), "logs dumped on failure");
    assert_eq!(status(&pool, dep.id).await, DeploymentStatus::Failed);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn cancellation_between_clone_and_build(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let (app_id, _uuid) = new_app(&pool, &fx, "nixpacks").await;
    let dep = queue(&pool, app_id, fx.server_id, false).await;

    let token = CancellationToken::new();
    let fake = Arc::new(
        FakeExecutor::new()
            .respond("git ls-remote", LS_REMOTE_OK)
            .cancel_after("git clone", token.clone()),
    );
    let d = common::deps(&pool, fake.clone());
    run_deployment(&d.deps, &token, &dep.uuid).await.unwrap();

    assert!(fake.ran("git clone"), "clone ran before cancellation");
    assert!(!fake.ran("docker build"), "build must not run after cancel");
    assert!(
        !fake.ran("docker compose"),
        "no rolling update after cancel"
    );
    assert!(
        fake.ran(&format!("docker rm -f {}", dep.uuid)),
        "helper cleanup must still run on cancel"
    );
    assert_eq!(status(&pool, dep.id).await, DeploymentStatus::Cancelled);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn shown_once_secret_is_redacted_from_logs(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let (app_id, _uuid) = new_app(&pool, &fx, "nixpacks").await;
    EnvVarRepo::new(pool.clone())
        .upsert(NewEnvVar {
            resource_kind: "application".into(),
            resource_id: app_id,
            key: "TOKEN".into(),
            value: "SUPERSECRETVALUE".into(),
            is_buildtime: false,
            is_literal: false,
            is_shown_once: true,
        })
        .await
        .unwrap();
    let dep = queue(&pool, app_id, fx.server_id, false).await;

    // Force the secret into a command's streamed output.
    let fake = Arc::new(
        FakeExecutor::new()
            .respond("git ls-remote", LS_REMOTE_OK)
            .respond("nixpacks build", "auth token=SUPERSECRETVALUE accepted"),
    );
    let d = common::deps(&pool, fake.clone());
    run_deployment(&d.deps, &CancellationToken::new(), &dep.uuid)
        .await
        .unwrap();

    let logs = DeploymentRepo::new(pool.clone())
        .logs(dep.id)
        .await
        .unwrap();
    assert!(
        logs.iter().all(|l| !l.content.contains("SUPERSECRETVALUE")),
        "secret must never appear in persisted logs"
    );
    assert!(
        logs.iter().any(|l| l.content.contains("[REDACTED]")),
        "the secret occurrence should be redacted"
    );
}
