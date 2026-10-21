//! The chat concurrency kernel (Go `chat_cancel.go`).
//!
//! - [`ChatTurn`]: one in-flight chat turn's cancel handle, its park / stop /
//!   timeout flags, its armed wedged-turn deadline, its `ended` signal
//!   (fired exactly once, in `unregister_chat_turn`) and its steer channel.
//! - [`ChatTabSlot`]: per-tab FIFO send order and the stop generation, so a
//!   queued send that loses its turn to Stop returns without running.
//! - Life locks: a ref-counted async mutex per tab that serializes reopen,
//!   send prologue, create/close and closed-chat delete.
//! - Continue generations for live switch_mode remounts.
//! - Delete/hoist tombstones that refuse new chats on a dying workspace.
//!
//! Go's `chatTabSlot.claim` returned holding the slot mutex and the send
//! path released it around blocking work with `pause`/`resume`. Here
//! [`ChatTabSlot::claim`] waits for the caller's turn without holding
//! anything, and the short sync sections Go ran under the mutex take
//! [`ChatTabSlot::lock`]; [`ChatTabSlot::finish`] consumes that guard.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::{Mutex, MutexGuard};
use recur_agent::{Steer, SteerInbox};
use tokio::sync::{Notify, OwnedMutexGuard, mpsc};
use tokio::task::AbortHandle;
use tokio_util::sync::CancellationToken;

use crate::Engine;
use crate::codes::{
    chat_life_key, chat_turn_key, is_app_chat_code, is_pair_chat_code, normalize_pair_chat_code,
};

#[derive(Default)]
struct TurnFlags {
    armed: Duration,
    timer: Option<AbortHandle>,
    parked: bool,
    /// User Stop (CancelChat); a park must not override it.
    stopped: bool,
    timed_out: bool,
    /// A live Send owns this turn and will unregister it. CancelChat fires
    /// `ended` itself only for unowned handles.
    owned: bool,
    /// The steer channel's sending half while armed.
    inject: Option<mpsc::Sender<Steer>>,
    /// The turn is unwinding: refuse new steers.
    inject_off: bool,
}

/// One in-flight chat turn. Compared by pointer (`Arc::ptr_eq`) so a stale
/// handle can never unregister a newer turn.
pub struct ChatTurn {
    cancel: CancellationToken,
    flags: Mutex<TurnFlags>,
    ended: CancellationToken,
    /// Fired by `stop_chat_inject` so a blocked `try_inject` returns.
    inject_done: CancellationToken,
}

impl ChatTurn {
    fn new(cancel: CancellationToken, owned: bool) -> Arc<Self> {
        Arc::new(Self {
            cancel,
            flags: Mutex::new(TurnFlags {
                owned,
                ..TurnFlags::default()
            }),
            ended: CancellationToken::new(),
            inject_done: CancellationToken::new(),
        })
    }

    /// The turn's cancel token.
    pub fn cancel_token(&self) -> &CancellationToken {
        &self.cancel
    }

    /// Fires once the turn has dropped status ownership (unregistered).
    pub fn ended(&self) -> CancellationToken {
        self.ended.clone()
    }

    /// Opens the mid-turn steer channel and returns the receiving half for
    /// the provider (`TurnContext.steer`). `None` once the turn is
    /// unwinding or when already armed.
    pub fn arm_chat_inject(&self) -> Option<SteerInbox> {
        let mut f = self.flags.lock();
        if f.inject_off || f.inject.is_some() {
            return None;
        }
        let (tx, rx) = mpsc::channel(1);
        f.inject = Some(tx);
        Some(SteerInbox(rx))
    }

    /// Stops accepting steers and unblocks any waiting `try_inject`.
    pub fn stop_chat_inject(&self) {
        let mut f = self.flags.lock();
        f.inject_off = true;
        f.inject = None;
        drop(f);
        self.inject_done.cancel();
    }

    fn close_ended(&self) {
        self.ended.cancel();
    }

