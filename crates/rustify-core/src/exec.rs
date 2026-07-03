// Contract C1: core execution trait. Implemented by `rustify-ssh`,
// mocked by `rustify-deploy` tests. Transcribed verbatim from the pinned
// contracts (rustfmt-formatted).
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct ServerConn {
    pub uuid: String,
    pub host: String,                 // ip or hostname
    pub port: u16,                    // ssh port
    pub user: String,                 // ssh user
    pub key_path: std::path::PathBuf, // 0600 key file on rustify host
    pub connection_timeout_secs: u32, // default 10
    /// Optional `ssh -o ProxyCommand=<value>` reach-through. Set when the server
    /// is behind a Cloudflare tunnel (`cloudflared access ssh --hostname %h`);
    /// injected into every ssh/scp/mux-master invocation. `None` = direct.
    pub proxy_command: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ExecOpts {
    pub timeout_secs: Option<u32>, // wraps remote cmd in `timeout N`; None = 3600 default
    pub disable_mux: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExecEvent {
    Stdout(String), // one line, no trailing \n
    Stderr(String),
}

#[derive(Debug, Clone)]
pub struct ExecOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    #[error("connection failed: {0}")]
    Connection(String),
    #[error("command failed with exit code {code}: {stderr}")]
    NonZero {
        code: i32,
        stdout: String,
        stderr: String,
    },
    #[error("timed out after {0}s")]
    Timeout(u32),
    #[error("io: {0}")]
    Io(String),
}

#[async_trait::async_trait]
pub trait CommandExecutor: Send + Sync {
    /// Run `script` (multi-line bash) on the server; buffered result. Ok even on non-zero when `allow_failure`—callers use `exec_checked` normally.
    async fn exec(
        &self,
        conn: &ServerConn,
        script: &str,
        opts: ExecOpts,
    ) -> Result<ExecOutput, ExecError>;
    /// Same, but streams line events into `tx` as they arrive, then returns the final output.
    async fn exec_streaming(
        &self,
        conn: &ServerConn,
        script: &str,
        opts: ExecOpts,
        tx: mpsc::Sender<ExecEvent>,
    ) -> Result<ExecOutput, ExecError>;
    /// scp a local file to remote path.
    async fn upload(
        &self,
        conn: &ServerConn,
        local: &std::path::Path,
        remote: &str,
    ) -> Result<(), ExecError>;
}
