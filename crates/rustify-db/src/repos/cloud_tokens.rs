//! Cloud-provider API tokens (Hetzner). The token is AES-256-GCM encrypted
//! (`rustify_core::crypto`) before it is written and is decrypted only on
//! explicit read; the plaintext is never logged or serialised.

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use rustify_core::{crypto, ids};

use crate::{DbError, DbResult};

/// A `cloud_provider_tokens` row with the encrypted blob elided — safe to
/// serialise to API responses (the token is write-only).
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct CloudProviderToken {
    pub id: i64,
    pub uuid: String,
    pub team_id: i64,
    pub provider: String,
    pub name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLS: &str = "id, uuid, team_id, provider, name, created_at, updated_at";

#[derive(Clone)]
pub struct CloudTokenRepo {
    pool: PgPool,
}

impl CloudTokenRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Store a token; `token` is encrypted before it is written.
    pub async fn create(
        &self,
        team_id: i64,
        provider: &str,
        name: Option<&str>,
        token: &str,
    ) -> DbResult<CloudProviderToken> {
        let uuid = ids::new_uuid();
        let enc = crypto::encrypt(token.as_bytes());
        let row = sqlx::query_as::<_, CloudProviderToken>(&format!(
            "INSERT INTO cloud_provider_tokens (uuid, team_id, provider, name, token_enc)
             VALUES ($1, $2, $3, $4, $5) RETURNING {COLS}"
        ))
        .bind(&uuid)
        .bind(team_id)
        .bind(provider)
        .bind(name)
        .bind(&enc)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn list(&self, team_id: i64) -> DbResult<Vec<CloudProviderToken>> {
        let rows = sqlx::query_as::<_, CloudProviderToken>(&format!(
            "SELECT {COLS} FROM cloud_provider_tokens WHERE team_id = $1 ORDER BY id"
        ))
        .bind(team_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn get_by_uuid(&self, uuid: &str) -> DbResult<Option<CloudProviderToken>> {
        let row = sqlx::query_as::<_, CloudProviderToken>(&format!(
            "SELECT {COLS} FROM cloud_provider_tokens WHERE uuid = $1"
        ))
        .bind(uuid)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Decrypt and return the token material for `uuid`, scoped to `team_id`.
    /// Used by the Hetzner client to authenticate API calls.
    pub async fn decrypt_token(&self, team_id: i64, uuid: &str) -> DbResult<String> {
        let enc: Vec<u8> = sqlx::query_scalar(
            "SELECT token_enc FROM cloud_provider_tokens WHERE uuid = $1 AND team_id = $2",
        )
        .bind(uuid)
        .bind(team_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(DbError::NotFound)?;
        let plain = crypto::decrypt(&enc)?;
        String::from_utf8(plain).map_err(|_| DbError::Utf8)
    }

    pub async fn delete(&self, team_id: i64, uuid: &str) -> DbResult<bool> {
        let result =
            sqlx::query("DELETE FROM cloud_provider_tokens WHERE uuid = $1 AND team_id = $2")
                .bind(uuid)
                .bind(team_id)
                .execute(&self.pool)
                .await?;
        Ok(result.rows_affected() == 1)
    }
}
