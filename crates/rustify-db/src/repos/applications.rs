//! Applications aggregate. The table is wide (contract C6); most columns have
//! SQL defaults, so `create` sets only the fields the create API supplies and
//! lets Postgres fill the rest.

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use rustify_core::{crypto, ids};

use crate::{DbError, DbResult};

/// A full row of the `applications` table.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct Application {
    pub id: i64,
    pub uuid: String,
    pub environment_id: i64,
    pub destination_id: i64,
    pub name: String,
    pub fqdn: Option<String>,
    pub git_repository: String,
    pub git_branch: String,
    pub git_commit_sha: String,
    pub build_pack: String,
    pub static_image: String,
    pub docker_registry_image_name: Option<String>,
    pub docker_registry_image_tag: Option<String>,
    pub dockerfile_location: String,
    pub docker_compose_location: String,
    pub base_directory: String,
    pub publish_directory: Option<String>,
    pub install_command: Option<String>,
    pub build_command: Option<String>,
    pub start_command: Option<String>,
    pub ports_exposes: String,
    pub ports_mappings: Option<String>,
    pub health_check_enabled: bool,
    pub health_check_path: String,
    pub health_check_port: Option<String>,
    pub health_check_host: String,
    pub health_check_method: String,
    pub health_check_return_code: i32,
    pub health_check_interval: i32,
    pub health_check_timeout: i32,
    pub health_check_retries: i32,
    pub health_check_start_period: i32,
    pub limits_memory: String,
    pub limits_cpus: String,
    pub custom_docker_run_options: Option<String>,
    pub status: String,
    pub restart_count: i32,
    pub max_restart_count: i32,
    /// Source discriminator: `github_app` when deployed via a GitHub App source,
    /// otherwise `NULL` (public/`git@`/`file://` clone). Added in migration 0006.
    pub source_type: Option<String>,
    /// FK into `github_apps` when `source_type = 'github_app'`.
    pub source_id: Option<i64>,
    /// FK into `private_keys` for a raw deploy-key (SSH) clone.
    pub private_key_id: Option<i64>,
    /// Provider-side repository id (used by webhook matching).
    pub repository_project_id: Option<i64>,
    /// Whether PR preview deployments are enabled (Coolify `isPRDeployable`).
    pub is_pr_deployments_enabled: bool,
    /// Whether previews from public/fork contributors are allowed (per-app).
    pub is_pr_deployments_public_enabled: bool,
    /// Template for a preview FQDN (default `{{pr_id}}.{{domain}}`).
    pub preview_url_template: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Fields accepted by the create-application API (contract C5); everything
/// else takes its schema default.
#[derive(Debug, Clone)]
pub struct NewApplication {
    pub environment_id: i64,
    pub destination_id: i64,
    pub name: String,
    pub git_repository: String,
    pub git_branch: String,
    pub build_pack: String,
    pub ports_exposes: String,
    pub fqdn: Option<String>,
}

/// Partial update for `PATCH /applications/{uuid}` (contract C5). Every field
/// is optional; `None` leaves the column unchanged (implemented with
/// `COALESCE`, so nullable columns cannot be cleared to `NULL` via PATCH —
/// Phase-1 limitation).
#[derive(Debug, Clone, Default)]
pub struct ApplicationPatch {
    pub name: Option<String>,
    pub fqdn: Option<String>,
    pub git_repository: Option<String>,
    pub git_branch: Option<String>,
    pub git_commit_sha: Option<String>,
    pub build_pack: Option<String>,
    pub static_image: Option<String>,
    pub dockerfile_location: Option<String>,
    pub docker_compose_location: Option<String>,
    pub base_directory: Option<String>,
    pub publish_directory: Option<String>,
    pub install_command: Option<String>,
    pub build_command: Option<String>,
    pub start_command: Option<String>,
    pub ports_exposes: Option<String>,
    pub ports_mappings: Option<String>,
    pub health_check_enabled: Option<bool>,
    pub health_check_path: Option<String>,
    pub limits_memory: Option<String>,
    pub limits_cpus: Option<String>,
    pub custom_docker_run_options: Option<String>,
}

