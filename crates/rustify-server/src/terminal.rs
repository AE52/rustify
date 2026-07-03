//! Interactive web terminal: an axum WebSocket route backed by an in-process
//! PTY over SSH.
//!
//! This replaces Coolify's separate Node `coolify-realtime` bridge
//! (`docker/coolify-realtime/terminal-server.js`) with a single in-process
//! handler. The wire protocol is kept compatible with Coolify's client
//! (`resources/js/terminal.js`) and server so the same message shapes work:
//!
//! Server → client (text sentinels): `pty-ready`, `pty-exited`,
//! `unprocessable`, `pong`. Everything else the server sends is raw PTY output,
//! transmitted as **binary** frames (Coolify sent UTF-8 text; binary avoids
//! corrupting multi-byte sequences split across reads).
//!
//! Client → server (single-key JSON frames):
//!   - `{command:[<target>]}`  resolve target, (re)spawn the PTY, reply `pty-ready`
//!   - `{message:<data>}`      write bytes to the PTY
//!   - `{resize:{cols,rows}}`  resize (cols<=0 → 80, rows<=0 → 30)
//!   - `{pause:true}` / `{resume:true}` flow control
//!   - `{ping:true}`           reply with the literal text `pong`
//!   - `{checkActive:'force'}` kill the PTY if one is active
//!
//! `message`/`resize`/`pause`/`resume` are ignored until a PTY is active;
//! `command`/`ping`/`checkActive` are always accepted.
//!
//! Unlike Coolify, the client never supplies an ssh string. It sends only a
//! server uuid (+ optional container); the server builds the ssh command from
//! team-scoped DB state (parity with `app/Livewire/Project/Shared/Terminal.php`,
//! whose `sendTerminalCommand` assembles the command server-side).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::extract::ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde::Deserialize;
use tokio::sync::mpsc;

use rustify_core::exec::{CommandExecutor, ExecOpts, ServerConn};
use rustify_db::repos::{KeyRepo, ServerRepo};
use rustify_ssh::SshExecutor;
use rustify_ssh::command::MUX_PERSIST_SECS;

use crate::app::AppState;
use crate::auth::{authenticate, resolve_bearer, resolve_team_role, token_role};
use crate::error::ApiError;
use crate::ws::WsQuery;

/// PTY geometry Coolify opens with (`terminal-server.js`: cols 80, rows 30).
const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 30;
/// `TERM` for the spawned shell (parity with node-pty `name: 'xterm-color'`).
const TERM_NAME: &str = "xterm-color";
/// `ServerAliveInterval` for the interactive ssh (matches rustify-ssh's value).
const ALIVE_INTERVAL: u32 = 20;
/// Hard session cap: 8h, matching Coolify `MAX_TERMINAL_SESSION_TIMEOUT_SECONDS`.
const SESSION_LIMIT: Duration = Duration::from_secs(8 * 60 * 60);
/// Application-level keepalive cadence (`HEARTBEAT_INTERVAL_MS = 30000`).
const HEARTBEAT: Duration = Duration::from_secs(30);
/// Kill retry policy (Coolify `killPtyProcess`: 5 attempts, 500ms apart).
const KILL_ATTEMPTS: u32 = 5;
const KILL_INTERVAL: Duration = Duration::from_millis(500);

/// The remote shell bootstrap, verbatim from Coolify `Terminal.php`
/// (`$shellCommand`): fix `PATH`, source `~/.profile`, then exec the login
/// shell (falling back to `sh`).
const SHELL_COMMAND: &str = "PATH=$PATH:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin && if [ -f ~/.profile ]; then . ~/.profile; fi && if [ -n \"$SHELL\" ] && [ -x \"$SHELL\" ]; then exec $SHELL; else sh; fi";

/// The moby#9098 workaround written into the PTY to bring down `docker exec -it`
/// (killing the ssh child alone does not stop the remote exec). Verbatim from
/// Coolify `killPtyProcess`.
const MOBY_KILL: &[u8] = b"set +o history\nkill -TERM -$$ && exit\nset -o history\n";

// --------------------------------------------------------------------------
// Pure protocol helpers (unit-tested)
// --------------------------------------------------------------------------

/// A resolved terminal target: a server, optionally a container on it.
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalTarget {
    pub server_uuid: String,
    pub container: Option<String>,
}

