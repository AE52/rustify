//! Periodic server + container metrics collection over SSH.
//!
//! Rustify skips Coolify's Sentinel agent (app/Actions/Server/StartSentinel.php
//! plus app/Traits/HasMetrics.php, which push/pull an in-container HTTP API) and
//! instead pulls the same figures with ONE SSH round-trip per server:
//!
//! - host CPU % — two `/proc/stat` samples one second apart (busy delta / total delta),
//! - host memory — `/proc/meminfo` (`MemTotal` / `MemAvailable`), used bytes + used %,
//! - host disk % — `df -P /`,
//! - per-container CPU/mem — `docker stats --no-stream --format '{{json .}}'`.
//!
//! Container `Name`s are mapped back to their `rustify.applicationUuid` via a
//! `docker ps` label read folded into the same round-trip (parity with the
//! label mapping in [`crate::status_sync`]). Host samples store
//! `container_uuid = NULL`; container samples store the resource uuid.
//!
//! Two scheduler closures are exported: [`metrics_collector_task`] (default 10s)
//! and [`metrics_retention_task`] (daily prune per the server's history window).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use chrono::{DateTime, Utc};

use rustify_core::ExecOpts;
use rustify_db::repos::{MetricSample, MetricsRepo, ServerRepo};
use rustify_docker::parse_containers;

use crate::{DeployEngineDeps, DeployError};

type Task = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Marker lines the collection script prints to delimit its sections. Chosen so
/// they never collide with `/proc` content, `df`, or docker JSON output.
const M_STAT1: &str = "#RUSTIFY_STAT1";
const M_MEM: &str = "#RUSTIFY_MEM";
const M_DISK: &str = "#RUSTIFY_DISK";
const M_STAT2: &str = "#RUSTIFY_STAT2";
const M_PS: &str = "#RUSTIFY_PS";
const M_STATS: &str = "#RUSTIFY_STATS";

/// The single remote script. Two `/proc/stat` reads bracket a 1s sleep so a CPU
/// delta can be computed; the docker reads carry labels (`ps`) and live usage
/// (`stats`). `|| true` keeps a missing docker daemon from failing the whole
/// pull — the host rows still land.
fn collect_script() -> String {
    format!(
        "echo {M_STAT1}; grep '^cpu ' /proc/stat; \
         echo {M_MEM}; cat /proc/meminfo; \
         echo {M_DISK}; df -P / | tail -n1; \
         sleep 1; \
         echo {M_STAT2}; grep '^cpu ' /proc/stat; \
         echo {M_PS}; docker ps -a --filter label=rustify.managed=true --format '{{{{json .}}}}' 2>/dev/null || true; \
         echo {M_STATS}; docker stats --no-stream --format '{{{{json .}}}}' 2>/dev/null || true"
    )
}

/// Host-level metrics parsed from one collection round.
#[derive(Debug, Clone, PartialEq)]
pub struct HostMetrics {
    pub cpu_percent: Option<f64>,
    pub mem_percent: Option<f64>,
    pub mem_used_bytes: Option<i64>,
    pub disk_percent: Option<f64>,
}

/// One container's live usage from `docker stats`.
#[derive(Debug, Clone, PartialEq)]
pub struct ContainerStat {
    pub name: String,
    pub cpu_percent: Option<f64>,
    pub mem_percent: Option<f64>,
    pub mem_used_bytes: Option<i64>,
}

// ----- Scheduler closures --------------------------------------------------

/// Build the [`rustify_jobs::Scheduler::every`] closure for the metrics pull.
/// Errors are logged and swallowed so the loop keeps ticking.
pub fn metrics_collector_task(deps: DeployEngineDeps) -> impl Fn() -> Task + Send + 'static {
    move || {
        let deps = deps.clone();
        Box::pin(async move {
            if let Err(e) = collect_all(&deps).await {
                tracing::warn!(error = %e, "metrics collection sweep failed");
            }
        })
    }
}

