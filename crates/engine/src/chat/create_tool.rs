//! The `create_workspace` tool: an Agent chat starts a fresh workspace (triage
//! schedules, one workspace per issue). Model and flags follow Settings (the
//! calling chat's pick, or the mode's default); Simplify follows the calling
//! chat. Idempotent on title, so a recurring schedule cannot fan out the same
//! issue twice. By default the new workspace is a merge-back child of the
//! caller.

use std::sync::Arc;
use std::time::Duration;

use recur_agent_tools::CreateWorkspaceRequest;
use recur_domain::{
    CHAT_MODE_AGENT, CHAT_MODE_LOOP, CHILD_WORKSPACE_MODEL_MODE, ChildFanOut, Workspace,
    normalize_chat_mode,
};

use crate::Engine;
use crate::schedules::PromptLaunch;
use crate::text::equal_fold;

/// Whether the new workspace is a merge-back child of the caller. Yes by
/// default; standalone when asked, when the caller is a scheduled run (its
/// branch is scratch), when mode is loop (a loop ships its own PR), or when
/// a base branch was named. A child cannot create workspaces; a child needs
/// a caller with a branch and worktree in the same project.
pub fn want_child(
    caller: &Workspace,
    req: &CreateWorkspaceRequest,
    mode: &str,
    project_id: &str,
) -> Result<bool, String> {
    if !caller.parent_id.trim().is_empty() {
        return Err("a child workspace cannot create workspaces".into());
    }
    let child = match req.standalone {
        Some(s) => !s,
        None => {
            caller.schedule_id.is_empty()
                && mode != CHAT_MODE_LOOP
                && req.base_branch.trim().is_empty()
        }
    };
    if !child {
        return Ok(false);
    }
    if project_id != caller.project_id
        || caller.branch.trim().is_empty()
        || caller.worktree_path.trim().is_empty()
    {
        return Err("a child workspace needs a calling workspace with a branch and worktree in the same project; pass standalone=true".into());
    }
    Ok(true)
}

impl Engine {
    /// Host `create_workspace`.
    pub async fn create_workspace_from_chat(
        &self,
        caller_workspace_id: &str,
        caller_code: &str,
        req: CreateWorkspaceRequest,
    ) -> String {
        if self.is_shutting_down() {
            return "create_workspace: Recur is closing".into();
        }
        let title = req.title.trim().to_owned();
        let prompt = req.prompt.trim().to_owned();
        if title.is_empty() || prompt.is_empty() {
            return "create_workspace: title and prompt are required".into();
        }
        let Some(caller) = self.store.workspace(caller_workspace_id) else {
            return "create_workspace: calling workspace not found".into();
        };
        let mut project_id = req.project_id.trim().to_owned();
        if project_id.is_empty() {
            project_id = caller.project_id.clone();
        }
        let Some(proj) = self.store.project(&project_id) else {
            return format!("create_workspace: project {:?} not found", project_id);
        };
        let mode = match normalize_chat_mode(&req.mode) {
            "" => CHAT_MODE_AGENT,
            m => m,
        };
        let mut l = PromptLaunch {
            project_id: project_id.clone(),
            name: title.clone(),
            prompt,
            mode: mode.to_owned(),
            base_branch: req.base_branch.trim().to_owned(),
            ..PromptLaunch::default()
        };
        let child = match want_child(&caller, &req, mode, &project_id) {
            Ok(c) => c,
            Err(reason) => return format!("create_workspace: {reason}"),
        };
        if child {
            l.base_branch = caller.branch.clone();
        }
        if l.base_branch.is_empty() {
            l.base_branch = proj.base_branch.clone();
        }
        let exists = tokio::time::timeout(
            Duration::from_secs(5),
            self.git_manager(&proj.repo_path)
                .base_branch_exists(&l.base_branch),
        )
        .await
        .unwrap_or(false);
        if !exists {
            return format!(
                "create_workspace: base branch {:?} not found in {}",
                l.base_branch, proj.name
            );
        }
        if mode == CHAT_MODE_LOOP {
            match self.resolve_guard(&req.loop_id, &req.policy_id, &self.store.policy().autonomy) {
                Ok(g) => {
                    l.loop_id = g.loop_def.id;
                    l.policy_id = g.policy.id;
                }
                Err(reason) => return format!("create_workspace: {reason}"),
            }
        }
        let set = self.settings();
        if set.effective_child_workspace_model() == CHILD_WORKSPACE_MODEL_MODE {
            (l.model, l.flags) = set.model_for_chat_mode(mode);
        } else {
            (l.model, l.flags, _) = self.workspace_chat_selection(caller_workspace_id, caller_code);
        }
        l.simplify = self.chat_simplify(caller_workspace_id, caller_code);
        let _sched = self.schedule.lock().await;
        if self.is_shutting_down() {
            return "create_workspace: Recur is closing".into();
        }
        for w in self.store.workspaces() {
            if w.project_id == project_id
                && !w.archived
                && !w.is_main
                && equal_fold(w.goal.trim(), &title)
            {
                return format!(
                    "Workspace {} already exists for {:?}; not creating a duplicate.",
                    w.id, title
                );
            }
        }
        let wave = if child {
            self.chat_child_wave(caller_workspace_id, caller_code)
        } else {
            0
        };
        let base = l.base_branch.clone();
        let link: Option<crate::schedules::LinkFn<'_>> = child.then(|| {
            let (store, parent, code, mode) = (
                Arc::clone(&self.store),
                caller_workspace_id.to_owned(),
                caller_code.to_owned(),
                mode.to_owned(),
            );
            Box::new(move |ws_id: &str| {
                store.update_workspace(ws_id, |w| {
                    w.parent_id = parent.clone();
                    w.fan_out = Some(ChildFanOut {
                        chat: code.clone(),
                        mode: mode.clone(),
                        wave,
                        ..ChildFanOut::default()
                    });
                });
                Ok(())
            }) as crate::schedules::LinkFn<'_>
        });
        let (log_title, log_caller) = (title.clone(), caller_workspace_id.to_owned());
        let id = match self
            .launch_prompt_workspace(
                l,
                link,
                Arc::new(move |status: &str, detail: &str| {
                    tracing::info!(
                        "recur: create_workspace {log_title} from {log_caller}: {status} {detail}"
                    );
                }),
            )
            .await
        {
            Ok(id) => id,
            Err(e) => return format!("create_workspace: {e}"),
        };
        if child {
            if let Some(ws) = self.store.workspace(&id) {
                self.emit_child_card(caller_workspace_id, &ws);
            }
            self.spawn_chat_child_watcher(caller_workspace_id, &id);
            return format!(
                "Created subagent {id} ({:?}) as a child workspace off {base}; its {mode} chat is running the prompt. This turn does not wait: when it finishes you get a new turn here with its report, then read_child, merge_child, or archive_child it.",
                title
            );
        }
        format!(
            "Created workspace {id} ({:?}) in {} from {base}; its {mode} chat is running the prompt. It appears in the sidebar as it allocates.",
            title, proj.name
        )
    }
}