/// Parse the `command[0]` target descriptor sent by the client. Accepted forms:
///   - `<uuid>` or `host:<uuid>`          → a host shell
///   - `container:<uuid>:<name>`          → a shell inside a container
///
/// The container name is validated here ([`is_valid_container_name`]); an
/// invalid name yields `None` so the caller rejects the command.
pub fn parse_target(raw: &str) -> Option<TerminalTarget> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some(rest) = raw.strip_prefix("container:") {
        let (uuid, name) = rest.split_once(':')?;
        if uuid.is_empty() || !is_valid_container_name(name) {
            return None;
        }
        return Some(TerminalTarget {
            server_uuid: uuid.to_string(),
            container: Some(name.to_string()),
        });
    }
    let uuid = raw.strip_prefix("host:").unwrap_or(raw);
    if uuid.is_empty() {
        return None;
    }
    Some(TerminalTarget {
        server_uuid: uuid.to_string(),
        container: None,
    })
}

/// Docker container-name validation, ported from Coolify
/// `ValidationPatterns::CONTAINER_NAME_PATTERN` = `/^[a-zA-Z0-9][a-zA-Z0-9._-]*$/`.
pub fn is_valid_container_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

/// The remote command run over ssh. For a container this is
/// `docker exec -it '<name>' sh -c '<shell>'` (sudo-prefixed on a non-root
/// server user); for a host it is the shell bootstrap alone. Parity with
/// `Terminal.php::sendTerminalCommand`.
pub fn remote_command(container: Option<&str>, non_root: bool) -> String {
    match container {
        Some(name) => {
            let sudo = if non_root { "sudo " } else { "" };
            format!("{sudo}docker exec -it '{name}' sh -c '{SHELL_COMMAND}'")
        }
        None => SHELL_COMMAND.to_string(),
    }
}

/// Build the full `ssh` argv for the PTY. Uses the exact option set the brief
/// pins, plus ControlMaster multiplexing over `mux_socket`. The command is
/// spawned directly (no local shell), so arguments are passed unquoted; the
/// remote command is a single trailing argument handed to the remote shell.
pub fn build_ssh_argv(
    conn: &ServerConn,
    container: Option<&str>,
    non_root: bool,
    mux_socket: Option<&Path>,
) -> Vec<String> {
    let mut argv: Vec<String> = vec!["ssh".into()];
    if let Some(socket) = mux_socket {
        argv.push("-o".into());
        argv.push("ControlMaster=auto".into());
        argv.push("-o".into());
        argv.push(format!("ControlPath={}", socket.display()));
        argv.push("-o".into());
        argv.push(format!("ControlPersist={MUX_PERSIST_SECS}"));
    }
    argv.push("-i".into());
    argv.push(conn.key_path.display().to_string());
    for opt in [
        "StrictHostKeyChecking=no",
        "UserKnownHostsFile=/dev/null",
        "PasswordAuthentication=no",
    ] {
        argv.push("-o".into());
        argv.push(opt.into());
    }
    argv.push("-o".into());
    argv.push(format!("ConnectTimeout={}", conn.connection_timeout_secs));
    argv.push("-o".into());
    argv.push(format!("ServerAliveInterval={ALIVE_INTERVAL}"));
    for opt in ["RequestTTY=yes", "LogLevel=ERROR"] {
        argv.push("-o".into());
        argv.push(opt.into());
    }
    argv.push("-p".into());
    argv.push(conn.port.to_string());
    argv.push(format!("{}@{}", conn.user, conn.host));
    argv.push(remote_command(container, non_root));
    argv
}

/// Client → server control frame after parsing. Single-key JSON per the
/// protocol; unknown/empty objects yield `None`.
#[derive(Debug, PartialEq)]
enum Frame {
    Command(Vec<String>),
    Message(String),
    Resize { cols: u16, rows: u16 },
    Pause,
    Resume,
    Ping,
    CheckActive(String),
}

#[derive(Debug, Deserialize)]
struct ResizeArg {
    #[serde(default)]
    cols: i64,
    #[serde(default)]
    rows: i64,
}

