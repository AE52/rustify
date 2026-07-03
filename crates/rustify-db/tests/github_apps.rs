//! GithubAppRepo: encrypted-at-rest secrets + CRUD.

use sqlx::PgPool;

use rustify_db::repos::github_apps::{GithubAppPatch, GithubAppRepo, NewGithubApp};

mod common;
use common::setup;

fn new_app(team_id: i64) -> NewGithubApp {
    NewGithubApp {
        team_id,
        name: "acme".into(),
        client_secret: Some("cs-super-secret".into()),
        webhook_secret: Some("wh-super-secret".into()),
        app_id: Some(12345),
        installation_id: Some(67890),
        client_id: Some("Iv1.abcdef".into()),
        ..Default::default()
    }
}

#[sqlx::test]
async fn create_applies_defaults(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let gh = GithubAppRepo::new(pool.clone())
        .create(new_app(fx.team_id))
        .await
        .unwrap();

    assert_eq!(gh.api_url, "https://api.github.com");
    assert_eq!(gh.html_url, "https://github.com");
    assert_eq!(gh.custom_user, "git");
    assert_eq!(gh.custom_port, 22);
    assert!(!gh.is_public);
    assert_eq!(gh.app_id, Some(12345));
}

#[sqlx::test]
async fn secrets_are_encrypted_at_rest_and_roundtrip(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let repo = GithubAppRepo::new(pool.clone());
    let gh = repo.create(new_app(fx.team_id)).await.unwrap();

    // The raw columns must not contain the plaintext.
    let (cs_enc, wh_enc): (Vec<u8>, Vec<u8>) = sqlx::query_as(
        "SELECT client_secret_enc, webhook_secret_enc FROM github_apps WHERE id = $1",
    )
    .bind(gh.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        !cs_enc.windows(3).any(|w| w == b"cs-"),
        "client secret ciphertext leaks plaintext"
    );
    assert!(
        !wh_enc.windows(3).any(|w| w == b"wh-"),
        "webhook secret ciphertext leaks plaintext"
    );

    // Decrypt round-trips to the original plaintext.
    assert_eq!(
        repo.decrypt_client_secret(gh.id).await.unwrap().as_deref(),
        Some("cs-super-secret")
    );
    assert_eq!(
        repo.decrypt_webhook_secret(gh.id).await.unwrap().as_deref(),
        Some("wh-super-secret")
    );
}

#[sqlx::test]
async fn update_reencrypts_secret_and_patches_fields(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let repo = GithubAppRepo::new(pool.clone());
    let gh = repo.create(new_app(fx.team_id)).await.unwrap();

    let patched = repo
        .update(
            &gh.uuid,
            &GithubAppPatch {
                name: Some("renamed".into()),
                client_secret: Some("rotated".into()),
                is_public: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(patched.name, "renamed");
    assert!(patched.is_public);
    // untouched secret is unchanged, rotated secret decrypts to the new value
    assert_eq!(
        repo.decrypt_client_secret(gh.id).await.unwrap().as_deref(),
        Some("rotated")
    );
    assert_eq!(
        repo.decrypt_webhook_secret(gh.id).await.unwrap().as_deref(),
        Some("wh-super-secret")
    );
}

#[sqlx::test]
async fn manifest_and_installation_setters_persist(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let repo = GithubAppRepo::new(pool.clone());
    let gh = repo
        .create(NewGithubApp {
            team_id: fx.team_id,
            name: "shell".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    // Create a private key to reference (reuse the fixture key material path).
    let pk_id: i64 = sqlx::query_scalar(
        "INSERT INTO private_keys (uuid, team_id, name, private_key_enc, public_key)
         VALUES ($1, $2, 'gh', $3, '') RETURNING id",
    )
    .bind(rustify_core::ids::new_uuid())
    .bind(fx.team_id)
    .bind(vec![0u8; 4])
    .fetch_one(&pool)
    .await
    .unwrap();

    repo.set_manifest_credentials(
        &gh.uuid, "slugged", 999, "cid", "csecret", "whsecret", pk_id,
    )
    .await
    .unwrap();
    repo.set_installation_id(&gh.uuid, 424242).await.unwrap();

    let reloaded = repo.get_by_uuid(&gh.uuid).await.unwrap().unwrap();
    assert_eq!(reloaded.name, "slugged");
    assert_eq!(reloaded.app_id, Some(999));
    assert_eq!(reloaded.client_id.as_deref(), Some("cid"));
    assert_eq!(reloaded.private_key_id, Some(pk_id));
    assert_eq!(reloaded.installation_id, Some(424242));
    assert_eq!(
        repo.decrypt_client_secret(gh.id).await.unwrap().as_deref(),
        Some("csecret")
    );
}
