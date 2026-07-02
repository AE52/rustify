//! Scripted command-executor test double for the deployment engine.
//!
//! [`FakeExecutor`] records every script it is asked to run (in order) and
//! answers with a substring-keyed scripted response. It never touches SSH or
//! Docker, so engine logic is exercised deterministically. An optional
//! cancel-hook cancels a [`CancellationToken`] the instant a given script runs,
//! letting tests drive the "cancel between steps" path.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use rustify_core::{CommandExecutor, ExecError, ExecEvent, ExecOpts, ExecOutput, ServerConn};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct Rule {
    needle: String,
    stdout: String,
    stderr: String,
    exit_code: i32,
}

/// A scripted, recording [`CommandExecutor`]. Build with the `respond*`
/// helpers, then wrap in an `Arc` and hand to the engine; keep a clone of the
/// `Arc` to inspect [`FakeExecutor::scripts`] afterwards.
pub struct FakeExecutor {
    rules: Vec<Rule>,
    scripts: Mutex<Vec<String>>,
    uploads: Mutex<Vec<(PathBuf, String)>>,
    cancel: Option<(String, CancellationToken)>,
    fail_connection_on: Option<String>,
}

impl FakeExecutor {
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            scripts: Mutex::new(Vec::new()),
            uploads: Mutex::new(Vec::new()),
            cancel: None,
            fail_connection_on: None,
        }
    }

    /// Any script containing `needle` returns `stdout` with exit code 0.
    pub fn respond(mut self, needle: &str, stdout: &str) -> Self {
        self.rules.push(Rule {
            needle: needle.to_string(),
            stdout: stdout.to_string(),
            stderr: String::new(),
            exit_code: 0,
        });
        self
    }

    /// Any script containing `needle` returns the given stdout/stderr/exit code.
    pub fn respond_full(mut self, needle: &str, stdout: &str, stderr: &str, code: i32) -> Self {
        self.rules.push(Rule {
            needle: needle.to_string(),
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            exit_code: code,
        });
        self
    }

    /// Cancel `token` the moment a script containing `needle` runs (after it is
    /// recorded), so the engine's next pre-command cancellation check trips.
    pub fn cancel_after(mut self, needle: &str, token: CancellationToken) -> Self {
        self.cancel = Some((needle.to_string(), token));
        self
    }

    /// Fail with a connection error on any script containing `needle`.
    pub fn fail_connection_on(mut self, needle: &str) -> Self {
        self.fail_connection_on = Some(needle.to_string());
        self
    }

    /// Every recorded script, in the order it was dispatched.
    pub fn scripts(&self) -> Vec<String> {
        self.scripts.lock().unwrap().clone()
    }

    /// Every recorded upload as (local path, remote path).
    pub fn uploads(&self) -> Vec<(PathBuf, String)> {
        self.uploads.lock().unwrap().clone()
    }

    /// Index of the first recorded script containing `needle`, if any.
    pub fn index_of(&self, needle: &str) -> Option<usize> {
        self.scripts().iter().position(|s| s.contains(needle))
    }

    /// Whether any recorded script contains `needle`.
    pub fn ran(&self, needle: &str) -> bool {
        self.index_of(needle).is_some()
    }

    fn record_and_respond(&self, script: &str) -> Result<ExecOutput, ExecError> {
        self.scripts.lock().unwrap().push(script.to_string());

        if let Some(needle) = &self.fail_connection_on
            && script.contains(needle.as_str())
        {
            return Err(ExecError::Connection("fake connection failure".into()));
        }

        if let Some((needle, token)) = &self.cancel
            && script.contains(needle.as_str())
        {
            token.cancel();
        }

        let rule = self.rules.iter().find(|r| script.contains(&r.needle));
        Ok(match rule {
            Some(r) => ExecOutput {
                exit_code: r.exit_code,
                stdout: r.stdout.clone(),
                stderr: r.stderr.clone(),
            },
            None => ExecOutput {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            },
        })
    }
}

impl Default for FakeExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CommandExecutor for FakeExecutor {
    async fn exec(
        &self,
        _conn: &ServerConn,
        script: &str,
        _opts: ExecOpts,
    ) -> Result<ExecOutput, ExecError> {
        self.record_and_respond(script)
    }

    async fn exec_streaming(
        &self,
        _conn: &ServerConn,
        script: &str,
        _opts: ExecOpts,
        tx: mpsc::Sender<ExecEvent>,
    ) -> Result<ExecOutput, ExecError> {
        let out = self.record_and_respond(script)?;
        for line in out.stdout.lines() {
            let _ = tx.send(ExecEvent::Stdout(line.to_string())).await;
        }
        for line in out.stderr.lines() {
            let _ = tx.send(ExecEvent::Stderr(line.to_string())).await;
        }
        Ok(out)
    }

    async fn upload(
        &self,
        _conn: &ServerConn,
        local: &Path,
        remote: &str,
    ) -> Result<(), ExecError> {
        self.uploads
            .lock()
            .unwrap()
            .push((local.to_path_buf(), remote.to_string()));
        Ok(())
    }
}
