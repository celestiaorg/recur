//! The titlebar's merge slot for a PR:
//! which one merge-adjacent action shows, its label and tooltip, and the
//! merge / update-with-base method pickers.

use recur_domain::Pr;
use recur_forge::view::{RollupKind, checks_rollup};

use crate::prs::is_forge_source;

pub const MERGE_METHODS: [&str; 3] = ["merge", "squash", "rebase"];
pub const UPDATE_BASE_METHODS: [&str; 2] = ["merge", "rebase"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeActionKind {
    Hidden,
    Updating,
    Ready,
    UpdateBase,
    Resolve,
    Conflicts,
    Checks,
    Blocked,
    Queued,
    Merge,
}

impl MergeActionKind {
    /// The kind's string id.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hidden => "hidden",
            Self::Updating => "updating",
            Self::Ready => "ready",
            Self::UpdateBase => "update-base",
            Self::Resolve => "resolve",
            Self::Conflicts => "conflicts",
            Self::Checks => "checks",
            Self::Blocked => "blocked",
            Self::Queued => "queued",
            Self::Merge => "merge",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeAction {
    pub kind: MergeActionKind,
    pub label: String,
    pub title: String,
    pub disabled: bool,
}

impl MergeAction {
    fn new(kind: MergeActionKind, label: &str, title: &str, disabled: bool) -> Self {
        Self {
            kind,
            label: label.into(),
            title: title.into(),
            disabled,
        }
    }

    fn hidden() -> Self {
        Self::new(MergeActionKind::Hidden, "", "", true)
    }
}

fn upper(s: &str) -> String {
    s.trim().to_uppercase()
}

fn viewer_is_author(pr: &Pr) -> bool {
    let (viewer, author) = (pr.viewer_login.trim(), pr.author_login.trim());
    !viewer.is_empty() && !author.is_empty() && viewer.to_lowercase() == author.to_lowercase()
}

fn review_approved(pr: &Pr) -> bool {
    upper(&pr.review_decision) == "APPROVED" || pr.state == "Approved"
}

fn review_blocks(pr: &Pr) -> bool {
    let d = upper(&pr.review_decision);
    d == "REVIEW_REQUIRED" || d == "CHANGES_REQUESTED" || pr.state == "Changes requested"
}

fn settled(pr: &Pr) -> bool {
    pr.state == "Merged" || pr.group == "merged" || pr.state == "Closed"
}

/// The one merge-adjacent titlebar slot for this PR. Local PRs always merge.
/// `awaiting_update` is an update-with-base the UI started.
pub fn pr_merge_action(pr: Option<&Pr>, awaiting_update: bool) -> MergeAction {
    use MergeActionKind as K;
    let Some(pr) = pr else {
        return MergeAction::hidden();
    };
    if settled(pr) {
        return MergeAction::hidden();
    }
    if awaiting_update || !pr.updating_base.is_empty() {
        return MergeAction::new(K::Updating, "Updating…", "", true);
    }
    if !is_forge_source(&pr.source) {
        return MergeAction::new(
            K::Merge,
            "Merge",
            "Local merge: you are the only gate; merges the branch into the base on disk",
            false,
        );
    }
    if pr.merge_queued {
        return MergeAction::new(
            K::Queued,
            "Queued",
            "Waiting in the GitHub merge queue",
            true,
        );
    }
    let status = upper(&pr.merge_state_status);
    let mergeable = upper(&pr.mergeable);
    let dirty = status == "DIRTY" || mergeable == "CONFLICTING";
    let draft = pr.draft || status == "DRAFT";
    let can_write = pr.viewer_can_write;
    let known_viewer = !pr.viewer_login.trim().is_empty();
    let can_ready = !known_viewer || can_write || viewer_is_author(pr);
    if draft && can_ready {
        return MergeAction::new(
            K::Ready,
            "Ready for review",
            "Mark this draft ready for review on GitHub",
            false,
        );
    }
    if known_viewer && !can_write {
        return MergeAction::hidden();
    }
    let has_workspace = !pr.workspace_id.trim().is_empty();
    let base = match pr.base.trim() {
        "" => "base",
        b => b,
    };
    if dirty && has_workspace {
        return MergeAction::new(
            K::Resolve,
            "Resolve conflicts",
            &format!(
                "Update with {}; an agent resolves the conflicts in the workspace's ops tab",
                short_base_name(base)
            ),
            false,
        );
    }
    if dirty {
        return MergeAction::new(
            K::Conflicts,
            "Conflicts",
            "Conflicts with the base branch. Open this PR in a workspace to resolve them.",
            true,
        );
    }
    if has_workspace && status == "BEHIND" {
        return MergeAction::new(
            K::UpdateBase,
            &update_with_base_label("merge", base),
            &update_base_title(pr, base),
            false,
        );
    }
    if status == "BLOCKED" {
        let checks = pr.checks.as_deref().unwrap_or_default();
        if checks_rollup(checks, Some(pr.checks_ok), Some(pr.has_checks)) == RollupKind::Fail {
            return MergeAction::new(
                K::Checks,
                "Fix checks",
                "Ask an agent to fix the failing GitHub checks",
                false,
            );
        }
        if review_blocks(pr) && !review_approved(pr) {
            return MergeAction::hidden();
        }
        return MergeAction::new(
            K::Blocked,
            "Merge blocked",
            "Merge blocked on GitHub: required reviews, checks, or unresolved conversations",
            true,
        );
    }
    if pr.merge_queue_enabled {
        return MergeAction::new(
            K::Merge,
            "Add to merge queue",
            "The base branch requires a merge queue; GitHub merges the PR once it passes the queue",
            false,
        );
    }
    let title = if status == "UNSTABLE" {
        "Some checks are not green, but GitHub still allows merge"
    } else {
        "Merge the PR"
    };
    MergeAction::new(K::Merge, "Merge", title, false)
}

/// Update with base is independent of the merge slot: review-blocked PRs
/// still catch up.
pub fn show_pr_update(pr: Option<&Pr>, action: &MergeAction) -> bool {
    let Some(pr) = pr.filter(|p| !p.workspace_id.trim().is_empty()) else {
        return false;
    };
    match action.kind {
        MergeActionKind::Updating => return false,
        MergeActionKind::UpdateBase | MergeActionKind::Resolve => return true,
        _ => {}
    }
    if pr.behind_base <= 0 || settled(pr) {
        return false;
    }
    pr.viewer_login.trim().is_empty() || pr.viewer_can_write
}

/// The author or a writer can flip a forge PR's draft state; an unknown
/// viewer is allowed.
pub fn can_set_pr_draft(pr: Option<&Pr>) -> bool {
    let Some(pr) = pr.filter(|p| is_forge_source(&p.source)) else {
        return false;
    };
    if settled(pr) {
        return false;
    }
    pr.viewer_login.trim().is_empty() || pr.viewer_can_write || viewer_is_author(pr)
}

/// The forge forbids Approve / Request changes on your own PR; Local PRs and
/// an unknown viewer still offer it.
pub fn can_approve_pr_review(pr: Option<&Pr>) -> bool {
    let Some(pr) = pr else {
        return false;
    };
    if settled(pr) {
        return false;
    }
    if !is_forge_source(&pr.source) {
        return true;
    }
    if viewer_is_author(pr) {
        return false;
    }
    pr.viewer_login.trim().is_empty() || pr.viewer_can_write
}

/// The repo's allowed merge types; empty, unknown or Local means all three.
pub fn allowed_merge_methods(pr: Option<&Pr>) -> Vec<&'static str> {
    let Some(pr) = pr.filter(|p| is_forge_source(&p.source)) else {
        return MERGE_METHODS.to_vec();
    };
    let raw: Vec<&'static str> = pr
        .allowed_merge_methods
        .iter()
        .filter_map(|s| {
            let s = s.trim().to_lowercase();
            MERGE_METHODS.iter().copied().find(|m| *m == s)
        })
        .collect();
    if raw.is_empty() {
        MERGE_METHODS.to_vec()
    } else {
        raw
    }
}

/// The preferred method when allowed, else the first allowed, else merge.
pub fn pick_allowed_merge_method(preferred: &str, allowed: &[&str]) -> &'static str {
    let pref = preferred.trim().to_lowercase();
    if let Some(m) = MERGE_METHODS.iter().find(|m| **m == pref)
        && allowed.contains(m)
    {
        return m;
    }
    allowed
        .first()
        .and_then(|first| MERGE_METHODS.iter().find(|m| *m == first))
        .copied()
        .unwrap_or("merge")
}

