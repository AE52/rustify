//! Build-server deploy flow: when an application pins a usable build server the
//! image is built + pushed on that server, then the SSH target switches to the
//! deploy server for the pull + compose rollout (two distinct hosts). Also
//! covers the early failure when a build server is requested without a registry
//! image name.

use std::sync::Arc;

use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

use rustify_core::DeploymentStatus;
use rustify_deploy::run_deployment;

mod common;
mod fake;

use common::{LS_REMOTE_OK, new_app, queue, setup, uid};
use fake::FakeExecutor;

async fn status(pool: &PgPool, id: i64) -> DeploymentStatus {
    sqlx::query_scalar("SELECT status FROM deployments WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Insert a usable build server (`is_build_server = true`) in the team, at a
/// distinct IP, and return `(id, ip)`.
async fn add_build_server(pool: &PgPool, team_id: i64, ip: &str) -> (i64, String) {
    let key_id: i64 = sqlx::query_scalar(
        "INSERT INTO private_keys (uuid, team_id, name, private_key_enc, public_key)
         VALUES ($1, $2, 'bkey', $3, 'ssh-ed25519 AAAA') RETURNING id",
    )
    .bind(uid())
    .bind(team_id)
    .bind(vec![0u8; 4])
    .fetch_one(pool)
    .await
    .unwrap();

    let server_id: i64 = sqlx::query_scalar(
        "INSERT INTO servers (uuid, team_id, name, ip, port, ssh_user, private_key_id, reachable, usable)
         VALUES ($1, $2, 'build', $3, 22, 'root', $4, true, true) RETURNING id",
    )
    .bind(uid())
    .bind(team_id)
    .bind(ip)
    .bind(key_id)
    .fetch_one(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO server_settings (server_id, concurrent_builds, is_build_server)
         VALUES ($1, 2, true)",
    )
    .bind(server_id)
    .execute(pool)
    .await
    .unwrap();

    (server_id, ip.to_string())
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn build_pushes_on_build_host_then_deploys_on_deploy_host(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let deploy_ip: String = sqlx::query_scalar("SELECT ip FROM servers WHERE id = $1")
        .bind(fx.server_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let (build_id, build_ip) = add_build_server(&pool, fx.team_id, "10.9.9.9").await;
    assert_ne!(deploy_ip, build_ip, "hosts must be distinct");

    let (app_id, _uuid) = new_app(&pool, &fx, "nixpacks").await;
    // Pin the build server and give the app a registry image name to push to.
    sqlx::query(
        "UPDATE applications SET build_server_id = $2,
            docker_registry_image_name = 'ghcr.io/acme/app'
         WHERE id = $1",
    )
    .bind(app_id)
    .bind(build_id)
    .execute(&pool)
    .await
    .unwrap();

    let dep = queue(&pool, app_id, fx.server_id, false).await;
    let fake = Arc::new(FakeExecutor::new().respond("git ls-remote", LS_REMOTE_OK));
    let d = common::deps(&pool, fake.clone());
    run_deployment(&d.deps, &CancellationToken::new(), &dep.uuid)
        .await
        .unwrap();

    let image = "ghcr.io/acme/app:latest";
    // The build runs on the build host with the registry image tag.
    assert_eq!(
        fake.host_for("docker build").as_deref(),
        Some(build_ip.as_str()),
        "build runs on the build server"
    );
    assert!(
        fake.ran(&format!("-t {image}")),
        "image is tagged with the registry name so it can be pushed"
    );
    // push on the build host, pull on the deploy host.
    assert_eq!(
        fake.host_for(&format!("docker push {image}")).as_deref(),
        Some(build_ip.as_str()),
        "push runs on the build server"
    );
    assert_eq!(
        fake.host_for(&format!("docker pull {image}")).as_deref(),
        Some(deploy_ip.as_str()),
        "pull runs on the deploy server"
    );
    // Ordering: push precedes pull, and compose up lands on the deploy host.
    let push_i = fake
        .index_of(&format!("docker push {image}"))
        .expect("push");
    let pull_i = fake
        .index_of(&format!("docker pull {image}"))
        .expect("pull");
    let up_i = fake.index_of("docker compose").expect("compose up");
    assert!(push_i < pull_i && pull_i < up_i, "push < pull < up");
    assert_eq!(
        fake.host_for("docker compose").as_deref(),
        Some(deploy_ip.as_str()),
        "compose rollout runs on the deploy server"
    );

    // The build helper is torn down on the build host where it was started.
    assert_eq!(
        fake.host_for(&format!("docker rm -f {}", dep.uuid))
            .as_deref(),
        Some(build_ip.as_str()),
        "helper cleanup targets the build server"
    );
    assert_eq!(status(&pool, dep.id).await, DeploymentStatus::Finished);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn build_server_without_registry_image_name_fails_early(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let (build_id, _ip) = add_build_server(&pool, fx.team_id, "10.9.9.8").await;
    let (app_id, _uuid) = new_app(&pool, &fx, "nixpacks").await;
    // Pin a build server but leave docker_registry_image_name NULL.
    sqlx::query("UPDATE applications SET build_server_id = $2 WHERE id = $1")
        .bind(app_id)
        .bind(build_id)
        .execute(&pool)
        .await
        .unwrap();

    let dep = queue(&pool, app_id, fx.server_id, false).await;
    let fake = Arc::new(FakeExecutor::new().respond("git ls-remote", LS_REMOTE_OK));
    let d = common::deps(&pool, fake.clone());
    run_deployment(&d.deps, &CancellationToken::new(), &dep.uuid)
        .await
        .unwrap();

    // We fail before starting the helper or building anything.
    assert!(
        !fake.ran("docker build"),
        "must not build without a push target"
    );
    assert!(!fake.ran("docker push"), "nothing to push");
    assert_eq!(status(&pool, dep.id).await, DeploymentStatus::Failed);
}
