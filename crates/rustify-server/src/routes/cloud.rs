//! Cloud-provider token management, Hetzner metadata lookups, and Hetzner
//! server provisioning (contract: OPS track).
//!
//! Tokens are stored encrypted (`CloudTokenRepo`) and shown only by uuid/name.
//! The lookup + provision routes resolve a token, decrypt it, and drive the
//! [`crate::hetzner::HetznerClient`]. Provisioning mirrors Coolify's
//! `HetznerController::createServer` + `ByHetzner::submit`.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::ToSchema;

use rustify_db::repos::{CloudTokenRepo, KeyRepo, NewHetznerServer, ServerRepo};

use crate::app::AppState;
use crate::auth::{CurrentTeam, RequireAdmin};
use crate::error::{ApiError, ApiResult};
use crate::hetzner::{CreateServerParams, HetznerClient, HetznerError, ReqwestTransport};

// --------------------------------------------------------------------------
// DTOs
// --------------------------------------------------------------------------

#[derive(Debug, Serialize, ToSchema)]
pub struct CloudTokenDto {
    pub uuid: String,
    pub provider: String,
    pub name: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CloudTokenCreate {
    pub provider: String,
    pub name: Option<String>,
    /// The plaintext API token — stored encrypted, never returned. Required for
    /// single-secret providers such as Hetzner.
    pub token: Option<String>,
    /// AWS access key id — required when `provider = "aws"`.
    pub access_key_id: Option<String>,
    /// AWS secret access key — required when `provider = "aws"`. Stored encrypted
    /// as JSON `{access_key_id, secret_access_key}`, never returned or logged.
    pub secret_access_key: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TokenQuery {
    pub token_uuid: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct HetznerProvision {
    pub token_uuid: String,
    pub name: String,
    pub server_type: String,
    pub location: String,
    pub image: i64,
    /// Optional SSH key to install/connect with; defaults to the team's first.
    pub private_key_uuid: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HetznerProvisionResponse {
    pub uuid: String,
    pub hetzner_server_id: i64,
    pub ip: String,
}

// --------------------------------------------------------------------------
// Error mapping
// --------------------------------------------------------------------------

fn map_hetzner(e: HetznerError) -> ApiError {
    match e {
        HetznerError::RateLimited { .. } => {
            ApiError::Conflict("Hetzner rate limit exceeded; try again later.".to_string())
        }
        HetznerError::Api(m) => ApiError::Validation(format!("Hetzner API error: {m}")),
        other => ApiError::Internal(other.to_string()),
    }
}

/// Resolve + decrypt a team-scoped Hetzner token into a ready client.
async fn hetzner_client(
    state: &AppState,
    team: &CurrentTeam,
    token_uuid: &str,
) -> ApiResult<HetznerClient<ReqwestTransport>> {
    let repo = CloudTokenRepo::new(state.pool.clone());
    let token = repo
        .get_by_uuid(token_uuid)
        .await?
        .filter(|t| t.team_id == team.id && t.provider == "hetzner")
        .ok_or(ApiError::NotFound)?;
    let secret = repo.decrypt_token(team.id, &token.uuid).await?;
    Ok(HetznerClient::new(ReqwestTransport::new(), secret))
}

// --------------------------------------------------------------------------
// Cloud tokens
// --------------------------------------------------------------------------

#[utoipa::path(get, path = "/cloud-tokens", operation_id = "list_cloud_tokens", tag = "cloud",
    responses((status = 200, description = "Cloud provider tokens", body = [CloudTokenDto])))]
pub async fn list_tokens(
    State(state): State<AppState>,
    team: CurrentTeam,
) -> ApiResult<Json<Vec<CloudTokenDto>>> {
    let tokens = CloudTokenRepo::new(state.pool.clone())
        .list(team.id)
        .await?;
    Ok(Json(
        tokens
            .into_iter()
            .map(|t| CloudTokenDto {
                uuid: t.uuid,
                provider: t.provider,
                name: t.name,
                created_at: t.created_at,
            })
            .collect(),
    ))
}

#[utoipa::path(post, path = "/cloud-tokens", operation_id = "create_cloud_token", tag = "cloud",
    request_body = CloudTokenCreate,
    responses((status = 201, description = "Token stored", body = CloudTokenDto)))]
pub async fn create_token(
    State(state): State<AppState>,
    team: CurrentTeam,
    _guard: RequireAdmin,
    Json(body): Json<CloudTokenCreate>,
) -> ApiResult<Response> {
    // AWS stores an encrypted JSON blob of {access_key_id, secret_access_key};
    // every other provider stores a single opaque token string.
    let material = if body.provider == "aws" {
        let access_key_id = body.access_key_id.as_deref().unwrap_or("").trim();
        let secret_access_key = body.secret_access_key.as_deref().unwrap_or("").trim();
        if access_key_id.is_empty() || secret_access_key.is_empty() {
            return Err(ApiError::Validation(
                "aws token requires access_key_id and secret_access_key".to_string(),
            ));
        }
        serde_json::to_string(&crate::aws::AwsCredentials {
            access_key_id: access_key_id.to_string(),
            secret_access_key: secret_access_key.to_string(),
        })
        .map_err(|e| ApiError::Internal(e.to_string()))?
    } else {
        let token = body.token.as_deref().unwrap_or("").trim();
        if token.is_empty() {
            return Err(ApiError::Validation("token must not be empty".to_string()));
        }
        token.to_string()
    };
    let token = CloudTokenRepo::new(state.pool.clone())
        .create(team.id, &body.provider, body.name.as_deref(), &material)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(CloudTokenDto {
            uuid: token.uuid,
            provider: token.provider,
            name: token.name,
            created_at: token.created_at,
        }),
    )
        .into_response())
}

