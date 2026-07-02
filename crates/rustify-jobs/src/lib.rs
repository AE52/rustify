//! Postgres-backed job queue and in-process scheduler for Rustify.
//!
//! - [`JobQueue`]: enqueue rows into the `jobs` table (contract C6) and run
//!   worker loops that claim jobs with `FOR UPDATE SKIP LOCKED`, retry with
//!   `10s * attempts` backoff, and drop jobs after 3 attempts recording
//!   `last_error`.
//! - [`Scheduler`]: fixed-period tokio interval loops that skip a tick when
//!   the previous run is still in flight.
#![forbid(unsafe_code)]

mod queue;
mod scheduler;

pub use queue::{JobHandler, JobQueue, JobRegistry, JobsError, MAX_ATTEMPTS};
pub use scheduler::Scheduler;