    /// Delivers one plain follow-up into the live turn. True only after the
    /// provider acked delivery; false when nothing is armed, the turn was
    /// stopped/parked/timed out, or it unwound first.
    pub async fn try_inject(&self, prompt: &str, display: &str) -> bool {
        if prompt.trim().is_empty() && display.trim().is_empty() {
            return false;
        }
        let tx = {
            let f = self.flags.lock();
            if f.inject_off || f.stopped || f.parked || f.timed_out {
                return false;
            }
            match &f.inject {
                Some(tx) => tx.clone(),
                None => return false,
            }
        };
        let (steer, mut ack) = Steer::new(prompt.trim(), Some(display.trim().to_owned()));
        tokio::select! {
            r = tx.send(steer) => {
                if r.is_err() {
                    return false;
                }
            }
            _ = self.inject_done.cancelled() => return false,
        }
        tokio::select! {
            r = &mut ack => r.unwrap_or(false),
            _ = self.inject_done.cancelled() => ack.try_recv().unwrap_or(false),
        }
    }

    pub fn mark_parked(&self) {
        self.flags.lock().parked = true;
        self.stop_chat_inject();
    }

    pub fn mark_stopped(&self) {
        self.flags.lock().stopped = true;
        self.stop_chat_inject();
    }

    /// Marks a handle with no owning send (tests), so Stop fires `ended`.
    pub fn drop_owner(&self) {
        self.flags.lock().owned = false;
    }

    pub fn is_owned(&self) -> bool {
        self.flags.lock().owned
    }

    pub fn is_stopped(&self) -> bool {
        self.flags.lock().stopped
    }

    pub fn is_parked(&self) -> bool {
        self.flags.lock().parked
    }

    pub fn is_timed_out(&self) -> bool {
        self.flags.lock().timed_out
    }

    /// Starts (or restarts) the wedged-turn deadline: when it fires the turn
    /// is marked timed out and cancelled.
    pub fn arm(self: &Arc<Self>, d: Duration) {
        if d.is_zero() {
            return;
        }
        let mut f = self.flags.lock();
        f.armed = d;
        self.rearm_locked(&mut f);
    }

    /// Stops a pending deadline (the turn is ending).
    pub fn disarm(&self) {
        let mut f = self.flags.lock();
        if let Some(t) = f.timer.take() {
            t.abort();
        }
        f.armed = Duration::ZERO;
    }

    fn rearm_locked(self: &Arc<Self>, f: &mut TurnFlags) {
        if f.armed.is_zero() {
            return;
        }
        if let Some(t) = f.timer.take() {
            t.abort();
        }
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let turn = Arc::downgrade(self);
        let d = f.armed;
        let h = rt.spawn(async move {
            tokio::time::sleep(d).await;
            if let Some(t) = turn.upgrade() {
                t.flags.lock().timed_out = true;
                t.stop_chat_inject();
                t.cancel.cancel();
            }
        });
        f.timer = Some(h.abort_handle());
    }
}

/// FIFO counters of one tab: `next` = sends begun, `cur` = sends finished.
#[derive(Debug, Default)]
pub struct SlotCounters {
    pub next: u64,
    pub cur: u64,
}

/// Per-tab send order and stop generation.
#[derive(Default)]
pub struct ChatTabSlot {
    counters: Mutex<SlotCounters>,
    turn: Notify,
    stop_gen: AtomicU64,
}

impl ChatTabSlot {
    /// Takes a ticket: `(seq, stop_gen)` at begin.
    pub fn begin(&self) -> (u64, u64) {
        let mut c = self.counters.lock();
        c.next += 1;
        (c.next, self.stop_gen.load(Ordering::SeqCst))
    }

    /// Waits until every earlier ticket finished.
    pub async fn claim(&self, seq: u64) {
        loop {
            let notified = self.turn.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.counters.lock().cur + 1 == seq {
                return;
            }
            notified.await;
        }
    }

    /// The slot's sync critical section (Go's held slot mutex).
    pub fn lock(&self) -> MutexGuard<'_, SlotCounters> {
        self.counters.lock()
    }

    /// Ends the claimed ticket and wakes the next one.
    pub fn finish(&self, mut held: MutexGuard<'_, SlotCounters>) {
        held.cur += 1;
        drop(held);
        self.turn.notify_waiters();
    }

    /// The current stop generation (bumped by every CancelChat).
    pub fn stop_gen(&self) -> u64 {
        self.stop_gen.load(Ordering::SeqCst)
    }

    fn bump_stop_gen(&self) {
        self.stop_gen.fetch_add(1, Ordering::SeqCst);
    }
}

