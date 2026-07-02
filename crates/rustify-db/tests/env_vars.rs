//! EnvVarRepo: encrypted-at-rest storage + unique-key upsert.

use sqlx::PgPool;

use rustify_db::repos::env_vars::{EnvVarRepo, NewEnvVar};

mod common;
use common::{new_app, setup};

fn var(app_id: i64, key: &str, value: &str) -> NewEnvVar {
    NewEnvVar {
        resource_kind: "application".into(),
        resource_id: app_id,
        key: key.into(),
        value: value.into(),
        is_buildtime: false,
        is_literal: false,
        is_shown_once: false,
    }
}

#[sqlx::test]
async fn upsert_is_unique_per_key_and_updates_in_place(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let app = new_app(&pool, &fx).await;
    let repo = EnvVarRepo::new(pool.clone());

    let first = repo.upsert(var(app, "API_URL", "http://a")).await.unwrap();
    let updated = repo.upsert(var(app, "API_URL", "http://b")).await.unwrap();

    // Same row (unique key), value replaced, no duplicate inserted.
    assert_eq!(first.id, updated.id);
    assert_eq!(updated.value, "http://b");

    repo.upsert(var(app, "OTHER", "x")).await.unwrap();
    let all = repo.list("application", app).await.unwrap();
    assert_eq!(all.len(), 2, "distinct keys are distinct rows");

    let api = all.iter().find(|v| v.key == "API_URL").unwrap();
    assert_eq!(api.value, "http://b", "decrypted value round-trips");
}

#[sqlx::test]
async fn value_is_encrypted_at_rest(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let app = new_app(&pool, &fx).await;
    let repo = EnvVarRepo::new(pool.clone());
    repo.upsert(var(app, "SECRET", "plaintext-value"))
        .await
        .unwrap();

    // The stored blob must not contain the plaintext bytes.
    let blob: Vec<u8> = sqlx::query_scalar(
        "SELECT value_enc FROM environment_variables WHERE resource_id = $1 AND key = 'SECRET'",
    )
    .bind(app)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        !blob.windows(15).any(|w| w == b"plaintext-value"),
        "value must be encrypted at rest"
    );
}
