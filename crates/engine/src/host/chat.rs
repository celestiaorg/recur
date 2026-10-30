//! The skills and terminals host tools: list and activate skills, list and
//! read terminals.

use recur_agent_tools::{HostError, TerminalInfo};
use recur_domain::AgentSkill;

use crate::Engine;
use crate::composer::skills::{
    SKILL_ACTIVATE_TIMEOUT, SKILLS_CATALOG_TIMEOUT, format_skill_content,
};
use crate::composer::terminals::{
    TerminalMention, resolve_terminal_mentions, terminal_exited_note,
};
use crate::deadline::deadline_token;

fn host_err(e: impl std::fmt::Display) -> HostError {
    HostError(e.to_string())
}

impl Engine {
    fn skill_worktree(&self, workspace_id: &str) -> Result<String, HostError> {
        self.store
            .workspace(workspace_id)
            .map(|ws| ws.worktree_path)
            .ok_or_else(|| HostError::new("workspace not found"))
    }

    /// Host `list_skills`: the progressive-disclosure catalog (name,
    /// description, scope; no bodies), minus model-disabled and disabled
    /// skills.
    pub fn list_agent_skills(&self, workspace_id: &str) -> Result<Vec<AgentSkill>, HostError> {
        let worktree = self.skill_worktree(workspace_id)?;
        let cancel = deadline_token(SKILLS_CATALOG_TIMEOUT);
        let all = recur_code::list_skills(&cancel, &worktree).map_err(host_err)?;
        Ok(all
            .into_iter()
            .filter(|s| !s.path.is_empty() && !recur_code::skill_disables_model_invocation(&s.path))
            .filter(|s| self.skill_enabled_for_workspace(workspace_id, &s.name))
            .map(|s| AgentSkill {
                name: s.name,
                description: s.description,
                scope: s.scope,
                ..AgentSkill::default()
            })
            .collect())
    }

    /// Host `activate_skill`: one skill's body for model-driven activation.
    pub fn activate_agent_skill(
        &self,
        workspace_id: &str,
        name: &str,
    ) -> Result<String, HostError> {
        let worktree = self.skill_worktree(workspace_id)?;
        let cancel = deadline_token(SKILL_ACTIVATE_TIMEOUT);
        let sk = recur_code::skill_by_name(&cancel, &worktree, name).map_err(host_err)?;
        let q = format!("{name:?}");
        let Some(sk) = sk.filter(|s| !s.path.is_empty()) else {
            return Err(HostError(format!(
                "unknown skill {q}: call list_skills for names"
            )));
        };
        if !self.skill_enabled_for_workspace(workspace_id, name) {
            return Err(HostError(format!("skill {q} is disabled")));
        }
        if recur_code::skill_disables_model_invocation(&sk.path) {
            return Err(HostError(format!(
                "skill {q} is user-invocable only (disable-model-invocation)"
            )));
        }
        let body = recur_code::read_skill_body(&sk.path);
        if body.trim().is_empty() {
            return Err(HostError(format!("skill {q} has empty instructions")));
        }
        Ok(format_skill_content(&sk.name, &sk.path, &body))
    }

    /// Host `list_terminals`: `None` without a terminal source.
    pub fn list_agent_terminals(&self, workspace_id: &str) -> Option<Vec<TerminalInfo>> {
        let src = self.terminals.as_ref()?;
        let active = src.active_id(workspace_id);
        Some(
            src.list(workspace_id)
                .into_iter()
                .map(|s| TerminalInfo {
                    active: s.id == active,
                    id: s.id,
                    title: s.title,
                    cwd: s.cwd,
                    shell: s.shell,
                    exited: s.exited,
                    exit_code: s.exit_code.map(i64::from),
                })
                .collect(),
        )
    }

    /// Host `read_terminal`: one terminal's plain tail by id, prefix or
    /// title; the focused one when `id` is empty and unambiguous.
    pub fn read_agent_terminal(
        &self,
        workspace_id: &str,
        id: &str,
        tail_lines: i64,
    ) -> Result<String, HostError> {
        let Some(src) = self.terminals.as_ref() else {
            return Err(HostError::new("no Observatory terminals in this session"));
        };
        let sessions = src.list(workspace_id);
        if sessions.is_empty() {
            return Err(HostError::new("no live terminals in this workspace"));
        }
        let id = id.trim();
        let hits = resolve_terminal_mentions(
            &TerminalMention {
                qual: id.to_owned(),
                ..TerminalMention::default()
            },
            &sessions,
            &src.active_id(workspace_id),
        );
        let Some(info) = hits.first() else {
            return Err(HostError(format!(
                "terminal {:?} not found: call list_terminals",
                id
            )));
        };
        if id.is_empty() && hits.len() > 1 {
            return Err(HostError::new(
                "several terminals are open: pass id from list_terminals",
            ));
        }
        let body = src.read_plain(&info.id, tail_lines).map_err(HostError)?;
        let title = if info.title.is_empty() {
            "terminal"
        } else {
            &info.title
        };
        let note = terminal_exited_note(info);
        if body.trim().is_empty() {
            return Ok(format!(
                "terminal {} ({title}{note}): idle: no recent output",
                info.id
            ));
        }
        Ok(format!("terminal {} ({title}{note}):\n{body}", info.id))
    }
}

#[cfg(test)]
#[path = "chat_tests.rs"]
mod tests;