/// Build the daily retention closure: prune samples past each server's window.
pub fn metrics_retention_task(deps: DeployEngineDeps) -> impl Fn() -> Task + Send + 'static {
    move || {
        let deps = deps.clone();
        Box::pin(async move {
            match MetricsRepo::new(deps.pool.clone()).prune_expired().await {
                Ok(n) if n > 0 => tracing::info!(deleted = n, "pruned expired metrics"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "metrics retention prune failed"),
            }
        })
    }
}

/// One collection sweep across every usable, metrics-enabled server. Servers
/// whose newest host sample is still fresher than their refresh rate are
/// skipped, so a global 10s tick honours a larger per-server refresh interval.
pub async fn collect_all(deps: &DeployEngineDeps) -> Result<(), DeployError> {
    let servers: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, uuid FROM servers WHERE usable = true ORDER BY id")
            .fetch_all(&deps.pool)
            .await?;
    let server_repo = ServerRepo::new(deps.pool.clone());
    let metrics_repo = MetricsRepo::new(deps.pool.clone());

    for (server_id, uuid) in servers {
        let Some(server) = server_repo.get_by_uuid(&uuid).await? else {
            continue;
        };
        let Some(settings) = server_repo.settings(server_id).await? else {
            continue;
        };
        if !settings.metrics_enabled {
            continue;
        }
        let refresh = settings.metrics_refresh_rate_seconds.max(1);
        // Honour a per-server refresh larger than the global tick: skip when the
        // last host sample is still within the refresh window (small slack).
        if let Ok(Some(last)) = metrics_repo.latest_host_ts(server_id).await {
            let elapsed = (Utc::now() - last).num_seconds();
            if elapsed >= 0 && elapsed < i64::from(refresh) - 1 {
                continue;
            }
        }

        let ct = settings.connection_timeout.max(1) as u32;
        let conn = crate::engine::build_conn(&deps.pool, &server, ct).await;
        let Ok(out) = deps
            .executor
            .exec(&conn, &collect_script(), ExecOpts::default())
            .await
        else {
            continue; // transiently unreachable; next sweep retries
        };

        let (host, stats, name_to_uuid) = parse_collection(&out.stdout);

        let _ = metrics_repo
            .insert(&MetricSample {
                server_id,
                container_uuid: None,
                cpu_percent: host.cpu_percent,
                mem_percent: host.mem_percent,
                mem_used_bytes: host.mem_used_bytes,
                disk_percent: host.disk_percent,
            })
            .await;

        for stat in stats {
            let Some(container_uuid) = name_to_uuid.get(&stat.name) else {
                continue; // unmanaged / unmapped container: no per-resource row
            };
            let _ = metrics_repo
                .insert(&MetricSample {
                    server_id,
                    container_uuid: Some(container_uuid.clone()),
                    cpu_percent: stat.cpu_percent,
                    mem_percent: stat.mem_percent,
                    mem_used_bytes: stat.mem_used_bytes,
                    disk_percent: None,
                })
                .await;
        }
    }
    Ok(())
}

// ----- Parsing -------------------------------------------------------------

/// Split the collection stdout into its sections, then parse each. Returns the
/// host metrics, the container stats, and a container-name → resource-uuid map
/// (from the `docker ps` label read).
pub fn parse_collection(
    stdout: &str,
) -> (HostMetrics, Vec<ContainerStat>, HashMap<String, String>) {
    let sections = split_sections(stdout);
    let get = |k: &str| sections.get(k).map(String::as_str).unwrap_or("");

    let cpu_percent = cpu_percent_from_proc(get(M_STAT1), get(M_STAT2));
    let (mem_percent, mem_used_bytes) = match mem_from_meminfo(get(M_MEM)) {
        Some((p, u)) => (Some(p), Some(u)),
        None => (None, None),
    };
    let host = HostMetrics {
        cpu_percent,
        mem_percent,
        mem_used_bytes,
        disk_percent: disk_percent_from_df(get(M_DISK)),
    };

    let stats = parse_docker_stats(get(M_STATS));
    let name_to_uuid = parse_containers(get(M_PS))
        .into_iter()
        .filter_map(|c| c.application_uuid.map(|u| (c.name, u)))
        .collect();

    (host, stats, name_to_uuid)
}

