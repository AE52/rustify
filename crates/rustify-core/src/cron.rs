//! Cron alias resolution and due-time matching, shared by the user
//! scheduled-tasks dispatcher and the system cron set.
//!
//! Behavioural port of Coolify's `VALID_CRON_STRINGS`
//! (bootstrap/helpers/constants.php:24-37) and `shouldRunCronNow`
//! (bootstrap/helpers/shared.php:675-696), reduced to the pure predicate
//! rustify needs: does `at` (evaluated in `tz`) fall on a cron tick?
//!
//! `at` is always truncated to the minute before matching, so a per-minute
//! scheduler tick fires each expression exactly once during the minute it is
//! due, regardless of sub-minute drift.

use chrono::{DateTime, TimeZone, Timelike, Utc};
use croner::Cron;

/// The exact alias table from Coolify (`VALID_CRON_STRINGS`). `@`-prefixed and
/// bare spellings both resolve; anything else is treated as a raw 5-field cron.
pub const VALID_CRON_STRINGS: &[(&str, &str)] = &[
    ("every_minute", "* * * * *"),
    ("hourly", "0 * * * *"),
    ("daily", "0 0 * * *"),
    ("weekly", "0 0 * * 0"),
    ("monthly", "0 0 1 * *"),
    ("yearly", "0 0 1 1 *"),
    ("@hourly", "0 * * * *"),
    ("@daily", "0 0 * * *"),
    ("@weekly", "0 0 * * 0"),
    ("@monthly", "0 0 1 * *"),
    ("@yearly", "0 0 1 1 *"),
];

/// Resolve a frequency alias to a raw cron expression, or return the input
/// unchanged when it is not an alias (`normalizeFrequency`).
pub fn resolve_alias(frequency: &str) -> &str {
    VALID_CRON_STRINGS
        .iter()
        .find_map(|(alias, cron)| (*alias == frequency).then_some(*cron))
        .unwrap_or(frequency)
}

/// Whether `frequency` (alias or raw 5-field cron) is due at `at`, evaluated in
/// timezone `tz`. `at` is truncated to the minute before matching. An
/// unparseable expression is never due.
pub fn is_due<Tz: TimeZone>(frequency: &str, at: DateTime<Utc>, tz: &Tz) -> bool {
    let expr = resolve_alias(frequency);
    let Ok(cron) = Cron::new(expr).parse() else {
        return false;
    };
    // Truncate to the minute: cron granularity is one minute and the dispatcher
    // ticks per minute.
    let at = at
        .with_second(0)
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(at);
    let local = at.with_timezone(tz);
    cron.is_time_matching(&local).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone};

    /// Build a fixed UTC instant from `YYYY-MM-DD HH:MM`.
    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn aliases_resolve_exactly() {
        assert_eq!(resolve_alias("every_minute"), "* * * * *");
        assert_eq!(resolve_alias("hourly"), "0 * * * *");
        assert_eq!(resolve_alias("@hourly"), "0 * * * *");
        assert_eq!(resolve_alias("daily"), "0 0 * * *");
        assert_eq!(resolve_alias("@daily"), "0 0 * * *");
        assert_eq!(resolve_alias("weekly"), "0 0 * * 0");
        assert_eq!(resolve_alias("@weekly"), "0 0 * * 0");
        assert_eq!(resolve_alias("monthly"), "0 0 1 * *");
        assert_eq!(resolve_alias("@monthly"), "0 0 1 * *");
        assert_eq!(resolve_alias("yearly"), "0 0 1 1 *");
        assert_eq!(resolve_alias("@yearly"), "0 0 1 1 *");
        // Unknown strings pass through unchanged.
        assert_eq!(resolve_alias("*/5 * * * *"), "*/5 * * * *");
    }

    #[test]
    fn every_minute_is_always_due() {
        assert!(is_due("every_minute", at(2026, 7, 3, 13, 37), &Utc));
        assert!(is_due("* * * * *", at(2026, 7, 3, 0, 0), &Utc));
    }

    #[test]
    fn hourly_only_on_minute_zero() {
        // 2026-07-03 was a Friday.
        assert!(is_due("hourly", at(2026, 7, 3, 13, 0), &Utc));
        assert!(is_due("@hourly", at(2026, 7, 3, 13, 0), &Utc));
        assert!(!is_due("hourly", at(2026, 7, 3, 13, 1), &Utc));
    }

    #[test]
    fn daily_only_at_midnight() {
        assert!(is_due("daily", at(2026, 7, 3, 0, 0), &Utc));
        assert!(is_due("@daily", at(2026, 7, 3, 0, 0), &Utc));
        assert!(!is_due("daily", at(2026, 7, 3, 1, 0), &Utc));
        assert!(!is_due("daily", at(2026, 7, 3, 0, 1), &Utc));
    }

    #[test]
    fn weekly_only_sunday_midnight() {
        // 2026-07-05 is a Sunday; 2026-07-03 is a Friday.
        assert!(is_due("weekly", at(2026, 7, 5, 0, 0), &Utc));
        assert!(is_due("@weekly", at(2026, 7, 5, 0, 0), &Utc));
        assert!(!is_due("weekly", at(2026, 7, 3, 0, 0), &Utc));
    }

    #[test]
    fn monthly_only_first_of_month() {
        assert!(is_due("monthly", at(2026, 7, 1, 0, 0), &Utc));
        assert!(is_due("@monthly", at(2026, 7, 1, 0, 0), &Utc));
        assert!(!is_due("monthly", at(2026, 7, 2, 0, 0), &Utc));
    }

    #[test]
    fn yearly_only_jan_first() {
        assert!(is_due("yearly", at(2026, 1, 1, 0, 0), &Utc));
        assert!(is_due("@yearly", at(2026, 1, 1, 0, 0), &Utc));
        assert!(!is_due("yearly", at(2026, 2, 1, 0, 0), &Utc));
        assert!(!is_due("yearly", at(2026, 7, 1, 0, 0), &Utc));
    }

    #[test]
    fn seconds_are_ignored_via_minute_truncation() {
        let t = Utc.with_ymd_and_hms(2026, 7, 3, 13, 0, 45).unwrap();
        assert!(is_due("hourly", t, &Utc));
    }

    #[test]
    fn timezone_shifts_the_daily_tick() {
        // 2026-07-03 05:00 UTC is midnight in UTC-5.
        let tz = FixedOffset::west_opt(5 * 3600).unwrap();
        let five_utc = at(2026, 7, 3, 5, 0);
        assert!(is_due("daily", five_utc, &tz));
        // Same instant is not midnight in UTC.
        assert!(!is_due("daily", five_utc, &Utc));
    }

    #[test]
    fn unparseable_expression_is_never_due() {
        assert!(!is_due("not a cron", at(2026, 7, 3, 0, 0), &Utc));
    }
}
