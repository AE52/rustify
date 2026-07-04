//! AWS-provisioned server bookkeeping (migration 0013): `create_aws` persists
//! `aws_instance_id`/`aws_region`, `set_swarm_role` toggles the swarm flags, and
//! an `aws` cloud token stores its JSON credentials encrypted at rest.

use sqlx::PgPool;

use rustify_db::repos::{CloudTokenRepo, NewAwsServer, ServerRepo};

mod common;
use common::init_secret_key;

fn uid() -> String {
    rustify_core::ids::new_uuid()
}

async fn seed_team_key(pool: &PgPool) -> (i64, i64) {
    let team: i64 =
        sqlx::query_scalar("INSERT INTO teams (uuid, name) VALUES ($1, 'team') RETURNING id")
            .bind(uid())
            .fetch_one(pool)
            .await
            .unwrap();
    let key: i64 = sqlx::query_scalar(
        "INSERT INTO private_keys (uuid, team_id, name, private_key_enc, public_key)
         VALUES ($1, $2, 'k', $3, 'ssh-ed25519 AAAA') RETURNING id",
    )
    .bind(uid())
    .bind(team)
    .bind(vec![0u8; 4])
    .fetch_one(pool)
    .await
    .unwrap();
    (team, key)
}

#[sqlx::test(migrations = "./migrations")]
async fn create_aws_persists_columns_and_swarm_flags(pool: PgPool) {
    init_secret_key();
    let (team, key) = seed_team_key(&pool).await;
    let token_repo = CloudTokenRepo::new(pool.clone());
    let token = token_repo
        .create(
            team,
            "aws",
            Some("prod"),
            "{\"access_key_id\":\"AKIA\",\"secret_access_key\":\"s\"}",
        )
        .await
        .unwrap();

    let repo = ServerRepo::new(pool.clone());
    let server = repo
        .create_aws(NewAwsServer {
            team_id: team,
            name: "web-1".to_string(),
            ip: "203.0.113.10".to_string(),
            port: 22,
            ssh_user: "ubuntu".to_string(),
            private_key_id: key,
            aws_instance_id: "i-0abc123".to_string(),
            aws_region: "eu-central-1".to_string(),
            cloud_provider_token_id: token.id,
        })
        .await
        .unwrap();

    // AWS columns round-trip.
    assert_eq!(server.aws_instance_id.as_deref(), Some("i-0abc123"));
    assert_eq!(server.aws_region.as_deref(), Some("eu-central-1"));
    assert_eq!(server.ssh_user, "ubuntu");
    assert_eq!(server.cloud_provider_token_id, Some(token.id));

    // A default settings row exists with both swarm flags false.
    let settings = repo.settings(server.id).await.unwrap().unwrap();
    assert!(!settings.is_swarm_manager);
    assert!(!settings.is_swarm_worker);

    // Promote to manager, then flip to worker.
    repo.set_swarm_role(server.id, true, false).await.unwrap();
    let s = repo.settings(server.id).await.unwrap().unwrap();
    assert!(s.is_swarm_manager && !s.is_swarm_worker);

    repo.set_swarm_role(server.id, false, true).await.unwrap();
    let s = repo.settings(server.id).await.unwrap().unwrap();
    assert!(s.is_swarm_worker && !s.is_swarm_manager);

    // Listed by the AWS status-sync query.
    let aws = repo.aws_servers().await.unwrap();
    assert_eq!(aws.len(), 1);
    assert_eq!(aws[0].id, server.id);

    // Cached instance state is updatable (non-destructive).
    repo.set_aws_status(server.id, "stopped").await.unwrap();
    let refreshed = repo.get_by_id(server.id).await.unwrap().unwrap();
    assert_eq!(refreshed.hetzner_server_status.as_deref(), Some("stopped"));
}

#[sqlx::test(migrations = "./migrations")]
async fn aws_token_json_encrypted_roundtrip(pool: PgPool) {
    init_secret_key();
    let team: i64 =
        sqlx::query_scalar("INSERT INTO teams (uuid, name) VALUES ($1, 'team') RETURNING id")
            .bind(uid())
            .fetch_one(&pool)
            .await
            .unwrap();
    let repo = CloudTokenRepo::new(pool.clone());

    let json = "{\"access_key_id\":\"AKIAEXAMPLE\",\"secret_access_key\":\"SUPERSECRETzz\"}";
    let token = repo.create(team, "aws", Some("prod"), json).await.unwrap();
    assert_eq!(token.provider, "aws");

    // Round-trips to the exact JSON blob.
    let decrypted = repo.decrypt_token(team, &token.uuid).await.unwrap();
    assert_eq!(decrypted, json);

    // The secret is genuinely encrypted at rest.
    let blob: Vec<u8> =
        sqlx::query_scalar("SELECT token_enc FROM cloud_provider_tokens WHERE id = $1")
            .bind(token.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let haystack = String::from_utf8_lossy(&blob);
    assert!(
        !haystack.contains("SUPERSECRET"),
        "aws secret must not be stored in plaintext"
    );
}
