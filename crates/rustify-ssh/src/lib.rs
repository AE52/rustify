#![forbid(unsafe_code)]

//! rustify-ssh: an [`SshExecutor`] implementing the core [`CommandExecutor`]
//! contract (C1) over the system `ssh`/`scp` binaries.
//!
//! Commands are transported to the remote host through a `bash -se` heredoc
//! ([`command`]), reusing a per-server ControlMaster multiplexed connection
//! when one is healthy ([`mux`]), with a silent fallback to a fresh connection.
//! Connection-class failures are retried with exponential backoff ([`retry`]),
//! and private keys are materialized `0600` on the host ([`keys`]).

pub mod command;
pub mod keys;
pub mod mux;
pub mod retry;

use async_trait::async_trait;
use command::{NonceGen, RandomNonce};
use mux::MuxManager;
use rustify_core::exec::{CommandExecutor, ExecError, ExecEvent, ExecOpts, ExecOutput, ServerConn};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

/// Default remote-command timeout (seconds) when `ExecOpts::timeout_secs` is
/// `None` (contract C1).
const DEFAULT_TIMEOUT_SECS: u32 = 3600;
/// Exit code GNU `timeout` reports when it kills a timed-out command.
const TIMEOUT_EXIT_CODE: i32 = 124;
/// ssh's exit code for connection/authentication failures.
const SSH_FAILURE_EXIT_CODE: i32 = 255;

/// Executes commands on remote servers over `ssh`, with connection
/// multiplexing, retries and heredoc script transport.
pub struct SshExecutor {
    mux: MuxManager,
    nonce: Box<dyn NonceGen>,
}

impl SshExecutor {
    /// Create an executor keeping its ControlMaster sockets under `mux_dir`.
    pub fn new(mux_dir: PathBuf) -> Self {
        Self {
            mux: MuxManager::new(mux_dir),
            nonce: Box::new(RandomNonce),
        }
    }

    /// Create an executor with a custom nonce generator (tests: deterministic
    /// heredoc delimiters).
    pub fn with_nonce(mux_dir: PathBuf, nonce: Box<dyn NonceGen>) -> Self {
        Self {
            mux: MuxManager::new(mux_dir),
            nonce,
        }
    }

    /// Resolve the multiplexing socket to use for this call, if any. Honors
    /// `disable_mux` and falls back silently when no healthy master exists.
    async fn resolve_mux(&self, conn: &ServerConn, opts: &ExecOpts) -> Option<PathBuf> {
        if opts.disable_mux {
            return None;
        }
        self.mux.ensure(conn).await
    }
}

/// Map a completed process into the C1 result surface.
fn classify(
    code: i32,
    stdout: String,
    stderr: String,
    timeout_secs: u32,
) -> Result<ExecOutput, ExecError> {
    if code == 0 {
        Ok(ExecOutput {
            exit_code: 0,
            stdout,
            stderr,
        })
    } else if code == TIMEOUT_EXIT_CODE {
        Err(ExecError::Timeout(timeout_secs))
    } else if code == SSH_FAILURE_EXIT_CODE || retry::is_retryable(&stderr) {
        Err(ExecError::Connection(stderr))
    } else {
        Err(ExecError::NonZero {
            code,
            stdout,
            stderr,
        })
    }
}

/// Spawn `cmd` under `sh -c`, draining stdout/stderr concurrently. When `tx`
/// is `Some`, each complete line is forwarded as an [`ExecEvent`] as it
/// arrives. Returns `(exit_code, stdout, stderr)` with lines rejoined by `\n`.
async fn run_shell(
    cmd: &str,
    tx: Option<mpsc::Sender<ExecEvent>>,
) -> Result<(i32, String, String), ExecError> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Kill the local process if the future is dropped (e.g. a deploy step
        // aborted on cancellation) so we don't leak `ssh`/`scp` children.
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| ExecError::Io(e.to_string()))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ExecError::Io("failed to capture stdout".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ExecError::Io("failed to capture stderr".into()))?;

    let tx_out = tx.clone();
    let out_fut = async move {
        let mut lines = BufReader::new(stdout).lines();
        let mut acc: Vec<String> = Vec::new();
        while let Some(line) = lines
            .next_line()
            .await
            .map_err(|e| ExecError::Io(e.to_string()))?
        {
            if let Some(t) = &tx_out {
                let _ = t.send(ExecEvent::Stdout(line.clone())).await;
            }
            acc.push(line);
        }
        Ok::<String, ExecError>(acc.join("\n"))
    };

    let err_fut = async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut acc: Vec<String> = Vec::new();
        while let Some(line) = lines
            .next_line()
            .await
            .map_err(|e| ExecError::Io(e.to_string()))?
        {
            if let Some(t) = &tx {
                let _ = t.send(ExecEvent::Stderr(line.clone())).await;
            }
            acc.push(line);
        }
        Ok::<String, ExecError>(acc.join("\n"))
    };

    let wait_fut = child.wait();
    let (out_res, err_res, status_res) = tokio::join!(out_fut, err_fut, wait_fut);
    let stdout = out_res?;
    let stderr = err_res?;
    let status = status_res.map_err(|e| ExecError::Io(e.to_string()))?;
    Ok((status.code().unwrap_or(-1), stdout, stderr))
}

