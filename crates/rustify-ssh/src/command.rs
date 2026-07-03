//! SSH/SCP command assembly.
//!
//! Behavior ported from Coolify's `SshMultiplexingHelper`
//! (coolify/app/Helpers/SshMultiplexingHelper.php):
//!   - `generateSshCommand` (lines 137-176): heredoc transport
//!     `'bash -se' << \{delimiter}` with the script sandwiched between two
//!     delimiter lines. Coolify derives the delimiter from
//!     `base64_encode(Hash::make($command))` (line 170); Rustify instead uses
//!     a random-nonce token `RUSTIFY_EOF_{nonce}` (nonce injected for tests).
//!   - `getCommonSshOptions` (lines 328-343): the exact option string.
//!   - `multiplexingOptions` (lines 271-276): ControlMaster/ControlPath/
//!     ControlPersist.
//!   - `generateScpCommand` (lines 101-135): `timeout N scp ...`.
//!   - `escapedUserAtHost` (lines 283-286): shell-quoted `user@host`.
//!
//! Rustify pins the option string to the values in the Track B brief:
//! `-i {key} -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null
//!  -o PasswordAuthentication=no -o ConnectTimeout={n} -o ServerAliveInterval=20
//!  -o RequestTTY=no -o LogLevel=ERROR -p {port}`.

use rustify_core::exec::ServerConn;
use std::path::Path;

/// ControlPersist window for the master connection, in seconds.
pub const MUX_PERSIST_SECS: u32 = 3600;

/// The `ProxyCommand` value used to reach a server behind a Cloudflare tunnel.
/// `%h` expands to the target hostname (parity with Coolify's cloudflared SSH
/// access, app/Helpers/SshMultiplexingHelper.php cloudflared branch).
pub const CLOUDFLARED_SSH_PROXY_COMMAND: &str = "cloudflared access ssh --hostname %h";

/// Render ` -o ProxyCommand='<value>'` (leading space) when `conn` carries a
/// proxy command, else the empty string. Injected into every ssh/scp/mux-master
/// command so a tunnelled server is reachable transparently.
pub(crate) fn proxy_command_opt(conn: &ServerConn) -> String {
    match &conn.proxy_command {
        Some(cmd) if !cmd.is_empty() => format!(" -o ProxyCommand={}", sh_quote(cmd)),
        _ => String::new(),
    }
}

/// Generates the heredoc delimiter nonce. Injected so tests can pin a
/// deterministic value and factor it out of golden strings.
pub trait NonceGen: Send + Sync {
    fn nonce(&self) -> String;
}

/// Production nonce: 128 random bits rendered as lowercase hex.
pub struct RandomNonce;

impl NonceGen for RandomNonce {
    fn nonce(&self) -> String {
        format!(
            "{:016x}{:016x}",
            rand::random::<u64>(),
            rand::random::<u64>()
        )
    }
}

/// Single-quote a string for POSIX `sh`, escaping embedded single quotes.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `user@host` with each part shell-quoted (parity: `escapedUserAtHost`).
fn user_at_host(conn: &ServerConn) -> String {
    format!("{}@{}", sh_quote(&conn.user), sh_quote(&conn.host))
}

/// The pinned common option string. `is_scp` swaps `-p` for scp's `-P`.
fn common_options(conn: &ServerConn, is_scp: bool) -> String {
    let port_flag = if is_scp { "-P" } else { "-p" };
    format!(
        "-i {key} -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
-o PasswordAuthentication=no -o ConnectTimeout={timeout} -o ServerAliveInterval=20 \
-o RequestTTY=no -o LogLevel=ERROR{proxy} {port_flag} {port}",
        key = conn.key_path.display(),
        timeout = conn.connection_timeout_secs,
        proxy = proxy_command_opt(conn),
        port_flag = port_flag,
        port = conn.port,
    )
}

/// Multiplexing options (trailing space) for a live master socket.
fn mux_options(socket: &Path) -> String {
    format!(
        "-o ControlMaster=auto -o ControlPath={socket} -o ControlPersist={persist} ",
        socket = socket.display(),
        persist = MUX_PERSIST_SECS,
    )
}

/// The heredoc delimiter for a given nonce.
pub fn delimiter(nonce: &str) -> String {
    format!("RUSTIFY_EOF_{nonce}")
}

/// Assemble the full `ssh` shell command that runs `script` remotely via a
/// `bash -se` heredoc. `timeout_secs > 0` wraps the invocation in `timeout N`.
/// `mux_socket` adds multiplexing options when a master connection is live.
pub fn build_ssh_command(
    conn: &ServerConn,
    script: &str,
    timeout_secs: u32,
    mux_socket: Option<&Path>,
    nonce: &str,
) -> String {
    let mut cmd = if timeout_secs > 0 {
        format!("timeout {timeout_secs} ssh ")
    } else {
        String::from("ssh ")
    };
    if let Some(socket) = mux_socket {
        cmd.push_str(&mux_options(socket));
    }
    cmd.push_str(&common_options(conn, false));
    cmd.push(' ');
    cmd.push_str(&user_at_host(conn));

    let delim = delimiter(nonce);
    // Strip any embedded delimiter so the heredoc cannot be terminated early.
    let script = script.replace(&delim, "");
    cmd.push_str(&format!(" 'bash -se' << \\{delim}\n{script}\n{delim}"));
    cmd
}