/// One tab's life lock and how many holders/waiters reference it (guarded
/// by `Engine::chat_life`, not by the lock itself, so retain and retire
/// cannot split a key across two locks).
pub struct LifeSlot {
    mu: Arc<tokio::sync::Mutex<()>>,
    refs: usize,
}

/// A held (or refused) per-tab life lock. `unlock` is idempotent; Drop
/// unlocks.
pub struct ChatLifeGuard {
    engine: Option<Arc<Engine>>,
    key: String,
    workspace_id: String,
    mu: Option<Arc<tokio::sync::Mutex<()>>>,
    guard: Option<OwnedMutexGuard<()>>,
}

impl ChatLifeGuard {
    fn refused() -> Self {
        Self {
            engine: None,
            key: String::new(),
            workspace_id: String::new(),
            mu: None,
            guard: None,
        }
    }

    /// Releases the tab. Safe to call more than once.
    pub fn unlock(&mut self) {
        let Some(e) = self.engine.take() else {
            return;
        };
        self.guard = None;
        if let Some(mu) = self.mu.take() {
            let mut map = e.chat_life.lock();
            let remove = match map.get_mut(&self.key) {
                Some(slot) if Arc::ptr_eq(&slot.mu, &mu) => {
                    slot.refs = slot.refs.saturating_sub(1);
                    slot.refs == 0
                }
                _ => false,
            };
            if remove {
                map.remove(&self.key);
            }
        }
        e.maybe_archive_linked_pr_chat_host(&self.workspace_id);
    }
}

impl Drop for ChatLifeGuard {
    fn drop(&mut self) {
        self.unlock();
    }
}

/// The chat a live switch_mode remount generation belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinueId {
    pub workspace_id: String,
    pub code: String,
}

#[derive(Default)]
pub struct ContinueGens {
    next: u64,
    live: HashMap<u64, ContinueId>,
}

impl Engine {
    /// The FIFO slot of one tab (created on first use, kept for the process).
    pub fn chat_tab_slot(&self, workspace_id: &str, code: &str) -> Arc<ChatTabSlot> {
        let key = chat_turn_key(workspace_id, code);
        Arc::clone(self.chat_tabs.lock().entry(key).or_default())
    }

    /// Serializes reopen, send prologue, create/close, dock Remember and
    /// closed-chat delete for one tab. The bool is false (and the guard
    /// holds nothing) while the workspace is being removed, hoisted, or is
    /// archived.
    pub async fn lock_chat_life(&self, workspace_id: &str, code: &str) -> (ChatLifeGuard, bool) {
        let key = chat_life_key(workspace_id, code);
        let mu = {
            let mut map = self.chat_life.lock();
            if self.chat_life_reserved(workspace_id) {
                return (ChatLifeGuard::refused(), false);
            }
            let slot = map.entry(key.clone()).or_insert_with(|| LifeSlot {
                mu: Arc::new(tokio::sync::Mutex::new(())),
                refs: 0,
            });
            slot.refs += 1;
            Arc::clone(&slot.mu)
        };
        // Built before the wait so a dropped future still returns the ref.
        let mut g = ChatLifeGuard {
            engine: Some(self.arc()),
            key,
            workspace_id: workspace_id.to_owned(),
            mu: Some(Arc::clone(&mu)),
            guard: None,
        };
        g.guard = Some(mu.lock_owned().await);
        (g, true)
    }

    /// Allocates a live remount generation for one chat.
    pub fn alloc_continue_gen(&self, workspace_id: &str, code: &str) -> u64 {
        let workspace_id = workspace_id.trim();
        let mut g = self.continue_gens.lock();
        g.next += 1;
        let generation = g.next;
        g.live.insert(
            generation,
            ContinueId {
                workspace_id: workspace_id.to_owned(),
                code: chat_life_key(workspace_id, code),
            },
        );
        generation
    }

    pub fn continue_gen_live(&self, generation: u64) -> bool {
        generation != 0 && self.continue_gens.lock().live.contains_key(&generation)
    }

