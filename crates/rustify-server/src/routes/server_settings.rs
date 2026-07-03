//! Server operational settings (OPS/UI track): proxy type, build-server flag,
//! interactive web-terminal enablement, and metrics collection. The Cloudflare
//! tunnel flag has its own enqueued flow (`/servers/{uuid}/cloudflared`) and is
//! surfaced here read-only.
//!
//! These columns live on `server_settings` (migrations 0010/0011). Parity with
//! Coolify's `app/Livewire/Server/Show.php` settings toggles: `is_build_server`,
//! `isTerminalEnabled`, `is_metrics_enabled`, `proxy->type`.

use axum::Json;
use axum::extract::{Path, State};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use rustify_db::repos::{ServerRepo, ServerSettings};

use crate::app::AppState;
use crate::auth::{CurrentTeam, RequireAdmin};
use crate::error::{ApiError, ApiResult};
use crate::routes::servers::owned;

#[derive(Debug, Serialize, ToSchema)]
pub struct ServerSettingsDto {
    pub proxy_type: String,
    pub is_build_server: bool,
    pub is_terminal_enabled: bool,
    pub metrics_enabled: bool,
    pub metrics_refresh_rate_seconds: i32,
    pub is_cloudflare_tunnel: bool,
}

impl From<ServerSettings> for ServerSettingsDto {
    fn from(s: ServerSettings) -> Self {
        ServerSettingsDto {
            proxy_type: s.proxy_type,
            is_build_server: s.is_build_server,
            is_terminal_enabled: s.is_terminal_enabled,
            metrics_enabled: s.metrics_enabled,
            metrics_refresh_rate_seconds: s.metrics_refresh_rate_seconds,
            is_cloudflare_tunnel: s.is_cloudflare_tunnel,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ServerSettingsUpdate {
    /// Reverse proxy type: `traefik` or `caddy`.
    pub proxy_type: Option<String>,
    pub is_build_server: Option<bool>,
    pub is_terminal_enabled: Option<bool>,
    pub metrics_enabled: Option<bool>,
    pub metrics_refresh_rate_seconds: Option<i32>,
}

/// Normalise + validate a proxy type to Coolify's supported reverse proxies.
fn validate_proxy_type(raw: &str) -> ApiResult<String> {
    match raw.trim().to_lowercase().as_str() {
        "traefik" => Ok("traefik".to_string()),
        "caddy" => Ok("caddy".to_string()),
        other => Err(ApiError::Validation(format!("unknown proxy type: {other}"))),
    }
}

#[utoipa::path(get, path = "/servers/{uuid}/settings", operation_id = "get_server_settings",
    tag = "servers", params(("uuid" = String, Path, description = "Server uuid")),
    responses((status = 200, description = "Server settings", body = ServerSettingsDto)))]
pub async fn get_settings(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Json<ServerSettingsDto>> {
    let server = owned(&state, &team, &uuid).await?;
    let settings = ServerRepo::new(state.pool.clone())
        .settings(server.id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(ServerSettingsDto::from(settings)))
}

#[utoipa::path(patch, path = "/servers/{uuid}/settings", operation_id = "update_server_settings",
    tag = "servers", params(("uuid" = String, Path, description = "Server uuid")),
    request_body = ServerSettingsUpdate,
    responses(
        (status = 200, description = "Updated settings", body = ServerSettingsDto),
        (status = 403, description = "Forbidden", body = crate::error::ApiErrorBody),
        (status = 422, description = "Validation error", body = crate::error::ApiErrorBody),
    ))]
pub async fn update_settings(
    State(state): State<AppState>,
    team: CurrentTeam,
    _guard: RequireAdmin,
    Path(uuid): Path<String>,
    Json(body): Json<ServerSettingsUpdate>,
) -> ApiResult<Json<ServerSettingsDto>> {
    let server = owned(&state, &team, &uuid).await?;
    let proxy_type = match body.proxy_type.as_deref() {
        Some(raw) => Some(validate_proxy_type(raw)?),
        None => None,
    };
    if let Some(rate) = body.metrics_refresh_rate_seconds {
        if rate < 1 {
            return Err(ApiError::Validation(
                "metrics_refresh_rate_seconds must be positive".to_string(),
            ));
        }
    }
    let settings = ServerRepo::new(state.pool.clone())
        .update_settings(
            server.id,
            proxy_type.as_deref(),
            body.is_build_server,
            body.is_terminal_enabled,
            body.metrics_enabled,
            body.metrics_refresh_rate_seconds,
        )
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(ServerSettingsDto::from(settings)))
}

#[cfg(test)]
mod tests {
    use super::validate_proxy_type;

    #[test]
    fn accepts_supported_proxies_case_insensitively() {
        assert_eq!(validate_proxy_type("Traefik").unwrap(), "traefik");
        assert_eq!(validate_proxy_type("  CADDY ").unwrap(), "caddy");
    }

    #[test]
    fn rejects_unknown_proxy() {
        assert!(validate_proxy_type("nginx").is_err());
    }
}
