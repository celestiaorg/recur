//! The skills and terminals host tools: list and activate skills, list and
//! read terminals.

use std::time::Duration;

use recur_agent_tools::{HostError, ReadTerminalRequest, TerminalInfo};
use recur_domain::AgentSkill;

use crate::Engine;
use crate::composer::skills::{
    SKILL_ACTIVATE_TIMEOUT, SKILLS_CATALOG_TIMEOUT, format_skill_content,
};
use crate::composer::terminals::{
    TerminalMention, resolve_terminal_mentions, terminal_exited_note,
};
use crate::deadline::deadline_token;
use crate::terminals::TerminalSession;

/// read_terminal's longest wait.
pub const MAX_TERMINAL_WAIT: Duration = Duration::from_secs(300);
const TERMINAL_WAIT_POLL: Duration = Duration::from_millis(250);

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
                    background: s.background,
                    started_by: s.started_by,
                })
                .collect(),
        )
    }

    /// Host `read_terminal`: one terminal's plain tail by id, prefix or
    /// title; the focused one when `id` is empty and unambiguous. A wait
    /// polls until `until` matches output after the agent's last read mark
    /// (its previous read_terminal or run_background on that terminal), the
    /// process exits, the wait ends or the call is cancelled.
    pub async fn read_agent_terminal(
        &self,
        cancel: &tokio_util::sync::CancellationToken,
        workspace_id: &str,
        req: ReadTerminalRequest,
    ) -> Result<String, HostError> {
        let (id, tail_lines) = (req.id.as_str(), req.tail_lines);
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
        let until = match req.until.trim() {
            "" => None,
            re => Some(
                regex::Regex::new(re)
                    .map_err(|e| HostError(format!("invalid until regex: {e}")))?,
            ),
        };
        let mut info = info.clone();
        let mark = self.terminal_read_mark(&info.id);
        let mut fresh = String::new();
        if until.is_some() {
            fresh = src.read_plain_since(&info.id, mark).map_err(HostError)?;
        }
        let wait = Duration::from_millis(u64::try_from(req.wait_ms).unwrap_or(0));
        let deadline = tokio::time::Instant::now() + wait.min(MAX_TERMINAL_WAIT);
        while tokio::time::Instant::now() < deadline
            && !info.exited
            && !until.as_ref().is_some_and(|re| re.is_match(&fresh))
        {
            tokio::select! {
                () = cancel.cancelled() => break,
                () = tokio::time::sleep(TERMINAL_WAIT_POLL) => {}
            }
            match session(src.as_ref(), workspace_id, &info.id) {
                Some(s) => info = s,
                None => {
                    return Err(HostError(format!(
                        "terminal {} was closed while waiting",
                        info.id
                    )));
                }
            }
            fresh = src.read_plain_since(&info.id, mark).map_err(HostError)?;
        }
        let body = src.read_plain(&info.id, tail_lines).map_err(HostError)?;
        self.set_terminal_read_mark(&info.id, src.output_written(&info.id));
        let title = if info.title.is_empty() {
            "terminal"
        } else {
            &info.title
        };
        let note = terminal_exited_note(&info);
        if body.trim().is_empty() {
            return Ok(format!(
                "terminal {} ({title}{note}): idle: no recent output",
                info.id
            ));
        }
        Ok(format!("terminal {} ({title}{note}):\n{body}", info.id))
    }
}

fn session(
    src: &dyn crate::terminals::TerminalSource,
    workspace_id: &str,
    id: &str,
) -> Option<TerminalSession> {
    src.list(workspace_id).into_iter().find(|s| s.id == id)
}

#[cfg(test)]
#[path = "chat_tests.rs"]
mod tests;
