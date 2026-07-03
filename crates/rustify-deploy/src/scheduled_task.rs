//! User scheduled-task runner (`docker exec` into an app/service container).
//!
//! Behavioural port of Coolify's `ScheduledTaskJob` (app/Jobs/ScheduledTaskJob.php):
//! resolve the target container(s) for the task's resource, pick the single one
//! (or the one whose name starts with `{task.container}-{resource_uuid}`), then
//! run `docker exec {container} sh -c '<cmd>'` over SSH with multiplexing
//! disabled and the task's per-run timeout. The execution row (opened by the
//! dispatcher or the trigger endpoint and passed as `execution_uuid`) is
//! finalised with `success`/`failed`, the command output, any error detail and
//! the wall-clock duration. Failures retry up to three attempts with a
//! `[30, 60, 120]s` backoff (ScheduledTaskJob::backoff), matching Coolify.
//!
//! Multiplexing is disabled to avoid the ControlMaster race that concurrent
//! `docker exec`s can hit (coolify issue #6736).

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Timelike, Utc};
use serde_json::{Value, json};

use rustify_core::cron::is_due;
use rustify_core::events::WsEvent;
use rustify_core::{ExecOpts, ServerConn};
use rustify_db::repos::{
    ApplicationRepo, ScheduledTask, ScheduledTaskRepo, ServerRepo, ServiceRepo,
};
use rustify_docker::parse_containers;
use rustify_jobs::{JobHandler, JobQueue};

use crate::{DeployEngineDeps, DeployError};

/// Job kind for the queue.
pub const SCHEDULED_TASK_KIND: &str = "scheduled_task";

/// Retry backoff between attempts (ScheduledTaskJob::backoff). With three
/// attempts the last entry is unused, mirroring Laravel's index-clamped backoff.
pub const SCHEDULED_TASK_BACKOFF_SECS: [u64; 3] = [30, 60, 120];
/// Total attempts (`ScheduledTaskJob::$tries`).
pub const SCHEDULED_TASK_TRIES: usize = 3;

/// [`JobHandler`] for kind `"scheduled_task"`, payload `{"execution_uuid": ".."}`.
pub struct ScheduledTaskHandler {
    deps: DeployEngineDeps,
    /// Sleep between failed exec attempts; injectable so tests run instantly.
    backoff: Vec<Duration>,
}

impl ScheduledTaskHandler {
    pub fn new(deps: DeployEngineDeps) -> Self {
        Self {
            deps,
            backoff: SCHEDULED_TASK_BACKOFF_SECS
                .iter()
                .map(|s| Duration::from_secs(*s))
                .collect(),
        }
    }

    /// Construct with an explicit backoff schedule (tests use zero delays).
    pub fn with_backoff(deps: DeployEngineDeps, backoff: Vec<Duration>) -> Self {
        Self { deps, backoff }
    }
}

#[async_trait]
impl JobHandler for ScheduledTaskHandler {
    async fn run(&self, payload: Value) -> anyhow::Result<()> {
        let execution_uuid = payload
            .get("execution_uuid")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("scheduled_task payload missing execution_uuid"))?;
        run_scheduled_task(&self.deps, execution_uuid, &self.backoff)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(())
    }
}

/// Build the per-minute dispatcher closure for [`rustify_jobs::Scheduler::every`].
/// Each tick enqueues a `scheduled_task` job for every enabled task that is due
/// (per [`is_due`]) whose resource is running. Errors are logged and swallowed
/// so the loop keeps running.
pub fn task_dispatcher_task(
    deps: DeployEngineDeps,
    queue: JobQueue,
) -> impl Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + 'static {
    move || {
        let deps = deps.clone();
        let queue = queue.clone();
        Box::pin(async move {
            if let Err(e) = dispatch_due_tasks(&deps, &queue, Utc::now()).await {
                tracing::warn!(error = %e, "scheduled-task dispatch sweep failed");
            }
        })
    }
}

