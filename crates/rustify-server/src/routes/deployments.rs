//! Deployments routes (contract C5): list per application, fetch detail with
//! logs (contract C3), and cancel.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use rustify_core::{DeploymentStatus, LogLine};
use rustify_db::repos::{ApplicationRepo, Deployment, DeploymentRepo, ProjectRepo, ServerRepo};

use crate::app::AppState;
use crate::auth::CurrentTeam;
use crate::error::{ApiError, ApiResult};

#[derive(Debug, Serialize, ToSchema)]
pub struct DeploymentDto {
    pub uuid: String,
    pub application_uuid: String,
    pub server_uuid: String,
    /// One of queued/in_progress/finished/failed/cancelled.
    pub status: String,
    pub commit_sha: Option<String>,
    pub commit_message: Option<String>,
    pub force_rebuild: bool,
    pub rollback: bool,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LogLineDto {
    pub order: i64,
    pub kind: String,
    pub content: String,
    pub hidden: bool,
    pub batch: i32,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl From<LogLine> for LogLineDto {
    fn from(l: LogLine) -> Self {
        Self {
            order: l.order,
            kind: l.kind,
            content: l.content,
            hidden: l.hidden,
            batch: l.batch,
            timestamp: l.timestamp,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DeploymentDetailDto {
    #[serde(flatten)]
    pub deployment: DeploymentDto,
    pub logs: Vec<LogLineDto>,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub application_uuid: Option<String>,
}

fn status_str(status: DeploymentStatus) -> String {
    match status {
        DeploymentStatus::Queued => "queued",
        DeploymentStatus::InProgress => "in_progress",
        DeploymentStatus::Finished => "finished",
        DeploymentStatus::Failed => "failed",
        DeploymentStatus::Cancelled => "cancelled",
    }
    .to_string()
}

/// A deployment with the uuids of its application and server resolved, plus a
/// team-ownership guarantee.
struct Resolved {
    deployment: Deployment,
    application_uuid: String,
    server_uuid: String,
}

async fn resolve(
    state: &AppState,
    team: &CurrentTeam,
    deployment: Deployment,
) -> ApiResult<Resolved> {
    let app = ApplicationRepo::new(state.pool.clone())
        .get_by_id(deployment.application_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    // Verify team ownership through environment → project.
    let projects = ProjectRepo::new(state.pool.clone());
    let environment = projects
        .environment_by_id(app.environment_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let project = projects
        .get_by_id(environment.project_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    if project.team_id != team.id {
        return Err(ApiError::NotFound);
    }
    let server = ServerRepo::new(state.pool.clone())
        .get_by_id(deployment.server_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Resolved {
        deployment,
        application_uuid: app.uuid,
        server_uuid: server.uuid,
    })
}

fn to_dto(r: Resolved) -> DeploymentDto {
    let d = r.deployment;
    DeploymentDto {
        uuid: d.uuid,
        application_uuid: r.application_uuid,
        server_uuid: r.server_uuid,
        status: status_str(d.status),
        commit_sha: d.commit_sha,
        commit_message: d.commit_message,
        force_rebuild: d.force_rebuild,
        rollback: d.rollback,
        started_at: d.started_at,
        finished_at: d.finished_at,
        created_at: d.created_at,
    }
}

#[utoipa::path(get, path = "/deployments", operation_id = "list_deployments", tag = "deployments",
    params(("application_uuid" = String, Query, description = "Filter by application uuid")),
    responses(
        (status = 200, description = "Deployments for the application", body = [DeploymentDto]),
        (status = 422, description = "Missing application_uuid", body = crate::error::ApiErrorBody),
    ))]
pub async fn list(
    State(state): State<AppState>,
    team: CurrentTeam,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Vec<DeploymentDto>>> {
    let application_uuid = q
        .application_uuid
        .ok_or_else(|| ApiError::Validation("application_uuid query param is required".into()))?;

    let app = ApplicationRepo::new(state.pool.clone())
        .get_by_uuid(&application_uuid)
        .await?
        .ok_or(ApiError::NotFound)?;

    let deployments = DeploymentRepo::new(state.pool.clone())
        .list_by_application(app.id)
        .await?;

    let mut out = Vec::with_capacity(deployments.len());
    for d in deployments {
        let resolved = resolve(&state, &team, d).await?;
        out.push(to_dto(resolved));
    }
    Ok(Json(out))
}

#[utoipa::path(get, path = "/deployments/{uuid}", operation_id = "get_deployment", tag = "deployments",
    params(("uuid" = String, Path, description = "Deployment uuid")),
    responses(
        (status = 200, description = "Deployment with logs", body = DeploymentDetailDto),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
    ))]
pub async fn get(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Json<DeploymentDetailDto>> {
    let repo = DeploymentRepo::new(state.pool.clone());
    let deployment = repo.get_by_uuid(&uuid).await?.ok_or(ApiError::NotFound)?;
    let deployment_id = deployment.id;
    let resolved = resolve(&state, &team, deployment).await?;
    let logs = repo.logs(deployment_id).await?;
    Ok(Json(DeploymentDetailDto {
        deployment: to_dto(resolved),
        logs: logs.into_iter().map(LogLineDto::from).collect(),
    }))
}

#[utoipa::path(post, path = "/deployments/{uuid}/cancel", tag = "deployments",
    params(("uuid" = String, Path, description = "Deployment uuid")),
    responses(
        (status = 202, description = "Cancellation requested"),
        (status = 409, description = "Deployment is not cancellable", body = crate::error::ApiErrorBody),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
    ))]
pub async fn cancel(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Response> {
    let repo = DeploymentRepo::new(state.pool.clone());
    let deployment = repo.get_by_uuid(&uuid).await?.ok_or(ApiError::NotFound)?;
    let deployment_id = deployment.id;
    resolve(&state, &team, deployment).await?; // team ownership check

    let cancelled = repo
        .transition(deployment_id, DeploymentStatus::Cancelled)
        .await?;
    if cancelled {
        Ok((
            StatusCode::ACCEPTED,
            Json(serde_json::json!({ "status": "cancelled" })),
        )
            .into_response())
    } else {
        Err(ApiError::Conflict(
            "deployment is already in a terminal state".into(),
        ))
    }
}
