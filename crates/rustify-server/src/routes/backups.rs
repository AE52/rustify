//! Scheduled database-backup routes: per-database CRUD, manual trigger and
//! execution history.
//!
//! Ownership flows through the database's environment → project → team chain
//! (mirroring the databases routes). `trigger` creates a `running` execution and
//! enqueues a `database_backup` job handled by `rustify-deploy`.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

use rustify_db::repos::{
    BackupExecution, BackupExecutionRepo, DatabaseRepo, NewScheduledBackup, ProjectRepo,
    S3StorageRepo, ScheduledBackup, ScheduledBackupPatch, ScheduledBackupRepo,
};

use crate::app::AppState;
use crate::auth::CurrentTeam;
use crate::error::{ApiError, ApiResult};

const BACKUP_JOB_KIND: &str = "database_backup";

// ----- DTOs ---------------------------------------------------------------

#[derive(Debug, Serialize, ToSchema)]
pub struct BackupDto {
    pub uuid: String,
    pub database_uuid: String,
    pub enabled: bool,
    pub frequency: String,
    pub save_s3: bool,
    pub s3_storage_uuid: Option<String>,
    pub databases_to_backup: Option<String>,
    pub dump_all: bool,
    pub disable_local_backup: bool,
    pub retention_amount_local: i32,
    pub retention_days_local: i32,
    pub retention_max_gb_local: i32,
    pub retention_amount_s3: i32,
    pub retention_days_s3: i32,
    pub retention_max_gb_s3: i32,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct BackupCreate {
    pub frequency: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub save_s3: bool,
    pub s3_storage_uuid: Option<String>,
    pub databases_to_backup: Option<String>,
    #[serde(default)]
    pub dump_all: bool,
    #[serde(default)]
    pub disable_local_backup: bool,
    #[serde(default)]
    pub retention_amount_local: i32,
    #[serde(default)]
    pub retention_days_local: i32,
    #[serde(default)]
    pub retention_max_gb_local: i32,
    #[serde(default)]
    pub retention_amount_s3: i32,
    #[serde(default)]
    pub retention_days_s3: i32,
    #[serde(default)]
    pub retention_max_gb_s3: i32,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct BackupUpdate {
    pub frequency: Option<String>,
    pub enabled: Option<bool>,
    pub save_s3: Option<bool>,
    /// Empty string detaches the S3 storage; omitted leaves it unchanged.
    pub s3_storage_uuid: Option<String>,
    pub databases_to_backup: Option<String>,
    pub dump_all: Option<bool>,
    pub disable_local_backup: Option<bool>,
    pub retention_amount_local: Option<i32>,
    pub retention_days_local: Option<i32>,
    pub retention_max_gb_local: Option<i32>,
    pub retention_amount_s3: Option<i32>,
    pub retention_days_s3: Option<i32>,
    pub retention_max_gb_s3: Option<i32>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExecutionDto {
    pub uuid: String,
    pub status: String,
    pub filename: Option<String>,
    pub size: i64,
    pub s3_uploaded: Option<bool>,
    pub message: Option<String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<BackupExecution> for ExecutionDto {
    fn from(e: BackupExecution) -> Self {
        Self {
            uuid: e.uuid,
            status: e.status,
            filename: e.filename,
            size: e.size,
            s3_uploaded: e.s3_uploaded,
            message: e.message,
            started_at: e.started_at,
            finished_at: e.finished_at,
            created_at: e.created_at,
        }
    }
}

// ----- ownership helpers --------------------------------------------------

/// Ensure a database (by uuid) belongs to the caller's team; returns its id.
async fn owned_database_id(state: &AppState, team: &CurrentTeam, uuid: &str) -> ApiResult<i64> {
    let db = DatabaseRepo::new(state.pool.clone())
        .get_by_uuid(uuid)
        .await?
        .ok_or(ApiError::NotFound)?;
    assert_db_team(state, team, db.environment_id).await?;
    Ok(db.id)
}

/// Resolve a schedule (by uuid), enforce ownership via its database, and return
/// `(backup, database_uuid)`.
async fn owned_backup(
    state: &AppState,
    team: &CurrentTeam,
    uuid: &str,
) -> ApiResult<(ScheduledBackup, String)> {
    let backup = ScheduledBackupRepo::new(state.pool.clone())
        .get_by_uuid(uuid)
        .await?
        .ok_or(ApiError::NotFound)?;
    let db = DatabaseRepo::new(state.pool.clone())
        .get_by_id(backup.database_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    assert_db_team(state, team, db.environment_id).await?;
    Ok((backup, db.uuid))
}

async fn assert_db_team(
    state: &AppState,
    team: &CurrentTeam,
    environment_id: i64,
) -> ApiResult<()> {
    let projects = ProjectRepo::new(state.pool.clone());
    let env = projects
        .environment_by_id(environment_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let project = projects
        .get_by_id(env.project_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    if project.team_id != team.id {
        return Err(ApiError::NotFound);
    }
    Ok(())
}

/// Map an S3 storage uuid to its id, enforcing team ownership.
async fn resolve_s3(state: &AppState, team: &CurrentTeam, uuid: &str) -> ApiResult<i64> {
    let s3 = S3StorageRepo::new(state.pool.clone())
        .get_by_uuid(uuid)
        .await?
        .filter(|s| s.team_id == team.id)
        .ok_or_else(|| ApiError::Validation("unknown s3_storage_uuid".into()))?;
    Ok(s3.id)
}

async fn to_dto(state: &AppState, backup: ScheduledBackup, database_uuid: String) -> BackupDto {
    let s3_storage_uuid = match backup.s3_storage_id {
        Some(id) => S3StorageRepo::new(state.pool.clone())
            .get_by_id(id)
            .await
            .ok()
            .flatten()
            .map(|s| s.uuid),
        None => None,
    };
    BackupDto {
        uuid: backup.uuid,
        database_uuid,
        enabled: backup.enabled,
        frequency: backup.frequency,
        save_s3: backup.save_s3,
        s3_storage_uuid,
        databases_to_backup: backup.databases_to_backup,
        dump_all: backup.dump_all,
        disable_local_backup: backup.disable_local_backup,
        retention_amount_local: backup.retention_amount_local,
        retention_days_local: backup.retention_days_local,
        retention_max_gb_local: backup.retention_max_gb_local,
        retention_amount_s3: backup.retention_amount_s3,
        retention_days_s3: backup.retention_days_s3,
        retention_max_gb_s3: backup.retention_max_gb_s3,
        created_at: backup.created_at,
        updated_at: backup.updated_at,
    }
}

// ----- routes -------------------------------------------------------------

#[utoipa::path(get, path = "/databases/{uuid}/backups", operation_id = "list_backups", tag = "backups",
    params(("uuid" = String, Path, description = "Database uuid")),
    responses((status = 200, description = "Schedules for the database", body = [BackupDto])))]
pub async fn list(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Json<Vec<BackupDto>>> {
    let db_id = owned_database_id(&state, &team, &uuid).await?;
    let rows = ScheduledBackupRepo::new(state.pool.clone())
        .list_by_database(db_id)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for b in rows {
        out.push(to_dto(&state, b, uuid.clone()).await);
    }
    Ok(Json(out))
}

#[utoipa::path(post, path = "/databases/{uuid}/backups", operation_id = "create_backup", tag = "backups",
    params(("uuid" = String, Path, description = "Database uuid")),
    request_body = BackupCreate,
    responses((status = 201, description = "Created", body = BackupDto),
        (status = 422, description = "Validation error", body = crate::error::ApiErrorBody)))]
pub async fn create(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
    Json(body): Json<BackupCreate>,
) -> ApiResult<Response> {
    let db_id = owned_database_id(&state, &team, &uuid).await?;
    if body.frequency.trim().is_empty() {
        return Err(ApiError::Validation("frequency is required".into()));
    }
    let s3_storage_id = match &body.s3_storage_uuid {
        Some(u) if !u.is_empty() => Some(resolve_s3(&state, &team, u).await?),
        _ => None,
    };
    if body.save_s3 && s3_storage_id.is_none() {
        return Err(ApiError::Validation(
            "save_s3 requires an s3_storage_uuid".into(),
        ));
    }
    let backup = ScheduledBackupRepo::new(state.pool.clone())
        .create(NewScheduledBackup {
            database_id: db_id,
            frequency: body.frequency,
            enabled: body.enabled,
            save_s3: body.save_s3,
            s3_storage_id,
            databases_to_backup: body.databases_to_backup,
            dump_all: body.dump_all,
            disable_local_backup: body.disable_local_backup,
            retention_amount_local: body.retention_amount_local,
            retention_days_local: body.retention_days_local,
            retention_max_gb_local: body.retention_max_gb_local,
            retention_amount_s3: body.retention_amount_s3,
            retention_days_s3: body.retention_days_s3,
            retention_max_gb_s3: body.retention_max_gb_s3,
        })
        .await?;
    let dto = to_dto(&state, backup, uuid).await;
    Ok((StatusCode::CREATED, Json(dto)).into_response())
}

#[utoipa::path(get, path = "/backups/{uuid}", operation_id = "get_backup", tag = "backups",
    params(("uuid" = String, Path, description = "Backup uuid")),
    responses((status = 200, description = "The schedule", body = BackupDto),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody)))]
pub async fn get(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Json<BackupDto>> {
    let (backup, db_uuid) = owned_backup(&state, &team, &uuid).await?;
    Ok(Json(to_dto(&state, backup, db_uuid).await))
}

#[utoipa::path(patch, path = "/backups/{uuid}", operation_id = "update_backup", tag = "backups",
    params(("uuid" = String, Path, description = "Backup uuid")),
    request_body = BackupUpdate,
    responses((status = 200, description = "Updated", body = BackupDto),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody)))]
