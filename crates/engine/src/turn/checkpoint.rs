//! Per-turn worktree checkpoints: every agent turn snapshots the worktree under
//! `refs/recur/checkpoints/<workspace>/<userMessageID>/{start,end}`; start →
//! end is the turn's own diff, end is what "reset to here" restores. Also the
//! review-pass checkpoint for `/re-review`.

use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

use recur_domain::{ChatItem, Commit, DiffStat, Message, Session, Workspace};
use recur_git::CHECKPOINT_REF_PREFIX;
use recur_store::Store;

use super::transcript::user_message_is_chat_prompt;
use crate::codes::{MAESTRO_AGENT_CODE, loop_owner_code, normalize_pair_chat_code};
use crate::{Engine, EngineError};

/// `refs/recur/checkpoints/<ws>/<user msg>/<phase>`.
pub fn checkpoint_ref(workspace_id: &str, user_msg_id: &str, phase: &str) -> String {
    format!("{CHECKPOINT_REF_PREFIX}{workspace_id}/{user_msg_id}/{phase}")
}

/// A timeout around a git call: dropping the future on expiry kills the process
/// group.
async fn within<T>(d: Duration, f: impl Future<Output = T>) -> Option<T> {
    tokio::time::timeout(d, f).await.ok()
}

async fn list_refs(d: Duration, worktree: &str, prefix: &str) -> HashMap<String, String> {
    within(d, recur_git::list_checkpoints(worktree, prefix))
        .await
        .flatten()
        .unwrap_or_default()
}

/// The human message that opened the turn whose footer is `footer_id`.
pub fn turn_user_message_id(msgs: &[Message], footer_id: &str) -> String {
    let mut user = String::new();
    for m in msgs {
        if m.role == "user" && user_message_is_chat_prompt(m) {
            user = m.id.clone();
        }
        if m.id == footer_id {
            return user;
        }
    }
    String::new()
}

/// The tab's most recent human message: the turn a retry or remount
/// continues.
pub fn last_chat_prompt_id(store: &Store, workspace_id: &str, code: &str) -> String {
    let Some(sess) = store.session_for_loop_agent(workspace_id, code) else {
        return String::new();
    };
    store
        .session_transcript(&sess.id)
        .ok()
        .flatten()
        .unwrap_or_default()
        .iter()
        .rev()
        .find(|m| m.role == "user" && user_message_is_chat_prompt(m))
        .map(|m| m.id.clone())
        .unwrap_or_default()
}

impl Engine {
    /// Snapshots one turn phase. Failures are logged, never surfaced: a
    /// send must not fail because git could not snapshot.
    pub async fn take_checkpoint(&self, ws: &Workspace, user_msg_id: &str, phase: &str) {
        if ws.worktree_path.is_empty() || user_msg_id.is_empty() {
            return;
        }
        let r = checkpoint_ref(&ws.id, user_msg_id, phase);
        match within(Duration::from_secs(120), self.checkpoint(ws, &r)).await {
            Some(Ok(_)) => {}
            Some(Err(e)) => {
                tracing::warn!("checkpoint {phase} {}/{user_msg_id}: {e}", ws.id);
            }
            None => tracing::warn!(
                "checkpoint {phase} {}/{user_msg_id}: context deadline exceeded",
                ws.id
            ),
        }
    }

    /// Snapshots `ws` into `ref_name` through the private index in the
    /// workspace data dir (never the worktree's own index).
    async fn checkpoint(&self, ws: &Workspace, ref_name: &str) -> Result<String, EngineError> {
        let dir = recur_store::paths::workspace_dir(&self.data_dir, &ws.id)
            .map_err(|e| EngineError(e.to_string()))?;
        let index = dir.join("checkpoint-index");
        recur_git::checkpoint(&ws.worktree_path, &index.to_string_lossy(), ref_name)
            .await
            .map_err(|e| EngineError(e.to_string()))
    }

    /// The in-flight user message a restart remount may continue: it has a
    /// start checkpoint and no footer yet. Empty means AutoResume must open
    /// a distinct turn so a finished prompt's end ref is not overwritten.
    pub async fn last_open_chat_prompt_id(&self, ws: &Workspace, code: &str) -> String {
        if ws.worktree_path.is_empty() {
            return String::new();
        }
        let Some(sess) = self.store.session_for_loop_agent(&ws.id, code) else {
            return String::new();
        };
        let msgs = self
            .store
            .session_transcript(&sess.id)
            .ok()
            .flatten()
            .unwrap_or_default();
        let mut user_id = String::new();
        let mut closed = false;
        for m in &msgs {
            if m.role == "user" && user_message_is_chat_prompt(m) {
                user_id = m.id.clone();
                closed = false;
            } else if !user_id.is_empty() && m.duration_ms > 0 {
                closed = true;
            }
        }
        if user_id.is_empty() || closed {
            return String::new();
        }
        let start = checkpoint_ref(&ws.id, &user_id, "start");
        let refs = list_refs(Duration::from_secs(10), &ws.worktree_path, &start).await;
        if refs.get(&start).is_none_or(String::is_empty) {
            return String::new();
        }
        user_id
    }

