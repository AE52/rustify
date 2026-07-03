//! Team authorization contract (multi-tenancy §4/§5): members may read and
//! deploy but not create/update/delete; admins/owners may manage; role-change
//! and invitation privilege guards; session revocation on role change; and
//! active-team switching. Exercised over `oneshot` + `#[sqlx::test(migrations = "../rustify-db/migrations")]`.

use axum::http::{StatusCode, header};
use serde_json::json;
use sqlx::PgPool;

use rustify_core::Role;
use rustify_db::repos::{TeamRepo, UserRepo};
use rustify_server::build_router;

mod common;
use common::{Req, login, seed_user, send, send_full, state};

const PW: &str = "correct horse battery staple";

/// Create a user in `team_id` with `role` and return `(uuid, email)`.
async fn add_user(pool: &PgPool, team_id: i64, email: &str, role: Role) -> (String, i64) {
    let user = UserRepo::new(pool.clone())
        .create(team_id, email, "U", PW)
        .await
        .unwrap();
    TeamRepo::new(pool.clone())
        .add_member(team_id, user.id, role)
        .await
        .unwrap();
    (user.uuid, user.id)
}

/// Log in as an arbitrary user, returning the `rustify_session=...` cookie.
async fn login_as(app: &axum::Router, email: &str) -> String {
    let (status, headers, _) = send_full(
        app,
        Req::post("/api/v1/auth/login")
            .json(json!({ "email": email, "password": PW }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "login should succeed for {email}");
    headers
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

/// Admin creates a key + server + project + application; returns the app uuid.
async fn scaffold_app(app: &axum::Router, cookie: &str) -> String {
    let (_, key) = send(
        app,
        Req::post("/api/v1/private-keys/generate")
            .cookie(cookie)
            .json(json!({ "name": "k" }))
            .build(),
    )
    .await;
    let key_uuid = key["uuid"].as_str().unwrap().to_string();
    let (st, server) = send(
        app,
        Req::post("/api/v1/servers")
            .cookie(cookie)
            .json(json!({ "name": "s", "ip": "10.9.9.9", "private_key_uuid": key_uuid }))
            .build(),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let server_uuid = server["uuid"].as_str().unwrap().to_string();
    let (st, project) = send(
        app,
        Req::post("/api/v1/projects")
            .cookie(cookie)
            .json(json!({ "name": "p" }))
            .build(),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let project_uuid = project["uuid"].as_str().unwrap().to_string();
    let (st, created) = send(
        app,
        Req::post("/api/v1/applications")
            .cookie(cookie)
            .json(json!({
                "project_uuid": project_uuid,
                "environment_name": "production",
                "server_uuid": server_uuid,
                "name": "web",
                "git_repository": "https://github.com/x/y.git",
            }))
            .build(),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "app create: {created}");
    created["uuid"].as_str().unwrap().to_string()
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn member_cannot_create_but_can_read_and_deploy(pool: PgPool) {
    let (team_id, _) = seed_user(&pool).await;
    let app = build_router(state(pool.clone()));
    let owner = login(&app).await;
    let app_uuid = scaffold_app(&app, &owner).await;

    add_user(&pool, team_id, "member@x.io", Role::Member).await;
    let member = login_as(&app, "member@x.io").await;

    // Read: allowed.
    let (st, _) = send(&app, Req::get("/api/v1/servers").cookie(&member).build()).await;
    assert_eq!(st, StatusCode::OK, "members may read");

    // Create: forbidden with the exact contract message.
    let (st, body) = send(
        &app,
        Req::post("/api/v1/servers")
            .cookie(&member)
            .json(json!({ "name": "s2", "ip": "1.2.3.4", "private_key_uuid": "nope" }))
            .build(),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "members may not create");
    assert_eq!(body["message"], "Missing required team role.");

    // Deploy: allowed (a deploy action, not a management action).
    let (st, _) = send(
        &app,
        Req::post(format!("/api/v1/applications/{app_uuid}/deploy"))
            .cookie(&member)
            .build(),
    )
    .await;
    assert_eq!(st, StatusCode::ACCEPTED, "members may deploy");
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn admin_can_create(pool: PgPool) {
    let (team_id, _) = seed_user(&pool).await;
    let app = build_router(state(pool.clone()));
    add_user(&pool, team_id, "admin@x.io", Role::Admin).await;
    let admin = login_as(&app, "admin@x.io").await;

    let (_, key) = send(
        &app,
        Req::post("/api/v1/private-keys/generate")
            .cookie(&admin)
            .json(json!({ "name": "k" }))
            .build(),
    )
    .await;
    let (st, _) = send(
        &app,
        Req::post("/api/v1/servers")
            .cookie(&admin)
            .json(json!({ "name": "s", "ip": "10.1.1.1", "private_key_uuid": key["uuid"] }))
            .build(),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "admins may create");
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn set_role_guards_and_revokes_sessions(pool: PgPool) {
    let (team_id, _) = seed_user(&pool).await;
    let app = build_router(state(pool.clone()));
    let owner = login(&app).await;

    let (member_uuid, _) = add_user(&pool, team_id, "member@x.io", Role::Member).await;
    let member_cookie = login_as(&app, "member@x.io").await;

    // Member has a live session before the change.
    let (st, _) = send(
        &app,
        Req::get("/api/v1/servers").cookie(&member_cookie).build(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // Owner promotes the member to admin.
    let (st, body) = send(
        &app,
        Req::patch(format!("/api/v1/teams/{team_id}/members/{member_uuid}"))
            .cookie(&owner)
            .json(json!({ "role": "admin" }))
            .build(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "owner may set roles: {body}");
    assert_eq!(body["role"], "admin");

    // The role change revoked the member's old session.
    let (st, _) = send(
        &app,
        Req::get("/api/v1/servers").cookie(&member_cookie).build(),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::UNAUTHORIZED,
        "role change revokes the affected user's sessions"
    );

    // The now-admin cannot grant a role above their own (owner).
    let admin_cookie = login_as(&app, "member@x.io").await;
    let (other_uuid, _) = add_user(&pool, team_id, "other@x.io", Role::Member).await;
    let (st, _) = send(
        &app,
        Req::patch(format!("/api/v1/teams/{team_id}/members/{other_uuid}"))
            .cookie(&admin_cookie)
            .json(json!({ "role": "owner" }))
            .build(),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "admins cannot grant owner");

    // A plain member cannot manage members at all.
    let member2 = login_as(&app, "other@x.io").await;
    let (st, _) = send(
        &app,
        Req::patch(format!("/api/v1/teams/{team_id}/members/{other_uuid}"))
            .cookie(&member2)
            .json(json!({ "role": "admin" }))
            .build(),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "members cannot manage members");
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn invitation_privilege_escalation_guard(pool: PgPool) {
    let (team_id, _) = seed_user(&pool).await;
    let app = build_router(state(pool.clone()));
    let owner = login(&app).await;

    add_user(&pool, team_id, "admin@x.io", Role::Admin).await;
    let admin = login_as(&app, "admin@x.io").await;

    // Owner may invite an owner.
    let (st, _) = send(
        &app,
        Req::post(format!("/api/v1/teams/{team_id}/invitations"))
            .cookie(&owner)
            .json(json!({ "email": "newowner@x.io", "role": "owner" }))
            .build(),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "owner may invite owner");

    // Admin may NOT invite an owner (cannot invite above own rank).
    let (st, _) = send(
        &app,
        Req::post(format!("/api/v1/teams/{team_id}/invitations"))
            .cookie(&admin)
            .json(json!({ "email": "wannabe@x.io", "role": "owner" }))
            .build(),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "admins cannot invite owners");

    // Admin may invite a member.
    let (st, _) = send(
        &app,
        Req::post(format!("/api/v1/teams/{team_id}/invitations"))
            .cookie(&admin)
            .json(json!({ "email": "amember@x.io", "role": "member" }))
            .build(),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "admins may invite members");

    // Plain members cannot invite at all.
    add_user(&pool, team_id, "plain@x.io", Role::Member).await;
    let plain = login_as(&app, "plain@x.io").await;
    let (st, _) = send(
        &app,
        Req::post(format!("/api/v1/teams/{team_id}/invitations"))
            .cookie(&plain)
            .json(json!({ "email": "x@x.io", "role": "member" }))
            .build(),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "members cannot invite");
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn invitation_accept_switches_active_team(pool: PgPool) {
    let (team_a, _) = seed_user(&pool).await;
    let app = build_router(state(pool.clone()));
    let owner = login(&app).await;

    // A second team the owner also owns, to invite from.
    let (st, team_b) = send(
        &app,
        Req::post("/api/v1/teams")
            .cookie(&owner)
            .json(json!({ "name": "beta" }))
            .build(),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let team_b_id = team_b["id"].as_i64().unwrap();

    // A user who currently belongs only to team A.
    add_user(&pool, team_a, "joiner@x.io", Role::Member).await;
    let joiner = login_as(&app, "joiner@x.io").await;

    // Invite joiner to team B and accept.
    let (st, inv) = send(
        &app,
        Req::post(format!("/api/v1/teams/{team_b_id}/invitations"))
            .cookie(&owner)
            .json(json!({ "email": "joiner@x.io", "role": "member" }))
            .build(),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let inv_uuid = inv["uuid"].as_str().unwrap();

    let (st, _) = send(
        &app,
        Req::post(format!("/api/v1/invitations/{inv_uuid}"))
            .cookie(&joiner)
            .build(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "accepting a valid invitation succeeds");

    // Accepting switched the active team to B.
    let (st, current) = send(
        &app,
        Req::get("/api/v1/teams/current").cookie(&joiner).build(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(current["id"].as_i64().unwrap(), team_b_id);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn switch_requires_membership(pool: PgPool) {
    let (team_a, _) = seed_user(&pool).await;
    let app = build_router(state(pool.clone()));
    let owner = login(&app).await;

    add_user(&pool, team_a, "u@x.io", Role::Member).await;
    let user = login_as(&app, "u@x.io").await;

    // A team the user is NOT a member of.
    let (_, team_b) = send(
        &app,
        Req::post("/api/v1/teams")
            .cookie(&owner)
            .json(json!({ "name": "beta" }))
            .build(),
    )
    .await;
    let team_b_id = team_b["id"].as_i64().unwrap();

    let (st, _) = send(
        &app,
        Req::post(format!("/api/v1/teams/{team_b_id}/switch"))
            .cookie(&user)
            .build(),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "cannot switch to a foreign team");

    // The creator (owner) is a member of team B and can switch.
    let (st, _) = send(
        &app,
        Req::post(format!("/api/v1/teams/{team_b_id}/switch"))
            .cookie(&owner)
            .build(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
}
