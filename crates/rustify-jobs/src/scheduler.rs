use std::future::Future;
use std::time::Duration;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Fixed-period background tasks with per-tick skip-if-still-running.
pub struct Scheduler {
    shutdown: CancellationToken,
    handles: Vec<JoinHandle<()>>,
}

impl Scheduler {
    pub fn new(shutdown: CancellationToken) -> Self {
        Self {
            shutdown,
            handles: Vec::new(),
        }
    }

    /// Run `task` every `period`. The task is awaited inline in the interval
    /// loop and missed ticks are skipped, so a run that overlaps the next
    /// tick delays it instead of stacking concurrent runs.
    pub fn every<F, Fut>(&mut self, period: Duration, name: &str, task: F)
    where
        F: Fn() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let shutdown = self.shutdown.clone();
        let name = name.to_string();
        self.handles.push(tokio::spawn(async move {
            let mut interval = tokio::time::interval(period);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                // `biased` polls shutdown first: once cancelled, no new run
                // ever starts even when a tick is already pending.
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => return,
                    _ = interval.tick() => {}
                }
                tracing::debug!(task = %name, "scheduler tick");
                // In-flight runs always finish; shutdown is only observed
                // between ticks.
                task().await;
            }
        }));
    }

    /// Wait for all loops to observe shutdown and finish their in-flight run.
    pub async fn join(self) {
        for handle in self.handles {
            if let Err(err) = handle.await {
                tracing::error!(error = %err, "scheduler task failed");
            }
        }
    }
}
