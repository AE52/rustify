//! Secret redaction for deployment logs.
//!
//! Behavior parity with Coolify's `redactSensitiveInfo`
//! (coolify/app/Models/ApplicationDeploymentQueue.php:140-179, which replaces
//! each sensitive env value in log text with a `REDACTED` marker defined in
//! coolify/bootstrap/helpers/constants.php:12). Rustify's marker is
//! `[REDACTED]`; longer secrets are replaced first so overlapping secrets
//! never leak a suffix/prefix.

const REDACTED: &str = "[REDACTED]";

/// Replace every occurrence of each non-empty secret in `content` with
/// `[REDACTED]`. Empty secrets are ignored.
pub fn redact(content: &str, secrets: &[&str]) -> String {
    let mut secrets: Vec<&str> = secrets.iter().copied().filter(|s| !s.is_empty()).collect();
    // Longest first so overlapping secrets are fully redacted; lexicographic
    // tiebreak keeps the pass deterministic and groups duplicates for dedup.
    secrets.sort_unstable_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    secrets.dedup();
    let mut out = content.to_string();
    for secret in secrets {
        out = out.replace(secret, REDACTED);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_every_occurrence() {
        assert_eq!(
            redact("token=abc123 retry token=abc123", &["abc123"]),
            "token=[REDACTED] retry token=[REDACTED]"
        );
    }

    #[test]
    fn replaces_multiple_secrets() {
        assert_eq!(
            redact("user=alice pass=hunter2", &["alice", "hunter2"]),
            "user=[REDACTED] pass=[REDACTED]"
        );
    }

    #[test]
    fn overlapping_secrets_longest_first() {
        // "secretkey" contains "secret"; the longer secret must win so no
        // suffix of it survives in the output.
        assert_eq!(
            redact("key=secretkey and word=secret", &["secret", "secretkey"]),
            "key=[REDACTED] and word=[REDACTED]"
        );
        // order in the slice must not matter
        assert_eq!(
            redact("key=secretkey and word=secret", &["secretkey", "secret"]),
            "key=[REDACTED] and word=[REDACTED]"
        );
    }

    #[test]
    fn overlapping_occurrences_replace_left_to_right() {
        assert_eq!(redact("aaa", &["aa"]), "[REDACTED]a");
    }

    #[test]
    fn empty_secret_list_is_noop() {
        assert_eq!(redact("nothing to hide", &[]), "nothing to hide");
    }

    #[test]
    fn empty_string_secret_is_ignored() {
        assert_eq!(redact("plain text", &["", ""]), "plain text");
        assert_eq!(redact("pass=x", &["", "x"]), "pass=[REDACTED]");
    }

    #[test]
    fn duplicate_secrets_behave_like_one() {
        assert_eq!(redact("v=s3cr3t", &["s3cr3t", "s3cr3t"]), "v=[REDACTED]");
    }

    #[test]
    fn no_match_returns_content_unchanged() {
        assert_eq!(redact("all public", &["s3cr3t"]), "all public");
    }
}
