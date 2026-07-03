//! Preview deploy path + cleanup handler against the scripted FakeExecutor.

use std::sync::Arc;

use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

use rustify_core::DeploymentStatus;
use rustify_db::repos::PreviewRepo;
use rustify_deploy::{cleanup_preview, run_deployment};

mod common;
mod fake;

use common::{LS_REMOTE_OK, new_app, queue_preview, setup};
use fake::FakeExecutor;

async fn dep_status(pool: &PgPool, id: i64) -> DeploymentStatus {
    sqlx::query_scalar("SELECT status FROM deployments WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn preview_deploy_uses_pr_tag_network_and_marks_running(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let (app_id, app_uuid) = new_app(&pool, &fx, "nixpacks").await;
    sqlx::query("UPDATE applications SET fqdn = 'https://app.example.com' WHERE id = $1")
        .bind(app_id)
        .execute(&pool)
        .await
        .unwrap();
    let dep = queue_preview(&pool, app_id, fx.server_id, 42).await;

    let fake = Arc::new(FakeExecutor::new().respond("git ls-remote", LS_REMOTE_OK));
    let d = common::deps(&pool, fake.clone());
    run_deployment(&d.deps, &CancellationToken::new(), &dep.uuid)
        .await
        .unwrap();

    assert_eq!(dep_status(&pool, dep.id).await, DeploymentStatus::Finished);

    // Preview image tag pr-42-<sha> is used somewhere in the build.
    assert!(fake.ran("pr-42-"), "preview image tag not used");
    // The PR's dedicated network is created and the proxy attached.
    let net = format!("{app_uuid}-42");
    assert!(
        fake.ran(&format!("docker network create {net}")),
        "preview network not created"
    );
    assert!(
        fake.ran(&format!("docker network connect {net} rustify-proxy")),
        "proxy not connected to preview network"
    );
    // Container filtering is scoped to this PR (never touches production, pr=0).
    assert!(
        fake.ran("rustify.pullRequestId=42"),
        "container filter not scoped to the PR"
    );

    // The preview row exists with a templated fqdn and a running status.
    let preview = PreviewRepo::new(pool.clone())
        .get(app_id, 42)
        .await
        .unwrap()
        .expect("preview row");
    assert_eq!(preview.status, "running");
    assert_eq!(preview.fqdn.as_deref(), Some("https://42.app.example.com"));
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn cleanup_cancels_deployments_removes_containers_and_network(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let (app_id, app_uuid) = new_app(&pool, &fx, "nixpacks").await;
    let repo = PreviewRepo::new(pool.clone());
    repo.upsert(app_id, 42, Some("https://gh/pr/42"), Some("github"))
        .await
        .unwrap();
    // An in-flight preview deployment that cleanup must cancel.
    let dep = queue_preview(&pool, app_id, fx.server_id, 42).await;

    // FakeExecutor reports one running PR container by name.
    let container = format!("{app_uuid}-pr-42");
    let fake = Arc::new(FakeExecutor::new().respond(
        &format!("docker ps -a --filter name={container}"),
        &container,
    ));
    let d = common::deps(&pool, fake.clone());

    cleanup_preview(&d.deps, &app_uuid, 42).await.unwrap();

    // The queued deployment was cancelled.
    assert_eq!(dep_status(&pool, dep.id).await, DeploymentStatus::Cancelled);
    // A cancellation log line was appended.
    let logged: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM deployment_logs WHERE deployment_id = $1 \
         AND content = 'Deployment cancelled: Pull request closed.'",
    )
    .bind(dep.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(logged, 1);

    // Container found by name is force-removed.
    assert!(fake.ran(&format!("docker rm -f {container}")));
    // The PR network is disconnected from the proxy then removed.
    let net = format!("{app_uuid}-42");
    assert!(fake.ran(&format!("docker network disconnect -f {net} rustify-proxy")));
    assert!(fake.ran(&format!("docker network rm {net}")));

    // The preview row is deleted.
    assert!(repo.get(app_id, 42).await.unwrap().is_none());
}
