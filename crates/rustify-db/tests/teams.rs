//! Multi-tenancy persistence: the `team_user` pivot, role lookups, invitations
//! (create/accept/expiry) and last-owner reconciliation on member removal.

use sqlx::PgPool;

use rustify_core::{Role, ids};
use rustify_db::repos::{TeamRepo, UserRepo};

async fn mk_user(pool: &PgPool, team_id: i64, email: &str) -> i64 {
    UserRepo::new(pool.clone())
        .create(team_id, email, "U", "correct horse battery staple")
        .await
        .unwrap()
        .id
}

#[sqlx::test]
async fn pivot_add_role_and_members(pool: PgPool) {
    let repo = TeamRepo::new(pool.clone());
    let team = repo.create_team("t", false).await.unwrap();
    let owner = mk_user(&pool, team.id, "owner@x.io").await;
    let member = mk_user(&pool, team.id, "member@x.io").await;

    repo.add_member(team.id, owner, Role::Owner).await.unwrap();
    repo.add_member(team.id, member, Role::Member)
        .await
        .unwrap();
    // Idempotent: a re-add does not duplicate or overwrite.
    repo.add_member(team.id, member, Role::Admin).await.unwrap();

    assert_eq!(
        repo.role_in_team(owner, team.id).await.unwrap(),
        Some(Role::Owner)
    );
    assert_eq!(
        repo.role_in_team(member, team.id).await.unwrap(),
        Some(Role::Member),
        "add_member must not overwrite an existing pivot role"
    );
    assert_eq!(repo.role_in_team(9999, team.id).await.unwrap(), None);

    let members = repo.members(team.id).await.unwrap();
    assert_eq!(members.len(), 2);
    assert_eq!(members[0].role(), Role::Owner);

    assert!(repo.set_role(team.id, member, Role::Admin).await.unwrap());
    assert_eq!(
        repo.role_in_team(member, team.id).await.unwrap(),
        Some(Role::Admin)
    );
}

#[sqlx::test]
async fn list_for_user_spans_multiple_teams(pool: PgPool) {
    let repo = TeamRepo::new(pool.clone());
    let a = repo.create_team("a", false).await.unwrap();
    let b = repo.create_team("b", false).await.unwrap();
    let user = mk_user(&pool, a.id, "u@x.io").await;
    repo.add_member(a.id, user, Role::Owner).await.unwrap();
    repo.add_member(b.id, user, Role::Member).await.unwrap();

    let teams = repo.list_for_user(user).await.unwrap();
    assert_eq!(teams.len(), 2);
    assert!(teams.iter().any(|t| t.id == a.id));
    assert!(teams.iter().any(|t| t.id == b.id));
}

#[sqlx::test]
async fn switch_changes_active_team(pool: PgPool) {
    let repo = TeamRepo::new(pool.clone());
    let a = repo.create_team("a", false).await.unwrap();
    let b = repo.create_team("b", false).await.unwrap();
    let users = UserRepo::new(pool.clone());
    let user = users
        .create(a.id, "u@x.io", "U", "correct horse battery staple")
        .await
        .unwrap();
    assert_eq!(user.team_id, a.id);

    let switched = users.set_active_team(user.id, b.id).await.unwrap().unwrap();
    assert_eq!(switched.team_id, b.id);
    let reread = users.get_by_id(user.id).await.unwrap().unwrap();
    assert_eq!(reread.team_id, b.id);
}

#[sqlx::test]
async fn invitation_create_and_accept(pool: PgPool) {
    let repo = TeamRepo::new(pool.clone());
    let team = repo.create_team("t", false).await.unwrap();
    // The invited user must already exist (matched by email on accept).
    let invited = mk_user(&pool, team.id, "invitee@x.io").await;

    let uuid = ids::new_invitation_uuid();
    assert_eq!(uuid.len(), 32, "invitation uuid is a 32-char cuid2");
    let inv = repo
        .create_invitation(
            team.id,
            &uuid,
            "Invitee@X.io",
            Role::Admin,
            Some("/l"),
            "link",
        )
        .await
        .unwrap();
    assert_eq!(inv.email, "invitee@x.io", "email is lowercased");
    assert_eq!(inv.role(), Role::Admin);
    assert!(inv.is_valid());

    assert_eq!(repo.list_invitations(team.id).await.unwrap().len(), 1);

    let team_id = repo.accept_invitation(&uuid, invited).await.unwrap();
    assert_eq!(team_id, Some(team.id));
    // Attached with the invitation role; invitation consumed.
    assert_eq!(
        repo.role_in_team(invited, team.id).await.unwrap(),
        Some(Role::Admin)
    );
    assert!(repo.get_invitation(&uuid).await.unwrap().is_none());
    assert_eq!(repo.list_invitations(team.id).await.unwrap().len(), 0);
}

