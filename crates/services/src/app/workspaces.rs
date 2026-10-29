//! `WorkspacesService`: workspace lifecycle, chat tabs, loops, plans, land and
//! remote actions over the engine. `OpenPath` is in `open_path.rs`.

use std::collections::BTreeMap;

use async_trait::async_trait;
use recur_domain::{
    AttentionCounts, AutonomyLevel, CHAT_MODE_LOOP, ChatAttachment, ChatItem, Commit, FeedItem,
    IssueRef, MergeMethod, OpenInTarget, OpenPathResult, SessionView, UpdateBaseMethod,
    UpdateWorkspaceResult, Workspace, WorkspaceChatTab, WorkspaceLandState, normalize_chat_mode,
};

use super::rt::rt;
use super::{AppServices, err};
use crate::api::{WorkspaceChatSelection, WorkspacesService};
use crate::{Error, Result};

fn not_found(id: &str) -> Error {
    Error::new(format!("workspace {:?} not found", id))
}

impl AppServices {
    /// Empty loop / policy ids fall back to Settings, then the shipped
    /// defaults.
    fn resolve_engage_ids(
        &self,
        loop_id: &str,
        policy_id: &str,
        autonomy: &str,
    ) -> (String, String) {
        let set = self.store().settings();
        let mut loop_id = loop_id.trim().to_owned();
        let mut policy_id = policy_id.trim().to_owned();
        if loop_id.is_empty() {
            loop_id = set.default_loop.trim().to_owned();
        }
        if loop_id.is_empty() {
            loop_id = recur_bundles::DEFAULT_LOOP_ID.into();
        }
        if policy_id.is_empty() {
            policy_id = set.default_policy.trim().to_owned();
        }
        if policy_id.is_empty() {
            policy_id = recur_bundles::policy_from_autonomy(autonomy).into();
        }
        (loop_id, policy_id)
    }

    #[allow(clippy::too_many_arguments)]
    async fn create_workspace_with_at(
        &self,
        project_id: &str,
        name: &str,
        base_branch: &str,
        mode: &str,
        prompt: &str,
        autonomy: &str,
        loop_id: &str,
        policy_id: &str,
        simplify: Option<bool>,
        attachments: &[ChatAttachment],
    ) -> Result<Workspace> {
        if normalize_chat_mode(mode) == CHAT_MODE_LOOP {
            let (loop_id, policy_id) = self.resolve_engage_ids(loop_id, policy_id, autonomy);
            return self
                .engine
                .begin_start_workspace_with(
                    project_id,
                    prompt,
                    name,
                    base_branch,
                    AutonomyLevel::from(autonomy),
                    &loop_id,
                    &policy_id,
                    simplify,
                    attachments,
                )
                .await
                .map_err(err);
        }
        let ws = self
            .engine
            .begin_create_workspace(project_id, name, base_branch)
            .await
            .map_err(err)?;
        if !loop_id.trim().is_empty() || !policy_id.trim().is_empty() {
            return self.set_workspace_loop_policy_at(&ws.id, loop_id, policy_id);
        }
        Ok(ws)
    }

    fn set_workspace_loop_policy_at(
        &self,
        workspace_id: &str,
        loop_id: &str,
        policy_id: &str,
    ) -> Result<Workspace> {
        let ws = self
            .store()
            .workspace(workspace_id)
            .ok_or_else(|| not_found(workspace_id))?;
        if ws.has_run() {
            return Err(Error::new(
                "loop and policy are pinned after the run starts — restart to switch",
            ));
        }
        let (loop_id, policy_id) = (loop_id.trim().to_owned(), policy_id.trim().to_owned());
        if !loop_id.is_empty() || !policy_id.is_empty() {
            let reason = self.engine.guard_reason(&loop_id, &policy_id, &ws.autonomy);
            if !reason.is_empty() {
                return Err(Error::new(reason));
            }
        }
        self.store()
            .update_workspace(workspace_id, |w| {
                if !loop_id.is_empty() {
                    w.loop_id = loop_id;
                }
                if !policy_id.is_empty() {
                    w.policy_id = policy_id;
                }
            })
            .ok_or_else(|| not_found(workspace_id))
    }

