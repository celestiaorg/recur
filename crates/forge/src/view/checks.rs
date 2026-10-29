//! Check rows for the PR views.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use recur_domain::Check;

use crate::model::{format_check_seconds, parse_time};

const PENDING_STATUSES: [&str; 6] = [
    "pending",
    "queued",
    "in_progress",
    "waiting",
    "requested",
    "expected",
];

/// Skipped by status, or by a "skipped" word in the detail.
pub fn is_skipped_check(status: &str, detail: &str) -> bool {
    let status = status.to_lowercase();
    if matches!(status.as_str(), "skipping" | "skipped" | "skip") {
        return true;
    }
    let lower = detail.to_ascii_lowercase();
    let b = lower.as_bytes();
    let word = |i: usize| {
        b.get(i)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
    };
    lower
        .match_indices("skipped")
        .any(|(i, _)| (i == 0 || !word(i - 1)) && !word(i + "skipped".len()))
}

/// Queued, running or expected (spaces and dashes read as underscores).
pub fn is_pending_check(status: &str) -> bool {
    let mut s = String::new();
    let mut gap = false;
    for c in status.to_lowercase().chars() {
        if c.is_whitespace() || c == '-' {
            if !gap {
                s.push('_');
            }
            gap = true;
        } else {
            s.push(c);
            gap = false;
        }
    }
    PENDING_STATUSES.contains(&s.as_str())
}

/// Checks split into the four Checks-tab groups, in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckGroups {
    pub fail: Vec<Check>,
    pub pending: Vec<Check>,
    pub pass: Vec<Check>,
    pub skip: Vec<Check>,
}

pub fn group_checks(checks: &[Check]) -> CheckGroups {
    let mut g = CheckGroups::default();
    for c in checks {
        if is_skipped_check(&c.status, &c.detail) {
            g.skip.push(c.clone());
        } else if is_pending_check(&c.status) {
            g.pending.push(c.clone());
        } else if c.passed {
            g.pass.push(c.clone());
        } else {
            g.fail.push(c.clone());
        }
    }
    g
}

/// Worst-wins rollup. The UI draws `Fail` as x/coral, `Pending` as a
/// loader/amber, `Pass` as a check/emerald and `None` as a faint check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollupKind {
    None,
    Fail,
    Pending,
    Pass,
}

/// Failed, then in progress, then passed. With no rows, the list's
/// `checks_ok` / `has_checks` bits decide.
pub fn checks_rollup(
    checks: &[Check],
    checks_ok: Option<bool>,
    has_checks: Option<bool>,
) -> RollupKind {
    if !checks.is_empty() {
        let failed = checks.iter().any(|c| {
            !c.passed && !is_skipped_check(&c.status, &c.detail) && !is_pending_check(&c.status)
        });
        if failed {
            return RollupKind::Fail;
        }
        if checks.iter().any(|c| is_pending_check(&c.status)) {
            return RollupKind::Pending;
        }
        return RollupKind::Pass;
    }
    if checks_ok == Some(false) {
        return RollupKind::Fail;
    }
    if has_checks == Some(true) {
        return RollupKind::Pass;
    }
    RollupKind::None
}

/// Checks box heading. `forge_label` names the forge for an empty forge PR.
pub fn checks_heading(checks: &[Check], forge_label: Option<&str>) -> String {
    if checks.is_empty() {
        return match forge_label {
            Some(label) => format!("No {label} checks yet"),
            None => "No checks".to_owned(),
        };
    }
    match checks_rollup(checks, None, None) {
        RollupKind::Fail => "Some checks failed",
        RollupKind::Pending => "Checks in progress",
        _ => "All checks passed",
    }
    .to_owned()
}

/// Whether a group starts collapsed: an explicit toggle wins; the compact
/// conversation card hides passing and skipped groups.
pub fn check_group_collapsed(key: &str, overrides: &HashMap<String, bool>, compact: bool) -> bool {
    if let Some(v) = overrides.get(key) {
        return *v;
    }
    compact && (key == "pass" || key == "skip")
}

/// `1 failed, 2 in progress, 3 successful, 1 skipped`.
pub fn checks_box_byline(g: &CheckGroups) -> String {
    [
        (g.fail.len(), "failed"),
        (g.pending.len(), "in progress"),
        (g.pass.len(), "successful"),
        (g.skip.len(), "skipped"),
    ]
    .into_iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, w)| format!("{n} {w}"))
    .collect::<Vec<_>>()
    .join(", ")
}