/// Split stdout into a map keyed by the marker lines emitted by [`collect_script`].
fn split_sections(stdout: &str) -> HashMap<String, String> {
    let markers = [M_STAT1, M_MEM, M_DISK, M_STAT2, M_PS, M_STATS];
    let mut out: HashMap<String, String> = HashMap::new();
    let mut current: Option<&str> = None;
    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Some(&m) = markers.iter().find(|&&m| m == trimmed) {
            current = Some(m);
            out.entry(m.to_string()).or_default();
            continue;
        }
        if let Some(m) = current {
            let buf = out.entry(m.to_string()).or_default();
            buf.push_str(line);
            buf.push('\n');
        }
    }
    out
}

/// CPU busy % between two `cpu ...` lines from `/proc/stat`: `(busyΔ / totalΔ) *
/// 100`, where `busy = total - idle - iowait`. Returns `None` if either line is
/// unparseable or no time elapsed.
pub fn cpu_percent_from_proc(prev: &str, cur: &str) -> Option<f64> {
    let (b0, t0) = proc_stat_busy_total(prev)?;
    let (b1, t1) = proc_stat_busy_total(cur)?;
    let total_delta = t1.checked_sub(t0)?;
    let busy_delta = b1.checked_sub(b0)?;
    if total_delta == 0 {
        return None;
    }
    let pct = (busy_delta as f64 / total_delta as f64) * 100.0;
    Some(pct.clamp(0.0, 100.0))
}

/// Parse a `/proc/stat` "cpu ..." aggregate line into `(busy, total)` jiffies.
fn proc_stat_busy_total(line: &str) -> Option<(u64, u64)> {
    let line = line.lines().find(|l| l.trim_start().starts_with("cpu "))?;
    let mut it = line.split_whitespace();
    if it.next()? != "cpu" {
        return None;
    }
    // user nice system idle iowait irq softirq steal guest guest_nice
    let vals: Vec<u64> = it.filter_map(|v| v.parse::<u64>().ok()).collect();
    if vals.len() < 4 {
        return None;
    }
    let total: u64 = vals.iter().sum();
    let idle = vals[3];
    let iowait = vals.get(4).copied().unwrap_or(0);
    let busy = total.saturating_sub(idle).saturating_sub(iowait);
    Some((busy, total))
}

/// Parse `/proc/meminfo` into `(used_percent, used_bytes)`. `used = MemTotal -
/// MemAvailable` (falling back to `MemFree` when `MemAvailable` is absent).
/// meminfo values are in kB.
pub fn mem_from_meminfo(s: &str) -> Option<(f64, i64)> {
    let mut total_kb: Option<u64> = None;
    let mut available_kb: Option<u64> = None;
    let mut free_kb: Option<u64> = None;
    for line in s.lines() {
        let (key, rest) = match line.split_once(':') {
            Some(kv) => kv,
            None => continue,
        };
        let value = rest.split_whitespace().next().and_then(|v| v.parse().ok());
        match key.trim() {
            "MemTotal" => total_kb = value,
            "MemAvailable" => available_kb = value,
            "MemFree" => free_kb = value,
            _ => {}
        }
    }
    let total = total_kb?;
    if total == 0 {
        return None;
    }
    let available = available_kb.or(free_kb)?;
    let used_kb = total.saturating_sub(available);
    let percent = (used_kb as f64 / total as f64) * 100.0;
    Some((percent, (used_kb as i64) * 1024))
}

/// Parse the used percentage from one `df -P /` data line (5th column, e.g. `42%`).
pub fn disk_percent_from_df(s: &str) -> Option<f64> {
    let line = s.lines().find(|l| !l.trim().is_empty())?;
    let field = line.split_whitespace().nth(4)?;
    let digits: String = field.chars().filter(|c| c.is_ascii_digit()).collect();
    digits.parse::<f64>().ok()
}

