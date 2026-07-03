//! Servers routes (contract C5), plus proxy config get/save/lifecycle and
//! reachability validation (enqueued as a `server_validate` job).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

use rustify_db::repos::{KeyRepo, NewServer, Server, ServerRepo};

use crate::app::AppState;
use crate::auth::{CurrentTeam, RequireAdmin};
use crate::error::{ApiError, ApiResult};

#[derive(Debug, Serialize, ToSchema)]
pub struct ServerDto {
    pub uuid: String,
    pub name: String,
    pub ip: String,
    pub port: i32,
    pub user: String,
    pub private_key_uuid: String,
    pub reachable: bool,
    pub usable: bool,
    pub validation_logs: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ServerCreate {
    pub name: String,
    pub ip: String,
    pub port: Option<i32>,
    pub user: Option<String>,
    pub private_key_uuid: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ServerUpdate {
    pub name: Option<String>,
    pub ip: Option<String>,
    pub port: Option<i32>,
    pub user: Option<String>,
    pub private_key_uuid: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProxyConfig {
    pub proxy_type: String,
    pub proxy_status: String,
    pub proxy_custom_config: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ProxyConfigUpdate {
    pub proxy_custom_config: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ValidateResponse {
    pub job_uuid: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CloudflaredEnable {
    /// The Cloudflare tunnel token to run the agent with.
    pub tunnel_token: String,
    /// The tunnel's public SSH hostname (Coolify: `ssh_domain`). Once the tunnel
    /// is healthy the server's `ip` is repointed at this hostname so subsequent
    /// ssh connections dial `cloudflared access ssh --hostname <this>`.
    pub ssh_hostname: String,
}

/// Normalise an operator-supplied tunnel SSH hostname: drop any `http(s)://`
/// scheme and trailing slash, matching Coolify's `automatedCloudflareConfig`.
fn normalize_ssh_hostname(raw: &str) -> String {
    raw.trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .trim()
        .to_string()
}

/// Render a server row with its key uuid resolved.
async fn server_dto(state: &AppState, server: Server) -> ApiResult<ServerDto> {
    let key = KeyRepo::new(state.pool.clone())
        .get_by_id(server.private_key_id)
        .await?;
    Ok(ServerDto {
        uuid: server.uuid,
        name: server.name,
        ip: server.ip,
        port: server.port,
        user: server.ssh_user,
        private_key_uuid: key.map(|k| k.uuid).unwrap_or_default(),
        reachable: server.reachable,
        usable: server.usable,
        validation_logs: server.validation_logs,
        created_at: server.created_at,
        updated_at: server.updated_at,
    })
}

/// Fetch a server by uuid, enforcing team ownership.
pub(crate) async fn owned(state: &AppState, team: &CurrentTeam, uuid: &str) -> ApiResult<Server> {
    let server = ServerRepo::new(state.pool.clone())
        .get_by_uuid(uuid)
        .await?
        .ok_or(ApiError::NotFound)?;
    if server.team_id != team.id {
        return Err(ApiError::NotFound);
    }
    Ok(server)
}

/// Resolve a key uuid to its id within the team, or a validation error.
async fn key_id_for(state: &AppState, team: &CurrentTeam, key_uuid: &str) -> ApiResult<i64> {
    let key = KeyRepo::new(state.pool.clone())
        .get_by_uuid(key_uuid)
        .await?
        .filter(|k| k.team_id == team.id)
        .ok_or_else(|| ApiError::Validation(format!("unknown private_key_uuid: {key_uuid}")))?;
    Ok(key.id)
}

#[utoipa::path(get, path = "/servers", operation_id = "list_servers", tag = "servers",
    responses((status = 200, description = "List of servers", body = [ServerDto])))]
pub async fn list(
    State(state): State<AppState>,
    team: CurrentTeam,
) -> ApiResult<Json<Vec<ServerDto>>> {
    let servers = ServerRepo::new(state.pool.clone()).list(team.id).await?;
    let mut out = Vec::with_capacity(servers.len());
    for s in servers {
        out.push(server_dto(&state, s).await?);
    }
    Ok(Json(out))
}

#[utoipa::path(post, path = "/servers", operation_id = "create_server", tag = "servers", request_body = ServerCreate,
    responses(
        (status = 201, description = "Server created", body = ServerDto),
        (status = 422, description = "Validation error", body = crate::error::ApiErrorBody),
    ))]
pub async fn create(
    State(state): State<AppState>,
    team: CurrentTeam,
    _guard: RequireAdmin,
    Json(body): Json<ServerCreate>,
) -> ApiResult<Response> {
    let private_key_id = key_id_for(&state, &team, &body.private_key_uuid).await?;
    let server = ServerRepo::new(state.pool.clone())
        .create(NewServer {
            team_id: team.id,
            name: body.name,
            ip: body.ip,
            port: body.port.unwrap_or(22),
            ssh_user: body.user.unwrap_or_else(|| "root".to_string()),
            private_key_id,
        })
        .await?;
    Ok((StatusCode::CREATED, Json(server_dto(&state, server).await?)).into_response())
}

#[utoipa::path(get, path = "/servers/{uuid}", operation_id = "get_server", tag = "servers",
    params(("uuid" = String, Path, description = "Server uuid")),
    responses(
        (status = 200, description = "The server", body = ServerDto),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
    ))]
