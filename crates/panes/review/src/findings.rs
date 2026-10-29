//! Findings in review: explorer flags and the finding ids pending review
//! drafts carry.

use std::collections::HashSet;

use recur_domain::{Finding, PendingLineComment};

/// Findings grouped by review-file path, in the caller's order (`ReadonlyMap<string, Finding[]>`).
pub type FindingsByPath = Vec<(String, Vec<Finding>)>;

/// A tree row's badge: how many open findings and the worst severity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindingFlag {
    pub count: usize,
    pub severity: String,
}

impl Default for FindingFlag {
    fn default() -> Self {
        FindingFlag {
            count: 0,
            severity: "info".into(),
        }
    }
}

pub fn severity_rank(s: &str) -> u8 {
    match s {
        "critical" => 4,
        "high" => 3,
        "medium" => 2,
        "low" => 1,
        _ => 0,
    }
}

fn severity_of(f: &Finding) -> &str {
    let s = f.severity.as_str();
    if s.is_empty() { "info" } else { s }
}

/// The flag for one file's findings.
pub fn flag_of(findings: &[Finding]) -> FindingFlag {
    let mut severity = "info";
    for f in findings {
        let s = severity_of(f);
        if severity_rank(s) > severity_rank(severity) {
            severity = s;
        }
    }
    FindingFlag {
        count: findings.len(),
        severity: severity.to_string(),
    }
}

/// `findingFlagForPath`: a file's entry, or a directory's children rolled up with the worst
/// severity. Always a flag (count 0 when nothing matches).
pub fn finding_flag_for_path(
    path: &str,
    is_dir: bool,
    by_path: &[(String, FindingFlag)],
) -> FindingFlag {
    if !is_dir {
        return by_path
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, f)| f.clone())
            .unwrap_or_default();
    }
    let prefix = format!("{path}/");
    let mut count = 0;
    let mut severity = "info".to_string();
    for (p, f) in by_path {
        if !p.starts_with(&prefix) {
            continue;
        }
        count += f.count;
        if severity_rank(&f.severity) > severity_rank(&severity) {
            severity = f.severity.clone();
        }
    }
    if count == 0 {
        return FindingFlag::default();
    }
    FindingFlag { count, severity }
}

/// Finding ids persisted on pending drafts (unique, trimmed, in order).
pub fn finding_ids_on_pending(pending: &[PendingLineComment]) -> Vec<String> {
    let mut seen = HashSet::new();
    pending
        .iter()
        .map(|pc| recur_engine::view::js::trim(&pc.finding_id).to_string())
        .filter(|id| !id.is_empty() && seen.insert(id.clone()))
        .collect()
}

/// Drop posted markers for drafts the operator deleted (not for a review submit). `None` when
/// nothing changed.
pub fn drop_finding_ids_removed_from_pending(
    posted: &HashSet<String>,
    prev: &[PendingLineComment],
    next: &[PendingLineComment],
) -> Option<HashSet<String>> {
    let kept: HashSet<String> = finding_ids_on_pending(next).into_iter().collect();
    let mut out: Option<HashSet<String>> = None;
    for id in finding_ids_on_pending(prev) {
        if kept.contains(&id) || !posted.contains(&id) {
            continue;
        }
        out.get_or_insert_with(|| posted.clone()).remove(&id);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flag(severity: &str, count: usize) -> FindingFlag {
        FindingFlag {
            count,
            severity: severity.into(),
        }
    }

    #[test]
    fn file_miss_and_empty_dir_still_return_a_flag() {
        assert_eq!(
            finding_flag_for_path("src/a.ts", false, &[]),
            flag("info", 0)
        );
        assert_eq!(finding_flag_for_path("src", true, &[]), flag("info", 0));
    }

    #[test]
    fn file_hit_is_the_entry_and_a_dir_rolls_up_the_worst_severity() {
        let by = vec![
            ("src/a.ts".to_string(), flag("low", 1)),
            ("src/b.ts".to_string(), flag("critical", 2)),
            ("lib/c.ts".to_string(), flag("high", 1)),
        ];
        assert_eq!(
            finding_flag_for_path("src/a.ts", false, &by),
            flag("low", 1)
        );
        assert_eq!(finding_flag_for_path("src", true, &by), flag("critical", 3));
        assert_eq!(finding_flag_for_path("lib", true, &by), flag("high", 1));
        assert_eq!(finding_flag_for_path("nope", true, &by), flag("info", 0));
    }

    fn pc(finding: &str) -> PendingLineComment {
        PendingLineComment {
            finding_id: finding.into(),
            ..Default::default()
        }
    }

    #[test]
    fn collects_unique_trimmed_finding_ids() {
        assert_eq!(
            finding_ids_on_pending(&[pc("F-1"), pc(" F-1 "), pc("F-2"), pc(""), pc("")]),
            vec!["F-1".to_string(), "F-2".to_string()]
        );
    }

    #[test]
    fn retires_a_deleted_draft_and_keeps_one_that_is_still_pending() {
        let posted: HashSet<String> = ["F-1".to_string(), "F-2".to_string()].into();
        let next =
            drop_finding_ids_removed_from_pending(&posted, &[pc("F-1"), pc("F-2")], &[pc("F-2")])
                .expect("changed");
        assert_eq!(next, ["F-2".to_string()].into());
        let posted: HashSet<String> = ["F-1".to_string()].into();
        assert!(
            drop_finding_ids_removed_from_pending(&posted, &[pc("F-1")], &[pc("F-1")]).is_none()
        );
    }

    #[test]
    fn flag_of_takes_the_worst_severity() {
        let fs = vec![
            Finding {
                severity: "low".into(),
                ..Default::default()
            },
            Finding::default(),
            Finding {
                severity: "high".into(),
                ..Default::default()
            },
        ];
        assert_eq!(flag_of(&fs), flag("high", 3));
    }
}
