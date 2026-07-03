//! Application PR-preview routes.
//!
//! Read-only listing of `application_previews` for an application plus two
//! actions that mirror the git-webhook flow (Webhook/Github.php): `redeploy`
//! re-queues a deployment for the PR, `cleanup` enqueues teardown of the
//! preview's containers/network. Ownership is enforced via the application's
//! team (parity with the other application sub-routes).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use utoipa::ToSchema;

use rustify_db::repos::{ApplicationPreview, DeploymentRepo, NewDeployment, PreviewRepo};

use crate::app::AppState;
use crate::auth::CurrentTeam;
use crate::error::{ApiError, ApiResult};
use crate::routes::applications::resolve;

/// A PR preview as returned by the API.
#[derive(Debug, Serialize, ToSchema)]
pub struct PreviewDto {
    pub uuid: String,
    pub pull_request_id: i32,
    pub pull_request_html_url: Option<String>,
    pub fqdn: Option<String>,
    pub status: String,
    pub git_type: Option<String>,
    pub last_online_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<ApplicationPreview> for PreviewDto {
    fn from(p: ApplicationPreview) -> Self {
        Self {
            uuid: p.uuid,
            pull_request_id: p.pull_request_id,
            pull_request_html_url: p.pull_request_html_url,
            fqdn: p.fqdn,
            status: p.status,
            git_type: p.git_type,
            last_online_at: p.last_online_at,
            created_at: p.created_at,
            updated_at: p.updated_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PreviewRedeployResponse {
    pub deployment_uuid: String,
}

#[utoipa::path(get, path = "/applications/{uuid}/previews", operation_id = "list_application_previews",
    tag = "applications",
    params(("uuid" = String, Path, description = "Application uuid")),
    responses((status = 200, description = "PR previews", body = [PreviewDto])))]
pub async fn list(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Json<Vec<PreviewDto>>> {
    let ctx = resolve(&state, &team, &uuid).await?;
    let previews = PreviewRepo::new(state.pool.clone())
        .list_by_application(ctx.app.id)
        .await?;
    Ok(Json(previews.into_iter().map(PreviewDto::from).collect()))
}

#[utoipa::path(post, path = "/applications/{uuid}/previews/{pr}/redeploy",
    operation_id = "redeploy_application_preview", tag = "applications",
    params(
        ("uuid" = String, Path, description = "Application uuid"),
        ("pr" = i32, Path, description = "Pull request id"),
    ),
    responses(
        (status = 202, description = "Preview deployment queued", body = PreviewRedeployResponse),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
    ))]
pub async fn redeploy(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path((uuid, pr)): Path<(String, i32)>,
) -> ApiResult<Response> {
    let ctx = resolve(&state, &team, &uuid).await?;
    let preview = PreviewRepo::new(state.pool.clone())
        .get(ctx.app.id, pr)
        .await?
        .ok_or(ApiError::NotFound)?;

    let deployment = DeploymentRepo::new(state.pool.clone())
        .create_queued(NewDeployment {
            application_id: ctx.app.id,
            server_id: ctx.server.id,
            commit_sha: None,
            force_rebuild: false,
            pull_request_id: preview.pull_request_id,
            git_type: preview.git_type.clone(),
            ..Default::default()
        })
        .await?;

    state
        .queue
        .enqueue(
            "deploy",
            json!({ "deployment_uuid": deployment.uuid }),
            None,
        )
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok((
        StatusCode::ACCEPTED,
        Json(PreviewRedeployResponse {
            deployment_uuid: deployment.uuid,
        }),
    )
        .into_response())
}

#[utoipa::path(delete, path = "/applications/{uuid}/previews/{pr}",
    operation_id = "cleanup_application_preview", tag = "applications",
    params(
        ("uuid" = String, Path, description = "Application uuid"),
        ("pr" = i32, Path, description = "Pull request id"),
    ),
    responses(
        (status = 202, description = "Preview cleanup enqueued"),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
    ))]
pub async fn cleanup(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path((uuid, pr)): Path<(String, i32)>,
) -> ApiResult<StatusCode> {
    let ctx = resolve(&state, &team, &uuid).await?;
    PreviewRepo::new(state.pool.clone())
        .get(ctx.app.id, pr)
        .await?
        .ok_or(ApiError::NotFound)?;

    state
        .queue
        .enqueue(
            "preview_cleanup",
            json!({ "application_uuid": ctx.app.uuid, "pull_request_id": pr }),
            None,
        )
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::ACCEPTED)
}
