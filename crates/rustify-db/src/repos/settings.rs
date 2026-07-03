//! Instance settings, API tokens and login sessions — the small aggregates the
//! auth and settings routes (contract C5) need.

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use rustify_core::ids;

use crate::DbResult;

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct InstanceSettings {
    pub id: i64,
    pub fqdn: Option<String>,
    pub wildcard_domain: Option<String>,
    pub registration_enabled: bool,
    /// Instance default for allowing preview deployments from public/fork
    /// contributors (migration 0008).
    pub is_pr_deployments_public_enabled: bool,
    pub updated_at: DateTime<Utc>,
}

/// The instance-settings columns shared by the get/update queries.
const INSTANCE_COLS: &str =
    "id, fqdn, wildcard_domain, registration_enabled, is_pr_deployments_public_enabled, updated_at";

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct ApiToken {
    pub id: i64,
    pub uuid: String,
    pub team_id: i64,
    pub name: String,
    pub abilities: Vec<String>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Session {
    pub id: String,
    pub user_id: i64,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct SettingsRepo {
    pool: PgPool,
}

impl SettingsRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Fetch the singleton instance-settings row, creating it on first access.
    pub async fn get(&self) -> DbResult<InstanceSettings> {
        let row = sqlx::query_as::<_, InstanceSettings>(&format!(
            "INSERT INTO instance_settings (id) VALUES (1)
             ON CONFLICT (id) DO UPDATE SET id = instance_settings.id
             RETURNING {INSTANCE_COLS}"
        ))
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn update(
        &self,
        fqdn: Option<&str>,
        wildcard_domain: Option<&str>,
        registration_enabled: bool,
        is_pr_deployments_public_enabled: bool,
    ) -> DbResult<InstanceSettings> {
        self.get().await?;
        let row = sqlx::query_as::<_, InstanceSettings>(&format!(
            "UPDATE instance_settings
                SET fqdn = $1, wildcard_domain = $2, registration_enabled = $3,
                    is_pr_deployments_public_enabled = $4, updated_at = now()
              WHERE id = 1
              RETURNING {INSTANCE_COLS}"
        ))
        .bind(fqdn)
        .bind(wildcard_domain)
        .bind(registration_enabled)
        .bind(is_pr_deployments_public_enabled)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    // --- sessions ---

    /// Create a login session with an opaque random id (caller supplies it).
    pub async fn create_session(
        &self,
        id: &str,
        user_id: i64,
        expires_at: DateTime<Utc>,
    ) -> DbResult<()> {
        sqlx::query("INSERT INTO sessions (id, user_id, expires_at) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(user_id)
            .bind(expires_at)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Fetch a session iff it exists and has not expired.
    pub async fn get_session(&self, id: &str) -> DbResult<Option<Session>> {
        let row = sqlx::query_as::<_, Session>(
            "SELECT id, user_id, expires_at, created_at FROM sessions
             WHERE id = $1 AND expires_at > now()",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn delete_session(&self, id: &str) -> DbResult<()> {
        sqlx::query("DELETE FROM sessions WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Revoke every session of a user — forces re-auth so a changed team role
    /// or membership takes effect immediately (Coolify `RevokeUserTeamTokens` +
    /// team-cache clear on member demotion/removal). Returns rows deleted.
    pub async fn revoke_user_sessions(&self, user_id: i64) -> DbResult<u64> {
        let result = sqlx::query("DELETE FROM sessions WHERE user_id = $1")
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    // --- api tokens ---

    /// Store an API token by its hash (the plaintext is shown once, never
    /// persisted — contract C5).
    pub async fn create_api_token(
        &self,
        team_id: i64,
        name: &str,
        token_hash: &str,
    ) -> DbResult<ApiToken> {
        let row = sqlx::query_as::<_, ApiToken>(
            "INSERT INTO api_tokens (uuid, team_id, name, token_hash)
             VALUES ($1, $2, $3, $4)
             RETURNING id, uuid, team_id, name, abilities, last_used_at, created_at",
        )
        .bind(ids::new_uuid())
        .bind(team_id)
        .bind(name)
        .bind(token_hash)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn list_api_tokens(&self, team_id: i64) -> DbResult<Vec<ApiToken>> {
        let rows = sqlx::query_as::<_, ApiToken>(
            "SELECT id, uuid, team_id, name, abilities, last_used_at, created_at
             FROM api_tokens WHERE team_id = $1 ORDER BY id",
        )
        .bind(team_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Resolve a presented token by its hash, bumping `last_used_at`.
    pub async fn find_api_token_by_hash(&self, token_hash: &str) -> DbResult<Option<ApiToken>> {
        let row = sqlx::query_as::<_, ApiToken>(
            "UPDATE api_tokens SET last_used_at = now() WHERE token_hash = $1
             RETURNING id, uuid, team_id, name, abilities, last_used_at, created_at",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn delete_api_token(&self, uuid: &str) -> DbResult<bool> {
        let result = sqlx::query("DELETE FROM api_tokens WHERE uuid = $1")
            .bind(uuid)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }
}
