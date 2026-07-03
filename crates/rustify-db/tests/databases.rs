//! Standalone-database repo coverage: create/read/update/delete and the
//! encrypted-at-rest credential roundtrip.

use sqlx::PgPool;

use rustify_core::{DatabaseCredentials, DatabaseEngine};
use rustify_db::repos::databases::{DatabasePatch, DatabaseRepo, NewDatabase};

mod common;
use common::{init_secret_key, setup};

fn creds() -> DatabaseCredentials {
    DatabaseCredentials {
        username: "postgres".into(),
        password: "s3cr3t-passw0rd".into(),
        database: "appdb".into(),
        root_password: "root-passw0rd".into(),
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn create_read_and_decrypt_roundtrip(pool: PgPool) {
    init_secret_key();
    let fx = setup(&pool, 2).await;
    let repo = DatabaseRepo::new(pool.clone());

    let db = repo
        .create(NewDatabase {
            environment_id: fx.environment_id,
            destination_id: fx.destination_id,
            name: "pg".into(),
            engine: DatabaseEngine::Postgresql.as_str().to_string(),
            image: "postgres:16-alpine".into(),
            credentials: creds(),
            is_public: false,
            public_port: None,
        })
        .await
        .unwrap();

    assert_eq!(db.engine, "postgresql");
    assert_eq!(db.status, "exited");
    assert_eq!(db.public_port_timeout, 3600);
    assert!(db.health_check_enabled);

    // Round-trips by uuid and id.
    let by_uuid = repo.get_by_uuid(&db.uuid).await.unwrap().unwrap();
    assert_eq!(by_uuid.id, db.id);
    let by_id = repo.get_by_id(db.id).await.unwrap().unwrap();
    assert_eq!(by_id.uuid, db.uuid);

    // Credentials decrypt back to the originals.
    let decrypted = repo.decrypt_credentials(&db.uuid).await.unwrap();
    assert_eq!(decrypted, creds());

    // The stored blob is genuinely encrypted (not the plaintext password).
    let blob: Vec<u8> =
        sqlx::query_scalar("SELECT credentials_enc FROM standalone_databases WHERE id = $1")
            .bind(db.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let haystack = String::from_utf8_lossy(&blob);
    assert!(
        !haystack.contains("s3cr3t-passw0rd"),
        "password must not be stored in plaintext"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn update_status_started_and_patch(pool: PgPool) {
    init_secret_key();
    let fx = setup(&pool, 2).await;
    let repo = DatabaseRepo::new(pool.clone());
    let db = repo
        .create(NewDatabase {
            environment_id: fx.environment_id,
            destination_id: fx.destination_id,
            name: "redis".into(),
            engine: DatabaseEngine::Redis.as_str().to_string(),
            image: "redis:7.2".into(),
            credentials: DatabaseEngine::Redis.default_credentials(),
            is_public: false,
            public_port: None,
        })
        .await
        .unwrap();

    repo.set_status(db.id, "running").await.unwrap();
    repo.mark_started(db.id).await.unwrap();
    let after = repo.get_by_id(db.id).await.unwrap().unwrap();
    assert_eq!(after.status, "running");
    assert!(after.started_at.is_some());

    let patched = repo
        .update(
            &db.uuid,
            &DatabasePatch {
                is_public: Some(true),
                public_port: Some(6390),
                limits_memory: Some("512m".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert!(patched.is_public);
    assert_eq!(patched.public_port, Some(6390));
    assert_eq!(patched.limits_memory, "512m");

    // list_by_environment sees it; delete removes it.
    assert_eq!(
        repo.list_by_environment(fx.environment_id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(repo.delete(&db.uuid).await.unwrap());
    assert!(repo.get_by_uuid(&db.uuid).await.unwrap().is_none());
}
