//! User scheduled tasks (migration `0005_scheduled_tasks.sql`) and their
//! execution history. A task targets exactly one resource — an application or a
//! service — enforced by the table CHECK constraint. Behavioural port of
//! Coolify's `ScheduledTask` / `ScheduledTaskExecution` models.

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use rustify_core::ids;

use crate::DbResult;

/// A full row of `scheduled_tasks`.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct ScheduledTask {
    pub id: i64,
    pub uuid: String,
    pub enabled: bool,
    pub name: String,
    pub command: String,
    pub frequency: String,
    pub container: Option<String>,
    pub timeout: i32,
    pub team_id: Option<i64>,
    pub application_id: Option<i64>,
    pub service_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A full row of `scheduled_task_executions`.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct ScheduledTaskExecution {
    pub id: i64,
    pub uuid: String,
    pub scheduled_task_id: i64,
    pub status: String,
    pub message: Option<String>,
    pub error_details: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub duration: Option<i32>,
}

/// Fields accepted when creating a task. Exactly one of `application_id` /
/// `service_id` should be set (the DB CHECK rejects both-null).
#[derive(Debug, Clone)]
pub struct NewScheduledTask {
    pub name: String,
    pub command: String,
    pub frequency: String,
    pub container: Option<String>,
    pub timeout: Option<i32>,
    pub team_id: Option<i64>,
    pub application_id: Option<i64>,
    pub service_id: Option<i64>,
}

/// Partial update for `PATCH /scheduled-tasks/{uuid}`. `None` leaves the column
/// unchanged (COALESCE), so nullable columns cannot be cleared to NULL here.
#[derive(Debug, Clone, Default)]
pub struct ScheduledTaskPatch {
    pub enabled: Option<bool>,
    pub name: Option<String>,
    pub command: Option<String>,
    pub frequency: Option<String>,
    pub container: Option<String>,
    pub timeout: Option<i32>,
}

const COLS: &str = "id, uuid, enabled, name, command, frequency, container, timeout, \
     team_id, application_id, service_id, created_at, updated_at";
const EXEC_COLS: &str = "id, uuid, scheduled_task_id, status, message, error_details, \
     started_at, finished_at, duration";

#[derive(Clone)]
pub struct ScheduledTaskRepo {
    pool: PgPool,
}

