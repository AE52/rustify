//! Private-keys aggregate. The private key material is AES-256-GCM encrypted
//! (`rustify_core::crypto`) before it is written and is decrypted only on
//! explicit read; the plaintext is never logged.

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use rustify_core::{crypto, ids};

use crate::{DbError, DbResult};

/// A row of `private_keys` with the encrypted blob elided — safe to serialise
/// to API responses (the private key is write-only, contract C5).
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct PrivateKey {
    pub id: i64,
    pub uuid: String,
    pub team_id: i64,
    pub name: String,
    pub public_key: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLS: &str = "id, uuid, team_id, name, public_key, created_at, updated_at";

#[derive(Clone)]
pub struct KeyRepo {
    pool: PgPool,
}

impl KeyRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Store a key pair; `private_key` is encrypted before it is written.
    pub async fn create(
        &self,
        team_id: i64,
        name: &str,
        private_key: &str,
        public_key: &str,
    ) -> DbResult<PrivateKey> {
        let uuid = ids::new_uuid();
        let enc = crypto::encrypt(private_key.as_bytes());
        let row = sqlx::query_as::<_, PrivateKey>(&format!(
            "INSERT INTO private_keys (uuid, team_id, name, private_key_enc, public_key)
             VALUES ($1, $2, $3, $4, $5) RETURNING {COLS}"
        ))
        .bind(&uuid)
        .bind(team_id)
        .bind(name)
        .bind(&enc)
        .bind(public_key)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn get_by_uuid(&self, uuid: &str) -> DbResult<Option<PrivateKey>> {
        let row = sqlx::query_as::<_, PrivateKey>(&format!(
            "SELECT {COLS} FROM private_keys WHERE uuid = $1"
        ))
        .bind(uuid)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Resolve a key by its numeric id — used by the API to render the
    /// `private_key_uuid` of a server (contract C5 server response shape).
    pub async fn get_by_id(&self, id: i64) -> DbResult<Option<PrivateKey>> {
        let row = sqlx::query_as::<_, PrivateKey>(&format!(
            "SELECT {COLS} FROM private_keys WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Partial update for `PATCH /private-keys/{uuid}` (contract C5). `name`
    /// renames; `key_material` (decrypted PEM + its derived public key)
    /// re-encrypts and rotates the stored key. `NULL` args leave the column
    /// unchanged. Returns the updated row, or `None` if the uuid is unknown.
    pub async fn update(
        &self,
        uuid: &str,
        name: Option<&str>,
        key_material: Option<(&str, &str)>,
    ) -> DbResult<Option<PrivateKey>> {
        let (enc, public_key) = match key_material {
            Some((private_key, public_key)) => (
                Some(crypto::encrypt(private_key.as_bytes())),
                Some(public_key),
            ),
            None => (None, None),
        };
        let row = sqlx::query_as::<_, PrivateKey>(&format!(
            "UPDATE private_keys
                SET name = COALESCE($2, name),
                    private_key_enc = COALESCE($3, private_key_enc),
                    public_key = COALESCE($4, public_key),
                    updated_at = now()
              WHERE uuid = $1
              RETURNING {COLS}"
        ))
        .bind(uuid)
        .bind(name)
        .bind(enc)
        .bind(public_key)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn list(&self, team_id: i64) -> DbResult<Vec<PrivateKey>> {
        let rows = sqlx::query_as::<_, PrivateKey>(&format!(
            "SELECT {COLS} FROM private_keys WHERE team_id = $1 ORDER BY id"
        ))
        .bind(team_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Decrypt and return the private key material for `id`. Used by track B
    /// (ssh) to materialise the on-disk 0600 key file.
    pub async fn decrypt_private_key(&self, id: i64) -> DbResult<String> {
        let enc: Vec<u8> =
            sqlx::query_scalar("SELECT private_key_enc FROM private_keys WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?
                .ok_or(DbError::NotFound)?;
        let plain = crypto::decrypt(&enc)?;
        String::from_utf8(plain).map_err(|_| DbError::Utf8)
    }

    pub async fn rename(&self, uuid: &str, name: &str) -> DbResult<Option<PrivateKey>> {
        let row = sqlx::query_as::<_, PrivateKey>(&format!(
            "UPDATE private_keys SET name = $2, updated_at = now() WHERE uuid = $1 RETURNING {COLS}"
        ))
        .bind(uuid)
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn delete(&self, uuid: &str) -> DbResult<bool> {
        let result = sqlx::query("DELETE FROM private_keys WHERE uuid = $1")
            .bind(uuid)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }
}
