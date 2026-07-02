//! One repository per aggregate. Repos are cheap `Clone` handles wrapping a
//! shared [`sqlx::PgPool`]; construct them from a single pool at startup.

pub mod applications;
pub mod deployments;
pub mod env_vars;
pub mod keys;
pub mod projects;
pub mod servers;
pub mod settings;
pub mod teams;
pub mod users;

pub use applications::{Application, ApplicationPatch, ApplicationRepo, NewApplication};
pub use deployments::{Deployment, DeploymentRepo, NewDeployment};
pub use env_vars::{EnvVar, EnvVarRepo, NewEnvVar};
pub use keys::{KeyRepo, PrivateKey};
pub use projects::{Environment, Project, ProjectRepo};
pub use servers::{Destination, NewServer, Server, ServerRepo, ServerSettings};
pub use settings::{ApiToken, InstanceSettings, Session, SettingsRepo};
pub use teams::{Team, TeamRepo};
pub use users::{User, UserRepo};

use sqlx::PgPool;

use rustify_core::ids;

use crate::{DbError, DbResult};

/// Idempotently seed the default team (#1) and the admin user, reading the
/// admin credentials from `RUSTIFY_ADMIN_EMAIL` / `RUSTIFY_ADMIN_PASSWORD`.
/// The password is stored as an argon2id hash. Safe to call on every startup.
pub async fn seed_default(pool: &PgPool) -> DbResult<()> {
    let email = std::env::var("RUSTIFY_ADMIN_EMAIL")
        .map_err(|_| DbError::Config("RUSTIFY_ADMIN_EMAIL is not set".into()))?;
    let password = std::env::var("RUSTIFY_ADMIN_PASSWORD")
        .map_err(|_| DbError::Config("RUSTIFY_ADMIN_PASSWORD is not set".into()))?;

    let mut tx = pool.begin().await?;

    // Reuse the lowest-id team as "team #1", or create it.
    let team_id: i64 = match sqlx::query_scalar("SELECT id FROM teams ORDER BY id LIMIT 1")
        .fetch_optional(&mut *tx)
        .await?
    {
        Some(id) => id,
        None => {
            sqlx::query_scalar("INSERT INTO teams (uuid, name) VALUES ($1, 'root') RETURNING id")
                .bind(ids::new_uuid())
                .fetch_one(&mut *tx)
                .await?
        }
    };

    let admin_exists: Option<i64> = sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind(&email)
        .fetch_optional(&mut *tx)
        .await?;
    if admin_exists.is_none() {
        let password_hash = users::hash_password(&password)?;
        sqlx::query(
            "INSERT INTO users (uuid, team_id, email, name, password_hash)
             VALUES ($1, $2, $3, 'Admin', $4)",
        )
        .bind(ids::new_uuid())
        .bind(team_id)
        .bind(&email)
        .bind(&password_hash)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}
