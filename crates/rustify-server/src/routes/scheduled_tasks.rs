//! Scheduled-task routes (contract C5): per-resource create/list, task
//! CRUD, manual trigger, and execution history. A task belongs to exactly one
//! application or service; team ownership is enforced through the resource's
//! environment → project → team chain.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

use rustify_db::repos::{
    Application, ApplicationRepo, NewScheduledTask, ProjectRepo, ScheduledTask,
    ScheduledTaskExecution, ScheduledTaskPatch, ScheduledTaskRepo, Service, ServiceRepo,
};
use rustify_deploy::SCHEDULED_TASK_KIND;

use crate::app::AppState;
use crate::auth::{CurrentTeam, RequireAdmin};
use crate::error::{ApiError, ApiResult};

// ----- DTOs ---------------------------------------------------------------

#[derive(Debug, Serialize, ToSchema)]
pub struct ScheduledTaskDto {
    pub uuid: String,
    pub enabled: bool,
    pub name: String,
    pub command: String,
    pub frequency: String,
    pub container: Option<String>,
    pub timeout: i32,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ScheduledTaskExecutionDto {
    pub uuid: String,
    pub status: String,
    pub message: Option<String>,
    pub error_details: Option<String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    pub duration: Option<i32>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ScheduledTaskCreate {
    pub name: String,
    pub command: String,
    pub frequency: String,
    pub container: Option<String>,
    pub timeout: Option<i32>,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct ScheduledTaskUpdate {
    pub enabled: Option<bool>,
    pub name: Option<String>,
    pub command: Option<String>,
    pub frequency: Option<String>,
    pub container: Option<String>,
    pub timeout: Option<i32>,
}

fn to_dto(t: ScheduledTask) -> ScheduledTaskDto {
    ScheduledTaskDto {
        uuid: t.uuid,
        enabled: t.enabled,
        name: t.name,
        command: t.command,
        frequency: t.frequency,
        container: t.container,
        timeout: t.timeout,
        created_at: t.created_at,
        updated_at: t.updated_at,
    }
}

fn exec_dto(e: ScheduledTaskExecution) -> ScheduledTaskExecutionDto {
    ScheduledTaskExecutionDto {
        uuid: e.uuid,
        status: e.status,
        message: e.message,
        error_details: e.error_details,
        started_at: e.started_at,
        finished_at: e.finished_at,
        duration: e.duration,
    }
}

// ----- Ownership resolution ----------------------------------------------

/// Resolve an application by uuid, enforcing team ownership via
/// environment → project → team.
async fn resolve_application(
    state: &AppState,
    team: &CurrentTeam,
    uuid: &str,
) -> ApiResult<Application> {
    let app = ApplicationRepo::new(state.pool.clone())
        .get_by_uuid(uuid)
        .await?
        .ok_or(ApiError::NotFound)?;
    if owning_team(state, app.environment_id).await? == team.id {
        Ok(app)
    } else {
        Err(ApiError::NotFound)
    }
}

/// Resolve a service by uuid, enforcing team ownership.
async fn resolve_service(state: &AppState, team: &CurrentTeam, uuid: &str) -> ApiResult<Service> {
    let service = ServiceRepo::new(state.pool.clone())
        .get_by_uuid(uuid)
        .await?
        .ok_or(ApiError::NotFound)?;
    if owning_team(state, service.environment_id).await? == team.id {
        Ok(service)
    } else {
        Err(ApiError::NotFound)
    }
}

/// Team id that owns an environment (environment → project → team).
async fn owning_team(state: &AppState, environment_id: i64) -> ApiResult<i64> {
    let projects = ProjectRepo::new(state.pool.clone());
    let environment = projects
        .environment_by_id(environment_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let project = projects
        .get_by_id(environment.project_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(project.team_id)
}

/// Resolve a task by uuid, enforcing team ownership through its resource.
async fn resolve_task(
    state: &AppState,
    team: &CurrentTeam,
    uuid: &str,
) -> ApiResult<ScheduledTask> {
    let task = ScheduledTaskRepo::new(state.pool.clone())
        .get_by_uuid(uuid)
        .await?
        .ok_or(ApiError::NotFound)?;
    let environment_id = if let Some(app_id) = task.application_id {
        ApplicationRepo::new(state.pool.clone())
            .get_by_id(app_id)
            .await?
            .map(|a| a.environment_id)
    } else if let Some(service_id) = task.service_id {
        ServiceRepo::new(state.pool.clone())
            .get_by_id(service_id)
            .await?
            .map(|s| s.environment_id)
    } else {
        None
    };
    let environment_id = environment_id.ok_or(ApiError::NotFound)?;
    if owning_team(state, environment_id).await? == team.id {
        Ok(task)
    } else {
        Err(ApiError::NotFound)
    }
}

// ----- Per-resource create/list ------------------------------------------

#[utoipa::path(get, path = "/applications/{uuid}/scheduled-tasks",
    operation_id = "list_application_scheduled_tasks", tag = "scheduled-tasks",
    params(("uuid" = String, Path, description = "Application uuid")),
    responses((status = 200, description = "Tasks", body = [ScheduledTaskDto])))]
pub async fn list_for_application(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Json<Vec<ScheduledTaskDto>>> {
    let app = resolve_application(&state, &team, &uuid).await?;
    let tasks = ScheduledTaskRepo::new(state.pool.clone())
        .list_by_application(app.id)
        .await?;
    Ok(Json(tasks.into_iter().map(to_dto).collect()))
}

#[utoipa::path(post, path = "/applications/{uuid}/scheduled-tasks",
    operation_id = "create_application_scheduled_task", tag = "scheduled-tasks",
    params(("uuid" = String, Path, description = "Application uuid")),
    request_body = ScheduledTaskCreate,
    responses((status = 201, description = "Created", body = ScheduledTaskDto)))]
pub async fn create_for_application(
    State(state): State<AppState>,
    team: CurrentTeam,
    _guard: RequireAdmin,
    Path(uuid): Path<String>,
    Json(body): Json<ScheduledTaskCreate>,
) -> ApiResult<Response> {
    let app = resolve_application(&state, &team, &uuid).await?;
    let task = ScheduledTaskRepo::new(state.pool.clone())
        .create(new_from(body, team.id, Some(app.id), None)?)
        .await?;
    Ok((StatusCode::CREATED, Json(to_dto(task))).into_response())
}

#[utoipa::path(get, path = "/services/{uuid}/scheduled-tasks",
    operation_id = "list_service_scheduled_tasks", tag = "scheduled-tasks",
    params(("uuid" = String, Path, description = "Service uuid")),
    responses((status = 200, description = "Tasks", body = [ScheduledTaskDto])))]
