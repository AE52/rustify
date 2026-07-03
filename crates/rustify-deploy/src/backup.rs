//! Scheduled database-backup handler + per-minute dispatcher.
//!
//! Behavioural port of Coolify's `DatabaseBackupJob`
//! (app/Jobs/DatabaseBackupJob.php) and the `removeOldBackups` retention helpers
//! (bootstrap/helpers/databases.php). For a pre-created `running` execution row
//! the handler: resolves the database + server + credentials, dumps into
//! `/data/rustify/backups/databases/<team-slug>-<team_id>/<db-slug>-<uuid>/`,
//! measures the file, optionally uploads to S3 via a throwaway
//! `coolify-helper` + `mc` container, applies the three retention rules, and
//! records the execution outcome.
//!
//! The dump/`mc alias` commands embed decrypted secrets and are therefore never
//! logged. Timestamps are taken here (chrono) and passed into the pure
//! `rustify_core::retention` selector.

use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;
use chrono::{DateTime, Datelike, Timelike, Utc};
use serde_json::{Value, json};

use rustify_core::backup_cmd::{backup_extension, dump_command, dump_prefix};
use rustify_core::events::WsEvent;
use rustify_core::{DatabaseEngine, ExecOpts, ServerConn, select_for_deletion};
use rustify_db::repos::{
    BackupExecution, BackupExecutionRepo, DatabaseRepo, ExecutionResult, S3Storage, S3StorageRepo,
    ScheduledBackup, ScheduledBackupRepo, ServerRepo, StandaloneDatabase,
};
use rustify_jobs::{JobHandler, JobQueue};

use crate::engine::build_conn;
use crate::{DeployEngineDeps, DeployError};

/// Job kind for a single backup execution.
pub const DATABASE_BACKUP_KIND: &str = "database_backup";

/// Server-side backups root (matches Coolify's `backup_dir()/databases`).
const BACKUP_ROOT: &str = "/data/rustify/backups/databases";

/// The throwaway container image used to push a dump to S3 via `mc`.
const HELPER_IMAGE: &str = "ghcr.io/coollabsio/coolify-helper:latest";

/// [`JobHandler`] for kind `"database_backup"`, payload `{"execution_uuid": ".."}`.
pub struct DatabaseBackupHandler {
    deps: DeployEngineDeps,
}

impl DatabaseBackupHandler {
    pub fn new(deps: DeployEngineDeps) -> Self {
        Self { deps }
    }
}

#[async_trait]
impl JobHandler for DatabaseBackupHandler {
    async fn run(&self, payload: Value) -> anyhow::Result<()> {
        let uuid = payload
            .get("execution_uuid")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("backup payload missing execution_uuid"))?;
        run_backup(&self.deps, uuid, Utc::now())
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(())
    }
}

/// Everything the handler needs after resolving the execution row.
struct Resolved {
    execution: BackupExecution,
    backup: ScheduledBackup,
    db: StandaloneDatabase,
    engine: DatabaseEngine,
    conn: ServerConn,
    network: String,
    team_id: i64,
    team_name: String,
}

