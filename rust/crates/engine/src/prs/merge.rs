//! Landing a PR (Go `pr.go` `MergePR`, `mergeLocal`, `markMerged` and the
//! commit message formats; `github.go` `mergeGitHub`). A Local PR merges
//! its branch into the base on disk and pushes a remote-tracking base; a
//! forge PR merges (or joins the merge queue) on the forge.

use std::path::Path;
use std::time::Duration;

use recur_domain::{
    Commit, MergeCommitMessage, MergeMethod, Pr, SquashCommitMessage, WorkspaceState,
    effective_merge_commit_message, effective_merge_method, effective_squash_commit_message,
    merge_method_permitted, normalize_merge_commit_message, normalize_merge_method,
    normalize_squash_commit_message,
};
use recur_forge::model::{format_commit_bullet_list, pr_description, single_line};
use recur_forge::{Forge, MergeCommitText, MergeOutcome};

use crate::Engine;
use crate::prs::{GROUP_MERGED, touch_pr_updated};
use crate::text::html_escape;

const MERGE_TIMEOUT: Duration = Duration::from_secs(60);

/// A squash commit: subject, then the commit list or the description by
/// `style`. A Local PR adds "Recur Local PR #n"; a forge PR's subject
/// carries `(#n)` and leaves an empty commit list to the forge.
pub fn format_squash_commit_message(
    title: &str,
    number: i64,
    forge: bool,
    commits: &[Commit],
    desc: &str,
    style: &SquashCommitMessage,
) -> MergeCommitText {
    let style = normalize_squash_commit_message(style.as_str());
    let subject = if forge && number > 0 {
        format!("{title} (#{number})")
    } else {
        title.to_owned()
    };
    let mut parts = Vec::new();
    let mut commit_list = String::new();
    match style {
        SquashCommitMessage::TitleAndCommits => {
            commit_list = format_commit_bullet_list(commits);
            if !commit_list.is_empty() {
                parts.push(commit_list.clone());
            }
        }
        SquashCommitMessage::TitleAndDescription => {
            let d = desc.trim();
            if !d.is_empty() {
                parts.push(d.to_owned());
            }
        }
        _ => {}
    }
    if !forge && number > 0 {
        parts.push(format!("Recur Local PR #{number}"));
    }
    MergeCommitText {
        subject,
        body: parts.join("\n\n"),
        omit_body: forge
            && style == SquashCommitMessage::TitleAndCommits
            && commit_list.is_empty(),
    }
}

/// A merge commit: `Merge Local PR #n: title` (or the forge's `title (#n)`),
/// with the description when `style` asks for it.
pub fn format_merge_commit_message(
    title: &str,
    number: i64,
    forge: bool,
    desc: &str,
    style: &MergeCommitMessage,
) -> MergeCommitText {
    let style = normalize_merge_commit_message(style.as_str());
    let subject = match (forge, number > 0) {
        (true, true) => format!("{title} (#{number})"),
        (false, true) => format!("Merge Local PR #{number}: {title}"),
        _ => title.to_owned(),
    };
    let body = if style == MergeCommitMessage::TitleAndDescription {
        desc.trim().to_owned()
    } else {
        String::new()
    };
    MergeCommitText {
        subject,
        body,
        omit_body: false,
    }
}

/// The on-disk commit message for a Local PR merge. Squash subjects are the
/// PR title (permanent history), merge commits keep a Merge prefix, rebase
/// carries none.
pub fn local_merge_commit_message(
    pr: &Pr,
    method: &MergeMethod,
    squash: &SquashCommitMessage,
    merge: &MergeCommitMessage,
) -> String {
    let mut title = single_line(&pr.title);
    if title.is_empty() {
        title = "Loop changes".into();
    }
    let commits = pr.commits.as_deref().unwrap_or_default();
    let text = match method {
        MergeMethod::Squash => format_squash_commit_message(
            &title,
            pr.number,
            false,
            commits,
            &pr_description(pr),
            squash,
        ),
        MergeMethod::Rebase => MergeCommitText::default(),
        _ => format_merge_commit_message(&title, pr.number, false, &pr_description(pr), merge),
    };
    text.combined()
}

