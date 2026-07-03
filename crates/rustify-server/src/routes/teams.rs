//! Team management: teams, members, roles, invitations and team switching
//! (multi-tenancy §5). Ported from Coolify `app/Livewire/Team/*`,
//! `SwitchTeam.php`, `TeamPolicy.php` and the invitation-accept controller.
//!
//! Authorization here is scoped to the *target* team in the path (not the
//! caller's active team): a session's role is looked up per-team via the
//! `team_user` pivot, matching `TeamPolicy`. These routes therefore require a
//! session (a bearer token has no acting user for privilege checks).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use rustify_core::{Role, ids};
use rustify_db::repos::{
    ROOT_TEAM_ID, SettingsRepo, Team, TeamInvitation, TeamMember, TeamRepo, UserRepo,
};

use crate::app::AppState;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ApiResult};

#[derive(Debug, Serialize, ToSchema)]
pub struct TeamDto {
    pub id: i64,
    pub uuid: String,
    pub name: String,
    pub description: Option<String>,
    pub personal_team: bool,
    pub custom_server_limit: Option<i32>,
    /// The requesting user's role in this team, when known.
    pub role: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl TeamDto {
    fn from(team: Team, role: Option<Role>) -> Self {
        TeamDto {
            id: team.id,
            uuid: team.uuid,
            name: team.name,
            description: team.description,
            personal_team: team.personal_team,
            custom_server_limit: team.custom_server_limit,
            role: role.map(|r| r.to_string()),
            created_at: team.created_at,
            updated_at: team.updated_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MemberDto {
    pub uuid: String,
    pub email: String,
    pub name: String,
    pub role: String,
}

impl From<TeamMember> for MemberDto {
    fn from(m: TeamMember) -> Self {
        MemberDto {
            uuid: m.user_uuid,
            email: m.email,
            name: m.name,
            role: m.role,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct InvitationDto {
    pub uuid: String,
    pub email: String,
    pub role: String,
    pub via: String,
    pub link: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<TeamInvitation> for InvitationDto {
    fn from(i: TeamInvitation) -> Self {
        InvitationDto {
            uuid: i.uuid,
            email: i.email,
            role: i.role,
            via: i.via,
            link: i.link,
            created_at: i.created_at,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct TeamCreate {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct TeamUpdate {
    pub name: Option<String>,
    pub description: Option<String>,
    pub custom_server_limit: Option<i32>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct RoleUpdate {
    pub role: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct InvitationCreate {
    pub email: String,
    pub role: Option<String>,
    pub via: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct InvitationInfo {
    pub uuid: String,
    pub email: String,
    pub role: String,
    pub team_name: String,
    pub valid: bool,
    pub already_member: bool,
}

// --- helpers ---

/// The caller's role in `team_id`, or `NotFound` when they are not a member
/// (do not leak the existence of teams the caller cannot see — TeamPolicy::view).
async fn membership(state: &AppState, user: &CurrentUser, team_id: i64) -> ApiResult<Role> {
    TeamRepo::new(state.pool.clone())
        .role_in_team(user.id, team_id)
        .await?
        .ok_or(ApiError::NotFound)
}

/// The caller's role, requiring admin/owner (TeamPolicy::manage*). Returns
/// `NotFound` when not a member, `Forbidden` when a member but not admin.
async fn require_admin(state: &AppState, user: &CurrentUser, team_id: i64) -> ApiResult<Role> {
    let role = membership(state, user, team_id).await?;
    if role.is_admin() {
        Ok(role)
    } else {
        Err(ApiError::Forbidden(
            "Missing required team role.".to_string(),
        ))
    }
}

// --- teams ---

#[utoipa::path(get, path = "/teams", operation_id = "list_teams", tag = "teams",
    responses((status = 200, description = "Teams the user belongs to", body = [TeamDto])))]
pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
) -> ApiResult<Json<Vec<TeamDto>>> {
    let repo = TeamRepo::new(state.pool.clone());
    let teams = repo.list_for_user(user.id).await?;
    let mut out = Vec::with_capacity(teams.len());
    for t in teams {
        let role = repo.role_in_team(user.id, t.id).await?;
        out.push(TeamDto::from(t, role));
    }
    Ok(Json(out))
}

#[utoipa::path(get, path = "/teams/current", operation_id = "get_current_team", tag = "teams",
    responses((status = 200, description = "The active team", body = TeamDto)))]
pub async fn current(State(state): State<AppState>, user: CurrentUser) -> ApiResult<Json<TeamDto>> {
    let repo = TeamRepo::new(state.pool.clone());
    let team = repo
        .get_by_id(user.team_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let role = repo.role_in_team(user.id, team.id).await?;
    Ok(Json(TeamDto::from(team, role)))
}

#[utoipa::path(get, path = "/teams/current/members", operation_id = "list_current_members",
    tag = "teams", responses((status = 200, description = "Members of the active team", body = [MemberDto])))]
pub async fn current_members(
    State(state): State<AppState>,
    user: CurrentUser,
) -> ApiResult<Json<Vec<MemberDto>>> {
    membership(&state, &user, user.team_id).await?;
    let members = TeamRepo::new(state.pool.clone())
        .members(user.team_id)
        .await?;
    Ok(Json(members.into_iter().map(MemberDto::from).collect()))
}

#[utoipa::path(get, path = "/teams/{id}", operation_id = "get_team", tag = "teams",
    params(("id" = i64, Path, description = "Team id")),
    responses(
        (status = 200, description = "The team", body = TeamDto),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
    ))]
pub async fn get(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<TeamDto>> {
    let role = membership(&state, &user, id).await?;
    let team = TeamRepo::new(state.pool.clone())
        .get_by_id(id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(TeamDto::from(team, Some(role))))
}

#[utoipa::path(get, path = "/teams/{id}/members", operation_id = "list_members", tag = "teams",
    params(("id" = i64, Path, description = "Team id")),
    responses((status = 200, description = "Team members", body = [MemberDto])))]
pub async fn members(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<MemberDto>>> {
    membership(&state, &user, id).await?;
    let members = TeamRepo::new(state.pool.clone()).members(id).await?;
    Ok(Json(members.into_iter().map(MemberDto::from).collect()))
}

#[utoipa::path(post, path = "/teams", operation_id = "create_team", tag = "teams",
    request_body = TeamCreate,
    responses((status = 201, description = "Team created", body = TeamDto)))]
pub async fn create(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<TeamCreate>,
) -> ApiResult<Response> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(ApiError::Validation("name is required".to_string()));
    }
    let repo = TeamRepo::new(state.pool.clone());
    let team = repo.create_team(name, false).await?;
    if let Some(desc) = body.description.as_deref() {
        repo.update(team.id, None, Some(desc), None).await?;
    }
    // The creator joins their new team as an admin (§5).
    repo.add_member(team.id, user.id, Role::Admin).await?;
    let team = repo.get_by_id(team.id).await?.ok_or(ApiError::NotFound)?;
    Ok((
        StatusCode::CREATED,
        Json(TeamDto::from(team, Some(Role::Admin))),
    )
        .into_response())
}

#[utoipa::path(patch, path = "/teams/{id}", operation_id = "update_team", tag = "teams",
    params(("id" = i64, Path, description = "Team id")),
    request_body = TeamUpdate,
    responses((status = 200, description = "Updated team", body = TeamDto)))]
pub async fn update(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
    Json(body): Json<TeamUpdate>,
) -> ApiResult<Json<TeamDto>> {
    let role = require_admin(&state, &user, id).await?;
    let repo = TeamRepo::new(state.pool.clone());
    let team = repo
        .update(
            id,
            body.name.as_deref(),
            body.description.as_deref(),
            body.custom_server_limit,
        )
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(TeamDto::from(team, Some(role))))
}

