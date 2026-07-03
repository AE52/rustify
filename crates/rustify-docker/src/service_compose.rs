//! One-click-service compose parser + mutator.
//!
//! Behavioural port of Coolify's service compose parser
//! (bootstrap/helpers/parsers.php `newParser`, lines ~440-1460) reduced to the
//! Phase-2 surface pinned by the track brief:
//!
//! - Resolve every `SERVICE_*` magic variable declared **or referenced** in the
//!   template (parsers.php:440-660). `SERVICE_FQDN_*` / `SERVICE_URL_*` always
//!   produce BOTH an FQDN (host only) and a URL (scheme://host[:port][/path])
//!   env var, plus a port-suffixed pair when the key carries a trailing numeric
//!   segment (parseServiceEnvironmentVariable, services.php:423-455). Every
//!   other command is generated via [`rustify_core::service_vars`]
//!   (generateEnvValue). Already-generated values are reused (persist-once).
//! - Inject the `COOLIFY_*` bookkeeping vars (parsers.php:1152-1277) and one
//!   `SERVICE_NAME_<UPPER>` per service (shared.php:3783-3790).
//! - Mutate every service: deterministic `container_name` (`<svc>-<uuid>`,
//!   parsers.php:1649), named-volume renaming to `<uuid>_<slug>`
//!   (parsers.php:835-836), attach the per-stack external network `<uuid>`,
//!   `env_file: ['.env']`, and the `rustify.*` + Traefik labels.

use std::collections::{BTreeMap, BTreeSet};

use rustify_core::service_vars::{
    generate_service_var, is_secret_command, parse_command, supabase_jwt,
};
use serde_yaml::{Mapping, Value};

/// The env-var `resource_kind` discriminator for services.
pub const SERVICE_RESOURCE_KIND: &str = "service";
/// Signing-key variable Supabase JWTs are derived from (Coolify convention).
const JWT_SIGNING_KEY: &str = "SERVICE_PASSWORD_JWT";

#[derive(Debug, thiserror::Error)]
pub enum ServiceComposeError {
    #[error("invalid compose yaml: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("compose template has no services")]
    NoServices,
}

/// Result of mutating a service template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutatedService {
    /// The rewritten, deploy-ready compose YAML.
    pub compose_mutated: String,
    /// Resolved environment for the shared `.env` file: `(key, value,
    /// is_shown_once)`, sorted by key.
    pub env: Vec<(String, String, bool)>,
}

/// FQDN/URL resolved for one service (host has no scheme).
#[derive(Debug, Clone)]
struct FqdnInfo {
    host: String,
    url: String,
    port: Option<String>,
}

