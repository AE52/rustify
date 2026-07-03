//! CloudTokenRepo: encrypted-at-rest storage of cloud-provider API tokens,
//! team-scoped decryption, listing and deletion.

use sqlx::PgPool;

use rustify_db::repos::CloudTokenRepo;

mod common;
use common::init_secret_key;

async fn mk_team(pool: &PgPool) -> i64 {
    sqlx::query_scalar("INSERT INTO teams (uuid, name) VALUES ($1, 'team') RETURNING id")
        .bind(rustify_core::ids::new_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn create_decrypt_roundtrip_and_encrypted_at_rest(pool: PgPool) {
    init_secret_key();
    let team_id = mk_team(&pool).await;
    let repo = CloudTokenRepo::new(pool.clone());

    let secret = "hetzner-api-token-SUPERSECRET-abc123";
    let token = repo
        .create(team_id, "hetzner", Some("prod"), secret)
        .await
        .unwrap();
    assert_eq!(token.provider, "hetzner");
    assert_eq!(token.name.as_deref(), Some("prod"));

    // Team-scoped decrypt round-trips to the original plaintext.
    let decrypted = repo.decrypt_token(team_id, &token.uuid).await.unwrap();
    assert_eq!(decrypted, secret);

    // The stored blob is genuinely encrypted (never the plaintext token).
    let blob: Vec<u8> =
        sqlx::query_scalar("SELECT token_enc FROM cloud_provider_tokens WHERE id = $1")
            .bind(token.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let haystack = String::from_utf8_lossy(&blob);
    assert!(
        !haystack.contains("SUPERSECRET"),
        "token must not be stored in plaintext"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn decrypt_is_team_scoped(pool: PgPool) {
    init_secret_key();
    let team_a = mk_team(&pool).await;
    let team_b = mk_team(&pool).await;
    let repo = CloudTokenRepo::new(pool.clone());

    let token = repo.create(team_a, "hetzner", None, "tok-A").await.unwrap();

    // Another team cannot decrypt it.
    assert!(repo.decrypt_token(team_b, &token.uuid).await.is_err());
    // The owning team can.
    assert_eq!(
        repo.decrypt_token(team_a, &token.uuid).await.unwrap(),
        "tok-A"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn list_and_delete_are_team_scoped(pool: PgPool) {
    init_secret_key();
    let team_a = mk_team(&pool).await;
    let team_b = mk_team(&pool).await;
    let repo = CloudTokenRepo::new(pool.clone());

    let a1 = repo.create(team_a, "hetzner", None, "a1").await.unwrap();
    repo.create(team_a, "hetzner", None, "a2").await.unwrap();
    repo.create(team_b, "hetzner", None, "b1").await.unwrap();

    assert_eq!(repo.list(team_a).await.unwrap().len(), 2);
    assert_eq!(repo.list(team_b).await.unwrap().len(), 1);

    // team_b cannot delete team_a's token.
    assert!(!repo.delete(team_b, &a1.uuid).await.unwrap());
    // team_a can.
    assert!(repo.delete(team_a, &a1.uuid).await.unwrap());
    assert_eq!(repo.list(team_a).await.unwrap().len(), 1);
}