/// Parse `docker stats --no-stream --format '{{json .}}'` (JSONL). Percentages
/// strip the trailing `%`; `MemUsage` ("10.5MiB / 1.9GiB") yields used bytes.
pub fn parse_docker_stats(s: &str) -> Vec<ContainerStat> {
    #[derive(serde::Deserialize)]
    struct StatLine {
        #[serde(rename = "Name", default)]
        name: String,
        #[serde(rename = "CPUPerc", default)]
        cpu_perc: String,
        #[serde(rename = "MemPerc", default)]
        mem_perc: String,
        #[serde(rename = "MemUsage", default)]
        mem_usage: String,
    }
    s.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter_map(|l| serde_json::from_str::<StatLine>(l).ok())
        .filter(|l| !l.name.is_empty())
        .map(|l| ContainerStat {
            name: l.name,
            cpu_percent: parse_percent(&l.cpu_perc),
            mem_percent: parse_percent(&l.mem_perc),
            mem_used_bytes: parse_mem_usage_bytes(&l.mem_usage),
        })
        .collect()
}

/// Parse a `"1.23%"`-style docker percentage into a float.
fn parse_percent(s: &str) -> Option<f64> {
    s.trim().trim_end_matches('%').trim().parse::<f64>().ok()
}

/// Parse the "used" side of a docker `MemUsage` string ("10.5MiB / 1.9GiB")
/// into bytes. Handles B / KiB / MiB / GiB / TiB (and their `*B` synonyms),
/// all treated as 1024-based per docker's binary reporting.
pub fn parse_mem_usage_bytes(s: &str) -> Option<i64> {
    let used = s.split('/').next()?.trim();
    parse_size_bytes(used)
}

/// Parse a size token like "10.5MiB" / "512kB" / "1GiB" / "900B" into bytes.
fn parse_size_bytes(token: &str) -> Option<i64> {
    let token = token.trim();
    let split = token
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(token.len());
    let (num, unit) = token.split_at(split);
    let value: f64 = num.trim().parse().ok()?;
    let mult: f64 = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "kib" | "kb" | "k" => 1024.0,
        "mib" | "mb" | "m" => 1024.0 * 1024.0,
        "gib" | "gb" | "g" => 1024.0 * 1024.0 * 1024.0,
        "tib" | "tb" | "t" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((value * mult) as i64)
}

// ----- Staleness -----------------------------------------------------------

