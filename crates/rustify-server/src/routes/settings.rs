//! Instance-settings routes (contract C5). Settings are instance-global; any
//! authenticated principal may read/update them in Phase 1.

use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use rustify_db::repos::SettingsRepo;

use crate::app::AppState;
use crate::auth::CurrentTeam;
use crate::error::ApiResult;

#[derive(Debug, Serialize, ToSchema)]
pub struct InstanceSettingsDto {
    pub fqdn: Option<String>,
    pub wildcard_domain: Option<String>,
    pub registration_enabled: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct InstanceSettingsUpdate {
    pub fqdn: Option<String>,
    pub wildcard_domain: Option<String>,
    pub registration_enabled: Option<bool>,
}

#[utoipa::path(get, path = "/settings", operation_id = "get_settings", tag = "settings",
    responses((status = 200, description = "Instance settings", body = InstanceSettingsDto)))]
pub async fn get(
    State(state): State<AppState>,
    _team: CurrentTeam,
) -> ApiResult<Json<InstanceSettingsDto>> {
    let s = SettingsRepo::new(state.pool.clone()).get().await?;
    Ok(Json(InstanceSettingsDto {
        fqdn: s.fqdn,
        wildcard_domain: s.wildcard_domain,
        registration_enabled: s.registration_enabled,
    }))
}

#[utoipa::path(patch, path = "/settings", operation_id = "update_settings", tag = "settings",
    request_body = InstanceSettingsUpdate,
    responses((status = 200, description = "Updated settings", body = InstanceSettingsDto)))]
pub async fn update(
    State(state): State<AppState>,
    _team: CurrentTeam,
    Json(body): Json<InstanceSettingsUpdate>,
) -> ApiResult<Json<InstanceSettingsDto>> {
    let repo = SettingsRepo::new(state.pool.clone());
    let current = repo.get().await?;
    // PATCH semantics: absent fields keep their current value.
    let fqdn = body.fqdn.or(current.fqdn);
    let wildcard_domain = body.wildcard_domain.or(current.wildcard_domain);
    let registration_enabled = body
        .registration_enabled
        .unwrap_or(current.registration_enabled);
    let updated = repo
        .update(
            fqdn.as_deref(),
            wildcard_domain.as_deref(),
            registration_enabled,
        )
        .await?;
    Ok(Json(InstanceSettingsDto {
        fqdn: updated.fqdn,
        wildcard_domain: updated.wildcard_domain,
        registration_enabled: updated.registration_enabled,
    }))
}
