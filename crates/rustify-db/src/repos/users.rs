//! Users aggregate + argon2id password hashing.

use argon2::Argon2;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use rustify_core::ids;

use crate::{DbError, DbResult};

/// A row of the `users` table. `password_hash` is a PHC-string argon2id hash,
/// never a plaintext or reversible value.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct User {
    pub id: i64,
    pub uuid: String,
    pub team_id: i64,
    pub email: String,
    pub name: String,
    pub password_hash: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLS: &str = "id, uuid, team_id, email, name, password_hash, created_at, updated_at";

/// Hash `password` with argon2id (default parameters) into a PHC string.
pub fn hash_password(password: &str) -> DbResult<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|_| DbError::PasswordHash)
}

/// Verify `password` against a stored argon2id PHC hash.
pub fn verify_password(password: &str, hash: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

#[derive(Clone)]
pub struct UserRepo {
    pool: PgPool,
}

impl UserRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Create a user; `password` is hashed with argon2id before storage.
    pub async fn create(
        &self,
        team_id: i64,
        email: &str,
        name: &str,
        password: &str,
    ) -> DbResult<User> {
        let uuid = ids::new_uuid();
        let password_hash = hash_password(password)?;
        let row = sqlx::query_as::<_, User>(&format!(
            "INSERT INTO users (uuid, team_id, email, name, password_hash)
             VALUES ($1, $2, $3, $4, $5) RETURNING {COLS}"
        ))
        .bind(&uuid)
        .bind(team_id)
        .bind(email)
        .bind(name)
        .bind(&password_hash)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn find_by_email(&self, email: &str) -> DbResult<Option<User>> {
        let row = sqlx::query_as::<_, User>(&format!("SELECT {COLS} FROM users WHERE email = $1"))
            .bind(email)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }

    pub async fn get_by_uuid(&self, uuid: &str) -> DbResult<Option<User>> {
        let row = sqlx::query_as::<_, User>(&format!("SELECT {COLS} FROM users WHERE uuid = $1"))
            .bind(uuid)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }

    pub async fn get_by_id(&self, id: i64) -> DbResult<Option<User>> {
        let row = sqlx::query_as::<_, User>(&format!("SELECT {COLS} FROM users WHERE id = $1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }
}
