//! Git command generation and `ls-remote` parsing.
//!
//! Ports the public-repository slice of Coolify's git handling
//! (app/Jobs/ApplicationDeploymentJob.php:2266-2360 `gitLsRemoteCommand` /
//! `check_git_if_build_needed`, and app/Models/Application.php:1545-1571
//! `generateGitImportCommands`), reduced to Phase 1 scope: public HTTPS repos,
//! single-branch shallow clone, no GitHub App / private submodule auth.

/// `git ls-remote` command resolving the exact branch ref, so we never match a
/// similarly named branch (parity with ApplicationDeploymentJob.php:2287-2298:
/// an exact `refs/heads/<branch>` refspec).
pub fn ls_remote_command(repository: &str, branch: &str) -> String {
    format!("git ls-remote {repository} refs/heads/{branch}")
}

/// Shallow single-branch clone into `dest` (brief step 5). Mirrors the
/// `git clone --depth=1 -b <branch>` shape of `generateGitImportCommands`.
pub fn clone_command(repository: &str, branch: &str, dest: &str) -> String {
    format!("git clone -b {branch} --single-branch --depth 1 {repository} {dest}")
}

/// Read the subject line of the resolved commit (ApplicationDeploymentJob.php:2378).
pub fn commit_message_command(workdir: &str) -> String {
    format!("cd {workdir} && git log -1 --pretty=%B")
}

// ---- private repositories: GitHub App (installation token) ----------------

