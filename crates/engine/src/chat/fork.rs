//! Forking a chat at a turn: a new tab (or a sibling workspace) whose
//! transcript is the parent's messages through the fork point. Every fork
//! starts a fresh provider session; the copied transcript rides the first send
//! as a fork preamble. The same inject repairs a foreign resume handle after a
//! provider switch.

use recur_agent::{HistoryMode, TurnRequest, TurnResult};
use recur_domain::{
    AgentRole, CHAT_MODE_AGENT, CHAT_MODE_LOOP, COMPACT_DONE, ContentBlock, Message,
    SessionContext, TranscriptOptions, Workspace, WorkspaceChatTab, normalize_chat_mode,
    parse_model_ref, transcript_xml,
};

use crate::codes::{MAESTRO_AGENT_CODE, PAIR_AGENT_CODE, is_pair_chat_code};
use crate::session_config::AgentConfig;
use crate::turn::transcript::FORK_PREAMBLE_LEAD;
use crate::workspace::create::DEFAULT_WORKSPACE_TITLE;
use crate::{Engine, EngineError, Result};

/// Rebuilds a forked transcript so a new provider session continues from
/// the fork point: every block but thinking (tool calls and results
/// survive), tool bodies clipped, prior fork-inject rows dropped, cut to the
/// last compaction watermark.
pub fn fork_history_preamble(
    msgs: &[Message],
    ctx: Option<&SessionContext>,
    image_root: &str,
) -> String {
    let body = transcript_xml(
        &fork_replay_messages(msgs, ctx),
        &TranscriptOptions {
            omit_thinking: true,
            clip_tools: true,
            image_root: image_root.to_owned(),
            ..TranscriptOptions::default()
        },
    );
    if body.is_empty() {
        return String::new();
    }
    format!(
        "{FORK_PREAMBLE_LEAD} Continue from the <transcript> below. Do not re-greet; pick up where it left off.\n\n{body}\n"
    )
}

/// Drops prior fork-inject rows, then cuts to the compact watermark so the
/// inject is summary + live tail.
pub fn fork_replay_messages(msgs: &[Message], ctx: Option<&SessionContext>) -> Vec<Message> {
    let msgs = skip_fork_preamble_messages(msgs);
    if let Some(replay) = fork_replay_from_context(&msgs, ctx) {
        return replay;
    }
    fork_replay_from_compact_blocks(msgs)
}

fn done_summary(b: &ContentBlock) -> Option<recur_domain::CompactPayload> {
    b.as_compact()
        .filter(|p| p.status == COMPACT_DONE && !p.summary.trim().is_empty())
}

/// The session context row is the source of truth for the watermark.
fn fork_replay_from_context(
    msgs: &[Message],
    ctx: Option<&SessionContext>,
) -> Option<Vec<Message>> {
    let ctx = ctx?;
    if ctx.summary.trim().is_empty() || ctx.last_compacted_message_id.is_empty() {
        return None;
    }
    let cut = msgs
        .iter()
        .position(|m| m.id == ctx.last_compacted_message_id)?;
    let mut head = msgs[cut].clone();
    let found = head
        .blocks()
        .iter()
        .enumerate()
        .rev()
        .find_map(|(j, b)| done_summary(b).map(|p| (j, p)));
    let mut out = Vec::with_capacity(msgs.len() - cut);
    match found {
        Some((block, payload)) => {
            let mut blocks = head.blocks()[block..].to_vec();
            let trigger = if payload.trigger.is_empty() {
                "auto".to_owned()
            } else {
                payload.trigger
            };
            blocks[0] =
                ContentBlock::compact(COMPACT_DONE, &trigger, &ctx.summary, payload.pre_tokens);
            head.content = Some(blocks);
            out.push(head);
        }
        None => out.push(Message {
            role: "assistant".into(),
            content: Some(vec![ContentBlock::compact(
                COMPACT_DONE,
                "auto",
                &ctx.summary,
                0,
            )]),
            ..Message::default()
        }),
    }
    out.extend_from_slice(&msgs[cut + 1..]);
    Some(out)
}

/// The fallback watermark: the last done compact block with a summary.
fn fork_replay_from_compact_blocks(msgs: Vec<Message>) -> Vec<Message> {
    let mut cut: Option<(usize, usize)> = None;
    for (i, m) in msgs.iter().enumerate() {
        for (j, b) in m.blocks().iter().enumerate() {
            if done_summary(b).is_some() {
                cut = Some((i, j));
            }
        }
    }
    let Some((cut_msg, cut_block)) = cut else {
        return msgs;
    };
    let mut head = msgs[cut_msg].clone();
    if cut_block > 0 {
        head.content = Some(head.blocks()[cut_block..].to_vec());
    }
    if head.blocks().is_empty() {
        return msgs[cut_msg + 1..].to_vec();
    }
    let mut out = Vec::with_capacity(msgs.len() - cut_msg);
    out.push(head);
    out.extend_from_slice(&msgs[cut_msg + 1..]);
    out
}

fn skip_fork_preamble_messages(msgs: &[Message]) -> Vec<Message> {
    msgs.iter()
        .filter(|m| !message_has_fork_preamble(m))
        .cloned()
        .collect()
}

