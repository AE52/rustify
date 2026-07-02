//! Scheduler tick-skip behavior.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use rustify_jobs::Scheduler;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

#[sqlx::test(migrations = "tests/migrations")]
async fn scheduler_skips_ticks_while_task_running(_pool: PgPool) {
    let runs = Arc::new(AtomicU32::new(0));
    let shutdown = CancellationToken::new();
    let mut scheduler = Scheduler::new(shutdown.clone());

    // Period 50ms, task takes 120ms: without skip-if-running ~10 runs fit in
    // 500ms; with it, at most ceil(500/120)+1.
    scheduler.every(Duration::from_millis(50), "busy_task", {
        let runs = runs.clone();
        move || {
            let runs = runs.clone();
            async move {
                runs.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(120)).await;
            }
        }
    });

    tokio::time::sleep(Duration::from_millis(500)).await;
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(2), scheduler.join())
        .await
        .expect("scheduler did not stop after shutdown");

    let runs = runs.load(Ordering::SeqCst);
    assert!(runs >= 2, "scheduler barely ran: {runs} runs");
    assert!(
        runs <= 6,
        "ticks overlapped instead of skipping: {runs} runs in 500ms"
    );
}