/// One dispatch sweep: enqueue a `scheduled_task` job (opening its execution
/// row) for every enabled, due, running task not already dispatched this minute.
/// Returns the number of tasks dispatched.
pub async fn dispatch_due_tasks(
    deps: &DeployEngineDeps,
    queue: &JobQueue,
    now: DateTime<Utc>,
) -> Result<usize, DeployError> {
    let repo = ScheduledTaskRepo::new(deps.pool.clone());
    let minute_start = now
        .with_second(0)
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(now);
    let mut dispatched = 0;

    for task in repo.list_enabled().await? {
        if !is_due(&task.frequency, now, &Utc) {
            continue;
        }
        if !resource_running(deps, &task).await? {
            continue;
        }
        // Fire at most once per minute even if the tick drifts.
        if repo.has_execution_since(task.id, minute_start).await? {
            continue;
        }
        let execution = repo.create_execution(task.id).await?;
        queue
            .enqueue(
                SCHEDULED_TASK_KIND,
                json!({ "execution_uuid": execution.uuid }),
                None,
            )
            .await
            .map_err(|e| DeployError::Jobs(e.to_string()))?;
        dispatched += 1;
    }
    Ok(dispatched)
}

/// Whether the task's resource reports a `running` status (DB read; the handler
/// re-checks the live containers).
async fn resource_running(
    deps: &DeployEngineDeps,
    task: &ScheduledTask,
) -> Result<bool, DeployError> {
    if let Some(app_id) = task.application_id {
        Ok(ApplicationRepo::new(deps.pool.clone())
            .get_by_id(app_id)
            .await?
            .map(|a| a.status.contains("running"))
            .unwrap_or(false))
    } else if let Some(service_id) = task.service_id {
        Ok(ServiceRepo::new(deps.pool.clone())
            .get_by_id(service_id)
            .await?
            .map(|s| s.status.contains("running"))
            .unwrap_or(false))
    } else {
        Ok(false)
    }
}

/// A recorded task outcome (as opposed to an infrastructure error).
struct TaskFail {
    message: String,
    details: Option<String>,
}

/// Run one execution to a terminal state. Task-level failures are recorded on
/// the execution row and return `Ok(())` (no queue retry — retries are handled
/// internally); only infrastructure errors (missing rows) return `Err`.
pub async fn run_scheduled_task(
    deps: &DeployEngineDeps,
    execution_uuid: &str,
    backoff: &[Duration],
) -> Result<(), DeployError> {
    let repo = ScheduledTaskRepo::new(deps.pool.clone());
    let execution = repo
        .get_execution_by_uuid(execution_uuid)
        .await?
        .ok_or_else(|| DeployError::NotFound(execution_uuid.to_string()))?;
    let task = repo
        .get_by_id(execution.scheduled_task_id)
        .await?
        .ok_or_else(|| {
            DeployError::Missing(format!("scheduled_task {}", execution.scheduled_task_id))
        })?;

    let start = Utc::now();
    let outcome = execute_task(deps, &task, backoff).await;
    let duration = (Utc::now() - start).num_seconds().max(0) as i32;

    let (status, message, details) = match &outcome {
        Ok(output) => ("success", Some(output.clone()), None),
        Err(fail) => ("failed", Some(fail.message.clone()), fail.details.clone()),
    };
    repo.finish_execution(
        execution.id,
        status,
        message.as_deref(),
        details.as_deref(),
        duration,
    )
    .await?;
    let _ = deps.events.send(WsEvent::scheduled_task_status_changed(
        &task.uuid,
        &execution.uuid,
        status,
    ));
    Ok(())
}

/// Resolve the resource, ensure it is running, pick the target container and run
/// the command with retry.
async fn execute_task(
    deps: &DeployEngineDeps,
    task: &ScheduledTask,
    backoff: &[Duration],
) -> Result<String, TaskFail> {
    // Resolve resource → (destination_id, resource_uuid, running, containers).
    let resolved = resolve_target(deps, task).await.map_err(|e| TaskFail {
        message: e.to_string(),
        details: None,
    })?;

    if !resolved.running {
        return Err(TaskFail {
            message: "Resource is not running.".to_string(),
            details: None,
        });
    }

    let container = resolve_container(
        &resolved.containers,
        task.container.as_deref(),
        &resolved.uuid,
    )
    .map_err(|message| TaskFail {
        message,
        details: None,
    })?;

    let conn = resolve_conn(deps, resolved.destination_id)
        .await
        .map_err(|e| TaskFail {
            message: e.to_string(),
            details: None,
        })?;

    let script = docker_exec_command(&container, &task.command);
    let opts = ExecOpts {
        timeout_secs: Some(task.timeout.max(1) as u32),
        disable_mux: true,
    };

    run_with_retry(deps, &conn, &script, &opts, backoff).await
}

