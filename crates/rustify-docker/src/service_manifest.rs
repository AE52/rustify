//! One-click-service template manifest.
//!
//! Each bundled compose template (`templates/services/*.yaml`) carries a block
//! of `# key: value` header comments describing it. This module parses those
//! headers into a [`ServiceTemplate`] and builds the committed
//! `templates/services/index.json` manifest.
//!
//! Behavioural port of Coolify's `generate:services` command
//! (app/Console/Commands/Generate/Services.php): header keys `documentation`,
//! `slogan`, `category`, `tags`, `logo`, `port`; `ignore: true` skips a file;
//! `tags` is lowercased, comma-split and trimmed. `compose_b64` is the base64
//! of the (verbatim) template YAML.

use std::collections::BTreeMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};

/// One entry of the service-template manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceTemplate {
    /// Template key / file stem, e.g. `umami`.
    pub name: String,
    /// One-line description (`# slogan:`).
    #[serde(default)]
    pub slogan: String,
    /// Docs URL (`# documentation:`).
    #[serde(default)]
    pub documentation: String,
    /// Category (`# category:`).
    #[serde(default)]
    pub category: Option<String>,
    /// Lowercased, trimmed tags (`# tags:`).
    #[serde(default)]
    pub tags: Vec<String>,
    /// Logo asset path (`# logo:`).
    #[serde(default)]
    pub logo: Option<String>,
    /// Primary published port (`# port:`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    /// Base64 of the verbatim compose YAML.
    pub compose_b64: String,
}

impl ServiceTemplate {
    /// Decode the compose YAML from `compose_b64`.
    pub fn compose_yaml(&self) -> Option<String> {
        let bytes = BASE64.decode(self.compose_b64.as_bytes()).ok()?;
        String::from_utf8(bytes).ok()
    }
}

/// Parse the `# key: value` header comments of one template.
///
/// Only lines matching `^#<key>:<value>` count (the value ends at the first
/// colon, matching Coolify's non-greedy regex). Scanning stops at the first
/// non-comment, non-blank line (the compose body).
fn parse_header(raw_yaml: &str) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    for line in raw_yaml.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix('#') else {
            // First real YAML line: header block is over.
            break;
        };
        if let Some((key, value)) = rest.split_once(':') {
            headers.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    headers
}