/// Stamps the forge's last-activity time when known, else now.
pub(crate) fn stamp_reconcile_updated_at(p: &mut Pr, updated_at: &str) {
    let u = recur_forge::model::normalize_time(updated_at);
    if u.is_empty() {
        touch_pr_updated(p);
    } else {
        p.updated_at = u;
    }
}

impl Engine {
    /// Merges a PR. `method` empty resolves project, then settings. A
    /// merged PR is idempotent; a closed one is refused.
    pub async fn merge_pr(&self, pr_id: &str, method: &MergeMethod) -> (Pr, bool) {
        let Some(pr) = self.store.pr(pr_id) else {
            return (Pr::default(), false);
        };
        if pr.state == "Merged" {
            return (pr, true);
        }
        let forge = self.pr_forge(&pr);
        if pr.state == "Closed" {
            let label = forge.as_ref().map_or("Local", |f| f.label());
            self.emit_workspace_event(
                &pr.workspace_id,
                "MA",
                "Maestro",
                &format!("Merge refused: this {label} PR is closed."),
            );
            return (pr, false);
        }
        let method = if method.is_empty() {
            let proj = self
                .store
                .project(&self.pr_project_id(&pr))
                .unwrap_or_default();
            effective_merge_method(&proj, &self.settings())
        } else {
            normalize_merge_method(method.as_str())
        };
        match forge {
            Some(f) => self.merge_forge(f.as_ref(), pr, &method).await,
            None => self.merge_local(pr, &method).await,
        }
    }

    /// The project's squash and merge message styles for a workspace.
    pub(crate) fn merge_message_styles(
        &self,
        workspace_id: &str,
    ) -> (SquashCommitMessage, MergeCommitMessage) {
        let proj = self
            .store
            .project(&self.loop_project_id(workspace_id))
            .unwrap_or_default();
        let set = self.settings();
        (
            effective_squash_commit_message(&proj, &set),
            effective_merge_commit_message(&proj, &set),
        )
    }

    /// Merges the branch into its base in the project repo and pushes a
    /// remote-tracking base. A failure is a feed note and `false`.
    async fn merge_local(&self, mut pr: Pr, method: &MergeMethod) -> (Pr, bool) {
        let Some(proj) = self
            .store
            .project(&self.pr_project_id(&pr))
            .filter(|p| !p.repo_path.is_empty())
        else {
            return (self.mark_merged(&pr.id, "").await, true);
        };
        // A mid-session rename wins over the cached branch name.
        self.sync_workspace_branch(&pr.workspace_id).await;
        if let Some(refreshed) = self.store.pr(&pr.id) {
            pr = refreshed;
        }
        let mgr = recur_git::Manager::new(&proj.repo_path, &self.worktree_root);
        let base = if pr.base.is_empty() {
            proj.base_branch.clone()
        } else {
            pr.base.clone()
        };
        let (sq, mg) = self.merge_message_styles(&pr.workspace_id);
        let msg = local_merge_commit_message(&pr, method, &sq, &mg);
        let landed = tokio::time::timeout(MERGE_TIMEOUT, async {
            if let Err(e) = mgr.merge(&pr.branch, &base, &msg, method.as_str()).await {
                let mut note = format!("Local merge failed: {}", html_escape(&e.to_string()));
                if e.dirty_base().is_none() {
                    note.push_str(" — resolve the conflict and try again.");
                }
                return Err(note);
            }
            let tracking = mgr.parse_remote_tracking(&base).await;
            if let Some((remote, mut short)) = tracking.clone() {
                if short.is_empty() {
                    short = mgr.short_branch_name(&base).await;
                }
                if let Err(e) = mgr.push_branch(&remote, &short).await {
                    return Err(format!(
                        "Landed on local <code>{}</code> but could not push to <code>{}</code>: {}",
                        html_escape(&short),
                        html_escape(&remote),
                        html_escape(&e.to_string())
                    ));
                }
                return Ok(Some((remote, short)));
            }
            Ok(None)
        })
        .await
        .unwrap_or_else(|_| {
            Err(format!(
                "Local merge failed: {} — resolve the conflict and try again.",
                html_escape("context deadline exceeded")
            ))
        });
        let remote = match landed {
            Ok(r) => r,
            Err(note) => {
                self.emit_workspace_event(&pr.workspace_id, "MA", "Maestro", &note);
                return (pr, false);
            }
        };
        let label = method.label();
        let merged = self.mark_merged(&pr.id, "").await;
        let note = match remote {
            Some((remote, short)) => format!(
                "{}: Local PR <code>#{}</code> to <code>{}/{}</code>.",
                html_escape(label),
                pr.number,
                html_escape(&remote),
                html_escape(&short)
            ),
            None => format!(
                "{}: Local PR <code>#{}</code> into <code>{}</code> on disk — you were the only gate.",
                html_escape(label),
                pr.number,
                html_escape(&base)
            ),
        };
        self.emit_workspace_event(&pr.workspace_id, "MA", "Maestro", &note);
        (merged, true)
    }

