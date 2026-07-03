//! SSH connection-multiplexing (ControlMaster) lifecycle.
//!
//! Behavior ported from Coolify's `SshMultiplexingHelper`
//! (coolify/app/Helpers/SshMultiplexingHelper.php):
//!   - `ensureMultiplexedConnection` (lines 26-65): per-server lock, reuse or
//!     (re)establish, silent fallback on failure.
//!   - `connectionIsReusable` (lines 240-259): master exists + age <= max +
//!     healthy.
//!   - `establishNewMultiplexedConnection` (lines 67-93): `ssh -fN` master.
//!   - `isConnectionHealthy` (187-202) / `masterConnectionExists` (235-238):
//!     `ssh -O check` plus an `echo` probe.
//!   - `refreshMultiplexedConnection` (223-228) / `removeMuxFile` (95-99):
//!     `ssh -O exit` then re-establish.
//!   - `muxSocket` (278-281): socket path `.../mux_{uuid}`.
//!
//! Rustify pins the connection cap to the Track B brief's `age <= 3600s` and
//! keeps connection metadata in-process (a `tokio::Mutex`-guarded map) rather
//! than in an external cache. Establishment is serialized per server via a
//! per-uuid `tokio::Mutex`.

use crate::command;
use rustify_core::exec::ServerConn;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::sync::Mutex;

/// Master connection is considered stale past this age (brief: `<= 3600s`).
const MUX_MAX_AGE: Duration = Duration::from_secs(3600);
/// Timeout for the `-O check` / echo health probe.
const HEALTH_CHECK_SECS: u32 = 5;
/// The health-probe marker echoed over the multiplexed channel.
const HEALTH_MARKER: &str = "rustify_mux_ok";

/// Manages per-server ControlMaster connections rooted at `mux_dir`.
pub struct MuxManager {
    mux_dir: PathBuf,
    /// Per-server serialization guards, created lazily.
    guards: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// Establish time per server uuid, for age-based expiry.
    established: Mutex<HashMap<String, Instant>>,
}

impl MuxManager {
    pub fn new(mux_dir: PathBuf) -> Self {
        Self {
            mux_dir,
            guards: Mutex::new(HashMap::new()),
            established: Mutex::new(HashMap::new()),
        }
    }

    /// Socket path for a server: `{mux_dir}/mux_{uuid}`.
    pub fn socket_path(&self, uuid: &str) -> PathBuf {
        self.mux_dir.join(format!("mux_{uuid}"))
    }

    /// Ensure a live master connection, returning its socket path when the
    /// caller should attach multiplexing options. Any failure falls back
    /// silently to non-multiplexed (returns `None`).
    pub async fn ensure(&self, conn: &ServerConn) -> Option<PathBuf> {
        let guard = self.guard_for(&conn.uuid).await;
        let _held = guard.lock().await;

        let socket = self.socket_path(&conn.uuid);
        if self.connection_is_reusable(conn, &socket).await {
            return Some(socket);
        }

        let ok = if self.master_exists(conn, &socket).await {
            self.refresh(conn, &socket).await
        } else {
            self.establish(conn, &socket).await
        };

        if ok { Some(socket) } else { None }
    }

    /// Tear down the master connection for a server, if any.
    pub async fn remove(&self, conn: &ServerConn) {
        let socket = self.socket_path(&conn.uuid);
        let _ = run_control(conn, &socket, "exit").await;
        self.established.lock().await.remove(&conn.uuid);
    }

