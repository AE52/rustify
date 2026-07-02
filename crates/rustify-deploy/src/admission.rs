//! Admission control for draining the deployment queue.
//!
//! After a deployment reaches a terminal state, the engine asks whether another
//! queued deployment for the same server may now run. All the admission rules
//! (at most one in-flight per application; at most `concurrent_builds` in-flight
//! per server; FIFO by `created_at`) live in
//! [`rustify_db::repos::DeploymentRepo::next_queuable`], which atomically claims
//! the winner (`queued`→`in_progress`). This module simply enqueues a `deploy`
//! job for whatever it claims, so the job workers pick it up.

use rustify_core::DeploymentStatus;
use rustify_core::events::WsEvent;
use rustify_db::repos::DeploymentRepo;
use rustify_jobs::JobQueue;
use serde_json::json;

use crate::{DeployEngineDeps, DeployError};

/// The job kind the deploy engine is registered under.
pub const DEPLOY_JOB_KIND: &str = "deploy";

/// Claim and enqueue the next admissible deployment for `server_id`, if any.
/// Returns the claimed deployment's uuid.
pub async fn queue_next(
    deps: &DeployEngineDeps,
    server_id: i64,
) -> Result<Option<String>, DeployError> {
    let repo = DeploymentRepo::new(deps.pool.clone());
    let Some(next) = repo.next_queuable(server_id).await? else {
        return Ok(None);
    };
    let queue = JobQueue::new(deps.pool.clone());
    queue
        .enqueue(
            DEPLOY_JOB_KIND,
            json!({ "deployment_uuid": next.uuid }),
            None,
        )
        .await
        .map_err(|e| DeployError::Jobs(e.to_string()))?;
    // `next_queuable` moved it to in_progress; announce the transition.
    let _ = deps.events.send(WsEvent::deployment_status_changed(
        &next.uuid,
        DeploymentStatus::InProgress,
    ));
    Ok(Some(next.uuid))
}