    /// Drops a remount generation (it ran, was cancelled, or its chat went
    /// away), then lets a deferred PR host archive proceed.
    pub fn drop_continue_gen(&self, generation: u64) {
        if generation == 0 {
            return;
        }
        let Some(id) = self.continue_gens.lock().live.remove(&generation) else {
            return;
        };
        // A send holds the life lock across this call and hoist waits for
        // those refs, so retrying here would deadlock; unlock retries.
        if self.chat_life_busy(&id.workspace_id) {
            return;
        }
        self.maybe_archive_linked_pr_chat_host(&id.workspace_id);
    }

    /// Drops every live generation of a workspace (or one tab).
    pub fn invalidate_continue_gens(&self, workspace_id: &str, code: &str) {
        let workspace_id = workspace_id.trim();
        if workspace_id.is_empty() {
            return;
        }
        let want = (!code.is_empty()).then(|| chat_life_key(workspace_id, code));
        self.continue_gens.lock().live.retain(|_, id| {
            id.workspace_id != workspace_id || want.as_ref().is_some_and(|w| &id.code != w)
        });
    }

    /// A live remount generation belongs to this workspace (a mode switch's
    /// continuation has not run yet).
    pub fn workspace_has_live_continue(&self, workspace_id: &str) -> bool {
        self.continue_gens
            .lock()
            .live
            .values()
            .any(|id| id.workspace_id == workspace_id)
    }

    /// Drops every live generation (ClearAll).
    pub(crate) fn invalidate_all_continue_gens(&self) {
        self.continue_gens.lock().live.clear();
    }

    /// Forgets idle life locks of a workspace and its remount generations.
    pub fn drop_chat_life_for_workspace(&self, workspace_id: &str) {
        let workspace_id = workspace_id.trim();
        if workspace_id.is_empty() {
            return;
        }
        let prefix = format!("{workspace_id}\x00");
        self.chat_life
            .lock()
            .retain(|k, slot| !(k.starts_with(&prefix) && slot.refs == 0));
        self.invalidate_continue_gens(workspace_id, "");
    }

    /// Whether any tab of the workspace holds (or waits on) its life lock.
    pub fn chat_life_busy(&self, workspace_id: &str) -> bool {
        let workspace_id = workspace_id.trim();
        if workspace_id.is_empty() {
            return false;
        }
        let prefix = format!("{workspace_id}\x00");
        self.chat_life
            .lock()
            .iter()
            .any(|(k, slot)| k.starts_with(&prefix) && slot.refs > 0)
    }

