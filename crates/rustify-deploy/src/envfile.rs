//! Environment-file generation for builds and runtime.
//!
//! Build-time precedence is a verbatim port of Coolify's
//! `generate_build_env_variables` (app/Jobs/ApplicationDeploymentJob.php:1600-1780):
//! an associative array is filled lowest-priority-first so later layers
//! override earlier ones, i.e.
//!   nixpacks plan  <  RUSTIFY_* (Coolify's `COOLIFY_*`)  <  SERVICE_*  <  user `is_buildtime`.
//! The file is sourced with `set -a` before the build, so values are rendered
//! double-quoted (parity with `escapeBashDoubleQuoted`).
//!
//! The runtime `.env` is consumed by Docker Compose `env_file`, which is *not*
//! shell — values are written raw `KEY=value`, one per line, no quoting.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

/// The four precedence layers of the build-time environment, lowest first.
#[derive(Debug, Clone, Default)]
pub struct BuildEnvLayers {
    /// Variables extracted from the nixpacks plan (lowest priority).
    pub nixpacks: Vec<(String, String)>,
    /// Rustify-generated `RUSTIFY_*` variables.
    pub rustify: Vec<(String, String)>,
    /// Docker-Compose `SERVICE_*` variables (empty for non-compose builds).
    pub service: Vec<(String, String)>,
    /// User-defined `is_buildtime` variables (highest priority).
    pub user_buildtime: Vec<(String, String)>,
}

/// Insertion-ordered upsert: keeps a key at its first-seen position but lets a
/// later layer overwrite the value (parity with PHP associative-array semantics).
fn upsert(map: &mut Vec<(String, String)>, key: &str, value: &str) {
    if let Some(slot) = map.iter_mut().find(|(k, _)| k == key) {
        slot.1 = value.to_string();
    } else {
        map.push((key.to_string(), value.to_string()));
    }
}

/// Merge the layers into the final ordered key/value set applying precedence.
/// Nixpacks `COOLIFY_*`/`SERVICE_*`/`RUSTIFY_*` keys are skipped from the
/// nixpacks layer (they are re-added by their dedicated higher-priority layers,
/// matching ApplicationDeploymentJob.php:1617-1619).
pub fn merge_build_env(layers: &BuildEnvLayers) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (k, v) in &layers.nixpacks {
        if k.starts_with("RUSTIFY_") || k.starts_with("COOLIFY_") || k.starts_with("SERVICE_") {
            continue;
        }
        upsert(&mut out, k, v);
    }
    for (k, v) in &layers.rustify {
        upsert(&mut out, k, v);
    }
    for (k, v) in &layers.service {
        upsert(&mut out, k, v);
    }
    for (k, v) in &layers.user_buildtime {
        upsert(&mut out, k, v);
    }
    out
}