pub fn is_update_base_method(s: &str) -> bool {
    UPDATE_BASE_METHODS.contains(&s)
}

/// `origin/` or `upstream/` stripped so the titlebar says "Update with main".
pub fn short_base_name(base: &str) -> String {
    let b = base.trim();
    let short = ["origin/", "upstream/"]
        .iter()
        .find_map(|p| b.strip_prefix(p).filter(|rest| !rest.is_empty()))
        .unwrap_or(b);
    if short.is_empty() {
        "base".into()
    } else {
        short.to_owned()
    }
}

/// UI override, then project, then settings, then merge.
pub fn resolve_update_base_method(
    preferred: Option<&str>,
    fallback: Option<&str>,
    override_: Option<&str>,
) -> &'static str {
    for m in [override_, preferred, fallback].into_iter().flatten() {
        if let Some(found) = UPDATE_BASE_METHODS.iter().find(|u| **u == m) {
            return found;
        }
    }
    "merge"
}

pub fn update_with_base_label(method: &str, base: &str) -> String {
    let name = short_base_name(base);
    if method == "rebase" {
        format!("Rebase onto {name}")
    } else {
        format!("Update with {name}")
    }
}

fn update_base_title(pr: &Pr, base: &str) -> String {
    let n = pr.behind_base;
    let name = short_base_name(base);
    if n > 0 {
        let s = if n == 1 { "" } else { "s" };
        format!("{n} commit{s} on {name} not in this branch")
    } else {
        format!("This branch is behind {name} on GitHub")
    }
}