#[derive(Debug, Deserialize)]
struct RawFrame {
    command: Option<Vec<String>>,
    message: Option<String>,
    resize: Option<ResizeArg>,
    pause: Option<bool>,
    resume: Option<bool>,
    ping: Option<bool>,
    #[serde(rename = "checkActive")]
    check_active: Option<String>,
}

/// Clamp a requested terminal size: non-positive dims fall back to 80×30
/// (Coolify `resize` handler).
fn clamp_size(cols: i64, rows: i64) -> (u16, u16) {
    let cols = if cols > 0 {
        cols.min(u16::MAX as i64) as u16
    } else {
        DEFAULT_COLS
    };
    let rows = if rows > 0 {
        rows.min(u16::MAX as i64) as u16
    } else {
        DEFAULT_ROWS
    };
    (cols, rows)
}

/// Parse a client text frame into a [`Frame`], or `None` when the JSON is
/// invalid or carries no recognised key.
fn parse_frame(text: &str) -> Option<Frame> {
    let raw: RawFrame = serde_json::from_str(text).ok()?;
    if let Some(cmd) = raw.command {
        return Some(Frame::Command(cmd));
    }
    if let Some(m) = raw.message {
        return Some(Frame::Message(m));
    }
    if let Some(r) = raw.resize {
        let (cols, rows) = clamp_size(r.cols, r.rows);
        return Some(Frame::Resize { cols, rows });
    }
    if raw.pause == Some(true) {
        return Some(Frame::Pause);
    }
    if raw.resume == Some(true) {
        return Some(Frame::Resume);
    }
    if raw.ping == Some(true) {
        return Some(Frame::Ping);
    }
    if let Some(v) = raw.check_active {
        return Some(Frame::CheckActive(v));
    }
    None
}

/// Whether a frame is processed given the current PTY-active state. `message`,
/// `resize`, `pause`, `resume` are no-ops until a PTY is active; `command`,
/// `ping`, `checkActive` are always allowed (Coolify `handleMessage` gate).
fn frame_allowed(frame: &Frame, active: bool) -> bool {
    match frame {
        Frame::Command(_) | Frame::Ping | Frame::CheckActive(_) => true,
        _ => active,
    }
}

// --------------------------------------------------------------------------
// PTY session (runtime)
// --------------------------------------------------------------------------

/// A live PTY: the ssh child, its master handle (for resize), a writer channel
/// feeding the PTY, and a reader channel draining PTY output.
struct PtySession {
    master: Box<dyn portable_pty::MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer_tx: mpsc::Sender<Vec<u8>>,
    output_rx: mpsc::Receiver<Vec<u8>>,
    /// ssh child pid (== its process-group id thanks to portable-pty's setsid).
    pgid: Option<i32>,
    /// Flow-control: when paused we stop draining output, filling the bounded
    /// channel and back-pressuring the PTY.
    paused: bool,
}

/// Spawn the ssh PTY for `argv`, wiring reader/writer bridge threads.
fn spawn_pty(argv: &[String]) -> Result<PtySession, String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: DEFAULT_ROWS,
            cols: DEFAULT_COLS,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())?;

    let mut cmd = CommandBuilder::new(&argv[0]);
    for arg in &argv[1..] {
        cmd.arg(arg);
    }
    cmd.env("TERM", TERM_NAME);

    let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    let pgid = child.process_id().map(|p| p as i32);
    // Drop the slave so the master sees EOF once the child exits.
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

    // Reader thread: PTY output → bounded channel (backpressure on pause).
    let (output_tx, output_rx) = mpsc::channel::<Vec<u8>>(64);
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if output_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Writer thread: channel → PTY input.
    let (writer_tx, mut writer_rx) = mpsc::channel::<Vec<u8>>(64);
    std::thread::spawn(move || {
        let mut writer = writer;
        while let Some(data) = writer_rx.blocking_recv() {
            if writer.write_all(&data).is_err() {
                break;
            }
            let _ = writer.flush();
        }
    });

    Ok(PtySession {
        master: pair.master,
        child,
        writer_tx,
        output_rx,
        pgid,
        paused: false,
    })
}