// Every column, in table order — shared by all SELECT/RETURNING queries.
const COLS: &str = "id, uuid, environment_id, destination_id, name, fqdn, git_repository, \
     git_branch, git_commit_sha, build_pack, static_image, docker_registry_image_name, \
     docker_registry_image_tag, dockerfile_location, docker_compose_location, base_directory, \
     publish_directory, install_command, build_command, start_command, ports_exposes, \
     ports_mappings, health_check_enabled, health_check_path, health_check_port, \
     health_check_host, health_check_method, health_check_return_code, health_check_interval, \
     health_check_timeout, health_check_retries, health_check_start_period, limits_memory, \
     limits_cpus, custom_docker_run_options, status, restart_count, max_restart_count, \
     source_type, source_id, private_key_id, repository_project_id, \
     is_pr_deployments_enabled, is_pr_deployments_public_enabled, preview_url_template, \
     created_at, updated_at";

#[derive(Clone)]
pub struct ApplicationRepo {
    pool: PgPool,
}

impl ApplicationRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(&self, new: NewApplication) -> DbResult<Application> {
        let uuid = ids::new_uuid();
        let row = sqlx::query_as::<_, Application>(&format!(
            "INSERT INTO applications
               (uuid, environment_id, destination_id, name, git_repository, git_branch,
                build_pack, ports_exposes, fqdn)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             RETURNING {COLS}"
        ))
        .bind(&uuid)
        .bind(new.environment_id)
        .bind(new.destination_id)
        .bind(&new.name)
        .bind(&new.git_repository)
        .bind(&new.git_branch)
        .bind(&new.build_pack)
        .bind(&new.ports_exposes)
        .bind(new.fqdn)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn get_by_uuid(&self, uuid: &str) -> DbResult<Option<Application>> {
        let row = sqlx::query_as::<_, Application>(&format!(
            "SELECT {COLS} FROM applications WHERE uuid = $1"
        ))
        .bind(uuid)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Resolve an application by numeric id — used to render a deployment's
    /// `application_uuid` (contract C5 shape).
    pub async fn get_by_id(&self, id: i64) -> DbResult<Option<Application>> {
        let row = sqlx::query_as::<_, Application>(&format!(
            "SELECT {COLS} FROM applications WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Apply a partial update (contract C5 `PATCH /applications/{uuid}`),
    /// returning the updated row or `None` if the uuid is unknown.
    pub async fn update(
        &self,
        uuid: &str,
        patch: &ApplicationPatch,
    ) -> DbResult<Option<Application>> {
        let row = sqlx::query_as::<_, Application>(&format!(
            "UPDATE applications SET
                name = COALESCE($2, name),
                fqdn = COALESCE($3, fqdn),
                git_repository = COALESCE($4, git_repository),
                git_branch = COALESCE($5, git_branch),
                git_commit_sha = COALESCE($6, git_commit_sha),
                build_pack = COALESCE($7, build_pack),
                static_image = COALESCE($8, static_image),
                dockerfile_location = COALESCE($9, dockerfile_location),
                docker_compose_location = COALESCE($10, docker_compose_location),
                base_directory = COALESCE($11, base_directory),
                publish_directory = COALESCE($12, publish_directory),
                install_command = COALESCE($13, install_command),
                build_command = COALESCE($14, build_command),
                start_command = COALESCE($15, start_command),
                ports_exposes = COALESCE($16, ports_exposes),
                ports_mappings = COALESCE($17, ports_mappings),
                health_check_enabled = COALESCE($18, health_check_enabled),
                health_check_path = COALESCE($19, health_check_path),
                limits_memory = COALESCE($20, limits_memory),
                limits_cpus = COALESCE($21, limits_cpus),
                custom_docker_run_options = COALESCE($22, custom_docker_run_options),
                updated_at = now()
              WHERE uuid = $1
              RETURNING {COLS}"
        ))
        .bind(uuid)
        .bind(&patch.name)
        .bind(&patch.fqdn)
        .bind(&patch.git_repository)
        .bind(&patch.git_branch)
        .bind(&patch.git_commit_sha)
        .bind(&patch.build_pack)
        .bind(&patch.static_image)
        .bind(&patch.dockerfile_location)
        .bind(&patch.docker_compose_location)
        .bind(&patch.base_directory)
        .bind(&patch.publish_directory)
        .bind(&patch.install_command)
        .bind(&patch.build_command)
        .bind(&patch.start_command)
        .bind(&patch.ports_exposes)
        .bind(&patch.ports_mappings)
        .bind(patch.health_check_enabled)
        .bind(&patch.health_check_path)
        .bind(&patch.limits_memory)
        .bind(&patch.limits_cpus)
        .bind(&patch.custom_docker_run_options)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn list_by_environment(&self, environment_id: i64) -> DbResult<Vec<Application>> {
        let rows = sqlx::query_as::<_, Application>(&format!(
            "SELECT {COLS} FROM applications WHERE environment_id = $1 ORDER BY id"
        ))
        .bind(environment_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Update the container status string (`running`, `exited`, ...) reported
    /// by the status sync loop (track E).
    pub async fn set_status(&self, id: i64, status: &str) -> DbResult<()> {
        sqlx::query("UPDATE applications SET status = $2, updated_at = now() WHERE id = $1")
            .bind(id)
            .bind(status)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Wire an application to its git source (migration 0006). A `github_app`
    /// source sets `source_type='github_app'` + `source_id`; a raw deploy key
    /// sets `private_key_id`. Passing `None` for both leaves a public clone.
    pub async fn set_source(
        &self,
        id: i64,
        source_type: Option<&str>,
        source_id: Option<i64>,
        private_key_id: Option<i64>,
    ) -> DbResult<()> {
        sqlx::query(
            "UPDATE applications
                SET source_type = $2, source_id = $3, private_key_id = $4, updated_at = now()
              WHERE id = $1",
        )
        .bind(id)
        .bind(source_type)
        .bind(source_id)
        .bind(private_key_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Record the commit actually built, so a later deploy can pin/rollback.
    pub async fn set_commit_sha(&self, id: i64, commit_sha: &str) -> DbResult<()> {
        sqlx::query(
            "UPDATE applications SET git_commit_sha = $2, updated_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(commit_sha)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete(&self, uuid: &str) -> DbResult<bool> {
        let result = sqlx::query("DELETE FROM applications WHERE uuid = $1")
            .bind(uuid)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }

    // ---- webhook matching (migration 0006/0008) -----------------------------

    /// App-mode webhook match (Webhook/Github.php `normal`): applications wired
    /// to `source_id` with the provider repository id `repository_project_id`,
    /// on the branch `git_branch`, whose source is a private GitHub App.
    pub async fn list_by_source_repo_branch(
        &self,
        source_id: i64,
        repository_project_id: i64,
        git_branch: &str,
    ) -> DbResult<Vec<Application>> {
        let rows = sqlx::query_as::<_, Application>(&format!(
            "SELECT {COLS} FROM applications
              WHERE source_id = $1
                AND repository_project_id = $2
                AND git_branch = $3"
        ))
        .bind(source_id)
        .bind(repository_project_id)
        .bind(git_branch)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Manual-mode webhook candidates: all applications on `git_branch` (the
    /// caller narrows by canonical `owner/repo` match + manual secret).
    pub async fn list_by_branch(&self, git_branch: &str) -> DbResult<Vec<Application>> {
        let rows = sqlx::query_as::<_, Application>(&format!(
            "SELECT {COLS} FROM applications WHERE git_branch = $1"
        ))
        .bind(git_branch)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Resolve the destination server id for an application (webhook enqueue
    /// needs it to create the queued deployment).
    pub async fn server_id(&self, application_id: i64) -> DbResult<Option<i64>> {
        let id: Option<i64> = sqlx::query_scalar(
            "SELECT d.server_id FROM applications a
               JOIN destinations d ON d.id = a.destination_id
              WHERE a.id = $1",
        )
        .bind(application_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(id)
    }

    /// Decrypt a per-provider manual webhook secret (`github`/`gitlab`/`gitea`/
    /// `bitbucket`), or `None` when unset. Never logged by callers.
    pub async fn decrypt_manual_webhook_secret(
        &self,
        id: i64,
        provider: &str,
    ) -> DbResult<Option<String>> {
        let column = match provider {
            "github" => "manual_webhook_secret_github_enc",
            "gitlab" => "manual_webhook_secret_gitlab_enc",
            "gitea" => "manual_webhook_secret_gitea_enc",
            "bitbucket" => "manual_webhook_secret_bitbucket_enc",
            _ => return Ok(None),
        };
        let enc: Option<Vec<u8>> =
            sqlx::query_scalar(&format!("SELECT {column} FROM applications WHERE id = $1"))
                .bind(id)
                .fetch_optional(&self.pool)
                .await?
                .flatten();
        match enc {
            Some(bytes) => {
                let plain = crypto::decrypt(&bytes)?;
                Ok(Some(String::from_utf8(plain).map_err(|_| DbError::Utf8)?))
            }
            None => Ok(None),
        }
    }
}
