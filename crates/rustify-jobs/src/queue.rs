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
        let _ = (kind, handler);
        todo!("register handler")
    }

    pub fn get(&self, kind: &str) -> Option<Arc<dyn JobHandler>> {
        let _ = kind;
        todo!("look up handler")
    }
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
        let _ = (kind, payload, run_at);
        todo!("insert job row")
    }

    /// Run `workers` claim loops until `shutdown` is cancelled; each worker
    /// finishes its in-flight job before stopping (drain).
    pub async fn run(&self, workers: usize, registry: JobRegistry, shutdown: CancellationToken) {
        let _ = (workers, registry, shutdown);
        todo!("worker loops")
    }
}