/// Interpolation origin for a pending check that has no start time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingClock {
    pub detail: String,
    pub base_sec: i64,
    pub origin: DateTime<Utc>,
}

pub fn check_clock_key(c: &Check) -> String {
    format!("{}\0{}", c.name, c.url)
}

/// Drops clocks for checks that are no longer pending.
pub fn prune_pending_clocks(clocks: &mut HashMap<String, PendingClock>, checks: &[Check]) {
    let keep: HashSet<String> = checks
        .iter()
        .filter(|c| is_pending_check(&c.status))
        .map(check_clock_key)
        .collect();
    clocks.retain(|k, _| keep.contains(k));
}

/// Seconds in a `45s` / `1m 4s` / `12m` / `1h` / `1h 2m` label.
pub fn parse_check_duration(detail: &str) -> Option<i64> {
    static RE: std::sync::LazyLock<[regex::Regex; 3]> = std::sync::LazyLock::new(|| {
        [
            r"^([0-9]+)h(?:\s+([0-9]+)m)?$",
            r"^([0-9]+)m(?:\s+([0-9]+)s)?$",
            r"^([0-9]+)s$",
        ]
        .map(|p| regex::Regex::new(p).expect("duration pattern"))
    });
    let s = detail.trim();
    let num = |m: Option<regex::Match>| m.map_or(Some(0), |m| m.as_str().parse::<i64>().ok());
    for (re, unit) in RE.iter().zip([3600, 60, 1]) {
        if let Some(c) = re.captures(s) {
            return Some(num(c.get(1))? * unit + num(c.get(2))? * (unit / 60));
        }
    }
    None
}

