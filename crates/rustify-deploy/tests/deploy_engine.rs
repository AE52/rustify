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
async fn railpack_dispatch_runs_prepare_then_buildx(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let (app_id, _uuid) = new_app(&pool, &fx, "railpack").await;
    // A build-time secret so the --env → --secret channel is exercised.
    EnvVarRepo::new(pool.clone())
        .upsert(NewEnvVar {
            resource_kind: "application".into(),
            resource_id: app_id,
            key: "API_KEY".into(),
            value: "sekret".into(),
            is_buildtime: true,
            is_literal: false,
            is_shown_once: false,
        })
        .await
        .unwrap();
    let dep = queue(&pool, app_id, fx.server_id, false).await;

    let fake = Arc::new(
        FakeExecutor::new()
            .respond("git ls-remote", LS_REMOTE_OK)
            // buildx present.
            .respond("docker buildx version", "github.com/docker/buildx v0.17.0")
            .respond(
                "cat /artifacts/railpack-plan.json",
                r#"{"steps":[{"name":"build"}],"secrets":["API_KEY"]}"#,
            ),
    );
    let d = common::deps(&pool, fake.clone());
    run_deployment(&d.deps, &CancellationToken::new(), &dep.uuid)
        .await
        .unwrap();

    // Dispatch selected railpack and ran the plan→buildx flow in order.
    let helper = fake.index_of("docker run -d --rm --name").expect("helper");
    let probe = fake
        .index_of("docker buildx version")
        .expect("buildx probe");
    let create = fake
        .index_of("docker buildx create --name coolify-railpack")
        .expect("builder create");
    let prepare = fake.index_of("railpack prepare").expect("prepare");
    let build = fake.index_of("docker buildx build").expect("buildx build");
    let prune = fake
        .index_of("docker buildx prune --builder coolify-railpack -af")
        .expect("prune");
    let up = fake.index_of("docker compose").expect("compose up");
    assert!(
        helper < probe && probe < create && create < prepare && prepare < build && build < up,
        "expected helper<probe<create<prepare<build<up, got \
         {helper}/{probe}/{create}/{prepare}/{build}/{up}"
    );
    assert!(prune > build, "prune runs after the build");

    // The buildx command wires the pinned frontend, the secret channel and the
    // secrets-hash cache buster (not force_rebuild).
    let buildx = &fake.scripts()[build];
    assert!(
        buildx.contains(
            "--build-arg BUILDKIT_SYNTAX=\"ghcr.io/railwayapp/railpack-frontend:v0.23.0\""
        )
    );
    assert!(buildx.contains("--secret 'id=API_KEY,env=API_KEY'"));
    assert!(buildx.contains("--build-arg cache-key="));
    assert!(buildx.contains("--build-arg secrets-hash="));
    assert!(!buildx.contains("--no-cache"), "not a force rebuild");

    // prepare passes the secret through --env, never a leaked flag.
    let prep = &fake.scripts()[prepare];
    assert!(prep.contains("--env 'API_KEY=sekret'"));
    assert!(prep.contains("--plan-out /artifacts/railpack-plan.json"));

    assert_eq!(status(&pool, dep.id).await, DeploymentStatus::Finished);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn railpack_force_rebuild_uses_no_cache(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let (app_id, _uuid) = new_app(&pool, &fx, "railpack").await;
    let dep = queue(&pool, app_id, fx.server_id, true).await;

    let fake = Arc::new(
        FakeExecutor::new()
            .respond("git ls-remote", LS_REMOTE_OK)
            .respond("docker buildx version", "v0.17.0"),
    );
    let d = common::deps(&pool, fake.clone());
    run_deployment(&d.deps, &CancellationToken::new(), &dep.uuid)
        .await
        .unwrap();

    let build = fake.index_of("docker buildx build").expect("buildx build");
    let buildx = &fake.scripts()[build];
    assert!(buildx.contains("--no-cache"), "force rebuild → --no-cache");
    assert!(
        !buildx.contains("cache-key="),
        "no cache-key on force rebuild"
    );
    assert!(
        !buildx.contains("secrets-hash="),
        "no secrets-hash on force rebuild"
    );
    assert_eq!(status(&pool, dep.id).await, DeploymentStatus::Finished);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn railpack_missing_buildx_fails_early(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let (app_id, _uuid) = new_app(&pool, &fx, "railpack").await;
    let dep = queue(&pool, app_id, fx.server_id, false).await;

    let fake = Arc::new(
        FakeExecutor::new()
            .respond("git ls-remote", LS_REMOTE_OK)
            // buildx probe fails.
            .respond_full("docker buildx version", "", "unknown command", 1),
    );
    let d = common::deps(&pool, fake.clone());
    run_deployment(&d.deps, &CancellationToken::new(), &dep.uuid)
        .await
        .unwrap();

    assert!(fake.ran("docker buildx version"), "probe ran");
    assert!(!fake.ran("railpack prepare"), "prepare must not run");
    assert!(!fake.ran("docker buildx build"), "build must not run");
    assert_eq!(status(&pool, dep.id).await, DeploymentStatus::Failed);
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
async fn rolling_compose_up_has_no_remove_orphans(pool: PgPool) {
    // On the rolling (Eligible) path the new container must come up ALONGSIDE
    // the old one: `docker compose up` must NOT carry `--remove-orphans`, or the
    // still-running previous container is deleted as an orphan during `up`.
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let (app_id, _uuid) = new_app(&pool, &fx, "nixpacks").await;
    let dep = queue(&pool, app_id, fx.server_id, false).await;

    let fake = Arc::new(FakeExecutor::new().respond("git ls-remote", LS_REMOTE_OK));
    let d = common::deps(&pool, fake.clone());
    run_deployment(&d.deps, &CancellationToken::new(), &dep.uuid)
        .await
        .unwrap();

    let up = fake.index_of("docker compose").expect("compose up");
    let up_script = &fake.scripts()[up];
    assert!(
        up_script.contains("docker compose -f docker-compose.yml up -d"),
        "rolling path brings the stack up"
    );
    assert!(
        !up_script.contains("--remove-orphans"),
        "rolling compose-up must NOT pass --remove-orphans (would delete the old container): {up_script}"
    );
    assert_eq!(status(&pool, dep.id).await, DeploymentStatus::Finished);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn healthy_stops_old_only_after_health_gate(pool: PgPool) {
    // With an old container present and healthchecking on: the old container is
    // stopped ONLY after the new one reports healthy (compose up < health < stop).
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

    // The ps sweep that stop_other_containers runs reports one managed OLD
    // container for this app; the new container is healthy.
    let old_ps = format!(
        r#"{{"ID":"old1","Names":"{app_uuid}-old","Image":"img","State":"running","Labels":"rustify.managed=true,rustify.applicationUuid={app_uuid},rustify.pullRequestId=0"}}"#
    );
    let fake = Arc::new(
        FakeExecutor::new()
            .respond("git ls-remote", LS_REMOTE_OK)
            .respond(
                "docker ps -a --filter label=rustify.applicationUuid",
                &old_ps,
            )
            .respond("State.Health.Status", "\"healthy\""),
    );
    let d = common::deps(&pool, fake.clone());
    run_deployment(&d.deps, &CancellationToken::new(), &dep.uuid)
        .await
        .unwrap();

    let up = fake.index_of("docker compose").expect("compose up");
    let health = fake.index_of("State.Health.Status").expect("health check");
    let stop = fake
        .index_of(&format!("docker stop {app_uuid}-old"))
        .expect("old container stopped");
    assert!(
        up < health && health < stop,
        "old container must be stopped only after the health gate passes (up={up} health={health} stop={stop})"
    );
    assert_eq!(status(&pool, dep.id).await, DeploymentStatus::Finished);
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