/// `rawurlencode` the GitHub installation token exactly like Coolify does
/// before embedding it in the clone URL (Application.php:1608
/// `rawurlencode($github_access_token)`): percent-encode everything except the
/// RFC 3986 unreserved set `A-Za-z0-9-_.~`.
pub fn urlencode(token: &str) -> String {
    let mut out = String::with_capacity(token.len());
    for b in token.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The `-c url....insteadOf...` git config that rewrites same-host HTTPS URLs to
/// inject the `x-access-token` credential (Application.php:1611). `enc` must
/// already be the url-encoded token.
fn insteadof_config(scheme: &str, host: &str, enc: &str) -> String {
    format!("-c 'url.{scheme}://x-access-token:{enc}@{host}/.insteadOf={scheme}://{host}/'")
}

/// Clone command for a **private** repo behind a GitHub App installation token
/// (Application.php:1607-1637). Golden shape (parity):
///
/// ```text
/// git -c 'url.<scheme>://x-access-token:<enc>@<host>/.insteadOf=<scheme>://<host>/' \
///     -c http.version=HTTP/1.1 clone [--depth=1] -b <branch> \
///     <scheme>://x-access-token:<enc>@<host>/<repo>.git <baseDir>
/// ```
///
/// The token is url-encoded (`enc`); the same-host `insteadOf` rewrite means git
/// authenticates submodules on the same host without persisting credentials.
pub fn github_app_clone_command(
    scheme: &str,
    host: &str,
    repo: &str,
    branch: &str,
    enc: &str,
    shallow: bool,
    base_dir: &str,
) -> String {
    let cfg = insteadof_config(scheme, host, enc);
    let depth = if shallow { " --depth=1" } else { "" };
    format!(
        "git {cfg} -c http.version=HTTP/1.1 clone{depth} -b {branch} \
         {scheme}://x-access-token:{enc}@{host}/{repo}.git {base_dir}"
    )
}

/// `git ls-remote` for a private GitHub-App repo: reuse the tokenised URL so the
/// exact-ref resolution (parity with the public [`ls_remote_command`]) works
/// against a private remote.
pub fn github_app_ls_remote_command(
    scheme: &str,
    host: &str,
    repo: &str,
    branch: &str,
    enc: &str,
) -> String {
    let cfg = insteadof_config(scheme, host, enc);
    format!(
        "git {cfg} -c http.version=HTTP/1.1 ls-remote \
         {scheme}://x-access-token:{enc}@{host}/{repo}.git refs/heads/{branch}"
    )
}

// ---- private repositories: raw deploy key (SSH) ---------------------------

/// The on-disk 0600 deploy-key path inside the build helper, keyed by the
/// deployment uuid (Application.php:1550 `id_rsa_coolify_{deployment_uuid}`).
pub fn deploy_key_path(deployment_uuid: &str) -> String {
    format!("/root/.ssh/id_rsa_coolify_{deployment_uuid}")
}

/// The `ssh -o ...` command Coolify hands to git via `GIT_SSH_COMMAND` for
/// deploy-key clones (Application.php:1734, 1552).
pub fn deploy_key_ssh_command(port: i32, key_path: &str) -> String {
    format!(
        "ssh -o ConnectTimeout=30 -p {port} -o Port={port} -o LogLevel=ERROR \
         -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -i {key_path} \
         -o IdentitiesOnly=yes"
    )
}

/// Shell commands (run inside the helper) that materialise the base64-encoded
/// deploy key to a 0600 file (Application.php:1741-1745). `b64_key` is the
/// base64 of the raw PEM; it is decoded on the host and never echoed to logs by
/// the caller (the engine redacts it).
pub fn deploy_key_materialise_commands(key_path: &str, b64_key: &str) -> Vec<String> {
    vec![
        "mkdir -p /root/.ssh".to_string(),
        format!("echo '{b64_key}' | base64 -d | tee {key_path} > /dev/null"),
        format!("chmod 600 {key_path}"),
    ]
}

/// Deploy-key clone: `GIT_SSH_COMMAND="ssh ..." git clone [--depth=1] -b
/// <branch> <repo> <baseDir>` (Application.php:1734-1740).
pub fn deploy_key_clone_command(
    repository: &str,
    port: i32,
    key_path: &str,
    branch: &str,
    shallow: bool,
    base_dir: &str,
) -> String {
    let ssh = deploy_key_ssh_command(port, key_path);
    let depth = if shallow { " --depth=1" } else { "" };
    format!("GIT_SSH_COMMAND=\"{ssh}\" git clone{depth} -b {branch} {repository} {base_dir}")
}

/// Deploy-key `ls-remote` with the same `GIT_SSH_COMMAND` (parity with the
/// deploy-key branch of `generateGitLsRemoteCommands`).
pub fn deploy_key_ls_remote_command(
    repository: &str,
    port: i32,
    key_path: &str,
    branch: &str,
) -> String {
    let ssh = deploy_key_ssh_command(port, key_path);
    format!("GIT_SSH_COMMAND=\"{ssh}\" git ls-remote {repository} refs/heads/{branch}")
}

/// Extract the 40-hex commit SHA from `git ls-remote` output. Git can prepend
/// warning lines and even put them on the same line as the result, so we scan
/// for a 40-hex token immediately preceding a tab (parity with the
/// `/\b([0-9a-fA-F]{40})(?=\s*\t)/` match at ApplicationDeploymentJob.php:2344).
pub fn parse_commit_sha(ls_remote_output: &str) -> Option<String> {
    for line in ls_remote_output.lines() {
        let Some((left, _)) = line.split_once('\t') else {
            continue;
        };
        // The SHA is the last whitespace-delimited token before the tab.
        if let Some(tok) = left.split_whitespace().next_back()
            && tok.len() == 40
            && tok.chars().all(|c| c.is_ascii_hexdigit())
        {
            return Some(tok.to_lowercase());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ls_remote_uses_exact_ref() {
        assert_eq!(
            ls_remote_command("https://x/r.git", "main"),
            "git ls-remote https://x/r.git refs/heads/main"
        );
    }

    #[test]
    fn clone_is_shallow_single_branch() {
        assert_eq!(
            clone_command("https://x/r.git", "dev", "/artifacts/d1"),
            "git clone -b dev --single-branch --depth 1 https://x/r.git /artifacts/d1"
        );
    }

    #[test]
    fn parses_plain_ls_remote() {
        let out = "9f1c2d3e4b5a69788091a2b3c4d5e6f708192a3b\trefs/heads/main\n";
        assert_eq!(
            parse_commit_sha(out).as_deref(),
            Some("9f1c2d3e4b5a69788091a2b3c4d5e6f708192a3b")
        );
    }

    #[test]
    fn parses_when_warning_prefixes_the_line() {
        let out = "warning: redirecting to https 9f1c2d3e4b5a69788091a2b3c4d5e6f708192a3b\trefs/heads/main";
        assert_eq!(
            parse_commit_sha(out).as_deref(),
            Some("9f1c2d3e4b5a69788091a2b3c4d5e6f708192a3b")
        );
    }

    #[test]
    fn none_when_no_sha() {
        assert_eq!(parse_commit_sha("fatal: repository not found"), None);
        assert_eq!(parse_commit_sha(""), None);
    }

    // ---- private repo goldens --------------------------------------------

    #[test]
    fn urlencode_matches_rawurlencode() {
        // Unreserved bytes pass through; everything else is %XX (uppercase hex).
        assert_eq!(urlencode("aZ0-_.~"), "aZ0-_.~");
        // A realistic installation token contains only [A-Za-z0-9_] but guard
        // the encoding of the characters that would break a URL.
        assert_eq!(urlencode("a/b c+d=e"), "a%2Fb%20c%2Bd%3De");
        assert_eq!(urlencode("ghs_AbC123_x"), "ghs_AbC123_x");
    }

    /// Golden for the private GitHub-App clone. Cite Application.php:1607-1637
    /// (`generateGitImportCommands`, GithubApp non-public branch): tokenised
    /// `insteadOf` config + `http.version=HTTP/1.1` + `--depth=1 -b <branch>` +
    /// tokenised `<repo>.git` URL + baseDir.
    #[test]
    fn github_app_private_clone_is_golden() {
        let enc = urlencode("ghs_TOKEN");
        let cmd = github_app_clone_command(
            "https",
            "github.com",
            "acme/widgets",
            "main",
            &enc,
            true,
            "/artifacts/dep-1",
        );
        assert_eq!(
            cmd,
            "git -c 'url.https://x-access-token:ghs_TOKEN@github.com/.insteadOf=https://github.com/' \
             -c http.version=HTTP/1.1 clone --depth=1 -b main \
             https://x-access-token:ghs_TOKEN@github.com/acme/widgets.git /artifacts/dep-1"
        );
    }

    #[test]
    fn github_app_private_clone_without_shallow_omits_depth() {
        let cmd = github_app_clone_command(
            "https",
            "github.com",
            "a/b",
            "dev",
            "TOK",
            false,
            "/artifacts/x",
        );
        assert!(
            cmd.contains(" clone -b dev "),
            "no --depth when not shallow: {cmd}"
        );
    }

    #[test]
    fn github_app_private_ls_remote_uses_token_and_exact_ref() {
        let cmd = github_app_ls_remote_command("https", "github.com", "a/b", "main", "TOK");
        assert_eq!(
            cmd,
            "git -c 'url.https://x-access-token:TOK@github.com/.insteadOf=https://github.com/' \
             -c http.version=HTTP/1.1 ls-remote \
             https://x-access-token:TOK@github.com/a/b.git refs/heads/main"
        );
    }

    /// Golden for the deploy-key `GIT_SSH_COMMAND` (Application.php:1734, and the
    /// GitLab-parity string at 1552): exact option order and both `-p`/`-o Port`.
    #[test]
    fn deploy_key_ssh_command_is_golden() {
        assert_eq!(
            deploy_key_ssh_command(2222, "/root/.ssh/id_rsa_coolify_dep-1"),
            "ssh -o ConnectTimeout=30 -p 2222 -o Port=2222 -o LogLevel=ERROR \
             -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
             -i /root/.ssh/id_rsa_coolify_dep-1 -o IdentitiesOnly=yes"
        );
    }

    #[test]
    fn deploy_key_clone_is_golden() {
        let key = deploy_key_path("dep-1");
        let cmd = deploy_key_clone_command(
            "git@github.com:acme/widgets.git",
            22,
            &key,
            "main",
            true,
            "/artifacts/dep-1",
        );
        assert_eq!(
            cmd,
            "GIT_SSH_COMMAND=\"ssh -o ConnectTimeout=30 -p 22 -o Port=22 -o LogLevel=ERROR \
             -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
             -i /root/.ssh/id_rsa_coolify_dep-1 -o IdentitiesOnly=yes\" \
             git clone --depth=1 -b main git@github.com:acme/widgets.git /artifacts/dep-1"
        );
    }

    #[test]
    fn deploy_key_materialise_writes_600_key() {
        let cmds = deploy_key_materialise_commands("/root/.ssh/id_rsa_coolify_dep-1", "QkFTRTY0");
        assert_eq!(cmds[0], "mkdir -p /root/.ssh");
        assert_eq!(
            cmds[1],
            "echo 'QkFTRTY0' | base64 -d | tee /root/.ssh/id_rsa_coolify_dep-1 > /dev/null"
        );
        assert_eq!(cmds[2], "chmod 600 /root/.ssh/id_rsa_coolify_dep-1");
    }

    #[test]
    fn deploy_key_ls_remote_is_golden() {
        let key = deploy_key_path("dep-1");
        let cmd = deploy_key_ls_remote_command("git@github.com:a/b.git", 22, &key, "main");
        assert!(cmd.starts_with("GIT_SSH_COMMAND=\"ssh -o ConnectTimeout=30 -p 22"));
        assert!(cmd.ends_with("git ls-remote git@github.com:a/b.git refs/heads/main"));
    }

    /// The installation token must never survive into a log line: the engine
    /// registers both the raw token and its url-encoded form as secrets, so
    /// `redact` scrubs the clone command before it is streamed.
    #[test]
    fn installation_token_is_redacted_from_logs() {
        let token = "ghs_SuPerSecret123";
        let enc = urlencode(token);
        let cmd = github_app_clone_command(
            "https",
            "github.com",
            "a/b",
            "main",
            &enc,
            true,
            "/artifacts/x",
        );
        assert!(
            cmd.contains(token),
            "precondition: token is embedded in the raw command"
        );

        let redacted = rustify_core::redact(&cmd, &[token, &enc]);
        assert!(
            !redacted.contains(token),
            "raw token leaked into logs: {redacted}"
        );
        assert!(
            !redacted.contains(&enc),
            "encoded token leaked into logs: {redacted}"
        );
        assert!(redacted.contains("[REDACTED]"));
    }
}
