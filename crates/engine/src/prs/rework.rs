//! Sending a PR back to its loop and updating a PR's worktree with its base.

use std::path::Path;
use std::time::Duration;

use recur_domain::{
    DiffStat, Pr, PrComment, UPDATE_WORKSPACE_CONFLICT, UPDATE_WORKSPACE_UPDATED, UpdateBaseMethod,
    UpdateWorkspaceResult, Workspace, effective_update_base_method, normalize_update_base_method,
};

use crate::Engine;
use crate::codes::{MAESTRO_AGENT_CODE, loop_owner_code};
use crate::core::{EngineError, Result};
use crate::prs::ops::{FORGE_WRITE_TIMEOUT, bounded, pending_to_comment};
use crate::prs::{none_if_empty, now_comment};
use crate::text::html_escape;

/// The update-with-base failure for a PR with no usable loop.
pub const ERR_PR_UNAVAILABLE: &str = "update-with-base failed: the PR or its loop is unavailable";

const UPDATE_TIMEOUT: Duration = Duration::from_secs(60);
const FINISH_TIMEOUT: Duration = Duration::from_secs(30);

/// The conductor's user turn from the send-back notes.
pub fn distill_instruction(comments: &[String]) -> String {
    let notes: Vec<&str> = comments
        .iter()
        .map(|c| c.trim())
        .filter(|c| !c.is_empty())
        .collect();
    match notes.as_slice() {
        [] => "Address the reviewer's feedback on this PR with minimal, correct edits.".into(),
        [one] => format!("Address this review comment: {one}"),
        many => format!(
            "Address the following review comments:\n- {}",
            many.join("\n- ")
        ),
    }
}

impl Engine {
    /// The PR Send Back control: flushes pending line notes, puts the
    /// review on the thread (and so in conductor context) and starts a
    /// conductor turn that answers or calls reenter_phase.
    pub async fn send_back_to_loop(&self, pr_id: &str, comments: &[String]) -> (Pr, bool) {
        let Some(mut pr) = self.store.pr(pr_id) else {
            return (Pr::default(), false);
        };
        if pr.state == "Merged" || pr.group == "merged" {
            return (pr, false);
        }
        let send_back_notes = comments.to_vec();
        let mut all = comments.to_vec();
        let pending = pr.pending_comments.clone();
        if !pending.is_empty() {
            all.extend(
                pending
                    .iter()
                    .filter(|pc| !pc.body.trim().is_empty())
                    .map(|pc| pc.body.clone()),
            );
            if let Some(forge) = self.pr_forge(&pr) {
                let repo = self.pr_repo_path(&pr);
                let res = bounded(
                    FORGE_WRITE_TIMEOUT,
                    forge.submit_review(Path::new(&repo), pr.number, "comment", "", &pending),
                )
                .await;
                if let Err(e) = res {
                    self.emit_workspace_event(
                        &pr.workspace_id,
                        "MA",
                        "Maestro",
                        &format!(
                            "Could not submit the review to {}: {}",
                            forge.label(),
                            html_escape(e.message.trim())
                        ),
                    );
                    return (pr, false);
                }
            }
            let mut flushed = Vec::new();
            if let Some(updated) = self.store.update_pr(pr_id, |p| {
                let files = p.files.clone().unwrap_or_default();
                flushed = p
                    .pending_comments
                    .iter()
                    .map(|pc| pending_to_comment(pc, &files))
                    .collect();
                p.comments
                    .get_or_insert_with(Vec::new)
                    .extend(flushed.iter().cloned());
                p.pending_comments.clear();
            }) {
                pr = updated;
                for c in &flushed {
                    self.publish_pr_comment(pr_id, c);
                }
                self.resolve_pending_findings(&pr, &pending);
            }
        }

        // Later conductor turns read the notes from the thread too.
        let mut posted: Vec<PrComment> = Vec::new();
        if let Some(updated) = self.store.update_pr(pr_id, |p| {
            for body in &send_back_notes {
                let body = body.trim();
                if body.is_empty() {
                    continue;
                }
                let mut c = now_comment("AR", "You", &html_escape(body));
                c.body_md = body.to_owned();
                p.comments.get_or_insert_with(Vec::new).push(c.clone());
                posted.push(c);
            }
        }) {
            pr = updated;
            for c in &posted {
                self.publish_pr_comment(pr_id, c);
            }
        }

        let instruction = distill_instruction(&all);
        let Some(ws) = self.store.workspace(&pr.workspace_id) else {
            return (pr, false);
        };
        let mut code = loop_owner_code(&ws);
        if code.is_empty() {
            code = MAESTRO_AGENT_CODE.into();
        }
        let mut text = "The human sent this PR's review back to you.".to_owned();
        if !instruction.is_empty() {
            text.push_str("\n\n");
            text.push_str(&instruction);
        }
        text.push_str("\n\nThe conversation and line notes are in your loop context. Answer if this is a question. If it is work, call reenter_phase on an allowed phase (usually converge).");
        let e = self.arc();
        let ws_id = pr.workspace_id.clone();
        self.tasks.chat_turns.spawn(async move {
            e.send_maestro_message_to(&ws_id, &code, &text, &[], "Sent review back")
                .await;
        });
        self.clear_pr_inbox(pr_id);
        self.publish_pr_updated(pr_id);
        (pr, true)
    }

