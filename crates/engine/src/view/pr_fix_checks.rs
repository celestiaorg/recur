//! `/fix-checks` from the PR checks tab:
//! run it in the PR's Agent-capable worktree, or create one from the PR
//! first and fire the queued request once it is ready.

use recur_domain::{Pr, Workspace};

use super::pr_workspace::pr_editable_workspace_id;

/// A `/fix-checks` waiting for CreateWorkspaceFromPR to finish allocating.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingFixChecks {
    pub pr_id: String,
    pub args: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixChecksStep {
    /// Run `/fix-checks` in this workspace now.
    Run(String),
    /// Queue it and call CreateWorkspaceFromPR.
    CreateWorkspace(PendingFixChecks),
}

/// The first step of a fix-checks request (`None` for a blank PR id). The UI
/// also switches the PR detail to the checks tab and opens the agent dock.
pub fn request_fix_checks(
    pr_id: &str,
    args: &str,
    prs: &[Pr],
    workspaces: &[Workspace],
) -> Option<FixChecksStep> {
    let id = pr_id.trim();
    if id.is_empty() {
        return None;
    }
    let ws = pr_editable_workspace_id(prs.iter().find(|p| p.id == id), workspaces);
    Some(if ws.is_empty() {
        FixChecksStep::CreateWorkspace(PendingFixChecks {
            pr_id: id.to_owned(),
            args: args.to_owned(),
        })
    } else {
        FixChecksStep::Run(ws)
    })
}

/// A workspace CreateWorkspaceFromPR returned is a real worktree (not a chat
/// host), so the PR row should link to it.
pub fn created_workspace_ready(ws: &Workspace) -> bool {
    !ws.worktree_path.trim().is_empty() && ws.host_pr_id.trim().is_empty()
}

/// Takes the queued request once its PR has an Agent-capable worktree:
/// `(workspace id, args)` to run.
pub fn flush_pending_fix_checks(
    pending: &mut Option<PendingFixChecks>,
    prs: &[Pr],
    workspaces: &[Workspace],
) -> Option<(String, String)> {
    let p = pending.as_ref()?;
    let ws = pr_editable_workspace_id(prs.iter().find(|pr| pr.id == p.pr_id), workspaces);
    if ws.is_empty() {
        return None;
    }
    pending.take().map(|p| (ws, p.args))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(linked: &str) -> Pr {
        Pr {
            id: "pr-1".into(),
            workspace_id: linked.into(),
            ..Pr::default()
        }
    }

    fn ws(id: &str, host: &str, path: &str) -> Workspace {
        Workspace {
            id: id.into(),
            host_pr_id: host.into(),
            worktree_path: path.into(),
            ..Workspace::default()
        }
    }

    #[test]
    fn runs_in_the_linked_worktree_or_creates_one() {
        let linked = [ws("ws-1", "", "/w")];
        assert_eq!(
            request_fix_checks(" pr-1 ", "ci", &[pr("ws-1")], &linked),
            Some(FixChecksStep::Run("ws-1".into()))
        );
        let host = [ws("ws-host", "pr-1", "")];
        let want = PendingFixChecks {
            pr_id: "pr-1".into(),
            args: "ci".into(),
        };
        assert_eq!(
            request_fix_checks("pr-1", "ci", &[pr("")], &host),
            Some(FixChecksStep::CreateWorkspace(want))
        );
        assert_eq!(request_fix_checks("  ", "", &[], &[]), None);
    }

    #[test]
    fn flushes_once_the_worktree_is_linked() {
        let mut pending = Some(PendingFixChecks {
            pr_id: "pr-1".into(),
            args: "ci".into(),
        });
        assert_eq!(flush_pending_fix_checks(&mut pending, &[pr("")], &[]), None);
        assert!(pending.is_some());
        let created = ws("ws-1", "", "/w");
        assert!(created_workspace_ready(&created));
        assert!(!created_workspace_ready(&ws("ws-h", "pr-1", "/w")));
        assert_eq!(
            flush_pending_fix_checks(&mut pending, &[pr("ws-1")], &[created]),
            Some(("ws-1".into(), "ci".into()))
        );
        assert!(pending.is_none());
    }
}