impl PtySession {
    /// Attempt to terminate the PTY: write the moby#9098 workaround into the
    /// PTY and SIGTERM the ssh child's process group, retrying up to
    /// [`KILL_ATTEMPTS`] times [`KILL_INTERVAL`] apart. Returns `true` once the
    /// child has exited.
    async fn kill(&mut self) -> bool {
        for _ in 0..KILL_ATTEMPTS {
            let _ = self.writer_tx.send(MOBY_KILL.to_vec()).await;
            if let Some(pgid) = self.pgid {
                signal_group(pgid);
            }
            tokio::time::sleep(KILL_INTERVAL).await;
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return true;
            }
        }
        false
    }

    fn resize(&self, cols: u16, rows: u16) {
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }
}

/// SIGTERM a process group. `nix` wraps `killpg` safely, keeping this crate's
/// `#![forbid(unsafe_code)]` intact.
fn signal_group(pgid: i32) {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;
    let _ = killpg(Pid::from_raw(pgid), Signal::SIGTERM);
}

// --------------------------------------------------------------------------
// Route handler
// --------------------------------------------------------------------------

/// `GET /terminal/ws`: authenticate (session cookie, bearer header, or
/// `?token=`), then gate the upgrade on `role.is_admin()` — members are denied
/// with `401` (Coolify `canAccessTerminal`). On success, hand the socket to the
/// per-connection loop, scoped to the caller's team.
pub async fn terminal_ws_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<WsQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let (team_id, role) = if let Some(token) = query.token.as_deref() {
        let t = resolve_bearer(&state, token).await?;
        (t.team_id, token_role(&t.abilities))
    } else {
        let principal = authenticate(&state, &headers).await?;
        resolve_team_role(&state, &principal).await?
    };
    // canAccessTerminal: admin or owner only.
    if !role.is_admin() {
        return Err(ApiError::Unauthorized);
    }
    Ok(ws.on_upgrade(move |socket| handle_terminal(socket, state, team_id)))
}

async fn handle_terminal(socket: WebSocket, state: AppState, team_id: i64) {
    let (mut sink, mut stream) = socket.split();
    let mut session: Option<PtySession> = None;

    // Start the heartbeat one period out so we do not ping immediately on
    // connect (parity with Coolify's `setInterval`, whose first tick is after
    // the interval, not at t=0).
    let mut heartbeat =
        tokio::time::interval_at(tokio::time::Instant::now() + HEARTBEAT, HEARTBEAT);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let deadline = tokio::time::sleep(SESSION_LIMIT);
    tokio::pin!(deadline);

    loop {
        // Only drain PTY output when a PTY is active and not paused.
        let pty_output = async {
            match session.as_mut() {
                Some(s) if !s.paused => s.output_rx.recv().await,
                _ => std::future::pending().await,
            }
        };

        tokio::select! {
            biased;

            incoming = stream.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if !handle_frame(&state, team_id, &mut session, &mut sink, text.as_str()).await {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {} // binary/ping/pong input: ignored
                    Some(Err(_)) => break,
                }
            }

            out = pty_output => {
                match out {
                    Some(bytes) => {
                        if sink.send(Message::Binary(bytes.into())).await.is_err() {
                            break;
                        }
                    }
                    None => {
                        // PTY reached EOF: the shell exited.
                        session = None;
                        let _ = sink.send(text_frame("pty-exited")).await;
                    }
                }
            }

            _ = heartbeat.tick() => {
                if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }

            _ = &mut deadline => {
                if let Some(mut s) = session.take() {
                    let _ = s.kill().await;
                }
                let _ = sink.send(text_frame("pty-exited")).await;
                break;
            }
        }
    }

    // Best-effort cleanup on disconnect.
    if let Some(mut s) = session.take() {
        let _ = s.kill().await;
    }
}

/// A text sentinel frame.
fn text_frame(s: &str) -> Message {
    Message::Text(Utf8Bytes::from(s.to_owned()))
}