pub async fn get(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Json<ServerDto>> {
    let server = owned(&state, &team, &uuid).await?;
    Ok(Json(server_dto(&state, server).await?))
}

#[utoipa::path(patch, path = "/servers/{uuid}", operation_id = "update_server", tag = "servers",
    params(("uuid" = String, Path, description = "Server uuid")),
    request_body = ServerUpdate,
    responses(
        (status = 200, description = "Updated server", body = ServerDto),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
    ))]
pub async fn update(
    State(state): State<AppState>,
    team: CurrentTeam,
    _guard: RequireAdmin,
    Path(uuid): Path<String>,
    Json(body): Json<ServerUpdate>,
) -> ApiResult<Json<ServerDto>> {
    owned(&state, &team, &uuid).await?;
    let private_key_id = match &body.private_key_uuid {
        Some(key_uuid) => Some(key_id_for(&state, &team, key_uuid).await?),
        None => None,
    };
    let server = ServerRepo::new(state.pool.clone())
        .update(
            &uuid,
            body.name.as_deref(),
            body.ip.as_deref(),
            body.port,
            body.user.as_deref(),
            private_key_id,
        )
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(server_dto(&state, server).await?))
}

#[utoipa::path(delete, path = "/servers/{uuid}", operation_id = "delete_server", tag = "servers",
    params(("uuid" = String, Path, description = "Server uuid")),
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
    if ServerRepo::new(state.pool.clone()).delete(&uuid).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

#[utoipa::path(post, path = "/servers/{uuid}/validate", tag = "servers",
    params(("uuid" = String, Path, description = "Server uuid")),
    responses((status = 202, description = "Validation enqueued", body = ValidateResponse)))]
pub async fn validate(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Response> {
    let server = owned(&state, &team, &uuid).await?;
    let job_id = state
        .queue
        .enqueue(
            "server_validate",
            json!({ "server_uuid": server.uuid }),
            None,
        )
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok((
        StatusCode::ACCEPTED,
        Json(ValidateResponse {
            job_uuid: job_id.to_string(),
        }),
    )
        .into_response())
}

#[utoipa::path(get, path = "/servers/{uuid}/proxy", tag = "servers",
    params(("uuid" = String, Path, description = "Server uuid")),
    responses((status = 200, description = "Proxy config", body = ProxyConfig)))]
pub async fn get_proxy(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Json<ProxyConfig>> {
    let server = owned(&state, &team, &uuid).await?;
    let settings = ServerRepo::new(state.pool.clone())
        .settings(server.id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(ProxyConfig {
        proxy_type: settings.proxy_type,
        proxy_status: settings.proxy_status,
        proxy_custom_config: settings.proxy_custom_config,
    }))
}

#[utoipa::path(patch, path = "/servers/{uuid}/proxy", tag = "servers",
    params(("uuid" = String, Path, description = "Server uuid")),
    request_body = ProxyConfigUpdate,
    responses((status = 200, description = "Saved proxy config", body = ProxyConfig)))]
pub async fn update_proxy(
    State(state): State<AppState>,
    team: CurrentTeam,
    _guard: RequireAdmin,
    Path(uuid): Path<String>,
    Json(body): Json<ProxyConfigUpdate>,
) -> ApiResult<Json<ProxyConfig>> {
    let server = owned(&state, &team, &uuid).await?;
    let repo = ServerRepo::new(state.pool.clone());
    repo.set_proxy_custom_config(server.id, body.proxy_custom_config.as_deref())
        .await?;
    let settings = repo.settings(server.id).await?.ok_or(ApiError::NotFound)?;
    Ok(Json(ProxyConfig {
        proxy_type: settings.proxy_type,
        proxy_status: settings.proxy_status,
        proxy_custom_config: settings.proxy_custom_config,
    }))
}

