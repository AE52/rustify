//! Session-cookie and bearer-token authentication.
//!
//! - Session: random 32-byte token stored as the `sessions.id`, delivered as
//!   the `rustify_session` cookie (`HttpOnly`, `SameSite=Lax`, 30-day expiry,
//!   `Secure` in production). Passwords are argon2id-verified by `rustify-db`.
//! - Bearer: `Authorization: Bearer <token>` → sha256-hex lookup in
//!   `api_tokens.token_hash`.
//!
//! `CurrentUser` and `CurrentTeam` are axum extractors that run this resolution
//! and inject the authenticated principal; every route except `/health` and
//! `/auth/login` requires one of them.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, header};
use rand::RngCore;
use rustify_core::Role;
use rustify_db::repos::{ApiToken, SettingsRepo, TeamRepo, User, UserRepo};
use sha2::{Digest, Sha256};

use crate::app::AppState;
use crate::error::ApiError;

/// Session cookie name (contract F: `rustify_session`).
pub const SESSION_COOKIE: &str = "rustify_session";
/// Session lifetime in days.
pub const SESSION_TTL_DAYS: i64 = 30;

/// Generate a fresh opaque token: 32 random bytes, hex-encoded (64 chars).
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// sha256 hex digest of `input` — how bearer tokens are stored/looked up.
pub fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Build the `Set-Cookie` value that installs a session cookie.
pub fn session_cookie(token: &str, secure: bool) -> String {
    let mut c = format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
        SESSION_TTL_DAYS * 24 * 60 * 60
    );
    if secure {
        c.push_str("; Secure");
    }
    c
}

/// Build the `Set-Cookie` value that clears the session cookie.
pub fn clear_cookie(secure: bool) -> String {
    let mut c = format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0");
    if secure {
        c.push_str("; Secure");
    }
    c
}

/// Extract a named cookie value from a `Cookie` header.
pub fn read_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for pair in raw.split(';') {
        let pair = pair.trim();
        if let Some((k, v)) = pair.split_once('=') {
            if k.trim() == name {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

/// Extract a bearer token from an `Authorization` header.
pub fn read_bearer(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    raw.strip_prefix("Bearer ").map(|t| t.trim().to_string())
}

/// The authenticated principal: either a logged-in user (session) or a team
/// scoped by an API token.
pub enum Principal {
    Session(User),
    Token(ApiToken),
}

impl Principal {
    pub fn team_id(&self) -> i64 {
        match self {
            Principal::Session(u) => u.team_id,
            Principal::Token(t) => t.team_id,
        }
    }
}

/// The role a bearer token carries in its team. API tokens have no per-user
/// pivot in this schema, so their effective role is derived from their
/// abilities (parity with Coolify's `EnsureTokenBelongsToCurrentTeamMember`,
/// which rejects write-capable tokens whose member role is not admin/owner):
/// write-capable tokens act as admins, read/deploy-only tokens as members.
pub fn token_role(abilities: &[String]) -> Role {
    let write_capable = abilities
        .iter()
        .any(|a| a == "root" || a == "write" || a == "write:sensitive");
    if write_capable {
        Role::Admin
    } else {
        Role::Member
    }
}

/// Resolve a session token to its (non-expired) user.
pub async fn resolve_session(state: &AppState, token: &str) -> Result<User, ApiError> {
    let settings = SettingsRepo::new(state.pool.clone());
    let session = settings
        .get_session(token)
        .await?
        .ok_or(ApiError::Unauthorized)?;
    UserRepo::new(state.pool.clone())
        .get_by_id(session.user_id)
        .await?
        .ok_or(ApiError::Unauthorized)
}

/// Resolve a bearer token to its `api_tokens` row.
pub async fn resolve_bearer(state: &AppState, raw: &str) -> Result<ApiToken, ApiError> {
    let hash = sha256_hex(raw);
    SettingsRepo::new(state.pool.clone())
        .find_api_token_by_hash(&hash)
        .await?
        .ok_or(ApiError::Unauthorized)
}

/// Resolve the principal from a request's headers (bearer wins over cookie).
pub async fn authenticate(state: &AppState, headers: &HeaderMap) -> Result<Principal, ApiError> {
    if let Some(raw) = read_bearer(headers) {
        let token = resolve_bearer(state, &raw).await?;
        return Ok(Principal::Token(token));
    }
    if let Some(token) = read_cookie(headers, SESSION_COOKIE) {
        let user = resolve_session(state, &token).await?;
        return Ok(Principal::Session(user));
    }
    Err(ApiError::Unauthorized)
}

/// Resolve the active team id and the caller's role in that team.
///
/// - Session: the user's active team (`users.team_id`); role from the
///   `team_user` pivot (defaulting to `member` when no pivot row exists).
/// - Bearer: the token's `team_id`; role derived from the token abilities.
pub async fn resolve_team_role(
    state: &AppState,
    principal: &Principal,
) -> Result<(i64, Role), ApiError> {
    match principal {
        Principal::Session(u) => {
            let role = TeamRepo::new(state.pool.clone())
                .role_in_team(u.id, u.team_id)
                .await?
                .unwrap_or(Role::Member);
            Ok((u.team_id, role))
        }
        Principal::Token(t) => Ok((t.team_id, token_role(&t.abilities))),
    }
}

/// A logged-in user (session only — bearer tokens have no user context).
#[derive(Debug, Clone)]
pub struct CurrentUser {
    pub id: i64,
    pub uuid: String,
    pub email: String,
    pub name: String,
    pub team_id: i64,
}

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        match authenticate(state, &parts.headers).await? {
            Principal::Session(u) => Ok(CurrentUser {
                id: u.id,
                uuid: u.uuid,
                email: u.email,
                name: u.name,
                team_id: u.team_id,
            }),
            Principal::Token(_) => Err(ApiError::Unauthorized),
        }
    }
}

/// The team scope of the request, resolved from either a session or a token,
/// carrying the caller's role in that team.
#[derive(Debug, Clone)]
pub struct CurrentTeam {
    pub id: i64,
    pub uuid: String,
    /// The caller's role in this team (from the pivot for sessions, from token
    /// abilities for bearer tokens).
    pub role: Role,
}

impl CurrentTeam {
    /// True when the caller may create/update/delete/manage in this team.
    pub fn is_admin(&self) -> bool {
        self.role.is_admin()
    }
}

impl FromRequestParts<AppState> for CurrentTeam {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let principal = authenticate(state, &parts.headers).await?;
        let (team_id, role) = resolve_team_role(state, &principal).await?;
        let team = TeamRepo::new(state.pool.clone())
            .get_by_id(team_id)
            .await?
            .ok_or(ApiError::Unauthorized)?;
        Ok(CurrentTeam {
            id: team.id,
            uuid: team.uuid,
            role,
        })
    }
}

/// Guard extractor for mutating routes: succeeds only when the caller is an
/// admin or owner of the active team (write-capable tokens qualify). Rejects
/// with `403 Missing required team role.` otherwise. Parity with Coolify's
/// `EnsureTokenBelongsToCurrentTeamMember` + the admin-only team policies.
#[derive(Debug, Clone)]
pub struct RequireAdmin {
    pub team: CurrentTeam,
}

impl FromRequestParts<AppState> for RequireAdmin {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let team = CurrentTeam::from_request_parts(parts, state).await?;
        if team.is_admin() {
            Ok(RequireAdmin { team })
        } else {
            Err(ApiError::Forbidden(
                "Missing required team role.".to_string(),
            ))
        }
    }
}