/// Run one backup execution end to end. Backup-level failures (dump/size/S3)
/// are recorded on the execution row and are *not* returned as errors;
/// infrastructure failures (missing rows, DB errors) propagate.
pub async fn run_backup(
    deps: &DeployEngineDeps,
    execution_uuid: &str,
    now: DateTime<Utc>,
) -> Result<(), DeployError> {
    let r = resolve(deps, execution_uuid).await?;
    let backup_uuid = r.backup.uuid.clone();
    let _ = deps.events.send(WsEvent::backup_status_changed(
        &backup_uuid,
        execution_uuid,
        "running",
    ));

    // Engine must be dumpable.
    let Some(prefix) = dump_prefix(r.engine) else {
        fail(deps, &r, "engine does not support backups").await?;
        return Ok(());
    };
    let ext = backup_extension(r.engine, r.backup.dump_all).unwrap_or("dmp");

    let credentials = DatabaseRepo::new(deps.pool.clone())
        .decrypt_credentials(&r.db.uuid)
        .await?;

    // The single database to dump (ignored for dump_all) and the filename label.
    let single_db = first_database(&r.backup).unwrap_or_else(|| credentials.database.clone());
    let label = if r.backup.dump_all {
        "all".to_string()
    } else {
        single_db.clone()
    };
    let Some(dump) = dump_command(r.engine, &credentials, &single_db, r.backup.dump_all) else {
        fail(deps, &r, "engine does not support backups").await?;
        return Ok(());
    };

    let dir = format!(
        "{BACKUP_ROOT}/{}-{}/{}-{}",
        slug(&r.team_name),
        r.team_id,
        slug(&r.db.name),
        r.db.uuid
    );
    let location = format!("{dir}/{prefix}-dump-{label}-{}.{ext}", now.timestamp());

    // Create the local dump. The dump command (with its embedded password) runs
    // inside the container; the host redirect captures its stdout.
    exec_checked(
        deps,
        &r.conn,
        &format!("mkdir -p {dir}"),
        "creating backup directory",
    )
    .await?;
    let run = format!(
        "docker exec {} sh -c \"{}\" > \"{}\"",
        r.db.uuid, dump, location
    );
    if let Err(msg) = exec_dump(deps, &r.conn, &run).await {
        fail(deps, &r, &msg).await?;
        return Ok(());
    }

    // Measure; an empty/absent file is a failure (DatabaseBackupJob.php:396-399).
    let size = measure(deps, &r.conn, &location).await;
    if size == 0 {
        fail(deps, &r, "local backup file is empty or was not created").await?;
        return Ok(());
    }

    // Optional S3 upload (independent of local success).
    let mut s3_uploaded: Option<bool> = None;
    let mut warning: Option<String> = None;
    if r.backup.save_s3 {
        match upload_to_s3(deps, &r, execution_uuid, &location).await {
            Ok(()) => s3_uploaded = Some(true),
            Err(e) => {
                s3_uploaded = Some(false);
                warning = Some(format!("S3 upload failed: {e}"));
            }
        }
    }

    // If local backups are disabled, drop the file once S3 has it.
    let mut local_deleted = false;
    if r.backup.disable_local_backup && s3_uploaded == Some(true) {
        exec_ignore(deps, &r.conn, &format!("rm -f \"{location}\"")).await;
        local_deleted = true;
    }

    BackupExecutionRepo::new(deps.pool.clone())
        .finish(
            r.execution.id,
            &ExecutionResult {
                status: "success".into(),
                size,
                filename: Some(location.clone()),
                s3_uploaded,
                message: warning,
                local_storage_deleted: local_deleted,
            },
        )
        .await?;

    apply_retention(deps, &r, now).await?;

    let _ = deps.events.send(WsEvent::backup_status_changed(
        &backup_uuid,
        execution_uuid,
        "success",
    ));
    Ok(())
}

/// Record a failed execution and broadcast the status.
async fn fail(deps: &DeployEngineDeps, r: &Resolved, message: &str) -> Result<(), DeployError> {
    BackupExecutionRepo::new(deps.pool.clone())
        .finish(
            r.execution.id,
            &ExecutionResult {
                status: "failed".into(),
                size: 0,
                filename: None,
                s3_uploaded: None,
                message: Some(message.to_string()),
                local_storage_deleted: false,
            },
        )
        .await?;
    let _ = deps.events.send(WsEvent::backup_status_changed(
        &r.backup.uuid,
        &r.execution.uuid,
        "failed",
    ));
    Ok(())
}

