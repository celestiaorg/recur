//! The machine-global resource limits (PRD §6.4): a cap on concurrently running
//! loops and on concurrently running agent turns.
//!
//! Loop slots never block (a loop that cannot get one stays queued). Agent
//! slots block until one frees or the caller is cancelled; a release wakes
//! one waiter and a cap change wakes all. Caps ≤ 0 mean unlimited, and
//! lowering a cap never preempts in-flight work.

mod keep_awake;

pub use keep_awake::AgentPermit;

use parking_lot::Mutex;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Default)]
struct State {
    max_loops: i64,
    max_agents: i64,
    loops: i64,
    agents: i64,
    /// `acquire_agent` callers blocked on the cap.
    waiting: i64,
    /// Review iterations completed machine-wide ("iters today").
    iters: i64,
}

pub struct Governor {
    state: Mutex<State>,
    agent_wait: Notify,
}

fn at_cap(n: i64, max: i64) -> bool {
    max > 0 && n >= max
}

impl Governor {
    /// Non-positive caps are unlimited so a misconfigured 0 never wedges
    /// the engine.
    pub fn new(max_loops: i64, max_agents: i64) -> Self {
        Self {
            state: Mutex::new(State {
                max_loops,
                max_agents,
                ..State::default()
            }),
            agent_wait: Notify::new(),
        }
    }

    /// Updates the live caps and wakes every agent waiter. Raising the loop
    /// cap does not start queued loops; the caller drains them.
    pub fn set_caps(&self, max_loops: i64, max_agents: i64) {
        let mut s = self.state.lock();
        s.max_loops = max_loops;
        s.max_agents = max_agents;
        drop(s);
        self.agent_wait.notify_waiters();
    }

    /// Takes a loop slot without blocking; false when all are in use.
    pub fn try_acquire_loop(&self) -> bool {
        let mut s = self.state.lock();
        if at_cap(s.loops, s.max_loops) {
            return false;
        }
        s.loops += 1;
        true
    }

    /// How many more loops may start now (a large number when unlimited).
    pub fn free_loops(&self) -> i64 {
        let s = self.state.lock();
        if s.max_loops <= 0 {
            return 1 << 20;
        }
        (s.max_loops - s.loops).max(0)
    }

    /// Returns a loop slot. Call once per successful acquire.
    pub fn release_loop(&self) {
        let mut s = self.state.lock();
        if s.loops > 0 {
            s.loops -= 1;
        }
    }

    /// Waits for an agent slot; true once taken. False when `cancel` fired
    /// before one freed (a free slot is still taken when `cancel` already
    /// fired). Pair every true with one [`Governor::release_agent`].
    pub async fn acquire_agent(&self, cancel: &CancellationToken) -> bool {
        loop {
            let notified = self.agent_wait.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut s = self.state.lock();
                if !at_cap(s.agents, s.max_agents) {
                    s.agents += 1;
                    return true;
                }
                if cancel.is_cancelled() {
                    return false;
                }
                s.waiting += 1;
            }
            tokio::select! {
                _ = &mut notified => {}
                _ = cancel.cancelled() => {}
            }
            self.state.lock().waiting -= 1;
            if cancel.is_cancelled() {
                // Pass a wake we may have consumed on to the next waiter.
                self.agent_wait.notify_one();
                return false;
            }
        }
    }

    /// Returns an agent slot and wakes one waiter.
    pub fn release_agent(&self) {
        let mut s = self.state.lock();
        if s.agents > 0 {
            s.agents -= 1;
        }
        drop(s);
        self.agent_wait.notify_one();
    }

    /// Counts completed review iterations into "iters today".
    pub fn add_iters(&self, n: i64) {
        self.state.lock().iters += n;
    }

    pub fn iters(&self) -> i64 {
        self.state.lock().iters
    }

    pub fn loops_running(&self) -> i64 {
        self.state.lock().loops
    }

    pub fn agents_running(&self) -> i64 {
        self.state.lock().agents
    }

    pub fn agents_waiting(&self) -> i64 {
        self.state.lock().waiting
    }
}

#[cfg(test)]
mod tests;