#[utoipa::path(delete, path = "/teams/{id}", operation_id = "delete_team", tag = "teams",
    params(("id" = i64, Path, description = "Team id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Forbidden", body = crate::error::ApiErrorBody),
    ))]
pub async fn delete(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    require_admin(&state, &user, id).await?;
    if id == ROOT_TEAM_ID {
        return Err(ApiError::Forbidden(
            "The root team cannot be deleted.".to_string(),
        ));
    }
    if TeamRepo::new(state.pool.clone()).delete(id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

// --- members ---

#[utoipa::path(patch, path = "/teams/{id}/members/{user_uuid}", operation_id = "set_member_role",
    tag = "teams",
    params(("id" = i64, Path, description = "Team id"), ("user_uuid" = String, Path, description = "Member user uuid")),
    request_body = RoleUpdate,
    responses(
        (status = 200, description = "Updated member", body = MemberDto),
        (status = 403, description = "Forbidden", body = crate::error::ApiErrorBody),
    ))]
pub async fn set_member_role(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, user_uuid)): Path<(i64, String)>,
    Json(body): Json<RoleUpdate>,
) -> ApiResult<Json<MemberDto>> {
    let acting = require_admin(&state, &user, id).await?;
    let new_role = Role::from_str_coerce(&body.role);

    let target = UserRepo::new(state.pool.clone())
        .get_by_uuid(&user_uuid)
        .await?
        .ok_or(ApiError::NotFound)?;
    let repo = TeamRepo::new(state.pool.clone());
    let current = repo
        .role_in_team(target.id, id)
        .await?
        .ok_or(ApiError::NotFound)?;

    // Privilege guards (Coolify Team/Member.php): you cannot grant a role above
    // your own, nor manage a member who outranks you.
    if acting.lt(new_role) || current.gt(acting) {
        return Err(ApiError::Forbidden(
            "You are not authorized to perform this action.".to_string(),
        ));
    }

    if !repo.set_role(id, target.id, new_role).await? {
        return Err(ApiError::NotFound);
    }
    // A role change revokes the affected user's sessions so the new role takes
    // effect immediately (Coolify RevokeUserTeamTokens + cache clear).
    SettingsRepo::new(state.pool.clone())
        .revoke_user_sessions(target.id)
        .await?;

    Ok(Json(MemberDto {
        uuid: target.uuid,
        email: target.email,
        name: target.name,
        role: new_role.to_string(),
    }))
}

