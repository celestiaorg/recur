//! "Needs your attention" codes and labels for PR rows (`lib/pr-attention.ts`).

use recur_domain::{
    ATTENTION_ASSIGNED, ATTENTION_AWAITING_REVIEW, ATTENTION_CHANGES_REQUESTED,
    ATTENTION_CHECKS_FAILING, ATTENTION_REVIEW_REQUESTED, Pr,
};

use super::checks::{RollupKind, checks_rollup};

fn label(code: &str) -> String {
    match code {
        ATTENTION_REVIEW_REQUESTED | ATTENTION_AWAITING_REVIEW => "Needs your review".to_owned(),
        ATTENTION_ASSIGNED => "Assigned to you".to_owned(),
        ATTENTION_CHANGES_REQUESTED => "Changes requested".to_owned(),
        ATTENTION_CHECKS_FAILING => "Checks failing".to_owned(),
        other => other.replace('_', " "),
    }
}

fn is_draft(pr: &Pr) -> bool {
    pr.draft || pr.merge_state_status.to_uppercase() == "DRAFT"
}

/// Reason codes, review request first. Legacy `awaiting_review` and local
/// "you" rows read as a review request; drafts never need attention.
pub fn pr_attention_codes(pr: &Pr) -> Vec<String> {
    if is_draft(pr) {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    let mut push = |code: &str| {
        let c = code.trim();
        if !c.is_empty() && !out.iter().any(|o| o == c) {
            out.push(c.to_owned());
        }
    };
    if pr.review_requested {
        push(ATTENTION_REVIEW_REQUESTED);
    }
    for c in &pr.attention_reasons {
        push(if c == ATTENTION_AWAITING_REVIEW {
            ATTENTION_REVIEW_REQUESTED
        } else {
            c
        });
    }
    if out.is_empty() && pr.group == "you" {
        out.push(ATTENTION_REVIEW_REQUESTED.to_owned());
    }
    out
}

/// Labels for the list and detail; failing checks show as an icon, not text.
pub fn pr_attention_labels(pr: &Pr) -> Vec<String> {
    let mut labels: Vec<String> = pr_attention_codes(pr)
        .iter()
        .filter(|c| *c != ATTENTION_CHECKS_FAILING)
        .map(|c| label(c))
        .collect();
    if pr.mergeable.to_uppercase() == "CONFLICTING"
        || pr.merge_state_status.to_uppercase() == "DIRTY"
    {
        labels.push("Merge conflicts".to_owned());
    }
    labels
}

pub fn pr_has_review_request(pr: &Pr) -> bool {
    pr_attention_codes(pr)
        .iter()
        .any(|c| c == ATTENTION_REVIEW_REQUESTED)
}

/// The PR reported at least one check run.
pub fn pr_has_check_status(pr: &Pr) -> bool {
    pr.has_checks
        || pr.checks.as_ref().is_some_and(|c| !c.is_empty())
        || !pr.checks_ok
        || pr
            .attention_reasons
            .iter()
            .any(|c| c == ATTENTION_CHECKS_FAILING)
}

pub fn pr_checks_rollup(pr: &Pr) -> RollupKind {
    checks_rollup(
        pr.checks.as_deref().unwrap_or_default(),
        Some(pr.checks_ok),
        Some(pr.has_checks),
    )
}

pub fn pr_checks_passing(pr: &Pr) -> bool {
    pr_checks_rollup(pr) == RollupKind::Pass
}

#[cfg(test)]
mod tests {
    use recur_domain::Check;

    use super::*;

    fn pr(f: impl FnOnce(&mut Pr)) -> Pr {
        let mut p = Pr {
            checks_ok: true,
            ..Default::default()
        };
        f(&mut p);
        p
    }

    #[test]
    fn codes() {
        let p = pr(|p| {
            p.review_requested = true;
            p.attention_reasons = vec![
                ATTENTION_REVIEW_REQUESTED.into(),
                ATTENTION_CHECKS_FAILING.into(),
            ];
        });
        assert_eq!(
            pr_attention_codes(&p),
            [ATTENTION_REVIEW_REQUESTED, ATTENTION_CHECKS_FAILING]
        );
        assert_eq!(
            pr_attention_codes(&pr(|p| p.group = "you".into())),
            [ATTENTION_REVIEW_REQUESTED]
        );
        assert!(pr_attention_codes(&pr(|p| p.group = "open".into())).is_empty());
        assert_eq!(
            pr_attention_codes(&pr(|p| p.attention_reasons = vec!["awaiting_review".into()])),
            [ATTENTION_REVIEW_REQUESTED]
        );
        let draft = pr(|p| {
            p.group = "you".into();
            p.draft = true;
            p.review_requested = true;
            p.attention_reasons = vec![ATTENTION_REVIEW_REQUESTED.into()];
        });
        assert!(pr_attention_codes(&draft).is_empty());
    }

    #[test]
    fn labels() {
        let p = pr(|p| {
            p.review_requested = true;
            p.attention_reasons = vec!["assigned".into(), ATTENTION_CHECKS_FAILING.into()];
        });
        assert_eq!(
            pr_attention_labels(&p),
            ["Needs your review", "Assigned to you"]
        );
        assert_eq!(
            pr_attention_labels(&pr(|p| p.group = "you".into())),
            ["Needs your review"]
        );
        assert_eq!(
            pr_attention_labels(&pr(|p| p.mergeable = "CONFLICTING".into())),
            ["Merge conflicts"]
        );
        let draft = pr(|p| {
            p.draft = true;
            p.merge_state_status = "DIRTY".into();
        });
        assert_eq!(pr_attention_labels(&draft), ["Merge conflicts"]);
        let clean = pr(|p| {
            p.mergeable = "MERGEABLE".into();
            p.merge_state_status = "CLEAN".into();
        });
        assert!(pr_attention_labels(&clean).is_empty());
    }

    #[test]
    fn review_request_and_check_status() {
        assert!(pr_has_review_request(&pr(|p| p.review_requested = true)));
        assert!(pr_has_review_request(&pr(
            |p| p.attention_reasons = vec![ATTENTION_REVIEW_REQUESTED.into()]
        )));
        assert!(pr_has_review_request(&pr(|p| p.group = "you".into())));
        assert!(!pr_has_review_request(&pr(
            |p| p.attention_reasons = vec![ATTENTION_CHECKS_FAILING.into()]
        )));
        assert!(!pr_has_review_request(&pr(|p| {
            p.group = "you".into();
            p.draft = true;
            p.review_requested = true;
        })));
        assert!(pr_has_check_status(&pr(|p| p.has_checks = true)));
        assert!(pr_has_check_status(&pr(
            |p| p.checks = Some(vec![Check::default()])
        )));
        assert!(pr_has_check_status(&pr(|p| p.checks_ok = false)));
        assert!(!pr_has_check_status(&pr(|_| {})));
    }

    #[test]
    fn rollup_treats_in_progress_as_pending() {
        let pending = pr(|p| {
            p.checks = Some(vec![Check {
                status: "pending".into(),
                ..Default::default()
            }]);
            p.checks_ok = false;
            p.has_checks = true;
        });
        assert_eq!(pr_checks_rollup(&pending), RollupKind::Pending);
        assert!(!pr_checks_passing(&pending));
        assert_eq!(
            pr_checks_rollup(&pr(|p| {
                p.checks_ok = false;
                p.has_checks = true;
            })),
            RollupKind::Fail
        );
        assert!(pr_checks_passing(&pr(|p| p.has_checks = true)));
    }
}
