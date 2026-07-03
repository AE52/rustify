//! Build-server flow: `push_then_pull` must `docker push` on the build server
//! and then switch the SSH target to the deploy server for `docker pull`.

use std::path::PathBuf;
use std::sync::Mutex;

use async_trait::async_trait;
use rustify_core::{CommandExecutor, ExecError, ExecEvent, ExecOpts, ExecOutput, ServerConn};
use rustify_deploy::{plan_build_targets, push_then_pull, registry_image_ref};
use tokio::sync::mpsc;

/// Records every (server-uuid, script) pair so we can assert both what ran and
/// where it ran.
#[derive(Default)]
struct RecordingExecutor {
    calls: Mutex<Vec<(String, String)>>,
}

impl RecordingExecutor {
    fn calls(&self) -> Vec<(String, String)> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl CommandExecutor for RecordingExecutor {
    async fn exec(
        &self,
        conn: &ServerConn,
        script: &str,
        _opts: ExecOpts,
    ) -> Result<ExecOutput, ExecError> {
        self.calls
            .lock()
            .unwrap()
            .push((conn.uuid.clone(), script.to_string()));
        Ok(ExecOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        })
    }

    async fn exec_streaming(
        &self,
        conn: &ServerConn,
        script: &str,
        opts: ExecOpts,
        _tx: mpsc::Sender<ExecEvent>,
    ) -> Result<ExecOutput, ExecError> {
        self.exec(conn, script, opts).await
    }

    async fn upload(
        &self,
        _conn: &ServerConn,
        _local: &std::path::Path,
        _remote: &str,
    ) -> Result<(), ExecError> {
        Ok(())
    }
}

fn conn(uuid: &str) -> ServerConn {
    ServerConn {
        uuid: uuid.to_string(),
        host: format!("{uuid}.example"),
        port: 22,
        user: "root".to_string(),
        key_path: PathBuf::from("/keys/x"),
        connection_timeout_secs: 10,
        proxy_command: None,
    }
}

#[tokio::test]
async fn push_on_build_server_then_pull_on_deploy_server() {
    let exec = RecordingExecutor::default();
    let build = conn("build-srv");
    let deploy = conn("deploy-srv");
    let image = registry_image_ref("ghcr.io/acme/app", Some("sha123")).unwrap();

    push_then_pull(&exec, &build, &deploy, &image)
        .await
        .unwrap();

    let calls = exec.calls();
    assert_eq!(calls.len(), 2);
    // First: push happens on the BUILD server.
    assert_eq!(calls[0].0, "build-srv");
    assert_eq!(calls[0].1, "docker push ghcr.io/acme/app:sha123");
    // Then: the SSH target switches to the DEPLOY server for the pull.
    assert_eq!(calls[1].0, "deploy-srv");
    assert_eq!(calls[1].1, "docker pull ghcr.io/acme/app:sha123");
}

#[tokio::test]
async fn no_split_when_build_server_disabled() {
    // With no build server, the plan builds on the deploy server and no
    // push/pull hop is orchestrated.
    let plan = plan_build_targets(7, None, &[]);
    assert!(!plan.use_build_server);
    assert_eq!(plan.build_server_id, 7);
    assert_eq!(plan.deploy_server_id, 7);
}