    async fn engage_loop_with_at(
        &self,
        workspace_id: &str,
        goal: &str,
        loop_id: &str,
        policy_id: &str,
    ) -> String {
        let (mut loop_id, mut policy_id) = (loop_id.to_owned(), policy_id.to_owned());
        let mut autonomy = String::new();
        if let Some(ws) = self.store().workspace(workspace_id) {
            if loop_id.trim().is_empty() {
                loop_id = ws.loop_id;
            }
            if policy_id.trim().is_empty() {
                policy_id = ws.policy_id;
            }
            autonomy = ws.autonomy.as_str().to_owned();
        }
        let (loop_id, policy_id) = self.resolve_engage_ids(&loop_id, &policy_id, &autonomy);
        match self
            .engine
            .start_run_on_workspace_with(workspace_id, goal, &loop_id, &policy_id)
            .await
        {
            Err(e) => format!("Refused: {e}"),
            Ok(true) => "Loop queued — it starts when a slot frees.".into(),
            Ok(false) => "Loop engaged — planning has started.".into(),
        }
    }

    async fn set_workspace_issue_at(
        &self,
        workspace_id: &str,
        issue: IssueRef,
    ) -> Result<Workspace> {
        if issue.source.is_empty() || issue.id.is_empty() {
            return Err(Error::new("issue source and id are required"));
        }
        let linked = issue.clone();
        let ws = self
            .store()
            .update_workspace(workspace_id, |w| w.issue = Some(linked))
            .ok_or_else(|| not_found(workspace_id))?;
        // An issue belongs to one workspace: the link moves.
        for w in self.store().workspaces() {
            if w.id != workspace_id
                && w.issue
                    .as_ref()
                    .is_some_and(|i| i.source == issue.source && i.id == issue.id)
            {
                self.store().update_workspace(&w.id, |w| w.issue = None);
            }
        }
        recur_issues::claim(
            &self.engine.trackers().all(),
            &issue,
            &self.store().settings(),
        )
        .await;
        Ok(ws)
    }

    fn open_workspace_in_at(&self, workspace_id: &str, target_id: &str) -> Result<()> {
        let ws = self
            .store()
            .workspace(workspace_id)
            .ok_or_else(|| not_found(workspace_id))?;
        if ws.worktree_path.is_empty() {
            return Err(Error::new(format!(
                "workspace {:?} has no folder on disk",
                workspace_id
            )));
        }
        recur_code::open_in(&ws.worktree_path, target_id).map_err(err)
    }
}

#[async_trait]
impl WorkspacesService for AppServices {
    async fn apply_provisional_chat_titles(
        &self,
        workspace_id: &str,
        code: &str,
        text: &str,
    ) -> Result<()> {
        rt!(self, workspace_id, code, text => |s| {
            s.engine.apply_provisional_titles(&workspace_id, &code, &text);
            Ok(())
        })
    }

    async fn approve_plan(&self, workspace_id: &str) -> Result<String> {
        rt!(self, workspace_id => |s| Ok(s.engine.approve_plan(&workspace_id)))
    }

    async fn attention_counts(&self) -> Result<AttentionCounts> {
        rt!(self, => |s| Ok(s.engine.attention_counts()))
    }

    async fn cancel_workspace_message(&self, workspace_id: &str, code: &str) -> Result<()> {
        rt!(self, workspace_id, code => |s| {
            s.engine.cancel_chat(&workspace_id, &code).await;
            Ok(())
        })
    }

    async fn clear_workspace_issue(&self, workspace_id: &str) -> Result<Workspace> {
        rt!(self, workspace_id => |s| {
            s.store()
                .update_workspace(&workspace_id, |w| w.issue = None)
                .ok_or_else(|| not_found(&workspace_id))
        })
    }

    async fn close_workspace_chat(
        &self,
        workspace_id: &str,
        code: &str,
    ) -> Result<Vec<WorkspaceChatTab>> {
        rt!(self, workspace_id, code => |s| Ok(s.engine.close_workspace_chat(&workspace_id, &code).await))
    }

    async fn create_workspace(
        &self,
        project_id: &str,
        name: &str,
        base_branch: &str,
        mode: &str,
        prompt: &str,
        autonomy: &str,
        simplify: Option<bool>,
    ) -> Result<Workspace> {
        rt!(self, project_id, name, base_branch, mode, prompt, autonomy, simplify => |s| {
            s.create_workspace_with_at(
                &project_id, &name, &base_branch, &mode, &prompt, &autonomy, "", "", simplify, &[],
            )
            .await
        })
    }