/// Parse `template_yaml` and produce a mutated compose + resolved env for a
/// service instance identified by `service_uuid` / `key`.
///
/// - `fqdn_base` is the host (no scheme) FQDN/URL vars resolve against.
/// - `existing_env` holds already-persisted values so generated secrets and
///   FQDNs stay stable across redeploys (persist-once).
pub fn parse_and_mutate_service(
    template_yaml: &str,
    service_uuid: &str,
    key: &str,
    fqdn_base: &str,
    existing_env: &BTreeMap<String, String>,
) -> Result<MutatedService, ServiceComposeError> {
    let mut doc: Value = serde_yaml::from_str(template_yaml)?;
    let services = doc
        .get("services")
        .and_then(Value::as_mapping)
        .ok_or(ServiceComposeError::NoServices)?
        .clone();

    // 1. Collect every magic variable, declared (with optional value) or
    //    referenced elsewhere in the template.
    let magic = collect_magic_vars(&services, template_yaml);

    // 2. Resolve them into concrete env entries (persist-once).
    let mut env: BTreeMap<String, (String, bool)> = BTreeMap::new();
    let mut fqdn_by_service: BTreeMap<String, FqdnInfo> = BTreeMap::new();

    for (name, declared) in &magic {
        let Some(command) = parse_command(name) else {
            continue;
        };
        if command == "FQDN" || command == "URL" {
            resolve_fqdn_var(
                name,
                declared.as_deref(),
                fqdn_base,
                existing_env,
                &mut env,
                &mut fqdn_by_service,
            );
        } else {
            resolve_value_var(name, &command, existing_env, &mut env);
        }
    }

    // 3. Inject COOLIFY_* bookkeeping + one SERVICE_NAME_<UPPER> per service.
    inject_bookkeeping(
        service_uuid,
        key,
        &services,
        &fqdn_by_service,
        existing_env,
        &mut env,
    );

    // 4. Mutate every service in place.
    let mut mutated = Mapping::new();
    for (svc_key, svc_val) in services.iter() {
        let child = svc_key.as_str().unwrap_or_default().to_string();
        let mutated_svc = mutate_service(svc_val, &child, service_uuid, &mut doc, &fqdn_by_service);
        mutated.insert(Value::String(child), mutated_svc);
    }

    // 5. Rewrite top-level services + attach the per-stack external network.
    if let Some(map) = doc.as_mapping_mut() {
        map.insert(Value::String("services".into()), Value::Mapping(mutated));
        let mut networks = Mapping::new();
        let mut net_def = Mapping::new();
        net_def.insert(Value::String("external".into()), Value::Bool(true));
        networks.insert(
            Value::String(service_uuid.to_string()),
            Value::Mapping(net_def),
        );
        map.insert(Value::String("networks".into()), Value::Mapping(networks));
    }

    let compose_mutated = serde_yaml::to_string(&doc)?;
    let env: Vec<(String, String, bool)> =
        env.into_iter().map(|(k, (v, once))| (k, v, once)).collect();

    Ok(MutatedService {
        compose_mutated,
        env,
    })
}