/// Resolve the execution row down to database, server connection and team.
async fn resolve(deps: &DeployEngineDeps, execution_uuid: &str) -> Result<Resolved, DeployError> {
    let exec_repo = BackupExecutionRepo::new(deps.pool.clone());
    let execution = exec_repo
        .get_by_uuid(execution_uuid)
        .await?
        .ok_or_else(|| DeployError::NotFound(execution_uuid.to_string()))?;
    let backup = ScheduledBackupRepo::new(deps.pool.clone())
        .get_by_id(execution.scheduled_database_backup_id)
        .await?
        .ok_or_else(|| {
            DeployError::Missing(format!("backup {}", execution.scheduled_database_backup_id))
        })?;
    let db = DatabaseRepo::new(deps.pool.clone())
        .get_by_id(backup.database_id)
        .await?
        .ok_or_else(|| DeployError::Missing(format!("database {}", backup.database_id)))?;
    let engine = DatabaseEngine::parse(&db.engine)
        .ok_or_else(|| DeployError::Payload(format!("unknown database engine {}", db.engine)))?;

    // Server + network from the destination.
    let (server_id, network): (i64, String) =
        sqlx::query_as("SELECT server_id, network FROM destinations WHERE id = $1")
            .bind(db.destination_id)
            .fetch_optional(&deps.pool)
            .await?
            .ok_or_else(|| DeployError::Missing(format!("destination {}", db.destination_id)))?;
    let server_repo = ServerRepo::new(deps.pool.clone());
    let server = server_repo
        .get_by_id(server_id)
        .await?
        .ok_or_else(|| DeployError::Missing(format!("server {server_id}")))?;
    let connection_timeout = server_repo
        .settings(server.id)
        .await?
        .map(|s| s.connection_timeout.max(1) as u32)
        .unwrap_or(10);
    let conn = build_conn(&deps.pool, &server, connection_timeout).await;

    // Team via environment → project → team.
    let (team_id, team_name): (i64, String) = sqlx::query_as(
        "SELECT t.id, t.name FROM environments e
         JOIN projects p ON p.id = e.project_id
         JOIN teams t ON t.id = p.team_id
         WHERE e.id = $1",
    )
    .bind(db.environment_id)
    .fetch_optional(&deps.pool)
    .await?
    .ok_or_else(|| DeployError::Missing(format!("team for environment {}", db.environment_id)))?;

    Ok(Resolved {
        execution,
        backup,
        db,
        engine,
        conn,
        network,
        team_id,
        team_name,
    })
}

/// Upload the dump to S3 by mounting it read-only into a `coolify-helper`
/// container and running `mc`. The container is always removed afterwards.
async fn upload_to_s3(
    deps: &DeployEngineDeps,
    r: &Resolved,
    execution_uuid: &str,
    location: &str,
) -> Result<(), DeployError> {
    let s3_id = r
        .backup
        .s3_storage_id
        .ok_or_else(|| DeployError::Missing("s3 storage".into()))?;
    let s3_repo = S3StorageRepo::new(deps.pool.clone());
    let s3: S3Storage = s3_repo
        .get_by_id(s3_id)
        .await?
        .ok_or_else(|| DeployError::Missing(format!("s3 storage {s3_id}")))?;
    let creds = s3_repo.decrypt_credentials(s3_id).await?;
    let endpoint = s3.endpoint.clone().unwrap_or_default();
    let dest = format!("temporary/{}{}/", s3.bucket, s3.path.trim_end_matches('/'));
    let container = format!("backup-of-{execution_uuid}");

    let result = upload_inner(deps, r, &container, location, &endpoint, &creds, &dest).await;
    // Always tear the helper down (Coolify `finally`).
    exec_ignore(deps, &r.conn, &format!("docker rm -f {container}")).await;
    result
}

#[allow(clippy::too_many_arguments)]
async fn upload_inner(
    deps: &DeployEngineDeps,
    r: &Resolved,
    container: &str,
    location: &str,
    endpoint: &str,
    creds: &rustify_db::repos::S3Credentials,
    dest: &str,
) -> Result<(), DeployError> {
    exec_ignore(deps, &r.conn, &format!("docker rm -f {container}")).await;
    exec_checked(
        deps,
        &r.conn,
        &format!(
            "docker run -d --network {} --name {container} --rm -v \"{location}:{location}:ro\" {HELPER_IMAGE}",
            r.network
        ),
        "starting backup helper container",
    )
    .await?;
    // `mc alias set` carries the access key + secret; never surface the command
    // or a raw stderr that might echo them.
    let alias = format!(
        "docker exec {container} mc alias set temporary {endpoint} {} {}",
        creds.key, creds.secret
    );
    let out = deps.executor.exec(&r.conn, &alias, mux_off()).await?;
    if out.exit_code != 0 {
        return Err(DeployError::Build("s3 alias configuration failed".into()));
    }
    exec_checked(
        deps,
        &r.conn,
        &format!("docker exec {container} mc cp \"{location}\" {dest}"),
        "uploading backup to s3",
    )
    .await?;
    Ok(())
}