    /// Flags turn footers whose start → end snapshot changed files, so the
    /// UI offers "diff this turn" only where it shows something.
    pub async fn mark_checkpoints(&self, ws: &Workspace, msgs: &[Message], items: &mut [ChatItem]) {
        if ws.worktree_path.is_empty() {
            return;
        }
        let prefix = format!("{CHECKPOINT_REF_PREFIX}{}/", ws.id);
        let trees = within(
            Duration::from_secs(10),
            recur_git::list_checkpoint_trees(&ws.worktree_path, &prefix),
        )
        .await
        .flatten()
        .unwrap_or_default();
        if trees.is_empty() {
            return;
        }
        let mut users: HashMap<&str, String> = HashMap::with_capacity(msgs.len());
        let mut user = String::new();
        for m in msgs {
            if m.role == "user" && user_message_is_chat_prompt(m) {
                user = m.id.clone();
            }
            users.insert(&m.id, user.clone());
        }
        for it in items.iter_mut() {
            if it.kind != "turn" {
                continue;
            }
            let Some(user) = users.get(it.message_id.as_str()).filter(|u| !u.is_empty()) else {
                continue;
            };
            let start = trees.get(&checkpoint_ref(&ws.id, user, "start"));
            let end = trees.get(&checkpoint_ref(&ws.id, user, "end"));
            it.checkpoint = match (start, end) {
                (Some(s), Some(e)) => !s.is_empty() && !e.is_empty() && s != e,
                _ => false,
            };
        }
    }

    /// A footer id on a tab resolved to (workspace, session, user message).
    fn chat_turn(
        &self,
        workspace_id: &str,
        code: &str,
        footer_id: &str,
    ) -> Result<(Workspace, Session, String), EngineError> {
        let code = normalize_pair_chat_code(code);
        let ws = self
            .store
            .workspace(workspace_id)
            .ok_or_else(|| EngineError::new("workspace not found"))?;
        let sess = self
            .store
            .session_for_loop_agent(workspace_id, &code)
            .ok_or_else(|| EngineError::new("chat has no session"))?;
        let msgs = self
            .store
            .session_transcript(&sess.id)
            .map_err(|e| EngineError(e.to_string()))?
            .unwrap_or_default();
        let user_id = turn_user_message_id(&msgs, footer_id);
        if user_id.is_empty() {
            return Err(EngineError::new("turn not found"));
        }
        Ok((ws, sess, user_id))
    }

    /// What the agent changed during the turn that ends at footer
    /// `message_id`: start against end (captured now when the end is
    /// missing), shaped as a message-less commit for the review surface.
    pub async fn workspace_chat_turn_diff(
        &self,
        workspace_id: &str,
        code: &str,
        message_id: &str,
    ) -> Result<Commit, EngineError> {
        let (ws, _, user_id) = self.chat_turn(workspace_id, code, message_id)?;
        let d = Duration::from_secs(60);
        let mut refs = list_refs(d, &ws.worktree_path, &checkpoint_ref(&ws.id, &user_id, "")).await;
        let start = checkpoint_ref(&ws.id, &user_id, "start");
        let end = checkpoint_ref(&ws.id, &user_id, "end");
        if refs.get(&start).is_none_or(String::is_empty) {
            return Err(EngineError::new("no checkpoint for this turn"));
        }
        if refs.get(&end).is_none_or(String::is_empty) {
            self.take_checkpoint(&ws, &user_id, "end").await;
            refs = list_refs(d, &ws.worktree_path, &end).await;
        }
        let files = within(
            d,
            recur_git::diff_checkpoints(&ws.worktree_path, &start, &end),
        )
        .await
        .ok_or_else(|| EngineError::new("context deadline exceeded"))?
        .map_err(|e| EngineError(e.to_string()))?;
        let sha = refs.get(&end).cloned().unwrap_or_default();
        let mut diff = DiffStat::default();
        for f in &files {
            diff.add += f.diff.add;
            diff.del += f.diff.del;
        }
        let hash = if sha.len() > 7 {
            sha[..7].to_owned()
        } else {
            sha.clone()
        };
        Ok(Commit {
            hash,
            sha,
            files: Some(files),
            diff,
            ..Commit::default()
        })
    }

