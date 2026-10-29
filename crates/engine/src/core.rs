//! The engine (E1): every piece of shared state, its construction, and the
//! small helpers every slice leans on.
//!
//! Lock map (plan §4.2), field by field:
//!
//! | Field | Kind |
//! |---|---|
//! | `dispatch_mu` | async, outermost |
//! | `chat_life` + per-tab async mutex | map sync, tab async |
//! | `chat_tabs` | FIFO ticket gate |
//! | `teardown` (guards `shutting_down`) | sync |
//! | `hoist_mu` | sync |
//! | `watch` → `chat_focus` | sync, fixed order |
//! | `chat_unseen_mu` | sync |
//! | `schedule` | async |
//! | `open_main_mu` | async |
//! | `id_mu` | async (held across `git branch --list`) |
//! | `diff_sync` + per-workspace async apply lock | |
//! | `stream_seq` | sync leaf |
//! | `continue_gens` | sync leaf |
//! | `keep_awake_mu` | sync leaf |
//!
//! Sync (`parking_lot`) locks are never held across an `.await`; the crate
//! denies `clippy::await_holding_lock`. Events are published after the
//! store write that caused them, outside engine locks, except where an
//! event is published under a lock to keep order (documented at the call site).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::Duration;

use chrono::{DateTime, Local};
use parking_lot::Mutex;
use recur_bundles::DiscoverRoots;
use recur_domain::{Settings, Workspace, effective_max_agents, effective_max_loops};
use recur_mcp::McpServer;
use recur_platform::power::Keeper;
use recur_store::Store;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::ask_user::AskUserBroker;
use crate::attention::ChatFocus;
use crate::bus::{Bus, EngineEvent, Notify};
use crate::chatlife::{ChatTabSlot, ChatTurn, ContinueGens, LifeSlot};
use crate::fswatch::WatchState;
use crate::governor::Governor;
use crate::host::EngineHost;
use crate::loop_registry::RunningLoop;
use crate::permissions::PermissionBroker;
use crate::registry::{ForgeRegistry, ProviderRegistry, TrackerRegistry};
use crate::terminals::TerminalSource;
use crate::workspace::create::{CreatingAlloc, PrHeadCheckoutSpec};
use crate::workspace::diff_sync::{DiffSync, RefreshAll};

/// An engine failure. Its text is the user-facing message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct EngineError(pub String);

impl EngineError {
    pub fn new(msg: impl Into<String>) -> Self {
        EngineError(msg.into())
    }
}

impl From<String> for EngineError {
    fn from(s: String) -> Self {
        EngineError(s)
    }
}

impl From<&str> for EngineError {
    fn from(s: &str) -> Self {
        EngineError(s.to_owned())
    }
}

pub type Result<T, E = EngineError> = std::result::Result<T, E>;

/// Git as the engine sees it: the managed worktree parent and the boot
/// probe's alert (empty when git is fine).
#[derive(Debug, Clone, Default)]
pub struct GitSetup {
    pub worktree_root: String,
    pub warning_title: String,
    pub warning: String,
}

impl GitSetup {
    /// Probes the installed git once.
    pub async fn probe(worktree_root: impl Into<String>) -> Self {
        let probed = recur_git::probe().await;
        let (warning_title, warning) = recur_git::unsupported_alert(&probed);
        Self {
            worktree_root: worktree_root.into(),
            warning_title,
            warning,
        }
    }
}

/// The task groups Shutdown joins (loops, teardowns, schedules and chat
/// turns).
#[derive(Default)]
pub struct Tasks {
    pub loops: TaskTracker,
    pub teardown: TaskTracker,
    pub schedules: TaskTracker,
    pub chat_turns: TaskTracker,
}

/// The schedule ticker's stop signal and task. E4's `start_scheduled_prompts`
/// installs them; Shutdown fires `stop` under `schedule` and joins `ticker`.
#[derive(Default)]
pub struct ScheduleControl {
    pub stop: Option<CancellationToken>,
    pub ticker: Option<JoinHandle<()>>,
}

