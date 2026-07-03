//! ServiceRepo: create/read/update/delete + child service_applications and
//! service-scoped env vars (resource_kind = 'service').

use sqlx::PgPool;

use rustify_db::repos::env_vars::{EnvVarRepo, NewEnvVar};
use rustify_db::repos::services::{NewService, ServiceRepo};

mod common;
use common::setup;

async fn new_service(pool: &PgPool, env_id: i64, dest_id: i64) -> String {
    let repo = ServiceRepo::new(pool.clone());
    let svc = repo
        .create(NewService {
            environment_id: env_id,
            destination_id: dest_id,
            name: "my-umami".into(),
            template_key: "umami".into(),
            compose_raw: "services:\n  umami:\n    image: umami\n".into(),
        })
        .await
        .unwrap();
    svc.uuid
}

#[sqlx::test]
async fn create_get_and_roundtrip(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let uuid = new_service(&pool, fx.environment_id, fx.destination_id).await;

    let repo = ServiceRepo::new(pool.clone());
    let got = repo.get_by_uuid(&uuid).await.unwrap().unwrap();
    assert_eq!(got.name, "my-umami");
    assert_eq!(got.template_key, "umami");
    assert_eq!(got.status, "exited", "default status");
    assert!(got.compose_mutated.is_none());
    assert!(got.compose_raw.contains("umami"));

    // by id + list by environment
    assert_eq!(repo.get_by_id(got.id).await.unwrap().unwrap().uuid, uuid);
    let listed = repo.list_by_environment(fx.environment_id).await.unwrap();
    assert_eq!(listed.len(), 1);
}

#[sqlx::test]
async fn mutate_status_and_rename(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let uuid = new_service(&pool, fx.environment_id, fx.destination_id).await;
    let repo = ServiceRepo::new(pool.clone());
    let svc = repo.get_by_uuid(&uuid).await.unwrap().unwrap();

    repo.set_mutated(svc.id, "services: {}\n", "deadbeef")
        .await
        .unwrap();
    repo.set_status(svc.id, "running").await.unwrap();
    let after = repo.get_by_uuid(&uuid).await.unwrap().unwrap();
    assert_eq!(after.status, "running");
    assert_eq!(after.compose_mutated.as_deref(), Some("services: {}\n"));
    assert_eq!(after.config_hash.as_deref(), Some("deadbeef"));

    let renamed = repo.rename(&uuid, "renamed").await.unwrap().unwrap();
    assert_eq!(renamed.name, "renamed");
}

#[sqlx::test]
async fn child_applications_upsert(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let uuid = new_service(&pool, fx.environment_id, fx.destination_id).await;
    let repo = ServiceRepo::new(pool.clone());
    let svc = repo.get_by_uuid(&uuid).await.unwrap().unwrap();

    let a = repo
        .upsert_application(svc.id, "umami", None, Some("umami:latest"), false)
        .await
        .unwrap();
    repo.upsert_application(svc.id, "postgresql", None, Some("postgres:16"), true)
        .await
        .unwrap();
    // Upsert by (service_id, name) is idempotent.
    let a2 = repo
        .upsert_application(svc.id, "umami", None, Some("umami:3.0"), false)
        .await
        .unwrap();
    assert_eq!(a.uuid, a2.uuid, "same row on conflict");

    let apps = repo.applications(svc.id).await.unwrap();
    assert_eq!(apps.len(), 2);
    let db = apps.iter().find(|x| x.name == "postgresql").unwrap();
    assert!(db.is_database);
    let web = apps.iter().find(|x| x.name == "umami").unwrap();
    assert_eq!(web.image.as_deref(), Some("umami:3.0"));
}

#[sqlx::test]
async fn service_scoped_env_vars(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let uuid = new_service(&pool, fx.environment_id, fx.destination_id).await;
    let repo = ServiceRepo::new(pool.clone());
    let svc = repo.get_by_uuid(&uuid).await.unwrap().unwrap();

    let env = EnvVarRepo::new(pool.clone());
    env.upsert(NewEnvVar {
        resource_kind: "service".into(),
        resource_id: svc.id,
        key: "SERVICE_PASSWORD_64_UMAMI".into(),
        value: "supersecret".into(),
        is_buildtime: false,
        is_literal: false,
        is_shown_once: true,
    })
    .await
    .unwrap();

    let vars = env.list("service", svc.id).await.unwrap();
    assert_eq!(vars.len(), 1);
    assert_eq!(vars[0].value, "supersecret");
    assert!(vars[0].is_shown_once);

    // Deleting the service cascades to its child applications.
    assert!(repo.delete(&uuid).await.unwrap());
    assert!(repo.get_by_uuid(&uuid).await.unwrap().is_none());
}