async fn proxy_lifecycle(
    state: &AppState,
    team: &CurrentTeam,
    uuid: &str,
    action: &str,
) -> ApiResult<Response> {
    let server = owned(state, team, uuid).await?;
    state
        .queue
        .enqueue(
            &format!("proxy_{action}"),
            json!({ "server_uuid": server.uuid }),
            None,
        )
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "status": "accepted" }))).into_response())
}

#[utoipa::path(post, path = "/servers/{uuid}/cloudflared", tag = "servers",
    params(("uuid" = String, Path, description = "Server uuid")),
    request_body = CloudflaredEnable,
    responses((status = 202, description = "Cloudflare tunnel configuration enqueued")))]
pub async fn cloudflared_enable(
    State(state): State<AppState>,
    team: CurrentTeam,
    _guard: RequireAdmin,
    Path(uuid): Path<String>,
    Json(body): Json<CloudflaredEnable>,
) -> ApiResult<Response> {
    let server = owned(&state, &team, &uuid).await?;
    if body.tunnel_token.trim().is_empty() {
        return Err(ApiError::Validation(
            "tunnel_token must not be empty".to_string(),
        ));
    }
    let ssh_hostname = normalize_ssh_hostname(&body.ssh_hostname);
    if ssh_hostname.is_empty() {
        return Err(ApiError::Validation(
            "ssh_hostname must not be empty".to_string(),
        ));
    }
    state
        .queue
        .enqueue(
            "configure_cloudflared",
            json!({
                "server_uuid": server.uuid,
                "tunnel_token": body.tunnel_token,
                "ssh_hostname": ssh_hostname,
                "action": "configure",
            }),
            None,
        )
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "status": "accepted" }))).into_response())
}

#[utoipa::path(delete, path = "/servers/{uuid}/cloudflared", tag = "servers",
    params(("uuid" = String, Path, description = "Server uuid")),
    responses((status = 202, description = "Cloudflare tunnel teardown enqueued")))]
pub async fn cloudflared_disable(
    State(state): State<AppState>,
    team: CurrentTeam,
    _guard: RequireAdmin,
    Path(uuid): Path<String>,
) -> ApiResult<Response> {
    let server = owned(&state, &team, &uuid).await?;
    state
        .queue
        .enqueue(
            "configure_cloudflared",
            json!({ "server_uuid": server.uuid, "action": "disable" }),
            None,
        )
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "status": "accepted" }))).into_response())
}

#[utoipa::path(post, path = "/servers/{uuid}/proxy/start", tag = "servers",
    params(("uuid" = String, Path, description = "Server uuid")),
    responses((status = 202, description = "Proxy start enqueued")))]
pub async fn proxy_start(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Response> {
    proxy_lifecycle(&state, &team, &uuid, "start").await
}

#[utoipa::path(post, path = "/servers/{uuid}/proxy/stop", tag = "servers",
    params(("uuid" = String, Path, description = "Server uuid")),
    responses((status = 202, description = "Proxy stop enqueued")))]
pub async fn proxy_stop(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Response> {
    proxy_lifecycle(&state, &team, &uuid, "stop").await
}

#[utoipa::path(post, path = "/servers/{uuid}/proxy/restart", tag = "servers",
    params(("uuid" = String, Path, description = "Server uuid")),
    responses((status = 202, description = "Proxy restart enqueued")))]
pub async fn proxy_restart(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Response> {
    proxy_lifecycle(&state, &team, &uuid, "restart").await
}

#[cfg(test)]
mod tests {
    use super::normalize_ssh_hostname;

    #[test]
    fn strips_scheme_and_trailing_slash() {
        assert_eq!(
            normalize_ssh_hostname("https://ssh.example.com/"),
            "ssh.example.com"
        );
        assert_eq!(
            normalize_ssh_hostname("http://ssh.example.com"),
            "ssh.example.com"
        );
        assert_eq!(
            normalize_ssh_hostname("  ssh.example.com  "),
            "ssh.example.com"
        );
        assert_eq!(normalize_ssh_hostname("ssh.example.com"), "ssh.example.com");
        assert_eq!(normalize_ssh_hostname("  "), "");
    }
}