type Hook = Arc<dyn Fn() + Send + Sync>;
type Clock = Arc<dyn Fn() -> DateTime<Local> + Send + Sync>;
/// A test seam that may wait.
pub(crate) type AsyncHook =
    Arc<dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync>;

/// Test seams (the promote, start-walk and child-queue hooks, the clock,
/// kitchen word lists, fswatch timing). All unset in production.
pub(crate) struct TestHooks {
    pub clock: Option<Clock>,
    pub promote: Option<Hook>,
    pub start_walk: Option<Hook>,
    pub child_queue: Option<Hook>,
    pub kitchen_words: Option<(Vec<String>, Vec<String>)>,
    pub fs_event_latency: Duration,
    pub fs_poll_backstop: Duration,
    /// After a send takes its FIFO ticket.
    pub before_chat_claim: Option<AsyncHook>,
    /// While the tab slot is released for the fork inject.
    pub before_apply_chat_fork: Option<AsyncHook>,
}

impl Default for TestHooks {
    fn default() -> Self {
        Self {
            clock: None,
            promote: None,
            start_walk: None,
            child_queue: None,
            kitchen_words: None,
            fs_event_latency: Duration::from_millis(400),
            fs_poll_backstop: Duration::from_secs(7),
            before_chat_claim: None,
            before_apply_chat_fork: None,
        }
    }
}

/// Everything the engine owns. Always used through `Arc<Engine>`.
pub struct Engine {
    weak: Weak<Engine>,

    pub store: Arc<Store>,
    pub bus: Arc<Bus>,

    pub(crate) gov: Governor,
    pub(crate) providers: ProviderRegistry,
    pub(crate) forges: ForgeRegistry,
    pub(crate) trackers: TrackerRegistry,
    pub(crate) terminals: Option<Arc<dyn TerminalSource>>,
    pub(crate) bundle_roots: DiscoverRoots,
    pub(crate) data_dir: PathBuf,
    pub(crate) worktree_root: String,
    pub(crate) git_warning: String,
    pub(crate) git_warning_title: String,

    /// The Recur MCP server; started lazily by `ensure_mcp`. The flag is
    /// `mcpMu` + `mcpStarted`: a failed start is retried, not cached.
    pub(crate) mcp: McpServer,
    pub(crate) mcp_started: tokio::sync::Mutex<bool>,

    /// Live loop walks keyed by workspace id (or workspace + `\x1e` + chat
    /// for a second walk on one workspace).
    pub(crate) loops: Mutex<HashMap<String, RunningLoop>>,
    /// In-flight chat turns keyed by `chat_turn_key`.
    pub(crate) chats: Mutex<HashMap<String, Arc<ChatTurn>>>,
    /// Per-tab FIFO send order and stop generation.
    pub(crate) chat_tabs: Mutex<HashMap<String, Arc<ChatTabSlot>>>,
    /// `chatLifeMu` and the ref-counted per-tab life locks it guards.
    pub(crate) chat_life: Mutex<HashMap<String, LifeSlot>>,
    /// Live switch_mode remount generations.
    pub(crate) continue_gens: Mutex<ContinueGens>,