    async fn create_workspace_chat(&self, workspace_id: &str) -> Result<WorkspaceChatTab> {
        rt!(self, workspace_id => |s| Ok(s.engine.create_workspace_chat(&workspace_id).await))
    }

    async fn create_workspace_with(
        &self,
        project_id: &str,
        name: &str,
        base_branch: &str,
        mode: &str,
        prompt: &str,
        autonomy: &str,
        loop_id: &str,
        policy_id: &str,
        simplify: Option<bool>,
        attachments: &[ChatAttachment],
    ) -> Result<Workspace> {
        rt!(self, project_id, name, base_branch, mode, prompt, autonomy, loop_id, policy_id, simplify, attachments => |s| {
            s.create_workspace_with_at(
                &project_id, &name, &base_branch, &mode, &prompt, &autonomy, &loop_id, &policy_id,
                simplify, &attachments,
            )
            .await
        })
    }

    async fn delete_closed_workspace_chat(
        &self,
        workspace_id: &str,
        code: &str,
    ) -> Result<Vec<WorkspaceChatTab>> {
        rt!(self, workspace_id, code => |s| {
            let (tabs, res) = s.engine.delete_closed_workspace_chat(&workspace_id, &code).await;
            res.map_err(err)?;
            Ok(tabs)
        })
    }

    async fn delete_workspace(&self, id: &str) -> Result<()> {
        rt!(self, id => |s| s.engine.delete_workspace(&id).await.map_err(err))
    }

    async fn engage_loop(&self, workspace_id: &str, goal: &str) -> Result<String> {
        rt!(self, workspace_id, goal => |s| {
            let (l, p) = s
                .store()
                .workspace(&workspace_id)
                .map(|w| (w.loop_id, w.policy_id))
                .unwrap_or_default();
            Ok(s.engage_loop_with_at(&workspace_id, &goal, &l, &p).await)
        })
    }

    async fn engage_loop_with(
        &self,
        workspace_id: &str,
        goal: &str,
        loop_id: &str,
        policy_id: &str,
    ) -> Result<String> {
        rt!(self, workspace_id, goal, loop_id, policy_id => |s| {
            Ok(s.engage_loop_with_at(&workspace_id, &goal, &loop_id, &policy_id).await)
        })
    }

    async fn fetch_working_copy_upstream(&self, id: &str) -> Result<WorkspaceLandState> {
        rt!(self, id => |s| Ok(s.engine.fetch_working_copy_upstream(&id).await))
    }

    async fn fork_workspace_chat(
        &self,
        workspace_id: &str,
        code: &str,
        message_id: &str,
    ) -> Result<WorkspaceChatTab> {
        rt!(self, workspace_id, code, message_id => |s| {
            Ok(s.engine.fork_workspace_chat(&workspace_id, &code, &message_id).await)
        })
    }

    async fn fork_workspace_chat_to_workspace(
        &self,
        workspace_id: &str,
        code: &str,
        message_id: &str,
    ) -> Result<Workspace> {
        rt!(self, workspace_id, code, message_id => |s| {
            s.engine
                .fork_workspace_chat_to_workspace(&workspace_id, &code, &message_id)
                .await
                .map_err(err)
        })
    }

    async fn get_workspace(&self, id: &str) -> Result<Workspace> {
        rt!(self, id => |s| {
            s.store()
                .workspace(&id)
                .ok_or_else(|| Error::new(format!("loop {:?} not found", id)))
        })
    }

    async fn get_workspace_feed(&self, workspace_id: &str) -> Result<Vec<FeedItem>> {
        rt!(self, workspace_id => |s| {
            match s.store().feed(&workspace_id) {
                Some(feed) if !feed.is_empty() => Ok(feed),
                // No cached feed: rebuild it from the persisted events.
                _ => Ok(s.engine.reconstruct_feed(&workspace_id)),
            }
        })
    }

    async fn last_assistant_text(&self, workspace_id: &str) -> Result<String> {
        rt!(self, workspace_id => |s| Ok(s.store().last_assistant_text(&workspace_id)))
    }

