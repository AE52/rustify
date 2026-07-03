//! Database-backup aggregate (migration `0004_backups.sql`): S3 storage
//! credentials, scheduled backups and their executions.
//!
//! S3 access key + secret are AES-256-GCM encrypted (`rustify_core::crypto`)
//! into `key_enc` / `secret_enc` before insert and decrypted only on explicit
//! read; the plaintext is never logged. Clean-slate port of Coolify's
//! `S3Storage` / `ScheduledDatabaseBackup` / `ScheduledDatabaseBackupExecution`.

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use rustify_core::{ExecMeta, crypto, ids};

use crate::{DbError, DbResult};

// ----- S3 storage ---------------------------------------------------------

/// A row of `s3_storages` with the encrypted key/secret blobs elided — safe to
/// serialise to API responses (credentials are write-only).
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct S3Storage {
    pub id: i64,
    pub uuid: String,
    pub team_id: i64,
    pub name: String,
    pub region: String,
    pub endpoint: Option<String>,
    pub bucket: String,
    pub path: String,
    pub use_path_style: bool,
    pub is_usable: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Decrypted S3 access credentials (never serialised).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3Credentials {
    pub key: String,
    pub secret: String,
}

/// Fields accepted when registering an S3 storage; key/secret are encrypted
/// before insert.
#[derive(Debug, Clone)]
pub struct NewS3Storage {
    pub team_id: i64,
    pub name: String,
    pub region: String,
    pub endpoint: Option<String>,
    pub bucket: String,
    pub key: String,
    pub secret: String,
    pub path: String,
    pub use_path_style: bool,
}

/// Partial update for an S3 storage. `None` leaves a column unchanged; key and
/// secret are only re-encrypted when provided.
#[derive(Debug, Clone, Default)]
pub struct S3StoragePatch {
    pub name: Option<String>,
    pub region: Option<String>,
    pub endpoint: Option<String>,
    pub bucket: Option<String>,
    pub key: Option<String>,
    pub secret: Option<String>,
    pub path: Option<String>,
    pub use_path_style: Option<bool>,
}

const S3_COLS: &str = "id, uuid, team_id, name, region, endpoint, bucket, path, \
     use_path_style, is_usable, created_at, updated_at";

#[derive(Clone)]
pub struct S3StorageRepo {
    pool: PgPool,
}

impl S3StorageRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(&self, new: NewS3Storage) -> DbResult<S3Storage> {
        let uuid = ids::new_uuid();
        let key_enc = crypto::encrypt(new.key.trim().as_bytes());
        let secret_enc = crypto::encrypt(new.secret.trim().as_bytes());
        let row = sqlx::query_as::<_, S3Storage>(&format!(
            "INSERT INTO s3_storages
               (uuid, team_id, name, region, endpoint, bucket, key_enc, secret_enc, path,
                use_path_style)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             RETURNING {S3_COLS}"
        ))
        .bind(&uuid)
        .bind(new.team_id)
        .bind(&new.name)
        .bind(&new.region)
        .bind(&new.endpoint)
        .bind(&new.bucket)
        .bind(&key_enc)
        .bind(&secret_enc)
        .bind(&new.path)
        .bind(new.use_path_style)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn get_by_uuid(&self, uuid: &str) -> DbResult<Option<S3Storage>> {
        let row = sqlx::query_as::<_, S3Storage>(&format!(
            "SELECT {S3_COLS} FROM s3_storages WHERE uuid = $1"
        ))
        .bind(uuid)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn get_by_id(&self, id: i64) -> DbResult<Option<S3Storage>> {
        let row = sqlx::query_as::<_, S3Storage>(&format!(
            "SELECT {S3_COLS} FROM s3_storages WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn list_by_team(&self, team_id: i64) -> DbResult<Vec<S3Storage>> {
        let rows = sqlx::query_as::<_, S3Storage>(&format!(
            "SELECT {S3_COLS} FROM s3_storages WHERE team_id = $1 ORDER BY name"
        ))
        .bind(team_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn update(&self, uuid: &str, patch: &S3StoragePatch) -> DbResult<Option<S3Storage>> {
        let key_enc = patch
            .key
            .as_ref()
            .map(|k| crypto::encrypt(k.trim().as_bytes()));
        let secret_enc = patch
            .secret
            .as_ref()
            .map(|s| crypto::encrypt(s.trim().as_bytes()));
        let row = sqlx::query_as::<_, S3Storage>(&format!(
            "UPDATE s3_storages SET
                name = COALESCE($2, name),
                region = COALESCE($3, region),
                endpoint = COALESCE($4, endpoint),
                bucket = COALESCE($5, bucket),
                key_enc = COALESCE($6, key_enc),
                secret_enc = COALESCE($7, secret_enc),
                path = COALESCE($8, path),
                use_path_style = COALESCE($9, use_path_style),
                updated_at = now()
              WHERE uuid = $1
              RETURNING {S3_COLS}"
        ))
        .bind(uuid)
        .bind(&patch.name)
        .bind(&patch.region)
        .bind(&patch.endpoint)
        .bind(&patch.bucket)
        .bind(&key_enc)
        .bind(&secret_enc)
        .bind(&patch.path)
        .bind(patch.use_path_style)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Decrypt and return the access key + secret for `id`.
    pub async fn decrypt_credentials(&self, id: i64) -> DbResult<S3Credentials> {
        let row: Option<(Vec<u8>, Vec<u8>)> =
            sqlx::query_as("SELECT key_enc, secret_enc FROM s3_storages WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        let (key_enc, secret_enc) = row.ok_or(DbError::NotFound)?;
        let key = String::from_utf8(crypto::decrypt(&key_enc)?).map_err(|_| DbError::Utf8)?;
        let secret = String::from_utf8(crypto::decrypt(&secret_enc)?).map_err(|_| DbError::Utf8)?;
        Ok(S3Credentials { key, secret })
    }

    /// Record the outcome of a connectivity test.
    pub async fn set_usable(&self, id: i64, usable: bool) -> DbResult<()> {
        sqlx::query("UPDATE s3_storages SET is_usable = $2, updated_at = now() WHERE id = $1")
            .bind(id)
            .bind(usable)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Delete a storage, first detaching any schedules that reference it
    /// (mirrors Coolify's `S3Storage::deleting` hook).
    pub async fn delete(&self, uuid: &str) -> DbResult<bool> {
        let mut tx = self.pool.begin().await?;
        let id: Option<i64> = sqlx::query_scalar("SELECT id FROM s3_storages WHERE uuid = $1")
            .bind(uuid)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(id) = id else {
            return Ok(false);
        };
        sqlx::query(
            "UPDATE scheduled_database_backups SET save_s3 = false, s3_storage_id = NULL, \
             updated_at = now() WHERE s3_storage_id = $1",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        let result = sqlx::query("DELETE FROM s3_storages WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(result.rows_affected() == 1)
    }
}

// ----- Scheduled backups --------------------------------------------------

/// A row of `scheduled_database_backups`.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct ScheduledBackup {
    pub id: i64,
    pub uuid: String,
    pub database_id: i64,
    pub enabled: bool,
    pub frequency: String,
    pub save_s3: bool,
    pub s3_storage_id: Option<i64>,
    pub databases_to_backup: Option<String>,
    pub dump_all: bool,
    pub disable_local_backup: bool,
    pub retention_amount_local: i32,
    pub retention_days_local: i32,
    pub retention_max_gb_local: i32,
    pub retention_amount_s3: i32,
    pub retention_days_s3: i32,
    pub retention_max_gb_s3: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Fields accepted when creating a schedule.
#[derive(Debug, Clone)]
pub struct NewScheduledBackup {
    pub database_id: i64,
    pub frequency: String,
    pub enabled: bool,
    pub save_s3: bool,
    pub s3_storage_id: Option<i64>,
    pub databases_to_backup: Option<String>,
    pub dump_all: bool,
    pub disable_local_backup: bool,
    pub retention_amount_local: i32,
    pub retention_days_local: i32,
    pub retention_max_gb_local: i32,
    pub retention_amount_s3: i32,
    pub retention_days_s3: i32,
    pub retention_max_gb_s3: i32,
}

/// Partial update for a schedule. `None` leaves the column unchanged.
#[derive(Debug, Clone, Default)]
pub struct ScheduledBackupPatch {
    pub frequency: Option<String>,
    pub enabled: Option<bool>,
    pub save_s3: Option<bool>,
    pub s3_storage_id: Option<Option<i64>>,
    pub databases_to_backup: Option<String>,
    pub dump_all: Option<bool>,
    pub disable_local_backup: Option<bool>,
    pub retention_amount_local: Option<i32>,
    pub retention_days_local: Option<i32>,
    pub retention_max_gb_local: Option<i32>,
    pub retention_amount_s3: Option<i32>,
    pub retention_days_s3: Option<i32>,
    pub retention_max_gb_s3: Option<i32>,
}

const BACKUP_COLS: &str = "id, uuid, database_id, enabled, frequency, save_s3, s3_storage_id, \
     databases_to_backup, dump_all, disable_local_backup, retention_amount_local, \
     retention_days_local, retention_max_gb_local, retention_amount_s3, retention_days_s3, \
     retention_max_gb_s3, created_at, updated_at";

#[derive(Clone)]
pub struct ScheduledBackupRepo {
    pool: PgPool,
}

impl ScheduledBackupRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(&self, new: NewScheduledBackup) -> DbResult<ScheduledBackup> {
        let uuid = ids::new_uuid();
        let row = sqlx::query_as::<_, ScheduledBackup>(&format!(
            "INSERT INTO scheduled_database_backups
               (uuid, database_id, enabled, frequency, save_s3, s3_storage_id,
                databases_to_backup, dump_all, disable_local_backup, retention_amount_local,
                retention_days_local, retention_max_gb_local, retention_amount_s3,
                retention_days_s3, retention_max_gb_s3)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)
             RETURNING {BACKUP_COLS}"
        ))
        .bind(&uuid)
        .bind(new.database_id)
        .bind(new.enabled)
        .bind(&new.frequency)
        .bind(new.save_s3)
        .bind(new.s3_storage_id)
        .bind(&new.databases_to_backup)
        .bind(new.dump_all)
        .bind(new.disable_local_backup)
        .bind(new.retention_amount_local)
        .bind(new.retention_days_local)
        .bind(new.retention_max_gb_local)
        .bind(new.retention_amount_s3)
        .bind(new.retention_days_s3)
        .bind(new.retention_max_gb_s3)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn get_by_uuid(&self, uuid: &str) -> DbResult<Option<ScheduledBackup>> {
        let row = sqlx::query_as::<_, ScheduledBackup>(&format!(
            "SELECT {BACKUP_COLS} FROM scheduled_database_backups WHERE uuid = $1"
        ))
        .bind(uuid)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn get_by_id(&self, id: i64) -> DbResult<Option<ScheduledBackup>> {
        let row = sqlx::query_as::<_, ScheduledBackup>(&format!(
            "SELECT {BACKUP_COLS} FROM scheduled_database_backups WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn list_by_database(&self, database_id: i64) -> DbResult<Vec<ScheduledBackup>> {
        let rows = sqlx::query_as::<_, ScheduledBackup>(&format!(
            "SELECT {BACKUP_COLS} FROM scheduled_database_backups WHERE database_id = $1 \
             ORDER BY id"
        ))
        .bind(database_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// All enabled schedules (for the per-minute dispatcher).
    pub async fn list_enabled(&self) -> DbResult<Vec<ScheduledBackup>> {
        let rows = sqlx::query_as::<_, ScheduledBackup>(&format!(
            "SELECT {BACKUP_COLS} FROM scheduled_database_backups WHERE enabled = true ORDER BY id"
        ))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn update(
        &self,
        uuid: &str,
        patch: &ScheduledBackupPatch,
    ) -> DbResult<Option<ScheduledBackup>> {
        // s3_storage_id is Option<Option<i64>>: outer None = leave, inner
        // controls set-to-null vs set-to-value. Split into (touch, value).
        let touch_s3 = patch.s3_storage_id.is_some();
        let s3_value = patch.s3_storage_id.flatten();
        let row = sqlx::query_as::<_, ScheduledBackup>(&format!(
            "UPDATE scheduled_database_backups SET
                frequency = COALESCE($2, frequency),
                enabled = COALESCE($3, enabled),
                save_s3 = COALESCE($4, save_s3),
                s3_storage_id = CASE WHEN $5 THEN $6 ELSE s3_storage_id END,
                databases_to_backup = COALESCE($7, databases_to_backup),
                dump_all = COALESCE($8, dump_all),
                disable_local_backup = COALESCE($9, disable_local_backup),
                retention_amount_local = COALESCE($10, retention_amount_local),
                retention_days_local = COALESCE($11, retention_days_local),
                retention_max_gb_local = COALESCE($12, retention_max_gb_local),
                retention_amount_s3 = COALESCE($13, retention_amount_s3),
                retention_days_s3 = COALESCE($14, retention_days_s3),
                retention_max_gb_s3 = COALESCE($15, retention_max_gb_s3),
                updated_at = now()
              WHERE uuid = $1
              RETURNING {BACKUP_COLS}"
        ))
        .bind(uuid)
        .bind(&patch.frequency)
        .bind(patch.enabled)
        .bind(patch.save_s3)
        .bind(touch_s3)
        .bind(s3_value)
        .bind(&patch.databases_to_backup)
        .bind(patch.dump_all)
        .bind(patch.disable_local_backup)
        .bind(patch.retention_amount_local)
        .bind(patch.retention_days_local)
        .bind(patch.retention_max_gb_local)
        .bind(patch.retention_amount_s3)
        .bind(patch.retention_days_s3)
        .bind(patch.retention_max_gb_s3)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn delete(&self, uuid: &str) -> DbResult<bool> {
        let result = sqlx::query("DELETE FROM scheduled_database_backups WHERE uuid = $1")
            .bind(uuid)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }
}

// ----- Executions ---------------------------------------------------------

/// A row of `scheduled_database_backup_executions`.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct BackupExecution {
    pub id: i64,
    pub uuid: String,
    pub scheduled_database_backup_id: i64,
    pub status: String,
    pub filename: Option<String>,
    pub size: i64,
    pub s3_uploaded: Option<bool>,
    pub local_storage_deleted: bool,
    pub s3_storage_deleted: bool,
    pub message: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// Terminal result recorded on an execution when the backup finishes.
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    pub status: String,
    pub size: i64,
    pub filename: Option<String>,
    pub s3_uploaded: Option<bool>,
    pub message: Option<String>,
    pub local_storage_deleted: bool,
}

const EXEC_COLS: &str = "id, uuid, scheduled_database_backup_id, status, filename, size, \
     s3_uploaded, local_storage_deleted, s3_storage_deleted, message, started_at, finished_at, \
     created_at";

#[derive(Clone)]
pub struct BackupExecutionRepo {
    pool: PgPool,
}

impl BackupExecutionRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Create a `running` execution row and return it (the handler payload
    /// carries only its uuid).
    pub async fn create_running(&self, backup_id: i64) -> DbResult<BackupExecution> {
        let uuid = ids::new_uuid();
        let row = sqlx::query_as::<_, BackupExecution>(&format!(
            "INSERT INTO scheduled_database_backup_executions
               (uuid, scheduled_database_backup_id, status)
             VALUES ($1, $2, 'running')
             RETURNING {EXEC_COLS}"
        ))
        .bind(&uuid)
        .bind(backup_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn get_by_uuid(&self, uuid: &str) -> DbResult<Option<BackupExecution>> {
        let row = sqlx::query_as::<_, BackupExecution>(&format!(
            "SELECT {EXEC_COLS} FROM scheduled_database_backup_executions WHERE uuid = $1"
        ))
        .bind(uuid)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn list_by_backup(&self, backup_id: i64) -> DbResult<Vec<BackupExecution>> {
        let rows = sqlx::query_as::<_, BackupExecution>(&format!(
            "SELECT {EXEC_COLS} FROM scheduled_database_backup_executions
             WHERE scheduled_database_backup_id = $1 ORDER BY created_at DESC, id DESC"
        ))
        .bind(backup_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Successful executions still holding a local copy, newest first — the
    /// input to the local retention rules.
    pub async fn successful_with_local(&self, backup_id: i64) -> DbResult<Vec<ExecMeta>> {
        self.successful_meta(backup_id, "local_storage_deleted")
            .await
    }

    /// Successful executions still holding an S3 copy, newest first.
    pub async fn successful_with_s3(&self, backup_id: i64) -> DbResult<Vec<ExecMeta>> {
        self.successful_meta(backup_id, "s3_storage_deleted").await
    }

    async fn successful_meta(&self, backup_id: i64, flag: &str) -> DbResult<Vec<ExecMeta>> {
        let rows: Vec<(i64, DateTime<Utc>, i64)> = sqlx::query_as(&format!(
            "SELECT id, created_at, size FROM scheduled_database_backup_executions
             WHERE scheduled_database_backup_id = $1 AND status = 'success' AND {flag} = false
             ORDER BY created_at DESC, id DESC"
        ))
        .bind(backup_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, created_at, size)| ExecMeta {
                id,
                created_at,
                size,
            })
            .collect())
    }

    /// Resolve `(id, filename)` for a set of execution ids (for issuing the
    /// remote `rm` / `mc rm` after retention selection).
    pub async fn filenames_for(&self, ids: &[i64]) -> DbResult<Vec<(i64, String)>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows: Vec<(i64, Option<String>)> = sqlx::query_as(
            "SELECT id, filename FROM scheduled_database_backup_executions WHERE id = ANY($1)",
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(id, f)| f.map(|f| (id, f)))
            .collect())
    }

    /// Record the terminal result of a backup execution.
    pub async fn finish(&self, id: i64, result: &ExecutionResult) -> DbResult<()> {
        sqlx::query(
            "UPDATE scheduled_database_backup_executions SET
                status = $2, size = $3, filename = $4, s3_uploaded = $5, message = $6,
                local_storage_deleted = $7, finished_at = now()
             WHERE id = $1",
        )
        .bind(id)
        .bind(&result.status)
        .bind(result.size)
        .bind(&result.filename)
        .bind(result.s3_uploaded)
        .bind(&result.message)
        .bind(result.local_storage_deleted)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn mark_local_deleted(&self, ids: &[i64]) -> DbResult<()> {
        if ids.is_empty() {
            return Ok(());
        }
        sqlx::query(
            "UPDATE scheduled_database_backup_executions SET local_storage_deleted = true \
             WHERE id = ANY($1)",
        )
        .bind(ids)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn mark_s3_deleted(&self, ids: &[i64]) -> DbResult<()> {
        if ids.is_empty() {
            return Ok(());
        }
        sqlx::query(
            "UPDATE scheduled_database_backup_executions SET s3_storage_deleted = true \
             WHERE id = ANY($1)",
        )
        .bind(ids)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Drop execution rows whose every backup copy is gone (Coolify
    /// removeOldBackups cleanup, databases.php:264-275).
    pub async fn prune_orphans(&self, backup_id: i64) -> DbResult<()> {
        sqlx::query(
            "DELETE FROM scheduled_database_backup_executions
             WHERE scheduled_database_backup_id = $1
               AND local_storage_deleted = true
               AND (s3_storage_deleted = true OR s3_uploaded IS NULL)",
        )
        .bind(backup_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Whether a schedule already has an execution created in the current
    /// clock-minute (per-minute dispatcher dedup, Task-Z style).
    pub async fn exists_in_current_minute(&self, backup_id: i64) -> DbResult<bool> {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM scheduled_database_backup_executions
             WHERE scheduled_database_backup_id = $1
               AND created_at >= date_trunc('minute', now()))",
        )
        .bind(backup_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(exists)
    }
}