    /// Rewinds a tab to the end of the turn closed by `message_id`: later
    /// messages are dropped and, when a later turn changed files, the
    /// worktree is restored to that turn's end checkpoint. The provider
    /// session is discarded; the next send re-injects the truncated
    /// transcript.
    pub async fn reset_workspace_chat(
        &self,
        workspace_id: &str,
        code: &str,
        message_id: &str,
    ) -> Result<(), EngineError> {
        let code = normalize_pair_chat_code(code);
        if self.chat_is_running(workspace_id, &code) {
            return Err(EngineError::new("stop the running turn first"));
        }
        let (ws, sess, user_id) = self.chat_turn(workspace_id, &code, message_id)?;
        // A Maestro-routed send never replays the fork inject, so a reset
        // would wipe the agent's memory while the UI still shows the turns.
        if code == MAESTRO_AGENT_CODE
            || self.chat_loop_started(&ws.id, &code)
            || (ws.has_run() && loop_owner_code(&ws) == code)
        {
            return Err(EngineError::new(
                "reset is not available on a running loop's chat",
            ));
        }
        let msgs = self
            .store
            .session_transcript(&sess.id)
            .map_err(|e| EngineError(e.to_string()))?
            .unwrap_or_default();
        // Only conversation follows: leave manual edits alone.
        if self.edited_after(&ws, &msgs, message_id).await {
            let d = Duration::from_secs(120);
            let end = checkpoint_ref(&ws.id, &user_id, "end");
            let refs = list_refs(d, &ws.worktree_path, &end).await;
            if refs.get(&end).is_none_or(String::is_empty) {
                return Err(EngineError::new("no checkpoint for this turn"));
            }
            within(d, recur_git::restore_checkpoint(&ws.worktree_path, &end))
                .await
                .ok_or_else(|| EngineError::new("context deadline exceeded"))?
                .map_err(|e| EngineError(e.to_string()))?;
        }
        self.store
            .truncate_session_after(&sess.id, message_id)
            .map_err(|e| EngineError(e.to_string()))?;
        self.clear_agent_session(workspace_id, &code);
        self.store
            .patch_session_config_merge(&sess.id, r#"{"forkInject":true}"#)
            .map_err(|e| EngineError(e.to_string()))
    }

    /// Whether any turn after footer `footer_id` changed files (the rule
    /// that flags footers in the UI).
    async fn edited_after(&self, ws: &Workspace, msgs: &[Message], footer_id: &str) -> bool {
        let mut later = Vec::new();
        let mut past = false;
        for m in msgs {
            if past && m.duration_ms > 0 {
                later.push(ChatItem {
                    kind: "turn".into(),
                    message_id: m.id.clone(),
                    ..ChatItem::default()
                });
            }
            past = past || m.id == footer_id;
        }
        self.mark_checkpoints(ws, msgs, &mut later).await;
        later.iter().any(|it| it.checkpoint)
    }

    /// Snapshots the worktree for a `/review` or `/re-review` pass and
    /// returns (since, now) when the pass should embed only what changed
    /// since this tab's previous review: `/re-review` with an earlier pass
    /// and a live provider session. Otherwise ("", "") and the caller embeds
    /// the full diff in a fresh session.
    pub async fn review_checkpoint(
        &self,
        workspace_id: &str,
        code: &str,
        slash: &str,
    ) -> (String, String) {
        let none = (String::new(), String::new());
        if slash != "review" && slash != "re-review" {
            return none;
        }
        let code = normalize_pair_chat_code(code);
        let Some(ws) = self.store.workspace(workspace_id) else {
            return none;
        };
        if ws.worktree_path.is_empty() {
            return none;
        }
        let d = Duration::from_secs(120);
        let r = format!("{CHECKPOINT_REF_PREFIX}{}/review/{code}", ws.id);
        let prev = list_refs(d, &ws.worktree_path, &r)
            .await
            .remove(&r)
            .unwrap_or_default();
        let oid = match within(d, self.checkpoint(&ws, &r)).await {
            Some(Ok(oid)) => oid,
            Some(Err(e)) => {
                tracing::warn!("review checkpoint {}/{code}: {e}", ws.id);
                return none;
            }
            None => {
                tracing::warn!(
                    "review checkpoint {}/{code}: context deadline exceeded",
                    ws.id
                );
                return none;
            }
        };
        let session = self
            .store
            .agent(workspace_id, &code)
            .map(|a| a.session_id)
            .unwrap_or_default();
        if slash != "re-review" || prev.is_empty() || session.is_empty() {
            return none;
        }
        (prev, oid)
    }

    /// Whether the tab's agent has a turn in flight.
    pub fn chat_is_running(&self, workspace_id: &str, code: &str) -> bool {
        self.store
            .agent(workspace_id, code)
            .is_some_and(|a| a.status == "working")
    }
}

#[cfg(test)]
#[path = "checkpoint_tests.rs"]
mod tests;