/// Whether a server's metrics are stale: no sample newer than `refresh * 3`
/// seconds (floored at 120s, matching the brief). `None` (never collected) is
/// always stale.
pub fn is_stale(last: Option<DateTime<Utc>>, refresh_secs: i32, now: DateTime<Utc>) -> bool {
    let threshold = i64::from(refresh_secs.max(1)).saturating_mul(3).max(120);
    match last {
        None => true,
        Some(ts) => (now - ts).num_seconds() > threshold,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn cpu_delta_from_two_proc_stat_samples() {
        // busy0=200 total0=900; busy1=900 total1=1900 → 700/1000 = 70%.
        let prev = "cpu  100 0 100 700 0 0 0 0 0 0";
        let cur = "cpu  400 0 500 900 100 0 0 0 0 0";
        let pct = cpu_percent_from_proc(prev, cur).unwrap();
        assert!((pct - 70.0).abs() < 0.001, "got {pct}");
    }

    #[test]
    fn cpu_none_when_no_time_elapsed() {
        let same = "cpu  100 0 100 700 0 0 0 0 0 0";
        assert_eq!(cpu_percent_from_proc(same, same), None);
    }

    #[test]
    fn cpu_none_on_garbage() {
        assert_eq!(cpu_percent_from_proc("garbage", "also garbage"), None);
    }

    #[test]
    fn meminfo_used_percent_and_bytes() {
        let s = "MemTotal:       1000 kB\nMemFree:         100 kB\nMemAvailable:    250 kB\nBuffers: 10 kB\n";
        let (pct, bytes) = mem_from_meminfo(s).unwrap();
        // used = 1000 - 250 = 750 kB → 75%, 750*1024 bytes.
        assert!((pct - 75.0).abs() < 0.001, "got {pct}");
        assert_eq!(bytes, 750 * 1024);
    }

    #[test]
    fn meminfo_falls_back_to_memfree() {
        let s = "MemTotal: 2000 kB\nMemFree: 500 kB\n";
        let (pct, bytes) = mem_from_meminfo(s).unwrap();
        assert!((pct - 75.0).abs() < 0.001);
        assert_eq!(bytes, 1500 * 1024);
    }

    #[test]
    fn disk_percent_from_df_line() {
        let s = "/dev/sda1  100000  42000  58000  42% /";
        assert_eq!(disk_percent_from_df(s), Some(42.0));
    }

    #[test]
    fn docker_stats_json_parses_cpu_mem() {
        let json = r#"{"BlockIO":"0B / 0B","CPUPerc":"1.50%","Container":"abc","ID":"abc","MemPerc":"4.25%","MemUsage":"10.5MiB / 1.945GiB","Name":"app-uuid-xyz","NetIO":"1kB / 2kB","PIDs":"5"}"#;
        let stats = parse_docker_stats(json);
        assert_eq!(stats.len(), 1);
        let s = &stats[0];
        assert_eq!(s.name, "app-uuid-xyz");
        assert_eq!(s.cpu_percent, Some(1.5));
        assert_eq!(s.mem_percent, Some(4.25));
        assert_eq!(s.mem_used_bytes, Some((10.5 * 1024.0 * 1024.0) as i64));
    }

    #[test]
    fn mem_usage_bytes_units() {
        assert_eq!(parse_mem_usage_bytes("900B / 1GiB"), Some(900));
        assert_eq!(parse_mem_usage_bytes("2KiB / 1GiB"), Some(2048));
        assert_eq!(
            parse_mem_usage_bytes("1GiB / 4GiB"),
            Some(1024 * 1024 * 1024)
        );
    }

    #[test]
    fn full_collection_maps_container_names_to_uuid() {
        let stdout = format!(
            "{M_STAT1}\ncpu  100 0 100 700 0 0 0 0 0 0\n\
             {M_MEM}\nMemTotal: 1000 kB\nMemAvailable: 250 kB\n\
             {M_DISK}\n/dev/sda1 100 42 58 42% /\n\
             {M_STAT2}\ncpu  400 0 500 900 100 0 0 0 0 0\n\
             {M_PS}\n{{\"ID\":\"abc\",\"Names\":\"app-uuid-xyz\",\"Image\":\"img\",\"State\":\"running\",\"Labels\":\"rustify.managed=true,rustify.applicationUuid=app-uuid\"}}\n\
             {M_STATS}\n{{\"Name\":\"app-uuid-xyz\",\"CPUPerc\":\"1.50%\",\"MemPerc\":\"4.25%\",\"MemUsage\":\"10.5MiB / 1.9GiB\"}}\n"
        );
        let (host, stats, map) = parse_collection(&stdout);
        assert!((host.cpu_percent.unwrap() - 70.0).abs() < 0.001);
        assert!((host.mem_percent.unwrap() - 75.0).abs() < 0.001);
        assert_eq!(host.disk_percent, Some(42.0));
        assert_eq!(stats.len(), 1);
        assert_eq!(
            map.get("app-uuid-xyz").map(String::as_str),
            Some("app-uuid")
        );
    }

    #[test]
    fn stale_when_never_collected() {
        assert!(is_stale(None, 10, Utc::now()));
    }

    #[test]
    fn stale_uses_120s_floor_for_small_refresh() {
        let now = Utc::now();
        // refresh*3 = 30s but floor is 120s: 100s ago is still fresh.
        assert!(!is_stale(Some(now - Duration::seconds(100)), 10, now));
        assert!(is_stale(Some(now - Duration::seconds(121)), 10, now));
    }

    #[test]
    fn stale_uses_refresh_times_three_when_above_floor() {
        let now = Utc::now();
        // refresh 60 → 180s threshold.
        assert!(!is_stale(Some(now - Duration::seconds(170)), 60, now));
        assert!(is_stale(Some(now - Duration::seconds(181)), 60, now));
    }
}