    async fn list_closed_workspace_chats(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<WorkspaceChatTab>> {
        rt!(self, workspace_id => |s| Ok(s.engine.list_closed_workspace_chats(&workspace_id)))
    }

    async fn list_open_in_targets(&self) -> Result<Vec<OpenInTarget>> {
        rt!(self, => |_s| Ok(recur_code::list_open_in_targets()))
    }

    async fn list_workspace_chats(&self, workspace_id: &str) -> Result<Vec<WorkspaceChatTab>> {
        rt!(self, workspace_id => |s| Ok(s.engine.list_workspace_chats(&workspace_id)))
    }

    async fn list_workspaces(&self) -> Result<Vec<Workspace>> {
        rt!(self, => |s| {
            s.engine.schedule_refresh_all_workspace_diffs();
            // Newest first for the sidebar; the store stays oldest first so
            // the queued-loop drain picks the oldest waiting run.
            let mut list = s.store().workspaces();
            list.reverse();
            Ok(list)
        })
    }

    async fn mark_workspace_chat_seen(&self, workspace_id: &str, code: &str) -> Result<()> {
        rt!(self, workspace_id, code => |s| {
            s.engine.mark_workspace_chat_seen(&workspace_id, &code);
            Ok(())
        })
    }

    async fn merge_workspace(&self, workspace_id: &str, method: &str) -> Result<Workspace> {
        rt!(self, workspace_id, method => |s| {
            s.engine
                .merge_workspace(&workspace_id, &MergeMethod::from(method))
                .await
                .map_err(err)
        })
    }

    async fn open_main_workspace(&self, project_id: &str) -> Result<Workspace> {
        rt!(self, project_id => |s| s.engine.open_main_workspace(&project_id).await.map_err(err))
    }

    async fn open_path(&self, path: &str) -> Result<OpenPathResult> {
        rt!(self, path => |s| s.open_path_at(&path).await)
    }

    async fn open_workspace_in(&self, workspace_id: &str, target_id: &str) -> Result<()> {
        rt!(self, workspace_id, target_id => |s| s.open_workspace_in_at(&workspace_id, &target_id))
    }

    async fn open_workspace_pr(&self, workspace_id: &str, force: bool) -> Result<String> {
        rt!(self, workspace_id, force => |s| {
            s.engine.open_workspace_pr(&workspace_id, force).await.map_err(err)
        })
    }

    async fn pause_loop(&self, id: &str) -> Result<()> {
        rt!(self, id => |s| {
            s.engine.pause(&id);
            Ok(())
        })
    }

    async fn pull_workspace_pr(&self, workspace_id: &str) -> Result<UpdateWorkspaceResult> {
        rt!(self, workspace_id => |s| s.engine.pull_workspace_pr(&workspace_id).await.map_err(err))
    }

    async fn push_working_copy(&self, workspace_id: &str) -> Result<Workspace> {
        rt!(self, workspace_id => |s| s.engine.push_working_copy(&workspace_id).await.map_err(err))
    }

    async fn remember_chat_loop_policy(
        &self,
        workspace_id: &str,
        code: &str,
        loop_id: &str,
        policy_id: &str,
    ) -> Result<()> {
        rt!(self, workspace_id, code, loop_id, policy_id => |s| {
            s.engine
                .remember_chat_loop_policy(&workspace_id, &code, &loop_id, &policy_id)
                .await;
            Ok(())
        })
    }

    async fn remember_workspace_chat_prompt_group(
        &self,
        workspace_id: &str,
        code: &str,
        group: &str,
    ) -> Result<()> {
        rt!(self, workspace_id, code, group => |s| {
            s.engine
                .remember_workspace_chat_prompt_group(&workspace_id, &code, &group)
                .await;
            Ok(())
        })
    }

    async fn remember_workspace_chat_selection(
        &self,
        workspace_id: &str,
        code: &str,
        model: &str,
        flags: &BTreeMap<String, String>,
        mode: &str,
        simplify: Option<bool>,
    ) -> Result<()> {
        rt!(self, workspace_id, code, model, flags, mode, simplify => |s| {
            s.engine
                .remember_workspace_chat_selection(
                    &workspace_id, &code, &model, Some(&flags), &mode, simplify,
                )
                .await;
            Ok(())
        })
    }

    async fn remember_workspace_chat_simplify(
        &self,
        workspace_id: &str,
        code: &str,
        on: bool,
    ) -> Result<()> {
        rt!(self, workspace_id, code, on => |s| {
            s.engine
                .remember_workspace_chat_simplify(&workspace_id, &code, on);
            Ok(())
        })
    }

    async fn remove_workspace(&self, id: &str) -> Result<Workspace> {
        rt!(self, id => |s| s.engine.remove_workspace(&id).await.map_err(err))
    }

    async fn reopen_workspace_chat(
        &self,
        workspace_id: &str,
        code: &str,
    ) -> Result<Vec<WorkspaceChatTab>> {
        rt!(self, workspace_id, code => |s| Ok(s.engine.reopen_workspace_chat(&workspace_id, &code).await))
    }

    async fn reorder_workspace_chats(&self, workspace_id: &str, codes: &[String]) -> Result<()> {
        rt!(self, workspace_id, codes => |s| {
            s.engine.reorder_workspace_chats(&workspace_id, &codes).1.map_err(err)
        })
    }

    async fn reset_workspace_chat(
        &self,
        workspace_id: &str,
        code: &str,
        message_id: &str,
    ) -> Result<()> {
        rt!(self, workspace_id, code, message_id => |s| {
            s.engine
                .reset_workspace_chat(&workspace_id, &code, &message_id)
                .await
                .map_err(err)
        })
    }

    async fn restart_loop(&self, id: &str) -> Result<String> {
        rt!(self, id => |s| Ok(s.engine.restart(&id).await))
    }

    async fn resume_loop(&self, id: &str) -> Result<()> {
        rt!(self, id => |s| {
            s.engine.resume(&id).await;
            Ok(())
        })
    }

    async fn retry_loop(&self, id: &str) -> Result<()> {
        rt!(self, id => |s| {
            s.engine.retry(&id).await;
            Ok(())
        })
    }

    async fn retry_workspace_setup(&self, id: &str) -> Result<()> {
        rt!(self, id => |s| s.engine.retry_workspace_setup(&id).await.map_err(err))
    }

    async fn send_workspace_message(
        &self,
        workspace_id: &str,
        code: &str,
        text: &str,
        model: &str,
        flags: &BTreeMap<String, String>,
        attachments: &[ChatAttachment],
        mode: &str,
        display_text: &str,
        fresh_session: bool,
    ) -> Result<String> {
        rt!(self, workspace_id, code, text, model, flags, attachments, mode, display_text, fresh_session => |s| {
            Ok(s.engine
                .send_workspace_turn(
                    &workspace_id,
                    &code,
                    &text,
                    &model,
                    Some(&flags),
                    &attachments,
                    &mode,
                    &display_text,
                    fresh_session,
                )
                .await)
        })
    }

    async fn set_chat_ui_occluded(&self, occluded: bool) -> Result<()> {
        rt!(self, occluded => |s| {
            s.engine.set_chat_ui_occluded(occluded);
            Ok(())
        })
    }

    async fn set_workspace_base_branch(
        &self,
        workspace_id: &str,
        base_branch: &str,
    ) -> Result<Workspace> {
        rt!(self, workspace_id, base_branch => |s| {
            s.engine
                .set_workspace_base_branch(&workspace_id, &base_branch)
                .await
                .map_err(err)
        })
    }

    async fn set_workspace_chat_focus(&self, workspace_id: &str, code: &str) -> Result<()> {
        rt!(self, workspace_id, code => |s| {
            s.engine.set_workspace_chat_focus(&workspace_id, &code);
            Ok(())
        })
    }

    async fn set_workspace_issue(&self, workspace_id: &str, issue: &IssueRef) -> Result<Workspace> {
        rt!(self, workspace_id, issue => |s| s.set_workspace_issue_at(&workspace_id, issue).await)
    }

    async fn set_workspace_loop_policy(
        &self,
        workspace_id: &str,
        loop_id: &str,
        policy_id: &str,
    ) -> Result<Workspace> {
        rt!(self, workspace_id, loop_id, policy_id => |s| {
            s.set_workspace_loop_policy_at(&workspace_id, &loop_id, &policy_id)
        })
    }

    async fn start_workspace(
        &self,
        project_id: &str,
        task: &str,
        base_branch: &str,
        autonomy: &str,
    ) -> Result<String> {
        rt!(self, project_id, task, base_branch, autonomy => |s| {
            s.engine
                .start_workspace(&project_id, &task, &base_branch, AutonomyLevel::from(autonomy))
                .await
                .map_err(err)
        })
    }

    async fn stop_loop(&self, id: &str) -> Result<()> {
        rt!(self, id => |s| {
            s.engine.stop(&id).await;
            Ok(())
        })
    }

    async fn submit_plan(
        &self,
        workspace_id: &str,
        summary: &str,
        body: &str,
        steps: &[String],
        step_ids: &[String],
    ) -> Result<String> {
        rt!(self, workspace_id, summary, body, steps, step_ids => |s| {
            Ok(s.engine
                .submit_human_plan(&workspace_id, &summary, &body, &steps, &step_ids))
        })
    }

    async fn update_plan(&self, workspace_id: &str, plan: &str) -> Result<String> {
        rt!(self, workspace_id, plan => |s| Ok(s.engine.edit_plan(&workspace_id, &plan)))
    }

    async fn update_plan_step(
        &self,
        workspace_id: &str,
        step_id: &str,
        status: &str,
    ) -> Result<String> {
        rt!(self, workspace_id, step_id, status => |s| {
            Ok(s.engine.update_human_plan_step(&workspace_id, &step_id, &status))
        })
    }

    async fn update_working_copy_from_remote(
        &self,
        workspace_id: &str,
        method: &str,
    ) -> Result<UpdateWorkspaceResult> {
        rt!(self, workspace_id, method => |s| {
            s.engine
                .update_working_copy_from_remote(&workspace_id, UpdateBaseMethod::from(method))
                .await
                .map_err(err)
        })
    }

    async fn update_workspace_with_base(
        &self,
        workspace_id: &str,
        method: &str,
    ) -> Result<UpdateWorkspaceResult> {
        rt!(self, workspace_id, method => |s| {
            s.engine
                .update_workspace_with_base(&workspace_id, &method)
                .await
                .map_err(err)
        })
    }

    async fn watch_workspace(&self, workspace_id: &str) -> Result<()> {
        rt!(self, workspace_id => |s| {
            s.engine.watch_workspace(&workspace_id);
            Ok(())
        })
    }

    async fn workspace_chat_items(&self, workspace_id: &str, code: &str) -> Result<Vec<ChatItem>> {
        rt!(self, workspace_id, code => |s| Ok(s.engine.workspace_chat_items(&workspace_id, &code).await))
    }

    async fn workspace_chat_models(&self) -> Result<Vec<String>> {
        Ok(["sonnet", "opus", "haiku", "fable"]
            .map(String::from)
            .to_vec())
    }

    async fn workspace_chat_selection(
        &self,
        workspace_id: &str,
        code: &str,
    ) -> Result<WorkspaceChatSelection> {
        rt!(self, workspace_id, code => |s| {
            let (model, flags, mode) = s.engine.workspace_chat_selection(&workspace_id, &code);
            Ok(WorkspaceChatSelection {
                model,
                flags,
                mode,
                simplify: s.engine.chat_simplify(&workspace_id, &code),
            })
        })
    }

    async fn workspace_chat_turn_diff(
        &self,
        workspace_id: &str,
        code: &str,
        message_id: &str,
    ) -> Result<Commit> {
        rt!(self, workspace_id, code, message_id => |s| {
            s.engine
                .workspace_chat_turn_diff(&workspace_id, &code, &message_id)
                .await
                .map_err(err)
        })
    }

    async fn workspace_discard_risk(&self, id: &str) -> Result<String> {
        rt!(self, id => |s| Ok(s.engine.workspace_discard_risk(&id).await))
    }

    async fn workspace_facts(&self, workspace_id: &str) -> Result<Vec<FeedItem>> {
        rt!(self, workspace_id => |s| Ok(s.engine.workspace_facts(&workspace_id)))
    }

    async fn workspace_land_state(&self, id: &str) -> Result<WorkspaceLandState> {
        rt!(self, id => |s| Ok(s.engine.workspace_land_state(&id).await))
    }

    async fn workspace_sessions(
        &self,
        workspace_id: &str,
        owner_code: &str,
        agent_code: &str,
    ) -> Result<Vec<SessionView>> {
        rt!(self, workspace_id, owner_code, agent_code => |s| {
            Ok(s.engine.workspace_sessions(&workspace_id, &owner_code, &agent_code))
        })
    }
}