pub async fn update(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
    Json(body): Json<BackupUpdate>,
) -> ApiResult<Json<BackupDto>> {
    let (_, db_uuid) = owned_backup(&state, &team, &uuid).await?;
    // s3_storage_uuid: absent = leave; "" = detach; value = resolve.
    let s3_storage_id = match &body.s3_storage_uuid {
        None => None,
        Some(u) if u.is_empty() => Some(None),
        Some(u) => Some(Some(resolve_s3(&state, &team, u).await?)),
    };
    let updated = ScheduledBackupRepo::new(state.pool.clone())
        .update(
            &uuid,
            &ScheduledBackupPatch {
                frequency: body.frequency,
                enabled: body.enabled,
                save_s3: body.save_s3,
                s3_storage_id,
                databases_to_backup: body.databases_to_backup,
                dump_all: body.dump_all,
                disable_local_backup: body.disable_local_backup,
                retention_amount_local: body.retention_amount_local,
                retention_days_local: body.retention_days_local,
                retention_max_gb_local: body.retention_max_gb_local,
                retention_amount_s3: body.retention_amount_s3,
                retention_days_s3: body.retention_days_s3,
                retention_max_gb_s3: body.retention_max_gb_s3,
            },
        )
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(to_dto(&state, updated, db_uuid).await))
}