    /// The conductor's `send_back`: the same Send Back on the workspace's
    /// open PR. Returns the tool result text.
    pub async fn maestro_send_back(&self, workspace_id: &str, instruction: &str) -> String {
        let instruction = instruction.trim();
        if instruction.is_empty() {
            return "send_back refused: instruction is empty.".into();
        }
        let Some(pr) = self.store.open_pr_by_workspace(workspace_id) else {
            return "send_back refused: this loop has no open PR — use plan to steer while it's still running.".into();
        };
        if pr.state == "Merged" || pr.group == "merged" {
            return "send_back refused: the PR is already merged.".into();
        }
        if !self
            .send_back_to_loop(&pr.id, &[instruction.to_owned()])
            .await
            .1
        {
            return "send_back failed — the PR or its loop is unavailable.".into();
        }
        "Sent review back — Maestro will answer or call reenter_phase.".into()
    }

    /// Integrates the latest base tip into the PR's worktree: a clean
    /// merge/rebase, or markers left for /update (status conflict). The PR
    /// stays In review; `updating_base` ("git") shows progress meanwhile.
    pub async fn update_pr_with_base(
        &self,
        pr_id: &str,
        method: &str,
    ) -> Result<UpdateWorkspaceResult> {
        let unavailable = || EngineError::new(ERR_PR_UNAVAILABLE);
        let pr = self.store.pr(pr_id).ok_or_else(unavailable)?;
        if pr.state == "Merged" || pr.group == "merged" {
            return Err(unavailable());
        }
        if !pr.updating_base.is_empty() {
            // Already in flight.
            return Ok(UpdateWorkspaceResult::default());
        }
        let lp = self
            .store
            .workspace(&pr.workspace_id)
            .filter(|w| !w.worktree_path.is_empty())
            .ok_or_else(unavailable)?;
        let proj = self
            .store
            .project(&lp.project_id)
            .filter(|p| !p.repo_path.is_empty())
            .ok_or_else(unavailable)?;
        let mut base = [&pr.base, &lp.base_branch, &proj.base_branch]
            .into_iter()
            .find(|b| !b.is_empty())
            .cloned()
            .ok_or_else(unavailable)?;
        let resolved = if method.trim().is_empty() {
            effective_update_base_method(&proj, &self.settings())
        } else {
            normalize_update_base_method(method)
        };
        let mgr = recur_git::Manager::new(&proj.repo_path, &self.worktree_root);
        if let Ok(Ok((r, _))) = tokio::time::timeout(
            recur_git::REMOTE_FETCH_TIMEOUT,
            mgr.behind_base(&lp.worktree_path, &base, true),
        )
        .await
            && !r.is_empty()
        {
            base = r;
        }
        tokio::time::timeout(
            UPDATE_TIMEOUT,
            self.update_pr_with_base_inner(pr_id, lp, &mgr, base, resolved),
        )
        .await
        .unwrap_or_else(|_| Err(EngineError::new("context deadline exceeded")))
    }