/// Parse a single template into a [`ServiceTemplate`], or `None` when the
/// header sets `ignore: true`. `name` is the file stem (e.g. `umami`).
pub fn parse_template(name: &str, raw_yaml: &str) -> Option<ServiceTemplate> {
    let headers = parse_header(raw_yaml);
    if headers
        .get("ignore")
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "true" | "1" | "yes"))
        .unwrap_or(false)
    {
        return None;
    }

    let tags = headers
        .get("tags")
        .map(|t| {
            t.to_lowercase()
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Some(ServiceTemplate {
        name: name.to_string(),
        slogan: headers.get("slogan").cloned().unwrap_or_default(),
        documentation: headers.get("documentation").cloned().unwrap_or_default(),
        category: headers.get("category").cloned(),
        tags,
        logo: headers.get("logo").cloned(),
        port: headers.get("port").cloned(),
        compose_b64: BASE64.encode(raw_yaml.as_bytes()),
    })
}

/// Build the full manifest from `(file_stem, raw_yaml)` pairs, keyed by template
/// name and skipping `ignore: true` files. Deterministic order (BTreeMap).
pub fn build_manifest(files: &[(String, String)]) -> BTreeMap<String, ServiceTemplate> {
    let mut manifest = BTreeMap::new();
    for (name, raw) in files {
        if let Some(tpl) = parse_template(name, raw) {
            manifest.insert(name.clone(), tpl);
        }
    }
    manifest
}

/// Deserialize a committed `index.json` manifest.
pub fn load_manifest(json: &str) -> Result<BTreeMap<String, ServiceTemplate>, serde_json::Error> {
    serde_json::from_str(json)
}

#[cfg(test)]
mod tests {
    use super::*;

    const UMAMI: &str = "# documentation: https://umami.is\n\
# slogan: Umami is web analytics.\n\
# category: analytics\n\
# tags: Analytics, Insights, Privacy\n\
# logo: svgs/umami.svg\n\
# port: 3000\n\
\n\
services:\n  umami:\n    image: umami\n";

    #[test]
    fn parses_all_header_fields() {
        let t = parse_template("umami", UMAMI).unwrap();
        assert_eq!(t.name, "umami");
        assert_eq!(t.slogan, "Umami is web analytics.");
        assert_eq!(t.documentation, "https://umami.is");
        assert_eq!(t.category.as_deref(), Some("analytics"));
        assert_eq!(t.tags, vec!["analytics", "insights", "privacy"]);
        assert_eq!(t.logo.as_deref(), Some("svgs/umami.svg"));
        assert_eq!(t.port.as_deref(), Some("3000"));
    }

    #[test]
    fn compose_b64_roundtrips_to_verbatim_yaml() {
        let t = parse_template("umami", UMAMI).unwrap();
        assert_eq!(t.compose_yaml().unwrap(), UMAMI);
    }

    #[test]
    fn ignore_true_is_skipped() {
        let yaml = "# ignore: true\n# slogan: x\nservices: {}\n";
        assert!(parse_template("skip", yaml).is_none());
        let files = vec![
            ("skip".to_string(), yaml.to_string()),
            ("umami".to_string(), UMAMI.to_string()),
        ];
        let m = build_manifest(&files);
        assert_eq!(m.len(), 1);
        assert!(m.contains_key("umami"));
    }

    #[test]
    fn header_scan_stops_at_body() {
        // A `#` inside the compose body must not be read as a header.
        let yaml = "# slogan: real\nservices:\n  a:\n    image: x # inline comment: nope\n";
        let t = parse_template("a", yaml).unwrap();
        assert_eq!(t.slogan, "real");
        assert!(t.category.is_none());
    }

    /// Path to the committed `templates/services` directory (repo root).
    fn templates_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../templates/services")
    }

    /// Read every bundled template as `(stem, raw_yaml)`.
    fn read_bundled() -> Vec<(String, String)> {
        let dir = templates_dir();
        let mut files = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("templates/services exists") {
            let path = entry.unwrap().path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext != "yaml" && ext != "yml" {
                continue;
            }
            let stem = path.file_stem().unwrap().to_string_lossy().to_string();
            let raw = std::fs::read_to_string(&path).unwrap();
            files.push((stem, raw));
        }
        files
    }

    /// Regenerate the committed `templates/services/index.json` from the bundled
    /// templates. Run with `cargo test -p rustify-docker regenerate_index --
    /// --ignored`.
    #[test]
    #[ignore = "writes the committed manifest; run manually"]
    fn regenerate_index() {
        let manifest = build_manifest(&read_bundled());
        let json = serde_json::to_string_pretty(&manifest).unwrap();
        let out = templates_dir().join("index.json");
        std::fs::write(&out, json + "\n").unwrap();
        eprintln!("wrote {} entries to {}", manifest.len(), out.display());
    }

    /// The committed manifest must match a fresh parse of the bundled templates.
    #[test]
    fn committed_index_matches_bundled_templates() {
        let expected = build_manifest(&read_bundled());
        let json = std::fs::read_to_string(templates_dir().join("index.json"))
            .expect("index.json is committed");
        let committed = load_manifest(&json).expect("index.json parses");
        assert_eq!(
            committed, expected,
            "index.json is stale; run the ignored `regenerate_index` test"
        );
    }

    #[test]
    fn manifest_json_roundtrip() {
        let files = vec![("umami".to_string(), UMAMI.to_string())];
        let m = build_manifest(&files);
        let json = serde_json::to_string_pretty(&m).unwrap();
        let back = load_manifest(&json).unwrap();
        assert_eq!(m, back);
    }
}