#[utoipa::path(delete, path = "/teams/{id}/members/{user_uuid}", operation_id = "remove_member",
    tag = "teams",
    params(("id" = i64, Path, description = "Team id"), ("user_uuid" = String, Path, description = "Member user uuid")),
    responses(
        (status = 204, description = "Removed"),
        (status = 403, description = "Forbidden", body = crate::error::ApiErrorBody),
    ))]
pub async fn remove_member(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, user_uuid)): Path<(i64, String)>,
) -> ApiResult<StatusCode> {
    let acting = require_admin(&state, &user, id).await?;
    let target = UserRepo::new(state.pool.clone())
        .get_by_uuid(&user_uuid)
        .await?
        .ok_or(ApiError::NotFound)?;
    let repo = TeamRepo::new(state.pool.clone());
    let current = repo
        .role_in_team(target.id, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    if current.gt(acting) {
        return Err(ApiError::Forbidden(
            "You are not authorized to perform this action.".to_string(),
        ));
    }
    if !repo.remove_member(id, target.id).await? {
        return Err(ApiError::Forbidden(
            "The last member of the root team cannot be removed.".to_string(),
        ));
    }
    SettingsRepo::new(state.pool.clone())
        .revoke_user_sessions(target.id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- invitations ---

#[utoipa::path(post, path = "/teams/{id}/invitations", operation_id = "create_invitation",
    tag = "teams", params(("id" = i64, Path, description = "Team id")),
    request_body = InvitationCreate,
    responses(
        (status = 201, description = "Invitation created", body = InvitationDto),
        (status = 403, description = "Forbidden", body = crate::error::ApiErrorBody),
        (status = 409, description = "Already a member / pending", body = crate::error::ApiErrorBody),
    ))]
pub async fn create_invitation(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
    Json(body): Json<InvitationCreate>,
) -> ApiResult<Response> {
    let acting = require_admin(&state, &user, id).await?;
    let email = body.email.trim().to_lowercase();
    if !email.contains('@') {
        return Err(ApiError::Validation(
            "a valid email is required".to_string(),
        ));
    }
    let role = Role::from_str_coerce(body.role.as_deref().unwrap_or("member"));
    let via = match body.via.as_deref().unwrap_or("link") {
        "email" => "email",
        "link" => "link",
        other => return Err(ApiError::Validation(format!("unknown via: {other}"))),
    };

    // Privilege guards (Coolify InviteLink.php): admins cannot invite owners.
    if acting.lt(role) {
        return Err(ApiError::Forbidden(
            "You cannot invite a member with a higher role than your own.".to_string(),
        ));
    }

    let repo = TeamRepo::new(state.pool.clone());
    // Reject inviting an existing member.
    if repo
        .members(id)
        .await?
        .iter()
        .any(|m| m.email.eq_ignore_ascii_case(&email))
    {
        return Err(ApiError::Conflict(format!(
            "{email} is already a member of this team."
        )));
    }
    // Replace an expired invitation; reject a still-valid one.
    if let Some(existing) = repo.find_invitation_for_email(id, &email).await? {
        if existing.is_valid() {
            return Err(ApiError::Conflict(format!(
                "A pending invitation already exists for {email}."
            )));
        }
        repo.delete_invitation(&existing.uuid).await?;
    }

    let uuid = ids::new_invitation_uuid();
    let link = format!("/invitations/{uuid}");
    let invitation = repo
        .create_invitation(id, &uuid, &email, role, Some(&link), via)
        .await?;
    Ok((StatusCode::CREATED, Json(InvitationDto::from(invitation))).into_response())
}

#[utoipa::path(get, path = "/teams/{id}/invitations", operation_id = "list_invitations",
    tag = "teams", params(("id" = i64, Path, description = "Team id")),
    responses((status = 200, description = "Pending invitations", body = [InvitationDto])))]