    /// The D26 promote critical section.
    pub(crate) dispatch_mu: tokio::sync::Mutex<()>,
    /// `teardownMu`, guarding `shuttingDown`.
    pub(crate) teardown: Mutex<bool>,
    /// Makes PR-host hoisting and `reserve_deleting` mutually exclusive.
    pub(crate) hoist_mu: Mutex<()>,
    pub(crate) hoisting: Mutex<HashSet<String>>,
    /// Workspaces whose store row is gone or archived but whose disk
    /// teardown still runs; kitchen ids stay taken meanwhile.
    pub(crate) deleting: Mutex<HashSet<String>>,
    /// In-flight worktree allocation and setup, cancelled by Remove/Delete/
    /// Shutdown.
    pub(crate) creating: Mutex<HashMap<String, Arc<CreatingAlloc>>>,
    /// Fork destination → source whose dirty tree still needs copying.
    pub(crate) fork_overlay: Mutex<HashMap<String, Workspace>>,
    /// The PR head a creating workspace checks out instead of the base.
    pub(crate) pr_head_checkout: Mutex<HashMap<String, PrHeadCheckoutSpec>>,
    /// `@{upstream}` branch name per workspace ("" = none).
    pub(crate) ws_upstream_cache: Mutex<HashMap<String, String>>,

    pub(crate) watch: Mutex<WatchState>,
    pub(crate) chat_focus: Mutex<ChatFocus>,
    pub(crate) chat_unseen_mu: Mutex<()>,

    pub(crate) diff_sync: Mutex<DiffSync>,
    pub(crate) diff_refresh_all: Mutex<RefreshAll>,

    pub(crate) schedule: tokio::sync::Mutex<ScheduleControl>,
    pub(crate) open_main_mu: tokio::sync::Mutex<()>,
    pub(crate) id_mu: tokio::sync::Mutex<()>,
    /// The last live-stream seq per workspace + agent + kind.
    pub(crate) stream_seq: Mutex<HashMap<String, i64>>,

    /// Human permission prompts.
    pub(crate) perms: PermissionBroker,
    /// Parked ask_user questions.
    pub(crate) ask_users: AskUserBroker,
    /// Serializes `ensure_agent_session`'s lookup-then-create.
    pub(crate) session_ensure_mu: Mutex<()>,
    /// Per-session TodoWrite lists for in-process (harness) turns.
    pub(crate) todos: Arc<recur_agent_tools::TodoStore>,

    pub(crate) keep_awake: Keeper,
    pub(crate) keep_awake_mu: Mutex<()>,
    /// The persisted "iters today" baseline captured at construction.
    pub(crate) iters_base: i64,

    /// The rename nudge computed at loop engage, for the planner turn.
    pub(crate) loop_rename_nudge: Mutex<HashMap<String, String>>,
    /// Titles stamped from a queued message before its send ran.
    pub(crate) early_titles: Mutex<crate::chat_shared::EarlyTitles>,
    /// Escalation raised/answered counters.
    pub(crate) esc_precision: crate::loops::escalation::EscPrecision,
    /// How often a fan-out parent re-checks its children.
    pub(crate) fan_out_poll: Mutex<Duration>,

    /// Pending switch_mode remounts and remount depth per tab.
    pub(crate) mode_continues: Mutex<crate::chat::switch_mode::ModeContinues>,
    /// A live parent chat turn (by pointer) → the subagent wave it creates.
    pub(crate) child_waves: Mutex<HashMap<usize, i64>>,
    /// Child ids with a live report watcher.
    pub(crate) chat_child_watchers: Mutex<HashSet<String>>,
    /// Serializes merge_child per parent workspace (held across git).
    pub(crate) merge_child_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Chat turn keys AutoResume relaunched whose resumed turn is not done.
    pub(crate) resuming_chats: Mutex<HashSet<String>>,
    /// The looking-at surface of each tab's last send, for Retry.
    pub(crate) last_chat_surfaces: Mutex<HashMap<String, crate::chat::surface::ChatSurface>>,
    /// Pull requests: holds, throttles, the sync single-flight and hoist
    /// aliases (E5).
    pub(crate) prs: crate::prs::PrState,
    /// PA*/MA tabs Recover found live with no turn, for AutoResume.
    pub(crate) interrupted_chats: Mutex<Vec<crate::chat::recover::InterruptedChat>>,
    /// Scheduled workspaces whose first turn settled into "waiting".
    pub(crate) schedule_wait: Mutex<HashSet<String>>,

