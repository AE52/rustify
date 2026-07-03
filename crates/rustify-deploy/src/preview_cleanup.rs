//! Preview teardown handler (`preview_cleanup` job, payload
//! `{application_uuid, pull_request_id}`).
//!
//! Port of Coolify's `CleanupPreviewDeployment` + `ApplicationPreview` force-delete:
//! cancel this PR's QUEUED/IN_PROGRESS deployments (logging the cancellation and
//! killing their helper containers), `docker rm -f` the PR's containers, remove
//! the PR's dedicated network (disconnecting the proxy first), then delete the
//! `ApplicationPreview` row. Also deletes the PR-status comment.

use async_trait::async_trait;
use serde_json::Value;

use rustify_core::{ExecOpts, ServerConn};
use rustify_db::repos::{
    ApplicationRepo, DeploymentRepo, GithubAppRepo, KeyRepo, PreviewRepo, ServerRepo,
};
use rustify_jobs::JobHandler;

use crate::engine::build_conn;
use crate::github::GithubAppRow;
use crate::{DeployEngineDeps, DeployError, pr_comment, preview};

/// The job kind the preview-cleanup handler is registered under.
pub const PREVIEW_CLEANUP_KIND: &str = "preview_cleanup";

/// [`JobHandler`] for `preview_cleanup`.
pub struct PreviewCleanupHandler {
    deps: DeployEngineDeps,
}

impl PreviewCleanupHandler {
    pub fn new(deps: DeployEngineDeps) -> Self {
        Self { deps }
    }
}

#[async_trait]
impl JobHandler for PreviewCleanupHandler {
    async fn run(&self, payload: Value) -> anyhow::Result<()> {
        let application_uuid = payload
            .get("application_uuid")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("preview_cleanup payload missing application_uuid"))?;
        let pull_request_id = payload
            .get("pull_request_id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| anyhow::anyhow!("preview_cleanup payload missing pull_request_id"))?
            as i32;
        cleanup_preview(&self.deps, application_uuid, pull_request_id)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(())
    }
}

/// Run the full teardown for `(application_uuid, pull_request_id)`.
pub async fn cleanup_preview(
    deps: &DeployEngineDeps,
    application_uuid: &str,
    pull_request_id: i32,
) -> Result<(), DeployError> {
    let app_repo = ApplicationRepo::new(deps.pool.clone());
    let dep_repo = DeploymentRepo::new(deps.pool.clone());
    let preview_repo = PreviewRepo::new(deps.pool.clone());

    let app = app_repo
        .get_by_uuid(application_uuid)
        .await?
        .ok_or_else(|| DeployError::Missing(format!("application {application_uuid}")))?;

    // 1. Cancel active deployments for this PR; log + kill each helper container.
    let cancelled = dep_repo
        .cancel_active_for_pr(app.id, pull_request_id)
        .await?;

    // Resolve the destination server so we can issue docker commands.
    let server_id = app_repo.server_id(app.id).await?.ok_or_else(|| {
        DeployError::Missing(format!("destination server for {application_uuid}"))
    })?;
    let server = ServerRepo::new(deps.pool.clone())
        .get_by_id(server_id)
        .await?
        .ok_or_else(|| DeployError::Missing(format!("server {server_id}")))?;
    let conn = build_conn(&deps.pool, &server, 10).await;
    let opts = ExecOpts {
        timeout_secs: Some(600),
        disable_mux: false,
    };

    for dep in &cancelled {
        // Record the cancellation on the deployment's log (best-effort).
        append_cancel_log(&deps.pool, dep.id).await;
        // Kill the helper container (named by deployment uuid) if it exists.
        kill_helper(deps, &conn, &opts, &dep.uuid).await;
        let _ = deps
            .events
            .send(rustify_core::events::WsEvent::deployment_status_changed(
                &dep.uuid,
                rustify_core::DeploymentStatus::Cancelled,
            ));
    }

    // 2. Stop + remove every container for this PR (checked by name).
    let container = preview::preview_container_name(&app.uuid, pull_request_id as i64);
    let ps = format!("docker ps -a --filter name={container} --format '{{{{.Names}}}}'");
    if let Ok(out) = deps.executor.exec(&conn, &ps, opts.clone()).await {
        for name in out.stdout.lines().map(str::trim).filter(|n| !n.is_empty()) {
            let _ = deps
                .executor
                .exec(&conn, &format!("docker rm -f {name}"), opts.clone())
                .await;
        }
    }

    // 3. Remove the PR's dedicated network (disconnect the proxy first).
    let network = preview::preview_network(&app.uuid, pull_request_id as i64);
    let _ = deps
        .executor
        .exec(
            &conn,
            &format!("docker network disconnect -f {network} rustify-proxy 2>/dev/null || true"),
            opts.clone(),
        )
        .await;
    let _ = deps
        .executor
        .exec(
            &conn,
            &format!("docker network rm {network} 2>/dev/null || true"),
            opts.clone(),
        )
        .await;

    // 4. Delete the preview row (and its PR-status comment) if present.
    if let Some(preview) = preview_repo.get(app.id, pull_request_id).await? {
        if let Some(comment_id) = preview.pull_request_issue_comment_id {
            delete_pr_comment(deps, &app, comment_id).await;
        }
        preview_repo.delete(preview.id).await?;
    }

    Ok(())
}

