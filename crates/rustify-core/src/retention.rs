//! Backup retention selection (Contract: three independent retention rules).
//!
//! Behavioural port of Coolify's `deleteOldBackupsLocally` /
//! `deleteOldBackupsFromS3` (bootstrap/helpers/databases.php:282-437). Given the
//! set of *successful* backup executions for a schedule, decide which ones to
//! delete under three independent rules whose selections are unioned:
//!
//! 1. **amount** — keep the newest `amount`, delete the rest (`skip($amount)`).
//! 2. **days** — delete anything older than `newest.created_at - days`
//!    (databases.php:314-316; the cutoff is anchored to the newest backup, not
//!    wall-clock `now`).
//! 3. **max_gb** — walking from the second-newest, accumulate sizes; the first
//!    time the running total exceeds `max_gb * 1024³`, delete every backup
//!    strictly older than that one (databases.php:319-337).
//!
//! The newest backup is always protected, and an all-zero configuration keeps
//! everything.

use std::collections::HashSet;

use chrono::{DateTime, Duration, Utc};

/// The minimal execution metadata the retention rules need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecMeta {
    pub id: i64,
    pub created_at: DateTime<Utc>,
    /// Backup size in bytes.
    pub size: i64,
}

const BYTES_PER_GB: u64 = 1024 * 1024 * 1024;

/// Select the execution ids to delete under the three retention rules, unioned.
///
/// `execs` need not be sorted. The result preserves newest-first order and never
/// contains the newest execution. `now` is accepted for signature symmetry with
/// time-based callers; per Coolify the `days` rule is anchored to the newest
/// backup's timestamp, so `now` is not consulted.
pub fn select_for_deletion(
    execs: &[ExecMeta],
    amount: u32,
    days: u32,
    max_gb: u32,
    now: DateTime<Utc>,
) -> Vec<i64> {
    let _ = now;
    if execs.is_empty() || (amount == 0 && days == 0 && max_gb == 0) {
        return Vec::new();
    }

    // Newest-first, ties broken by id so ordering is deterministic.
    let mut sorted: Vec<&ExecMeta> = execs.iter().collect();
    sorted.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));

    let newest = sorted[0];
    let mut marked: HashSet<i64> = HashSet::new();

    // Rule 1: keep the newest `amount`, delete the rest.
    if amount > 0 {
        for e in sorted.iter().skip(amount as usize) {
            marked.insert(e.id);
        }
    }

    // Rule 2: delete anything older than `newest - days`.
    if days > 0 {
        let cutoff = newest.created_at - Duration::days(days as i64);
        for e in &sorted {
            if e.created_at < cutoff {
                marked.insert(e.id);
            }
        }
    }

    // Rule 3: accumulate from the second-newest; once the total passes the cap,
    // delete everything strictly older than the crossing execution.
    if max_gb > 0 {
        let max_bytes = max_gb as u64 * BYTES_PER_GB;
        let mut total: u64 = 0;
        for i in 1..sorted.len() {
            total += sorted[i].size.max(0) as u64;
            if total > max_bytes {
                let threshold = sorted[i].created_at;
                // Everything with created_at <= threshold, minus the first of
                // that group (the crossing execution itself is kept).
                for e in sorted.iter().filter(|e| e.created_at <= threshold).skip(1) {
                    marked.insert(e.id);
                }
                break;
            }
        }
    }

    // Union, newest-first order, newest always protected.
    sorted
        .iter()
        .filter(|e| e.id != newest.id && marked.contains(&e.id))
        .map(|e| e.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(days_ago: i64) -> DateTime<Utc> {
        // Fixed reference so ordering is stable; newest = day 0.
        DateTime::parse_from_rfc3339("2026-01-31T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
            - Duration::days(days_ago)
    }

    /// Ten backups, newest (id 0) at day 0 down to oldest (id 9) at day 9,
    /// each 1 GiB.
    fn sample() -> Vec<ExecMeta> {
        (0..10)
            .map(|i| ExecMeta {
                id: i,
                created_at: ts(i),
                size: BYTES_PER_GB as i64,
            })
            .collect()
    }

    fn now() -> DateTime<Utc> {
        ts(0)
    }

    #[test]
    fn all_zero_keeps_everything() {
        assert!(select_for_deletion(&sample(), 0, 0, 0, now()).is_empty());
    }

    #[test]
    fn empty_input_is_empty() {
        assert!(select_for_deletion(&[], 5, 5, 5, now()).is_empty());
    }

    #[test]
    fn amount_keeps_newest_n() {
        // Keep newest 3 (ids 0,1,2); delete 3..9.
        let del = select_for_deletion(&sample(), 3, 0, 0, now());
        assert_eq!(del, vec![3, 4, 5, 6, 7, 8, 9]);
    }

    #[test]
    fn amount_one_protects_newest_only() {
        let del = select_for_deletion(&sample(), 1, 0, 0, now());
        assert_eq!(del, vec![1, 2, 3, 4, 5, 6, 7, 8, 9]);
        assert!(!del.contains(&0), "newest always protected");
    }

    #[test]
    fn days_cutoff_relative_to_newest() {
        // newest at day 0; cutoff = day -5. Delete created_at < (day0 - 5d),
        // i.e. strictly older than day 5 -> ids 6..9.
        let del = select_for_deletion(&sample(), 0, 5, 0, now());
        assert_eq!(del, vec![6, 7, 8, 9]);
    }

    #[test]
    fn max_gb_deletes_past_accumulation() {
        // 1 GiB each. cap 3 GiB. Accumulate from id1: after id1=1,id2=2,id3=3
        // (not > 3), id4=4 (> 3) crosses at id4 (day 4). Delete everything
        // strictly older than day 4 -> ids 5..9.
        let del = select_for_deletion(&sample(), 0, 0, 3, now());
        assert_eq!(del, vec![5, 6, 7, 8, 9]);
    }

    #[test]
    fn max_gb_zero_when_never_exceeded() {
        // cap huge -> nothing selected by this rule.
        let del = select_for_deletion(&sample(), 0, 0, 100, now());
        assert!(del.is_empty());
    }

    #[test]
    fn union_of_rules() {
        // amount=8 -> delete ids 8,9. days=5 -> delete 6,7,8,9. Union = 6,7,8,9.
        let del = select_for_deletion(&sample(), 8, 5, 0, now());
        assert_eq!(del, vec![6, 7, 8, 9]);
    }

    #[test]
    fn unsorted_input_is_handled() {
        let mut execs = sample();
        execs.reverse();
        let del = select_for_deletion(&execs, 3, 0, 0, now());
        assert_eq!(del, vec![3, 4, 5, 6, 7, 8, 9]);
    }

    #[test]
    fn newest_never_deleted_even_with_aggressive_rules() {
        let del = select_for_deletion(&sample(), 0, 0, 0, now());
        assert!(del.is_empty());
        // Even with tiny cap, id 0 (newest) is protected.
        let del = select_for_deletion(&sample(), 0, 0, 1, now());
        assert!(!del.contains(&0));
    }
}
