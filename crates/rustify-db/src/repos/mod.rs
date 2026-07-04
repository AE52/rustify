//! One repository per aggregate. Repos are cheap `Clone` handles wrapping a
//! shared [`sqlx::PgPool`]; construct them from a single pool at startup.

pub mod applications;
pub mod backups;
pub mod cloud_tokens;
pub mod databases;
pub mod deployments;
pub mod env_vars;
pub mod github_apps;
pub mod keys;
pub mod metrics;
pub mod notifications;
pub mod previews;
pub mod projects;
pub mod scheduled_tasks;
pub mod servers;
pub mod services;
pub mod settings;
pub mod teams;
pub mod users;

pub use applications::{Application, ApplicationPatch, ApplicationRepo, NewApplication};
pub use backups::{
    BackupExecution, BackupExecutionRepo, ExecutionResult, NewS3Storage, NewScheduledBackup,
    S3Credentials, S3Storage, S3StoragePatch, S3StorageRepo, ScheduledBackup, ScheduledBackupPatch,
    ScheduledBackupRepo,
};
pub use cloud_tokens::{CloudProviderToken, CloudTokenRepo};
pub use databases::{DatabasePatch, DatabaseRepo, NewDatabase, StandaloneDatabase};
pub use deployments::{Deployment, DeploymentRepo, NewDeployment};
pub use env_vars::{EnvVar, EnvVarRepo, NewEnvVar};
pub use github_apps::{GithubApp, GithubAppPatch, GithubAppRepo, NewGithubApp};
pub use keys::{KeyRepo, PrivateKey};
pub use metrics::{MetricColumn, MetricPoint, MetricSample, MetricsRepo};
pub use notifications::{NotificationSettings, NotificationSettingsPatch, NotificationsRepo};
pub use previews::{ApplicationPreview, PreviewRepo};
pub use projects::{Environment, Project, ProjectRepo};
pub use scheduled_tasks::{
    NewScheduledTask, ScheduledTask, ScheduledTaskExecution, ScheduledTaskPatch, ScheduledTaskRepo,
};
pub use servers::{
    Destination, NewAwsServer, NewHetznerServer, NewServer, Server, ServerRepo, ServerSettings,
};
pub use services::{NewService, Service, ServiceApplication, ServiceRepo};
pub use settings::{ApiToken, InstanceSettings, Session, SettingsRepo};
pub use teams::{
    INVITATION_EXPIRATION_DAYS, ROOT_TEAM_ID, Team, TeamInvitation, TeamMember, TeamRepo,
};
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

    // Ensure the instance-wide root team (`id = 0`) exists. Its owner is the
    // instance admin (multi-tenancy §5). Explicit id keeps it stable and does
    // not advance the BIGSERIAL sequence used by user-created teams.
    sqlx::query(
        "INSERT INTO teams (id, uuid, name, personal_team) VALUES (0, $1, 'root', false)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(ids::new_uuid())
    .execute(&mut *tx)
    .await?;

    // Home team for the seeded admin: the lowest-id team (the root team on a
    // fresh install).
    let team_id: i64 = sqlx::query_scalar("SELECT id FROM teams ORDER BY id LIMIT 1")
        .fetch_one(&mut *tx)
        .await?;

    let admin_id: i64 = match sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind(&email)
        .fetch_optional(&mut *tx)
        .await?
    {
        Some(id) => id,
        None => {
            let password_hash = users::hash_password(&password)?;
            sqlx::query_scalar(
                "INSERT INTO users (uuid, team_id, email, name, password_hash)
                 VALUES ($1, $2, $3, 'Admin', $4) RETURNING id",
            )
            .bind(ids::new_uuid())
            .bind(team_id)
            .bind(&email)
            .bind(&password_hash)
            .fetch_one(&mut *tx)
            .await?
        }
    };

    // Instance admin owns its home team via the membership pivot.
    sqlx::query(
        "INSERT INTO team_user (team_id, user_id, role) VALUES ($1, $2, 'owner')
         ON CONFLICT (team_id, user_id) DO NOTHING",
    )
    .bind(team_id)
    .bind(admin_id)
    .execute(&mut *tx)
    .await?;

    // Auto-provision the team's notification settings row with the sane-default
    // event matrix (critical failures opt in on every channel). Idempotent: the
    // `team_id` unique constraint makes a re-seed a no-op.
    sqlx::query(
        "INSERT INTO notification_settings (uuid, team_id, event_matrix)
         VALUES ($1, $2, $3)
         ON CONFLICT (team_id) DO NOTHING",
    )
    .bind(ids::new_uuid())
    .bind(team_id)
    .bind(rustify_core::notify::default_event_matrix())
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}
