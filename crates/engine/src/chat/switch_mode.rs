//! Privilege-safe `switch_mode`. A switch with a turn in flight parks that turn
//! (the ask_user pattern) and remounts a continuation with the destination
//! mode's tool shape, so the new tools are live immediately (a provider binds
//! its allowlist at spawn). A remount cap stops plan/ask flip-flops from
//! spawning turns forever.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use recur_domain::{
    CHAT_MODE_AGENT, CHAT_MODE_ASK, CHAT_MODE_LOOP, CHAT_MODE_PLAN, CHAT_MODE_REVIEW,
    normalize_chat_mode,
};

use crate::Engine;
use crate::ask_user::ASK_USER_PARK_DELAY;
use crate::bus::{ChatModeChanged, EngineEvent};
use crate::chatlife::ChatTurn;
use crate::codes::{
    MAESTRO_AGENT_CODE, chat_turn_key, is_pair_chat_code, normalize_pair_chat_code,
};
use crate::session_config::AgentConfig;

/// Appended to the tool result on a mid-turn remount: the model must stop.
pub const SWITCH_MODE_PARK_RESULT_SUFFIX: &str = " Stop now. This turn is remounting with the new tool policy. Do not keep working with the old tools; a continuation turn is starting.";

/// Consecutive remounts allowed on one tab before switches stop remounting
/// (a user send resets the count).
pub const MAX_MODE_REMOUNTS: i64 = 3;

/// How long the continuation waits so the parked turn finishes unwinding.
const REMOUNT_DELAY: Duration = Duration::from_millis(50);

/// Whether an interactive chat may move `from` → `to`: agent may go to any
/// read-only mode; ask/plan/review move among themselves; never up to agent.
pub fn switch_mode_allowed(from: &str, to: &str) -> bool {
    let from = normalize_chat_mode(from);
    let to = normalize_chat_mode(to);
    if to == CHAT_MODE_LOOP || to.is_empty() {
        return false;
    }
    if from == to {
        return true;
    }
    let readonly = |m: &str| matches!(m, CHAT_MODE_ASK | CHAT_MODE_PLAN | CHAT_MODE_REVIEW);
    if from == CHAT_MODE_AGENT {
        return readonly(to);
    }
    readonly(from) && readonly(to)
}

/// One in-flight remount, keyed like chats.
#[derive(Clone)]
pub(crate) struct PendingModeContinue {
    pub from: String,
    pub to: String,
    pub turn: Arc<ChatTurn>,
    pub epoch: u64,
}

/// Pending remounts and the per-tab remount depth.
#[derive(Default)]
pub(crate) struct ModeContinues {
    pending: HashMap<String, PendingModeContinue>,
    depth: HashMap<String, i64>,
}

fn mode_continue_key(workspace_id: &str, code: &str) -> String {
    if is_pair_chat_code(code) {
        return chat_turn_key(workspace_id, &normalize_pair_chat_code(code));
    }
    chat_turn_key(workspace_id, code)
}

/// The continuation prompt: the new policy is live on this turn.
pub fn format_mode_switch_resume_prompt(from: &str, to: &str) -> String {
    let from = normalize_chat_mode(from);
    let to = normalize_chat_mode(to);
    let mut b = format!(
        "Your switch_mode call remounted this chat from {from} to {to}. The new tool policy is live on THIS turn. Continue the user's request. Do not ask them to switch modes, and do not use Cursor's built-in SwitchMode.\n\n"
    );
    b.push_str(match to {
        CHAT_MODE_PLAN => "You are in PLAN mode (read-only on the git worktree). Investigate if needed, then file the FULL detailed plan with mcp__recur__submit_plan (body is what later agents execute from, not chat). You may create artifacts with mcp__recur__create_artifact in addition to, never instead of, submit_plan, and pass their ids in attachments.",
        CHAT_MODE_ASK => "You are in ASK mode (read-only on the git worktree). Answer questions; do not edit repository files. You may create artifacts with mcp__recur__create_artifact.",
        CHAT_MODE_REVIEW => "You are in REVIEW mode (read-only on the git worktree). File findings with mcp__recur__report_finding. On a re-review, mark prior findings resolved or invalid with mcp__recur__update_finding. You may create artifacts with mcp__recur__create_artifact in addition to, never instead of, report_finding.",
        _ => "You are in AGENT mode (read+edit). Proceed with the task; do not ask the user to switch to Agent.",
    });
    b
}

