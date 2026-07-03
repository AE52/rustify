//! Teams aggregate + multi-tenancy: the `team_user` membership pivot and
//! `team_invitations`. Ported from Coolify `app/Models/Team.php`,
//! `TeamInvitation.php`, `app/Livewire/Team/*` and `SwitchTeam.php`.
//!
//! `users.team_id` is the user's CURRENT/active team pointer; membership and
//! roles live in `team_user`.

use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;

use rustify_core::{Role, ids};

use crate::DbResult;

/// Invitation link/email validity window (Coolify
/// `config('constants.invitation.link.expiration_days')` = 3).
pub const INVITATION_EXPIRATION_DAYS: i64 = 3;

/// The instance-wide root team. `id = 0` has unlimited limits and cannot be
/// deleted down to its last member.
pub const ROOT_TEAM_ID: i64 = 0;

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct Team {
    pub id: i64,
    pub uuid: String,
    pub name: String,
    pub description: Option<String>,
    pub personal_team: bool,
    pub custom_server_limit: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLS: &str =
    "id, uuid, name, description, personal_team, custom_server_limit, created_at, updated_at";

/// A member of a team: the user's identity plus their pivot role.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct TeamMember {
    pub user_id: i64,
    pub user_uuid: String,
    pub email: String,
    pub name: String,
    pub role: String,
    pub joined_at: DateTime<Utc>,
}

impl TeamMember {
    pub fn role(&self) -> Role {
        Role::from_str_coerce(&self.role)
    }
}

/// A pending invitation to join a team.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct TeamInvitation {
    pub id: i64,
    pub uuid: String,
    pub team_id: i64,
    pub email: String,
    pub role: String,
    pub link: Option<String>,
    pub via: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TeamInvitation {
    pub fn role(&self) -> Role {
        Role::from_str_coerce(&self.role)
    }

    /// True while within the expiration window (Coolify `TeamInvitation::isValid`).
    pub fn is_valid(&self) -> bool {
        Utc::now() <= self.created_at + Duration::days(INVITATION_EXPIRATION_DAYS)
    }
}

const INVITATION_COLS: &str = "id, uuid, team_id, email, role, link, via, created_at, updated_at";

#[derive(Clone)]
pub struct TeamRepo {
    pool: PgPool,
}

