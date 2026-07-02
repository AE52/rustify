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
        Self { shutdown, handles: Vec::new() }
    }

    /// Run `task` every `period`. A tick that fires while the previous run is
    /// still in flight is skipped, never queued or overlapped.
    pub fn every<F, Fut>(&mut self, period: Duration, name: &str, task: F)
    where
        F: Fn() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let _ = (period, name, task, &self.shutdown);
        todo!("interval loop")
    }

    /// Wait for all loops to observe shutdown and finish their in-flight run.
    pub async fn join(self) {
        todo!("await handles")
    }
}