#[utoipa::path(delete, path = "/backups/{uuid}", operation_id = "delete_backup", tag = "backups",
    params(("uuid" = String, Path, description = "Backup uuid")),
    responses((status = 204, description = "Deleted"),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody)))]
pub async fn delete(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<StatusCode> {
    owned_backup(&state, &team, &uuid).await?;
    if ScheduledBackupRepo::new(state.pool.clone())
        .delete(&uuid)
        .await?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

#[utoipa::path(post, path = "/backups/{uuid}/trigger", operation_id = "trigger_backup", tag = "backups",
    params(("uuid" = String, Path, description = "Backup uuid")),
    responses((status = 202, description = "Backup enqueued")))]
pub async fn trigger(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Response> {
    let (backup, _) = owned_backup(&state, &team, &uuid).await?;
    let execution = BackupExecutionRepo::new(state.pool.clone())
        .create_running(backup.id)
        .await?;
    state
        .queue
        .enqueue(
            BACKUP_JOB_KIND,
            json!({ "execution_uuid": execution.uuid }),
            None,
        )
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "status": "accepted", "execution_uuid": execution.uuid })),
    )
        .into_response())
}

#[utoipa::path(get, path = "/backups/{uuid}/executions", operation_id = "list_backup_executions", tag = "backups",
    params(("uuid" = String, Path, description = "Backup uuid")),
    responses((status = 200, description = "Execution history", body = [ExecutionDto])))]
pub async fn executions(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Json<Vec<ExecutionDto>>> {
    let (backup, _) = owned_backup(&state, &team, &uuid).await?;
    let rows = BackupExecutionRepo::new(state.pool.clone())
        .list_by_backup(backup.id)
        .await?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}
