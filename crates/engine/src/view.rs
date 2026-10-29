//! The UI's pure workspace rules (removal, waiting for a worktree, inbox
//! read state), so the GPUI views do not re-derive them. The submodules hold
//! the chat-surface rules (transcript, live sessions, artifacts, prompt
//! actions and the composer's text model), the loop pickers and cockpit, the
//! chat modes, looking-at surface, active-tab memory, subagent display and
//! launch rules, and the PR rules (merge action, PR workspace, comment
//! threads, resolve holds, detail skeletons, fix checks).

pub mod artifacts;
pub mod chat;
pub mod chat_surface;
pub mod chat_tab_session;
pub mod comment_thread;
pub mod composer;
pub mod digest;
pub mod js;
pub mod live;
pub mod loop_cockpit;
pub mod loops;
pub mod mentions;
pub mod modes;
pub mod new_workspace;
pub mod pr_detail_pending;
pub mod pr_fix_checks;
pub mod pr_merge_action;
pub mod pr_thread_resolved;
pub mod pr_workspace;
pub mod prompts;
pub mod subagent;

use recur_domain::{InboxItem, Workspace};

/// The optimistic local copy of a workspace being removed: archived, with
/// its worktree and port released (matches what Remove stores).
pub fn mark_removed(w: &Workspace) -> Workspace {
    Workspace {
        archived: true,
        worktree_path: String::new(),
        port: 0,
        ..w.clone()
    }
}

/// The success toast after removing `n` workspaces ("" for none).
pub fn removed_toast(n: usize) -> String {
    match n {
        0 => String::new(),
        1 => "Workspace removed".to_owned(),
        n => format!("{n} workspaces removed"),
    }
}

/// The ids a bulk remove acts on: non-empty, first occurrence order.
pub fn unique_remove_ids(ids: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in ids {
        if !id.is_empty() && !out.contains(id) {
            out.push(id.clone());
        }
    }
    out
}

/// Still creating: no error, and phase "creating" or a slot wait without a
/// worktree (`wsCreating`).
pub fn ws_creating(w: &Workspace) -> bool {
    w.last_error.is_empty()
        && (w.phase == "creating"
            || (w.phase == "waiting for a slot" && w.worktree_path.is_empty()))
}

/// Where a wait for `id`'s worktree stands (`waitForWorktree`'s `current`).
#[derive(Debug, Clone, PartialEq)]
pub enum WorktreeWait {
    /// Create/setup finished: the workspace with its path.
    Ready(Box<Workspace>),
    /// Gone, failed, or aborted: stop waiting.
    Gone,
    /// Keep waiting.
    Pending,
}

/// Classifies `id` in the current list.
pub fn worktree_wait(workspaces: &[Workspace], id: &str, aborted: bool) -> WorktreeWait {
    if aborted {
        return WorktreeWait::Gone;
    }
    let Some(w) = workspaces.iter().find(|w| w.id == id) else {
        return WorktreeWait::Gone;
    };
    if !w.last_error.is_empty() || w.phase == "failed" {
        return WorktreeWait::Gone;
    }
    if ws_creating(w) {
        return WorktreeWait::Pending;
    }
    if !w.worktree_path.trim().is_empty() {
        return WorktreeWait::Ready(Box::new(w.clone()));
    }
    WorktreeWait::Pending
}

/// Opening a workspace's Briefing card marks its unread notifications read
/// (Gmail semantics): the ids to mark through the inbox service.
pub fn workspace_unread_ids(inbox: &[InboxItem], workspace_id: &str) -> Vec<String> {
    if workspace_id.is_empty() {
        return Vec::new();
    }
    inbox
        .iter()
        .filter(|i| i.workspace_id == workspace_id && i.unread)
        .map(|i| i.id.clone())
        .collect()
}

/// The optimistic inbox after marking a workspace's rows read.
pub fn mark_workspace_read(inbox: &[InboxItem], workspace_id: &str) -> Vec<InboxItem> {
    inbox
        .iter()
        .map(|i| {
            if i.workspace_id == workspace_id {
                InboxItem {
                    unread: false,
                    ..i.clone()
                }
            } else {
                i.clone()
            }
        })
        .collect()
}

/// The optimistic inbox after marking one row read.
pub fn mark_item_read(inbox: &[InboxItem], id: &str) -> Vec<InboxItem> {
    inbox
        .iter()
        .map(|i| {
            if i.id == id {
                InboxItem {
                    unread: false,
                    ..i.clone()
                }
            } else {
                i.clone()
            }
        })
        .collect()
}

