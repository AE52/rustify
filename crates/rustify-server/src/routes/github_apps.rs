//! GitHub App source routes (coolify app/Http/Controllers/Api/GithubController.php
//! + app/Http/Controllers/Webhook/Github.php).
//!
//! CRUD over `github_apps` plus two GitHub-backed read endpoints (repositories /
//! branches) and the app-manifest web flow (redirect / install). `client_secret`
//! and `webhook_secret` are encrypted at rest and never serialised back.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use utoipa::ToSchema;

use rustify_db::repos::{GithubApp, GithubAppPatch, GithubAppRepo, KeyRepo, NewGithubApp};
use rustify_deploy::github::{self, GithubAppRow};

use crate::app::AppState;
use crate::auth::CurrentTeam;
use crate::error::{ApiError, ApiResult};

// ----- DTOs ---------------------------------------------------------------

/// A GitHub App as returned by the API (secrets elided).
#[derive(Debug, Serialize, ToSchema)]
pub struct GithubAppDto {
    pub uuid: String,
    pub name: String,
    pub organization: Option<String>,
    pub api_url: String,
    pub html_url: String,
    pub custom_user: String,
    pub custom_port: i32,
    pub app_id: Option<i64>,
    pub installation_id: Option<i64>,
    pub client_id: Option<String>,
    pub is_system_wide: bool,
    pub is_public: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<GithubApp> for GithubAppDto {
    fn from(g: GithubApp) -> Self {
        Self {
            uuid: g.uuid,
            name: g.name,
            organization: g.organization,
            api_url: g.api_url,
            html_url: g.html_url,
            custom_user: g.custom_user,
            custom_port: g.custom_port,
            app_id: g.app_id,
            installation_id: g.installation_id,
            client_id: g.client_id,
            is_system_wide: g.is_system_wide,
            is_public: g.is_public,
            created_at: g.created_at,
            updated_at: g.updated_at,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct GithubAppCreate {
    pub name: String,
    pub api_url: Option<String>,
    pub html_url: Option<String>,
    pub organization: Option<String>,
    pub app_id: Option<i64>,
    pub installation_id: Option<i64>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub webhook_secret: Option<String>,
    /// uuid of an existing private key holding the App's RSA PEM.
    pub private_key_uuid: Option<String>,
    pub is_public: Option<bool>,
    pub is_system_wide: Option<bool>,
    pub custom_user: Option<String>,
    pub custom_port: Option<i32>,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct GithubAppUpdate {
    pub name: Option<String>,
    pub api_url: Option<String>,
    pub html_url: Option<String>,
    pub organization: Option<String>,
    pub app_id: Option<i64>,
    pub installation_id: Option<i64>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub webhook_secret: Option<String>,
    pub private_key_uuid: Option<String>,
    pub is_public: Option<bool>,
    pub is_system_wide: Option<bool>,
    pub custom_user: Option<String>,
    pub custom_port: Option<i32>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RepositoriesResponse {
    pub repositories: Vec<serde_json::Value>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BranchesResponse {
    pub branches: Vec<serde_json::Value>,
}

// ----- ownership helper ---------------------------------------------------

async fn owned(state: &AppState, team: &CurrentTeam, uuid: &str) -> ApiResult<GithubApp> {
    let gh = GithubAppRepo::new(state.pool.clone())
        .get_by_uuid(uuid)
        .await?
        .ok_or(ApiError::NotFound)?;
    if gh.team_id != team.id {
        return Err(ApiError::NotFound);
    }
    Ok(gh)
}

/// Resolve a team-owned `private_key_uuid` to its numeric id.
async fn resolve_key(state: &AppState, team: &CurrentTeam, uuid: &str) -> ApiResult<i64> {
    let key = KeyRepo::new(state.pool.clone())
        .get_by_uuid(uuid)
        .await?
        .filter(|k| k.team_id == team.id)
        .ok_or_else(|| ApiError::Validation("unknown private_key_uuid".into()))?;
    Ok(key.id)
}

// ----- CRUD ---------------------------------------------------------------

#[utoipa::path(get, path = "/github-apps", operation_id = "list_github_apps", tag = "github-apps",
    responses((status = 200, description = "List of GitHub apps", body = [GithubAppDto])))]
pub async fn list(
    State(state): State<AppState>,
    team: CurrentTeam,
) -> ApiResult<Json<Vec<GithubAppDto>>> {
    let apps = GithubAppRepo::new(state.pool.clone()).list(team.id).await?;
    Ok(Json(apps.into_iter().map(GithubAppDto::from).collect()))
}

#[utoipa::path(post, path = "/github-apps", operation_id = "create_github_app", tag = "github-apps",
    request_body = GithubAppCreate,
    responses(
        (status = 201, description = "GitHub app created", body = GithubAppDto),
        (status = 422, description = "Validation error", body = crate::error::ApiErrorBody),
    ))]
pub async fn create(
    State(state): State<AppState>,
    team: CurrentTeam,
    Json(body): Json<GithubAppCreate>,
) -> ApiResult<Response> {
    let private_key_id = match &body.private_key_uuid {
        Some(uuid) => Some(resolve_key(&state, &team, uuid).await?),
        None => None,
    };
    let gh = GithubAppRepo::new(state.pool.clone())
        .create(NewGithubApp {
            team_id: team.id,
            name: body.name,
            organization: body.organization,
            api_url: body.api_url,
            html_url: body.html_url,
            custom_user: body.custom_user,
            custom_port: body.custom_port,
            app_id: body.app_id,
            installation_id: body.installation_id,
            client_id: body.client_id,
            client_secret: body.client_secret,
            webhook_secret: body.webhook_secret,
            private_key_id,
            is_public: body.is_public,
            is_system_wide: body.is_system_wide,
        })
        .await?;
    Ok((StatusCode::CREATED, Json(GithubAppDto::from(gh))).into_response())
}

#[utoipa::path(get, path = "/github-apps/{uuid}", operation_id = "get_github_app", tag = "github-apps",
    params(("uuid" = String, Path, description = "GitHub app uuid")),
    responses(
        (status = 200, description = "The GitHub app", body = GithubAppDto),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
    ))]
pub async fn get(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Json<GithubAppDto>> {
    let gh = owned(&state, &team, &uuid).await?;
    Ok(Json(GithubAppDto::from(gh)))
}

#[utoipa::path(patch, path = "/github-apps/{uuid}", operation_id = "update_github_app", tag = "github-apps",
    params(("uuid" = String, Path, description = "GitHub app uuid")),
    request_body = GithubAppUpdate,
    responses(
        (status = 200, description = "Updated GitHub app", body = GithubAppDto),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
    ))]