/// Collect magic `SERVICE_*` variables: those declared in a service's
/// `environment` (with their optional `=value`) plus any referenced as
/// `$SERVICE_*` / `${SERVICE_*}` anywhere in the raw template.
fn collect_magic_vars(services: &Mapping, raw: &str) -> BTreeMap<String, Option<String>> {
    let mut magic: BTreeMap<String, Option<String>> = BTreeMap::new();

    for (_, svc) in services.iter() {
        let Some(env) = svc.get("environment") else {
            continue;
        };
        match env {
            Value::Sequence(seq) => {
                for item in seq {
                    if let Some(s) = item.as_str() {
                        let (k, v) = match s.split_once('=') {
                            Some((k, v)) => (k.trim(), Some(v.to_string())),
                            None => (s.trim(), None),
                        };
                        if k.starts_with("SERVICE_") {
                            magic.entry(k.to_string()).or_insert(v);
                        }
                    }
                }
            }
            Value::Mapping(map) => {
                for (k, v) in map {
                    if let Some(k) = k.as_str() {
                        if k.starts_with("SERVICE_") {
                            let val = v.as_str().map(str::to_string);
                            magic.entry(k.to_string()).or_insert(val);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Referenced variables (`$SERVICE_X` / `${SERVICE_X}`) with no declaration.
    for name in scan_service_refs(raw) {
        magic.entry(name).or_insert(None);
    }
    magic
}

/// Find every `SERVICE_*` token used as a `$`-reference in the raw template.
fn scan_service_refs(raw: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' {
            let mut j = i + 1;
            if j < bytes.len() && bytes[j] == b'{' {
                j += 1;
            }
            let start = j;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            let token = &raw[start..j];
            if token.starts_with("SERVICE_") {
                out.insert(token.to_string());
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// Resolve a `SERVICE_FQDN_*` / `SERVICE_URL_*` variable into BOTH the FQDN and
/// URL env vars (base + optional port pair). Persist-once via `existing_env`.
fn resolve_fqdn_var(
    name: &str,
    declared: Option<&str>,
    fqdn_base: &str,
    existing_env: &BTreeMap<String, String>,
    env: &mut BTreeMap<String, (String, bool)>,
    fqdn_by_service: &mut BTreeMap<String, FqdnInfo>,
) {
    let (preserved, port) = split_service_name(name);
    // A declared value starting with `/` (and not just `/`) is a URL path.
    let path = declared
        .filter(|v| v.starts_with('/') && *v != "/")
        .unwrap_or("");

    let host = format!("{fqdn_base}{path}");
    let url = format!("https://{fqdn_base}{path}");
    let fqdn_key = format!("SERVICE_FQDN_{preserved}");
    let url_key = format!("SERVICE_URL_{preserved}");

    put_persisted(env, existing_env, &fqdn_key, host.clone(), false);
    put_persisted(env, existing_env, &url_key, url.clone(), false);

    if let Some(port) = &port {
        let host_p = format!("{fqdn_base}:{port}{path}");
        let url_p = format!("https://{fqdn_base}:{port}{path}");
        put_persisted(
            env,
            existing_env,
            &format!("{fqdn_key}_{port}"),
            host_p,
            false,
        );
        put_persisted(
            env,
            existing_env,
            &format!("{url_key}_{port}"),
            url_p,
            false,
        );
    }

    let norm = normalize_service_name(&preserved);
    fqdn_by_service.entry(norm).or_insert(FqdnInfo {
        host: fqdn_base.to_string(),
        url,
        port,
    });
}

/// Resolve a non-FQDN magic variable via `generate_service_var`, persisting
/// once. Supabase JWTs derive from (and lazily create) the shared JWT key.
fn resolve_value_var(
    name: &str,
    command: &str,
    existing_env: &BTreeMap<String, String>,
    env: &mut BTreeMap<String, (String, bool)>,
) {
    let once = is_secret_command(command);
    if let Some(existing) = existing_env.get(name) {
        env.insert(name.to_string(), (existing.clone(), once));
        return;
    }
    let value = if command == "SUPABASEANON" || command == "SUPABASESERVICE" {
        let jwt_key = ensure_jwt_key(existing_env, env);
        let role = if command == "SUPABASEANON" {
            "anon"
        } else {
            "service_role"
        };
        supabase_jwt(role, &jwt_key)
    } else {
        match generate_service_var(command) {
            Some(v) => v,
            None => return, // unknown command: leave the ${VAR} reference intact
        }
    };
    env.insert(name.to_string(), (value, once));
}

/// Ensure the shared `SERVICE_PASSWORD_JWT` exists, generating it if needed.
fn ensure_jwt_key(
    existing_env: &BTreeMap<String, String>,
    env: &mut BTreeMap<String, (String, bool)>,
) -> String {
    if let Some(v) = existing_env.get(JWT_SIGNING_KEY) {
        return v.clone();
    }
    if let Some((v, _)) = env.get(JWT_SIGNING_KEY) {
        return v.clone();
    }
    let key = generate_service_var("PASSWORD").unwrap_or_default();
    env.insert(JWT_SIGNING_KEY.to_string(), (key.clone(), true));
    key
}

/// Inject the `COOLIFY_*` bookkeeping vars and per-service `SERVICE_NAME_*`.
fn inject_bookkeeping(
    service_uuid: &str,
    key: &str,
    services: &Mapping,
    fqdn_by_service: &BTreeMap<String, FqdnInfo>,
    existing_env: &BTreeMap<String, String>,
    env: &mut BTreeMap<String, (String, bool)>,
) {
    put_persisted(
        env,
        existing_env,
        "COOLIFY_RESOURCE_UUID",
        service_uuid.to_string(),
        false,
    );
    put_persisted(
        env,
        existing_env,
        "COOLIFY_CONTAINER_NAME",
        format!("{key}-{service_uuid}"),
        false,
    );

    // SERVICE_NAME_<UPPER(svc with -/. -> _)> = <svc> (shared.php:3787).
    for (svc_key, _) in services.iter() {
        if let Some(child) = svc_key.as_str() {
            let upper = normalize_service_name(child).to_uppercase();
            put_persisted(
                env,
                existing_env,
                &format!("SERVICE_NAME_{upper}"),
                child.to_string(),
                false,
            );
        }
    }

    // COOLIFY_FQDN / COOLIFY_URL = comma-joined resolved hosts/urls.
    if !fqdn_by_service.is_empty() {
        let fqdns = fqdn_by_service
            .values()
            .map(|f| f.host.clone())
            .collect::<Vec<_>>()
            .join(",");
        let urls = fqdn_by_service
            .values()
            .map(|f| f.url.clone())
            .collect::<Vec<_>>()
            .join(",");
        put_persisted(env, existing_env, "COOLIFY_FQDN", fqdns, false);
        put_persisted(env, existing_env, "COOLIFY_URL", urls, false);
    }
}

/// Mutate a single service definition: container name, env_file, network,
/// volumes and labels.
fn mutate_service(
    svc: &Value,
    child: &str,
    service_uuid: &str,
    doc: &mut Value,
    fqdn_by_service: &BTreeMap<String, FqdnInfo>,
) -> Value {
    let mut map = svc.as_mapping().cloned().unwrap_or_default();

    // container_name = <svc>-<uuid> (parsers.php:1649).
    map.insert(
        Value::String("container_name".into()),
        Value::String(format!("{child}-{service_uuid}")),
    );
    // Every managed service loads the shared .env.
    map.insert(
        Value::String("env_file".into()),
        Value::Sequence(vec![Value::String(".env".into())]),
    );
    // Attach the per-stack external network.
    map.insert(
        Value::String("networks".into()),
        Value::Sequence(vec![Value::String(service_uuid.to_string())]),
    );

    // Rename named volumes to <uuid>_<slug> (parsers.php:835-836).
    rewrite_volumes(&mut map, service_uuid, doc);

    // Labels: rustify bookkeeping + Traefik for FQDN services (Contract C7).
    let mut labels = vec![
        "rustify.managed=true".to_string(),
        "rustify.type=service".to_string(),
        format!("rustify.serviceUuid={service_uuid}"),
    ];
    if let Some(info) = fqdn_by_service.get(&normalize_service_name(child)) {
        labels.extend(traefik_service_labels(child, service_uuid, info));
    }
    map.insert(
        Value::String("labels".into()),
        Value::Sequence(labels.into_iter().map(Value::String).collect()),
    );

    Value::Mapping(map)
}

/// Rewrite a service's named-volume mounts to the per-stack `<uuid>_<slug>` name
/// and register the renamed volume in the top-level `volumes:` map.
///
/// A mount `source:target` is a *named* volume (renamed) unless `source` is a
/// bind mount — an absolute/relative/home path. This matches Coolify's
/// volume-vs-bind detection (parsers.php `parseDockerVolumeString`), so named
/// volumes are renamed even when the template omits the top-level declaration.
fn rewrite_volumes(map: &mut Mapping, service_uuid: &str, doc: &mut Value) {
    let Some(Value::Sequence(vols)) = map.get("volumes").cloned() else {
        return;
    };
    let mut new_vols = Vec::new();
    let mut renamed: BTreeMap<String, String> = BTreeMap::new();
    for v in vols {
        if let Some(s) = v.as_str() {
            if let Some((source, rest)) = s.split_once(':') {
                if is_named_volume(source) {
                    let new_name = format!("{service_uuid}_{}", slug(source));
                    renamed.insert(source.to_string(), new_name.clone());
                    new_vols.push(Value::String(format!("{new_name}:{rest}")));
                    continue;
                }
            }
        }
        new_vols.push(v);
    }
    map.insert(Value::String("volumes".into()), Value::Sequence(new_vols));

    // Register/replace the renamed volumes in the top-level `volumes:` map.
    if !renamed.is_empty() {
        let top_map = doc.as_mapping_mut().expect("compose root is a mapping");
        if !top_map.contains_key(Value::String("volumes".into())) {
            top_map.insert(
                Value::String("volumes".into()),
                Value::Mapping(Mapping::new()),
            );
        }
        if let Some(top) = top_map
            .get_mut(Value::String("volumes".into()))
            .and_then(Value::as_mapping_mut)
        {
            for (old, new) in &renamed {
                top.remove(Value::String(old.clone()));
                let mut def = Mapping::new();
                def.insert(Value::String("name".into()), Value::String(new.clone()));
                top.insert(Value::String(new.clone()), Value::Mapping(def));
            }
        }
    }
}

/// True when a volume mount `source` names a Docker volume rather than a bind
/// mount (absolute, relative `./`..`../`, or `~` home paths are binds).
fn is_named_volume(source: &str) -> bool {
    !source.is_empty()
        && !source.starts_with('/')
        && !source.starts_with('.')
        && !source.starts_with('~')
        && !source.contains('$')
}

/// Traefik router/service labels for an FQDN-exposed service.
fn traefik_service_labels(child: &str, service_uuid: &str, info: &FqdnInfo) -> Vec<String> {
    let router = format!("{child}-{service_uuid}");
    let port = info.port.clone().unwrap_or_else(|| "80".to_string());
    vec![
        "traefik.enable=true".to_string(),
        format!("traefik.http.routers.{router}.rule=Host(`{}`)", info.host),
        format!("traefik.http.routers.{router}.entrypoints=http"),
        format!(
            "traefik.http.routers.{router}-secure.rule=Host(`{}`)",
            info.host
        ),
        format!("traefik.http.routers.{router}-secure.entrypoints=https"),
        format!("traefik.http.routers.{router}-secure.tls.certresolver=letsencrypt"),
        format!("traefik.http.services.{router}.loadbalancer.server.port={port}"),
    ]
}

/// Insert `key=value` unless `existing_env` already holds a value for it.
fn put_persisted(
    env: &mut BTreeMap<String, (String, bool)>,
    existing_env: &BTreeMap<String, String>,
    key: &str,
    value: String,
    once: bool,
) {
    let v = existing_env.get(key).cloned().unwrap_or(value);
    env.insert(key.to_string(), (v, once));
}

/// Split a `SERVICE_FQDN_<NAME>[_<PORT>]` key into the case-preserved name and
/// optional port (parseServiceEnvironmentVariable, services.php:423-455).
fn split_service_name(key: &str) -> (String, Option<String>) {
    let rest = key
        .strip_prefix("SERVICE_URL_")
        .or_else(|| key.strip_prefix("SERVICE_FQDN_"))
        .unwrap_or(key);
    let last = rest.rsplit('_').next().unwrap_or("");
    let has_port = !last.is_empty() && last.bytes().all(|b| b.is_ascii_digit());
    if has_port {
        let name = &rest[..rest.len() - last.len() - 1];
        (name.to_string(), Some(last.to_string()))
    } else {
        (rest.to_string(), None)
    }
}

/// Normalize a service name for matching: lowercase, `-`/`.` → `_`.
fn normalize_service_name(name: &str) -> String {
    name.to_lowercase().replace(['-', '.'], "_")
}

/// Docker-safe volume slug: lowercase, non-alphanumeric runs collapsed to `-`.
fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    #[test]
    fn split_service_name_with_and_without_port() {
        assert_eq!(
            split_service_name("SERVICE_URL_APP_3000"),
            ("APP".to_string(), Some("3000".to_string()))
        );
        assert_eq!(
            split_service_name("SERVICE_FQDN_UMAMI"),
            ("UMAMI".to_string(), None)
        );
        assert_eq!(
            split_service_name("SERVICE_URL_MY_API_8080"),
            ("MY_API".to_string(), Some("8080".to_string()))
        );
    }

    #[test]
    fn slug_collapses_non_alnum() {
        assert_eq!(slug("postgresql-data"), "postgresql-data");
        assert_eq!(slug("pg_data.vol"), "pg-data-vol");
    }

    #[test]
    fn scans_referenced_service_vars() {
        let raw = "a: $SERVICE_PASSWORD_DB\nb: ${SERVICE_USER_POSTGRES}\nc: nothing";
        let refs = scan_service_refs(raw);
        assert!(refs.contains("SERVICE_PASSWORD_DB"));
        assert!(refs.contains("SERVICE_USER_POSTGRES"));
        assert_eq!(refs.len(), 2);
    }

    // A minimal template exercising SERVICE_FQDN (list, port-suffixed) +
    // SERVICE_PASSWORD (referenced) + a named volume. Mirrors umami.yaml
    // (templates/services/umami.yaml).
    const TEMPLATE: &str = "\
services:
  umami:
    image: ghcr.io/umami-software/umami:latest
    environment:
      - SERVICE_URL_UMAMI_3000
      - APP_SECRET=$SERVICE_PASSWORD_64_UMAMI
    volumes:
      - umami-data:/app/data
volumes:
  umami-data:
";

    #[test]
    fn fqdn_creates_both_pairs_and_port_variants() {
        let out =
            parse_and_mutate_service(TEMPLATE, "svc123", "umami", "umami.example.com", &empty())
                .unwrap();
        let keys: BTreeMap<&str, &str> = out
            .env
            .iter()
            .map(|(k, v, _)| (k.as_str(), v.as_str()))
            .collect();

        // BOTH base pairs (cite parsers.php:560-578).
        assert_eq!(keys.get("SERVICE_FQDN_UMAMI"), Some(&"umami.example.com"));
        assert_eq!(
            keys.get("SERVICE_URL_UMAMI"),
            Some(&"https://umami.example.com")
        );
        // Port-suffixed pair (parsers.php:580-597).
        assert_eq!(
            keys.get("SERVICE_FQDN_UMAMI_3000"),
            Some(&"umami.example.com:3000")
        );
        assert_eq!(
            keys.get("SERVICE_URL_UMAMI_3000"),
            Some(&"https://umami.example.com:3000")
        );
    }

    #[test]
    fn referenced_password_is_generated_once_and_secret() {
        let out =
            parse_and_mutate_service(TEMPLATE, "svc123", "umami", "umami.example.com", &empty())
                .unwrap();
        let pw = out
            .env
            .iter()
            .find(|(k, _, _)| k == "SERVICE_PASSWORD_64_UMAMI")
            .expect("password generated for referenced var");
        assert_eq!(pw.1.len(), 64, "PASSWORD_64 length");
        assert!(pw.2, "password is shown-once");
    }

    #[test]
    fn persist_once_reuses_existing_values() {
        let mut existing = BTreeMap::new();
        existing.insert(
            "SERVICE_PASSWORD_64_UMAMI".to_string(),
            "KEEP_ME".to_string(),
        );
        let out =
            parse_and_mutate_service(TEMPLATE, "svc123", "umami", "umami.example.com", &existing)
                .unwrap();
        let pw = out
            .env
            .iter()
            .find(|(k, _, _)| k == "SERVICE_PASSWORD_64_UMAMI")
            .unwrap();
        assert_eq!(pw.1, "KEEP_ME");
    }

    #[test]
    fn injects_bookkeeping_vars() {
        let out =
            parse_and_mutate_service(TEMPLATE, "svc123", "umami", "umami.example.com", &empty())
                .unwrap();
        let keys: BTreeMap<&str, &str> = out
            .env
            .iter()
            .map(|(k, v, _)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(keys.get("COOLIFY_RESOURCE_UUID"), Some(&"svc123"));
        assert_eq!(keys.get("COOLIFY_CONTAINER_NAME"), Some(&"umami-svc123"));
        assert_eq!(keys.get("SERVICE_NAME_UMAMI"), Some(&"umami"));
        assert_eq!(keys.get("COOLIFY_FQDN"), Some(&"umami.example.com"));
    }

    /// Golden run against the verbatim bundled `umami.yaml`
    /// (templates/services/umami.yaml), which declares `SERVICE_URL_UMAMI_3000`
    /// and references `SERVICE_PASSWORD_64_UMAMI`, `SERVICE_USER_POSTGRES`,
    /// `SERVICE_PASSWORD_POSTGRES`. Exercises the full FQDN pair generation
    /// (parsers.php:560-597) + generateEnvValue path (parsers.php:630).
    #[test]
    fn golden_real_umami_template() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../templates/services/umami.yaml");
        let raw = std::fs::read_to_string(&path).expect("bundled umami.yaml");
        let out = parse_and_mutate_service(&raw, "svc123", "umami", "umami.example.com", &empty())
            .unwrap();

        let keys: BTreeMap<&str, &str> = out
            .env
            .iter()
            .map(|(k, v, _)| (k.as_str(), v.as_str()))
            .collect();
        // FQDN pair for the exposed web service (BOTH created).
        assert_eq!(keys.get("SERVICE_FQDN_UMAMI"), Some(&"umami.example.com"));
        assert_eq!(
            keys.get("SERVICE_URL_UMAMI_3000"),
            Some(&"https://umami.example.com:3000")
        );
        // Referenced secrets generated.
        assert_eq!(
            keys.get("SERVICE_PASSWORD_64_UMAMI").map(|s| s.len()),
            Some(64)
        );
        assert!(keys.contains_key("SERVICE_USER_POSTGRES"));
        assert!(keys.contains_key("SERVICE_PASSWORD_POSTGRES"));
        // Per-service SERVICE_NAME injections.
        assert_eq!(keys.get("SERVICE_NAME_UMAMI"), Some(&"umami"));
        assert_eq!(keys.get("SERVICE_NAME_POSTGRESQL"), Some(&"postgresql"));

        // Both services mutated with container_name + network + labels.
        let doc: Value = serde_yaml::from_str(&out.compose_mutated).unwrap();
        assert_eq!(doc["services"]["umami"]["container_name"], "umami-svc123");
        assert_eq!(
            doc["services"]["postgresql"]["container_name"],
            "postgresql-svc123"
        );
        assert_eq!(doc["networks"]["svc123"]["external"], Value::Bool(true));
        // postgres named volume renamed.
        assert_eq!(
            doc["volumes"]["svc123_postgresql-data"]["name"],
            "svc123_postgresql-data"
        );
    }

    #[test]
    fn mutates_compose_service() {
        let out =
            parse_and_mutate_service(TEMPLATE, "svc123", "umami", "umami.example.com", &empty())
                .unwrap();
        let doc: Value = serde_yaml::from_str(&out.compose_mutated).unwrap();
        let svc = &doc["services"]["umami"];
        assert_eq!(svc["container_name"], Value::String("umami-svc123".into()));
        assert_eq!(
            svc["env_file"],
            Value::Sequence(vec![Value::String(".env".into())])
        );
        assert_eq!(
            svc["networks"],
            Value::Sequence(vec![Value::String("svc123".into())])
        );
        // Named volume renamed to <uuid>_<slug>.
        assert_eq!(
            svc["volumes"],
            Value::Sequence(vec![Value::String("svc123_umami-data:/app/data".into())])
        );
        assert_eq!(
            doc["volumes"]["svc123_umami-data"]["name"],
            "svc123_umami-data"
        );
        // External per-stack network.
        assert_eq!(doc["networks"]["svc123"]["external"], Value::Bool(true));
        // rustify + traefik labels present.
        let labels = svc["labels"].as_sequence().unwrap();
        let labels: Vec<&str> = labels.iter().filter_map(Value::as_str).collect();
        assert!(labels.contains(&"rustify.managed=true"));
        assert!(labels.contains(&"rustify.type=service"));
        assert!(labels.contains(&"rustify.serviceUuid=svc123"));
        assert!(labels.contains(&"traefik.enable=true"));
        assert!(
            labels
                .iter()
                .any(|l| l.contains("Host(`umami.example.com`)"))
        );
        assert!(
            labels
                .iter()
                .any(|l| l.contains("loadbalancer.server.port=3000"))
        );
    }
}