/// Apply the local (always) and S3 (when enabled) retention rules.
async fn apply_retention(
    deps: &DeployEngineDeps,
    r: &Resolved,
    now: DateTime<Utc>,
) -> Result<(), DeployError> {
    let exec_repo = BackupExecutionRepo::new(deps.pool.clone());

    // Local: rm the selected files, then flag them deleted.
    let local = exec_repo.successful_with_local(r.backup.id).await?;
    let local_del = select_for_deletion(
        &local,
        r.backup.retention_amount_local.max(0) as u32,
        r.backup.retention_days_local.max(0) as u32,
        r.backup.retention_max_gb_local.max(0) as u32,
        now,
    );
    for (_, filename) in exec_repo.filenames_for(&local_del).await? {
        exec_ignore(deps, &r.conn, &format!("rm -f \"{filename}\"")).await;
    }
    exec_repo.mark_local_deleted(&local_del).await?;

    // S3: mc rm the selected objects via a helper, then flag them deleted.
    if r.backup.save_s3
        && let Some(s3_id) = r.backup.s3_storage_id
    {
        let s3 = exec_repo.successful_with_s3(r.backup.id).await?;
        let s3_del = select_for_deletion(
            &s3,
            r.backup.retention_amount_s3.max(0) as u32,
            r.backup.retention_days_s3.max(0) as u32,
            r.backup.retention_max_gb_s3.max(0) as u32,
            now,
        );
        let files = exec_repo.filenames_for(&s3_del).await?;
        if !files.is_empty() {
            remove_from_s3(deps, r, s3_id, &files).await?;
        }
        exec_repo.mark_s3_deleted(&s3_del).await?;
    }

    exec_repo.prune_orphans(r.backup.id).await?;
    Ok(())
}

/// `mc rm` a set of objects using a throwaway helper container.
async fn remove_from_s3(
    deps: &DeployEngineDeps,
    r: &Resolved,
    s3_id: i64,
    files: &[(i64, String)],
) -> Result<(), DeployError> {
    let s3_repo = S3StorageRepo::new(deps.pool.clone());
    let s3 = s3_repo
        .get_by_id(s3_id)
        .await?
        .ok_or_else(|| DeployError::Missing(format!("s3 storage {s3_id}")))?;
    let creds = s3_repo.decrypt_credentials(s3_id).await?;
    let endpoint = s3.endpoint.clone().unwrap_or_default();
    let dest = format!("temporary/{}{}", s3.bucket, s3.path.trim_end_matches('/'));
    let container = format!("backup-retention-{}", r.execution.uuid);

    exec_ignore(deps, &r.conn, &format!("docker rm -f {container}")).await;
    let run = format!(
        "docker run -d --network {} --name {container} --rm --entrypoint sleep {HELPER_IMAGE} 300",
        r.network
    );
    let result = async {
        exec_checked(deps, &r.conn, &run, "starting retention helper").await?;
        let alias = format!(
            "docker exec {container} mc alias set temporary {endpoint} {} {}",
            creds.key, creds.secret
        );
        let out = deps.executor.exec(&r.conn, &alias, mux_off()).await?;
        if out.exit_code != 0 {
            return Err(DeployError::Build("s3 alias configuration failed".into()));
        }
        for (_, filename) in files {
            let base = filename.rsplit('/').next().unwrap_or(filename);
            exec_ignore(
                deps,
                &r.conn,
                &format!("docker exec {container} mc rm {dest}/{base}"),
            )
            .await;
        }
        Ok(())
    }
    .await;
    exec_ignore(deps, &r.conn, &format!("docker rm -f {container}")).await;
    result
}

// ----- remote command helpers ---------------------------------------------

fn mux_off() -> ExecOpts {
    ExecOpts {
        disable_mux: true,
        ..Default::default()
    }
}

/// Run a command, treating a non-zero exit as fatal (stderr is included; the
/// caller must ensure the command carries no secrets).
async fn exec_checked(
    deps: &DeployEngineDeps,
    conn: &ServerConn,
    script: &str,
    what: &str,
) -> Result<(), DeployError> {
    let out = deps.executor.exec(conn, script, mux_off()).await?;
    if out.exit_code != 0 {
        return Err(DeployError::Build(format!(
            "{what} failed ({}): {}",
            out.exit_code,
            out.stderr.trim()
        )));
    }
    Ok(())
}

/// Run the dump command. On failure returns a message safe to store (stderr
/// only — never the command, which embeds the password).
async fn exec_dump(deps: &DeployEngineDeps, conn: &ServerConn, script: &str) -> Result<(), String> {
    match deps.executor.exec(conn, script, mux_off()).await {
        Ok(out) if out.exit_code == 0 => Ok(()),
        Ok(out) => Err(format!(
            "dump failed (exit {}): {}",
            out.exit_code,
            out.stderr.trim()
        )),
        Err(e) => Err(format!("dump command errored: {e}")),
    }
}