pub async fn update(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
    Json(body): Json<GithubAppUpdate>,
) -> ApiResult<Json<GithubAppDto>> {
    owned(&state, &team, &uuid).await?;
    let private_key_id = match &body.private_key_uuid {
        Some(u) => Some(resolve_key(&state, &team, u).await?),
        None => None,
    };
    let patch = GithubAppPatch {
        name: body.name,
        organization: body.organization,
        api_url: body.api_url,
        html_url: body.html_url,
        custom_user: body.custom_user,
        custom_port: body.custom_port,
        app_id: body.app_id,
        installation_id: body.installation_id,
        client_id: body.client_id,
        client_secret: body.client_secret,
        webhook_secret: body.webhook_secret,
        private_key_id,
        is_public: body.is_public,
        is_system_wide: body.is_system_wide,
    };
    let gh = GithubAppRepo::new(state.pool.clone())
        .update(&uuid, &patch)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(GithubAppDto::from(gh)))
}

#[utoipa::path(delete, path = "/github-apps/{uuid}", operation_id = "delete_github_app", tag = "github-apps",
    params(("uuid" = String, Path, description = "GitHub app uuid")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
    ))]
pub async fn delete(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<StatusCode> {
    owned(&state, &team, &uuid).await?;
    if GithubAppRepo::new(state.pool.clone()).delete(&uuid).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

// ----- GitHub-backed reads ------------------------------------------------

/// Build the [`GithubAppRow`] (with decrypted RSA PEM) needed to mint tokens.
async fn app_row(state: &AppState, gh: &GithubApp) -> ApiResult<GithubAppRow> {
    let pk_id = gh
        .private_key_id
        .ok_or_else(|| ApiError::Validation("github app has no private key".into()))?;
    let pem = KeyRepo::new(state.pool.clone())
        .decrypt_private_key(pk_id)
        .await
        .map_err(|e| ApiError::Internal(format!("private key: {e}")))?;
    Ok(GithubAppRow {
        id: gh.id,
        app_id: gh.app_id.unwrap_or(0),
        installation_id: gh.installation_id.unwrap_or(0),
        api_url: gh.api_url.clone(),
        private_key_pem: pem,
    })
}

/// A GitHub-authenticated request builder (parity with Coolify's `Http::GitHub`
/// macro at AppServiceProvider.php:68): v3 accept + api-version + bearer token.
fn github_get(client: &reqwest::Client, url: &str, token: &str) -> reqwest::RequestBuilder {
    client
        .get(url)
        .header("Accept", "application/vnd.github.v3+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("Authorization", format!("Bearer {token}"))
}

#[utoipa::path(get, path = "/github-apps/{uuid}/repositories", operation_id = "github_app_repositories",
    tag = "github-apps",
    params(("uuid" = String, Path, description = "GitHub app uuid")),
    responses((status = 200, description = "Repositories", body = RepositoriesResponse)))]
pub async fn repositories(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Json<RepositoriesResponse>> {
    let gh = owned(&state, &team, &uuid).await?;
    let row = app_row(&state, &gh).await?;
    let client = reqwest::Client::new();
    let token = github::installation_token(&client, &row, Utc::now())
        .await
        .map_err(|e| ApiError::Internal(format!("github token: {e}")))?;

    let api_url = gh.api_url.trim_end_matches('/');
    let mut repos: Vec<serde_json::Value> = Vec::new();
    // Safety limit: 100 pages of 100 repos (GithubController::load_repositories).
    for page in 1..=100 {
        let url = format!("{api_url}/installation/repositories?per_page=100&page={page}");
        let resp = github_get(&client, &url, &token)
            .send()
            .await
            .map_err(|e| ApiError::Internal(format!("github request: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
            let msg = body
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("failed to load repositories");
            return Err(ApiError::Internal(format!("github api {status}: {msg}")));
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ApiError::Internal(format!("github json: {e}")))?;
        let page_repos = body
            .get("repositories")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();
        if page_repos.is_empty() {
            break;
        }
        repos.extend(page_repos);
    }
    Ok(Json(RepositoriesResponse {
        repositories: repos,
    }))
}

#[utoipa::path(get, path = "/github-apps/{uuid}/repositories/{owner}/{repo}/branches",
    operation_id = "github_app_branches", tag = "github-apps",
    params(
        ("uuid" = String, Path, description = "GitHub app uuid"),
        ("owner" = String, Path, description = "Repository owner"),
        ("repo" = String, Path, description = "Repository name"),
    ),
    responses((status = 200, description = "Branches", body = BranchesResponse)))]
pub async fn branches(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path((uuid, owner, repo)): Path<(String, String, String)>,
) -> ApiResult<Json<BranchesResponse>> {
    let gh = owned(&state, &team, &uuid).await?;
    let row = app_row(&state, &gh).await?;
    let client = reqwest::Client::new();
    let token = github::installation_token(&client, &row, Utc::now())
        .await
        .map_err(|e| ApiError::Internal(format!("github token: {e}")))?;

    let api_url = gh.api_url.trim_end_matches('/');
    let url = format!("{api_url}/repos/{owner}/{repo}/branches?per_page=100");
    let resp = github_get(&client, &url, &token)
        .send()
        .await
        .map_err(|e| ApiError::Internal(format!("github request: {e}")))?;
    if !resp.status().is_success() {
        let status = resp.status();
        return Err(ApiError::Internal(format!("github api {status}")));
    }
    let branches: Vec<serde_json::Value> = resp
        .json()
        .await
        .map_err(|e| ApiError::Internal(format!("github json: {e}")))?;
    Ok(Json(BranchesResponse { branches }))
}

// ----- app-manifest setup-state cache -------------------------------------

/// A single-use setup-state payload (Webhook/Github.php consumeGithubAppSetupState).
#[derive(Debug, Clone)]
struct SetupState {
    action: String,
    github_app_id: i64,
    team_id: i64,
    expires_at: DateTime<Utc>,
}

type StateMap = Mutex<HashMap<String, SetupState>>;

fn state_map() -> &'static StateMap {
    static MAP: OnceLock<StateMap> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `github-app-setup-state:{sha256(state)}` (parity with the Coolify cache key).
fn setup_state_key(state: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(state.as_bytes());
    format!("github-app-setup-state:{:x}", hasher.finalize())
}

/// Store a setup state for 60 minutes and return the random `state` token.
///
/// This is the flow-initiation seam: the UI calls it (via a thin handler) to
/// begin either the app-manifest (`action = "manifest"`) or installation
/// (`action = "install"`) exchange, embedding the returned token as the GitHub
/// `state` query parameter so [`redirect`] / [`install`] can consume it.
pub fn store_setup_state(
    action: &str,
    github_app_id: i64,
    team_id: i64,
    now: DateTime<Utc>,
) -> String {
    let raw: String = {
        use rand::Rng as _;
        let mut rng = rand::thread_rng();
        (0..64)
            .map(|_| {
                const CHARS: &[u8] =
                    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
                CHARS[rng.gen_range(0..CHARS.len())] as char
            })
            .collect()
    };
    if let Ok(mut map) = state_map().lock() {
        map.insert(
            setup_state_key(&raw),
            SetupState {
                action: action.to_string(),
                github_app_id,
                team_id,
                expires_at: now + Duration::minutes(60),
            },
        );
    }
    raw
}

/// Consume (single-use) a setup state, enforcing action + team + TTL.
fn consume_setup_state(
    state: &str,
    action: &str,
    team_id: i64,
    now: DateTime<Utc>,
) -> Option<SetupState> {
    let mut map = state_map().lock().ok()?;
    let payload = map.remove(&setup_state_key(state))?;
    if payload.action != action || payload.team_id != team_id || payload.expires_at <= now {
        return None;
    }
    Some(payload)
}

// ----- app-manifest web flow ----------------------------------------------

#[derive(Debug, Deserialize)]
pub struct RedirectQuery {
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub state: String,
}

/// GET `/webhooks/source/github/redirect`: exchange the app-manifest `code` for
/// the App's credentials (Webhook/Github.php::redirect), persist them + a new
/// private key, then redirect back to the app's UI page.
pub async fn redirect(
    State(state): State<AppState>,
    team: CurrentTeam,
    Query(q): Query<RedirectQuery>,
) -> ApiResult<Response> {
    if q.code.is_empty() {
        return Err(ApiError::Validation("missing manifest code".into()));
    }
    let payload =
        consume_setup_state(&q.state, "manifest", team.id, Utc::now()).ok_or(ApiError::NotFound)?;
    let repo = GithubAppRepo::new(state.pool.clone());
    let gh = repo
        .get_by_id(payload.github_app_id)
        .await?
        .filter(|g| g.team_id == team.id)
        .ok_or(ApiError::NotFound)?;

    let api_url = gh.api_url.trim_end_matches('/');
    let url = format!("{api_url}/app-manifests/{}/conversions", q.code);
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| ApiError::Internal(format!("github request: {e}")))?;
    if !resp.status().is_success() {
        return Err(ApiError::Internal(format!(
            "manifest conversion failed: {}",
            resp.status()
        )));
    }
    let data: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ApiError::Internal(format!("github json: {e}")))?;

    let app_id = data.get("id").and_then(|v| v.as_i64());
    let slug = data.get("slug").and_then(|v| v.as_str());
    let client_id = data.get("client_id").and_then(|v| v.as_str());
    let client_secret = data.get("client_secret").and_then(|v| v.as_str());
    let pem = data.get("pem").and_then(|v| v.as_str());
    let webhook_secret = data.get("webhook_secret").and_then(|v| v.as_str());
    let (
        Some(app_id),
        Some(slug),
        Some(client_id),
        Some(client_secret),
        Some(pem),
        Some(webhook_secret),
    ) = (app_id, slug, client_id, client_secret, pem, webhook_secret)
    else {
        return Err(ApiError::Internal(
            "manifest conversion response is incomplete".into(),
        ));
    };

    // Store the RSA PEM as a private key (no SSH public key — RSA JWT only).
    let key = KeyRepo::new(state.pool.clone())
        .create(team.id, &format!("github-app-{slug}"), pem, "")
        .await?;
    repo.set_manifest_credentials(
        &gh.uuid,
        slug,
        app_id,
        client_id,
        client_secret,
        webhook_secret,
        key.id,
    )
    .await?;

    Ok(Redirect::to(&format!("/sources/github/{}", gh.uuid)).into_response())
}