    pub tasks: Tasks,
    pub(crate) hooks: Mutex<TestHooks>,
}

impl Engine {
    /// Builds the engine over a store. Governor caps come from the stored
    /// Settings; the MCP server is constructed (backed by [`EngineHost`])
    /// but not started until a turn needs it.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: Arc<Store>,
        bus: Arc<Bus>,
        git: GitSetup,
        bundles: DiscoverRoots,
        term: Option<Arc<dyn TerminalSource>>,
        registry: ProviderRegistry,
        forges: ForgeRegistry,
        trackers: TrackerRegistry,
        data_dir: PathBuf,
    ) -> Arc<Engine> {
        Self::build(
            store, bus, git, bundles, term, registry, forges, trackers, data_dir, None,
        )
    }

    /// [`Engine::new`] with the MCP listen address overridden (tests bind
    /// an occupied port to fail the first start).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build(
        store: Arc<Store>,
        bus: Arc<Bus>,
        git: GitSetup,
        bundles: DiscoverRoots,
        term: Option<Arc<dyn TerminalSource>>,
        registry: ProviderRegistry,
        forges: ForgeRegistry,
        trackers: TrackerRegistry,
        data_dir: PathBuf,
        mcp_addr: Option<String>,
    ) -> Arc<Engine> {
        let set = store.settings();
        let iters_base = store.resource().iters_today;
        Arc::new_cyclic(|weak: &Weak<Engine>| {
            let host: Arc<dyn recur_agent_tools::Host> = Arc::new(EngineHost::new(weak.clone()));
            Engine {
                weak: weak.clone(),
                store,
                bus,
                gov: Governor::new(
                    effective_max_loops(set.max_loops),
                    effective_max_agents(set.max_agents),
                ),
                providers: registry,
                forges,
                trackers,
                terminals: term,
                bundle_roots: bundles,
                data_dir,
                worktree_root: git.worktree_root,
                git_warning: git.warning,
                git_warning_title: git.warning_title,
                mcp: match mcp_addr {
                    Some(addr) => McpServer::new(Some(host)).with_addr(addr),
                    None => McpServer::new(Some(host)),
                },
                mcp_started: tokio::sync::Mutex::new(false),
                loops: Mutex::new(HashMap::new()),
                chats: Mutex::new(HashMap::new()),
                chat_tabs: Mutex::new(HashMap::new()),
                chat_life: Mutex::new(HashMap::new()),
                continue_gens: Mutex::new(ContinueGens::default()),
                dispatch_mu: tokio::sync::Mutex::new(()),
                teardown: Mutex::new(false),
                hoist_mu: Mutex::new(()),
                hoisting: Mutex::new(HashSet::new()),
                deleting: Mutex::new(HashSet::new()),
                creating: Mutex::new(HashMap::new()),
                fork_overlay: Mutex::new(HashMap::new()),
                pr_head_checkout: Mutex::new(HashMap::new()),
                ws_upstream_cache: Mutex::new(HashMap::new()),
                watch: Mutex::new(WatchState::default()),
                // Assume key until the window loses focus; false would
                // suppress every "seen".
                chat_focus: Mutex::new(ChatFocus::new()),
                chat_unseen_mu: Mutex::new(()),
                diff_sync: Mutex::new(DiffSync::default()),
                diff_refresh_all: Mutex::new(RefreshAll::default()),
                schedule: tokio::sync::Mutex::new(ScheduleControl::default()),
                open_main_mu: tokio::sync::Mutex::new(()),
                id_mu: tokio::sync::Mutex::new(()),
                stream_seq: Mutex::new(HashMap::new()),
                perms: broker_for(weak),
                ask_users: ask_broker_for(weak),
                session_ensure_mu: Mutex::new(()),
                todos: Arc::new(recur_agent_tools::TodoStore::default()),
                keep_awake: Keeper::new(),
                keep_awake_mu: Mutex::new(()),
                iters_base,
                loop_rename_nudge: Mutex::new(HashMap::new()),
                early_titles: Mutex::new(crate::chat_shared::EarlyTitles::default()),
                esc_precision: crate::loops::escalation::EscPrecision::default(),
                fan_out_poll: Mutex::new(crate::loops::fanout::DEFAULT_FAN_OUT_POLL),
                mode_continues: Mutex::new(Default::default()),
                child_waves: Mutex::new(HashMap::new()),
                chat_child_watchers: Mutex::new(HashSet::new()),
                merge_child_locks: Mutex::new(HashMap::new()),
                resuming_chats: Mutex::new(HashSet::new()),
                last_chat_surfaces: Mutex::new(HashMap::new()),
                prs: crate::prs::PrState::default(),
                interrupted_chats: Mutex::new(Vec::new()),
                schedule_wait: Mutex::new(HashSet::new()),
                tasks: Tasks::default(),
                hooks: Mutex::new(TestHooks::default()),
            }
        })
    }

    /// A strong handle to this engine (for spawned tasks).
    pub(crate) fn arc(&self) -> Arc<Engine> {
        self.weak
            .upgrade()
            .expect("engine methods run while an Arc<Engine> is alive")
    }

    /// The live settings snapshot.
    pub fn settings(&self) -> Settings {
        self.store.settings()
    }

    /// The resource governor (status-bar counters, caps).
    pub fn governor(&self) -> &Governor {
        &self.gov
    }

    pub fn providers(&self) -> &ProviderRegistry {
        &self.providers
    }

    pub fn forges(&self) -> &ForgeRegistry {
        &self.forges
    }

    pub fn trackers(&self) -> &TrackerRegistry {
        &self.trackers
    }

    /// The Observatory terminal source (`None` when no dock is attached).
    pub fn terminals(&self) -> Option<&Arc<dyn TerminalSource>> {
        self.terminals.as_ref()
    }

    /// Recur's managed data dir (`workspaces/<id>/…` lives under it).
    pub fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }

    /// The managed worktrees parent.
    pub fn worktree_root(&self) -> &str {
        &self.worktree_root
    }

    pub fn bundle_roots(&self) -> &DiscoverRoots {
        &self.bundle_roots
    }

    /// The boot alert when git is missing or too old ("" when fine).
    pub fn git_warning(&self) -> &str {
        &self.git_warning
    }

    /// Publishes a notify (OS notification + resource.updated) when git is
    /// missing or too old.
    pub fn check_git(&self) {
        if self.git_warning.is_empty() {
            return;
        }
        tracing::warn!("git: {}", self.git_warning);
        let title = if self.git_warning_title.is_empty() {
            "Git is too old".to_owned()
        } else {
            self.git_warning_title.clone()
        };
        self.bus.publish(EngineEvent::Notify(Notify {
            title,
            body: self.git_warning.clone(),
            target: None,
        }));
        self.publish_resource();
    }

    /// The MCP server (runners register per-turn URLs on it).
    pub fn mcp(&self) -> &McpServer {
        &self.mcp
    }

    /// Starts the MCP server on first use. A failure is returned and retried
    /// on the next call, never remembered.
    pub async fn ensure_mcp(&self) -> Result<(), recur_mcp::McpError> {
        let mut started = self.mcp_started.lock().await;
        if *started {
            return Ok(());
        }
        self.mcp.start().await?;
        *started = true;
        Ok(())
    }

    /// Revokes every MCP token of one agent (chat close).
    pub fn drop_mcp_agent(&self, workspace_id: &str, agent_code: &str) {
        self.mcp.drop_agent(workspace_id, agent_code);
    }

    /// The git path segment prepended to new workspace branches ("" = none).
    /// The stored Settings string is the source of truth.
    pub fn branch_name_prefix(&self) -> String {
        recur_git::normalize_branch_prefix(&self.settings().effective_branch_name_prefix())
    }

    pub fn workspace_branch(&self, slug: &str) -> String {
        recur_git::prefixed_branch(&self.branch_name_prefix(), slug)
    }

    /// A worktree manager for one project repo, with the live branch prefix.
    pub fn git_manager(&self, repo_path: &str) -> recur_git::Manager {
        recur_git::Manager::new(repo_path, &self.worktree_root)
            .with_branch_prefix(&self.branch_name_prefix())
    }

    /// A manager with the default prefix (teardown paths).
    pub(crate) fn plain_git_manager(&self, repo_path: &str) -> recur_git::Manager {
        recur_git::Manager::new(repo_path, &self.worktree_root)
    }

    /// Settings.DisabledSkills plus the workspace project's overrides.
    pub fn skill_enabled_for_workspace(&self, workspace_id: &str, name: &str) -> bool {
        let global = self.settings().disabled_skills;
        let Some(ws) = self.store.workspace(workspace_id) else {
            return recur_domain::effective_skill_enabled(name, &global, &[], &[]);
        };
        let Some(p) = self.store.project(&ws.project_id) else {
            return recur_domain::effective_skill_enabled(name, &global, &[], &[]);
        };
        recur_domain::effective_skill_enabled(name, &global, &p.disabled_skills, &p.enabled_skills)
    }

    /// The engine clock, injectable in tests.
    pub fn now(&self) -> DateTime<Local> {
        let clock = self.hooks.lock().clock.clone();
        match clock {
            Some(f) => f(),
            None => Local::now(),
        }
    }

    /// Updates the live caps and drains queued loops into newly freed slots.
    /// In-flight work is not preempted.
    pub async fn apply_governor_caps(&self, max_loops: i64, max_agents: i64) {
        self.gov.set_caps(
            effective_max_loops(max_loops),
            effective_max_agents(max_agents),
        );
        loop {
            let n = self.gov.loops_running();
            self.drain_queued().await;
            if self.gov.loops_running() <= n {
                break;
            }
        }
        self.publish_resource();
    }

    /// Whether Shutdown has started.
    pub fn is_shutting_down(&self) -> bool {
        *self.teardown.lock()
    }
}