/// The continuation's bubble label ("Switched to Plan: continuing").
pub fn format_mode_switch_display(to: &str) -> String {
    let mut to = normalize_chat_mode(to);
    if to.is_empty() {
        to = CHAT_MODE_AGENT;
    }
    let mut chars = to.chars();
    let head: String = chars
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    format!("Switched to {head}{}: continuing", chars.as_str())
}

/// The remount died before reaching the provider (the send refused).
pub fn is_mode_switch_resume_dead_end(reply: &str) -> bool {
    let reply = reply.trim();
    if reply.is_empty() {
        return false;
    }
    reply.starts_with("I can't find that workspace")
        || reply.starts_with("I can't run right now")
        || reply.starts_with("I'm at capacity")
}

impl Engine {
    pub(crate) fn note_mode_continue(
        &self,
        workspace_id: &str,
        code: &str,
        from: &str,
        to: &str,
        turn: &Arc<ChatTurn>,
    ) {
        let generation = self.alloc_continue_gen(workspace_id, code);
        let old = self.mode_continues.lock().pending.insert(
            mode_continue_key(workspace_id, code),
            PendingModeContinue {
                from: from.to_owned(),
                to: to.to_owned(),
                turn: Arc::clone(turn),
                epoch: generation,
            },
        );
        if let Some(old) = old {
            self.drop_continue_gen(old.epoch);
        }
    }

    pub(crate) fn take_mode_continue(
        &self,
        workspace_id: &str,
        code: &str,
    ) -> Option<PendingModeContinue> {
        self.take_mode_continue_for(workspace_id, code, None)
    }

    /// Removes the pending remount; with `expect`, only while it still
    /// belongs to that turn (an older unwind must not steal a newer one).
    pub(crate) fn take_mode_continue_for(
        &self,
        workspace_id: &str,
        code: &str,
        expect: Option<&Arc<ChatTurn>>,
    ) -> Option<PendingModeContinue> {
        let mut mc = self.mode_continues.lock();
        let key = mode_continue_key(workspace_id, code);
        let p = mc.pending.get(&key)?;
        if let Some(t) = expect
            && !Arc::ptr_eq(&p.turn, t)
        {
            return None;
        }
        mc.pending.remove(&key)
    }

    /// A switch_mode remount is about to continue this tab.
    pub(crate) fn pending_mode_continue_for(&self, workspace_id: &str, code: &str) -> bool {
        self.mode_continues
            .lock()
            .pending
            .contains_key(&mode_continue_key(workspace_id, code))
    }

    pub(crate) fn bump_mode_remounts(&self, workspace_id: &str, code: &str) -> i64 {
        let mut mc = self.mode_continues.lock();
        let n = mc
            .depth
            .entry(mode_continue_key(workspace_id, code))
            .or_insert(0);
        *n += 1;
        *n
    }

    pub(crate) fn reset_mode_remounts(&self, workspace_id: &str, code: &str) {
        self.mode_continues
            .lock()
            .depth
            .remove(&mode_continue_key(workspace_id, code));
    }

    pub(crate) fn uncount_mode_remount(&self, workspace_id: &str, code: &str) {
        let mut mc = self.mode_continues.lock();
        let key = mode_continue_key(workspace_id, code);
        let n = mc.depth.get(&key).copied().unwrap_or(0);
        if n <= 1 {
            mc.depth.remove(&key);
        } else {
            mc.depth.insert(key, n - 1);
        }
    }