    async fn guard_for(&self, uuid: &str) -> Arc<Mutex<()>> {
        let mut guards = self.guards.lock().await;
        guards
            .entry(uuid.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    async fn connection_is_reusable(&self, conn: &ServerConn, socket: &Path) -> bool {
        if !self.master_exists(conn, socket).await {
            return false;
        }
        // Adopt an untracked-but-live master (e.g. from a prior process).
        {
            let mut times = self.established.lock().await;
            times.entry(conn.uuid.clone()).or_insert_with(Instant::now);
        }
        if self.is_expired(&conn.uuid).await {
            return false;
        }
        self.is_healthy(conn, socket).await
    }

    async fn is_expired(&self, uuid: &str) -> bool {
        match self.established.lock().await.get(uuid) {
            Some(t) => t.elapsed() > MUX_MAX_AGE,
            None => false,
        }
    }

    async fn master_exists(&self, conn: &ServerConn, socket: &Path) -> bool {
        matches!(run_control(conn, socket, "check").await, Some(0))
    }

    async fn is_healthy(&self, conn: &ServerConn, socket: &Path) -> bool {
        let uah = command_user_at_host(conn);
        let cmd = format!(
            "timeout {HEALTH_CHECK_SECS} ssh -o ControlMaster=auto -o ControlPath={socket} \
{uah} 'echo \"{HEALTH_MARKER}\"'",
            socket = socket.display(),
        );
        match run_capture(&cmd).await {
            Some((0, stdout, _)) => stdout.contains(HEALTH_MARKER),
            _ => false,
        }
    }

    async fn establish(&self, conn: &ServerConn, socket: &Path) -> bool {
        if let Some(dir) = socket.parent() {
            if tokio::fs::create_dir_all(dir).await.is_err() {
                return false;
            }
        }
        let cmd = format!(
            "ssh -fN {mux}{common} {uah}",
            mux = format_args!(
                "-o ControlMaster=auto -o ControlPath={} -o ControlPersist={} ",
                socket.display(),
                command::MUX_PERSIST_SECS
            ),
            common = common_establish_options(conn),
            uah = command_user_at_host(conn),
        );
        match run_capture(&cmd).await {
            Some((0, _, _)) => {
                self.established
                    .lock()
                    .await
                    .insert(conn.uuid.clone(), Instant::now());
                true
            }
            _ => false,
        }
    }

    async fn refresh(&self, conn: &ServerConn, socket: &Path) -> bool {
        let _ = run_control(conn, socket, "exit").await;
        self.established.lock().await.remove(&conn.uuid);
        self.establish(conn, socket).await
    }
}

/// Common ssh options for master establishment: the pinned brief string,
/// derived by reusing the ssh command builder with an empty script and no
/// timeout wrapper, then extracting the option segment. Kept simple and
/// explicit here to avoid coupling to the heredoc assembly.
fn common_establish_options(conn: &ServerConn) -> String {
    format!(
        "-i {key} -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
-o PasswordAuthentication=no -o ConnectTimeout={timeout} -o ServerAliveInterval=20 \
-o RequestTTY=no -o LogLevel=ERROR{proxy} -p {port}",
        key = conn.key_path.display(),
        timeout = conn.connection_timeout_secs,
        proxy = command::proxy_command_opt(conn),
        port = conn.port,
    )
}

fn command_user_at_host(conn: &ServerConn) -> String {
    let q = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    format!("{}@{}", q(&conn.user), q(&conn.host))
}

/// Run `ssh -O {op} -o ControlPath={socket} user@host`, returning the exit code.
async fn run_control(conn: &ServerConn, socket: &Path, op: &str) -> Option<i32> {
    let cmd = format!(
        "ssh -O {op} -o ControlPath={socket} {uah}",
        socket = socket.display(),
        uah = command_user_at_host(conn),
    );
    run_capture(&cmd).await.map(|(code, _, _)| code)
}

/// Run a shell command, returning (exit_code, stdout, stderr) or None on spawn
/// failure.
async fn run_capture(cmd: &str) -> Option<(i32, String, String)> {
    let out = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .stdin(Stdio::null())
        .output()
        .await
        .ok()?;
    Some((
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_path_uses_mux_prefix() {
        let m = MuxManager::new(PathBuf::from("/var/mux"));
        assert_eq!(m.socket_path("abc"), PathBuf::from("/var/mux/mux_abc"));
    }

    #[test]
    fn establish_options_match_pinned_string() {
        let conn = ServerConn {
            uuid: "u".into(),
            host: "h".into(),
            port: 2222,
            user: "deploy".into(),
            key_path: PathBuf::from("/k"),
            connection_timeout_secs: 15,
            proxy_command: None,
        };
        assert_eq!(
            common_establish_options(&conn),
            "-i /k -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
-o PasswordAuthentication=no -o ConnectTimeout=15 -o ServerAliveInterval=20 \
-o RequestTTY=no -o LogLevel=ERROR -p 2222"
        );
    }

    #[test]
    fn establish_options_inject_proxy_command_for_tunnel() {
        let conn = ServerConn {
            uuid: "u".into(),
            host: "h".into(),
            port: 2222,
            user: "deploy".into(),
            key_path: PathBuf::from("/k"),
            connection_timeout_secs: 15,
            proxy_command: Some(command::CLOUDFLARED_SSH_PROXY_COMMAND.to_string()),
        };
        assert_eq!(
            common_establish_options(&conn),
            "-i /k -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
-o PasswordAuthentication=no -o ConnectTimeout=15 -o ServerAliveInterval=20 \
-o RequestTTY=no -o LogLevel=ERROR -o ProxyCommand='cloudflared access ssh --hostname %h' -p 2222"
        );
    }
}
