//! One-click-service routes (contract C5): CRUD plus deploy/stop/restart. A
//! service is created from a bundled template (`template_key`); its raw compose
//! is copied from the embedded manifest. Deploy/stop enqueue the
//! `service_deploy` / `service_stop` jobs handled by `rustify-deploy`.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

use rustify_db::repos::{
    NewService, ProjectRepo, ServerRepo, Service, ServiceApplication, ServiceRepo,
};

use crate::app::AppState;
use crate::auth::CurrentTeam;
use crate::error::{ApiError, ApiResult};
use crate::routes::service_templates::template_compose;

// ----- DTOs ---------------------------------------------------------------

#[derive(Debug, Serialize, ToSchema)]
pub struct ServiceApplicationDto {
    pub uuid: String,
    pub name: String,
    pub fqdn: Option<String>,
    pub image: Option<String>,
    pub status: String,
    pub is_database: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ServiceDto {
    pub uuid: String,
    pub name: String,
    pub template_key: String,
    pub status: String,
    pub environment_uuid: String,
    pub project_uuid: String,
    pub server_uuid: String,
    pub applications: Vec<ServiceApplicationDto>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ServiceCreate {
    pub project_uuid: String,
    pub environment_name: String,
    pub server_uuid: String,
    pub template_key: String,
    pub name: String,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct ServiceUpdate {
    pub name: Option<String>,
}

// ----- Resolution ---------------------------------------------------------

struct ServiceContext {
    service: Service,
    environment_uuid: String,
    project_uuid: String,
    server_uuid: String,
    applications: Vec<ServiceApplication>,
}

/// Resolve a service by uuid, enforcing team ownership via
/// environment → project → team.
async fn resolve(state: &AppState, team: &CurrentTeam, uuid: &str) -> ApiResult<ServiceContext> {
    let repo = ServiceRepo::new(state.pool.clone());
    let service = repo.get_by_uuid(uuid).await?.ok_or(ApiError::NotFound)?;

    let projects = ProjectRepo::new(state.pool.clone());
    let environment = projects
        .environment_by_id(service.environment_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let project = projects
        .get_by_id(environment.project_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    if project.team_id != team.id {
        return Err(ApiError::NotFound);
    }

    let servers = ServerRepo::new(state.pool.clone());
    let destination = servers
        .destination_by_id(service.destination_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let server = servers
        .get_by_id(destination.server_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    let applications = repo.applications(service.id).await?;

    Ok(ServiceContext {
        service,
        environment_uuid: environment.uuid,
        project_uuid: project.uuid,
        server_uuid: server.uuid,
        applications,
    })
}

fn to_dto(ctx: ServiceContext) -> ServiceDto {
    let s = ctx.service;
    ServiceDto {
        uuid: s.uuid,
        name: s.name,
        template_key: s.template_key,
        status: s.status,
        environment_uuid: ctx.environment_uuid,
        project_uuid: ctx.project_uuid,
        server_uuid: ctx.server_uuid,
        applications: ctx
            .applications
            .into_iter()
            .map(|a| ServiceApplicationDto {
                uuid: a.uuid,
                name: a.name,
                fqdn: a.fqdn,
                image: a.image,
                status: a.status,
                is_database: a.is_database,
            })
            .collect(),
        created_at: s.created_at,
        updated_at: s.updated_at,
    }
}

// ----- CRUD ---------------------------------------------------------------

#[utoipa::path(get, path = "/services", operation_id = "list_services", tag = "services",
    responses((status = 200, description = "List of services", body = [ServiceDto])))]
pub async fn list(
    State(state): State<AppState>,
    team: CurrentTeam,
) -> ApiResult<Json<Vec<ServiceDto>>> {
    let projects = ProjectRepo::new(state.pool.clone()).list(team.id).await?;
    let project_repo = ProjectRepo::new(state.pool.clone());
    let repo = ServiceRepo::new(state.pool.clone());
    let mut out = Vec::new();
    for project in projects {
        for env in project_repo.environments(project.id).await? {
            for svc in repo.list_by_environment(env.id).await? {
                let ctx = resolve(&state, &team, &svc.uuid).await?;
                out.push(to_dto(ctx));
            }
        }
    }
    Ok(Json(out))
}

#[utoipa::path(post, path = "/services", operation_id = "create_service", tag = "services",
    request_body = ServiceCreate,
    responses(
        (status = 201, description = "Service created", body = ServiceDto),
        (status = 422, description = "Validation error", body = crate::error::ApiErrorBody),
    ))]
pub async fn create(
    State(state): State<AppState>,
    team: CurrentTeam,
    Json(body): Json<ServiceCreate>,
) -> ApiResult<Response> {
    let compose_raw = template_compose(&body.template_key).ok_or_else(|| {
        ApiError::Validation(format!("unknown template_key '{}'", body.template_key))
    })?;

    let projects = ProjectRepo::new(state.pool.clone());
    let project = projects
        .get_by_uuid(&body.project_uuid)
        .await?
        .filter(|p| p.team_id == team.id)
        .ok_or_else(|| ApiError::Validation("unknown project_uuid".into()))?;
    let environment = projects
        .environment_by_name(project.id, &body.environment_name)
        .await?
        .ok_or_else(|| ApiError::Validation("unknown environment_name".into()))?;

    let servers = ServerRepo::new(state.pool.clone());
    let server = servers
        .get_by_uuid(&body.server_uuid)
        .await?
        .filter(|s| s.team_id == team.id)
        .ok_or_else(|| ApiError::Validation("unknown server_uuid".into()))?;
    let destination = servers
        .default_destination(server.id)
        .await?
        .ok_or_else(|| ApiError::Validation("server has no destination".into()))?;

    let service = ServiceRepo::new(state.pool.clone())
        .create(NewService {
            environment_id: environment.id,
            destination_id: destination.id,
            name: body.name.clone(),
            template_key: body.template_key.clone(),
            compose_raw,
        })
        .await?;

    let ctx = resolve(&state, &team, &service.uuid).await?;
    Ok((StatusCode::CREATED, Json(to_dto(ctx))).into_response())
}

#[utoipa::path(get, path = "/services/{uuid}", operation_id = "get_service", tag = "services",
    params(("uuid" = String, Path, description = "Service uuid")),
    responses(
        (status = 200, description = "The service", body = ServiceDto),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
    ))]
pub async fn get(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Json<ServiceDto>> {
    let ctx = resolve(&state, &team, &uuid).await?;
    Ok(Json(to_dto(ctx)))
}

#[utoipa::path(patch, path = "/services/{uuid}", operation_id = "update_service", tag = "services",
    params(("uuid" = String, Path, description = "Service uuid")),
    request_body = ServiceUpdate,
    responses(
        (status = 200, description = "Updated service", body = ServiceDto),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
    ))]