/// Run the command, retrying on failure per `backoff`. Returns the stdout on
/// success or the final [`TaskFail`].
async fn run_with_retry(
    deps: &DeployEngineDeps,
    conn: &ServerConn,
    script: &str,
    opts: &ExecOpts,
    backoff: &[Duration],
) -> Result<String, TaskFail> {
    let mut last = TaskFail {
        message: "scheduled task did not run".to_string(),
        details: None,
    };
    for attempt in 0..SCHEDULED_TASK_TRIES {
        match deps.executor.exec(conn, script, opts.clone()).await {
            Ok(out) if out.exit_code == 0 => return Ok(out.stdout.trim_end().to_string()),
            Ok(out) => {
                let stderr = out.stderr.trim().to_string();
                let stdout = out.stdout.trim().to_string();
                last = TaskFail {
                    message: if !stderr.is_empty() { stderr } else { stdout },
                    details: Some(format!("command exited with code {}", out.exit_code)),
                };
            }
            Err(e) => {
                last = TaskFail {
                    message: e.to_string(),
                    details: None,
                };
            }
        }
        if attempt + 1 < SCHEDULED_TASK_TRIES {
            if let Some(delay) = backoff.get(attempt) {
                tokio::time::sleep(*delay).await;
            }
        }
    }
    Err(last)
}

/// The command line Coolify builds: `docker exec {container} sh -c '<cmd>'`,
/// with single quotes in the command escaped as `'\''` (ScheduledTaskJob:150).
pub fn docker_exec_command(container: &str, command: &str) -> String {
    let escaped = command.replace('\'', "'\\''");
    format!("docker exec {container} sh -c '{escaped}'")
}

/// Choose the container to exec into (ScheduledTaskJob:140-167):
/// - none running → error;
/// - more than one and no `task.container` → error;
/// - exactly one → use it;
/// - otherwise the first whose name starts with `{container}-{resource_uuid}`.
pub fn resolve_container(
    containers: &[String],
    task_container: Option<&str>,
    resource_uuid: &str,
) -> Result<String, String> {
    if containers.is_empty() {
        return Err("No containers running.".to_string());
    }
    let named = task_container.map(str::trim).filter(|c| !c.is_empty());
    if containers.len() > 1 && named.is_none() {
        return Err(
            "More than one container exists but no container name was provided.".to_string(),
        );
    }
    if containers.len() == 1 {
        return Ok(containers[0].clone());
    }
    let prefix = format!("{}-{resource_uuid}", named.unwrap_or_default());
    containers
        .iter()
        .find(|c| c.starts_with(&prefix))
        .cloned()
        .ok_or_else(|| "No valid container was found. Is the container name correct?".to_string())
}

/// The resolved target of a scheduled task.
struct Target {
    destination_id: i64,
    uuid: String,
    running: bool,
    containers: Vec<String>,
}

async fn resolve_target(
    deps: &DeployEngineDeps,
    task: &ScheduledTask,
) -> Result<Target, DeployError> {
    if let Some(app_id) = task.application_id {
        let app = ApplicationRepo::new(deps.pool.clone())
            .get_by_id(app_id)
            .await?
            .ok_or_else(|| DeployError::Missing(format!("application {app_id}")))?;
        let running = app.status.contains("running");
        let containers = if running {
            application_containers(deps, app.destination_id, app.id).await
        } else {
            Vec::new()
        };
        Ok(Target {
            destination_id: app.destination_id,
            uuid: app.uuid,
            running,
            containers,
        })
    } else if let Some(service_id) = task.service_id {
        let repo = ServiceRepo::new(deps.pool.clone());
        let service = repo
            .get_by_id(service_id)
            .await?
            .ok_or_else(|| DeployError::Missing(format!("service {service_id}")))?;
        let running = service.status.contains("running");
        // Running child containers named `{child}-{service_uuid}`.
        let containers = repo
            .applications(service.id)
            .await?
            .into_iter()
            .filter(|a| a.status.contains("running"))
            .map(|a| format!("{}-{}", a.name, service.uuid))
            .collect();
        Ok(Target {
            destination_id: service.destination_id,
            uuid: service.uuid,
            running,
            containers,
        })
    } else {
        Err(DeployError::Missing(format!(
            "scheduled_task {} has no resource",
            task.id
        )))
    }
}

