//! `ServiceDeployHandler` / `deploy_service` behaviour against the scripted
//! [`FakeExecutor`]: template mutation, env persistence, remote uploads, the
//! compose `up` command, and the status event.

use std::sync::Arc;

use sqlx::PgPool;

use rustify_db::repos::env_vars::EnvVarRepo;
use rustify_db::repos::services::{NewService, ServiceRepo};
use rustify_deploy::{deploy_service, stop_service};

mod common;
mod fake;

use fake::FakeExecutor;

const TEMPLATE: &str = "\
services:
  umami:
    image: ghcr.io/umami-software/umami:latest
    environment:
      - SERVICE_URL_UMAMI_3000
      - APP_SECRET=$SERVICE_PASSWORD_64_UMAMI
    volumes:
      - umami-data:/app/data
  postgresql:
    image: postgres:16-alpine
    environment:
      - POSTGRES_PASSWORD=$SERVICE_PASSWORD_POSTGRES
volumes:
  umami-data:
";

async fn new_service(pool: &PgPool, env_id: i64, dest_id: i64) -> String {
    ServiceRepo::new(pool.clone())
        .create(NewService {
            environment_id: env_id,
            destination_id: dest_id,
            name: "umami".into(),
            template_key: "umami".into(),
            compose_raw: TEMPLATE.into(),
        })
        .await
        .unwrap()
        .uuid
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn deploy_mutates_persists_and_brings_up_stack(pool: PgPool) {
    common::init_secret_key();
    let fx = common::setup(&pool, 2).await;
    let uuid = new_service(&pool, fx.environment_id, fx.destination_id).await;

    let fake = Arc::new(FakeExecutor::new());
    let mut d = common::deps(&pool, fake.clone());

    deploy_service(&d.deps, &uuid).await.unwrap();

    // The compose stack is brought up with the project name = service uuid.
    let up = format!("docker compose --project-name {uuid} up -d --remove-orphans");
    assert!(fake.ran(&up), "ran compose up: {up}");
    assert!(
        fake.ran(&format!("mkdir -p /data/rustify/services/{uuid}")),
        "created the service dir"
    );

    // Both the compose file and the .env were uploaded to the service dir.
    let uploads = fake.uploads();
    assert!(
        uploads
            .iter()
            .any(|(_, r)| r == &format!("/data/rustify/services/{uuid}/docker-compose.yml")),
        "uploaded compose: {uploads:?}"
    );
    assert!(
        uploads
            .iter()
            .any(|(_, r)| r == &format!("/data/rustify/services/{uuid}/.env")),
        "uploaded .env"
    );

    // Status is running and a status event was emitted.
    let repo = ServiceRepo::new(pool.clone());
    let svc = repo.get_by_uuid(&uuid).await.unwrap().unwrap();
    assert_eq!(svc.status, "running");
    assert!(svc.compose_mutated.is_some());
    assert!(svc.config_hash.is_some());

    let mut saw_running = false;
    while let Ok(ev) = d.events_rx.try_recv() {
        if ev.event == "service_status_changed" && ev.data["status"] == "running" {
            saw_running = true;
        }
    }
    assert!(saw_running, "emitted service_status_changed=running");

    // Generated secrets are persisted (encrypted at rest) and reusable.
    let env = EnvVarRepo::new(pool.clone())
        .list("service", svc.id)
        .await
        .unwrap();
    let keys: Vec<&str> = env.iter().map(|e| e.key.as_str()).collect();
    assert!(keys.contains(&"SERVICE_PASSWORD_64_UMAMI"), "{keys:?}");
    assert!(keys.contains(&"SERVICE_PASSWORD_POSTGRES"));
    assert!(keys.contains(&"SERVICE_FQDN_UMAMI"));
    assert!(keys.contains(&"COOLIFY_RESOURCE_UUID"));

    let pw = env
        .iter()
        .find(|e| e.key == "SERVICE_PASSWORD_64_UMAMI")
        .unwrap();
    assert_eq!(pw.value.len(), 64);
    assert!(pw.is_shown_once, "password is a shown-once secret");

    // The child containers were recorded, postgres flagged as a database.
    let apps = repo.applications(svc.id).await.unwrap();
    assert_eq!(apps.len(), 2);
    assert!(
        apps.iter()
            .find(|a| a.name == "postgresql")
            .unwrap()
            .is_database
    );
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn redeploy_persists_secrets_once(pool: PgPool) {
    common::init_secret_key();
    let fx = common::setup(&pool, 2).await;
    let uuid = new_service(&pool, fx.environment_id, fx.destination_id).await;

    let fake = Arc::new(FakeExecutor::new());
    let d = common::deps(&pool, fake.clone());

    deploy_service(&d.deps, &uuid).await.unwrap();
    let svc = ServiceRepo::new(pool.clone())
        .get_by_uuid(&uuid)
        .await
        .unwrap()
        .unwrap();
    let first = EnvVarRepo::new(pool.clone())
        .list("service", svc.id)
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.key == "SERVICE_PASSWORD_64_UMAMI")
        .unwrap()
        .value;

    // Redeploy must NOT regenerate the password (persist-once).
    deploy_service(&d.deps, &uuid).await.unwrap();
    let second = EnvVarRepo::new(pool.clone())
        .list("service", svc.id)
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.key == "SERVICE_PASSWORD_64_UMAMI")
        .unwrap()
        .value;
    assert_eq!(first, second, "secret is stable across redeploys");
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn stop_brings_stack_down(pool: PgPool) {
    common::init_secret_key();
    let fx = common::setup(&pool, 2).await;
    let uuid = new_service(&pool, fx.environment_id, fx.destination_id).await;

    let fake = Arc::new(FakeExecutor::new());
    let d = common::deps(&pool, fake.clone());
    stop_service(&d.deps, &uuid).await.unwrap();

    assert!(
        fake.ran(&format!("docker compose --project-name {uuid} down")),
        "ran compose down"
    );
    let svc = ServiceRepo::new(pool.clone())
        .get_by_uuid(&uuid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(svc.status, "exited");
}
