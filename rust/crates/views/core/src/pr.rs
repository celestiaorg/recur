//! PR chrome (`lib/colors.ts` PR helpers) and the PR chat-host bookkeeping
//! (`lib/pr-workspace.ts`).

use std::collections::{BTreeMap, HashSet};

use recur_domain::{Pr, PrSource, Workspace};
use recur_ui::IconName;

/// GitHub-style PR lifecycle for chrome (sidebar icon tint).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrLifecycle {
    Open,
    Draft,
    Queued,
    Merged,
    Closed,
}

impl PrLifecycle {
    /// The `pr-<life>` class suffix.
    pub fn as_str(self) -> &'static str {
        match self {
            PrLifecycle::Open => "open",
            PrLifecycle::Draft => "draft",
            PrLifecycle::Queued => "queued",
            PrLifecycle::Merged => "merged",
            PrLifecycle::Closed => "closed",
        }
    }
}

pub fn pr_lifecycle(pr: Option<&Pr>) -> Option<PrLifecycle> {
    let pr = pr?;
    let state = pr.state.to_lowercase();
    Some(if state.contains("merged") {
        PrLifecycle::Merged
    } else if state.contains("closed") {
        PrLifecycle::Closed
    } else if pr.group == "merged" {
        PrLifecycle::Merged
    } else if pr.merge_queued {
        PrLifecycle::Queued
    } else if pr.draft || pr.merge_state_status.eq_ignore_ascii_case("draft") {
        PrLifecycle::Draft
    } else {
        PrLifecycle::Open
    })
}

/// Keep a local Merged lifecycle when a stale GetPR or event still looks open.
pub fn keep_merged_pr(prev: Option<&Pr>, mut next: Pr) -> Pr {
    if let Some(prev) = prev
        && pr_lifecycle(Some(prev)) == Some(PrLifecycle::Merged)
        && pr_lifecycle(Some(&next)) != Some(PrLifecycle::Merged)
    {
        next.state = prev.state.clone();
        next.group = prev.group.clone();
    }
    next
}

/// Chrome glyph for a PR: draft dotted, closed crossed, merged git-merge, queued dashed.
pub fn pr_icon_name(pr: Option<&Pr>) -> IconName {
    match pr_lifecycle(pr) {
        Some(PrLifecycle::Merged) => IconName::GitMerge,
        Some(PrLifecycle::Queued) => IconName::GitMergeQueued,
        Some(PrLifecycle::Closed) => IconName::GitPullRequestClosed,
        Some(PrLifecycle::Draft) => IconName::GitPullRequestDraft,
        _ => IconName::GitPullRequest,
    }
}

fn is_live_pr(life: Option<PrLifecycle>) -> bool {
    matches!(
        life,
        Some(PrLifecycle::Open | PrLifecycle::Draft | PrLifecycle::Queued)
    )
}

/// Lower is better: GitHub over Local, then live over settled.
fn workspace_pr_rank(p: &Pr) -> u8 {
    let github = if p.source == PrSource::GitHub { 0 } else { 1 };
    let live = if is_live_pr(pr_lifecycle(Some(p))) {
        0
    } else {
        1
    };
    github * 2 + live
}

/// The PR used for workspace chrome (sidebar icon, View PR). A leftover Local PR must not
/// hide a GitHub PR on the same workspace. Ties go to the later (newer) entry.
pub fn pr_for_workspace<'a>(list: &'a [Pr], workspace_id: &str) -> Option<&'a Pr> {
    let mut best: Option<(&Pr, u8)> = None;
    for p in list.iter().filter(|p| p.workspace_id == workspace_id) {
        let rank = workspace_pr_rank(p);
        if best.is_none_or(|(_, b)| rank <= b) {
            best = Some((p, rank));
        }
    }
    best.map(|(p, _)| p)
}

/// The workspace the PR-detail agent dock binds to: a live chat host first (an in-flight
/// PR-detail turn is not abandoned when a worktree gets linked), then the linked workspace.
pub fn pr_agent_workspace_id(pr_id: &str, linked: &str, workspaces: &[Workspace]) -> String {
    let pr_id = pr_id.trim();
    if !pr_id.is_empty()
        && let Some(host) = workspaces
            .iter()
            .find(|w| !w.archived && w.host_pr_id.trim() == pr_id)
    {
        return host.id.clone();
    }
    let linked = linked.trim();
    if !linked.is_empty()
        && workspaces
            .iter()
            .find(|w| w.id == linked)
            .is_some_and(|w| !w.archived)
    {
        return linked.to_string();
    }
    String::new()
}

/// How many hidden PR chat hosts have a live agent.
pub fn pr_chat_host_running_count(workspaces: &[Workspace], running: &HashSet<String>) -> usize {
    let mut seen = HashSet::new();
    workspaces
        .iter()
        .filter(|w| !w.archived && !w.host_pr_id.trim().is_empty())
        .filter(|w| running.contains(&w.id) && seen.insert(w.id.as_str()))
        .count()
}

