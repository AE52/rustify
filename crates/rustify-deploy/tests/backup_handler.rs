//! Database-backup handler against the scripted [`FakeExecutor`]: the dump +
//! size + S3 upload + retention command sequence, and the recorded execution
//! outcome.

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;

use rustify_core::DatabaseEngine;
use rustify_db::repos::databases::{DatabaseRepo, NewDatabase};
use rustify_db::repos::{
    BackupExecutionRepo, NewS3Storage, NewScheduledBackup, S3StorageRepo, ScheduledBackupRepo,
};
use rustify_deploy::run_backup;

mod common;
mod fake;

use common::{deps, init_secret_key, setup};
use fake::FakeExecutor;

async fn make_pg(pool: &PgPool, fx: &common::Fixture) -> (i64, String) {
    let db = DatabaseRepo::new(pool.clone())
        .create(NewDatabase {
            environment_id: fx.environment_id,
            destination_id: fx.destination_id,
            name: "orders".into(),
            engine: DatabaseEngine::Postgresql.as_str().into(),
            image: "postgres:16-alpine".into(),
            credentials: DatabaseEngine::Postgresql.default_credentials(),
            is_public: false,
            public_port: None,
        })
        .await
        .unwrap();
    (db.id, db.uuid)
}

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-07-03T02:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn backup_dumps_measures_uploads_s3(pool: PgPool) {
    init_secret_key();
    let fx = setup(&pool, 2).await;
    let (db_id, db_uuid) = make_pg(&pool, &fx).await;

    let s3 = S3StorageRepo::new(pool.clone())
        .create(NewS3Storage {
            team_id: fx.team_id,
            name: "b".into(),
            region: "us-east-1".into(),
            endpoint: Some("https://s3.example.com".into()),
            bucket: "mybucket".into(),
            key: "AKIA".into(),
            secret: "sk".into(),
            path: "/".into(),
            use_path_style: true,
        })
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

    // du returns a non-zero size so the local backup is considered successful.
    let fake = Arc::new(FakeExecutor::new().respond("du -b", "8192"));
    let d = deps(&pool, fake.clone());
    run_backup(&d.deps, &exec.uuid, now()).await.unwrap();

    // The dump runs the pg_dump command inside the container; the password is
    // present (in-container command) but the file redirect targets the backup dir.
    let dir = format!(
        "/data/rustify/backups/databases/team-{}/orders-{}",
        fx.team_id, db_uuid
    );
    assert!(fake.ran(&format!("mkdir -p {dir}")));
    let dump = fake
        .scripts()
        .into_iter()
        .find(|s| s.contains("pg_dump --format=custom"))
        .expect("dump command ran");
    assert!(dump.contains(&format!("docker exec {db_uuid} sh -c")));
    assert!(dump.contains("PGPASSWORD="));
    assert!(dump.contains(&format!("{dir}/pg-dump-postgres-")));
    assert!(dump.contains(".dmp"));

    assert!(fake.ran("du -b"));
    // S3 upload: helper container + mc alias + mc cp to bucket+path.
    assert!(fake.ran(&format!("--name backup-of-{}", exec.uuid)));
    assert!(fake.ran("mc alias set temporary https://s3.example.com AKIA sk"));
    assert!(fake.ran("mc cp"));
    assert!(fake.ran(&format!("mc cp \"{dir}/pg-dump-postgres-")));
    assert!(fake.ran("temporary/mybucket/"));
    assert!(fake.ran(&format!("docker rm -f backup-of-{}", exec.uuid)));

    // Execution recorded success + size + s3_uploaded.
    let done = execs.get_by_uuid(&exec.uuid).await.unwrap().unwrap();
    assert_eq!(done.status, "success");
    assert_eq!(done.size, 8192);
    assert_eq!(done.s3_uploaded, Some(true));
    assert!(done.filename.is_some());
    assert!(done.finished_at.is_some());
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn empty_dump_is_recorded_failed(pool: PgPool) {
    init_secret_key();
    let fx = setup(&pool, 2).await;
    let (db_id, _uuid) = make_pg(&pool, &fx).await;
    let backup = ScheduledBackupRepo::new(pool.clone())
        .create(simple_backup(db_id))
        .await
        .unwrap();
    let execs = BackupExecutionRepo::new(pool.clone());
    let exec = execs.create_running(backup.id).await.unwrap();

    // du returns 0 -> local backup considered empty -> failure.
    let fake = Arc::new(FakeExecutor::new().respond("du -b", "0"));
    let d = deps(&pool, fake.clone());
    run_backup(&d.deps, &exec.uuid, now()).await.unwrap();

    let done = execs.get_by_uuid(&exec.uuid).await.unwrap().unwrap();
    assert_eq!(done.status, "failed");
    assert!(done.message.unwrap().contains("empty"));
    // No S3 work for a private, non-s3 backup.
    assert!(!fake.ran("mc cp"));
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn retention_removes_old_local_files(pool: PgPool) {
    init_secret_key();
    let fx = setup(&pool, 2).await;
    let (db_id, _uuid) = make_pg(&pool, &fx).await;
    let backup = ScheduledBackupRepo::new(pool.clone())
        .create(NewScheduledBackup {
            retention_amount_local: 2, // keep newest 2
            ..simple_backup(db_id)
        })
        .await
        .unwrap();
    let execs = BackupExecutionRepo::new(pool.clone());

    // Three older successful executions with known filenames; oldest first.
    let mut old_files = Vec::new();
    for i in 0..3 {
        let uuid = rustify_core::ids::new_uuid();
        let filename = format!("/data/rustify/backups/old-{i}.dmp");
        old_files.push(filename.clone());
        sqlx::query(
            "INSERT INTO scheduled_database_backup_executions
               (uuid, scheduled_database_backup_id, status, filename, size, created_at)
             VALUES ($1,$2,'success',$3,1024,$4)",
        )
        .bind(&uuid)
        .bind(backup.id)
        .bind(&filename)
        .bind(now() - Duration::days(10 - i))
        .execute(&pool)
        .await
        .unwrap();
    }

    // The new (newest) execution to run.
    let exec = execs.create_running(backup.id).await.unwrap();
    let fake = Arc::new(FakeExecutor::new().respond("du -b", "1024"));
    let d = deps(&pool, fake.clone());
    run_backup(&d.deps, &exec.uuid, now()).await.unwrap();

    // With 4 successful backups and keep-newest-2, the two oldest files are rm'd.
    assert!(fake.ran(&format!("rm -f \"{}\"", old_files[0])));
    assert!(fake.ran(&format!("rm -f \"{}\"", old_files[1])));
    // The third-oldest is within retention (kept).
    assert!(!fake.ran(&format!("rm -f \"{}\"", old_files[2])));
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn dispatcher_enqueues_due_backups_once(pool: PgPool) {
    use rustify_deploy::dispatch_due_backups;
    init_secret_key();
    let fx = setup(&pool, 2).await;
    let (db_id, _uuid) = make_pg(&pool, &fx).await;
    let repo = ScheduledBackupRepo::new(pool.clone());

    // Due at 02:00.
    let due = repo
        .create(NewScheduledBackup {
            frequency: "0 2 * * *".into(),
            ..simple_backup(db_id)
        })
        .await
        .unwrap();
    // Not due at 02:00.
    repo.create(NewScheduledBackup {
        frequency: "0 5 * * *".into(),
        ..simple_backup(db_id)
    })
    .await
    .unwrap();
    // Disabled + otherwise due.
    repo.create(NewScheduledBackup {
        enabled: false,
        frequency: "0 2 * * *".into(),
        ..simple_backup(db_id)
    })
    .await
    .unwrap();

    let fake = Arc::new(FakeExecutor::new());
    let d = deps(&pool, fake);

    let n = dispatch_due_backups(&d.deps, now()).await.unwrap();
    assert_eq!(n, 1, "only the enabled, due schedule dispatches");

    // A `database_backup` job was enqueued for the due schedule's execution.
    let payload: serde_json::Value =
        sqlx::query_scalar("SELECT payload FROM jobs WHERE kind = 'database_backup' LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    let exec = BackupExecutionRepo::new(pool.clone())
        .list_by_backup(due.id)
        .await
        .unwrap();
    assert_eq!(exec.len(), 1);
    assert_eq!(payload["execution_uuid"], exec[0].uuid);

    // Dedup: a second sweep in the same minute enqueues nothing more.
    let n2 = dispatch_due_backups(&d.deps, now()).await.unwrap();
    assert_eq!(n2, 0);
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'database_backup'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(jobs, 1);
}

fn simple_backup(db_id: i64) -> NewScheduledBackup {
    NewScheduledBackup {
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
    }
}