/// A user row that carried a fork preamble (a failed first send).
pub fn message_has_fork_preamble(m: &Message) -> bool {
    m.role == "user"
        && m.blocks()
            .iter()
            .filter_map(ContentBlock::as_text)
            .any(|p| p.text.contains(FORK_PREAMBLE_LEAD))
}

fn fork_code_ok(code: &str) -> Option<String> {
    let code = code.trim();
    let code = if code.is_empty() {
        PAIR_AGENT_CODE
    } else {
        code
    };
    (code == MAESTRO_AGENT_CODE || is_pair_chat_code(code)).then(|| code.to_owned())
}

impl Engine {
    /// Branches a chat at an exact message (the turn footer the UI sends)
    /// into a new PA* tab of the same workspace. Empty tab on refusal.
    pub async fn fork_workspace_chat(
        &self,
        workspace_id: &str,
        code: &str,
        message_id: &str,
    ) -> WorkspaceChatTab {
        if message_id.trim().is_empty() {
            return WorkspaceChatTab::default();
        }
        let Some(code) = fork_code_ok(code) else {
            return WorkspaceChatTab::default();
        };
        let (_life, held) = self.lock_chat_life(workspace_id, &code).await;
        if !held {
            return WorkspaceChatTab::default();
        }
        let Some(ws) = self.store.workspace(workspace_id) else {
            return WorkspaceChatTab::default();
        };
        let Some(parent) = self.store.session_for_loop_agent(workspace_id, &code) else {
            return WorkspaceChatTab::default();
        };
        if !ws.has_run() && self.pair_chat_agents(workspace_id).is_empty() {
            self.ensure_pair_agent(workspace_id, PAIR_AGENT_CODE, "");
        }
        let new_code = self.next_pair_chat_code(workspace_id);
        self.materialize_chat_fork(workspace_id, workspace_id, &new_code, &parent, message_id)
            .unwrap_or_default()
    }

    /// Creates a sibling hand-driven workspace in the same project from the
    /// source branch HEAD and copies this chat through `message_id` onto its
    /// PA tab. The worktree allocates in the background, then the source's
    /// uncommitted changes are overlaid.
    pub async fn fork_workspace_chat_to_workspace(
        &self,
        workspace_id: &str,
        code: &str,
        message_id: &str,
    ) -> Result<Workspace> {
        if message_id.trim().is_empty() {
            return Err(EngineError::new("fork message is required"));
        }
        let ws = self
            .store
            .workspace(workspace_id)
            .ok_or_else(|| EngineError(format!("workspace {workspace_id:?} not found")))?;
        let code = fork_code_ok(code)
            .ok_or_else(|| EngineError(format!("cannot fork agent {:?}", code.trim())))?;
        let parent = self
            .store
            .session_for_loop_agent(workspace_id, &code)
            .ok_or_else(|| EngineError::new("no chat session to fork"))?;
        if !self.store.session_contains_message(&parent.id, message_id) {
            return Err(EngineError(format!(
                "fork message {message_id:?} is not in this chat"
            )));
        }
        if ws.is_pr_chat_host() {
            return Err(EngineError::new(
                "create a workspace from this pull request before forking the chat into a new worktree",
            ));
        }
        let mut name = self.chat_tab_title(workspace_id, &code);
        if name.is_empty() || name == "New chat" {
            name = ws.short.trim().to_owned();
        }
        if name.is_empty() {
            name = DEFAULT_WORKSPACE_TITLE.into();
        }
        let mut start = ws.branch.trim().to_owned();
        if start.is_empty() && !ws.is_main {
            start = ws.base_branch.clone();
        }
        // Persist, copy the session, then allocate: starting git before the
        // store fork succeeded raced the rollback against the allocation.
        let dest = self
            .persist_creating_workspace(&ws.project_id, &name, &start)
            .await?;
        if self
            .materialize_chat_fork(workspace_id, &dest.id, PAIR_AGENT_CODE, &parent, message_id)
            .is_none()
        {
            let _ = self.delete_workspace(&dest.id).await;
            return Err(EngineError::new(
                "could not copy chat into the new workspace",
            ));
        }
        self.fork_overlay.lock().insert(dest.id.clone(), ws);
        self.start_allocate(dest.clone());
        Ok(self.store.workspace(&dest.id).unwrap_or(dest))
    }