pub async fn list_for_service(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Json<Vec<ScheduledTaskDto>>> {
    let service = resolve_service(&state, &team, &uuid).await?;
    let tasks = ScheduledTaskRepo::new(state.pool.clone())
        .list_by_service(service.id)
        .await?;
    Ok(Json(tasks.into_iter().map(to_dto).collect()))
}

#[utoipa::path(post, path = "/services/{uuid}/scheduled-tasks",
    operation_id = "create_service_scheduled_task", tag = "scheduled-tasks",
    params(("uuid" = String, Path, description = "Service uuid")),
    request_body = ScheduledTaskCreate,
    responses((status = 201, description = "Created", body = ScheduledTaskDto)))]
pub async fn create_for_service(
    State(state): State<AppState>,
    team: CurrentTeam,
    _guard: RequireAdmin,
    Path(uuid): Path<String>,
    Json(body): Json<ScheduledTaskCreate>,
) -> ApiResult<Response> {
    let service = resolve_service(&state, &team, &uuid).await?;
    let task = ScheduledTaskRepo::new(state.pool.clone())
        .create(new_from(body, team.id, None, Some(service.id))?)
        .await?;
    Ok((StatusCode::CREATED, Json(to_dto(task))).into_response())
}

/// Validate the create body and build the repo insert struct.
fn new_from(
    body: ScheduledTaskCreate,
    team_id: i64,
    application_id: Option<i64>,
    service_id: Option<i64>,
) -> ApiResult<NewScheduledTask> {
    if body.name.trim().is_empty() {
        return Err(ApiError::Validation("name is required".into()));
    }
    if body.command.trim().is_empty() {
        return Err(ApiError::Validation("command is required".into()));
    }
    if body.frequency.trim().is_empty() {
        return Err(ApiError::Validation("frequency is required".into()));
    }
    Ok(NewScheduledTask {
        name: body.name,
        command: body.command,
        frequency: body.frequency,
        container: body.container,
        timeout: body.timeout,
        team_id: Some(team_id),
        application_id,
        service_id,
    })
}

