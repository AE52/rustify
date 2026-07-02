//! Integration tests for `SshExecutor` against a live sshd.
//!
//! Gated behind the `ssh-tests` feature and exercised by Track H's harness
//! (a local sshd container). Not part of the default quality gate; compiled
//! via `cargo check --features ssh-tests`.
//!
//! Connection parameters are taken from the environment so the harness can
//! point them at its container:
//!   RUSTIFY_SSH_TEST_HOST (default 127.0.0.1)
//!   RUSTIFY_SSH_TEST_PORT (default 2222)
//!   RUSTIFY_SSH_TEST_USER (default root)
//!   RUSTIFY_SSH_TEST_KEY  (path to a 0600 private key; required)
#![cfg(feature = "ssh-tests")]

use rustify_core::exec::{CommandExecutor, ExecError, ExecEvent, ExecOpts, ServerConn};
use rustify_ssh::SshExecutor;
use std::path::PathBuf;

fn conn() -> ServerConn {
    ServerConn {
        uuid: "itest-server".into(),
        host: std::env::var("RUSTIFY_SSH_TEST_HOST").unwrap_or_else(|_| "127.0.0.1".into()),
        port: std::env::var("RUSTIFY_SSH_TEST_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(2222),
        user: std::env::var("RUSTIFY_SSH_TEST_USER").unwrap_or_else(|_| "root".into()),
        key_path: PathBuf::from(
            std::env::var("RUSTIFY_SSH_TEST_KEY").expect("RUSTIFY_SSH_TEST_KEY must be set"),
        ),
        connection_timeout_secs: 10,
    }
}

fn executor() -> SshExecutor {
    let dir = std::env::temp_dir().join("rustify-ssh-itest-mux");
    SshExecutor::new(dir)
}

#[tokio::test]
async fn echo_hello_returns_stdout() {
    let out = executor()
        .exec(&conn(), "echo hello", ExecOpts::default())
        .await
        .expect("exec ok");
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout.trim(), "hello");
}

#[tokio::test]
async fn streaming_interleaves_stdout_and_stderr() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let exec = executor();
    let handle = tokio::spawn(async move {
        exec.exec_streaming(
            &conn(),
            "echo out1; echo err1 1>&2; echo out2",
            ExecOpts::default(),
            tx,
        )
        .await
    });
    let mut events = Vec::new();
    while let Some(ev) = rx.recv().await {
        events.push(ev);
    }
    let out = handle.await.unwrap().expect("exec ok");
    assert_eq!(out.exit_code, 0);
    assert!(events.contains(&ExecEvent::Stdout("out1".into())));
    assert!(events.contains(&ExecEvent::Stdout("out2".into())));
    assert!(events.contains(&ExecEvent::Stderr("err1".into())));
}

#[tokio::test]
async fn non_zero_exit_maps_to_nonzero() {
    let err = executor()
        .exec(&conn(), "exit 7", ExecOpts::default())
        .await
        .expect_err("should fail");
    match err {
        ExecError::NonZero { code, .. } => assert_eq!(code, 7),
        other => panic!("expected NonZero, got {other:?}"),
    }
}

#[tokio::test]
async fn slow_command_maps_to_timeout() {
    let err = executor()
        .exec(
            &conn(),
            "sleep 5",
            ExecOpts {
                timeout_secs: Some(1),
                disable_mux: false,
            },
        )
        .await
        .expect_err("should time out");
    assert!(matches!(err, ExecError::Timeout(1)));
}
