//! The rename host tools: chat tab titles, workspace titles and branch names.
//! `open_pr` lives in `prs`.

use std::time::Duration;

use recur_domain::WorkspaceState;
use recur_domain::run_active;

use crate::Engine;
use crate::bus::{ChatRenamed, ChatTab, EngineEvent};
use crate::text::{hard_truncate_runes, truncate};
use crate::turn::sessions::is_specialist_chat_code;
use crate::workspace::create::DEFAULT_WORKSPACE_TITLE;

/// A model reply as a UI-safe title of at most `max_len` (quotes, a
/// trailing period and later lines dropped; the default title is empty).
pub fn sanitize_title(raw: &str, max_len: usize) -> String {
    let s = raw.trim();
    if s.is_empty() {
        return String::new();
    }
    let mut s = s.trim_matches(['"', '\'', '`']);
    if let Some(i) = s.find('\n') {
        s = s[..i].trim();
    }
    let s = s.trim_end_matches([' ', '.', ',', ';', ':', '!']);
    let s = truncate(s.trim(), max_len);
    if s.is_empty() || s == DEFAULT_WORKSPACE_TITLE {
        return String::new();
    }
    s
}

/// [`sanitize_title`] for the sidebar (60).
pub fn sanitize_workspace_title(raw: &str) -> String {
    sanitize_title(raw, 60)
}

impl Engine {
    /// The chat tab title of Maestro or a PA* session. A specialist code
    /// (PL, …) retargets its owning loop's chat tab.
    pub fn rename_chat_session(&self, workspace_id: &str, agent_code: &str, title: &str) -> String {
        self.rename_chat_session_owned(workspace_id, agent_code, "", title)
    }

    /// Host `rename_session`; `owner_chat` is the calling turn's conductor
    /// tab when known.
    pub fn rename_chat_session_owned(
        &self,
        workspace_id: &str,
        agent_code: &str,
        owner_chat: &str,
        title: &str,
    ) -> String {
        let title = title.trim();
        if title.is_empty() {
            return "rename_session: title is required".into();
        }
        // A hard rune cap: a display ellipsis is wrong for a title.
        let title = hard_truncate_runes(title, 200).to_owned();
        let mut code = agent_code.trim().to_owned();
        if code.is_empty() {
            return "rename_session: missing agent".into();
        }
        if is_specialist_chat_code(&code) {
            let owner = owner_chat.trim();
            if !owner.is_empty() {
                code = owner.to_owned();
            } else if let Some(lp) = self
                .running_loop_for_done(workspace_id, &code)
                .filter(|lp| !lp.chat_code().trim().is_empty())
            {
                code = lp.chat_code().trim().to_owned();
            } else if self.running_loops_on(workspace_id).len() <= 1
                && let Some(ws) = self.store.workspace(workspace_id)
            {
                // Several walks share role codes and LoopChatCode is the
                // first tab: never retarget a sibling.
                let owner = crate::codes::loop_owner_code(&ws);
                if !owner.is_empty() {
                    code = owner;
                }
            }
        }
        if self.store.agent(workspace_id, &code).is_none() {
            return "rename_session: unknown chat tab".into();
        }
        // The provider comes from the tab's own model: the Settings default
        // would overwrite session.provider and the next dock remember would
        // think the provider switched (and fork-inject).
        let (kind, name, provider) = self.chat_session_meta(workspace_id, &code);
        let Some(sess) = self.ensure_agent_session(workspace_id, &kind, &code, &name, &provider)
        else {
            return "rename_session: could not open session".into();
        };
        if let Err(e) = self.store.patch_session_config_title(&sess.id, &title) {
            return format!("rename_session: {e}");
        }
        self.bus.publish(EngineEvent::ChatRenamed(ChatRenamed {
            agent_code: code.clone(),
            title: title.clone(),
            workspace_id: workspace_id.to_owned(),
        }));
        self.bus.publish(EngineEvent::WorkspaceChatTab(ChatTab {
            code,
            title: title.clone(),
            workspace_id: workspace_id.to_owned(),
        }));
        format!("Renamed chat to {title}.")
    }

    /// Host `rename_workspace`: the sidebar title (Goal/Short). The main
    /// working copy keeps the project name.
    pub fn rename_workspace(&self, workspace_id: &str, title: &str) -> String {
        let title = title.trim();
        if title.is_empty() {
            return "rename_workspace: title is required".into();
        }
        let title = sanitize_title(hard_truncate_runes(title, 200), 60);
        if title.is_empty() {
            return "rename_workspace: invalid title".into();
        }
        let Some(ws) = self.store.workspace(workspace_id) else {
            return "rename_workspace: unknown workspace".into();
        };
        if ws.is_main {
            return "rename_workspace: the main working copy keeps the project name".into();
        }
        let Some(u) = self.store.update_workspace(workspace_id, |lp| {
            if lp.is_main {
                return;
            }
            lp.goal = title.clone();
            lp.short = truncate(&title, 48);
        }) else {
            return "rename_workspace: unknown workspace".into();
        };
        if u.is_main || u.goal != title {
            return "rename_workspace: the main working copy keeps the project name".into();
        }
        self.publish_loop_updated(&u.id, u.state.clone(), &u.phase, u.iteration);
        format!("Renamed workspace to {title}.")
    }

    /// Host `rename_branch`: sanitizes the name, runs `git branch -m` in the
    /// worktree, then syncs the cached branch (and a local PR) from the live
    /// checkout.
    pub async fn rename_workspace_branch(&self, workspace_id: &str, name: &str) -> String {
        let Some(ws) = self.store.workspace(workspace_id) else {
            return "rename_branch: unknown workspace".into();
        };
        if ws.worktree_path.trim().is_empty() {
            return "rename_branch: no worktree".into();
        }
        if run_active(&ws.state) && ws.state != WorkspaceState::Planning {
            return "rename_branch: a loop is running; wait until it pauses".into();
        }
        let applied = match tokio::time::timeout(
            Duration::from_secs(15),
            recur_git::rename_branch_with_prefix(
                &ws.worktree_path,
                &self.branch_name_prefix(),
                name,
            ),
        )
        .await
        {
            Ok(Ok(b)) => b,
            Ok(Err(e)) => return format!("rename_branch: {e}"),
            Err(_) => return "rename_branch: context deadline exceeded".into(),
        };
        let mut live = self.sync_workspace_branch(workspace_id).await;
        if live.is_empty() {
            live = applied;
        }
        format!("Renamed branch to {live}.")
    }
}

#[cfg(test)]
#[path = "renames_tests.rs"]
mod tests;