/// Assemble the full `scp` shell command uploading `local` to `remote`.
pub fn build_scp_command(
    conn: &ServerConn,
    local: &Path,
    remote: &str,
    timeout_secs: u32,
    mux_socket: Option<&Path>,
) -> String {
    let mut cmd = format!("timeout {timeout_secs} scp ");
    if let Some(socket) = mux_socket {
        cmd.push_str(&mux_options(socket));
    }
    cmd.push_str(&common_options(conn, true));
    cmd.push(' ');
    cmd.push_str(&sh_quote(&local.to_string_lossy()));
    cmd.push(' ');
    cmd.push_str(&user_at_host(conn));
    cmd.push(':');
    cmd.push_str(&sh_quote(remote));
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn conn() -> ServerConn {
        ServerConn {
            uuid: "srv123".into(),
            host: "1.2.3.4".into(),
            port: 22,
            user: "root".into(),
            key_path: PathBuf::from("/keys/ssh_key@k1"),
            connection_timeout_secs: 10,
            proxy_command: None,
        }
    }

    fn tunnel_conn() -> ServerConn {
        ServerConn {
            proxy_command: Some(CLOUDFLARED_SSH_PROXY_COMMAND.to_string()),
            ..conn()
        }
    }

    const NONCE: &str = "TESTNONCE";

    #[test]
    fn ssh_command_without_mux_golden() {
        let got = build_ssh_command(&conn(), "echo hello", 3600, None, NONCE);
        let want = "timeout 3600 ssh -i /keys/ssh_key@k1 -o StrictHostKeyChecking=no \
-o UserKnownHostsFile=/dev/null -o PasswordAuthentication=no -o ConnectTimeout=10 \
-o ServerAliveInterval=20 -o RequestTTY=no -o LogLevel=ERROR -p 22 'root'@'1.2.3.4' \
'bash -se' << \\RUSTIFY_EOF_TESTNONCE\necho hello\nRUSTIFY_EOF_TESTNONCE";
        assert_eq!(got, want);
    }

    #[test]
    fn ssh_command_with_mux_golden() {
        let socket = PathBuf::from("/mux/mux_srv123");
        let got = build_ssh_command(&conn(), "echo hello", 3600, Some(&socket), NONCE);
        let want = "timeout 3600 ssh -o ControlMaster=auto -o ControlPath=/mux/mux_srv123 \
-o ControlPersist=3600 -i /keys/ssh_key@k1 -o StrictHostKeyChecking=no \
-o UserKnownHostsFile=/dev/null -o PasswordAuthentication=no -o ConnectTimeout=10 \
-o ServerAliveInterval=20 -o RequestTTY=no -o LogLevel=ERROR -p 22 'root'@'1.2.3.4' \
'bash -se' << \\RUSTIFY_EOF_TESTNONCE\necho hello\nRUSTIFY_EOF_TESTNONCE";
        assert_eq!(got, want);
    }

    #[test]
    fn ssh_command_no_timeout_wrapper_when_zero() {
        let got = build_ssh_command(&conn(), "true", 0, None, NONCE);
        assert!(got.starts_with("ssh -i /keys/ssh_key@k1"));
    }

    #[test]
    fn ssh_command_strips_embedded_delimiter() {
        let script = "echo a\nRUSTIFY_EOF_TESTNONCE\necho b";
        let got = build_ssh_command(&conn(), script, 3600, None, NONCE);
        // Exactly two delimiter occurrences remain (the heredoc open + close);
        // the injected one inside the script body is removed.
        assert_eq!(got.matches("RUSTIFY_EOF_TESTNONCE").count(), 2);
    }

    #[test]
    fn scp_command_golden() {
        let local = PathBuf::from("/tmp/artifact.tar");
        let got = build_scp_command(&conn(), &local, "/data/app.tar", 3600, None);
        let want = "timeout 3600 scp -i /keys/ssh_key@k1 -o StrictHostKeyChecking=no \
-o UserKnownHostsFile=/dev/null -o PasswordAuthentication=no -o ConnectTimeout=10 \
-o ServerAliveInterval=20 -o RequestTTY=no -o LogLevel=ERROR -P 22 '/tmp/artifact.tar' \
'root'@'1.2.3.4':'/data/app.tar'";
        assert_eq!(got, want);
    }

    #[test]
    fn scp_command_with_mux_golden() {
        let local = PathBuf::from("/tmp/a");
        let socket = PathBuf::from("/mux/mux_srv123");
        let got = build_scp_command(&conn(), &local, "/data/a", 3600, Some(&socket));
        assert!(got.starts_with(
            "timeout 3600 scp -o ControlMaster=auto -o ControlPath=/mux/mux_srv123 \
-o ControlPersist=3600 -i /keys/ssh_key@k1"
        ));
    }

    #[test]
    fn user_at_host_is_shell_quoted() {
        assert_eq!(user_at_host(&conn()), "'root'@'1.2.3.4'");
    }

    #[test]
    fn proxy_command_opt_empty_when_none() {
        assert_eq!(proxy_command_opt(&conn()), "");
    }

    #[test]
    fn proxy_command_injected_into_ssh_command() {
        let got = build_ssh_command(&tunnel_conn(), "echo hi", 3600, None, NONCE);
        assert!(
            got.contains(
                "-o LogLevel=ERROR -o ProxyCommand='cloudflared access ssh --hostname %h' -p 22"
            ),
            "ssh must carry the cloudflared ProxyCommand: {got}"
        );
    }

    #[test]
    fn proxy_command_injected_into_scp_command() {
        let local = PathBuf::from("/tmp/a");
        let got = build_scp_command(&tunnel_conn(), &local, "/data/a", 3600, None);
        assert!(
            got.contains(
                "-o LogLevel=ERROR -o ProxyCommand='cloudflared access ssh --hostname %h' -P 22"
            ),
            "scp must carry the cloudflared ProxyCommand: {got}"
        );
    }

    #[test]
    fn direct_connection_never_has_proxy_command() {
        let got = build_ssh_command(&conn(), "echo hi", 3600, None, NONCE);
        assert!(!got.contains("ProxyCommand"));
    }
}