/// Test seams (R15). Production never sets them.
#[cfg(any(test, feature = "testing"))]
impl Engine {
    /// Freezes the clock (`nowFn`).
    pub fn set_clock(&self, f: impl Fn() -> DateTime<Local> + Send + Sync + 'static) {
        self.hooks.lock().clock = Some(Arc::new(f));
    }

    /// Runs inside the dispatch critical section.
    pub fn set_promote_hook(&self, f: impl Fn() + Send + Sync + 'static) {
        self.hooks.lock().promote = Some(Arc::new(f));
    }

    /// Runs after a chat loop walk releases its life lock.
    pub fn set_start_walk_hook(&self, f: impl Fn() + Send + Sync + 'static) {
        self.hooks.lock().start_walk = Some(Arc::new(f));
    }

    /// Runs after a fan-out child is inserted.
    pub fn set_child_queue_hook(&self, f: impl Fn() + Send + Sync + 'static) {
        self.hooks.lock().child_queue = Some(Arc::new(f));
    }

    /// Pins the kitchen word lists.
    pub fn set_kitchen_words(&self, adjectives: &[&str], dishes: &[&str]) {
        self.hooks.lock().kitchen_words = Some((
            adjectives.iter().map(|s| (*s).to_owned()).collect(),
            dishes.iter().map(|s| (*s).to_owned()).collect(),
        ));
    }