    /// The pending remount's generation (tests).
    #[cfg(test)]
    pub(crate) fn pending_mode_continue_epoch(
        &self,
        workspace_id: &str,
        code: &str,
    ) -> Option<u64> {
        self.mode_continues
            .lock()
            .pending
            .get(&mode_continue_key(workspace_id, code))
            .map(|p| p.epoch)
    }

    /// The remount depth of one tab (tests).
    pub fn mode_remount_depth(&self, workspace_id: &str, code: &str) -> i64 {
        self.mode_continues
            .lock()
            .depth
            .get(&mode_continue_key(workspace_id, code))
            .copied()
            .unwrap_or(0)
    }

    /// CancelChat's remount drop: Stop takes the pending continuation,
    /// frees its generation and gives back its remount count.
    pub(crate) fn cancel_mode_continue(&self, workspace_id: &str, code: &str) {
        if let Some(p) = self.take_mode_continue(workspace_id, code) {
            self.drop_continue_gen(p.epoch);
            self.uncount_mode_remount(workspace_id, code);
        }
    }

    /// Drops every pending remount and depth of a workspace (remove/delete).
    pub(crate) fn clear_mode_continues_for_workspace(&self, workspace_id: &str) {
        let workspace_id = workspace_id.trim();
        if workspace_id.is_empty() {
            return;
        }
        let prefix = format!("{workspace_id}\x00");
        let gens: Vec<u64> = {
            let mut mc = self.mode_continues.lock();
            let keys: Vec<String> = mc
                .pending
                .keys()
                .filter(|k| k.starts_with(&prefix))
                .cloned()
                .collect();
            let gens = keys
                .iter()
                .filter_map(|k| mc.pending.remove(k))
                .map(|p| p.epoch)
                .collect();
            mc.depth.retain(|k, _| !k.starts_with(&prefix));
            gens
        };
        for g in gens {
            self.drop_continue_gen(g);
        }
    }

    /// Applies a privilege-safe mode change on a PA* chat (Host
    /// `switch_mode`). With a turn in flight the turn is parked and a
    /// continuation remounts with the new shape.
    pub async fn switch_chat_mode(
        &self,
        workspace_id: &str,
        agent_code: &str,
        mode: &str,
    ) -> String {
        let code = agent_code.trim();
        if code.is_empty() || code == MAESTRO_AGENT_CODE || !code.starts_with("PA") {
            return "switch_mode is only available on interactive pair chats (not Maestro or loop specialists).".into();
        }
        let to = normalize_chat_mode(mode);
        if !matches!(
            to,
            CHAT_MODE_ASK | CHAT_MODE_PLAN | CHAT_MODE_AGENT | CHAT_MODE_REVIEW
        ) {
            return format!("Unknown mode {mode}. Use ask, plan, agent, or review.");
        }
        let mut from = CHAT_MODE_AGENT.to_owned();
        let mut flags = None;
        let mut model = String::new();
        if let Some(sess) = self.store.session_for_loop_agent(workspace_id, code) {
            let cfg = AgentConfig::parse(&sess.config_json);
            let m = normalize_chat_mode(&cfg.mode);
            if !m.is_empty() && m != CHAT_MODE_LOOP {
                from = m.to_owned();
            }
            model = cfg.model;
            flags = Some(cfg.flags);
        }
        if from == to {
            return format!("Already in {to} mode.");
        }
        if !switch_mode_allowed(&from, to) {
            if to == CHAT_MODE_AGENT {
                return format!(
                    "Cannot switch from {from} to agent (would gain write access). Ask the user to switch this chat to Agent mode from the dock."
                );
            }
            return format!("Cannot switch from {from} to {to}.");
        }
        self.remember_workspace_chat_selection(
            workspace_id,
            code,
            &model,
            flags.as_ref(),
            to,
            None,
        )
        .await;
        self.bus
            .publish(EngineEvent::ChatModeChanged(ChatModeChanged {
                agent_code: code.to_owned(),
                from: from.clone(),
                mode: to.to_owned(),
                workspace_id: workspace_id.to_owned(),
            }));
        let msg = format!("Switched chat mode from {from} to {to}.");
        let Some(turn) = self.live_chat_turn(workspace_id, code) else {
            return format!("{msg} Subsequent turns use the new tool policy.");
        };
        if self.bump_mode_remounts(workspace_id, code) > MAX_MODE_REMOUNTS {
            return format!("{msg} Subsequent turns use the new tool policy.");
        }
        self.note_mode_continue(workspace_id, code, &from, to, &turn);
        let (eng, ws, c) = (self.arc(), workspace_id.to_owned(), code.to_owned());
        tokio::spawn(async move {
            tokio::time::sleep(ASK_USER_PARK_DELAY).await;
            eng.park_chat_turn_if(&ws, &c, Some(&turn));
        });
        format!("{msg}{SWITCH_MODE_PARK_RESULT_SUFFIX}")
    }

