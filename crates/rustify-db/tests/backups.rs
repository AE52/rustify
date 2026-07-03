//! Backup persistence: S3 credential encryption roundtrip, schedule config,
//! executions and retention-input queries against a real Postgres via
//! `#[sqlx::test]`.

use base64::Engine as _;
use sqlx::PgPool;

use rustify_db::repos::{
    BackupExecutionRepo, NewS3Storage, NewScheduledBackup, S3StorageRepo, ScheduledBackupPatch,
    ScheduledBackupRepo,
};

fn init_key() {
    // Fixed 32-byte key so crypto encrypt/decrypt is deterministic in-process.
    let key = base64::engine::general_purpose::STANDARD.encode([5u8; 32]);
    // SAFETY: set once per test binary before any crypto call.
    unsafe {
        std::env::set_var("RUSTIFY_SECRET_KEY", key);
    }
}

async fn seed_team(pool: &PgPool) -> i64 {
    sqlx::query_scalar("INSERT INTO teams (uuid, name) VALUES ($1, 't') RETURNING id")
        .bind(rustify_core::ids::new_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Minimal team → server → destination → project → environment → database chain.
async fn seed_database(pool: &PgPool, team_id: i64) -> i64 {
    init_key();
    let key_id: i64 = sqlx::query_scalar(
        "INSERT INTO private_keys (uuid, team_id, name, private_key_enc, public_key)
         VALUES ($1, $2, 'k', $3, 'ssh-ed25519 AAAA') RETURNING id",
    )
    .bind(rustify_core::ids::new_uuid())
    .bind(team_id)
    .bind(vec![0u8; 4])
    .fetch_one(pool)
    .await
    .unwrap();
    let server_id: i64 = sqlx::query_scalar(
        "INSERT INTO servers (uuid, team_id, name, ip, private_key_id) VALUES ($1,$2,'s','10.0.0.1',$3) RETURNING id",
    )
    .bind(rustify_core::ids::new_uuid())
    .bind(team_id)
    .bind(key_id)
    .fetch_one(pool)
    .await
    .unwrap();
    let dest_id: i64 = sqlx::query_scalar(
        "INSERT INTO destinations (uuid, server_id, network) VALUES ($1,$2,'rustify') RETURNING id",
    )
    .bind(rustify_core::ids::new_uuid())
    .bind(server_id)
    .fetch_one(pool)
    .await
    .unwrap();
    let project_id: i64 = sqlx::query_scalar(
        "INSERT INTO projects (uuid, team_id, name) VALUES ($1,$2,'p') RETURNING id",
    )
    .bind(rustify_core::ids::new_uuid())
    .bind(team_id)
    .fetch_one(pool)
    .await
    .unwrap();
    let env_id: i64 = sqlx::query_scalar(
        "INSERT INTO environments (uuid, project_id, name) VALUES ($1,$2,'production') RETURNING id",
    )
    .bind(rustify_core::ids::new_uuid())
    .bind(project_id)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query_scalar(
        "INSERT INTO standalone_databases (uuid, environment_id, destination_id, name, engine, image, credentials_enc)
         VALUES ($1,$2,$3,'db','postgresql','postgres:16-alpine',$4) RETURNING id",
    )
    .bind(rustify_core::ids::new_uuid())
    .bind(env_id)
    .bind(dest_id)
    .bind(rustify_core::crypto::encrypt(b"{}"))
    .fetch_one(pool)
    .await
    .unwrap()
}

fn new_s3(team_id: i64) -> NewS3Storage {
    NewS3Storage {
        team_id,
        name: "backups".into(),
        region: "us-east-1".into(),
        endpoint: Some("https://s3.example.com".into()),
        bucket: "mybucket".into(),
        key: "  AKIAKEY  ".into(),
        secret: "topsecret".into(),
        path: "/".into(),
        use_path_style: true,
    }
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn s3_credentials_are_encrypted_and_roundtrip(pool: PgPool) {
    init_key();
    let team_id = seed_team(&pool).await;
    let repo = S3StorageRepo::new(pool.clone());
    let s3 = repo.create(new_s3(team_id)).await.unwrap();

    // Plaintext credentials never appear on the serialisable row.
    let json = serde_json::to_value(&s3).unwrap();
    assert!(json.get("key").is_none());
    assert!(json.get("secret").is_none());
    assert!(json.get("key_enc").is_none());

    // Stored blobs are non-empty and not the plaintext.
    let (key_enc, _secret_enc): (Vec<u8>, Vec<u8>) =
        sqlx::query_as("SELECT key_enc, secret_enc FROM s3_storages WHERE id = $1")
            .bind(s3.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!key_enc.is_empty() && key_enc != b"AKIAKEY");

    // Decrypt roundtrips (and whitespace was trimmed on write).
    let creds = repo.decrypt_credentials(s3.id).await.unwrap();
    assert_eq!(creds.key, "AKIAKEY");
    assert_eq!(creds.secret, "topsecret");
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn deleting_s3_detaches_schedules(pool: PgPool) {
    init_key();
    let team_id = seed_team(&pool).await;
    let db_id = seed_database(&pool, team_id).await;
    let s3 = S3StorageRepo::new(pool.clone())
        .create(new_s3(team_id))
        .await
        .unwrap();
    let backup = ScheduledBackupRepo::new(pool.clone())
        .create(NewScheduledBackup {
            database_id: db_id,
            frequency: "0 2 * * *".into(),
            enabled: true,
            save_s3: true,
            s3_storage_id: Some(s3.id),
            databases_to_backup: None,
            dump_all: false,
            disable_local_backup: false,
            retention_amount_local: 3,
            retention_days_local: 0,
            retention_max_gb_local: 0,
            retention_amount_s3: 0,
            retention_days_s3: 0,
            retention_max_gb_s3: 0,
        })
        .await
        .unwrap();

    assert!(
        S3StorageRepo::new(pool.clone())
            .delete(&s3.uuid)
            .await
            .unwrap()
    );

    let reloaded = ScheduledBackupRepo::new(pool.clone())
        .get_by_uuid(&backup.uuid)
        .await
        .unwrap()
        .unwrap();
    assert!(!reloaded.save_s3);
    assert_eq!(reloaded.s3_storage_id, None);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn schedule_config_and_patch(pool: PgPool) {
    init_key();
    let team_id = seed_team(&pool).await;
    let db_id = seed_database(&pool, team_id).await;
    let repo = ScheduledBackupRepo::new(pool.clone());
    let backup = repo
        .create(NewScheduledBackup {
            database_id: db_id,
            frequency: "0 * * * *".into(),
            enabled: true,
            save_s3: false,
            s3_storage_id: None,
            databases_to_backup: Some("app".into()),
            dump_all: false,
            disable_local_backup: false,
            retention_amount_local: 5,
            retention_days_local: 7,
            retention_max_gb_local: 10,
            retention_amount_s3: 0,
            retention_days_s3: 0,
            retention_max_gb_s3: 0,
        })
        .await
        .unwrap();
    assert_eq!(backup.retention_amount_local, 5);
    assert!(
        repo.list_enabled()
            .await
            .unwrap()
            .iter()
            .any(|b| b.id == backup.id)
    );

    // Disable it and confirm it drops out of list_enabled.
    let patched = repo
        .update(
            &backup.uuid,
            &ScheduledBackupPatch {
                enabled: Some(false),
                frequency: Some("*/15 * * * *".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert!(!patched.enabled);
    assert_eq!(patched.frequency, "*/15 * * * *");
    assert!(repo.list_enabled().await.unwrap().is_empty());
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn executions_and_retention_inputs(pool: PgPool) {
    init_key();
    let team_id = seed_team(&pool).await;
    let db_id = seed_database(&pool, team_id).await;
    let backup = ScheduledBackupRepo::new(pool.clone())
        .create(NewScheduledBackup {
            database_id: db_id,
            frequency: "0 2 * * *".into(),
            enabled: true,
            save_s3: false,
            s3_storage_id: None,
            databases_to_backup: None,
            dump_all: false,
            disable_local_backup: false,
            retention_amount_local: 0,
            retention_days_local: 0,
            retention_max_gb_local: 0,
            retention_amount_s3: 0,
            retention_days_s3: 0,
            retention_max_gb_s3: 0,
        })
        .await
        .unwrap();
    let execs = BackupExecutionRepo::new(pool.clone());

    let exec = execs.create_running(backup.id).await.unwrap();
    assert_eq!(exec.status, "running");

    // Dedup guard: a fresh execution counts within the current minute.
    assert!(execs.exists_in_current_minute(backup.id).await.unwrap());

    // A running execution is not a retention candidate; finishing as success is.
    assert!(
        execs
            .successful_with_local(backup.id)
            .await
            .unwrap()
            .is_empty()
    );
    execs
        .finish(
            exec.id,
            &rustify_db::repos::ExecutionResult {
                status: "success".into(),
                size: 4096,
                filename: Some("/data/rustify/backups/x/pg-dump-app-1.dmp".into()),
                s3_uploaded: None,
                message: None,
                local_storage_deleted: false,
            },
        )
        .await
        .unwrap();

    let meta = execs.successful_with_local(backup.id).await.unwrap();
    assert_eq!(meta.len(), 1);
    assert_eq!(meta[0].size, 4096);

    // filenames_for + mark_local_deleted removes it from the retention set.
    let files = execs.filenames_for(&[exec.id]).await.unwrap();
    assert_eq!(
        files,
        vec![(
            exec.id,
            "/data/rustify/backups/x/pg-dump-app-1.dmp".to_string()
        )]
    );
    execs.mark_local_deleted(&[exec.id]).await.unwrap();
    assert!(
        execs
            .successful_with_local(backup.id)
            .await
            .unwrap()
            .is_empty()
    );

    // prune_orphans drops rows with local deleted + no s3 copy.
    execs.prune_orphans(backup.id).await.unwrap();
    assert!(execs.list_by_backup(backup.id).await.unwrap().is_empty());
}