pub async fn list_invitations(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<InvitationDto>>> {
    require_admin(&state, &user, id).await?;
    let invitations = TeamRepo::new(state.pool.clone())
        .list_invitations(id)
        .await?;
    Ok(Json(
        invitations.into_iter().map(InvitationDto::from).collect(),
    ))
}

#[utoipa::path(delete, path = "/invitations/{uuid}", operation_id = "delete_invitation",
    tag = "teams", params(("uuid" = String, Path, description = "Invitation uuid")),
    responses((status = 204, description = "Deleted")))]
pub async fn delete_invitation(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(uuid): Path<String>,
) -> ApiResult<StatusCode> {
    let repo = TeamRepo::new(state.pool.clone());
    let invitation = repo
        .get_invitation(&uuid)
        .await?
        .ok_or(ApiError::NotFound)?;
    require_admin(&state, &user, invitation.team_id).await?;
    if repo.delete_invitation(&uuid).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

#[utoipa::path(get, path = "/invitations/{uuid}", operation_id = "get_invitation", tag = "teams",
    params(("uuid" = String, Path, description = "Invitation uuid")),
    responses((status = 200, description = "Invitation info", body = InvitationInfo)))]
pub async fn get_invitation(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(uuid): Path<String>,
) -> ApiResult<Json<InvitationInfo>> {
    let repo = TeamRepo::new(state.pool.clone());
    let invitation = repo
        .get_invitation(&uuid)
        .await?
        .ok_or(ApiError::NotFound)?;
    let team = repo
        .get_by_id(invitation.team_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let already_member = repo.is_member(invitation.team_id, user.id).await?;
    let valid = invitation.is_valid();
    Ok(Json(InvitationInfo {
        uuid: invitation.uuid,
        email: invitation.email,
        role: invitation.role,
        team_name: team.name,
        valid,
        already_member,
    }))
}

#[utoipa::path(post, path = "/invitations/{uuid}", operation_id = "accept_invitation", tag = "teams",
    params(("uuid" = String, Path, description = "Invitation uuid")),
    responses(
        (status = 200, description = "Accepted; active team switched", body = TeamDto),
        (status = 400, description = "Expired invitation", body = crate::error::ApiErrorBody),
        (status = 403, description = "Not your invitation", body = crate::error::ApiErrorBody),
    ))]
pub async fn accept_invitation(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(uuid): Path<String>,
) -> ApiResult<Json<TeamDto>> {
    let repo = TeamRepo::new(state.pool.clone());
    let invitation = repo
        .get_invitation(&uuid)
        .await?
        .ok_or(ApiError::NotFound)?;

    // Only the invited user may accept (Coolify acceptInvitation).
    if !user.email.eq_ignore_ascii_case(&invitation.email) {
        return Err(ApiError::Forbidden(
            "You are not allowed to accept this invitation.".to_string(),
        ));
    }
    if !invitation.is_valid() {
        repo.delete_invitation(&uuid).await?;
        return Err(ApiError::Validation("Invitation expired.".to_string()));
    }

    let team_id = repo
        .accept_invitation(&uuid, user.id)
        .await?
        .ok_or(ApiError::NotFound)?;
    // Switch the accepting user's active team (refreshSession).
    UserRepo::new(state.pool.clone())
        .set_active_team(user.id, team_id)
        .await?;
    let role = repo.role_in_team(user.id, team_id).await?;
    let team = repo.get_by_id(team_id).await?.ok_or(ApiError::NotFound)?;
    Ok(Json(TeamDto::from(team, role)))
}

// --- switch ---

#[utoipa::path(post, path = "/teams/{id}/switch", operation_id = "switch_team", tag = "teams",
    params(("id" = i64, Path, description = "Team id")),
    responses(
        (status = 200, description = "Active team switched", body = TeamDto),
        (status = 404, description = "Not a member", body = crate::error::ApiErrorBody),
    ))]
pub async fn switch_team(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<TeamDto>> {
    let repo = TeamRepo::new(state.pool.clone());
    // Verify membership before switching (SwitchTeam::switch_to).
    let role = repo
        .role_in_team(user.id, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    UserRepo::new(state.pool.clone())
        .set_active_team(user.id, id)
        .await?;
    let team = repo.get_by_id(id).await?.ok_or(ApiError::NotFound)?;
    Ok(Json(TeamDto::from(team, Some(role))))
}