    /// Waits until no tab of the workspace holds its life lock, so teardown
    /// cannot race a prologue that already passed its liveness check.
    pub async fn wait_chat_life_idle(&self, workspace_id: &str) {
        while self.chat_life_busy(workspace_id) {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    /// Publishes a turn's cancel so CancelChat can interrupt it. A prior turn
    /// on the same tab is cancelled first (a tab never runs two agents).
    /// `None` while shutting down or when the workspace is being removed.
    pub fn register_chat_turn(
        &self,
        workspace_id: &str,
        code: &str,
        cancel: CancellationToken,
    ) -> Option<Arc<ChatTurn>> {
        if self.is_shutting_down() || self.workspace_remove_blocks_chat(workspace_id) {
            return None;
        }
        let turn = ChatTurn::new(cancel.clone(), true);
        let key = chat_turn_key(workspace_id, code);
        let prev = self.chats.lock().insert(key.clone(), Arc::clone(&turn));
        if let Some(p) = prev {
            p.stop_chat_inject();
            p.cancel.cancel();
        }
        if self.is_shutting_down() || self.workspace_remove_blocks_chat(workspace_id) {
            self.compare_and_delete_chat(&key, &turn);
            cancel.cancel();
            return None;
        }
        Some(turn)
    }

    /// Registers only when the tab is idle (switch_mode remounts, restart
    /// AutoResume), so they cannot displace a user send.
    pub fn register_chat_turn_if_idle(
        &self,
        workspace_id: &str,
        code: &str,
        cancel: CancellationToken,
    ) -> Option<Arc<ChatTurn>> {
        if self.is_shutting_down() || self.workspace_remove_blocks_chat(workspace_id) {
            return None;
        }
        let turn = ChatTurn::new(cancel.clone(), true);
        let key = chat_turn_key(workspace_id, code);
        {
            let mut chats = self.chats.lock();
            if chats.contains_key(&key) {
                return None;
            }
            chats.insert(key.clone(), Arc::clone(&turn));
        }
        if self.is_shutting_down() || self.workspace_remove_blocks_chat(workspace_id) {
            self.compare_and_delete_chat(&key, &turn);
            cancel.cancel();
            return None;
        }
        Some(turn)
    }

    fn compare_and_delete_chat(&self, key: &str, turn: &Arc<ChatTurn>) {
        let mut chats = self.chats.lock();
        if chats.get(key).is_some_and(|t| Arc::ptr_eq(t, turn)) {
            chats.remove(key);
        }
    }

    /// Drops the handle when the turn ends; only the matching turn is
    /// removed. Fires `ended` exactly once.
    pub fn unregister_chat_turn(&self, workspace_id: &str, code: &str, turn: &Arc<ChatTurn>) {
        turn.stop_chat_inject();
        turn.disarm();
        turn.close_ended();
        self.compare_and_delete_chat(&chat_turn_key(workspace_id, code), turn);
        self.forget_child_wave(turn);
        // A send holds the life lock across continuation setup and hoist
        // waits for those refs, so retrying here would deadlock.
        if self.chat_life_busy(workspace_id) {
            return;
        }
        self.maybe_archive_linked_pr_chat_host(workspace_id);
    }

    /// Delivers a follow-up onto the tab's live turn. False means no live
    /// steer channel: the caller starts a new turn.
    pub async fn try_inject_chat(
        &self,
        workspace_id: &str,
        code: &str,
        prompt: &str,
        display: &str,
    ) -> bool {
        match self.live_chat_turn(workspace_id, code) {
            Some(t) => t.try_inject(prompt, display).await,
            None => false,
        }
    }

    /// An in-flight turn on any tab of the workspace.
    pub fn workspace_has_live_chat(&self, id: &str) -> bool {
        if id.is_empty() {
            return false;
        }
        let prefix = format!("{id}\x00");
        self.chats.lock().keys().any(|k| k.starts_with(&prefix))
    }

    /// Remove/Delete is tearing the workspace down, or its row is archived
    /// or gone: no new chat may start. App-scoped chats (id "") pass.
    pub fn workspace_remove_blocks_chat(&self, id: &str) -> bool {
        if id.is_empty() {
            return false;
        }
        if self.deleting.lock().contains(id) {
            return true;
        }
        self.store.workspace(id).is_none_or(|ws| ws.archived)
    }

    /// The deleting or hoisting tombstone is held, or the row is archived.
    /// Missing rows are not reserved.
    pub fn chat_life_reserved(&self, id: &str) -> bool {
        if id.is_empty() {
            return false;
        }
        if self.deleting.lock().contains(id) || self.hoisting.lock().contains(id) {
            return true;
        }
        self.store.workspace(id).is_some_and(|ws| ws.archived)
    }

    /// Marks `id` as tearing down. False when a hoist covers it or another
    /// Remove/Delete already holds the reservation.
    pub fn reserve_deleting(&self, id: &str) -> bool {
        let id = id.trim();
        if id.is_empty() {
            return false;
        }
        let _hoist = self.hoist_mu.lock();
        if self.hoisting.lock().contains(id) {
            return false;
        }
        self.deleting.lock().insert(id.to_owned())
    }

    /// Releases a deleting reservation.
    pub fn release_deleting(&self, id: &str) {
        self.deleting.lock().remove(id);
    }

    /// Whether a deleting reservation is held.
    pub fn is_deleting(&self, id: &str) -> bool {
        self.deleting.lock().contains(id)
    }

    /// Remove/Delete has archived or dropped the row (the tombstone alone is
    /// not enough: teardown may still refuse).
    pub fn workspace_teardown_committed(&self, id: &str) -> bool {
        if id.is_empty() {
            return false;
        }
        self.store.workspace(id).is_none_or(|ws| ws.archived)
    }

    /// Interrupts the tab's turn (Stop). True when a turn was cancelled, a
    /// ticketed send was stopped before registering, the tab's loop walk
    /// was idled, or a zombie live status was healed.
    pub async fn cancel_chat(&self, workspace_id: &str, code: &str) -> bool {
        enum Found {
            Live(Arc<ChatTurn>, bool),
            Ticketed,
            Idle,
        }
        let slot = self.chat_tab_slot(workspace_id, code);
        let found = {
            let held = slot.lock();
            if let Some(turn) = self.live_chat_turn(workspace_id, code) {
                slot.bump_stop_gen();
                turn.mark_stopped();
                let owned = turn.is_owned();
                if !owned {
                    // No owning send will unregister: fire ended so a waiter
                    // is not stuck, then drop the handle after cancel.
                    turn.close_ended();
                }
                Found::Live(turn, owned)
            } else if held.next > held.cur {
                // A ticket is taken but the turn is not registered yet.
                slot.bump_stop_gen();
                Found::Ticketed
            } else {
                Found::Idle
            }
        };
        match found {
            Found::Live(turn, owned) => {
                self.cancel_mode_continue(workspace_id, code);
                turn.cancel.cancel();
                if !owned {
                    self.compare_and_delete_chat(&chat_turn_key(workspace_id, code), &turn);
                }
                self.idle_walk_by_chat(workspace_id, code).await;
                true
            }
            Found::Ticketed => {
                self.heal_zombie_chat(workspace_id, code).await;
                self.idle_walk_by_chat(workspace_id, code).await;
                true
            }
            Found::Idle => {
                if self.idle_walk_by_chat(workspace_id, code).await {
                    return true;
                }
                self.heal_zombie_chat(workspace_id, code).await
            }
        }
    }

    /// Parks the tab's live turn unconditionally (tests).
    pub fn park_chat_turn(&self, workspace_id: &str, code: &str) -> bool {
        let live = self.live_chat_turn(workspace_id, code);
        self.park_chat_turn_if(workspace_id, code, live.as_ref())
    }

    /// Parks `expect` only while it is still the tab's live turn and was not
    /// stopped, so answering a parked ask_user cannot cancel a newer send.
    pub fn park_chat_turn_if(
        &self,
        workspace_id: &str,
        code: &str,
        expect: Option<&Arc<ChatTurn>>,
    ) -> bool {
        let Some(expect) = expect else {
            return false;
        };
        let Some(turn) = self.live_chat_turn(workspace_id, code) else {
            return false;
        };
        if !Arc::ptr_eq(&turn, expect) || turn.is_stopped() {
            return false;
        }
        turn.mark_parked();
        turn.cancel.cancel();
        true
    }

    /// The live turn was parked for ask_user / switch_mode.
    pub fn chat_turn_was_parked(&self, workspace_id: &str, code: &str) -> bool {
        self.live_chat_turn(workspace_id, code)
            .is_some_and(|t| t.is_parked())
    }

    /// The live turn's wedged deadline fired.
    pub fn chat_turn_was_timed_out(&self, workspace_id: &str, code: &str) -> bool {
        self.live_chat_turn(workspace_id, code)
            .is_some_and(|t| t.is_timed_out())
    }

    /// The tab's live turn. App-scoped turns use an empty workspace with an
    /// APP/APx code; any other empty-workspace lookup is invalid.
    pub fn live_chat_turn(&self, workspace_id: &str, code: &str) -> Option<Arc<ChatTurn>> {
        let workspace_id = workspace_id.trim();
        let code = code.trim();
        if code.is_empty() {
            return None;
        }
        if workspace_id.is_empty() && !is_app_chat_code(code) {
            return None;
        }
        let code = if is_pair_chat_code(code) {
            normalize_pair_chat_code(code)
        } else {
            code.to_owned()
        };
        self.chats
            .lock()
            .get(&chat_turn_key(workspace_id, &code))
            .cloned()
    }

    /// The prior turn's unregister signal; `None` when the tab is idle.
    pub fn chat_turn_ended(&self, workspace_id: &str, code: &str) -> Option<CancellationToken> {
        self.live_chat_turn(workspace_id, code).map(|t| t.ended())
    }

    /// Every live turn (Shutdown, ClearAll, child removal).
    pub(crate) fn live_chat_turns(&self) -> Vec<(String, Arc<ChatTurn>)> {
        self.chats
            .lock()
            .iter()
            .map(|(k, t)| (k.clone(), Arc::clone(t)))
            .collect()
    }
}

#[cfg(test)]
mod tests;