#[cfg(test)]
mod tests {
    use recur_domain::{Check, PrSource};

    use super::*;

    fn gh(over: impl FnOnce(&mut Pr)) -> Pr {
        let mut p = Pr {
            source: PrSource::GitHub,
            state: "In review".into(),
            group: "you".into(),
            checks_ok: true,
            viewer_can_write: true,
            viewer_login: "josh".into(),
            workspace_id: "ws-1".into(),
            ..Pr::default()
        };
        over(&mut p);
        p
    }

    fn kind(p: &Pr) -> &'static str {
        pr_merge_action(Some(p), false).kind.as_str()
    }

    #[test]
    fn hides_merged_and_closed() {
        assert_eq!(
            kind(&gh(|p| {
                p.state = "Merged".into();
                p.group = "merged".into();
            })),
            "hidden"
        );
        assert_eq!(kind(&gh(|p| p.state = "Closed".into())), "hidden");
        assert_eq!(pr_merge_action(None, false).kind, MergeActionKind::Hidden);
    }

    #[test]
    fn merge_queue() {
        let q = pr_merge_action(
            Some(&gh(|p| {
                p.merge_state_status = "CLEAN".into();
                p.merge_queue_enabled = true;
            })),
            false,
        );
        assert_eq!(q.label, "Add to merge queue");
        let queued = pr_merge_action(
            Some(&gh(|p| {
                p.merge_queue_enabled = true;
                p.merge_queued = true;
            })),
            false,
        );
        assert_eq!(
            (queued.kind, queued.disabled),
            (MergeActionKind::Queued, true)
        );
    }

    #[test]
    fn updating() {
        assert_eq!(kind(&gh(|p| p.updating_base = "git".into())), "updating");
        assert_eq!(
            pr_merge_action(Some(&gh(|_| {})), true).kind.as_str(),
            "updating"
        );
    }

    #[test]
    fn local_always_merges() {
        let p = Pr {
            source: PrSource::Local,
            state: "In review".into(),
            behind_base: 2,
            ..Pr::default()
        };
        let act = pr_merge_action(Some(&p), false);
        assert_eq!(act.kind, MergeActionKind::Merge);
        assert!(act.title.contains("Local merge"));
    }

    #[test]
    fn drafts() {
        assert_eq!(kind(&gh(|p| p.draft = true)), "ready");
        assert_eq!(
            kind(&gh(|p| p.merge_state_status = "DRAFT".into())),
            "ready"
        );
        assert_eq!(
            kind(&gh(|p| {
                p.draft = true;
                p.behind_base = 2;
            })),
            "ready"
        );
        assert_eq!(
            kind(&gh(|p| {
                p.draft = true;
                p.viewer_can_write = false;
                p.author_login = "josh".into();
            })),
            "ready"
        );
    }

    #[test]
    fn no_write_hides_merge() {
        assert_eq!(
            kind(&gh(|p| {
                p.viewer_login = "guest".into();
                p.viewer_can_write = false;
                p.merge_state_status = "CLEAN".into();
            })),
            "hidden"
        );
    }

    #[test]
    fn behind_and_conflicts() {
        assert_eq!(
            kind(&gh(|p| p.merge_state_status = "BEHIND".into())),
            "update-base"
        );
        let act = pr_merge_action(
            Some(&gh(|p| {
                p.merge_state_status = "BEHIND".into();
                p.behind_base = 3;
                p.base = "main".into();
            })),
            false,
        );
        assert!(act.title.contains("3 commits on main"), "{}", act.title);
        assert_eq!(
            kind(&gh(|p| {
                p.merge_state_status = "CLEAN".into();
                p.behind_base = 2;
            })),
            "merge"
        );
        assert_eq!(kind(&gh(|p| p.behind_base = 2)), "merge");
        let pr = gh(|p| {
            p.merge_state_status = "DIRTY".into();
            p.mergeable = "CONFLICTING".into();
        });
        let act = pr_merge_action(Some(&pr), false);
        assert_eq!(
            (act.kind, act.label.as_str(), act.disabled),
            (MergeActionKind::Resolve, "Resolve conflicts", false)
        );
        assert!(show_pr_update(Some(&pr), &act));
        assert_ne!(
            kind(&gh(|p| {
                p.workspace_id.clear();
                p.merge_state_status = "BEHIND".into();
            })),
            "update-base"
        );
        let act = pr_merge_action(
            Some(&gh(|p| {
                p.workspace_id.clear();
                p.mergeable = "CONFLICTING".into();
                p.merge_state_status = "DIRTY".into();
            })),
            false,
        );
        assert_eq!((act.kind, act.disabled), (MergeActionKind::Conflicts, true));
    }

    fn check(status: &str) -> Check {
        Check {
            status: status.into(),
            ..Check::default()
        }
    }

    #[test]
    fn blocked() {
        assert_eq!(
            kind(&gh(|p| {
                p.merge_state_status = "BLOCKED".into();
                p.checks_ok = false;
                p.review_decision = "APPROVED".into();
                p.checks = Some(vec![check("fail"), check("pending")]);
            })),
            "checks"
        );
        let act = pr_merge_action(
            Some(&gh(|p| {
                p.merge_state_status = "BLOCKED".into();
                p.checks_ok = false;
                p.review_decision = "APPROVED".into();
            })),
            false,
        );
        assert_eq!(act.label, "Fix checks");
        assert_eq!(
            kind(&gh(|p| {
                p.merge_state_status = "BLOCKED".into();
                p.checks_ok = false;
                p.review_decision = "APPROVED".into();
                p.checks = Some(vec![check("pending")]);
            })),
            "blocked"
        );
        for d in ["REVIEW_REQUIRED", "CHANGES_REQUESTED"] {
            let pr = gh(|p| {
                p.merge_state_status = "BLOCKED".into();
                p.review_decision = d.into();
            });
            assert_eq!(kind(&pr), "hidden");
            let pr = gh(|p| {
                p.merge_state_status = "BLOCKED".into();
                p.review_decision = d.into();
                p.behind_base = 2;
            });
            let act = pr_merge_action(Some(&pr), false);
            assert_eq!(act.kind, MergeActionKind::Hidden);
            assert!(show_pr_update(Some(&pr), &act), "still offers Update");
        }
        let guest = gh(|p| {
            p.viewer_login = "guest".into();
            p.viewer_can_write = false;
            p.behind_base = 2;
        });
        assert!(!show_pr_update(
            Some(&guest),
            &pr_merge_action(Some(&guest), false)
        ));
        let closed = gh(|p| {
            p.state = "Closed".into();
            p.behind_base = 2;
        });
        assert!(!show_pr_update(
            Some(&closed),
            &pr_merge_action(Some(&closed), false)
        ));
        let act = pr_merge_action(
            Some(&gh(|p| {
                p.merge_state_status = "BLOCKED".into();
                p.review_decision = "APPROVED".into();
            })),
            false,
        );
        assert_eq!((act.kind, act.disabled), (MergeActionKind::Blocked, true));
    }

    #[test]
    fn draft_and_approve_permissions() {
        assert!(can_set_pr_draft(Some(&gh(|_| {}))));
        assert!(can_set_pr_draft(Some(&gh(|p| {
            p.viewer_can_write = false;
            p.author_login = "josh".into();
        }))));
        assert!(!can_set_pr_draft(Some(&gh(|p| {
            p.viewer_login = "guest".into();
            p.viewer_can_write = false;
        }))));
        assert!(!can_set_pr_draft(Some(&gh(|p| {
            p.state = "Merged".into();
            p.group = "merged".into();
        }))));
        let local = Pr {
            source: PrSource::Local,
            state: "In review".into(),
            ..Pr::default()
        };
        assert!(!can_set_pr_draft(Some(&local)));
        assert!(!can_approve_pr_review(Some(&gh(
            |p| p.author_login = "josh".into()
        ))));
        assert!(can_approve_pr_review(Some(&gh(
            |p| p.author_login = "loopbot".into()
        ))));
        assert!(!can_approve_pr_review(Some(&gh(|p| {
            p.viewer_login = "guest".into();
            p.viewer_can_write = false;
        }))));
        assert!(can_approve_pr_review(Some(&gh(|p| p.viewer_login.clear()))));
        assert!(can_approve_pr_review(Some(&local)));
        assert!(!can_approve_pr_review(Some(&gh(|p| {
            p.state = "Merged".into();
            p.group = "merged".into();
            p.author_login = "loopbot".into();
        }))));
        assert!(!can_approve_pr_review(Some(&gh(|p| {
            p.state = "Closed".into();
            p.author_login = "loopbot".into();
        }))));
    }

    #[test]
    fn merge_methods() {
        let local = Pr::default();
        assert_eq!(allowed_merge_methods(Some(&local)), MERGE_METHODS);
        assert_eq!(allowed_merge_methods(Some(&gh(|_| {}))), MERGE_METHODS);
        assert_eq!(
            allowed_merge_methods(Some(&gh(
                |p| p.allowed_merge_methods = vec!["squash".into()]
            ))),
            ["squash"]
        );
        assert_eq!(
            pick_allowed_merge_method("merge", &["squash", "rebase"]),
            "squash"
        );
        assert_eq!(
            pick_allowed_merge_method("rebase", &["squash", "rebase"]),
            "rebase"
        );
        assert_eq!(pick_allowed_merge_method("x", &[]), "merge");
    }

    #[test]
    fn still_merges_on_other_states() {
        assert!(
            pr_merge_action(
                Some(&gh(|p| p.merge_state_status = "UNSTABLE".into())),
                false
            )
            .title
            .contains("not green")
        );
        for s in ["CLEAN", "HAS_HOOKS", "UNKNOWN", ""] {
            assert_eq!(
                kind(&gh(|p| p.merge_state_status = s.into())),
                "merge",
                "{s}"
            );
        }
    }

    #[test]
    fn update_labels() {
        assert_eq!(short_base_name("origin/develop"), "develop");
        assert_eq!(short_base_name("upstream/main"), "main");
        assert_eq!(short_base_name("main"), "main");
        assert_eq!(short_base_name(""), "base");
        assert_eq!(resolve_update_base_method(None, None, None), "merge");
        assert_eq!(
            resolve_update_base_method(Some("rebase"), Some("merge"), None),
            "rebase"
        );
        assert_eq!(
            resolve_update_base_method(Some("nope"), Some("rebase"), None),
            "rebase"
        );
        assert_eq!(
            resolve_update_base_method(Some("rebase"), Some("merge"), Some("merge")),
            "merge"
        );
        assert_eq!(
            update_with_base_label("merge", "origin/main"),
            "Update with main"
        );
        assert_eq!(
            update_with_base_label("rebase", "develop"),
            "Rebase onto develop"
        );
        assert_eq!(
            pr_merge_action(
                Some(&gh(|p| {
                    p.merge_state_status = "BEHIND".into();
                    p.behind_base = 2;
                    p.base = "origin/main".into();
                })),
                false
            )
            .label,
            "Update with main"
        );
        assert!(is_update_base_method("rebase") && !is_update_base_method("squash"));
    }
}
