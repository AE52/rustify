//! Environment-variables aggregate. Values are AES-256-GCM encrypted
//! (`rustify_core::crypto`) at rest; the unique key `(resource_kind,
//! resource_id, key)` makes writes an idempotent upsert.

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use rustify_core::{crypto, ids};

use crate::DbResult;

/// An env var with its (decrypted) value. Callers that must not leak the value
/// (e.g. `is_shown_once` after first read) are responsible for redacting it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EnvVar {
    pub id: i64,
    pub uuid: String,
    pub resource_kind: String,
    pub resource_id: i64,
    pub key: String,
    pub value: String,
    pub is_buildtime: bool,
    pub is_literal: bool,
    pub is_shown_once: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Raw row as stored (encrypted value blob); converted to [`EnvVar`] on read.
#[derive(sqlx::FromRow)]
struct EnvVarRow {
    id: i64,
    uuid: String,
    resource_kind: String,
    resource_id: i64,
    key: String,
    value_enc: Vec<u8>,
    is_buildtime: bool,
    is_literal: bool,
    is_shown_once: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl EnvVarRow {
    fn decrypt(self) -> DbResult<EnvVar> {
        let value = String::from_utf8(crypto::decrypt(&self.value_enc)?)
            .map_err(|_| crate::DbError::Utf8)?;
        Ok(EnvVar {
            id: self.id,
            uuid: self.uuid,
            resource_kind: self.resource_kind,
            resource_id: self.resource_id,
            key: self.key,
            value,
            is_buildtime: self.is_buildtime,
            is_literal: self.is_literal,
            is_shown_once: self.is_shown_once,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

/// Fields for an env-var write (contract C5).
#[derive(Debug, Clone)]
pub struct NewEnvVar {
    pub resource_kind: String,
    pub resource_id: i64,
    pub key: String,
    pub value: String,
    pub is_buildtime: bool,
    pub is_literal: bool,
    pub is_shown_once: bool,
}

const SELECT_COLS: &str = "id, uuid, resource_kind, resource_id, key, value_enc, \
     is_buildtime, is_literal, is_shown_once, created_at, updated_at";

#[derive(Clone)]
pub struct EnvVarRepo {
    pool: PgPool,
}

impl EnvVarRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Insert or update by `(resource_kind, resource_id, key)`. The value is
    /// re-encrypted on every write; the `uuid` is preserved across updates.
    pub async fn upsert(&self, new: NewEnvVar) -> DbResult<EnvVar> {
        let enc = crypto::encrypt(new.value.as_bytes());
        let row = sqlx::query_as::<_, EnvVarRow>(&format!(
            "INSERT INTO environment_variables
               (uuid, resource_kind, resource_id, key, value_enc, is_buildtime, is_literal, is_shown_once)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (resource_kind, resource_id, key) DO UPDATE
               SET value_enc = EXCLUDED.value_enc,
                   is_buildtime = EXCLUDED.is_buildtime,
                   is_literal = EXCLUDED.is_literal,
                   is_shown_once = EXCLUDED.is_shown_once,
                   updated_at = now()
             RETURNING {SELECT_COLS}"
        ))
        .bind(ids::new_uuid())
        .bind(&new.resource_kind)
        .bind(new.resource_id)
        .bind(&new.key)
        .bind(&enc)
        .bind(new.is_buildtime)
        .bind(new.is_literal)
        .bind(new.is_shown_once)
        .fetch_one(&self.pool)
        .await?;
        row.decrypt()
    }

    /// All env vars for a resource, values decrypted, ordered by key.
    pub async fn list(&self, resource_kind: &str, resource_id: i64) -> DbResult<Vec<EnvVar>> {
        let rows = sqlx::query_as::<_, EnvVarRow>(&format!(
            "SELECT {SELECT_COLS} FROM environment_variables
             WHERE resource_kind = $1 AND resource_id = $2 ORDER BY key"
        ))
        .bind(resource_kind)
        .bind(resource_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(EnvVarRow::decrypt).collect()
    }

    pub async fn delete(&self, uuid: &str) -> DbResult<bool> {
        let result = sqlx::query("DELETE FROM environment_variables WHERE uuid = $1")
            .bind(uuid)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }
}
