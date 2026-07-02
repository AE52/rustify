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
}