/// Kill a helper container by name, checking it exists first (parity with
/// `CleanupPreviewDeployment::killHelperContainer`).
async fn kill_helper(
    deps: &DeployEngineDeps,
    conn: &ServerConn,
    opts: &ExecOpts,
    deployment_uuid: &str,
) {
    let check = format!("docker ps -a --filter name={deployment_uuid} --format '{{{{.Names}}}}'");
    if let Ok(out) = deps.executor.exec(conn, &check, opts.clone()).await
        && !out.stdout.trim().is_empty()
    {
        let _ = deps
            .executor
            .exec(
                conn,
                &format!("docker rm -f {deployment_uuid}"),
                opts.clone(),
            )
            .await;
    }
}

/// Append the "cancelled by PR close" log line to a deployment.
async fn append_cancel_log(pool: &sqlx::PgPool, deployment_id: i64) {
    let next_ord: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(ord), -1) + 1 FROM deployment_logs WHERE deployment_id = $1",
    )
    .bind(deployment_id)
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    let _ = sqlx::query(
        "INSERT INTO deployment_logs (deployment_id, ord, kind, content, hidden, batch)
         VALUES ($1, $2, 'stderr', 'Deployment cancelled: Pull request closed.', false, 1)",
    )
    .bind(deployment_id)
    .bind(next_ord)
    .execute(pool)
    .await;
}

/// Best-effort delete of the PR-status comment (github source, non-public).
async fn delete_pr_comment(
    deps: &DeployEngineDeps,
    app: &rustify_db::repos::Application,
    comment_id: i64,
) {
    if app.source_type.as_deref() != Some("github_app") {
        return;
    }
    let Some(src_id) = app.source_id else { return };
    let gh = match GithubAppRepo::new(deps.pool.clone())
        .get_by_id(src_id)
        .await
    {
        Ok(Some(g)) if !g.is_public => g,
        _ => return,
    };
    let Some(pk_id) = gh.private_key_id else {
        return;
    };
    let Ok(pem) = KeyRepo::new(deps.pool.clone())
        .decrypt_private_key(pk_id)
        .await
    else {
        return;
    };
    let row = GithubAppRow {
        id: gh.id,
        app_id: gh.app_id.unwrap_or(0),
        installation_id: gh.installation_id.unwrap_or(0),
        api_url: gh.api_url.clone(),
        private_key_pem: pem,
    };
    let client = reqwest::Client::new();
    if let Err(e) = pr_comment::delete(&client, &row, &app.git_repository, comment_id).await {
        tracing::warn!(error = %e, "failed to delete PR status comment");
    }
}
