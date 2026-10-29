//! Which workspace a PR's agent dock binds to: a hidden PR chat host or the
//! linked worktree.

use std::collections::{HashMap, HashSet};

use recur_domain::{Pr, Workspace};

fn find<'a>(workspaces: &'a [Workspace], id: &str) -> Option<&'a Workspace> {
    workspaces.iter().find(|w| w.id == id)
}

fn is_host(w: &Workspace) -> bool {
    !w.host_pr_id.trim().is_empty()
}

/// The workspace the PR-detail agent dock binds to.
pub fn pr_agent_workspace_id(pr: Option<&Pr>, workspaces: &[Workspace]) -> String {
    let Some(pr) = pr else {
        return String::new();
    };
    let pr_id = pr.id.trim();
    // A leftover chat host stays bound until hoist archives it, so an
    // in-flight PR-detail turn is not abandoned when a worktree gets linked.
    if !pr_id.is_empty()
        && let Some(host) = workspaces
            .iter()
            .find(|w| !w.archived && w.host_pr_id.trim() == pr_id)
    {
        return host.id.clone();
    }
    let linked = pr.workspace_id.trim();
    if !linked.is_empty() && find(workspaces, linked).is_some_and(|w| !w.archived) {
        return linked.to_owned();
    }
    String::new()
}

/// The linked worktree that can run Agent (live, not a PR chat host).
pub fn pr_editable_workspace_id(pr: Option<&Pr>, workspaces: &[Workspace]) -> String {
    let id = pr.map_or("", |p| p.workspace_id.trim());
    match find(workspaces, id) {
        Some(w) if !id.is_empty() && !w.archived && !is_host(w) => id.to_owned(),
        _ => String::new(),
    }
}

fn is_archived_chat_host(w: Option<&Workspace>) -> bool {
    w.is_some_and(|w| w.archived && is_host(w))
}

/// The PR's dock workspace (linked worktree or hidden chat host) has a live
/// agent.
pub fn pr_agent_is_running(
    pr: Option<&Pr>,
    workspaces: &[Workspace],
    running: &HashSet<String>,
) -> bool {
    let id = pr_agent_workspace_id(pr, workspaces);
    !id.is_empty() && running.contains(&id) && !is_archived_chat_host(find(workspaces, &id))
}

/// How many hidden PR chat hosts have a live agent.
pub fn pr_chat_host_running_count(workspaces: &[Workspace], running: &HashSet<String>) -> usize {
    let mut seen = HashSet::new();
    workspaces
        .iter()
        .filter(|w| !w.archived && is_host(w))
        .filter(|w| running.contains(&w.id) && seen.insert(w.id.as_str()))
        .count()
}

/// Unseen finished chat turns across hidden PR chat hosts (no sidebar row of
/// their own).
pub fn pr_chat_host_unseen_count(
    workspaces: &[Workspace],
    by_workspace: &HashMap<String, i64>,
) -> i64 {
    let mut seen = HashSet::new();
    workspaces
        .iter()
        .filter(|w| !w.archived && is_host(w) && seen.insert(w.id.as_str()))
        .map(|w| by_workspace.get(&w.id).copied().unwrap_or(0))
        .sum()
}

/// Inbox `chat:` targets on a hidden PR host open the PR, not the host.
pub fn pr_id_for_chat_workspace(workspace_id: &str, workspaces: &[Workspace]) -> String {
    let id = workspace_id.trim();
    if id.is_empty() {
        return String::new();
    }
    find(workspaces, id).map_or_else(String::new, |w| w.host_pr_id.trim().to_owned())
}

