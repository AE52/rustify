//! AWS EC2 metadata lookups + EC2 provisioning (aws-provision track).
//!
//! The AWS twin of the Hetzner routes in [`crate::routes::cloud`]. Regions and
//! instance types are served from curated static metadata (no token needed).
//! Provisioning resolves the `aws` token, decrypts it, builds a region-scoped
//! [`crate::aws::Ec2Client`], and drives [`crate::aws::provision_aws`], which
//! launches the instances, registers a Rustify server per instance, enqueues
//! the existing `server_validate` install pipeline, and (multi-node) forms a
//! Docker Swarm. Kubernetes/EKS is intentionally out of scope.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use rustify_core::CommandExecutor;
use rustify_db::repos::{CloudTokenRepo, KeyRepo};
use rustify_ssh::SshExecutor;

use crate::app::AppState;
use crate::auth::{CurrentTeam, RequireAdmin};
use crate::aws::{self, AwsCredentials, Ec2Client, ProvisionInput, provision_aws};
use crate::error::{ApiError, ApiResult};

// --------------------------------------------------------------------------
// DTOs
// --------------------------------------------------------------------------

#[derive(Debug, Serialize, ToSchema)]
pub struct AwsRegionDto {
    pub name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AwsInstanceTypeDto {
    pub name: String,
    pub vcpus: u32,
    pub mem_gb: u32,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AwsProvision {
    pub token_uuid: String,
    pub region: String,
    pub instance_type: String,
    /// Number of instances. `1` = single node; `>= 2` = a Docker Swarm cluster.
    pub count: i32,
    pub name: String,
    /// SSH key to install/connect with; defaults to the team's first.
    pub private_key_uuid: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AwsServerDto {
    pub uuid: String,
    pub name: String,
    pub ip: String,
    pub aws_instance_id: Option<String>,
    pub aws_region: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AwsProvisionResponse {
    pub servers: Vec<AwsServerDto>,
    /// Whether the servers were joined into a Docker Swarm.
    pub swarm: bool,
    /// Whether some non-fatal step failed (instance without a public IP, or a
    /// worker that failed to join) while the run still registered servers.
    pub partial: bool,
}

// --------------------------------------------------------------------------
// Metadata lookups (static; no token required)
// --------------------------------------------------------------------------

#[utoipa::path(get, path = "/aws/regions", operation_id = "aws_regions", tag = "cloud",
    responses((status = 200, description = "AWS regions", body = [AwsRegionDto])))]
pub async fn regions(_team: CurrentTeam) -> ApiResult<Json<Vec<AwsRegionDto>>> {
    Ok(Json(
        aws::known_regions()
            .into_iter()
            .map(|name| AwsRegionDto {
                name: name.to_string(),
            })
            .collect(),
    ))
}

#[utoipa::path(get, path = "/aws/instance-types", operation_id = "aws_instance_types", tag = "cloud",
    responses((status = 200, description = "Curated AWS instance types", body = [AwsInstanceTypeDto])))]
pub async fn instance_types(_team: CurrentTeam) -> ApiResult<Json<Vec<AwsInstanceTypeDto>>> {
    Ok(Json(
        aws::curated_instance_types()
            .into_iter()
            .map(|t| AwsInstanceTypeDto {
                name: t.name.to_string(),
                vcpus: t.vcpus,
                mem_gb: t.mem_gb,
            })
            .collect(),
    ))
}

// --------------------------------------------------------------------------
// Provisioning
// --------------------------------------------------------------------------

fn map_provision(e: aws::ProvisionError) -> ApiError {
    match e {
        aws::ProvisionError::Aws(aws::AwsError::Api(m)) => {
            ApiError::Validation(format!("AWS API error: {m}"))
        }
        aws::ProvisionError::Db(e) => ApiError::Internal(e.to_string()),
        other => ApiError::Internal(other.to_string()),
    }
}

#[utoipa::path(post, path = "/servers/provision/aws", operation_id = "provision_aws_servers",
    tag = "cloud", request_body = AwsProvision,
    responses(
        (status = 201, description = "Servers provisioned + validation enqueued", body = AwsProvisionResponse),
        (status = 404, description = "Token or key not found", body = crate::error::ApiErrorBody),
        (status = 422, description = "Validation error", body = crate::error::ApiErrorBody),
    ))]
pub async fn provision(
    State(state): State<AppState>,
    team: CurrentTeam,
    _guard: RequireAdmin,
    Json(body): Json<AwsProvision>,
) -> ApiResult<Response> {
    if body.count < 1 {
        return Err(ApiError::Validation("count must be at least 1".to_string()));
    }
    if body.region.trim().is_empty() {
        return Err(ApiError::Validation("region must not be empty".to_string()));
    }
    if body.instance_type.trim().is_empty() {
        return Err(ApiError::Validation(
            "instance_type must not be empty".to_string(),
        ));
    }

    // Resolve + decrypt the aws token, parse into static credentials.
    let token_repo = CloudTokenRepo::new(state.pool.clone());
    let token = token_repo
        .get_by_uuid(&body.token_uuid)
        .await?
        .filter(|t| t.team_id == team.id && t.provider == "aws")
        .ok_or(ApiError::NotFound)?;
    let secret = token_repo.decrypt_token(team.id, &token.uuid).await?;
    let creds: AwsCredentials = serde_json::from_str(&secret)
        .map_err(|_| ApiError::Validation("stored aws token is malformed".to_string()))?;

    // Resolve the private key (pinned or the team's first) + decrypt material.
    let key_repo = KeyRepo::new(state.pool.clone());
    let key = match &body.private_key_uuid {
        Some(uuid) => key_repo
            .get_by_uuid(uuid)
            .await?
            .filter(|k| k.team_id == team.id)
            .ok_or_else(|| ApiError::Validation(format!("unknown private_key_uuid: {uuid}")))?,
        None => key_repo
            .list(team.id)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| {
                ApiError::Validation("no private key available; create one first".to_string())
            })?,
    };
    let key_material = key_repo.decrypt_private_key(key.id).await?;

    let client = Ec2Client::new(&creds, &body.region);
    let executor: Arc<dyn CommandExecutor> =
        Arc::new(SshExecutor::new(state.config.ssh_mux_dir.clone()));

    let out = provision_aws(
        &client,
        executor.as_ref(),
        &state.pool,
        &state.queue,
        &state.config.ssh_key_dir,
        ProvisionInput {
            team_id: team.id,
            name: body.name,
            region: body.region,
            instance_type: body.instance_type,
            count: body.count,
            key_id: key.id,
            key_name: key.name,
            key_public: key.public_key,
            key_material,
            token_id: token.id,
        },
    )
    .await
    .map_err(map_provision)?;

    let response = AwsProvisionResponse {
        servers: out
            .servers
            .into_iter()
            .map(|s| AwsServerDto {
                uuid: s.uuid,
                name: s.name,
                ip: s.ip,
                aws_instance_id: s.aws_instance_id,
                aws_region: s.aws_region,
            })
            .collect(),
        swarm: out.swarm,
        partial: out.partial,
    };
    Ok((StatusCode::CREATED, Json(response)).into_response())
}
