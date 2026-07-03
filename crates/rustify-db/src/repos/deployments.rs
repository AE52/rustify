//! Deployment aggregate: the queue that the deploy engine (track E) drains.
//!
//! Transition legality (contract C2) is enforced **in SQL** — an `UPDATE`
//! whose `WHERE` clause names the legal predecessor states, so a lost race
//! reports `false` instead of corrupting state. Admission control
//! (`next_queuable`) implements Coolify's rules
//! (bootstrap/helpers/applications.php `next_queuable`, lines 142-167):
//! at most one in-flight deployment per application, and at most
//! `server_settings.concurrent_builds` in-flight per server, FIFO by
//! `created_at`.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgPool;

use rustify_core::{DeploymentStatus, LogLine, ids};

use crate::DbResult;

/// A row of the `deployments` table.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct Deployment {
    pub id: i64,
    pub uuid: String,
    pub application_id: i64,
    pub server_id: i64,
    pub status: DeploymentStatus,
    pub commit_sha: Option<String>,
    pub commit_message: Option<String>,
    pub force_rebuild: bool,
    pub rollback: bool,
    pub config_snapshot: Option<Value>,
    /// PR number for a PREVIEW deploy; `0` for the production path (migration 0008).
    pub pull_request_id: i32,
    /// Provider (`github`/`gitlab`/`gitea`/`bitbucket`) for a preview deploy.
    pub git_type: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// Parameters for enqueuing a deployment.
#[derive(Debug, Clone, Default)]
pub struct NewDeployment {
    pub application_id: i64,
    pub server_id: i64,
    pub commit_sha: Option<String>,
    pub commit_message: Option<String>,
    pub force_rebuild: bool,
    pub rollback: bool,
    pub config_snapshot: Option<Value>,
    /// PR number for a PREVIEW deploy; `0` (default) is a production deploy.
    pub pull_request_id: i32,
    pub git_type: Option<String>,
}

const COLS: &str = "id, uuid, application_id, server_id, status, commit_sha, commit_message, \
     force_rebuild, rollback, config_snapshot, pull_request_id, git_type, \
     started_at, finished_at, created_at";

#[derive(Clone)]
pub struct DeploymentRepo {
    pool: PgPool,
}