    async fn update_pr_with_base_inner(
        &self,
        pr_id: &str,
        lp: Workspace,
        mgr: &recur_git::Manager,
        base: String,
        resolved: UpdateBaseMethod,
    ) -> Result<UpdateWorkspaceResult> {
        let result = |status: &str, lp: Workspace| UpdateWorkspaceResult {
            status: status.into(),
            workspace: lp,
            base: base.clone(),
            ..UpdateWorkspaceResult::default()
        };
        if mgr.update_in_progress(&lp.worktree_path).await {
            return Ok(result(UPDATE_WORKSPACE_CONFLICT, lp));
        }
        // The git phase drives the PR view's Updating banner.
        if self
            .store
            .update_pr(pr_id, |p| p.updating_base = "git".into())
            .is_some()
        {
            self.publish_pr_updated(pr_id);
        }
        // update_worktree_with_base aborts on conflict; begin reruns it
        // leaving markers.
        let mut res = mgr
            .update_worktree_with_base(&lp.worktree_path, &base, resolved.as_str())
            .await;
        if res.is_err() {
            res = mgr
                .begin_update_with_base(&lp.worktree_path, &base, resolved.as_str())
                .await;
        }
        let err = match res {
            Ok(()) => {
                if !self
                    .finish_update_with_base(pr_id, &lp, mgr, &base, &resolved)
                    .await
                {
                    return Err(EngineError::new(ERR_PR_UNAVAILABLE));
                }
                return Ok(result(UPDATE_WORKSPACE_UPDATED, lp));
            }
            Err(e) => e,
        };
        if self
            .store
            .update_pr(pr_id, |p| p.updating_base.clear())
            .is_some()
        {
            self.publish_pr_updated(pr_id);
        }
        if !mgr.update_in_progress(&lp.worktree_path).await {
            return Err(EngineError(format!("update with {base}: {err}")));
        }
        self.emit_workspace_event(
            &lp.id,
            "MA",
            "Maestro",
            &format!(
                "Update with base ({}) hit conflicts. Resolving them with /update.",
                html_escape(resolved.label())
            ),
        );
        Ok(result(UPDATE_WORKSPACE_CONFLICT, lp))
    }

    /// Refreshes the PR's diff and commits after a clean base integration
    /// and clears `behind_base`. A Local PR gets a thread note (a forge PR
    /// shows the real commits, and the note would flicker on the next
    /// thread sync).
    async fn finish_update_with_base(
        &self,
        pr_id: &str,
        lp: &Workspace,
        mgr: &recur_git::Manager,
        base: &str,
        method: &UpdateBaseMethod,
    ) -> bool {
        let (files, add, del, commits) = tokio::time::timeout(FINISH_TIMEOUT, async {
            let (mut files, mut add, mut del) = (Vec::new(), 0, 0);
            if let Ok(d) = mgr.diff_pr(&lp.worktree_path, base).await {
                (files, add, del) = (d.files, d.add, d.del);
            }
            let mut commits = Vec::new();
            if let Ok(cs) = mgr.log_commits(&lp.worktree_path, base).await {
                commits = cs;
                for c in &mut commits {
                    if let Ok(cf) = mgr.commit_diff(&lp.worktree_path, &c.hash).await {
                        c.files = Some(cf);
                    }
                    if c.when.is_empty() {
                        c.when = "now".into();
                    }
                }
            }
            (files, add, del, commits)
        })
        .await
        .unwrap_or_default();
        let label = method.label();
        let post_note = self
            .store
            .pr(pr_id)
            .is_none_or(|cur| self.pr_forge(&cur).is_none());
        let note = post_note.then(|| {
            now_comment(
                "MA",
                "Maestro",
                &format!("Updated with latest base ({}).", html_escape(label)),
            )
        });
        let updated = self.store.update_pr(pr_id, |p| {
            p.file_count = files.len() as i64;
            p.files = none_if_empty(files);
            p.diff = DiffStat { add, del };
            p.commits = none_if_empty(commits);
            p.behind_base = 0;
            p.updating_base.clear();
            if let Some(n) = &note {
                p.comments.get_or_insert_with(Vec::new).push(n.clone());
            }
        });
        if updated.is_none() {
            return false;
        }
        if let Some(n) = &note {
            self.publish_pr_comment(pr_id, n);
        }
        self.publish_pr_updated(pr_id);
        self.emit_workspace_event(
            &lp.id,
            "MA",
            "Maestro",
            &format!(
                "Updated PR with latest <code>{}</code> ({}).",
                html_escape(base),
                html_escape(label)
            ),
        );
        true
    }
}
