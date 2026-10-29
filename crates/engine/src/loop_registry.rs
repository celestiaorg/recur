//! The running-loop registry (Go `reporter.go` lookups + `loopExited`):
//! which walks are live, keyed by workspace id (or workspace + `\x1e` +
//! chat for a second walk), and how callers find the one they mean.

use std::sync::Arc;

use recur_domain::{Workspace, WorkspaceState};
use tokio_util::sync::CancellationToken;

use crate::Engine;
use crate::codes::loop_owner_code;
use crate::loop_handle::{Command, LoopHandle};
use crate::loops::WrapUpInputs;

/// Separates the workspace id from the chat code in a second walk's key.
pub const LOOP_MAP_SEP: &str = "\x1e";

/// A live walk: its control plane and the cancel that stops its agents.
#[derive(Clone)]
pub struct RunningLoop {
    pub handle: Arc<LoopHandle>,
    pub cancel: CancellationToken,
}

impl Engine {
    /// The registry key for a dispatch of `rec`: the workspace id, unless a
    /// walk already holds it and this run names a conductor.
    pub fn loop_map_key(&self, rec: &Workspace) -> String {
        let c = rec.loop_chat_code.trim();
        if c.is_empty() {
            return rec.id.clone();
        }
        if self.loops.lock().contains_key(&rec.id) {
            return format!("{}{LOOP_MAP_SEP}{c}", rec.id);
        }
        rec.id.clone()
    }

    /// Registers a walk under its key. False when the key is taken (a
    /// concurrent dispatch won, D26).
    pub fn register_loop(&self, rl: RunningLoop) -> bool {
        let mut loops = self.loops.lock();
        if loops.contains_key(&rl.handle.map_key) {
            return false;
        }
        loops.insert(rl.handle.map_key.clone(), rl);
        true
    }

    /// Drops a walk's registry entry (only the same walk).
    pub fn remove_loop(&self, handle: &Arc<LoopHandle>) {
        let mut loops = self.loops.lock();
        if loops
            .get(&handle.map_key)
            .is_some_and(|rl| Arc::ptr_eq(&rl.handle, handle))
        {
            loops.remove(&handle.map_key);
        }
    }

    /// Whether a walk is registered under exactly this key (Go
    /// `e.loops.Load(id)`).
    pub fn loop_key_running(&self, key: &str) -> bool {
        self.loops.lock().contains_key(key)
    }

    /// Every live walk on the workspace: the primary one first.
    pub fn running_loops_on(&self, workspace_id: &str) -> Vec<RunningLoop> {
        let loops = self.loops.lock();
        let mut out = Vec::new();
        if let Some(rl) = loops.get(workspace_id) {
            out.push(rl.clone());
        }
        let prefix = format!("{workspace_id}{LOOP_MAP_SEP}");
        for (k, rl) in loops.iter() {
            if k.starts_with(&prefix) {
                out.push(rl.clone());
            }
        }
        out
    }

    /// Every live walk (Shutdown, ClearAll).
    pub(crate) fn all_running_loops(&self) -> Vec<RunningLoop> {
        self.loops.lock().values().cloned().collect()
    }

    /// A live walk on the workspace.
    pub fn running_loop_by_id(&self, workspace_id: &str) -> Option<Arc<LoopHandle>> {
        self.running_loops_on(workspace_id)
            .into_iter()
            .next()
            .map(|rl| rl.handle)
    }

    /// The walk a done call belongs to: the one specialist that claimed it,
    /// else the only walk. Ambiguous claims return `None`.
    pub fn running_loop_for_done(
        &self,
        workspace_id: &str,
        agent_code: &str,
    ) -> Option<Arc<LoopHandle>> {
        let all = self.running_loops_on(workspace_id);
        let claimed: Vec<&RunningLoop> = all
            .iter()
            .filter(|rl| rl.handle.holds_done_turn(agent_code))
            .collect();
        if claimed.len() == 1 {
            return Some(Arc::clone(&claimed[0].handle));
        }
        if all.len() == 1 {
            return Some(Arc::clone(&all[0].handle));
        }
        None
    }