// ----- Task CRUD ----------------------------------------------------------

#[utoipa::path(get, path = "/scheduled-tasks/{uuid}", operation_id = "get_scheduled_task",
    tag = "scheduled-tasks",
    params(("uuid" = String, Path, description = "Task uuid")),
    responses(
        (status = 200, description = "The task", body = ScheduledTaskDto),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
    ))]
pub async fn get(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Json<ScheduledTaskDto>> {
    let task = resolve_task(&state, &team, &uuid).await?;
    Ok(Json(to_dto(task)))
}

#[utoipa::path(patch, path = "/scheduled-tasks/{uuid}", operation_id = "update_scheduled_task",
    tag = "scheduled-tasks",
    params(("uuid" = String, Path, description = "Task uuid")),
    request_body = ScheduledTaskUpdate,
    responses(
        (status = 200, description = "Updated", body = ScheduledTaskDto),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
    ))]
pub async fn update(
    State(state): State<AppState>,
    team: CurrentTeam,
    _guard: RequireAdmin,
    Path(uuid): Path<String>,
    Json(body): Json<ScheduledTaskUpdate>,
) -> ApiResult<Json<ScheduledTaskDto>> {
    resolve_task(&state, &team, &uuid).await?;
    let patch = ScheduledTaskPatch {
        enabled: body.enabled,
        name: body.name,
        command: body.command,
        frequency: body.frequency,
        container: body.container,
        timeout: body.timeout,
    };
    let task = ScheduledTaskRepo::new(state.pool.clone())
        .update(&uuid, &patch)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(to_dto(task)))
}

#[utoipa::path(delete, path = "/scheduled-tasks/{uuid}", operation_id = "delete_scheduled_task",
    tag = "scheduled-tasks",
    params(("uuid" = String, Path, description = "Task uuid")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
    ))]
pub async fn delete(
    State(state): State<AppState>,
    team: CurrentTeam,
    _guard: RequireAdmin,
    Path(uuid): Path<String>,
) -> ApiResult<StatusCode> {
    resolve_task(&state, &team, &uuid).await?;
    if ScheduledTaskRepo::new(state.pool.clone())
        .delete(&uuid)
        .await?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

// ----- Trigger + executions ----------------------------------------------

#[utoipa::path(post, path = "/scheduled-tasks/{uuid}/trigger", tag = "scheduled-tasks",
    params(("uuid" = String, Path, description = "Task uuid")),
    responses((status = 202, description = "Run enqueued")))]
pub async fn trigger(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Response> {
    let task = resolve_task(&state, &team, &uuid).await?;
    let execution = ScheduledTaskRepo::new(state.pool.clone())
        .create_execution(task.id)
        .await?;
    state
        .queue
        .enqueue(
            SCHEDULED_TASK_KIND,
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

#[utoipa::path(get, path = "/scheduled-tasks/{uuid}/executions",
    operation_id = "list_scheduled_task_executions", tag = "scheduled-tasks",
    params(("uuid" = String, Path, description = "Task uuid")),
    responses((status = 200, description = "Executions", body = [ScheduledTaskExecutionDto])))]
pub async fn executions(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Json<Vec<ScheduledTaskExecutionDto>>> {
    let task = resolve_task(&state, &team, &uuid).await?;
    let list = ScheduledTaskRepo::new(state.pool.clone())
        .executions(task.id, 100)
        .await?;
    Ok(Json(list.into_iter().map(exec_dto).collect()))
}
