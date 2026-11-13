//! Check rollups: buckets, reconciliation of list rows with fetched runs,
//! and duration labels.

use std::collections::{HashMap, HashSet};

use chrono::TimeDelta;
use recur_domain::Check;

use super::text::eq_fold;

/// Check status buckets stored on `Check.status`.
pub const BUCKET_PASS: &str = "pass";
pub const BUCKET_FAIL: &str = "fail";
pub const BUCKET_PENDING: &str = "pending";
pub const BUCKET_SKIPPING: &str = "skipping";
pub const BUCKET_CANCEL: &str = "cancel";

/// True when nothing failed: pending and skipped rows do not count.
pub fn all_checks_passed(checks: &[Check]) -> bool {
    checks
        .iter()
        .all(|c| c.status == BUCKET_PENDING || c.status == BUCKET_SKIPPING || c.passed)
}

/// none, fail, pending or pass for a set of rows, ignoring skipped ones.
pub fn check_rows_kind(checks: &[Check]) -> &'static str {
    let (mut fail, mut pending, mut n) = (false, false, 0);
    for c in checks {
        if eq_fold(&c.status, BUCKET_SKIPPING) {
            continue;
        }
        n += 1;
        if eq_fold(&c.status, BUCKET_PENDING) {
            pending = true;
            continue;
        }
        if !c.passed {
            fail = true;
        }
    }
    if n == 0 {
        "none"
    } else if fail {
        "fail"
    } else if pending {
        "pending"
    } else {
        "pass"
    }
}

/// Merges a list call's rollup into stored rows. A stored row whose kind
/// still matches keeps its richer detail (duration, link). Stored pending
/// rows the list no longer names are kept, and `refresh` asks for a full
/// checks fetch to settle them.
pub fn reconcile_listed_checks(existing: &[Check], listed: &[Check]) -> (Vec<Check>, bool) {
    if listed.is_empty() {
        return (existing.to_vec(), false);
    }
    let mut by_name: HashMap<String, &Check> = HashMap::new();
    for c in existing {
        let name = c.name.trim();
        if !name.is_empty() {
            by_name.insert(name.to_lowercase(), c);
        }
    }
    let mut out = Vec::with_capacity(listed.len() + existing.len());
    let mut seen = HashSet::new();
    for c in listed {
        let key = c.name.trim().to_lowercase();
        let keep = by_name.get(&key).filter(|ex| {
            check_rows_kind(std::slice::from_ref(*ex)) == check_rows_kind(std::slice::from_ref(c))
        });
        out.push(keep.map_or_else(|| c.clone(), |ex| (*ex).clone()));
        seen.insert(key);
    }
    let mut refresh = false;
    for c in existing {
        if seen.contains(&c.name.trim().to_lowercase()) {
            continue;
        }
        if eq_fold(&c.status, BUCKET_PENDING) {
            out.push(c.clone());
            refresh = true;
        }
    }
    (out, refresh)
}

/// Duration label for a check: 45s, 1m 4s, 12m, 1h, 1h 2m.
pub fn format_check_duration(d: TimeDelta) -> String {
    let d = d.max(TimeDelta::zero());
    let secs = match d.num_nanoseconds() {
        Some(ns) => (ns + 500_000_000) / 1_000_000_000,
        None => d.num_seconds(),
    };
    format_check_seconds(secs)
}

/// [`format_check_duration`] for whole seconds.
pub fn format_check_seconds(sec: i64) -> String {
    let sec = sec.max(0);
    if sec < 60 {
        return format!("{sec}s");
    }
    let mut m = sec / 60;
    let s = sec % 60;
    if m >= 60 {
        let h = m / 60;
        m %= 60;
        if m == 0 {
            return format!("{h}h");
        }
        return format!("{h}h {m}m");
    }
    if s == 0 {
        return format!("{m}m");
    }
    format!("{m}m {s}s")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(name: &str, status: &str, passed: bool, detail: &str) -> Check {
        Check {
            name: name.into(),
            status: status.into(),
            passed,
            detail: detail.into(),
            ..Default::default()
        }
    }

    #[test]
    fn format_check_duration_labels() {
        for (secs, want) in [
            (0, "0s"),
            (1, "1s"),
            (45, "45s"),
            (60, "1m"),
            (64, "1m 4s"),
            (12 * 60, "12m"),
            (3600, "1h"),
            (3600 + 120, "1h 2m"),
        ] {
            assert_eq!(format_check_duration(TimeDelta::seconds(secs)), want);
        }
        assert_eq!(format_check_duration(TimeDelta::milliseconds(1500)), "2s");
        assert_eq!(format_check_duration(TimeDelta::seconds(-5)), "0s");
    }

    #[test]
    fn rows_kind_skips_skipped() {
        assert_eq!(check_rows_kind(&[]), "none");
        assert_eq!(
            check_rows_kind(&[check("a", "skipping", false, "")]),
            "none"
        );
        assert_eq!(
            check_rows_kind(&[check("a", "PENDING", false, "")]),
            "pending"
        );
        assert_eq!(
            check_rows_kind(&[
                check("a", "pending", false, ""),
                check("b", "fail", false, "")
            ]),
            "fail"
        );
        assert_eq!(check_rows_kind(&[check("a", "pass", true, "")]), "pass");
    }

    #[test]
    fn reconcile_keeps_detail_and_stale_pending() {
        let existing = [
            check("build", "pass", true, "1m 4s"),
            check("lint", "pending", false, "10s"),
            check("old", "pass", true, "3s"),
        ];
        let listed = [
            check("Build", "pass", true, ""),
            check("test", "fail", false, ""),
        ];
        let (out, refresh) = reconcile_listed_checks(&existing, &listed);
        assert_eq!(
            out,
            vec![
                check("build", "pass", true, "1m 4s"),
                check("test", "fail", false, ""),
                check("lint", "pending", false, "10s"),
            ]
        );
        assert!(refresh);

        let (out, refresh) =
            reconcile_listed_checks(&existing, &[check("build", "fail", false, "")]);
        assert_eq!(out[0], check("build", "fail", false, ""));
        assert!(refresh);

        let (out, refresh) = reconcile_listed_checks(&existing, &[]);
        assert_eq!(out, existing.to_vec());
        assert!(!refresh);
    }

    #[test]
    fn all_passed_ignores_pending_and_skipped() {
        assert!(all_checks_passed(&[
            check("a", "pending", false, ""),
            check("b", "skipping", false, ""),
            check("c", "", true, ""),
        ]));
        assert!(!all_checks_passed(&[check("a", "cancel", false, "")]));
        assert!(!all_checks_passed(&[check(
            "Independent review",
            "",
            false,
            ""
        )]));
    }
}