#[sqlx::test]
async fn invitation_expires_after_three_days(pool: PgPool) {
    let repo = TeamRepo::new(pool.clone());
    let team = repo.create_team("t", false).await.unwrap();
    let uuid = ids::new_invitation_uuid();
    repo.create_invitation(team.id, &uuid, "x@x.io", Role::Member, None, "link")
        .await
        .unwrap();

    // Fresh invitation is valid.
    assert!(
        repo.get_invitation(&uuid)
            .await
            .unwrap()
            .unwrap()
            .is_valid()
    );

    // Backdate just inside the window (3 days) -> still valid.
    sqlx::query("UPDATE team_invitations SET created_at = now() - interval '2 days 23 hours' WHERE uuid = $1")
        .bind(&uuid)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        repo.get_invitation(&uuid)
            .await
            .unwrap()
            .unwrap()
            .is_valid()
    );

    // Backdate past the window -> expired.
    sqlx::query(
        "UPDATE team_invitations SET created_at = now() - interval '4 days' WHERE uuid = $1",
    )
    .bind(&uuid)
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        !repo
            .get_invitation(&uuid)
            .await
            .unwrap()
            .unwrap()
            .is_valid()
    );
}

#[sqlx::test]
async fn remove_last_owner_promotes_first_member(pool: PgPool) {
    let repo = TeamRepo::new(pool.clone());
    let team = repo.create_team("t", false).await.unwrap();
    let owner = mk_user(&pool, team.id, "owner@x.io").await;
    let member = mk_user(&pool, team.id, "member@x.io").await;
    repo.add_member(team.id, owner, Role::Owner).await.unwrap();
    repo.add_member(team.id, member, Role::Member)
        .await
        .unwrap();

    assert!(repo.remove_member(team.id, owner).await.unwrap());
    // The sole remaining member is promoted to owner.
    assert_eq!(
        repo.role_in_team(member, team.id).await.unwrap(),
        Some(Role::Owner)
    );
}

#[sqlx::test]
async fn remove_final_member_deletes_non_root_team(pool: PgPool) {
    // The root team is the fallback active-team pointer for the orphaned user.
    sqlx::query("INSERT INTO teams (id, uuid, name, personal_team) VALUES (0, $1, 'root', false)")
        .bind(ids::new_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let repo = TeamRepo::new(pool.clone());
    let team = repo.create_team("t", false).await.unwrap();
    let owner = mk_user(&pool, team.id, "owner@x.io").await;
    repo.add_member(team.id, owner, Role::Owner).await.unwrap();

    assert!(repo.remove_member(team.id, owner).await.unwrap());
    assert!(
        repo.get_by_id(team.id).await.unwrap().is_none(),
        "an emptied non-root team is deleted"
    );
    // The orphaned user was repointed to the root team.
    let active: i64 = sqlx::query_scalar("SELECT team_id FROM users WHERE id = $1")
        .bind(owner)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(active, 0);
}

#[sqlx::test]
async fn root_team_sole_member_cannot_be_removed(pool: PgPool) {
    // seed_default is not used here; create the root team explicitly at id 0.
    sqlx::query("INSERT INTO teams (id, uuid, name, personal_team) VALUES (0, $1, 'root', false)")
        .bind(ids::new_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let repo = TeamRepo::new(pool.clone());
    let admin = mk_user(&pool, 0, "admin@x.io").await;
    repo.add_member(0, admin, Role::Owner).await.unwrap();

    assert!(
        !repo.remove_member(0, admin).await.unwrap(),
        "the last member of the root team must not be removable"
    );
    assert_eq!(
        repo.role_in_team(admin, 0).await.unwrap(),
        Some(Role::Owner),
        "the refused removal leaves the pivot intact"
    );
    // The root team itself is undeletable too.
    assert!(!repo.delete(0).await.unwrap());
}

#[sqlx::test]
async fn sole_owner_self_demote_is_prevented(pool: PgPool) {
    let repo = TeamRepo::new(pool.clone());
    let team = repo.create_team("t", false).await.unwrap();
    let owner = mk_user(&pool, team.id, "owner@x.io").await;
    let member = mk_user(&pool, team.id, "member@x.io").await;
    repo.add_member(team.id, owner, Role::Owner).await.unwrap();
    repo.add_member(team.id, member, Role::Member)
        .await
        .unwrap();

    // Demoting the only owner would leave the team ownerless: rejected.
    let err = repo
        .set_role(team.id, owner, Role::Member)
        .await
        .expect_err("sole-owner demote must be rejected");
    assert!(
        matches!(err, rustify_db::DbError::Invalid(_)),
        "expected an Invalid error, got {err:?}"
    );
    // The refused change leaves the owner's role intact.
    assert_eq!(
        repo.role_in_team(owner, team.id).await.unwrap(),
        Some(Role::Owner)
    );
}

#[sqlx::test]
async fn non_last_owner_demote_succeeds(pool: PgPool) {
    let repo = TeamRepo::new(pool.clone());
    let team = repo.create_team("t", false).await.unwrap();
    let owner_a = mk_user(&pool, team.id, "a@x.io").await;
    let owner_b = mk_user(&pool, team.id, "b@x.io").await;
    repo.add_member(team.id, owner_a, Role::Owner)
        .await
        .unwrap();
    repo.add_member(team.id, owner_b, Role::Owner)
        .await
        .unwrap();

    // With two owners, demoting one still leaves an owner: allowed.
    assert!(repo.set_role(team.id, owner_a, Role::Admin).await.unwrap());
    assert_eq!(
        repo.role_in_team(owner_a, team.id).await.unwrap(),
        Some(Role::Admin)
    );
    assert_eq!(
        repo.role_in_team(owner_b, team.id).await.unwrap(),
        Some(Role::Owner)
    );
}