pub async fn update(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
    Json(body): Json<ServiceUpdate>,
) -> ApiResult<Json<ServiceDto>> {
    resolve(&state, &team, &uuid).await?;
    if let Some(name) = &body.name {
        ServiceRepo::new(state.pool.clone())
            .rename(&uuid, name)
            .await?
            .ok_or(ApiError::NotFound)?;
    }
    let ctx = resolve(&state, &team, &uuid).await?;
    Ok(Json(to_dto(ctx)))
}

#[utoipa::path(delete, path = "/services/{uuid}", operation_id = "delete_service", tag = "services",
    params(("uuid" = String, Path, description = "Service uuid")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
    ))]
pub async fn delete(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<StatusCode> {
    resolve(&state, &team, &uuid).await?;
    if ServiceRepo::new(state.pool.clone()).delete(&uuid).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

// ----- Lifecycle ----------------------------------------------------------

async fn enqueue(state: &AppState, uuid: &str, kind: &str) -> ApiResult<Response> {
    state
        .queue
        .enqueue(kind, json!({ "service_uuid": uuid }), None)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "status": "accepted" }))).into_response())
}

#[utoipa::path(post, path = "/services/{uuid}/deploy", tag = "services",
    params(("uuid" = String, Path, description = "Service uuid")),
    responses((status = 202, description = "Deploy enqueued")))]
pub async fn deploy(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Response> {
    let ctx = resolve(&state, &team, &uuid).await?;
    enqueue(&state, &ctx.service.uuid, "service_deploy").await
}

#[utoipa::path(post, path = "/services/{uuid}/stop", tag = "services",
    params(("uuid" = String, Path, description = "Service uuid")),
    responses((status = 202, description = "Stop enqueued")))]
pub async fn stop(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Response> {
    let ctx = resolve(&state, &team, &uuid).await?;
    enqueue(&state, &ctx.service.uuid, "service_stop").await
}

#[utoipa::path(post, path = "/services/{uuid}/restart", tag = "services",
    params(("uuid" = String, Path, description = "Service uuid")),
    responses((status = 202, description = "Restart enqueued")))]
pub async fn restart(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Response> {
    // `docker compose up -d --remove-orphans` recreates changed containers, so
    // a redeploy is an idempotent restart.
    let ctx = resolve(&state, &team, &uuid).await?;
    enqueue(&state, &ctx.service.uuid, "service_deploy").await
}
