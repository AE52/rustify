use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

/// Jobs are dropped (never claimed again) once they fail this many attempts.
pub const MAX_ATTEMPTS: i32 = 3;

#[derive(Debug, thiserror::Error)]
pub enum JobsError {
    #[error("database: {0}")]
    Database(#[from] sqlx::Error),
}

#[async_trait::async_trait]
pub trait JobHandler: Send + Sync {
    async fn run(&self, payload: Value) -> anyhow::Result<()>;
}

/// Maps a job `kind` to its handler.
#[derive(Clone, Default)]
pub struct JobRegistry {
    handlers: HashMap<String, Arc<dyn JobHandler>>,
}

impl JobRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, kind: &str, handler: Arc<dyn JobHandler>) {
        self.handlers.insert(kind.to_string(), handler);
    }

    pub fn get(&self, kind: &str) -> Option<Arc<dyn JobHandler>> {
        self.handlers.get(kind).cloned()
    }
}

/// One row claimed from the `jobs` table.
#[derive(Debug, sqlx::FromRow)]
struct ClaimedJob {
    id: i64,
    kind: String,
    payload: Value,
    attempts: i32,
}

/// Queue backed by the `jobs` table (contract C6).
#[derive(Clone)]
pub struct JobQueue {
    pool: PgPool,
    poll_interval: Duration,
    backoff_base: Duration,
    stale_lock_timeout: Duration,
}

impl JobQueue {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            poll_interval: Duration::from_secs(1),
            backoff_base: Duration::from_secs(10),
            stale_lock_timeout: Duration::from_secs(300),
        }
    }

    /// How often idle workers poll for ready jobs. Default 1s.
    pub fn with_poll_interval(mut self, poll_interval: Duration) -> Self {
        self.poll_interval = poll_interval;
        self
    }

    /// Backoff after a failed attempt is `backoff_base * attempts`. Default 10s.
    pub fn with_backoff_base(mut self, backoff_base: Duration) -> Self {
        self.backoff_base = backoff_base;
        self
    }

    /// Locks older than this are considered abandoned and re-claimable;
    /// running workers heartbeat-re-lock at half this period. Default 300s.
    pub fn with_stale_lock_timeout(mut self, stale_lock_timeout: Duration) -> Self {
        self.stale_lock_timeout = stale_lock_timeout;
        self
    }

    /// Insert a job; `run_at = None` means run as soon as possible.
    pub async fn enqueue(
        &self,
        kind: &str,
        payload: Value,
        run_at: Option<DateTime<Utc>>,
    ) -> Result<i64, JobsError> {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO jobs (kind, payload, run_at) VALUES ($1, $2, COALESCE($3, now()))
             RETURNING id",
        )
        .bind(kind)
        .bind(payload)
        .bind(run_at)
        .fetch_one(&self.pool)
        .await?;
        Ok(id)
    }

    /// Run `workers` claim loops until `shutdown` is cancelled; each worker
    /// finishes its in-flight job before stopping (drain).
    pub async fn run(&self, workers: usize, registry: JobRegistry, shutdown: CancellationToken) {
        let mut handles = Vec::with_capacity(workers);
        for n in 0..workers {
            let worker = Worker {
                queue: self.clone(),
                registry: registry.clone(),
                id: format!("{}-w{n}", std::process::id()),
                shutdown: shutdown.clone(),
            };
            handles.push(tokio::spawn(worker.run_loop()));
        }
        for handle in handles {
            if let Err(err) = handle.await {
                tracing::error!(error = %err, "job worker task failed");
            }
        }
    }
}

/// Backoff before retry number `attempts + 1`: `base * attempts` (10s, 20s with defaults).
fn backoff_duration(base: Duration, attempts: i32) -> Duration {
    base.saturating_mul(attempts.max(1) as u32)
}

struct Worker {
    queue: JobQueue,
    registry: JobRegistry,
    id: String,
    shutdown: CancellationToken,
}

impl Worker {
    async fn run_loop(self) {
        loop {
            if self.shutdown.is_cancelled() {
                return;
            }
            match self.claim().await {
                Ok(Some(job)) => {
                    // Not raced against shutdown: an in-flight job always
                    // finishes (drain semantics).
                    self.execute(job).await;
                }
                Ok(None) => {
                    if self.idle().await {
                        return;
                    }
                }
                Err(err) => {
                    tracing::error!(worker = %self.id, error = %err, "job claim failed");
                    if self.idle().await {
                        return;
                    }
                }
            }
        }
    }

