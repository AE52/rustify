//! Integration tests for the `jobs`-table-backed queue (contract C6).
//!
//! Uses a test-only copy of the `jobs` migration (see tests/migrations/0001_jobs.sql)
//! until rustify-db's MIGRATOR lands on master.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use rustify_jobs::{JobHandler, JobQueue, JobRegistry};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

/// Poll `cond` every 20ms until it returns true or `timeout` elapses.
async fn wait_until<F, Fut>(timeout: Duration, mut cond: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if cond().await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Increments a per-job counter row; a job claimed twice shows runs > 1.
struct CountingHandler {
    pool: PgPool,
}

#[async_trait::async_trait]
impl JobHandler for CountingHandler {
    async fn run(&self, payload: Value) -> anyhow::Result<()> {
        // Hold the job briefly so all 4 workers overlap.
        tokio::time::sleep(Duration::from_millis(10)).await;
        let job_no = payload["job_no"].as_i64().unwrap();
        sqlx::query(
            "INSERT INTO test_counters (job_no, runs) VALUES ($1, 1)
             ON CONFLICT (job_no) DO UPDATE SET runs = test_counters.runs + 1",
        )
        .bind(job_no)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[sqlx::test(migrations = "tests/migrations")]
async fn four_workers_run_each_job_exactly_once(pool: PgPool) {
    sqlx::query("CREATE TABLE test_counters (job_no BIGINT PRIMARY KEY, runs INT NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();

    let queue = JobQueue::new(pool.clone()).with_poll_interval(Duration::from_millis(20));
    for n in 0..20i64 {
        queue
            .enqueue("count", json!({ "job_no": n }), None)
            .await
            .unwrap();
    }

    let mut registry = JobRegistry::new();
    registry.register("count", Arc::new(CountingHandler { pool: pool.clone() }));

    let shutdown = CancellationToken::new();
    let run = tokio::spawn({
        let queue = queue.clone();
        let shutdown = shutdown.clone();
        async move { queue.run(4, registry, shutdown).await }
    });

    let drained = wait_until(Duration::from_secs(15), || {
        let pool = pool.clone();
        async move {
            let left: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs")
                .fetch_one(&pool)
                .await
                .unwrap();
            left == 0
        }
    })
    .await;
    assert!(drained, "queue did not drain in time");

    shutdown.cancel();
    run.await.unwrap();

    let rows: Vec<(i64, i32)> =
        sqlx::query_as("SELECT job_no, runs FROM test_counters ORDER BY job_no")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(rows.len(), 20, "every enqueued job must have run");
    for (job_no, runs) in rows {
        assert_eq!(
            runs, 1,
            "job {job_no} ran {runs} times, expected exactly once"
        );
    }
}

/// Always fails; counts invocations.
struct FailingHandler {
    calls: Arc<AtomicU32>,
}

#[async_trait::async_trait]
impl JobHandler for FailingHandler {
    async fn run(&self, _payload: Value) -> anyhow::Result<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("boom")
    }
}

#[sqlx::test(migrations = "tests/migrations")]
async fn failing_job_retries_with_backoff_then_drops(pool: PgPool) {
    let backoff_base = Duration::from_millis(150);
    let queue = JobQueue::new(pool.clone())
        .with_poll_interval(Duration::from_millis(20))
        .with_backoff_base(backoff_base);

    let calls = Arc::new(AtomicU32::new(0));
    let mut registry = JobRegistry::new();
    registry.register(
        "fail",
        Arc::new(FailingHandler {
            calls: calls.clone(),
        }),
    );

    let started = std::time::Instant::now();
    queue.enqueue("fail", json!({}), None).await.unwrap();

    let shutdown = CancellationToken::new();
    let run = tokio::spawn({
        let queue = queue.clone();
        let shutdown = shutdown.clone();
        async move { queue.run(2, registry, shutdown).await }
    });

    let exhausted = wait_until(Duration::from_secs(10), || {
        let pool = pool.clone();
        async move {
            let attempts: Option<i32> = sqlx::query_scalar("SELECT attempts FROM jobs LIMIT 1")
                .fetch_optional(&pool)
                .await
                .unwrap();
            attempts == Some(3)
        }
    })
    .await;
    assert!(exhausted, "job never reached 3 attempts");

    // Retries must respect the 10s*attempts-shaped backoff (scaled down here):
    // attempt 2 waits base*1, attempt 3 waits base*2 => at least base*3 total.
    let elapsed = started.elapsed();
    assert!(
        elapsed >= backoff_base * 3,
        "retries ignored backoff: 3 attempts in {elapsed:?}"
    );

    // Dropped after 3 attempts: give it time to (incorrectly) run again.
    tokio::time::sleep(backoff_base * 4).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "dropped job must not run again"
    );

    let (attempts, last_error, locked): (
        i32,
        Option<String>,
        Option<chrono::DateTime<chrono::Utc>>,
    ) = sqlx::query_as("SELECT attempts, last_error, locked_at FROM jobs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(attempts, 3);
    assert!(
        last_error.as_deref().unwrap_or_default().contains("boom"),
        "last_error must record the handler error, got {last_error:?}"
    );
    assert!(locked.is_none(), "dead job must not stay locked");

    shutdown.cancel();
    run.await.unwrap();
}

/// Sleeps to simulate work in flight; counts completed runs.
struct SlowHandler {
    started: Arc<AtomicBool>,
    completed: Arc<AtomicU32>,
}

#[async_trait::async_trait]
impl JobHandler for SlowHandler {
    async fn run(&self, _payload: Value) -> anyhow::Result<()> {
        self.started.store(true, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(400)).await;
        self.completed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[sqlx::test(migrations = "tests/migrations")]
async fn shutdown_drains_inflight_job_then_stops(pool: PgPool) {
    let started = Arc::new(AtomicBool::new(false));
    let completed = Arc::new(AtomicU32::new(0));

    let queue = JobQueue::new(pool.clone()).with_poll_interval(Duration::from_millis(20));
    let mut registry = JobRegistry::new();
    registry.register(
        "slow",
        Arc::new(SlowHandler {
            started: started.clone(),
            completed: completed.clone(),
        }),
    );

    queue.enqueue("slow", json!({}), None).await.unwrap();

    let shutdown = CancellationToken::new();
    let run = tokio::spawn({
        let queue = queue.clone();
        let shutdown = shutdown.clone();
        async move { queue.run(1, registry, shutdown).await }
    });

    let picked_up = wait_until(Duration::from_secs(5), || {
        let started = started.clone();
        async move { started.load(Ordering::SeqCst) }
    })
    .await;
    assert!(picked_up, "worker never claimed the job");

    // A second ready job that must NOT be claimed once shutdown is requested.
    queue.enqueue("slow", json!({}), None).await.unwrap();
    shutdown.cancel();

    tokio::time::timeout(Duration::from_secs(2), run)
        .await
        .expect("run() did not stop after shutdown")
        .unwrap();

    assert_eq!(
        completed.load(Ordering::SeqCst),
        1,
        "in-flight job must complete during drain; queued job must not start"
    );

    let (remaining, locked): (i64, i64) =
        sqlx::query_as("SELECT count(*), count(locked_at) FROM jobs")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(remaining, 1, "finished job removed, queued job kept");
    assert_eq!(locked, 0, "remaining job must be unclaimed");
}
