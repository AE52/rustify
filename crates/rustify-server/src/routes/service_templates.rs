//! Service-template catalog routes (contract C5): browse the bundled one-click
//! service templates and fetch one by key. The manifest is the committed
//! `templates/services/index.json`, embedded at build time.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use axum::Json;
use axum::extract::{Path, State};
use serde::Serialize;
use utoipa::ToSchema;

use rustify_docker::service_manifest::{ServiceTemplate, load_manifest};

use crate::app::AppState;
use crate::auth::CurrentTeam;
use crate::error::{ApiError, ApiResult};

/// The committed manifest, embedded and parsed once.
const MANIFEST_JSON: &str = include_str!("../../../../templates/services/index.json");

fn manifest() -> &'static BTreeMap<String, ServiceTemplate> {
    static MANIFEST: OnceLock<BTreeMap<String, ServiceTemplate>> = OnceLock::new();
    MANIFEST.get_or_init(|| load_manifest(MANIFEST_JSON).unwrap_or_default())
}

/// Look up a bundled template's raw compose YAML by key (used by service
/// creation). Returns `None` for an unknown key.
pub fn template_compose(key: &str) -> Option<String> {
    manifest().get(key).and_then(ServiceTemplate::compose_yaml)
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ServiceTemplateDto {
    pub key: String,
    pub name: String,
    pub slogan: String,
    pub documentation: String,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub logo: Option<String>,
    pub port: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ServiceTemplateDetailDto {
    pub key: String,
    pub name: String,
    pub slogan: String,
    pub documentation: String,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub logo: Option<String>,
    pub port: Option<String>,
    /// Base64 of the compose YAML.
    pub compose_b64: String,
}

fn to_summary(key: &str, t: &ServiceTemplate) -> ServiceTemplateDto {
    ServiceTemplateDto {
        key: key.to_string(),
        name: t.name.clone(),
        slogan: t.slogan.clone(),
        documentation: t.documentation.clone(),
        category: t.category.clone(),
        tags: t.tags.clone(),
        logo: t.logo.clone(),
        port: t.port.clone(),
    }
}

#[utoipa::path(get, path = "/service-templates", operation_id = "list_service_templates",
    tag = "service-templates",
    responses((status = 200, description = "Available service templates", body = [ServiceTemplateDto])))]
pub async fn list(
    State(_state): State<AppState>,
    _team: CurrentTeam,
) -> ApiResult<Json<Vec<ServiceTemplateDto>>> {
    let out = manifest().iter().map(|(k, t)| to_summary(k, t)).collect();
    Ok(Json(out))
}

#[utoipa::path(get, path = "/service-templates/{key}", operation_id = "get_service_template",
    tag = "service-templates",
    params(("key" = String, Path, description = "Template key")),
    responses(
        (status = 200, description = "The template", body = ServiceTemplateDetailDto),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
    ))]
pub async fn get(
    State(_state): State<AppState>,
    _team: CurrentTeam,
    Path(key): Path<String>,
) -> ApiResult<Json<ServiceTemplateDetailDto>> {
    let t = manifest().get(&key).ok_or(ApiError::NotFound)?;
    Ok(Json(ServiceTemplateDetailDto {
        key,
        name: t.name.clone(),
        slogan: t.slogan.clone(),
        documentation: t.documentation.clone(),
        category: t.category.clone(),
        tags: t.tags.clone(),
        logo: t.logo.clone(),
        port: t.port.clone(),
        compose_b64: t.compose_b64.clone(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_manifest_parses_and_is_non_empty() {
        assert!(!manifest().is_empty(), "index.json embedded and parsed");
    }

    #[test]
    fn template_compose_decodes_known_key() {
        // umami is in the bundled subset.
        let compose = template_compose("umami").expect("umami template present");
        assert!(compose.contains("services:"));
        assert!(template_compose("does-not-exist").is_none());
    }
}