/// The optimistic inbox after Mark all read.
pub fn mark_all_read(inbox: &[InboxItem]) -> Vec<InboxItem> {
    inbox
        .iter()
        .map(|i| InboxItem {
            unread: false,
            ..i.clone()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(id: &str, f: impl FnOnce(&mut Workspace)) -> Workspace {
        let mut w = Workspace {
            id: id.into(),
            project_id: "p".into(),
            ..Workspace::default()
        };
        f(&mut w);
        w
    }

    #[test]
    fn dropped_record_settles_null_even_if_a_replacement_is_ready() {
        let list = vec![ws("new", |w| w.worktree_path = "/tmp/wt".into())];
        assert_eq!(worktree_wait(&list, "old", false), WorktreeWait::Gone);
    }

    #[test]
    fn waits_while_published_path_is_still_creating() {
        let creating = vec![ws("WS-NEW", |w| {
            w.worktree_path = "/tmp/wt".into();
            w.phase = "creating".into();
        })];
        assert_eq!(
            worktree_wait(&creating, "WS-NEW", false),
            WorktreeWait::Pending
        );
        let ready = vec![ws("WS-NEW", |w| {
            w.worktree_path = "/tmp/wt".into();
            w.phase = "editing".into();
        })];
        match worktree_wait(&ready, "WS-NEW", false) {
            WorktreeWait::Ready(w) => assert_eq!(w.worktree_path, "/tmp/wt"),
            other => panic!("want ready, got {other:?}"),
        }
    }

    #[test]
    fn setup_failure_after_publish_is_null() {
        let failed = vec![ws("WS-NEW", |w| {
            w.worktree_path = "/tmp/wt".into();
            w.phase = "failed".into();
            w.last_error = "setup command failed".into();
        })];
        assert_eq!(worktree_wait(&failed, "WS-NEW", false), WorktreeWait::Gone);
        assert_eq!(worktree_wait(&failed, "WS-NEW", true), WorktreeWait::Gone);
    }

    #[test]
    fn slot_wait_with_tree_is_not_creating() {
        let w = ws("a", |w| {
            w.phase = "waiting for a slot".into();
            w.worktree_path = "/x".into();
        });
        assert!(!ws_creating(&w));
        assert!(ws_creating(
            &ws("a", |w| w.phase = "waiting for a slot".into())
        ));
        assert!(!ws_creating(&ws("a", |w| {
            w.phase = "creating".into();
            w.last_error = "boom".into();
        })));
    }

    // Workspace removal
    #[test]
    fn mark_removed_releases_tree_and_port() {
        let w = ws("a", |w| {
            w.worktree_path = "/tmp/a".into();
            w.port = 1;
        });
        let r = mark_removed(&w);
        assert!(r.archived);
        assert_eq!(r.worktree_path, "");
        assert_eq!(r.port, 0);
        assert_eq!(r.id, "a");
    }

    #[test]
    fn toast_and_ids() {
        assert_eq!(removed_toast(0), "");
        assert_eq!(removed_toast(1), "Workspace removed");
        assert_eq!(removed_toast(2), "2 workspaces removed");
        let ids = vec!["a".into(), "".into(), "b".into(), "a".into()];
        assert_eq!(
            unique_remove_ids(&ids),
            vec!["a".to_owned(), "b".to_owned()]
        );
    }

    #[test]
    fn inbox_read_rules() {
        let inbox = vec![
            InboxItem {
                id: "1".into(),
                workspace_id: "w".into(),
                unread: true,
                ..InboxItem::default()
            },
            InboxItem {
                id: "2".into(),
                workspace_id: "w".into(),
                unread: false,
                ..InboxItem::default()
            },
            InboxItem {
                id: "3".into(),
                workspace_id: "x".into(),
                unread: true,
                ..InboxItem::default()
            },
        ];
        assert_eq!(workspace_unread_ids(&inbox, "w"), vec!["1".to_owned()]);
        assert!(workspace_unread_ids(&inbox, "").is_empty());
        let after = mark_workspace_read(&inbox, "w");
        assert!(!after[0].unread && after[2].unread);
        let after = mark_item_read(&inbox, "3");
        assert!(after[0].unread && !after[2].unread);
        assert!(mark_all_read(&inbox).iter().all(|i| !i.unread));
    }
}