#[derive(Debug, Deserialize)]
pub struct InstallQuery {
    #[serde(default)]
    pub installation_id: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub setup_action: String,
}

/// GET `/webhooks/source/github/install`: verify the installation id against the
/// App's own credentials (Webhook/Github.php::install), then persist it.
pub async fn install(
    State(state): State<AppState>,
    team: CurrentTeam,
    Query(q): Query<InstallQuery>,
) -> ApiResult<Response> {
    if q.setup_action != "install" && q.setup_action != "update" {
        return Err(ApiError::Validation("invalid setup_action".into()));
    }
    let installation_id: i64 = q
        .installation_id
        .parse()
        .map_err(|_| ApiError::Validation("missing installation_id".into()))?;
    let payload =
        consume_setup_state(&q.state, "install", team.id, Utc::now()).ok_or(ApiError::NotFound)?;
    let repo = GithubAppRepo::new(state.pool.clone());
    let gh = repo
        .get_by_id(payload.github_app_id)
        .await?
        .filter(|g| g.team_id == team.id)
        .ok_or(ApiError::NotFound)?;

    // Verify the installation belongs to this App (githubInstallationBelongsToApp).
    let row = app_row(&state, &gh).await?;
    let jwt = rustify_core::github_jwt::app_jwt(row.app_id, &row.private_key_pem, Utc::now())
        .map_err(|e| ApiError::Internal(format!("jwt: {e}")))?;
    let api_url = gh.api_url.trim_end_matches('/');
    let url = format!("{api_url}/app/installations/{installation_id}");
    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("Authorization", format!("Bearer {jwt}"))
        .send()
        .await
        .map_err(|e| ApiError::Internal(format!("github request: {e}")))?;
    let verified = resp.status().is_success() && {
        let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
        body.get("app_id").and_then(|v| v.as_i64()) == gh.app_id
    };
    if !verified {
        return Err(ApiError::Validation(
            "installation could not be verified".into(),
        ));
    }

    repo.set_installation_id(&gh.uuid, installation_id).await?;
    Ok(Redirect::to(&format!("/sources/github/{}", gh.uuid)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_state_key_is_sha256_prefixed() {
        // sha256("abc") = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
        assert_eq!(
            setup_state_key("abc"),
            "github-app-setup-state:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn setup_state_is_single_use_and_ttl_enforced() {
        let now = Utc::now();
        let raw = store_setup_state("install", 7, 3, now);
        // wrong action / team is rejected
        assert!(consume_setup_state(&raw, "manifest", 3, now).is_none());
        // re-store since the wrong-action consume above removed it
        let raw = store_setup_state("install", 7, 3, now);
        assert!(consume_setup_state(&raw, "install", 99, now).is_none());
        let raw = store_setup_state("install", 7, 3, now);
        // correct consume succeeds exactly once
        let got = consume_setup_state(&raw, "install", 3, now).expect("first consume");
        assert_eq!(got.github_app_id, 7);
        assert!(
            consume_setup_state(&raw, "install", 3, now).is_none(),
            "single use"
        );
    }

    #[test]
    fn expired_state_is_rejected() {
        let now = Utc::now();
        let raw = store_setup_state("manifest", 1, 1, now);
        assert!(
            consume_setup_state(&raw, "manifest", 1, now + Duration::minutes(61)).is_none(),
            "state older than 60 minutes is rejected"
        );
    }
}