    /// The walk driven by this chat tab (or a legacy walk with no conductor).
    pub fn running_loop_by_chat(&self, workspace_id: &str, code: &str) -> Option<Arc<LoopHandle>> {
        let code = code.trim();
        let loops = self.loops.lock();
        if let Some(rl) = loops.get(&format!("{workspace_id}{LOOP_MAP_SEP}{code}")) {
            return Some(Arc::clone(&rl.handle));
        }
        let rl = loops.get(workspace_id)?;
        let owner = rl.handle.chat_code();
        if owner.is_empty() || owner == code {
            return Some(Arc::clone(&rl.handle));
        }
        None
    }

    /// Whether this tab conducts a live walk.
    pub fn chat_drives_live_loop(&self, workspace_id: &str, code: &str) -> bool {
        self.running_loop_by_chat(workspace_id, code)
            .is_some_and(|h| h.chat_code() == code.trim())
    }

    /// A Maestro hand's target: this chat's walk when a code is given, else
    /// any walk (UI / tests).
    pub fn running_loop_for_hand(
        &self,
        workspace_id: &str,
        agent_code: &str,
    ) -> Option<Arc<LoopHandle>> {
        if agent_code.trim().is_empty() {
            self.running_loop_by_id(workspace_id)
        } else {
            self.running_loop_by_chat(workspace_id, agent_code)
        }
    }

    /// The walk this tab owns, including a legacy walk (no ChatCode) only
    /// when the tab is the workspace's conductor.
    pub fn running_loop_record_by_chat(
        &self,
        workspace_id: &str,
        code: &str,
    ) -> Option<RunningLoop> {
        let code = code.trim();
        let rl = {
            let loops = self.loops.lock();
            if let Some(rl) = loops.get(&format!("{workspace_id}{LOOP_MAP_SEP}{code}")) {
                return Some(rl.clone());
            }
            loops.get(workspace_id)?.clone()
        };
        let owner = rl.handle.chat_code();
        if owner == code {
            return Some(rl);
        }
        if !owner.is_empty() {
            return None;
        }
        let ws = self.store.workspace(workspace_id)?;
        (loop_owner_code(&ws) == code).then_some(rl)
    }

    /// Stops this tab's walk (closing a tab: StateStopped). Sibling chats'
    /// walks keep running.
    pub fn stop_loop_by_chat(&self, workspace_id: &str, code: &str) {
        if let Some(rl) = self.running_loop_record_by_chat(workspace_id, code) {
            rl.handle.send(Command::Stop);
            rl.cancel.cancel();
        }
    }

    /// Composer Stop on a conductor tab: cancel its walk and park it idle
    /// (the checkpoint stays for reenter_phase), or idle a queued run the tab
    /// owns. True when something was idled.
    pub async fn idle_walk_by_chat(&self, workspace_id: &str, code: &str) -> bool {
        if let Some(rl) = self.running_loop_record_by_chat(workspace_id, code) {
            rl.handle.request_idle();
            rl.cancel.cancel();
            return true;
        }
        let Some(ws) = self.store.workspace(workspace_id) else {
            return false;
        };
        if loop_owner_code(&ws) != code || ws.state != WorkspaceState::Queued {
            return false;
        }
        let _d = self.dispatch_mu.lock().await;
        if let Some(rl) = self.running_loop_record_by_chat(workspace_id, code) {
            rl.handle.request_idle();
            rl.cancel.cancel();
            return true;
        }
        let Some(ws) = self.store.workspace(workspace_id) else {
            return false;
        };
        if loop_owner_code(&ws) != code || ws.state != WorkspaceState::Queued {
            return false;
        }
        self.set_workspace_state(workspace_id, WorkspaceState::Paused, "idle");
        true
    }

    /// A walk task returned: drop it from the registry, release its slot,
    /// run the settled wrap-up (`wrap` is `None` after a panic), badge the
    /// conductor when it finished off-screen, and promote the oldest queued
    /// loop (D23).
    pub async fn loop_exited(&self, handle: &Arc<LoopHandle>, wrap: Option<WrapUpInputs>) {
        self.remove_loop(handle);
        self.gov.release_loop();
        self.publish_resource();
        if let Some(w) = &wrap {
            self.emit_settled_wrap_up(handle, w).await;
        }
        self.maybe_flag_loop_finished(handle).await;
        self.drain_queued().await;
    }

    /// Resets the verify collector before the QA agent runs.
    pub fn begin_verify(&self, handle: &LoopHandle) {
        handle.reset_evidence();
        self.store
            .update_workspace(&handle.id, |ws| ws.test_result = None);
    }
}