/// Handle one parsed client frame. Returns `false` when the connection should
/// close.
async fn handle_frame(
    state: &AppState,
    team_id: i64,
    session: &mut Option<PtySession>,
    sink: &mut (impl SinkExt<Message> + Unpin),
    text: &str,
) -> bool {
    let Some(frame) = parse_frame(text) else {
        return true; // unparseable / unknown key: ignore (Coolify parity)
    };
    let active = session.is_some();
    if !frame_allowed(&frame, active) {
        return true; // no-op until a PTY is active
    }

    match frame {
        Frame::Ping => {
            let _ = sink.send(text_frame("pong")).await;
        }
        Frame::CheckActive(value) => {
            if value == "force" {
                if let Some(mut s) = session.take() {
                    let _ = s.kill().await;
                    let _ = sink.send(text_frame("pty-exited")).await;
                }
            } else {
                let _ = sink
                    .send(text_frame(if active { "true" } else { "false" }))
                    .await;
            }
        }
        Frame::Command(args) => {
            handle_command(state, team_id, session, sink, args).await;
        }
        Frame::Message(data) => {
            if let Some(s) = session.as_ref() {
                let _ = s.writer_tx.send(data.into_bytes()).await;
            }
        }
        Frame::Resize { cols, rows } => {
            if let Some(s) = session.as_ref() {
                s.resize(cols, rows);
            }
        }
        Frame::Pause => {
            if let Some(s) = session.as_mut() {
                s.paused = true;
            }
        }
        Frame::Resume => {
            if let Some(s) = session.as_mut() {
                s.paused = false;
            }
        }
    }
    true
}

/// Handle a `{command:[target]}` frame: kill any active PTY, resolve the target
/// to an ssh command, spawn a new PTY, and reply `pty-ready` (or `unprocessable`
/// on failure).
async fn handle_command(
    state: &AppState,
    team_id: i64,
    session: &mut Option<PtySession>,
    sink: &mut (impl SinkExt<Message> + Unpin),
    args: Vec<String>,
) {
    // A new command while active kills the old PTY first.
    if let Some(mut s) = session.take() {
        if !s.kill().await {
            // Could not terminate the previous PTY: keep it and refuse.
            *session = Some(s);
            let _ = sink.send(text_frame("unprocessable")).await;
            return;
        }
    }

    let target = match args.first().and_then(|t| parse_target(t)) {
        Some(t) => t,
        None => {
            let _ = sink.send(text_frame("unprocessable")).await;
            return;
        }
    };

    match build_and_spawn(state, team_id, &target).await {
        Ok(pty) => {
            *session = Some(pty);
            let _ = sink.send(text_frame("pty-ready")).await;
        }
        Err(reason) => {
            tracing::warn!(target = %target.server_uuid, %reason, "terminal spawn rejected");
            let _ = sink.send(text_frame("unprocessable")).await;
        }
    }
}

/// Resolve a target to a spawned PTY: verify the server is in the caller's team
/// and terminal-enabled, materialise its key, validate the container (name +
/// running), then spawn ssh.
async fn build_and_spawn(
    state: &AppState,
    team_id: i64,
    target: &TerminalTarget,
) -> Result<PtySession, String> {
    let server_repo = ServerRepo::new(state.pool.clone());
    let server = server_repo
        .get_by_uuid(&target.server_uuid)
        .await
        .map_err(|e| e.to_string())?
        .filter(|s| s.team_id == team_id)
        .ok_or_else(|| "server not found in team".to_string())?;

    let settings = server_repo
        .settings(server.id)
        .await
        .map_err(|e| e.to_string())?;
    let connection_timeout_secs = settings
        .as_ref()
        .map(|s| s.connection_timeout as u32)
        .unwrap_or(10);
    let terminal_enabled = settings
        .as_ref()
        .map(|s| s.is_terminal_enabled)
        .unwrap_or(false);
    if !terminal_enabled {
        return Err("terminal disabled on server".to_string());
    }

    // Materialise the server's private key `0600`.
    let keys = KeyRepo::new(state.pool.clone());
    let key = keys
        .get_by_id(server.private_key_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "server key missing".to_string())?;
    let pem = keys
        .decrypt_private_key(server.private_key_id)
        .await
        .map_err(|e| e.to_string())?;
    let key_path = rustify_ssh::keys::materialize(&key.uuid, &pem, &state.config.ssh_key_dir)
        .map_err(|e| e.to_string())?;

    let proxy_command = settings
        .as_ref()
        .filter(|s| s.is_cloudflare_tunnel)
        .map(|_| rustify_ssh::command::CLOUDFLARED_SSH_PROXY_COMMAND.to_string());

    let conn = ServerConn {
        uuid: server.uuid.clone(),
        host: server.ip.clone(),
        port: server.port as u16,
        user: server.ssh_user.clone(),
        key_path,
        connection_timeout_secs,
        proxy_command,
    };
    let non_root = server.ssh_user != "root";

    // For a container target: validate the name and confirm it is running.
    if let Some(container) = &target.container {
        if !is_valid_container_name(container) {
            return Err("invalid container name".to_string());
        }
        if !container_is_running(state, &conn, container).await {
            return Err("container not running".to_string());
        }
    }

    let mux_socket: PathBuf = state
        .config
        .ssh_mux_dir
        .join(format!("mux_{}", server.uuid));
    let argv = build_ssh_argv(
        &conn,
        target.container.as_deref(),
        non_root,
        Some(&mux_socket),
    );
    spawn_pty(&argv)
}