impl ScheduledTaskRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(&self, new: NewScheduledTask) -> DbResult<ScheduledTask> {
        let uuid = ids::new_uuid();
        let row = sqlx::query_as::<_, ScheduledTask>(&format!(
            "INSERT INTO scheduled_tasks
               (uuid, name, command, frequency, container, timeout, team_id,
                application_id, service_id)
             VALUES ($1, $2, $3, $4, $5, COALESCE($6, 300), $7, $8, $9)
             RETURNING {COLS}"
        ))
        .bind(&uuid)
        .bind(&new.name)
        .bind(&new.command)
        .bind(&new.frequency)
        .bind(&new.container)
        .bind(new.timeout)
        .bind(new.team_id)
        .bind(new.application_id)
        .bind(new.service_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn get_by_uuid(&self, uuid: &str) -> DbResult<Option<ScheduledTask>> {
        let row = sqlx::query_as::<_, ScheduledTask>(&format!(
            "SELECT {COLS} FROM scheduled_tasks WHERE uuid = $1"
        ))
        .bind(uuid)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn get_by_id(&self, id: i64) -> DbResult<Option<ScheduledTask>> {
        let row = sqlx::query_as::<_, ScheduledTask>(&format!(
            "SELECT {COLS} FROM scheduled_tasks WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn list_by_application(&self, application_id: i64) -> DbResult<Vec<ScheduledTask>> {
        let rows = sqlx::query_as::<_, ScheduledTask>(&format!(
            "SELECT {COLS} FROM scheduled_tasks WHERE application_id = $1 ORDER BY id"
        ))
        .bind(application_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn list_by_service(&self, service_id: i64) -> DbResult<Vec<ScheduledTask>> {
        let rows = sqlx::query_as::<_, ScheduledTask>(&format!(
            "SELECT {COLS} FROM scheduled_tasks WHERE service_id = $1 ORDER BY id"
        ))
        .bind(service_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// All enabled tasks (the dispatcher's candidate set).
    pub async fn list_enabled(&self) -> DbResult<Vec<ScheduledTask>> {
        let rows = sqlx::query_as::<_, ScheduledTask>(&format!(
            "SELECT {COLS} FROM scheduled_tasks WHERE enabled = true ORDER BY id"
        ))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn update(
        &self,
        uuid: &str,
        patch: &ScheduledTaskPatch,
    ) -> DbResult<Option<ScheduledTask>> {
        let row = sqlx::query_as::<_, ScheduledTask>(&format!(
            "UPDATE scheduled_tasks SET
                enabled = COALESCE($2, enabled),
                name = COALESCE($3, name),
                command = COALESCE($4, command),
                frequency = COALESCE($5, frequency),
                container = COALESCE($6, container),
                timeout = COALESCE($7, timeout),
                updated_at = now()
              WHERE uuid = $1
              RETURNING {COLS}"
        ))
        .bind(uuid)
        .bind(patch.enabled)
        .bind(&patch.name)
        .bind(&patch.command)
        .bind(&patch.frequency)
        .bind(&patch.container)
        .bind(patch.timeout)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn delete(&self, uuid: &str) -> DbResult<bool> {
        let result = sqlx::query("DELETE FROM scheduled_tasks WHERE uuid = $1")
            .bind(uuid)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }

    // ---- executions --------------------------------------------------------

    /// Open a new execution row (status `running`) and return it.
    pub async fn create_execution(&self, task_id: i64) -> DbResult<ScheduledTaskExecution> {
        let uuid = ids::new_uuid();
        let row = sqlx::query_as::<_, ScheduledTaskExecution>(&format!(
            "INSERT INTO scheduled_task_executions (uuid, scheduled_task_id)
             VALUES ($1, $2)
             RETURNING {EXEC_COLS}"
        ))
        .bind(&uuid)
        .bind(task_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn get_execution_by_uuid(
        &self,
        uuid: &str,
    ) -> DbResult<Option<ScheduledTaskExecution>> {
        let row = sqlx::query_as::<_, ScheduledTaskExecution>(&format!(
            "SELECT {EXEC_COLS} FROM scheduled_task_executions WHERE uuid = $1"
        ))
        .bind(uuid)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Finalise an execution with its outcome (`success`/`failed`), output
    /// message, optional error detail and wall-clock duration in seconds.
    pub async fn finish_execution(
        &self,
        id: i64,
        status: &str,
        message: Option<&str>,
        error_details: Option<&str>,
        duration: i32,
    ) -> DbResult<()> {
        sqlx::query(
            "UPDATE scheduled_task_executions
               SET status = $2, message = $3, error_details = $4,
                   finished_at = now(), duration = $5
             WHERE id = $1",
        )
        .bind(id)
        .bind(status)
        .bind(message)
        .bind(error_details)
        .bind(duration)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn executions(
        &self,
        task_id: i64,
        limit: i64,
    ) -> DbResult<Vec<ScheduledTaskExecution>> {
        let rows = sqlx::query_as::<_, ScheduledTaskExecution>(&format!(
            "SELECT {EXEC_COLS} FROM scheduled_task_executions
             WHERE scheduled_task_id = $1 ORDER BY started_at DESC, id DESC LIMIT $2"
        ))
        .bind(task_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Whether the task already has an execution that started at or after
    /// `since` — used by the dispatcher to avoid double-firing within a minute.
    pub async fn has_execution_since(&self, task_id: i64, since: DateTime<Utc>) -> DbResult<bool> {
        let exists: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM scheduled_task_executions
             WHERE scheduled_task_id = $1 AND started_at >= $2 LIMIT 1",
        )
        .bind(task_id)
        .bind(since)
        .fetch_optional(&self.pool)
        .await?;
        Ok(exists.is_some())
    }
}