    /// Shrinks the fan-out parent's child poll.
    pub fn set_fan_out_poll(&self, d: Duration) {
        *self.fan_out_poll.lock() = d;
    }

    /// Shrinks the fswatch coalescing window and fingerprint poll.
    pub fn set_fs_timing(&self, latency: Duration, poll: Duration) {
        let mut h = self.hooks.lock();
        h.fs_event_latency = latency;
        h.fs_poll_backstop = poll;
    }

    /// Runs after a send takes its FIFO ticket.
    pub fn set_before_chat_claim_hook<F, Fut>(&self, f: F)
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        self.hooks.lock().before_chat_claim = Some(Arc::new(move || Box::pin(f())));
    }

    /// Runs while the tab slot is released for the fork inject.
    pub fn set_before_apply_chat_fork_hook<F, Fut>(&self, f: F)
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        self.hooks.lock().before_apply_chat_fork = Some(Arc::new(move || Box::pin(f())));
    }
}

impl Engine {
    fn run_hook(&self, pick: impl Fn(&TestHooks) -> Option<Hook>) {
        let hook = pick(&self.hooks.lock());
        if let Some(h) = hook {
            h();
        }
    }

    /// Inside the dispatch critical section, after the target was confirmed not
    /// running (no-op in production).
    pub fn run_promote_hook(&self) {
        self.run_hook(|h| h.promote.clone());
    }