/// Check a container's status over SSH (parity with Coolify `getContainerStatus`
/// gating): true only when `docker inspect` reports `running`.
async fn container_is_running(state: &AppState, conn: &ServerConn, container: &str) -> bool {
    let executor = SshExecutor::new(state.config.ssh_mux_dir.clone());
    let script = format!("docker inspect --format '{{{{.State.Status}}}}' '{container}'");
    match executor.exec(conn, &script, ExecOpts::default()).await {
        Ok(out) => out.stdout.trim() == "running",
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> ServerConn {
        ServerConn {
            uuid: "srv1".into(),
            host: "10.0.0.1".into(),
            port: 22,
            user: "root".into(),
            key_path: PathBuf::from("/keys/ssh_key@k1"),
            connection_timeout_secs: 10,
            proxy_command: None,
        }
    }

    // ---- target parsing / container validation --------------------------

    #[test]
    fn parses_plain_uuid_as_host() {
        assert_eq!(
            parse_target("abc-123"),
            Some(TerminalTarget {
                server_uuid: "abc-123".into(),
                container: None
            })
        );
    }

    #[test]
    fn parses_host_prefixed_target() {
        assert_eq!(
            parse_target("host:srv-9"),
            Some(TerminalTarget {
                server_uuid: "srv-9".into(),
                container: None
            })
        );
    }

    #[test]
    fn parses_container_target() {
        assert_eq!(
            parse_target("container:srv-9:my_app-1"),
            Some(TerminalTarget {
                server_uuid: "srv-9".into(),
                container: Some("my_app-1".into()),
            })
        );
    }

    #[test]
    fn rejects_empty_and_bad_container() {
        assert_eq!(parse_target(""), None);
        assert_eq!(parse_target("   "), None);
        assert_eq!(parse_target("container:srv:"), None);
        assert_eq!(parse_target("container::name"), None);
        // shell metacharacters in the container name are rejected
        assert_eq!(parse_target("container:srv:bad;rm -rf"), None);
        assert_eq!(parse_target("container:srv:$(whoami)"), None);
    }

    #[test]
    fn container_name_pattern_matches_coolify() {
        assert!(is_valid_container_name("app"));
        assert!(is_valid_container_name("my-app_1.2"));
        assert!(is_valid_container_name("A0"));
        assert!(!is_valid_container_name(""));
        assert!(!is_valid_container_name("-leading"));
        assert!(!is_valid_container_name(".leading"));
        assert!(!is_valid_container_name("has space"));
        assert!(!is_valid_container_name("has/slash"));
        assert!(!is_valid_container_name("quote'"));
    }

    // ---- ssh command generation ----------------------------------------

    #[test]
    fn host_argv_golden() {
        let argv = build_ssh_argv(&conn(), None, false, None);
        assert_eq!(
            argv,
            vec![
                "ssh",
                "-i",
                "/keys/ssh_key@k1",
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "UserKnownHostsFile=/dev/null",
                "-o",
                "PasswordAuthentication=no",
                "-o",
                "ConnectTimeout=10",
                "-o",
                "ServerAliveInterval=20",
                "-o",
                "RequestTTY=yes",
                "-o",
                "LogLevel=ERROR",
                "-p",
                "22",
                "root@10.0.0.1",
                SHELL_COMMAND,
            ]
        );
    }

    #[test]
    fn container_argv_uses_docker_exec() {
        let argv = build_ssh_argv(&conn(), Some("web-1"), false, None);
        let remote = argv.last().unwrap();
        assert_eq!(
            remote,
            &format!("docker exec -it 'web-1' sh -c '{SHELL_COMMAND}'")
        );
        assert!(!remote.starts_with("sudo "));
    }

    #[test]
    fn container_argv_sudo_prefixed_for_non_root() {
        let mut c = conn();
        c.user = "deploy".into();
        let argv = build_ssh_argv(&c, Some("web-1"), true, None);
        let remote = argv.last().unwrap();
        assert_eq!(
            remote,
            &format!("sudo docker exec -it 'web-1' sh -c '{SHELL_COMMAND}'")
        );
        assert_eq!(argv[argv.len() - 2], "deploy@10.0.0.1");
    }

    #[test]
    fn argv_includes_mux_options_when_socket_present() {
        let socket = PathBuf::from("/mux/mux_srv1");
        let argv = build_ssh_argv(&conn(), None, false, Some(&socket));
        assert_eq!(argv[0], "ssh");
        assert_eq!(argv[1], "-o");
        assert_eq!(argv[2], "ControlMaster=auto");
        assert_eq!(argv[3], "-o");
        assert_eq!(argv[4], "ControlPath=/mux/mux_srv1");
        assert_eq!(argv[5], "-o");
        assert_eq!(argv[6], format!("ControlPersist={MUX_PERSIST_SECS}"));
        // RequestTTY is forced on for the interactive PTY.
        assert!(argv.iter().any(|a| a == "RequestTTY=yes"));
        assert!(!argv.iter().any(|a| a == "RequestTTY=no"));
    }

    #[test]
    fn remote_command_host_is_shell_bootstrap() {
        assert_eq!(remote_command(None, false), SHELL_COMMAND);
        assert_eq!(remote_command(None, true), SHELL_COMMAND);
    }

    // ---- frame parsing / gating ----------------------------------------

    #[test]
    fn parses_command_frame() {
        assert_eq!(
            parse_frame(r#"{"command":["host:srv1"]}"#),
            Some(Frame::Command(vec!["host:srv1".into()]))
        );
    }

    #[test]
    fn parses_message_and_ping() {
        assert_eq!(
            parse_frame(r#"{"message":"ls -la\r"}"#),
            Some(Frame::Message("ls -la\r".into()))
        );
        assert_eq!(parse_frame(r#"{"ping":true}"#), Some(Frame::Ping));
    }

    #[test]
    fn parses_check_active_force() {
        assert_eq!(
            parse_frame(r#"{"checkActive":"force"}"#),
            Some(Frame::CheckActive("force".into()))
        );
    }

    #[test]
    fn resize_clamps_non_positive() {
        assert_eq!(
            parse_frame(r#"{"resize":{"cols":0,"rows":-4}}"#),
            Some(Frame::Resize {
                cols: DEFAULT_COLS,
                rows: DEFAULT_ROWS
            })
        );
        assert_eq!(
            parse_frame(r#"{"resize":{"cols":120,"rows":40}}"#),
            Some(Frame::Resize {
                cols: 120,
                rows: 40
            })
        );
    }

    #[test]
    fn invalid_or_unknown_frames_are_none() {
        assert_eq!(parse_frame("not json"), None);
        assert_eq!(parse_frame("{}"), None);
        assert_eq!(parse_frame(r#"{"bogus":1}"#), None);
    }

    #[test]
    fn gating_ignores_io_frames_before_active() {
        // message/resize/pause/resume are ignored until a PTY is active.
        assert!(!frame_allowed(&Frame::Message("x".into()), false));
        assert!(!frame_allowed(&Frame::Resize { cols: 80, rows: 30 }, false));
        assert!(!frame_allowed(&Frame::Pause, false));
        assert!(!frame_allowed(&Frame::Resume, false));
        // command/ping/checkActive are always allowed.
        assert!(frame_allowed(&Frame::Command(vec![]), false));
        assert!(frame_allowed(&Frame::Ping, false));
        assert!(frame_allowed(&Frame::CheckActive("force".into()), false));
        // once active, the io frames are processed.
        assert!(frame_allowed(&Frame::Message("x".into()), true));
        assert!(frame_allowed(&Frame::Resize { cols: 80, rows: 30 }, true));
    }
}