#[utoipa::path(delete, path = "/cloud-tokens/{uuid}", operation_id = "delete_cloud_token", tag = "cloud",
    params(("uuid" = String, Path, description = "Cloud token uuid")),
    responses((status = 204, description = "Deleted")))]
pub async fn delete_token(
    State(state): State<AppState>,
    team: CurrentTeam,
    _guard: RequireAdmin,
    Path(uuid): Path<String>,
) -> ApiResult<StatusCode> {
    if CloudTokenRepo::new(state.pool.clone())
        .delete(team.id, &uuid)
        .await?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

// --------------------------------------------------------------------------
// Hetzner lookups
// --------------------------------------------------------------------------

#[utoipa::path(get, path = "/hetzner/locations", operation_id = "hetzner_locations", tag = "cloud",
    params(("token_uuid" = String, Query, description = "Cloud token uuid")),
    responses((status = 200, description = "Hetzner locations")))]
pub async fn hetzner_locations(
    State(state): State<AppState>,
    team: CurrentTeam,
    Query(q): Query<TokenQuery>,
) -> ApiResult<Json<Value>> {
    let client = hetzner_client(&state, &team, &q.token_uuid).await?;
    let items = client.get_locations().await.map_err(map_hetzner)?;
    Ok(Json(Value::Array(items)))
}

#[utoipa::path(get, path = "/hetzner/server-types", operation_id = "hetzner_server_types", tag = "cloud",
    params(("token_uuid" = String, Query, description = "Cloud token uuid")),
    responses((status = 200, description = "Hetzner server types")))]
pub async fn hetzner_server_types(
    State(state): State<AppState>,
    team: CurrentTeam,
    Query(q): Query<TokenQuery>,
) -> ApiResult<Json<Value>> {
    let client = hetzner_client(&state, &team, &q.token_uuid).await?;
    let items = client.get_server_types().await.map_err(map_hetzner)?;
    Ok(Json(Value::Array(items)))
}

#[utoipa::path(get, path = "/hetzner/images", operation_id = "hetzner_images", tag = "cloud",
    params(("token_uuid" = String, Query, description = "Cloud token uuid")),
    responses((status = 200, description = "Hetzner system images")))]
pub async fn hetzner_images(
    State(state): State<AppState>,
    team: CurrentTeam,
    Query(q): Query<TokenQuery>,
) -> ApiResult<Json<Value>> {
    let client = hetzner_client(&state, &team, &q.token_uuid).await?;
    let items = client.get_images().await.map_err(map_hetzner)?;
    Ok(Json(Value::Array(items)))
}

// --------------------------------------------------------------------------
// Provisioning
// --------------------------------------------------------------------------

#[utoipa::path(post, path = "/servers/provision/hetzner", operation_id = "provision_hetzner_server",
    tag = "cloud", request_body = HetznerProvision,
    responses(
        (status = 201, description = "Server provisioned + validation enqueued", body = HetznerProvisionResponse),
        (status = 404, description = "Token or key not found", body = crate::error::ApiErrorBody),
    ))]
pub async fn provision_hetzner(
    State(state): State<AppState>,
    team: CurrentTeam,
    _guard: RequireAdmin,
    Json(body): Json<HetznerProvision>,
) -> ApiResult<Response> {
    // Resolve token + client.
    let token_repo = CloudTokenRepo::new(state.pool.clone());
    let token = token_repo
        .get_by_uuid(&body.token_uuid)
        .await?
        .filter(|t| t.team_id == team.id && t.provider == "hetzner")
        .ok_or(ApiError::NotFound)?;
    let secret = token_repo.decrypt_token(team.id, &token.uuid).await?;
    let client = HetznerClient::new(ReqwestTransport::new(), secret);

    // Resolve the private key (pinned or the team's first).
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

    // Ensure the SSH key exists on Hetzner (dedupe by fingerprint).
    let ssh_key_id = client
        .ensure_ssh_key(&key.name, &key.public_key)
        .await
        .map_err(map_hetzner)?;

    // Create the server.
    let params = CreateServerParams::new(
        &body.name,
        &body.server_type,
        body.image,
        &body.location,
        vec![ssh_key_id],
    );
    let hetzner_server = client.create_server(&params).await.map_err(map_hetzner)?;
    let ip = hetzner_server
        .public_ip()
        .ok_or_else(|| ApiError::Validation("Hetzner returned no public IP address".to_string()))?;

    // Register in Rustify (user root, port 22, proxy traefik/exited).
    let server = ServerRepo::new(state.pool.clone())
        .create_hetzner(NewHetznerServer {
            team_id: team.id,
            name: params.name.clone(),
            ip: ip.clone(),
            port: 22,
            ssh_user: "root".to_string(),
            private_key_id: key.id,
            hetzner_server_id: hetzner_server.id,
            cloud_provider_token_id: token.id,
        })
        .await?;

    // Enqueue the existing validate/install pipeline.
    state
        .queue
        .enqueue(
            "server_validate",
            json!({ "server_uuid": server.uuid }),
            None,
        )
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok((
        StatusCode::CREATED,
        Json(HetznerProvisionResponse {
            uuid: server.uuid,
            hetzner_server_id: hetzner_server.id,
            ip,
        }),
    )
        .into_response())
}