    /// Merges on the forge, or enqueues when the base uses a merge queue
    /// (the PR then stays open and queued).
    async fn merge_forge(&self, forge: &dyn Forge, pr: Pr, method: &MergeMethod) -> (Pr, bool) {
        let repo = self
            .store
            .project(&self.pr_project_id(&pr))
            .map(|p| p.repo_path)
            .unwrap_or_default();
        if !merge_method_permitted(&pr.allowed_merge_methods, method) {
            self.emit_workspace_event(
                &pr.workspace_id,
                "MA",
                "Maestro",
                &format!(
                    "{} doesn't allow {} on this repository.",
                    forge.label(),
                    html_escape(method.label())
                ),
            );
            return (pr, false);
        }
        let outcome =
            crate::prs::ops::bounded(MERGE_TIMEOUT, forge.merge(Path::new(&repo), &pr, method))
                .await;
        match outcome {
            Err(e) => {
                self.emit_workspace_event(
                    &pr.workspace_id,
                    "MA",
                    "Maestro",
                    &forge.merge_failure_note(&e),
                );
                (pr, false)
            }
            Ok(MergeOutcome::Queued) => {
                let pr = self
                    .store
                    .update_pr(&pr.id, |p| {
                        p.merge_queue_enabled = true;
                        p.merge_queued = true;
                    })
                    .unwrap_or(pr);
                self.publish_pr_updated(&pr.id);
                self.schedule_refresh_pr_conversation(&pr.id);
                self.emit_workspace_event(
                    &pr.workspace_id,
                    "MA",
                    "Maestro",
                    &format!(
                        "Added {} PR <code>#{}</code> to the merge queue.",
                        forge.label(),
                        pr.number
                    ),
                );
                (pr, true)
            }
            Ok(MergeOutcome::Merged) => {
                let merged = self.mark_merged(&pr.id, "").await;
                self.prs.conv_last.lock().remove(&pr.id);
                self.schedule_refresh_pr_conversation(&pr.id);
                self.emit_workspace_event(
                    &pr.workspace_id,
                    "MA",
                    "Maestro",
                    &format!(
                        "{}: {} PR <code>#{}</code> via {}.",
                        html_escape(method.label()),
                        forge.label(),
                        pr.number,
                        pr.source.as_str()
                    ),
                );
                (merged, true)
            }
        }
    }

    /// Flips a PR and its workspace to merged, clears its inbox row, drops
    /// unmerged siblings and archives leftover chat hosts. `updated_at` is
    /// the forge's last-activity stamp when known ("" = now).
    pub(crate) async fn mark_merged(&self, pr_id: &str, updated_at: &str) -> Pr {
        let Some(merged) = self.store.update_pr(pr_id, |p| {
            p.state = "Merged".into();
            p.group = GROUP_MERGED.into();
            stamp_reconcile_updated_at(p, updated_at);
        }) else {
            return Pr::default();
        };
        self.set_workspace_state(&merged.workspace_id, WorkspaceState::Merged, "merged");
        // The human acted: the "ready for you" row is no longer actionable.
        self.clear_pr_inbox(pr_id);
        self.publish_pr_updated(pr_id);
        self.drop_unmerged_sibling_prs(&merged.workspace_id, &merged.id);
        let keep = self.live_pr_workspace_id(&merged.workspace_id);
        self.archive_pr_chat_hosts(pr_id, &keep).await;
        merged
    }
}