async fn exec_ignore(deps: &DeployEngineDeps, conn: &ServerConn, script: &str) {
    let _ = deps.executor.exec(conn, script, mux_off()).await;
}

/// `du -b <loc> | cut -f1`, defaulting to 0 on any error (Coolify
/// calculate_size, DatabaseBackupJob.php:663-666).
async fn measure(deps: &DeployEngineDeps, conn: &ServerConn, location: &str) -> i64 {
    let cmd = format!("du -b \"{location}\" | cut -f1");
    deps.executor
        .exec(conn, &cmd, mux_off())
        .await
        .ok()
        .and_then(|o| o.stdout.trim().parse::<i64>().ok())
        .unwrap_or(0)
}

/// First entry of a comma-separated `databases_to_backup`, trimmed.
fn first_database(backup: &ScheduledBackup) -> Option<String> {
    backup
        .databases_to_backup
        .as_deref()
        .and_then(|s| s.split(',').map(str::trim).find(|p| !p.is_empty()))
        .map(str::to_string)
}

/// Filesystem-safe slug: lowercase, non-alphanumeric runs collapsed to `-`.
fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

// ----- dispatcher ---------------------------------------------------------

/// Build the per-minute dispatcher closure for [`rustify_jobs::Scheduler::every`].
pub fn backup_dispatcher_task(
    deps: DeployEngineDeps,
) -> impl Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + 'static {
    move || {
        let deps = deps.clone();
        Box::pin(async move {
            if let Err(e) = dispatch_due_backups(&deps, Utc::now()).await {
                tracing::warn!(error = %e, "backup dispatcher sweep failed");
            }
        })
    }
}

/// Enqueue a `database_backup` job for every enabled schedule whose cron is due
/// at `now`, skipping any that already produced an execution this clock-minute.
pub async fn dispatch_due_backups(
    deps: &DeployEngineDeps,
    now: DateTime<Utc>,
) -> Result<u64, DeployError> {
    let backup_repo = ScheduledBackupRepo::new(deps.pool.clone());
    let exec_repo = BackupExecutionRepo::new(deps.pool.clone());
    let queue = JobQueue::new(deps.pool.clone());
    let mut dispatched = 0;
    for backup in backup_repo.list_enabled().await? {
        if !cron_is_due(&backup.frequency, now) {
            continue;
        }
        // Task-Z style dedup: at most one execution per schedule per minute.
        if exec_repo.exists_in_current_minute(backup.id).await? {
            continue;
        }
        let execution = exec_repo.create_running(backup.id).await?;
        queue
            .enqueue(
                DATABASE_BACKUP_KIND,
                json!({ "execution_uuid": execution.uuid }),
                None,
            )
            .await
            .map_err(|e| DeployError::Jobs(e.to_string()))?;
        dispatched += 1;
    }
    Ok(dispatched)
}

// ----- minimal cron ------------------------------------------------------

/// Whether the 5-field cron `expr` (or a `@macro`) matches `dt` to the minute.
/// Supports `*`, `*/n`, `a`, `a-b`, `a-b/n` and comma lists in each field, plus
/// the common `@hourly`/`@daily`/`@weekly`/`@monthly`/`@yearly` macros. Placed
/// here because p2-scheduled's `cron.rs` is not yet merged.
pub fn cron_is_due(expr: &str, dt: DateTime<Utc>) -> bool {
    let expanded = expand_macro(expr.trim());
    let fields: Vec<&str> = expanded.split_whitespace().collect();
    if fields.len() != 5 {
        return false;
    }
    let weekday = dt.weekday().num_days_from_sunday();
    field_matches(fields[0], dt.minute(), false)
        && field_matches(fields[1], dt.hour(), false)
        && field_matches(fields[2], dt.day(), false)
        && field_matches(fields[3], dt.month(), false)
        && field_matches(fields[4], weekday, true)
}

fn expand_macro(expr: &str) -> String {
    match expr {
        "@yearly" | "@annually" => "0 0 1 1 *",
        "@monthly" => "0 0 1 * *",
        "@weekly" => "0 0 * * 0",
        "@daily" | "@midnight" => "0 0 * * *",
        "@hourly" => "0 * * * *",
        "@every_minute" => "* * * * *",
        other => other,
    }
    .to_string()
}

fn field_matches(field: &str, value: u32, is_weekday: bool) -> bool {
    field
        .split(',')
        .any(|part| part_matches(part, value, is_weekday))
}