/// Unseen finished chat turns on the PR's dock workspace.
pub fn pr_unseen_count(
    pr: Option<&Pr>,
    workspaces: &[Workspace],
    by_workspace: &HashMap<String, i64>,
) -> i64 {
    let id = pr_agent_workspace_id(pr, workspaces);
    if id.is_empty() || is_archived_chat_host(find(workspaces, &id)) {
        return 0;
    }
    by_workspace.get(&id).copied().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(id: &str, host: &str, archived: bool) -> Workspace {
        Workspace {
            id: id.into(),
            host_pr_id: host.into(),
            archived,
            ..Workspace::default()
        }
    }

    fn host() -> Workspace {
        ws("ws-host", "pr-1", false)
    }
    fn linked() -> Workspace {
        ws("ws-linked", "", false)
    }
    fn other_host() -> Workspace {
        ws("ws-host-2", "pr-2", false)
    }
    fn archived(mut w: Workspace) -> Workspace {
        w.archived = true;
        w
    }

    fn pr(linked: &str) -> Pr {
        Pr {
            id: "pr-1".into(),
            workspace_id: linked.into(),
            ..Pr::default()
        }
    }

    fn set(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| (*s).to_owned()).collect()
    }

    fn counts(pairs: &[(&str, i64)]) -> HashMap<String, i64> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect()
    }

    #[test]
    fn agent_workspace_id() {
        let id = |p: &Pr, w: &[Workspace]| pr_agent_workspace_id(Some(p), w);
        assert_eq!(id(&pr("ws-linked"), &[host(), linked()]), "ws-host");
        assert_eq!(id(&pr(""), &[host()]), "ws-host");
        assert_eq!(id(&pr(""), &[archived(host())]), "");
        assert_eq!(
            id(&pr("ws-linked"), &[host(), archived(linked())]),
            "ws-host"
        );
        assert_eq!(
            id(&pr("ws-linked"), &[archived(host()), linked()]),
            "ws-linked"
        );
    }

    #[test]
    fn editable_workspace_id() {
        let id = |p: &Pr, w: &[Workspace]| pr_editable_workspace_id(Some(p), w);
        assert_eq!(id(&pr("ws-linked"), &[host(), linked()]), "ws-linked");
        assert_eq!(id(&pr("ws-host"), &[host()]), "");
        assert_eq!(id(&pr(""), &[host()]), "");
        assert_eq!(id(&pr("ws-gone"), &[host()]), "");
        assert_eq!(id(&pr("ws-linked"), &[host(), archived(linked())]), "");
    }

    #[test]
    fn agent_is_running() {
        let run = |p: &Pr, w: &[Workspace], r: &[&str]| pr_agent_is_running(Some(p), w, &set(r));
        assert!(run(&pr(""), &[host()], &["ws-host"]));
        assert!(run(&pr("ws-linked"), &[linked()], &["ws-linked"]));
        assert!(run(&pr("ws-linked"), &[host(), linked()], &["ws-host"]));
        assert!(!run(&pr(""), &[host()], &["ws-other"]));
        assert!(!run(&pr(""), &[host()], &[]));
        assert!(!run(&pr(""), &[archived(host())], &["ws-host"]));
        assert!(run(
            &pr("ws-linked"),
            &[archived(host()), linked()],
            &["ws-linked"]
        ));
    }

    #[test]
    fn host_running_count() {
        let n = |w: &[Workspace], r: &[&str]| pr_chat_host_running_count(w, &set(r));
        assert_eq!(
            n(&[host(), linked(), other_host()], &["ws-host", "ws-linked"]),
            1
        );
        assert_eq!(n(&[host(), linked()], &["ws-linked"]), 0);
        assert_eq!(n(&[host(), other_host()], &["ws-host", "ws-host-2"]), 2);
        assert_eq!(
            n(&[archived(host()), other_host()], &["ws-host", "ws-host-2"]),
            1
        );
        assert_eq!(n(&[host(), host()], &["ws-host"]), 1);
    }

    #[test]
    fn host_unseen_count() {
        let n = |w: &[Workspace], c: &[(&str, i64)]| pr_chat_host_unseen_count(w, &counts(c));
        let all = [host(), linked(), other_host()];
        assert_eq!(
            n(&all, &[("ws-host", 2), ("ws-host-2", 1), ("ws-linked", 9)]),
            3
        );
        assert_eq!(n(&[host(), linked()], &[("ws-linked", 4)]), 0);
        assert_eq!(
            n(
                &[archived(host()), other_host()],
                &[("ws-host", 2), ("ws-host-2", 1)]
            ),
            1
        );
        assert_eq!(n(&[host(), host()], &[("ws-host", 3)]), 3);
        assert_eq!(n(&[host(), other_host()], &[]), 0);
        assert_eq!(
            n(&[host(), other_host()], &[("ws-host", 2), ("ws-host-2", 1)]),
            3
        );
        assert_eq!(n(&[host(), other_host()], &[("ws-host-2", 1)]), 1);
    }

    #[test]
    fn pr_for_chat_workspace() {
        assert_eq!(
            pr_id_for_chat_workspace("ws-host", &[host(), linked()]),
            "pr-1"
        );
        assert_eq!(
            pr_id_for_chat_workspace("ws-linked", &[host(), linked()]),
            ""
        );
    }

    #[test]
    fn unseen_count() {
        let n =
            |p: &Pr, w: &[Workspace], c: &[(&str, i64)]| pr_unseen_count(Some(p), w, &counts(c));
        assert_eq!(n(&pr(""), &[host()], &[("ws-host", 2)]), 2);
        assert_eq!(
            n(
                &pr("ws-linked"),
                &[host(), linked()],
                &[("ws-linked", 1), ("ws-host", 9)]
            ),
            9
        );
        assert_eq!(n(&pr(""), &[host()], &[("ws-other", 3)]), 0);
        assert_eq!(n(&pr(""), &[host()], &[]), 0);
        assert_eq!(n(&pr(""), &[archived(host())], &[("ws-host", 2)]), 0);
        assert_eq!(
            n(
                &pr("ws-linked"),
                &[archived(host()), linked()],
                &[("ws-linked", 1), ("ws-host", 9)]
            ),
            1
        );
    }
}
