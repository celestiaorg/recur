//! Agent slots with keep-awake: while any agent turn holds a slot and
//! Settings.PreventSleepWhileAgentsRun is on, a macOS `caffeinate` assertion
//! suppresses idle sleep. The last release also tells providers the machine
//! went idle (Cursor's deferred bridge close).

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::Engine;

/// One held agent slot. Dropping it (or [`AgentPermit::release`]) returns
/// the slot exactly once and re-syncs keep-awake.
pub struct AgentPermit {
    engine: Arc<Engine>,
    released: bool,
}

impl AgentPermit {
    pub fn release(mut self) {
        self.release_once();
    }

    fn release_once(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        let e = &self.engine;
        e.gov.release_agent();
        if e.gov.agents_running() == 0
            && let Ok(rt) = tokio::runtime::Handle::try_current()
        {
            for p in e.providers.lifecycle() {
                rt.spawn(async move { p.on_idle().await });
            }
        }
        e.sync_keep_awake();
    }
}

impl Drop for AgentPermit {
    fn drop(&mut self) {
        self.release_once();
    }
}

impl Engine {
    /// Takes a governor agent slot (waiting while at the cap) and syncs the
    /// keep-awake assertion. `None` when `cancel` fired first.
    pub async fn acquire_agent(&self, cancel: &CancellationToken) -> Option<AgentPermit> {
        if !self.gov.acquire_agent(cancel).await {
            return None;
        }
        self.sync_keep_awake();
        Some(AgentPermit {
            engine: self.arc(),
            released: false,
        })
    }

    /// Re-evaluates keep-awake from the live setting and slot count (after a
    /// settings save, so toggling applies to in-flight agents).
    pub fn sync_keep_awake(&self) {
        // Read and Set together so a stale false from a release cannot
        // clobber a newer acquire's true (or the reverse).
        let _g = self.keep_awake_mu.lock();
        let want =
            self.store.settings().prevent_sleep_while_agents_run && self.gov.agents_running() > 0;
        self.keep_awake.set(want);
    }
}