fn part_matches(part: &str, value: u32, is_weekday: bool) -> bool {
    // Sunday is both 0 and 7 in the weekday field.
    if is_weekday && value == 0 && part_matches_raw(part, 7) {
        return true;
    }
    part_matches_raw(part, value)
}

fn part_matches_raw(part: &str, value: u32) -> bool {
    let (spec, step) = match part.split_once('/') {
        Some((spec, step)) => (spec, step.parse::<u32>().ok()),
        None => (part, None),
    };
    match step {
        Some(step) if step > 0 => {
            let (lo, hi) = match spec {
                "*" => (0, u32::MAX),
                s => match s.split_once('-') {
                    Some((a, b)) => match (a.parse(), b.parse()) {
                        (Ok(a), Ok(b)) => (a, b),
                        _ => return false,
                    },
                    None => match s.parse() {
                        Ok(n) => (n, u32::MAX),
                        Err(_) => return false,
                    },
                },
            };
            value >= lo && value <= hi && (value - lo) % step == 0
        }
        Some(_) => false,
        None => match spec {
            "*" => true,
            s => match s.split_once('-') {
                Some((a, b)) => match (a.parse::<u32>(), b.parse::<u32>()) {
                    (Ok(a), Ok(b)) => value >= a && value <= b,
                    _ => false,
                },
                None => s.parse::<u32>().map(|n| n == value).unwrap_or(false),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn slug_collapses_non_alnum() {
        assert_eq!(slug("My Team!"), "my-team");
        assert_eq!(slug("root"), "root");
    }

    #[test]
    fn first_database_splits() {
        let mut b = sample_backup();
        b.databases_to_backup = Some(" app , other ".into());
        assert_eq!(first_database(&b).as_deref(), Some("app"));
        b.databases_to_backup = None;
        assert_eq!(first_database(&b), None);
    }

    #[test]
    fn cron_every_minute_and_specific() {
        assert!(cron_is_due("* * * * *", dt("2026-07-03T14:07:00Z")));
        // 02:00 daily
        assert!(cron_is_due("0 2 * * *", dt("2026-07-03T02:00:00Z")));
        assert!(!cron_is_due("0 2 * * *", dt("2026-07-03T03:00:00Z")));
    }

    #[test]
    fn cron_step_and_range_and_list() {
        // every 15 minutes
        assert!(cron_is_due("*/15 * * * *", dt("2026-07-03T00:30:00Z")));
        assert!(!cron_is_due("*/15 * * * *", dt("2026-07-03T00:31:00Z")));
        // hour range
        assert!(cron_is_due("0 9-17 * * *", dt("2026-07-03T12:00:00Z")));
        assert!(!cron_is_due("0 9-17 * * *", dt("2026-07-03T18:00:00Z")));
        // minute list
        assert!(cron_is_due("0,30 * * * *", dt("2026-07-03T08:30:00Z")));
    }

    #[test]
    fn cron_weekday_sunday_is_zero_or_seven() {
        // 2026-07-05 is a Sunday.
        assert!(cron_is_due("0 0 * * 0", dt("2026-07-05T00:00:00Z")));
        assert!(cron_is_due("0 0 * * 7", dt("2026-07-05T00:00:00Z")));
        assert!(!cron_is_due("0 0 * * 1", dt("2026-07-05T00:00:00Z")));
    }

    #[test]
    fn cron_macros_and_invalid() {
        assert!(cron_is_due("@hourly", dt("2026-07-03T15:00:00Z")));
        assert!(cron_is_due("@daily", dt("2026-07-03T00:00:00Z")));
        assert!(!cron_is_due("bogus", dt("2026-07-03T00:00:00Z")));
        assert!(!cron_is_due("* * *", dt("2026-07-03T00:00:00Z")));
    }

    fn sample_backup() -> ScheduledBackup {
        ScheduledBackup {
            id: 1,
            uuid: "u".into(),
            database_id: 1,
            enabled: true,
            frequency: "0 2 * * *".into(),
            save_s3: false,
            s3_storage_id: None,
            databases_to_backup: None,
            dump_all: false,
            disable_local_backup: false,
            retention_amount_local: 0,
            retention_days_local: 0,
            retention_max_gb_local: 0,
            retention_amount_s3: 0,
            retention_days_s3: 0,
            retention_max_gb_s3: 0,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }
}
