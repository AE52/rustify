//! Projects + environments routes (contract C5). Creating a project
//! auto-creates a `production` environment (handled by `rustify-db`).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use rustify_db::repos::{Environment, Project, ProjectRepo};

use crate::app::AppState;
use crate::auth::{CurrentTeam, RequireAdmin};
use crate::error::{ApiError, ApiResult};

#[derive(Debug, Serialize, ToSchema)]
pub struct EnvironmentDto {
    pub uuid: String,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<Environment> for EnvironmentDto {
    fn from(e: Environment) -> Self {
        Self {
            uuid: e.uuid,
            name: e.name,
            created_at: e.created_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectDto {
    pub uuid: String,
    pub name: String,
    pub description: Option<String>,
    pub environments: Vec<EnvironmentDto>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ProjectCreate {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ProjectUpdate {
    pub name: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct EnvironmentCreate {
    pub name: String,
}

async fn project_dto(state: &AppState, project: Project) -> ApiResult<ProjectDto> {
    let envs = ProjectRepo::new(state.pool.clone())
        .environments(project.id)
        .await?;
    Ok(ProjectDto {
        uuid: project.uuid,
        name: project.name,
        description: project.description,
        environments: envs.into_iter().map(EnvironmentDto::from).collect(),
        created_at: project.created_at,
        updated_at: project.updated_at,
    })
}

async fn owned(state: &AppState, team: &CurrentTeam, uuid: &str) -> ApiResult<Project> {
    let project = ProjectRepo::new(state.pool.clone())
        .get_by_uuid(uuid)
        .await?
        .ok_or(ApiError::NotFound)?;
    if project.team_id != team.id {
        return Err(ApiError::NotFound);
    }
    Ok(project)
}

#[utoipa::path(get, path = "/projects", operation_id = "list_projects", tag = "projects",
    responses((status = 200, description = "List of projects", body = [ProjectDto])))]
pub async fn list(
    State(state): State<AppState>,
    team: CurrentTeam,
) -> ApiResult<Json<Vec<ProjectDto>>> {
    let projects = ProjectRepo::new(state.pool.clone()).list(team.id).await?;
    let mut out = Vec::with_capacity(projects.len());
    for p in projects {
        out.push(project_dto(&state, p).await?);
    }
    Ok(Json(out))
}

#[utoipa::path(post, path = "/projects", operation_id = "create_project", tag = "projects", request_body = ProjectCreate,
    responses((status = 201, description = "Project created", body = ProjectDto)))]
pub async fn create(
    State(state): State<AppState>,
    team: CurrentTeam,
    _guard: RequireAdmin,
    Json(body): Json<ProjectCreate>,
) -> ApiResult<Response> {
    let project = ProjectRepo::new(state.pool.clone())
        .create(team.id, &body.name, body.description.as_deref())
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(project_dto(&state, project).await?),
    )
        .into_response())
}

#[utoipa::path(get, path = "/projects/{uuid}", operation_id = "get_project", tag = "projects",
    params(("uuid" = String, Path, description = "Project uuid")),
    responses(
        (status = 200, description = "The project", body = ProjectDto),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
    ))]
pub async fn get(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Json<ProjectDto>> {
    let project = owned(&state, &team, &uuid).await?;
    Ok(Json(project_dto(&state, project).await?))
}

#[utoipa::path(patch, path = "/projects/{uuid}", operation_id = "update_project", tag = "projects",
    params(("uuid" = String, Path, description = "Project uuid")),
    request_body = ProjectUpdate,
    responses(
        (status = 200, description = "Updated project", body = ProjectDto),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
    ))]
pub async fn update(
    State(state): State<AppState>,
    team: CurrentTeam,
    _guard: RequireAdmin,
    Path(uuid): Path<String>,
    Json(body): Json<ProjectUpdate>,
) -> ApiResult<Json<ProjectDto>> {
    owned(&state, &team, &uuid).await?;
    let project = ProjectRepo::new(state.pool.clone())
        .update(&uuid, body.name.as_deref(), body.description.as_deref())
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(project_dto(&state, project).await?))
}

#[utoipa::path(delete, path = "/projects/{uuid}", operation_id = "delete_project", tag = "projects",
    params(("uuid" = String, Path, description = "Project uuid")),
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
    owned(&state, &team, &uuid).await?;
    if ProjectRepo::new(state.pool.clone()).delete(&uuid).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

#[utoipa::path(get, path = "/projects/{uuid}/environments", tag = "projects",
    params(("uuid" = String, Path, description = "Project uuid")),
    responses((status = 200, description = "Environments", body = [EnvironmentDto])))]
pub async fn list_environments(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Json<Vec<EnvironmentDto>>> {
    let project = owned(&state, &team, &uuid).await?;
    let envs = ProjectRepo::new(state.pool.clone())
        .environments(project.id)
        .await?;
    Ok(Json(envs.into_iter().map(EnvironmentDto::from).collect()))
}

#[utoipa::path(post, path = "/projects/{uuid}/environments", tag = "projects",
    params(("uuid" = String, Path, description = "Project uuid")),
    request_body = EnvironmentCreate,
    responses((status = 201, description = "Environment created", body = EnvironmentDto)))]
pub async fn create_environment(
    State(state): State<AppState>,
    team: CurrentTeam,
    _guard: RequireAdmin,
    Path(uuid): Path<String>,
    Json(body): Json<EnvironmentCreate>,
) -> ApiResult<Response> {
    let project = owned(&state, &team, &uuid).await?;
    let env = ProjectRepo::new(state.pool.clone())
        .create_environment(project.id, &body.name)
        .await?;
    Ok((StatusCode::CREATED, Json(EnvironmentDto::from(env))).into_response())
}
