//! Coverage for the field-update and by-id lookup methods added for the
//! contract C5 PATCH/response routes (Track F).

use sqlx::PgPool;

use rustify_db::repos::applications::{ApplicationPatch, ApplicationRepo, NewApplication};
use rustify_db::repos::keys::KeyRepo;
use rustify_db::repos::projects::ProjectRepo;
use rustify_db::repos::servers::{NewServer, ServerRepo};

mod common;
use common::init_secret_key;

async fn team(pool: &PgPool) -> i64 {
    sqlx::query_scalar("INSERT INTO teams (uuid, name) VALUES ($1, 'team') RETURNING id")
        .bind(rustify_core::ids::new_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test]
async fn key_get_by_id_and_update(pool: PgPool) {
    init_secret_key();
    let team_id = team(&pool).await;
    let repo = KeyRepo::new(pool.clone());
    let key = repo
        .create(team_id, "k", "PRIVATE-A", "ssh-ed25519 AAAA")
        .await
        .unwrap();

    let by_id = repo.get_by_id(key.id).await.unwrap().unwrap();
    assert_eq!(by_id.uuid, key.uuid);

    // Rename only.
    let renamed = repo
        .update(&key.uuid, Some("k2"), None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(renamed.name, "k2");
    assert_eq!(renamed.public_key, "ssh-ed25519 AAAA");

    // Rotate material: public key changes, private key re-encrypted.
    let rotated = repo
        .update(&key.uuid, None, Some(("PRIVATE-B", "ssh-ed25519 BBBB")))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rotated.public_key, "ssh-ed25519 BBBB");
    assert_eq!(rotated.name, "k2");
    assert_eq!(repo.decrypt_private_key(key.id).await.unwrap(), "PRIVATE-B");

    assert!(
        repo.update("nope", Some("x"), None)
            .await
            .unwrap()
            .is_none()
    );
}

#[sqlx::test]
async fn server_lookups_and_update(pool: PgPool) {
    init_secret_key();
    let team_id = team(&pool).await;
    let key = KeyRepo::new(pool.clone())
        .create(team_id, "k", "PEM", "ssh-ed25519 AAAA")
        .await
        .unwrap();
    let repo = ServerRepo::new(pool.clone());
    let srv = repo
        .create(NewServer {
            team_id,
            name: "srv".into(),
            ip: "10.0.0.1".into(),
            port: 22,
            ssh_user: "root".into(),
            private_key_id: key.id,
        })
        .await
        .unwrap();

    assert_eq!(
        repo.get_by_id(srv.id).await.unwrap().unwrap().uuid,
        srv.uuid
    );
    let dest = repo.default_destination(srv.id).await.unwrap().unwrap();
    assert_eq!(
        repo.destination_by_id(dest.id)
            .await
            .unwrap()
            .unwrap()
            .server_id,
        srv.id
    );

    let updated = repo
        .update(&srv.uuid, Some("renamed"), None, Some(2222), None, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated.name, "renamed");
    assert_eq!(updated.port, 2222);
    assert_eq!(updated.ip, "10.0.0.1");

    repo.set_proxy_custom_config(srv.id, Some("# custom"))
        .await
        .unwrap();
    let settings = repo.settings(srv.id).await.unwrap().unwrap();
    assert_eq!(settings.proxy_custom_config.as_deref(), Some("# custom"));
}

#[sqlx::test]
async fn project_lookups_and_update(pool: PgPool) {
    let team_id = team(&pool).await;
    let repo = ProjectRepo::new(pool.clone());
    let proj = repo.create(team_id, "p", Some("d")).await.unwrap();

    assert_eq!(
        repo.get_by_id(proj.id).await.unwrap().unwrap().uuid,
        proj.uuid
    );
    let env = repo
        .environment_by_name(proj.id, "production")
        .await
        .unwrap()
        .unwrap();
    let by_id = repo.environment_by_id(env.id).await.unwrap().unwrap();
    assert_eq!(by_id.name, "production");

    let updated = repo
        .update(&proj.uuid, Some("p2"), None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated.name, "p2");
    assert_eq!(updated.description.as_deref(), Some("d"));
}

#[sqlx::test]
async fn application_lookup_and_update(pool: PgPool) {
    init_secret_key();
    let team_id = team(&pool).await;
    let key = KeyRepo::new(pool.clone())
        .create(team_id, "k", "PEM", "ssh-ed25519 AAAA")
        .await
        .unwrap();
    let srv = ServerRepo::new(pool.clone())
        .create(NewServer {
            team_id,
            name: "srv".into(),
            ip: "10.0.0.2".into(),
            port: 22,
            ssh_user: "root".into(),
            private_key_id: key.id,
        })
        .await
        .unwrap();
    let dest = ServerRepo::new(pool.clone())
        .default_destination(srv.id)
        .await
        .unwrap()
        .unwrap();
    let proj_repo = ProjectRepo::new(pool.clone());
    let proj = proj_repo.create(team_id, "p", None).await.unwrap();
    let env = proj_repo
        .environment_by_name(proj.id, "production")
        .await
        .unwrap()
        .unwrap();

    let repo = ApplicationRepo::new(pool.clone());
    let app = repo
        .create(NewApplication {
            environment_id: env.id,
            destination_id: dest.id,
            name: "app".into(),
            git_repository: "https://example.com/r.git".into(),
            git_branch: "main".into(),
            build_pack: "nixpacks".into(),
            ports_exposes: "80".into(),
            fqdn: None,
        })
        .await
        .unwrap();

    assert_eq!(
        repo.get_by_id(app.id).await.unwrap().unwrap().uuid,
        app.uuid
    );

    let patch = ApplicationPatch {
        name: Some("renamed".into()),
        ports_exposes: Some("3000".into()),
        ..Default::default()
    };
    let updated = repo.update(&app.uuid, &patch).await.unwrap().unwrap();
    assert_eq!(updated.name, "renamed");
    assert_eq!(updated.ports_exposes, "3000");
    // Unspecified fields keep their previous value.
    assert_eq!(updated.git_repository, "https://example.com/r.git");

    assert!(repo.update("nope", &patch).await.unwrap().is_none());
}
