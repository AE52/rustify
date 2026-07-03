//! GitHub App sources aggregate (coolify app/Models/GithubApp.php).
//!
//! `client_secret` and `webhook_secret` are AES-256-GCM encrypted
//! (`rustify_core::crypto`) before they are written and never serialised back:
//! the [`GithubApp`] read model elides them. The App's RSA private key lives in
//! `private_keys` (referenced by `private_key_id`), reusing the existing
//! encrypted-at-rest key store.

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use rustify_core::{crypto, ids};

use crate::DbResult;

/// A `github_apps` row with the encrypted secrets elided — safe to serialise to
/// API responses (parity with GithubController::removeSensitiveData).
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct GithubApp {
    pub id: i64,
    pub uuid: String,
    pub team_id: i64,
    pub private_key_id: Option<i64>,
    pub name: String,
    pub organization: Option<String>,
    pub api_url: String,
    pub html_url: String,
    pub custom_user: String,
    pub custom_port: i32,
    pub app_id: Option<i64>,
    pub installation_id: Option<i64>,
    pub client_id: Option<String>,
    pub is_system_wide: bool,
    pub is_public: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLS: &str = "id, uuid, team_id, private_key_id, name, organization, api_url, html_url, \
     custom_user, custom_port, app_id, installation_id, client_id, is_system_wide, is_public, \
     created_at, updated_at";

/// Fields accepted when creating a GitHub App source.
#[derive(Debug, Clone, Default)]
pub struct NewGithubApp {
    pub team_id: i64,
    pub name: String,
    pub organization: Option<String>,
    pub api_url: Option<String>,
    pub html_url: Option<String>,
    pub custom_user: Option<String>,
    pub custom_port: Option<i32>,
    pub app_id: Option<i64>,
    pub installation_id: Option<i64>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub webhook_secret: Option<String>,
    pub private_key_id: Option<i64>,
    pub is_public: Option<bool>,
    pub is_system_wide: Option<bool>,
}

/// Partial update for `PATCH /github-apps/{uuid}`; `None` leaves a column
/// unchanged (via `COALESCE`).
#[derive(Debug, Clone, Default)]
pub struct GithubAppPatch {
    pub name: Option<String>,
    pub organization: Option<String>,
    pub api_url: Option<String>,
    pub html_url: Option<String>,
    pub custom_user: Option<String>,
    pub custom_port: Option<i32>,
    pub app_id: Option<i64>,
    pub installation_id: Option<i64>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub webhook_secret: Option<String>,
    pub private_key_id: Option<i64>,
    pub is_public: Option<bool>,
    pub is_system_wide: Option<bool>,
}

#[derive(Clone)]
pub struct GithubAppRepo {
    pool: PgPool,
}

impl GithubAppRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(&self, new: NewGithubApp) -> DbResult<GithubApp> {
        let uuid = ids::new_uuid();
        let client_secret_enc = new.client_secret.map(|s| crypto::encrypt(s.as_bytes()));
        let webhook_secret_enc = new.webhook_secret.map(|s| crypto::encrypt(s.as_bytes()));
        let row = sqlx::query_as::<_, GithubApp>(&format!(
            "INSERT INTO github_apps
               (uuid, team_id, name, organization, api_url, html_url, custom_user, custom_port,
                app_id, installation_id, client_id, client_secret_enc, webhook_secret_enc,
                private_key_id, is_public, is_system_wide)
             VALUES ($1,$2,$3,$4,
                     COALESCE($5,'https://api.github.com'),
                     COALESCE($6,'https://github.com'),
                     COALESCE($7,'git'),
                     COALESCE($8,22),
                     $9,$10,$11,$12,$13,$14,
                     COALESCE($15,false),
                     COALESCE($16,false))
             RETURNING {COLS}"
        ))
        .bind(&uuid)
        .bind(new.team_id)
        .bind(&new.name)
        .bind(&new.organization)
        .bind(&new.api_url)
        .bind(&new.html_url)
        .bind(&new.custom_user)
        .bind(new.custom_port)
        .bind(new.app_id)
        .bind(new.installation_id)
        .bind(&new.client_id)
        .bind(&client_secret_enc)
        .bind(&webhook_secret_enc)
        .bind(new.private_key_id)
        .bind(new.is_public)
        .bind(new.is_system_wide)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn get_by_uuid(&self, uuid: &str) -> DbResult<Option<GithubApp>> {
        let row = sqlx::query_as::<_, GithubApp>(&format!(
            "SELECT {COLS} FROM github_apps WHERE uuid = $1"
        ))
        .bind(uuid)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn get_by_id(&self, id: i64) -> DbResult<Option<GithubApp>> {
        let row = sqlx::query_as::<_, GithubApp>(&format!(
            "SELECT {COLS} FROM github_apps WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn list(&self, team_id: i64) -> DbResult<Vec<GithubApp>> {
        let rows = sqlx::query_as::<_, GithubApp>(&format!(
            "SELECT {COLS} FROM github_apps WHERE team_id = $1 ORDER BY id"
        ))
        .bind(team_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn update(&self, uuid: &str, patch: &GithubAppPatch) -> DbResult<Option<GithubApp>> {
        let client_secret_enc = patch
            .client_secret
            .as_ref()
            .map(|s| crypto::encrypt(s.as_bytes()));
        let webhook_secret_enc = patch
            .webhook_secret
            .as_ref()
            .map(|s| crypto::encrypt(s.as_bytes()));
        let row = sqlx::query_as::<_, GithubApp>(&format!(
            "UPDATE github_apps SET
                name = COALESCE($2, name),
                organization = COALESCE($3, organization),
                api_url = COALESCE($4, api_url),
                html_url = COALESCE($5, html_url),
                custom_user = COALESCE($6, custom_user),
                custom_port = COALESCE($7, custom_port),
                app_id = COALESCE($8, app_id),
                installation_id = COALESCE($9, installation_id),
                client_id = COALESCE($10, client_id),
                client_secret_enc = COALESCE($11, client_secret_enc),
                webhook_secret_enc = COALESCE($12, webhook_secret_enc),
                private_key_id = COALESCE($13, private_key_id),
                is_public = COALESCE($14, is_public),
                is_system_wide = COALESCE($15, is_system_wide),
                updated_at = now()
              WHERE uuid = $1
              RETURNING {COLS}"
        ))
        .bind(uuid)
        .bind(&patch.name)
        .bind(&patch.organization)
        .bind(&patch.api_url)
        .bind(&patch.html_url)
        .bind(&patch.custom_user)
        .bind(patch.custom_port)
        .bind(patch.app_id)
        .bind(patch.installation_id)
        .bind(&patch.client_id)
        .bind(&client_secret_enc)
        .bind(&webhook_secret_enc)
        .bind(patch.private_key_id)
        .bind(patch.is_public)
        .bind(patch.is_system_wide)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Persist the credentials produced by the app-manifest conversion
    /// (Webhook/Github.php redirect): app_id, name, client_id, secrets and the
    /// newly created private key.
    #[allow(clippy::too_many_arguments)]
    pub async fn set_manifest_credentials(
        &self,
        uuid: &str,
        name: &str,
        app_id: i64,
        client_id: &str,
        client_secret: &str,
        webhook_secret: &str,
        private_key_id: i64,
    ) -> DbResult<Option<GithubApp>> {
        let patch = GithubAppPatch {
            name: Some(name.to_string()),
            app_id: Some(app_id),
            client_id: Some(client_id.to_string()),
            client_secret: Some(client_secret.to_string()),
            webhook_secret: Some(webhook_secret.to_string()),
            private_key_id: Some(private_key_id),
            ..Default::default()
        };
        self.update(uuid, &patch).await
    }

    /// Persist a verified installation id (Webhook/Github.php install).
    pub async fn set_installation_id(
        &self,
        uuid: &str,
        installation_id: i64,
    ) -> DbResult<Option<GithubApp>> {
        self.update(
            uuid,
            &GithubAppPatch {
                installation_id: Some(installation_id),
                ..Default::default()
            },
        )
        .await
    }

    /// Decrypt the stored client secret (used by the OAuth/webhook flows).
    pub async fn decrypt_client_secret(&self, id: i64) -> DbResult<Option<String>> {
        self.decrypt_column("client_secret_enc", id).await
    }

    /// Decrypt the stored webhook secret (used to verify webhook signatures).
    pub async fn decrypt_webhook_secret(&self, id: i64) -> DbResult<Option<String>> {
        self.decrypt_column("webhook_secret_enc", id).await
    }

    async fn decrypt_column(&self, column: &str, id: i64) -> DbResult<Option<String>> {
        let enc: Option<Vec<u8>> =
            sqlx::query_scalar(&format!("SELECT {column} FROM github_apps WHERE id = $1"))
                .bind(id)
                .fetch_optional(&self.pool)
                .await?
                .flatten();
        match enc {
            Some(bytes) => {
                let plain = crypto::decrypt(&bytes)?;
                Ok(Some(
                    String::from_utf8(plain).map_err(|_| crate::DbError::Utf8)?,
                ))
            }
            None => Ok(None),
        }
    }

    pub async fn delete(&self, uuid: &str) -> DbResult<bool> {
        let result = sqlx::query("DELETE FROM github_apps WHERE uuid = $1")
            .bind(uuid)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }
}
