//! Standalone-databases aggregate (migration `0002_databases.sql`). Engine
//! credentials are AES-256-GCM encrypted (`rustify_core::crypto`) into
//! `credentials_enc` before they touch the database and decrypted only on
//! explicit read; the plaintext is never logged.
//!
//! Clean-slate collapse of Coolify's eight `Standalone*` models into one repo
//! discriminated by the `engine` column.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgPool;

use rustify_core::{DatabaseCredentials, crypto, ids};

use crate::{DbError, DbResult};

/// A full row of `standalone_databases`, with the encrypted credential blob
/// elided — safe to serialise to API responses (credentials are write-only).
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct StandaloneDatabase {
    pub id: i64,
    pub uuid: String,
    pub environment_id: i64,
    pub destination_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub engine: String,
    pub image: String,
    pub status: String,
    pub is_public: bool,
    pub public_port: Option<i32>,
    pub public_port_timeout: i32,
    pub ports_mappings: Option<String>,
    pub engine_config: Value,
    pub limits_memory: String,
    pub limits_cpus: String,
    pub health_check_enabled: bool,
    pub health_check_interval: i32,
    pub health_check_timeout: i32,
    pub health_check_retries: i32,
    pub health_check_start_period: i32,
    pub restart_count: i32,
    pub started_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Fields accepted by the create-database API; everything else takes its schema
/// default. `credentials` are encrypted before insert.
#[derive(Debug, Clone)]
pub struct NewDatabase {
    pub environment_id: i64,
    pub destination_id: i64,
    pub name: String,
    pub engine: String,
    pub image: String,
    pub credentials: DatabaseCredentials,
    pub is_public: bool,
    pub public_port: Option<i32>,
}

/// Partial update for `PATCH /databases/{uuid}`. `None` leaves the column
/// unchanged (COALESCE), so nullable columns cannot be cleared to NULL here.
#[derive(Debug, Clone, Default)]
pub struct DatabasePatch {
    pub name: Option<String>,
    pub description: Option<String>,
    pub image: Option<String>,
    pub is_public: Option<bool>,
    pub public_port: Option<i32>,
    pub public_port_timeout: Option<i32>,
    pub ports_mappings: Option<String>,
    pub limits_memory: Option<String>,
    pub limits_cpus: Option<String>,
    pub health_check_enabled: Option<bool>,
}

// Every column except the encrypted blob, in table order.
const COLS: &str = "id, uuid, environment_id, destination_id, name, description, engine, image, \
     status, is_public, public_port, public_port_timeout, ports_mappings, engine_config, \
     limits_memory, limits_cpus, health_check_enabled, health_check_interval, \
     health_check_timeout, health_check_retries, health_check_start_period, restart_count, \
     started_at, created_at, updated_at";

#[derive(Clone)]
pub struct DatabaseRepo {
    pool: PgPool,
}

impl DatabaseRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(&self, new: NewDatabase) -> DbResult<StandaloneDatabase> {
        let uuid = ids::new_uuid();
        let creds_json = serde_json::to_vec(&new.credentials)?;
        let enc = crypto::encrypt(&creds_json);
        let row = sqlx::query_as::<_, StandaloneDatabase>(&format!(
            "INSERT INTO standalone_databases
               (uuid, environment_id, destination_id, name, engine, image, credentials_enc,
                is_public, public_port)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             RETURNING {COLS}"
        ))
        .bind(&uuid)
        .bind(new.environment_id)
        .bind(new.destination_id)
        .bind(&new.name)
        .bind(&new.engine)
        .bind(&new.image)
        .bind(&enc)
        .bind(new.is_public)
        .bind(new.public_port)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn get_by_uuid(&self, uuid: &str) -> DbResult<Option<StandaloneDatabase>> {
        let row = sqlx::query_as::<_, StandaloneDatabase>(&format!(
            "SELECT {COLS} FROM standalone_databases WHERE uuid = $1"
        ))
        .bind(uuid)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn get_by_id(&self, id: i64) -> DbResult<Option<StandaloneDatabase>> {
        let row = sqlx::query_as::<_, StandaloneDatabase>(&format!(
            "SELECT {COLS} FROM standalone_databases WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn list_by_environment(
        &self,
        environment_id: i64,
    ) -> DbResult<Vec<StandaloneDatabase>> {
        let rows = sqlx::query_as::<_, StandaloneDatabase>(&format!(
            "SELECT {COLS} FROM standalone_databases WHERE environment_id = $1 ORDER BY id"
        ))
        .bind(environment_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn update(
        &self,
        uuid: &str,
        patch: &DatabasePatch,
    ) -> DbResult<Option<StandaloneDatabase>> {
        let row = sqlx::query_as::<_, StandaloneDatabase>(&format!(
            "UPDATE standalone_databases SET
                name = COALESCE($2, name),
                description = COALESCE($3, description),
                image = COALESCE($4, image),
                is_public = COALESCE($5, is_public),
                public_port = COALESCE($6, public_port),
                public_port_timeout = COALESCE($7, public_port_timeout),
                ports_mappings = COALESCE($8, ports_mappings),
                limits_memory = COALESCE($9, limits_memory),
                limits_cpus = COALESCE($10, limits_cpus),
                health_check_enabled = COALESCE($11, health_check_enabled),
                updated_at = now()
              WHERE uuid = $1
              RETURNING {COLS}"
        ))
        .bind(uuid)
        .bind(&patch.name)
        .bind(&patch.description)
        .bind(&patch.image)
        .bind(patch.is_public)
        .bind(patch.public_port)
        .bind(patch.public_port_timeout)
        .bind(&patch.ports_mappings)
        .bind(&patch.limits_memory)
        .bind(&patch.limits_cpus)
        .bind(patch.health_check_enabled)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Update the container status string (`running`, `exited`, ...).
    pub async fn set_status(&self, id: i64, status: &str) -> DbResult<()> {
        sqlx::query(
            "UPDATE standalone_databases SET status = $2, updated_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(status)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Stamp `started_at = now()` (called when a start job succeeds).
    pub async fn mark_started(&self, id: i64) -> DbResult<()> {
        sqlx::query(
            "UPDATE standalone_databases SET started_at = now(), updated_at = now() WHERE id = $1",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Decrypt and return the engine credentials for `uuid`.
    pub async fn decrypt_credentials(&self, uuid: &str) -> DbResult<DatabaseCredentials> {
        let enc: Vec<u8> =
            sqlx::query_scalar("SELECT credentials_enc FROM standalone_databases WHERE uuid = $1")
                .bind(uuid)
                .fetch_optional(&self.pool)
                .await?
                .ok_or(DbError::NotFound)?;
        let plain = crypto::decrypt(&enc)?;
        Ok(serde_json::from_slice(&plain)?)
    }

    pub async fn delete(&self, uuid: &str) -> DbResult<bool> {
        let result = sqlx::query("DELETE FROM standalone_databases WHERE uuid = $1")
            .bind(uuid)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }
}