#[async_trait]
impl CommandExecutor for SshExecutor {
    async fn exec(
        &self,
        conn: &ServerConn,
        script: &str,
        opts: ExecOpts,
    ) -> Result<ExecOutput, ExecError> {
        let timeout_secs = opts.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
        let mux_socket = self.resolve_mux(conn, &opts).await;
        retry::run_with_retry(|| async {
            let nonce = self.nonce.nonce();
            let cmd = command::build_ssh_command(
                conn,
                script,
                timeout_secs,
                mux_socket.as_deref(),
                &nonce,
            );
            let (code, stdout, stderr) = run_shell(&cmd, None).await?;
            classify(code, stdout, stderr, timeout_secs)
        })
        .await
    }

    async fn exec_streaming(
        &self,
        conn: &ServerConn,
        script: &str,
        opts: ExecOpts,
        tx: mpsc::Sender<ExecEvent>,
    ) -> Result<ExecOutput, ExecError> {
        let timeout_secs = opts.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
        let mux_socket = self.resolve_mux(conn, &opts).await;
        retry::run_with_retry(|| async {
            let nonce = self.nonce.nonce();
            let cmd = command::build_ssh_command(
                conn,
                script,
                timeout_secs,
                mux_socket.as_deref(),
                &nonce,
            );
            let (code, stdout, stderr) = run_shell(&cmd, Some(tx.clone())).await?;
            classify(code, stdout, stderr, timeout_secs)
        })
        .await
    }

    async fn upload(&self, conn: &ServerConn, local: &Path, remote: &str) -> Result<(), ExecError> {
        let mux_socket = self.resolve_mux(conn, &ExecOpts::default()).await;
        retry::run_with_retry(|| async {
            let cmd = command::build_scp_command(
                conn,
                local,
                remote,
                DEFAULT_TIMEOUT_SECS,
                mux_socket.as_deref(),
            );
            let (code, stdout, stderr) = run_shell(&cmd, None).await?;
            classify(code, stdout, stderr, DEFAULT_TIMEOUT_SECS)
        })
        .await
        .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_zero_is_ok() {
        let out = classify(0, "hi".into(), String::new(), 3600).unwrap();
        assert_eq!(out.exit_code, 0);
        assert_eq!(out.stdout, "hi");
    }

    #[test]
    fn classify_timeout_exit_maps_to_timeout() {
        let err = classify(TIMEOUT_EXIT_CODE, String::new(), String::new(), 42).unwrap_err();
        assert!(matches!(err, ExecError::Timeout(42)));
    }

    #[test]
    fn classify_ssh_255_maps_to_connection() {
        let err = classify(255, String::new(), "connection refused".into(), 3600).unwrap_err();
        assert!(matches!(err, ExecError::Connection(_)));
    }

    #[test]
    fn classify_retryable_stderr_maps_to_connection() {
        let err =
            classify(1, String::new(), "kex_exchange_identification".into(), 3600).unwrap_err();
        assert!(matches!(err, ExecError::Connection(_)));
    }

    #[test]
    fn classify_plain_nonzero_maps_to_nonzero() {
        let err = classify(2, "o".into(), "command not found".into(), 3600).unwrap_err();
        match err {
            ExecError::NonZero { code, stderr, .. } => {
                assert_eq!(code, 2);
                assert_eq!(stderr, "command not found");
            }
            other => panic!("expected NonZero, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_shell_captures_stdout_stderr_and_code() {
        let (code, out, err) = run_shell("printf 'a\\nb\\n'; printf 'e1\\n' 1>&2; exit 3", None)
            .await
            .unwrap();
        assert_eq!(code, 3);
        assert_eq!(out, "a\nb");
        assert_eq!(err, "e1");
    }

    #[tokio::test]
    async fn run_shell_streams_line_events() {
        let (tx, mut rx) = mpsc::channel(16);
        let handle = tokio::spawn(async move { run_shell("printf 'x\\ny\\n'", Some(tx)).await });
        let mut events = Vec::new();
        while let Some(ev) = rx.recv().await {
            events.push(ev);
        }
        let (code, out, _) = handle.await.unwrap().unwrap();
        assert_eq!(code, 0);
        assert_eq!(out, "x\ny");
        assert!(events.contains(&ExecEvent::Stdout("x".into())));
        assert!(events.contains(&ExecEvent::Stdout("y".into())));
    }
}