/// `docker ps -a --filter label=rustify.applicationId={id}` → running container
/// names with any leading `/` stripped.
async fn application_containers(
    deps: &DeployEngineDeps,
    destination_id: i64,
    application_id: i64,
) -> Vec<String> {
    let Ok(conn) = resolve_conn(deps, destination_id).await else {
        return Vec::new();
    };
    let cmd = format!(
        "docker ps -a --filter label=rustify.applicationId={application_id} --format '{{{{json .}}}}'"
    );
    let Ok(out) = deps.executor.exec(&conn, &cmd, ExecOpts::default()).await else {
        return Vec::new();
    };
    parse_containers(&out.stdout)
        .into_iter()
        .filter(|c| c.state.eq_ignore_ascii_case("running"))
        .map(|c| c.name.trim_start_matches('/').to_string())
        .collect()
}

/// Build a [`ServerConn`] for the server behind a destination.
async fn resolve_conn(
    deps: &DeployEngineDeps,
    destination_id: i64,
) -> Result<ServerConn, DeployError> {
    let servers = ServerRepo::new(deps.pool.clone());
    let destination = servers
        .destination_by_id(destination_id)
        .await?
        .ok_or_else(|| DeployError::Missing(format!("destination {destination_id}")))?;
    let server = servers
        .get_by_id(destination.server_id)
        .await?
        .ok_or_else(|| DeployError::Missing(format!("server {}", destination.server_id)))?;
    let ct = servers
        .settings(server.id)
        .await?
        .map(|s| s.connection_timeout.max(1) as u32)
        .unwrap_or(10);
    Ok(crate::engine::build_conn(&deps.pool, &server, ct).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_is_docker_exec_sh_c() {
        assert_eq!(
            docker_exec_command("web-123", "ls -la"),
            "docker exec web-123 sh -c 'ls -la'"
        );
    }

    #[test]
    fn command_escapes_single_quotes() {
        // A single quote must become the classic `'\''` sequence.
        assert_eq!(
            docker_exec_command("c", "echo 'hi'"),
            "docker exec c sh -c 'echo '\\''hi'\\'''"
        );
    }

    #[test]
    fn single_container_is_used() {
        let c = vec!["only-one".to_string()];
        assert_eq!(resolve_container(&c, None, "uuid").unwrap(), "only-one");
        // A container name hint is ignored when there is exactly one.
        assert_eq!(
            resolve_container(&c, Some("web"), "uuid").unwrap(),
            "only-one"
        );
    }

    #[test]
    fn named_container_matches_prefix() {
        let c = vec!["web-uuid-abc".to_string(), "db-uuid-xyz".to_string()];
        assert_eq!(
            resolve_container(&c, Some("web"), "uuid").unwrap(),
            "web-uuid-abc"
        );
    }

    #[test]
    fn ambiguous_without_name_errors() {
        let c = vec!["a-uuid".to_string(), "b-uuid".to_string()];
        let err = resolve_container(&c, None, "uuid").unwrap_err();
        assert!(err.contains("More than one container"));
    }

    #[test]
    fn no_containers_errors() {
        let err = resolve_container(&[], Some("web"), "uuid").unwrap_err();
        assert!(err.contains("No containers running"));
    }

    #[test]
    fn named_but_no_match_errors() {
        let c = vec!["web-uuid".to_string(), "db-uuid".to_string()];
        let err = resolve_container(&c, Some("cache"), "uuid").unwrap_err();
        assert!(err.contains("No valid container"));
    }
}
