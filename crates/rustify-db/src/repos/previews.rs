//! Application PR previews aggregate (coolify app/Models/ApplicationPreview.php).
//!
//! One row per `(application_id, pull_request_id)`. The deploy engine generates
//! the preview `fqdn`/`status`; the webhook upserts the row when a PR opens and
//! the cleanup handler deletes it (with its containers/network) when the PR
//! closes. `status` changes stamp `last_online_at` (parity with the model's
//! `saving` hook).

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgPool;

use rustify_core::ids;

use crate::DbResult;

/// A row of the `application_previews` table.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct ApplicationPreview {
    pub id: i64,
    pub uuid: String,
    pub application_id: i64,
    pub pull_request_id: i32,
    pub pull_request_html_url: Option<String>,
    pub pull_request_issue_comment_id: Option<i64>,
    pub fqdn: Option<String>,
    pub status: String,
    pub git_type: Option<String>,
    pub docker_compose_domains: Option<Value>,
    pub docker_registry_image_tag: Option<String>,
    pub last_online_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLS: &str = "id, uuid, application_id, pull_request_id, pull_request_html_url, \
     pull_request_issue_comment_id, fqdn, status, git_type, docker_compose_domains, \
     docker_registry_image_tag, last_online_at, created_at, updated_at";

#[derive(Clone)]
pub struct PreviewRepo {
    pool: PgPool,
}

impl PreviewRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Insert or refresh the preview for `(application_id, pull_request_id)`,
    /// returning the row. On conflict the html url + git type are refreshed but
    /// the generated fqdn/status/comment id are preserved.
    pub async fn upsert(
        &self,
        application_id: i64,
        pull_request_id: i32,
        pull_request_html_url: Option<&str>,
        git_type: Option<&str>,
    ) -> DbResult<ApplicationPreview> {
        let uuid = ids::new_uuid();
        let row = sqlx::query_as::<_, ApplicationPreview>(&format!(
            "INSERT INTO application_previews
               (uuid, application_id, pull_request_id, pull_request_html_url, git_type)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (application_id, pull_request_id) DO UPDATE SET
               pull_request_html_url = COALESCE(EXCLUDED.pull_request_html_url,
                                                 application_previews.pull_request_html_url),
               git_type = COALESCE(EXCLUDED.git_type, application_previews.git_type),
               updated_at = now()
             RETURNING {COLS}"
        ))
        .bind(&uuid)
        .bind(application_id)
        .bind(pull_request_id)
        .bind(pull_request_html_url)
        .bind(git_type)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn get(
        &self,
        application_id: i64,
        pull_request_id: i32,
    ) -> DbResult<Option<ApplicationPreview>> {
        let row = sqlx::query_as::<_, ApplicationPreview>(&format!(
            "SELECT {COLS} FROM application_previews
             WHERE application_id = $1 AND pull_request_id = $2"
        ))
        .bind(application_id)
        .bind(pull_request_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn list_by_application(
        &self,
        application_id: i64,
    ) -> DbResult<Vec<ApplicationPreview>> {
        let rows = sqlx::query_as::<_, ApplicationPreview>(&format!(
            "SELECT {COLS} FROM application_previews WHERE application_id = $1 ORDER BY id"
        ))
        .bind(application_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Set the generated preview `fqdn`; leaves status/comment id untouched.
    pub async fn set_fqdn(&self, id: i64, fqdn: Option<&str>) -> DbResult<()> {
        sqlx::query("UPDATE application_previews SET fqdn = $2, updated_at = now() WHERE id = $1")
            .bind(id)
            .bind(fqdn)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Update the container status; stamps `last_online_at` (model `saving` hook).
    pub async fn set_status(&self, id: i64, status: &str) -> DbResult<()> {
        sqlx::query(
            "UPDATE application_previews
                SET status = $2, last_online_at = now(), updated_at = now()
              WHERE id = $1",
        )
        .bind(id)
        .bind(status)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Persist the single editable PR issue-comment id.
    pub async fn set_comment_id(&self, id: i64, comment_id: Option<i64>) -> DbResult<()> {
        sqlx::query(
            "UPDATE application_previews
                SET pull_request_issue_comment_id = $2, updated_at = now()
              WHERE id = $1",
        )
        .bind(id)
        .bind(comment_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete(&self, id: i64) -> DbResult<bool> {
        let result = sqlx::query("DELETE FROM application_previews WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }
}