impl TeamRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    // --- teams CRUD ---

    /// Create a team. `personal` marks it as an auto-created personal team.
    pub async fn create_team(&self, name: &str, personal: bool) -> DbResult<Team> {
        let uuid = ids::new_uuid();
        let row = sqlx::query_as::<_, Team>(&format!(
            "INSERT INTO teams (uuid, name, personal_team) VALUES ($1, $2, $3) RETURNING {COLS}"
        ))
        .bind(&uuid)
        .bind(name)
        .bind(personal)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    /// Back-compat helper used by the seed/tests: create a personal team.
    pub async fn create(&self, name: &str) -> DbResult<Team> {
        self.create_team(name, true).await
    }

    pub async fn get_by_id(&self, id: i64) -> DbResult<Option<Team>> {
        let row = sqlx::query_as::<_, Team>(&format!("SELECT {COLS} FROM teams WHERE id = $1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }

    pub async fn get_by_uuid(&self, uuid: &str) -> DbResult<Option<Team>> {
        let row = sqlx::query_as::<_, Team>(&format!("SELECT {COLS} FROM teams WHERE uuid = $1"))
            .bind(uuid)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }

    /// Teams the user belongs to (via the `team_user` pivot), lowest id first.
    pub async fn list_for_user(&self, user_id: i64) -> DbResult<Vec<Team>> {
        let rows = sqlx::query_as::<_, Team>(&format!(
            "SELECT {} FROM teams t
             JOIN team_user tu ON tu.team_id = t.id
             WHERE tu.user_id = $1 ORDER BY t.id",
            COLS.split(", ")
                .map(|c| format!("t.{c}"))
                .collect::<Vec<_>>()
                .join(", ")
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn update(
        &self,
        id: i64,
        name: Option<&str>,
        description: Option<&str>,
        custom_server_limit: Option<i32>,
    ) -> DbResult<Option<Team>> {
        let row = sqlx::query_as::<_, Team>(&format!(
            "UPDATE teams SET
                name = COALESCE($2, name),
                description = COALESCE($3, description),
                custom_server_limit = COALESCE($4, custom_server_limit),
                updated_at = now()
             WHERE id = $1 RETURNING {COLS}"
        ))
        .bind(id)
        .bind(name)
        .bind(description)
        .bind(custom_server_limit)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Delete a team. Refuses the root team (`id = 0`). Cascade removes the
    /// pivot/invitation rows.
    pub async fn delete(&self, id: i64) -> DbResult<bool> {
        if id == ROOT_TEAM_ID {
            return Ok(false);
        }
        let result = sqlx::query("DELETE FROM teams WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }

    // --- membership ---

    pub async fn members(&self, team_id: i64) -> DbResult<Vec<TeamMember>> {
        let rows = sqlx::query_as::<_, TeamMember>(
            "SELECT u.id AS user_id, u.uuid AS user_uuid, u.email, u.name,
                    tu.role, tu.created_at AS joined_at
             FROM team_user tu JOIN users u ON u.id = tu.user_id
             WHERE tu.team_id = $1 ORDER BY tu.id",
        )
        .bind(team_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Idempotently attach a user to a team with `role`.
    pub async fn add_member(&self, team_id: i64, user_id: i64, role: Role) -> DbResult<()> {
        sqlx::query(
            "INSERT INTO team_user (team_id, user_id, role) VALUES ($1, $2, $3)
             ON CONFLICT (team_id, user_id) DO NOTHING",
        )
        .bind(team_id)
        .bind(user_id)
        .bind(role.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn set_role(&self, team_id: i64, user_id: i64, role: Role) -> DbResult<bool> {
        let result = sqlx::query(
            "UPDATE team_user SET role = $3, updated_at = now()
             WHERE team_id = $1 AND user_id = $2",
        )
        .bind(team_id)
        .bind(user_id)
        .bind(role.as_str())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn is_member(&self, team_id: i64, user_id: i64) -> DbResult<bool> {
        let found: Option<i64> =
            sqlx::query_scalar("SELECT 1 FROM team_user WHERE team_id = $1 AND user_id = $2")
                .bind(team_id)
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(found.is_some())
    }

    /// The user's role in a team (pivot lookup), or `None` if not a member.
    pub async fn role_in_team(&self, user_id: i64, team_id: i64) -> DbResult<Option<Role>> {
        let role: Option<String> =
            sqlx::query_scalar("SELECT role FROM team_user WHERE user_id = $1 AND team_id = $2")
                .bind(user_id)
                .bind(team_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(role.map(|r| Role::from_str_coerce(&r)))
    }

    pub async fn member_count(&self, team_id: i64) -> DbResult<i64> {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM team_user WHERE team_id = $1")
            .bind(team_id)
            .fetch_one(&self.pool)
            .await?;
        Ok(n)
    }

    /// Remove a member. Refuses to remove the sole member of the root team.
    /// When the last owner leaves, the first remaining member is promoted to
    /// owner; if nobody remains, the (non-root) team is deleted. Returns
    /// `false` when the removal is refused (root sole-member).
    pub async fn remove_member(&self, team_id: i64, user_id: i64) -> DbResult<bool> {
        let mut tx = self.pool.begin().await?;

        let total: i64 = sqlx::query_scalar("SELECT count(*) FROM team_user WHERE team_id = $1")
            .bind(team_id)
            .fetch_one(&mut *tx)
            .await?;
        if team_id == ROOT_TEAM_ID && total <= 1 {
            tx.rollback().await?;
            return Ok(false);
        }

        let removed = sqlx::query("DELETE FROM team_user WHERE team_id = $1 AND user_id = $2")
            .bind(team_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        if removed.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(false);
        }

        Self::reconcile_ownership(&mut tx, team_id).await?;
        tx.commit().await?;
        Ok(true)
    }

    /// After a membership/role change, ensure the team still has an owner:
    /// promote the earliest-joined member when none remains, or delete an empty
    /// non-root team.
    async fn reconcile_ownership(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        team_id: i64,
    ) -> DbResult<()> {
        let remaining: i64 =
            sqlx::query_scalar("SELECT count(*) FROM team_user WHERE team_id = $1")
                .bind(team_id)
                .fetch_one(&mut **tx)
                .await?;
        if remaining == 0 {
            if team_id != ROOT_TEAM_ID {
                // Repoint anyone whose active team is this (soon-deleted) team to
                // another team they still belong to, else the root team — the
                // `users.team_id` FK is NOT NULL and blocks deleting a team that
                // is still someone's active pointer.
                sqlx::query(
                    "UPDATE users SET team_id = COALESCE(
                         (SELECT tu.team_id FROM team_user tu
                           WHERE tu.user_id = users.id ORDER BY tu.team_id LIMIT 1),
                         $2),
                       updated_at = now()
                     WHERE team_id = $1",
                )
                .bind(team_id)
                .bind(ROOT_TEAM_ID)
                .execute(&mut **tx)
                .await?;
                sqlx::query("DELETE FROM teams WHERE id = $1")
                    .bind(team_id)
                    .execute(&mut **tx)
                    .await?;
            }
            return Ok(());
        }

        let owners: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM team_user WHERE team_id = $1 AND role = 'owner'",
        )
        .bind(team_id)
        .fetch_one(&mut **tx)
        .await?;
        if owners == 0 {
            sqlx::query(
                "UPDATE team_user SET role = 'owner', updated_at = now()
                 WHERE id = (SELECT id FROM team_user WHERE team_id = $1 ORDER BY id LIMIT 1)",
            )
            .bind(team_id)
            .execute(&mut **tx)
            .await?;
        }
        Ok(())
    }

    // --- invitations ---

    /// Create an invitation. `uuid`/`link`/`via` are supplied by the caller.
    pub async fn create_invitation(
        &self,
        team_id: i64,
        uuid: &str,
        email: &str,
        role: Role,
        link: Option<&str>,
        via: &str,
    ) -> DbResult<TeamInvitation> {
        let row = sqlx::query_as::<_, TeamInvitation>(&format!(
            "INSERT INTO team_invitations (uuid, team_id, email, role, link, via)
             VALUES ($1, $2, $3, $4, $5, $6) RETURNING {INVITATION_COLS}"
        ))
        .bind(uuid)
        .bind(team_id)
        .bind(email.to_lowercase())
        .bind(role.as_str())
        .bind(link)
        .bind(via)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn list_invitations(&self, team_id: i64) -> DbResult<Vec<TeamInvitation>> {
        let rows = sqlx::query_as::<_, TeamInvitation>(&format!(
            "SELECT {INVITATION_COLS} FROM team_invitations WHERE team_id = $1 ORDER BY id"
        ))
        .bind(team_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn get_invitation(&self, uuid: &str) -> DbResult<Option<TeamInvitation>> {
        let row = sqlx::query_as::<_, TeamInvitation>(&format!(
            "SELECT {INVITATION_COLS} FROM team_invitations WHERE uuid = $1"
        ))
        .bind(uuid)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn find_invitation_for_email(
        &self,
        team_id: i64,
        email: &str,
    ) -> DbResult<Option<TeamInvitation>> {
        let row = sqlx::query_as::<_, TeamInvitation>(&format!(
            "SELECT {INVITATION_COLS} FROM team_invitations WHERE team_id = $1 AND email = $2"
        ))
        .bind(team_id)
        .bind(email.to_lowercase())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn delete_invitation(&self, uuid: &str) -> DbResult<bool> {
        let result = sqlx::query("DELETE FROM team_invitations WHERE uuid = $1")
            .bind(uuid)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Accept an invitation: attach the user with the invitation's role and
    /// delete the invitation, atomically. Returns the joined `team_id`.
    pub async fn accept_invitation(&self, uuid: &str, user_id: i64) -> DbResult<Option<i64>> {
        let mut tx = self.pool.begin().await?;
        let invite = sqlx::query_as::<_, TeamInvitation>(&format!(
            "SELECT {INVITATION_COLS} FROM team_invitations WHERE uuid = $1 FOR UPDATE"
        ))
        .bind(uuid)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(invite) = invite else {
            tx.rollback().await?;
            return Ok(None);
        };
        sqlx::query(
            "INSERT INTO team_user (team_id, user_id, role) VALUES ($1, $2, $3)
             ON CONFLICT (team_id, user_id) DO NOTHING",
        )
        .bind(invite.team_id)
        .bind(user_id)
        .bind(invite.role)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM team_invitations WHERE id = $1")
            .bind(invite.id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Some(invite.team_id))
    }
}