    /// Starts the remounted turn after switch_mode, once the send that owns
    /// `turn` released its agent slot and MCP token. User Stop, shutdown or
    /// a parked ask_user drop the remount. The model finishing before the
    /// park lands still remounts: the pending entry is the intent.
    pub(crate) fn maybe_continue_after_mode_switch(
        &self,
        workspace_id: &str,
        code: &str,
        turn: &Arc<ChatTurn>,
    ) {
        let Some(p) = self.take_mode_continue_for(workspace_id, code, Some(turn)) else {
            return;
        };
        if turn.is_stopped() || self.ask_user_pending_for(workspace_id, code) {
            self.uncount_mode_remount(workspace_id, code);
            self.drop_continue_gen(p.epoch);
            return;
        }
        let td = self.teardown.lock();
        if *td {
            drop(td);
            self.uncount_mode_remount(workspace_id, code);
            self.drop_continue_gen(p.epoch);
            return;
        }
        let (eng, ws, c) = (self.arc(), workspace_id.to_owned(), code.to_owned());
        self.tasks.teardown.spawn(async move {
            tokio::time::sleep(REMOUNT_DELAY).await;
            eng.resume_after_mode_switch(&ws, &c, &p.from, &p.to, p.epoch)
                .await;
        });
    }

    pub(crate) async fn resume_after_mode_switch(
        &self,
        workspace_id: &str,
        code: &str,
        from: &str,
        to: &str,
        epoch: u64,
    ) {
        self.resume_after_mode_switch_inner(workspace_id, code, from, to, epoch)
            .await;
        self.drop_continue_gen(epoch);
    }

    async fn resume_after_mode_switch_inner(
        &self,
        workspace_id: &str,
        code: &str,
        from: &str,
        to: &str,
        epoch: u64,
    ) {
        if self.is_shutting_down() {
            return;
        }
        if self.live_chat_turn(workspace_id, code).is_some() || !self.continue_gen_live(epoch) {
            self.uncount_mode_remount(workspace_id, code);
            return;
        }
        let (model, flags, mut mode) = self.workspace_chat_selection(workspace_id, code);
        if normalize_chat_mode(&mode).is_empty() {
            mode = to.to_owned();
        }
        let prompt = format_mode_switch_resume_prompt(from, &mode);
        let display = format_mode_switch_display(&mode);
        let reply = self
            .send_workspace_turn_epoch(
                workspace_id,
                code,
                &prompt,
                &model,
                Some(&flags),
                &[],
                &mode,
                &display,
                false,
                epoch,
            )
            .await;
        if is_mode_switch_resume_dead_end(&reply) {
            self.persist_chat_failure(
                workspace_id,
                code,
                &prompt,
                &display,
                &reply,
                0,
                &recur_agent::TurnResult::default(),
                false,
                "",
            );
            self.flag_chat_unseen(workspace_id, code, false);
        }
    }
}