/// Unseen finished chat turns across hidden PR chat hosts (no sidebar row of their own).
pub fn pr_chat_host_unseen_count(
    workspaces: &[Workspace],
    by_workspace: &BTreeMap<String, i64>,
) -> i64 {
    let mut seen = HashSet::new();
    workspaces
        .iter()
        .filter(|w| !w.archived && !w.host_pr_id.trim().is_empty())
        .filter(|w| seen.insert(w.id.as_str()))
        .map(|w| by_workspace.get(&w.id).copied().unwrap_or(0))
        .sum()
}

/// Inbox `chat:` targets on a hidden PR host open the PR, not the host workspace.
pub fn pr_id_for_chat_workspace(workspace_id: &str, workspaces: &[Workspace]) -> String {
    let id = workspace_id.trim();
    if id.is_empty() {
        return String::new();
    }
    workspaces
        .iter()
        .find(|w| w.id == id)
        .map(|w| w.host_pr_id.trim().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(f: impl FnOnce(&mut Pr)) -> Pr {
        let mut p = Pr::default();
        f(&mut p);
        p
    }

    fn ws(id: &str, host: &str, archived: bool) -> Workspace {
        Workspace {
            id: id.into(),
            host_pr_id: host.into(),
            archived,
            ..Workspace::default()
        }
    }

    fn set(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn lifecycle() {
        fn l(f: impl FnOnce(&mut Pr)) -> Option<PrLifecycle> {
            pr_lifecycle(Some(&pr(f)))
        }
        assert_eq!(pr_lifecycle(None), None);
        assert_eq!(
            l(|p| {
                p.group = "merged".into();
                p.state = "In review".into();
            }),
            Some(PrLifecycle::Merged)
        );
        assert_eq!(
            l(|p| {
                p.group = "you".into();
                p.state = "Merged".into();
            }),
            Some(PrLifecycle::Merged)
        );
        assert_eq!(
            l(|p| {
                p.group = "merged".into();
                p.state = "Closed".into();
            }),
            Some(PrLifecycle::Closed)
        );
        assert_eq!(l(|p| p.state = "Approved".into()), Some(PrLifecycle::Open));
        assert_eq!(
            l(|p| {
                p.state = "In review".into();
                p.draft = true;
            }),
            Some(PrLifecycle::Draft)
        );
        assert_eq!(
            l(|p| {
                p.state = "In review".into();
                p.merge_state_status = "DRAFT".into();
            }),
            Some(PrLifecycle::Draft)
        );
        assert_eq!(
            l(|p| {
                p.group = "merged".into();
                p.state = "Merged".into();
                p.draft = true;
            }),
            Some(PrLifecycle::Merged)
        );
    }

    #[test]
    fn keep_merged() {
        let prev = pr(|p| {
            p.state = "Merged".into();
            p.group = "merged".into();
            p.title = "old".into();
        });
        let next = pr(|p| {
            p.state = "In review".into();
            p.group = "you".into();
            p.title = "from GetPR".into();
        });
        let kept = keep_merged_pr(Some(&prev), next.clone());
        assert_eq!(
            (
                kept.state.as_str(),
                kept.group.as_str(),
                kept.title.as_str()
            ),
            ("Merged", "merged", "from GetPR")
        );
        assert_eq!(keep_merged_pr(None, next.clone()), next);
        let closed = pr(|p| {
            p.state = "Closed".into();
            p.group = "merged".into();
        });
        let reopened = pr(|p| {
            p.state = "Open".into();
            p.group = "open".into();
        });
        assert_eq!(keep_merged_pr(Some(&closed), reopened.clone()), reopened);
    }

    #[test]
    fn icons() {
        fn i(f: impl FnOnce(&mut Pr)) -> IconName {
            pr_icon_name(Some(&pr(f)))
        }
        assert_eq!(i(|p| p.draft = true), IconName::GitPullRequestDraft);
        assert_eq!(
            i(|p| p.merge_state_status = "DRAFT".into()),
            IconName::GitPullRequestDraft
        );
        assert_eq!(pr_icon_name(None), IconName::GitPullRequest);
        assert_eq!(
            i(|p| {
                p.group = "merged".into();
                p.draft = true;
            }),
            IconName::GitMerge
        );
        assert_eq!(
            i(|p| {
                p.state = "In review".into();
                p.merge_queued = true;
            }),
            IconName::GitMergeQueued
        );
        assert_eq!(
            i(|p| {
                p.state = "Merged".into();
                p.merge_queued = true;
            }),
            IconName::GitMerge
        );
        assert_eq!(
            i(|p| {
                p.state = "Closed".into();
                p.draft = true;
            }),
            IconName::GitPullRequestClosed
        );
    }

    #[test]
    fn for_workspace() {
        let p = |ws: &str, id: &str, src: &str, group: &str, state: &str| {
            pr(|p| {
                p.workspace_id = ws.into();
                p.id = id.into();
                p.source = src.into();
                p.group = group.into();
                p.state = state.into();
            })
        };
        let list = [
            p("ws-a", "1", "", "merged", "Merged"),
            p("ws-a", "2", "", "you", "In review"),
            p("ws-b", "3", "", "merged", "Merged"),
        ];
        assert_eq!(pr_for_workspace(&list, "ws-a").map(|p| &*p.id), Some("2"));
        assert_eq!(pr_for_workspace(&list, "ws-b").map(|p| &*p.id), Some("3"));
        assert!(pr_for_workspace(&list, "ws-none").is_none());
        let local = p("ws-c", "4", "local", "you", "In review");
        let gh = p("ws-c", "gh-156", "gh", "merged", "Merged");
        let pick = |l: &[Pr]| pr_for_workspace(l, "ws-c").map(|p| p.id.clone());
        assert_eq!(
            pick(&[local.clone(), gh.clone()]).as_deref(),
            Some("gh-156")
        );
        assert_eq!(pick(&[gh, local.clone()]).as_deref(), Some("gh-156"));
        let closed = p("ws-d", "gh-9", "gh", "merged", "Closed");
        let merged = p("ws-d", "gh-13", "gh", "merged", "Merged");
        assert_eq!(
            pr_for_workspace(&[closed, merged], "ws-d").map(|p| &*p.id),
            Some("gh-13")
        );
        let open_gh = p("ws-c", "gh-156", "gh", "you", "In review");
        assert_eq!(pick(&[local, open_gh]).as_deref(), Some("gh-156"));
    }

    #[test]
    fn agent_workspace() {
        let host = ws("ws-host", "pr-1", false);
        let linked = ws("ws-linked", "", false);
        let a =
            |linked_id: &str, list: &[Workspace]| pr_agent_workspace_id("pr-1", linked_id, list);
        assert_eq!(a("ws-linked", &[host.clone(), linked.clone()]), "ws-host");
        assert_eq!(a("", std::slice::from_ref(&host)), "ws-host");
        assert_eq!(a("", &[ws("ws-host", "pr-1", true)]), "");
        assert_eq!(
            a("ws-linked", &[host.clone(), ws("ws-linked", "", true)]),
            "ws-host"
        );
        assert_eq!(
            a("ws-linked", &[ws("ws-host", "pr-1", true), linked.clone()]),
            "ws-linked"
        );
    }

    #[test]
    fn host_counts() {
        let host = ws("ws-host", "pr-1", false);
        let linked = ws("ws-linked", "", false);
        let other = ws("ws-host-2", "pr-2", false);
        assert_eq!(
            pr_chat_host_running_count(
                &[host.clone(), linked.clone(), other.clone()],
                &set(&["ws-host", "ws-linked"])
            ),
            1
        );
        assert_eq!(
            pr_chat_host_running_count(&[host.clone(), linked.clone()], &set(&["ws-linked"])),
            0
        );
        assert_eq!(
            pr_chat_host_running_count(
                &[host.clone(), other.clone()],
                &set(&["ws-host", "ws-host-2"])
            ),
            2
        );
        assert_eq!(
            pr_chat_host_running_count(
                &[ws("ws-host", "pr-1", true), other.clone()],
                &set(&["ws-host", "ws-host-2"])
            ),
            1
        );
        assert_eq!(
            pr_chat_host_running_count(&[host.clone(), host.clone()], &set(&["ws-host"])),
            1
        );
        let m = |pairs: &[(&str, i64)]| -> BTreeMap<String, i64> {
            pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
        };
        assert_eq!(
            pr_chat_host_unseen_count(
                &[host.clone(), linked.clone(), other.clone()],
                &m(&[("ws-host", 2), ("ws-host-2", 1), ("ws-linked", 9)])
            ),
            3
        );
        assert_eq!(
            pr_chat_host_unseen_count(&[host.clone(), linked], &m(&[("ws-linked", 4)])),
            0
        );
        assert_eq!(
            pr_chat_host_unseen_count(
                &[ws("ws-host", "pr-1", true), other.clone()],
                &m(&[("ws-host", 2), ("ws-host-2", 1)])
            ),
            1
        );
        assert_eq!(
            pr_chat_host_unseen_count(&[host.clone(), host.clone()], &m(&[("ws-host", 3)])),
            3
        );
        assert_eq!(pr_chat_host_unseen_count(&[host, other], &m(&[])), 0);
    }

    #[test]
    fn chat_workspace_pr() {
        let list = [ws("ws-host", "pr-1", false), ws("ws-linked", "", false)];
        assert_eq!(pr_id_for_chat_workspace("ws-host", &list), "pr-1");
        assert_eq!(pr_id_for_chat_workspace("ws-linked", &list), "");
    }
}