    /// Copies `parent` through `message_id` onto the destination tab and
    /// marks it so the first send injects the transcript.
    pub(crate) fn materialize_chat_fork(
        &self,
        src_workspace_id: &str,
        dest_workspace_id: &str,
        dest_code: &str,
        parent: &recur_domain::Session,
        message_id: &str,
    ) -> Option<WorkspaceChatTab> {
        let mut cfg = AgentConfig::parse(&parent.config_json);
        cfg.code = dest_code.to_owned();
        cfg.name = "Agent".into();
        cfg.fork_inject = true;
        cfg.native_history_after.clear();
        if normalize_chat_mode(&cfg.mode) == CHAT_MODE_LOOP {
            cfg.mode = CHAT_MODE_AGENT.into();
        }
        let cfg_json = serde_json::to_string(&cfg).ok()?;
        let child = if dest_workspace_id == src_workspace_id {
            self.store
                .fork_session(&parent.id, message_id, AgentRole::Coder.as_str(), &cfg_json)
        } else {
            self.store.fork_session_to_workspace(
                &parent.id,
                message_id,
                dest_workspace_id,
                AgentRole::Coder.as_str(),
                &cfg_json,
            )
        }
        .ok()?;
        let model = cfg.model.trim().to_owned();
        self.ensure_pair_agent(dest_workspace_id, dest_code, &model);
        if !model.is_empty() {
            self.store
                .update_agent(dest_workspace_id, dest_code, |a| a.model = model.clone());
        }
        Some(WorkspaceChatTab {
            code: dest_code.to_owned(),
            title: self.chat_tab_title(dest_workspace_id, dest_code),
            updated_at: child.updated_at,
            mode: cfg.mode,
            ..WorkspaceChatTab::default()
        })
    }

    /// Sanitizes a foreign resume handle and, when needed, prepends the
    /// copied transcript. True when this turn carries the preamble, so
    /// [`Engine::finish_chat_fork`] clears `forkInject` only then. A restart
    /// turn with a usable handle resumes and leaves the flag for a later user
    /// send. A Retry runs through here too (a retry after a provider switch
    /// must drop the foreign handle and replay), but never nests a second
    /// preamble onto a wire that already opens with one.
    pub(crate) fn apply_chat_fork(
        &self,
        workspace_id: &str,
        code: &str,
        req: &mut TurnRequest,
        resume_turn: bool,
    ) -> bool {
        let dest = parse_model_ref(&req.model).provider;
        let mut cleared = false;
        if !self.resume_fits_provider(&req.resume_session, &dest) {
            req.resume_session.clear();
            req.fork_session = false;
            self.clear_agent_session(workspace_id, code);
            cleared = true;
        }
        if resume_turn && !req.resume_session.is_empty() {
            return false;
        }
        let Some(sess) = self.store.session_for_loop_agent(workspace_id, code) else {
            return false;
        };
        let cfg = AgentConfig::parse(&sess.config_json);
        if !resume_turn && cfg.fork_inject && !req.resume_session.is_empty() {
            // A dock switch between two providers whose handles look alike
            // leaves a handle shape cannot flag; forkInject marks it stale.
            req.resume_session.clear();
            req.fork_session = false;
            self.clear_agent_session(workspace_id, code);
            cleared = true;
        }
        if !req.resume_session.is_empty() {
            return false;
        }
        // App-history providers rebuild their input from the copied
        // transcript themselves.
        if self
            .providers
            .for_model(&req.model)
            .is_some_and(|p| p.caps().history == HistoryMode::App)
        {
            return false;
        }
        if !cfg.fork_inject && !cleared {
            return false;
        }
        if req.prompt.starts_with(FORK_PREAMBLE_LEAD) {
            // A retry replaying a wire an earlier send injected: no second
            // header, but it still rides stdin and consumes the flag once a
            // session id lands.
            req.prompt_on_stdin = true;
            if !cfg.fork_inject {
                let _ = self
                    .store
                    .patch_session_config_merge(&sess.id, r#"{"forkInject":true}"#);
            }
            return true;
        }
        let msgs = self
            .store
            .session_transcript(&sess.id)
            .ok()
            .flatten()
            .unwrap_or_default();
        if msgs.is_empty() {
            return false;
        }
        let ctx = self.store.active_session_context(&sess.id);
        let preamble = fork_history_preamble(&msgs, ctx.as_ref(), &self.data_dir.to_string_lossy());
        if preamble.is_empty() {
            return false;
        }
        if req.display_text.is_empty() {
            req.display_text = req.prompt.clone();
        }
        req.prompt = format!("{preamble}{}", req.prompt);
        req.prompt_on_stdin = true;
        if !cfg.fork_inject {
            let _ = self
                .store
                .patch_session_config_merge(&sess.id, r#"{"forkInject":true}"#);
        }
        true
    }

    /// Clears `forkInject` after a turn that carried the preamble also
    /// recorded a provider session id. A failed first send keeps it.
    pub(crate) fn finish_chat_fork(
        &self,
        workspace_id: &str,
        code: &str,
        injected: bool,
        res: &TurnResult,
    ) {
        if !injected {
            return;
        }
        let mut new_id = res.session_id.trim().to_owned();
        if let Some(a) = self.store.agent(workspace_id, code)
            && !a.session_id.trim().is_empty()
        {
            new_id = a.session_id;
        }
        if new_id.is_empty() {
            return;
        }
        self.consume_fork_inject(workspace_id, code);
    }

    pub(crate) fn consume_fork_inject(&self, workspace_id: &str, code: &str) {
        let Some(sess) = self.store.session_for_loop_agent(workspace_id, code) else {
            return;
        };
        if !AgentConfig::parse(&sess.config_json).fork_inject {
            return;
        }
        let _ = self
            .store
            .patch_session_config_merge(&sess.id, r#"{"forkInject":null}"#);
    }
}