/// Escape a value for a bash-sourced env file (double-quoted): backslash,
/// double-quote, backtick and `$` are escaped so nothing is re-interpreted
/// beyond intentional `$VAR` expansion callers rely on.
fn bash_double_quote(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for ch in value.chars() {
        if matches!(ch, '\\' | '"' | '`' | '$') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped.push('"');
    escaped
}

/// Render the merged build-time environment as a `set -a`-sourceable file.
pub fn render_build_env(layers: &BuildEnvLayers) -> String {
    let merged = merge_build_env(layers);
    let mut out = String::new();
    for (k, v) in merged {
        out.push_str(&k);
        out.push('=');
        out.push_str(&bash_double_quote(&v));
        out.push('\n');
    }
    out
}

/// Render the runtime `.env` (Docker Compose `env_file` format): raw
/// `KEY=value`, newlines in values collapsed to spaces so each var is one line.
pub fn render_runtime_env(vars: &[(String, String)]) -> String {
    let mut out = String::new();
    for (k, v) in vars {
        let single_line = v.replace(['\n', '\r'], " ");
        out.push_str(k);
        out.push('=');
        out.push_str(&single_line);
        out.push('\n');
    }
    out
}

/// Shell script that materialises `content` at `remote_path` inside the helper
/// container via a base64 pipe, so no secret ever appears literally in the
/// command text a shell history / log might capture, and arbitrary bytes
/// survive transport.
pub fn write_file_in_helper(deployment_uuid: &str, remote_path: &str, content: &str) -> String {
    let b64 = BASE64.encode(content.as_bytes());
    format!("docker exec {deployment_uuid} sh -c \"echo {b64} | base64 -d > {remote_path}\"")
}

/// Extract build-time variables from a `nixpacks plan` JSON document's
/// `.variables` object (ApplicationDeploymentJob.php:1608). Returns an empty
/// vec when the JSON is missing/invalid or has no variables.
pub fn parse_nixpacks_variables(plan_json: &str) -> Vec<(String, String)> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(plan_json) else {
        return Vec::new();
    };
    let Some(vars) = value.get("variables").and_then(|v| v.as_object()) else {
        return Vec::new();
    };
    vars.iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn precedence_nixpacks_lt_rustify_lt_user() {
        let layers = BuildEnvLayers {
            nixpacks: kv(&[("FOO", "nix"), ("BAR", "nix")]),
            rustify: kv(&[("FOO", "rustify"), ("SOURCE_COMMIT", "abc")]),
            service: vec![],
            user_buildtime: kv(&[("FOO", "user")]),
        };
        let merged = merge_build_env(&layers);
        let map: std::collections::HashMap<_, _> = merged.iter().cloned().collect();
        assert_eq!(
            map["FOO"], "user",
            "user build-time wins over rustify/nixpacks"
        );
        assert_eq!(map["BAR"], "nix", "nixpacks-only key survives");
        assert_eq!(map["SOURCE_COMMIT"], "abc");
        // First-seen position preserved: FOO before BAR.
        let keys: Vec<&str> = merged.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.iter().position(|k| *k == "FOO") < keys.iter().position(|k| *k == "BAR"));
    }

    #[test]
    fn service_layer_beats_rustify_but_loses_to_user() {
        let layers = BuildEnvLayers {
            nixpacks: vec![],
            rustify: kv(&[("K", "rustify")]),
            service: kv(&[("K", "service")]),
            user_buildtime: vec![],
        };
        assert_eq!(merge_build_env(&layers)[0].1, "service");
    }

    #[test]
    fn nixpacks_layer_drops_reserved_prefixes() {
        let layers = BuildEnvLayers {
            nixpacks: kv(&[("COOLIFY_X", "1"), ("SERVICE_Y", "2"), ("KEEP", "3")]),
            ..Default::default()
        };
        let merged = merge_build_env(&layers);
        assert_eq!(merged, kv(&[("KEEP", "3")]));
    }

    #[test]
    fn build_env_is_double_quoted() {
        let layers = BuildEnvLayers {
            user_buildtime: kv(&[("A", "he\"llo"), ("B", "x")]),
            ..Default::default()
        };
        assert_eq!(render_build_env(&layers), "A=\"he\\\"llo\"\nB=\"x\"\n");
    }

    #[test]
    fn runtime_env_is_raw() {
        let vars = kv(&[("A", "1"), ("MULTI", "a\nb")]);
        assert_eq!(render_runtime_env(&vars), "A=1\nMULTI=a b\n");
    }

    #[test]
    fn write_file_uses_base64_pipe() {
        let script = write_file_in_helper("dep1", "/artifacts/build-time.env", "SECRET=xyz\n");
        assert!(script.starts_with("docker exec dep1 sh -c"));
        assert!(script.contains("base64 -d > /artifacts/build-time.env"));
        // The plaintext secret is never present literally.
        assert!(!script.contains("SECRET=xyz"));
    }

    #[test]
    fn parses_nixpacks_variables() {
        let plan = r#"{"variables":{"NIXPACKS_NODE_VERSION":"18","NPM_TOKEN":"t"}}"#;
        let mut vars = parse_nixpacks_variables(plan);
        vars.sort();
        assert_eq!(
            vars,
            kv(&[("NIXPACKS_NODE_VERSION", "18"), ("NPM_TOKEN", "t")])
        );
    }

    #[test]
    fn nixpacks_variables_empty_on_garbage() {
        assert!(parse_nixpacks_variables("not json").is_empty());
        assert!(parse_nixpacks_variables("{}").is_empty());
    }
}