    /// Sleep one poll interval; returns true when shutdown was requested.
    async fn idle(&self) -> bool {
        tokio::select! {
            biased;
            _ = self.shutdown.cancelled() => true,
            _ = tokio::time::sleep(self.queue.poll_interval) => false,
        }
    }

    /// Claim one ready job with `FOR UPDATE SKIP LOCKED` so no two workers
    /// ever hold the same row. Locks older than `stale_lock_timeout` are
    /// treated as abandoned by a dead worker and re-claimed.
    async fn claim(&self) -> Result<Option<ClaimedJob>, JobsError> {
        let job = sqlx::query_as::<_, ClaimedJob>(
            "UPDATE jobs SET locked_at = now(), locked_by = $1
             WHERE id = (
                 SELECT id FROM jobs
                 WHERE run_at <= now()
                   AND attempts < $2
                   AND (locked_at IS NULL OR locked_at < now() - ($3 * interval '1 second'))
                 ORDER BY run_at
                 FOR UPDATE SKIP LOCKED
                 LIMIT 1
             )
             RETURNING id, kind, payload, attempts",
        )
        .bind(&self.id)
        .bind(MAX_ATTEMPTS)
        .bind(self.queue.stale_lock_timeout.as_secs_f64())
        .fetch_optional(&self.pool())
        .await?;
        Ok(job)
    }

    fn pool(&self) -> PgPool {
        self.queue.pool.clone()
    }

    async fn execute(&self, job: ClaimedJob) {
        let handler = self.registry.get(&job.kind);
        let run = async {
            match handler {
                Some(handler) => handler.run(job.payload.clone()).await,
                None => Err(anyhow::anyhow!(
                    "no handler registered for kind '{}'",
                    job.kind
                )),
            }
        };
        tokio::pin!(run);

        // Heartbeat re-lock so a long-running job is not mistaken for an
        // abandoned one and re-claimed by another worker.
        let heartbeat_period = (self.queue.stale_lock_timeout / 2).max(Duration::from_millis(50));
        let mut heartbeat = tokio::time::interval(heartbeat_period);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        heartbeat.tick().await; // consume the immediate first tick

        let result = loop {
            tokio::select! {
                result = &mut run => break result,
                _ = heartbeat.tick() => {
                    if let Err(err) =
                        sqlx::query("UPDATE jobs SET locked_at = now() WHERE id = $1 AND locked_by = $2")
                            .bind(job.id)
                            .bind(&self.id)
                            .execute(&self.queue.pool)
                            .await
                    {
                        tracing::warn!(worker = %self.id, job_id = job.id, error = %err, "heartbeat re-lock failed");
                    }
                }
            }
        };

        match result {
            Ok(()) => {
                if let Err(err) = sqlx::query("DELETE FROM jobs WHERE id = $1")
                    .bind(job.id)
                    .execute(&self.queue.pool)
                    .await
                {
                    tracing::error!(worker = %self.id, job_id = job.id, error = %err, "failed to delete finished job");
                }
            }
            Err(job_err) => {
                let attempts = job.attempts + 1;
                let backoff = backoff_duration(self.queue.backoff_base, attempts);
                if attempts >= MAX_ATTEMPTS {
                    tracing::error!(worker = %self.id, job_id = job.id, kind = %job.kind, attempts, error = %job_err, "job dropped after max attempts");
                } else {
                    tracing::warn!(worker = %self.id, job_id = job.id, kind = %job.kind, attempts, error = %job_err, "job failed, will retry");
                }
                // attempts >= MAX_ATTEMPTS rows are never claimed again
                // ("dropped"), with last_error kept for inspection.
                if let Err(err) = sqlx::query(
                    "UPDATE jobs SET attempts = $2, last_error = $3,
                         locked_at = NULL, locked_by = NULL,
                         run_at = now() + ($4 * interval '1 second')
                     WHERE id = $1",
                )
                .bind(job.id)
                .bind(attempts)
                .bind(format!("{job_err:#}"))
                .bind(backoff.as_secs_f64())
                .execute(&self.queue.pool)
                .await
                {
                    tracing::error!(worker = %self.id, job_id = job.id, error = %err, "failed to record job failure");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_is_10s_times_attempts_by_default() {
        let base = Duration::from_secs(10);
        assert_eq!(backoff_duration(base, 1), Duration::from_secs(10));
        assert_eq!(backoff_duration(base, 2), Duration::from_secs(20));
        assert_eq!(backoff_duration(base, 0), Duration::from_secs(10));
    }
}