    /// After a chat loop walk releases its life lock and before Resume (no-op
    /// in production).
    pub fn run_start_walk_hook(&self) {
        self.run_hook(|h| h.start_walk.clone());
    }

    /// After a fan-out child row is inserted queued and before its parent link
    /// is written (no-op in production).
    pub fn run_child_queue_hook(&self) {
        self.run_hook(|h| h.child_queue.clone());
    }

    /// Awaits an async test seam (no-op in production).
    pub(crate) async fn run_async_hook(&self, pick: impl Fn(&TestHooks) -> Option<AsyncHook>) {
        let hook = pick(&self.hooks.lock());
        if let Some(h) = hook {
            h().await;
        }
    }

    /// The last live-stream seqs (text, thinking) for one agent, so a new
    /// turn's broadcast continues the process-wide monotonic sequence.
    pub fn stream_seq_seed(&self, workspace_id: &str, code: &str) -> (i64, i64) {
        let seqs = self.stream_seq.lock();
        let get = |kind: &str| {
            seqs.get(&stream_seq_key(workspace_id, code, kind))
                .copied()
                .unwrap_or(0)
        };
        (get("text"), get("thinking"))
    }

    /// Records a published live-stream seq (only ever increases).
    pub fn remember_stream_seq(&self, workspace_id: &str, code: &str, kind: &str, seq: i64) {
        if seq <= 0 {
            return;
        }
        let mut seqs = self.stream_seq.lock();
        let cur = seqs
            .entry(stream_seq_key(workspace_id, code, kind))
            .or_insert(0);
        if seq > *cur {
            *cur = seq;
        }
    }
}

/// The permission broker, publishing through the engine.
fn broker_for(weak: &Weak<Engine>) -> PermissionBroker {
    let b = PermissionBroker::new();
    let w = weak.clone();
    b.set_notify(move |req| {
        if let Some(e) = w.upgrade() {
            e.publish_permission_requested(req);
        }
    });
    let w = weak.clone();
    b.set_resolved(move |id, d| {
        if let Some(e) = w.upgrade() {
            e.publish_permission_resolved(id, d);
        }
    });
    b
}

fn ask_broker_for(weak: &Weak<Engine>) -> AskUserBroker {
    let b = AskUserBroker::new();
    let w = weak.clone();
    b.set_notify(move |q| {
        if let Some(e) = w.upgrade() {
            e.publish_ask_user_requested(q);
        }
    });
    let w = weak.clone();
    b.set_resolved(move |id, ans| {
        if let Some(e) = w.upgrade() {
            e.publish_ask_user_resolved(id, ans);
        }
    });
    b
}

fn stream_seq_key(workspace_id: &str, code: &str, kind: &str) -> String {
    format!("{workspace_id}\x00{code}\x00{kind}")
}