impl DeploymentRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Insert a deployment in the `queued` state and return the full row.
    pub async fn create_queued(&self, new: NewDeployment) -> DbResult<Deployment> {
        let uuid = ids::new_uuid();
        let row = sqlx::query_as::<_, Deployment>(&format!(
            "INSERT INTO deployments
               (uuid, application_id, server_id, status, commit_sha, commit_message,
                force_rebuild, rollback, config_snapshot, pull_request_id, git_type)
             VALUES ($1, $2, $3, 'queued', $4, $5, $6, $7, $8, $9, $10)
             RETURNING {COLS}"
        ))
        .bind(&uuid)
        .bind(new.application_id)
        .bind(new.server_id)
        .bind(new.commit_sha)
        .bind(new.commit_message)
        .bind(new.force_rebuild)
        .bind(new.rollback)
        .bind(new.config_snapshot)
        .bind(new.pull_request_id)
        .bind(new.git_type)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    /// Cancel every QUEUED/IN_PROGRESS deployment for `(application_id,
    /// pull_request_id)` (a PR was closed), returning the cancelled rows so the
    /// caller can log + kill their helper containers. Parity with Coolify's
    /// `CleanupPreviewDeployment::cancelActiveDeployments`.
    pub async fn cancel_active_for_pr(
        &self,
        application_id: i64,
        pull_request_id: i32,
    ) -> DbResult<Vec<Deployment>> {
        let rows = sqlx::query_as::<_, Deployment>(&format!(
            "UPDATE deployments
                SET status = 'cancelled',
                    finished_at = now()
              WHERE application_id = $1
                AND pull_request_id = $2
                AND status IN ('queued', 'in_progress')
              RETURNING {COLS}"
        ))
        .bind(application_id)
        .bind(pull_request_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Attempt the state transition `_ -> next`, enforcing contract C2
    /// legality in the SQL `WHERE` clause. Returns `true` iff exactly one row
    /// moved; `false` means the row was not in a legal predecessor state
    /// (e.g. a concurrent worker already advanced it — the loser of a race).
    pub async fn transition(&self, id: i64, next: DeploymentStatus) -> DbResult<bool> {
        let froms = legal_predecessors(next);
        let result = sqlx::query(
            "UPDATE deployments
               SET status = $2,
                   started_at = CASE WHEN $2 = 'in_progress' THEN now() ELSE started_at END,
                   finished_at = CASE WHEN $2 IN ('finished','failed','cancelled')
                                      THEN now() ELSE finished_at END
             WHERE id = $1 AND status = ANY($3)",
        )
        .bind(id)
        .bind(next)
        .bind(&froms)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Append log lines (contract C3) in a single atomic statement, preserving
    /// their `order`. `LogLine.timestamp` is stored in `created_at`.
    pub async fn append_logs(&self, deployment_id: i64, lines: &[LogLine]) -> DbResult<()> {
        if lines.is_empty() {
            return Ok(());
        }
        let ords: Vec<i64> = lines.iter().map(|l| l.order).collect();
        let kinds: Vec<String> = lines.iter().map(|l| l.kind.clone()).collect();
        let contents: Vec<String> = lines.iter().map(|l| l.content.clone()).collect();
        let hiddens: Vec<bool> = lines.iter().map(|l| l.hidden).collect();
        let batches: Vec<i32> = lines.iter().map(|l| l.batch).collect();
        let times: Vec<DateTime<Utc>> = lines.iter().map(|l| l.timestamp).collect();

        sqlx::query(
            "INSERT INTO deployment_logs (deployment_id, ord, kind, content, hidden, batch, created_at)
             SELECT $1, ord, kind, content, hidden, batch, ts
             FROM UNNEST($2::bigint[], $3::text[], $4::text[], $5::bool[], $6::int[], $7::timestamptz[])
                  AS t(ord, kind, content, hidden, batch, ts)",
        )
        .bind(deployment_id)
        .bind(&ords)
        .bind(&kinds)
        .bind(&contents)
        .bind(&hiddens)
        .bind(&batches)
        .bind(&times)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// All log lines for a deployment, in `order`. Consumed by the API's
    /// `GET /deployments/{uuid}` (contract C5).
    pub async fn logs(&self, deployment_id: i64) -> DbResult<Vec<LogLine>> {
        let rows = sqlx::query_as::<_, LogRow>(
            "SELECT ord, kind, content, hidden, batch, created_at
             FROM deployment_logs WHERE deployment_id = $1 ORDER BY ord",
        )
        .bind(deployment_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(LogRow::into_line).collect())
    }

    /// True if the deployment has been asked to cancel (its status is
    /// `cancelled`). The engine polls this to abort a running build.
    pub async fn cancel_requested(&self, id: i64) -> DbResult<bool> {
        let cancelled: Option<bool> =
            sqlx::query_scalar("SELECT status = 'cancelled' FROM deployments WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(cancelled.unwrap_or(false))
    }

    /// Atomically claim the next runnable queued deployment for `server_id`
    /// and mark it `in_progress`, or return `None` if admission control blocks
    /// (in-flight per-application cap, or server `concurrent_builds` reached).
    ///
    /// A per-server transaction-scoped advisory lock serialises admission
    /// decisions so the in-flight count is never over-committed; the inner
    /// `FOR UPDATE SKIP LOCKED` skips rows a concurrent transition holds.
    pub async fn next_queuable(&self, server_id: i64) -> DbResult<Option<Deployment>> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(server_id)
            .execute(&mut *tx)
            .await?;
        let row = sqlx::query_as::<_, Deployment>(&format!(
            "UPDATE deployments
                SET status = 'in_progress', started_at = now()
              WHERE id = (
                  SELECT d.id FROM deployments d
                  WHERE d.server_id = $1
                    AND d.status = 'queued'
                    AND NOT EXISTS (
                        SELECT 1 FROM deployments ip
                        WHERE ip.application_id = d.application_id
                          AND ip.status = 'in_progress'
                    )
                    AND (
                        SELECT count(*) FROM deployments ac
                        WHERE ac.server_id = $1 AND ac.status = 'in_progress'
                    ) < COALESCE(
                        (SELECT concurrent_builds FROM server_settings WHERE server_id = $1), 2)
                  ORDER BY d.created_at, d.id
                  FOR UPDATE SKIP LOCKED
                  LIMIT 1
              )
              RETURNING {COLS}"
        ))
        .bind(server_id)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn get_by_uuid(&self, uuid: &str) -> DbResult<Option<Deployment>> {
        let row = sqlx::query_as::<_, Deployment>(&format!(
            "SELECT {COLS} FROM deployments WHERE uuid = $1"
        ))
        .bind(uuid)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Deployments for an application, newest first.
    pub async fn list_by_application(&self, application_id: i64) -> DbResult<Vec<Deployment>> {
        let rows = sqlx::query_as::<_, Deployment>(&format!(
            "SELECT {COLS} FROM deployments WHERE application_id = $1 ORDER BY created_at DESC, id DESC"
        ))
        .bind(application_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
}

/// Legal predecessor states of `next` per contract C2 — the single source of
/// truth is `DeploymentStatus::can_transition_to`, reused here rather than
/// duplicated in SQL.
fn legal_predecessors(next: DeploymentStatus) -> Vec<DeploymentStatus> {
    use DeploymentStatus::*;
    [Queued, InProgress, Finished, Failed, Cancelled]
        .into_iter()
        .filter(|from| from.can_transition_to(next))
        .collect()
}

#[derive(sqlx::FromRow)]
struct LogRow {
    ord: i64,
    kind: String,
    content: String,
    hidden: bool,
    batch: i32,
    created_at: DateTime<Utc>,
}

impl LogRow {
    fn into_line(self) -> LogLine {
        LogLine {
            order: self.ord,
            kind: self.kind,
            content: self.content,
            hidden: self.hidden,
            batch: self.batch,
            timestamp: self.created_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustify_core::DeploymentStatus::*;

    #[test]
    fn predecessors_track_contract_c2() {
        assert_eq!(legal_predecessors(InProgress), vec![Queued]);
        assert_eq!(legal_predecessors(Finished), vec![InProgress]);
        assert_eq!(legal_predecessors(Failed), vec![InProgress]);
        assert_eq!(legal_predecessors(Cancelled), vec![Queued, InProgress]);
        assert!(legal_predecessors(Queued).is_empty());
    }
}
