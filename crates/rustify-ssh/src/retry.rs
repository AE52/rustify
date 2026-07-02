//! Retry with exponential backoff on connection-class SSH failures.
//!
//! Behavior ported from Coolify's `SshRetryable` trait
//! (coolify/app/Traits/SshRetryable.php): `isRetryableSshError` (lines 12-54)
//! provides the retryable substring set; `calculateRetryDelay` (lines 59-68)
//! computes `base * multiplier^attempt`. Rustify pins the backoff to the Track
//! B brief's `2s / 4s / 8s` schedule and retries only errors classified as
//! `ExecError::Connection`.

use rustify_core::exec::{ExecError, ExecOutput};
use std::future::Future;
use std::time::Duration;

/// Backoff delays between retries, in seconds (brief: 2s / 4s / 8s). The
/// length is also the maximum number of retries.
pub const RETRY_DELAYS_SECS: [u64; 3] = [2, 4, 8];

/// Substrings (case-insensitive) that mark an SSH failure as a transient
/// connection-class error worth retrying. Ported verbatim from Coolify's
/// `isRetryableSshError` pattern list.
const RETRYABLE_PATTERNS: &[&str] = &[
    "kex_exchange_identification",
    "connection reset by peer",
    "connection refused",
    "connection timed out",
    "connection closed by remote host",
    "ssh_exchange_identification",
    "bad file descriptor",
    "broken pipe",
    "no route to host",
    "network is unreachable",
    "host is down",
    "no buffer space available",
    "connection reset by",
    "permission denied, please try again",
    "received disconnect from",
    "disconnected from",
    "lost connection",
    "timeout, server not responding",
    "cannot assign requested address",
    "network is down",
    "host key verification failed",
    "operation timed out",
    "connection closed unexpectedly",
    "remote host closed connection",
    "authentication failed",
    "too many authentication failures",
];

/// Whether `stderr` names a transient connection-class SSH failure.
pub fn is_retryable(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    RETRYABLE_PATTERNS.iter().any(|p| lower.contains(p))
}

/// Backoff duration before the retry after the given zero-based attempt index.
pub fn backoff_delay(attempt: usize) -> Duration {
    let idx = attempt.min(RETRY_DELAYS_SECS.len() - 1);
    Duration::from_secs(RETRY_DELAYS_SECS[idx])
}

/// Run `op`, retrying on `ExecError::Connection` with 2s/4s/8s backoff. Any
/// other error (or success) returns immediately.
pub async fn run_with_retry<F, Fut>(mut op: F) -> Result<ExecOutput, ExecError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<ExecOutput, ExecError>>,
{
    let mut attempt = 0usize;
    loop {
        match op().await {
            Ok(out) => return Ok(out),
            Err(err) => {
                let retryable = matches!(err, ExecError::Connection(_));
                if retryable && attempt < RETRY_DELAYS_SECS.len() {
                    tracing::warn!(
                        attempt = attempt + 1,
                        max = RETRY_DELAYS_SECS.len(),
                        error = %err,
                        "retrying ssh command after connection-class failure"
                    );
                    tokio::time::sleep(backoff_delay(attempt)).await;
                    attempt += 1;
                    continue;
                }
                return Err(err);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn classifies_retryable_errors() {
        assert!(is_retryable(
            "kex_exchange_identification: Connection reset by peer"
        ));
        assert!(is_retryable(
            "ssh: connect to host x port 22: Connection refused"
        ));
        assert!(is_retryable("Operation timed out"));
    }

    #[test]
    fn non_connection_errors_not_retryable() {
        assert!(!is_retryable("bash: line 1: foo: command not found"));
        assert!(!is_retryable("exit status 1"));
    }

    #[test]
    fn backoff_schedule_is_2_4_8() {
        assert_eq!(backoff_delay(0), Duration::from_secs(2));
        assert_eq!(backoff_delay(1), Duration::from_secs(4));
        assert_eq!(backoff_delay(2), Duration::from_secs(8));
        // Saturates at the last delay.
        assert_eq!(backoff_delay(9), Duration::from_secs(8));
    }

    #[tokio::test(start_paused = true)]
    async fn retries_connection_errors_then_succeeds() {
        let calls = Cell::new(0u32);
        let out = run_with_retry(|| {
            let n = calls.get();
            calls.set(n + 1);
            async move {
                if n < 2 {
                    Err(ExecError::Connection("connection refused".into()))
                } else {
                    Ok(ExecOutput {
                        exit_code: 0,
                        stdout: "ok".into(),
                        stderr: String::new(),
                    })
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(out.stdout, "ok");
        assert_eq!(calls.get(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn does_not_retry_non_connection_errors() {
        let calls = Cell::new(0u32);
        let res = run_with_retry(|| {
            calls.set(calls.get() + 1);
            async move {
                Err::<ExecOutput, _>(ExecError::NonZero {
                    code: 1,
                    stdout: String::new(),
                    stderr: "boom".into(),
                })
            }
        })
        .await;
        assert!(matches!(res, Err(ExecError::NonZero { code: 1, .. })));
        assert_eq!(calls.get(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn gives_up_after_max_retries() {
        let calls = Cell::new(0u32);
        let res = run_with_retry(|| {
            calls.set(calls.get() + 1);
            async move { Err::<ExecOutput, _>(ExecError::Connection("connection refused".into())) }
        })
        .await;
        assert!(matches!(res, Err(ExecError::Connection(_))));
        // 1 initial attempt + 3 retries.
        assert_eq!(calls.get(), 4);
    }
}
