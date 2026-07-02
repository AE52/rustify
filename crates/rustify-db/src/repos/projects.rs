//! Projects aggregate + their environments. Creating a project auto-creates a
//! `production` environment (contract C5).

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use rustify_core::ids;

use crate::DbResult;

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct Project {
    pub id: i64,
    pub uuid: String,
    pub team_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct Environment {
    pub id: i64,
    pub uuid: String,
    pub project_id: i64,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

const PROJECT_COLS: &str = "id, uuid, team_id, name, description, created_at, updated_at";
const ENV_COLS: &str = "id, uuid, project_id, name, created_at";

#[derive(Clone)]
pub struct ProjectRepo {
    pool: PgPool,
}

impl ProjectRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Create a project and its default `production` environment atomically.
    pub async fn create(
        &self,
        team_id: i64,
        name: &str,
        description: Option<&str>,
    ) -> DbResult<Project> {
        let mut tx = self.pool.begin().await?;
        let project = sqlx::query_as::<_, Project>(&format!(
            "INSERT INTO projects (uuid, team_id, name, description)
             VALUES ($1, $2, $3, $4) RETURNING {PROJECT_COLS}"
        ))
        .bind(ids::new_uuid())
        .bind(team_id)
        .bind(name)
        .bind(description)
        .fetch_one(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO environments (uuid, project_id, name) VALUES ($1, $2, 'production')",
        )
        .bind(ids::new_uuid())
        .bind(project.id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(project)
    }

    pub async fn get_by_uuid(&self, uuid: &str) -> DbResult<Option<Project>> {
        let row = sqlx::query_as::<_, Project>(&format!(
            "SELECT {PROJECT_COLS} FROM projects WHERE uuid = $1"
        ))
        .bind(uuid)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn list(&self, team_id: i64) -> DbResult<Vec<Project>> {
        let rows = sqlx::query_as::<_, Project>(&format!(
            "SELECT {PROJECT_COLS} FROM projects WHERE team_id = $1 ORDER BY id"
        ))
        .bind(team_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn create_environment(&self, project_id: i64, name: &str) -> DbResult<Environment> {
        let row = sqlx::query_as::<_, Environment>(&format!(
            "INSERT INTO environments (uuid, project_id, name) VALUES ($1, $2, $3) RETURNING {ENV_COLS}"
        ))
        .bind(ids::new_uuid())
        .bind(project_id)
        .bind(name)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn environments(&self, project_id: i64) -> DbResult<Vec<Environment>> {
        let rows = sqlx::query_as::<_, Environment>(&format!(
            "SELECT {ENV_COLS} FROM environments WHERE project_id = $1 ORDER BY id"
        ))
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Look up an environment by (project, name) — the address form used when
    /// creating applications (contract C5 create body).
    pub async fn environment_by_name(
        &self,
        project_id: i64,
        name: &str,
    ) -> DbResult<Option<Environment>> {
        let row = sqlx::query_as::<_, Environment>(&format!(
            "SELECT {ENV_COLS} FROM environments WHERE project_id = $1 AND name = $2"
        ))
        .bind(project_id)
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn delete(&self, uuid: &str) -> DbResult<bool> {
        let result = sqlx::query("DELETE FROM projects WHERE uuid = $1")
            .bind(uuid)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }
}