/// Live duration label. Pending rows tick from `started_at`, or from the
/// last snapshot's detail until a refresh replaces it; completed rows show
/// their final detail and drop any clock.
pub fn live_check_detail(
    c: &Check,
    now: DateTime<Utc>,
    clocks: &mut HashMap<String, PendingClock>,
) -> String {
    let key = check_clock_key(c);
    if !is_pending_check(&c.status) {
        clocks.remove(&key);
        return c.detail.clone();
    }
    if let Some(start) = parse_time(&c.started_at) {
        clocks.remove(&key);
        return format_check_seconds((now - start).num_seconds().max(0));
    }
    let Some(parsed) = parse_check_duration(&c.detail) else {
        return c.detail.clone();
    };
    let clock = clocks.entry(key).or_insert_with(|| PendingClock {
        detail: c.detail.clone(),
        base_sec: parsed,
        origin: now,
    });
    if clock.detail != c.detail {
        *clock = PendingClock {
            detail: c.detail.clone(),
            base_sec: parsed,
            origin: now,
        };
    }
    format_check_seconds(clock.base_sec + (now - clock.origin).num_seconds().max(0))
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
    fn pending_statuses() {
        for s in [
            "pending",
            "QUEUED",
            "in_progress",
            "EXPECTED",
            "in progress",
            "in-progress",
        ] {
            assert!(is_pending_check(s), "{s}");
        }
        for s in ["stale", "pass", ""] {
            assert!(!is_pending_check(s), "{s}");
        }
    }

    #[test]
    fn groups_keep_pending_out_of_failed() {
        let g = group_checks(&[
            check("test", "pass", true, ""),
            check("build", "pending", false, ""),
            check("license", "expected", false, ""),
            check("lint", "fail", false, ""),
            check("deploy", "skipping", false, "skipped"),
        ]);
        let names = |v: &[Check]| v.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&g.pending), ["build", "license"]);
        assert_eq!(names(&g.fail), ["lint"]);
        assert_eq!(names(&g.pass), ["test"]);
        assert_eq!(names(&g.skip), ["deploy"]);
        assert!(is_skipped_check("", "Job was skipped."));
        assert!(!is_skipped_check("", "unskipped"));
        assert_eq!(
            checks_box_byline(&group_checks(&[
                check("a", "pending", false, ""),
                check("b", "pass", true, "")
            ])),
            "1 in progress, 1 successful"
        );
    }

    #[test]
    fn collapsed_groups() {
        let none = HashMap::new();
        assert!(check_group_collapsed("pass", &none, true));
        assert!(check_group_collapsed("skip", &none, true));
        assert!(!check_group_collapsed("fail", &none, true));
        assert!(!check_group_collapsed("pending", &none, true));
        assert!(!check_group_collapsed("pass", &none, false));
        assert!(check_group_collapsed(
            "pass",
            &HashMap::from([("pass".to_owned(), true)]),
            false
        ));
        assert!(!check_group_collapsed(
            "pass",
            &HashMap::from([("pass".to_owned(), false)]),
            true
        ));
    }

    #[test]
    fn heading_and_rollup() {
        assert_eq!(
            checks_heading(&[check("a", "pending", false, "")], Some("GitHub")),
            "Checks in progress"
        );
        assert_eq!(checks_heading(&[], Some("GitHub")), "No GitHub checks yet");
        assert_eq!(checks_heading(&[], None), "No checks");
        assert_eq!(
            checks_heading(
                &[
                    check("a", "fail", false, ""),
                    check("b", "pending", false, "")
                ],
                None
            ),
            "Some checks failed"
        );
        assert_eq!(
            checks_rollup(&[check("a", "fail", false, "")], None, None),
            RollupKind::Fail
        );
        assert_eq!(
            checks_rollup(&[check("a", "pending", false, "")], None, None),
            RollupKind::Pending
        );
        assert_eq!(
            checks_rollup(&[check("a", "pass", true, "")], None, None),
            RollupKind::Pass
        );
        assert_eq!(checks_rollup(&[], Some(false), None), RollupKind::Fail);
        assert_eq!(checks_rollup(&[], Some(true), Some(true)), RollupKind::Pass);
        assert_eq!(checks_rollup(&[], None, None), RollupKind::None);
    }

    #[test]
    fn durations_parse() {
        for (s, want) in [
            ("45s", 45),
            ("1m", 60),
            ("1m 4s", 64),
            ("12m", 720),
            ("1h", 3600),
            ("1h 2m", 3720),
        ] {
            assert_eq!(parse_check_duration(s), Some(want), "{s}");
        }
        for s in ["pending", "skipped", ""] {
            assert_eq!(parse_check_duration(s), None, "{s}");
        }
    }

    #[test]
    fn live_detail_ticks_and_settles() {
        let t0 = parse_time("2026-08-21T12:00:00Z").unwrap();
        let secs = |n| t0 + chrono::TimeDelta::seconds(n);
        let mut clocks = HashMap::new();
        let mut c = check("build", "pending", false, "45s");
        c.started_at = "2026-08-21T12:00:00Z".into();
        assert_eq!(live_check_detail(&c, secs(45), &mut clocks), "45s");
        assert_eq!(live_check_detail(&c, secs(64), &mut clocks), "1m 4s");

        let mut clocks = HashMap::new();
        let key = check_clock_key(&check("build", "", false, ""));
        assert_eq!(
            live_check_detail(&check("build", "pending", false, "45s"), t0, &mut clocks),
            "45s"
        );
        assert_eq!(
            live_check_detail(
                &check("build", "pending", false, "45s"),
                secs(5),
                &mut clocks
            ),
            "50s"
        );
        assert_eq!(clocks[&key].detail, "45s");
        assert_eq!(
            live_check_detail(
                &check("build", "pending", false, "50s"),
                secs(5),
                &mut clocks
            ),
            "50s"
        );
        assert_eq!(
            live_check_detail(
                &check("build", "pending", false, "50s"),
                secs(8),
                &mut clocks
            ),
            "53s"
        );
        assert_eq!(
            live_check_detail(
                &check("build", "pass", true, "1m 4s"),
                secs(20),
                &mut clocks
            ),
            "1m 4s"
        );
        assert!(clocks.is_empty());

        live_check_detail(&check("build", "pending", false, "10s"), t0, &mut clocks);
        live_check_detail(&check("lint", "pending", false, "4s"), t0, &mut clocks);
        prune_pending_clocks(&mut clocks, &[check("lint", "pending", false, "4s")]);
        assert_eq!(
            clocks.keys().cloned().collect::<Vec<_>>(),
            [check_clock_key(&check("lint", "", false, ""))]
        );
    }
}
