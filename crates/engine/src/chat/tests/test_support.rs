//! Shared E4 test scaffolding: a temp project with a scripted provider, the
//! Go `captureRunner` lookups, and small store/registry pokes Go tests did
//! through package internals.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use recur_agent::error::ErrorKind;
use recur_agent::{AgentEvent, Provider, ProviderError, Steer, TurnRequest, TurnResult};
use recur_domain::{AgentRole, ChatItem, Workspace, WorkspaceChatTab};
use tokio_util::sync::CancellationToken;

use crate::chatlife::ChatTurn;
use crate::loop_handle::{LoopHandle, LoopState};
use crate::loop_registry::RunningLoop;
use crate::loops::test_support::{AsyncProvider, CaptureScript};
use crate::testing::{TempRepo, TestEngine, add_project, init_repo};
use crate::turn::test_support::maestro_fixture;
use crate::turn::transcript::content_items;

/// Go `failCallRunner`: call `at` fails with "provider unavailable", the
/// rest replay `inner`.
pub fn fail_at(inner: &Arc<CaptureScript>, at: usize) -> Arc<dyn Provider> {
    let (inner, ok) = (Arc::clone(inner), inner.provider());
    AsyncProvider::wrap(move |req, cx| {
        let (inner, ok) = (Arc::clone(&inner), Arc::clone(&ok));
        async move {
            let call = inner.specs.lock().len();
            if call == at {
                inner.specs.lock().push(req);
                return Err(ProviderError::new(
                    ErrorKind::Failed,
                    "provider unavailable",
                ));
            }
            ok.run_turn(req, cx).await
        }
    })
}

/// One engine, one repo, project "proj".
pub struct Fx {
    pub te: TestEngine,
    pub repo: TempRepo,
    pub proj: String,
}

/// The shipped Claude, Cursor and Codex providers (for their resume, display
/// and history rules), Cursor's project dirs under `home`.
pub fn shipped_providers(home: &std::path::Path) -> Vec<Arc<dyn Provider>> {
    vec![
        Arc::new(recur_provider_claude::ClaudeProvider::new(
            recur_provider_claude::ClaudeConfig::default(),
        )),
        Arc::new(recur_provider_cursor::CursorProvider::new(
            recur_provider_cursor::CursorConfig {
                data_dir: home.to_path_buf(),
                home: Some(home.to_path_buf()),
                ..recur_provider_cursor::CursorConfig::default()
            },
        )),
        Arc::new(recur_provider_codex::CodexProvider::new(
            recur_provider_codex::CodexConfig {
                data_dir: home.to_path_buf(),
                ..recur_provider_codex::CodexConfig::default()
            },
        )),
    ]
}

impl Fx {
    /// Turns go to `provider` (Go `SetRunner`; `None` fails them closed);
    /// the shipped providers stay registered for their per-provider rules,
    /// which Go applied statically.
    pub fn new(provider: Option<Arc<dyn Provider>>) -> Self {
        Self::with(provider, crate::testing::TestOptions::default())
    }

    /// [`Self::new`] over `opts` (trackers, a dead MCP address).
    pub fn with(provider: Option<Arc<dyn Provider>>, opts: crate::testing::TestOptions) -> Self {
        let te = TestEngine::with(crate::testing::TestOptions {
            registered: Some(Vec::new()),
            ..opts
        });
        te.engine
            .providers()
            .replace(shipped_providers(&te.data_dir.join("home")));
        te.engine.providers().pin(provider);
        let repo = init_repo();
        let proj = add_project(&te.store, &repo.path);
        Self { te, repo, proj }
    }

    pub fn eng(&self) -> &Arc<crate::Engine> {
        &self.te.engine
    }

    pub async fn workspace(&self, name: &str) -> Workspace {
        self.te
            .engine
            .create_workspace(&self.proj, name, "")
            .await
            .expect("create workspace")
    }

    pub fn ws(&self, id: &str) -> Workspace {
        self.te.store.workspace(id).expect("workspace")
    }

    /// The transcript's content rows (Go `contentItems(WorkspaceChatItems)`).
    pub async fn items(&self, ws: &str, code: &str) -> Vec<ChatItem> {
        content_items(&self.te.engine.workspace_chat_items(ws, code).await)
    }

    /// `SendWorkspaceMessage(ws, code, text, model, nil, nil, mode, display)`.
    pub async fn send(&self, ws: &str, code: &str, text: &str, model: &str) -> String {
        self.te
            .engine
            .send_workspace_message(ws, code, text, model, None, &[], "", "")
            .await
    }

    pub async fn send_mode(
        &self,
        ws: &str,
        code: &str,
        text: &str,
        model: &str,
        mode: &str,
    ) -> String {
        self.te
            .engine
            .send_workspace_message(ws, code, text, model, None, &[], mode, "")
            .await
    }
}

/// A capture runner replaying `maestro_fixture(<prefix>-<call>, reply)`.
pub fn capture(prefix: &'static str, reply: &'static str) -> Arc<CaptureScript> {
    CaptureScript::new(move |call| maestro_fixture(&format!("{prefix}-{call}"), reply))
}

/// A capture runner replaying the same session every call.
pub fn capture_same(session: &'static str, reply: &'static str) -> Arc<CaptureScript> {
    CaptureScript::new(move |_| maestro_fixture(session, reply))
}

/// Go `specsWithAgentID`: every turn of one agent id (role set).
pub fn with_agent(c: &CaptureScript, agent_id: &str) -> Vec<TurnRequest> {
    c.specs
        .lock()
        .iter()
        .filter(|s| s.agent_id == agent_id && !s.role.is_empty())
        .cloned()
        .collect()
}

pub fn coder_turns(c: &CaptureScript) -> Vec<TurnRequest> {
    c.with_role(AgentRole::Coder)
}

pub fn has_tool(req: &TurnRequest, tool: &str) -> bool {
    req.allowed_tools
        .as_ref()
        .is_some_and(|t| t.iter().any(|x| x == tool))
}

pub fn tab_codes(tabs: &[WorkspaceChatTab]) -> Vec<String> {
    tabs.iter().map(|t| t.code.clone()).collect()
}

/// A registered walk on `key` driving tab `chat` (Go
/// `eng.loops.Store(key, &runningLoop{loop: &Loop{ChatCode: chat}})`).
pub fn walk(te: &TestEngine, ws: &str, chat: &str, key: &str) -> Arc<LoopHandle> {
    let h = LoopHandle::new(
        ws,
        "proj",
        key,
        LoopState {
            chat_code: chat.into(),
            ..LoopState::default()
        },
    );
    assert!(te.engine.register_loop(RunningLoop {
        handle: Arc::clone(&h),
        cancel: CancellationToken::new(),
    }));
    h
}

/// A registered turn no send owns (Go `eng.chats.Store(key, &chatTurn{})`).
pub fn bare_turn(te: &TestEngine, ws: &str, code: &str) -> Arc<ChatTurn> {
    let t = te
        .engine
        .register_chat_turn(ws, code, CancellationToken::new())
        .expect("register turn");
    t.drop_owner();
    t
}

/// Polls every 5 ms for up to `d`.
pub async fn eventually(d: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + d;
    while tokio::time::Instant::now() < deadline {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    cond()
}

fn turn_end(session: &str) -> TurnResult {
    TurnResult {
        subtype: "success".into(),
        session_id: session.into(),
        text: "done".into(),
        ..TurnResult::default()
    }
}

/// Go `holdInjectRunner`: holds the turn until `release`, recording every
/// steer (prompt, display) and acking it (after `ack_wait` when set).
#[derive(Default)]
pub struct HoldInject {
    pub started: CancellationToken,
    pub release: CancellationToken,
    pub got_inject: CancellationToken,
    pub ack_wait: Option<CancellationToken>,
    pub specs: Mutex<Vec<TurnRequest>>,
    pub injected: Mutex<Vec<(String, String)>>,
}

impl HoldInject {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn with_ack_wait() -> Arc<Self> {
        Arc::new(Self {
            ack_wait: Some(CancellationToken::new()),
            ..Self::default()
        })
    }

    pub fn runs(&self) -> usize {
        self.specs.lock().len()
    }

    pub fn last_spec(&self) -> Option<TurnRequest> {
        self.specs.lock().last().cloned()
    }

    /// The injected user text: the display, else the prompt (Go
    /// `decodeInjectPlain`).
    pub fn injected_users(&self) -> Vec<String> {
        self.injected
            .lock()
            .iter()
            .map(|(p, d)| if d.is_empty() { p.clone() } else { d.clone() })
            .collect()
    }

    async fn note(&self, s: Steer, cancel: &CancellationToken) {
        let prompt = s.prompt.trim().to_owned();
        if prompt.is_empty() {
            return;
        }
        self.injected
            .lock()
            .push((prompt, s.display.clone().unwrap_or_default()));
        self.got_inject.cancel();
        if let Some(wait) = &self.ack_wait {
            tokio::select! {
                _ = wait.cancelled() => {}
                _ = self.release.cancelled() => {}
                _ = cancel.cancelled() => {
                    s.ack(false);
                    return;
                }
            }
        }
        s.ack(true);
    }

    pub fn provider(self: &Arc<Self>) -> Arc<dyn Provider> {
        let me = Arc::clone(self);
        AsyncProvider::wrap(move |req, mut cx| {
            let me = Arc::clone(&me);
            async move {
                me.specs.lock().push(req);
                me.started.cancel();
                let cancel = cx.cancel.clone();
                let mut inbox = cx.steer.take();
                let first = async {
                    match inbox.as_mut() {
                        Some(i) => i.recv().await,
                        None => std::future::pending().await,
                    }
                };
                tokio::select! {
                    Some(s) = first => {
                        me.note(s, &cancel).await;
                        if let Some(i) = inbox.as_mut() {
                            while let Some(s) = i.try_next() {
                                me.note(s, &cancel).await;
                            }
                        }
                        tokio::select! {
                            _ = me.release.cancelled() => {}
                            _ = cancel.cancelled() => return Err(ProviderError::canceled()),
                        }
                    }
                    _ = me.release.cancelled() => {
                        if let Some(i) = inbox.as_mut() {
                            while let Some(s) = i.try_next() {
                                me.note(s, &cancel).await;
                            }
                        }
                    }
                    _ = cancel.cancelled() => return Err(ProviderError::canceled()),
                }
                let res = turn_end("cursor-steer");
                cx.sink.emit(AgentEvent::TurnEnd(res.clone()));
                Ok(res)
            }
        })
    }
}

/// Go `holdAfterResultRunner`: emits the result first, then the first run
/// waits for `release` and later runs for `second_release`.
#[derive(Default)]
pub struct HoldAfterResult {
    pub started: CancellationToken,
    pub release: CancellationToken,
    pub second_started: CancellationToken,
    pub second_release: CancellationToken,
    pub specs: Mutex<Vec<TurnRequest>>,
}

impl HoldAfterResult {
    pub fn provider(self: &Arc<Self>) -> Arc<dyn Provider> {
        let me = Arc::clone(self);
        AsyncProvider::wrap(move |req, cx| {
            let me = Arc::clone(&me);
            async move {
                let n = {
                    let mut s = me.specs.lock();
                    s.push(req);
                    s.len()
                };
                cx.sink.emit(AgentEvent::TurnEnd(turn_end("agent-done")));
                let (started, gate) = if n == 1 {
                    (&me.started, &me.release)
                } else {
                    (&me.second_started, &me.second_release)
                };
                started.cancel();
                tokio::select! {
                    _ = gate.cancelled() => Ok(turn_end("agent-done")),
                    _ = cx.cancel.cancelled() => Err(ProviderError::canceled()),
                }
            }
        })
    }

    pub fn spec_resume(&self, i: usize) -> String {
        self.specs
            .lock()
            .get(i)
            .map(|s| s.resume_session.clone())
            .unwrap_or_default()
    }
}

/// Go `holdUntilRunner`: holds until `release` without reading steers;
/// counts runs and cancels.
#[derive(Default)]
pub struct HoldUntil {
    pub started: CancellationToken,
    pub release: CancellationToken,
    pub runs: Mutex<usize>,
    pub cancels: Mutex<usize>,
}

impl HoldUntil {
    pub fn provider(self: &Arc<Self>) -> Arc<dyn Provider> {
        let me = Arc::clone(self);
        AsyncProvider::wrap(move |_req, cx| {
            let me = Arc::clone(&me);
            async move {
                *me.runs.lock() += 1;
                me.started.cancel();
                tokio::select! {
                    _ = me.release.cancelled() => {}
                    _ = cx.cancel.cancelled() => {
                        *me.cancels.lock() += 1;
                        return Err(ProviderError::canceled());
                    }
                }
                let res = turn_end("hold");
                cx.sink.emit(AgentEvent::TurnEnd(res.clone()));
                Ok(res)
            }
        })
    }
}

/// How long a step may take before a test calls it hung. Go's tests used 3s, but a stopped
/// or finished turn writes its git checkpoint before the send returns, and under a loaded
/// full-suite run that alone can pass 3s (0.2s idle, 1.4s at load 66).
const HANG: Duration = Duration::from_secs(10);

/// Waits for a token, failing as hung after [`HANG`].
pub async fn within_hang(t: &CancellationToken, what: &str) {
    tokio::time::timeout(HANG, t.cancelled())
        .await
        .unwrap_or_else(|_| panic!("timed out: {what}"));
}

/// Awaits a spawned send, failing as hung after [`HANG`].
pub async fn join_hang<T>(h: tokio::task::JoinHandle<T>, what: &str) -> T {
    tokio::time::timeout(HANG, h)
        .await
        .unwrap_or_else(|_| panic!("timed out: {what}"))
        .expect("task")
}

/// Go `durationCancelRunner`: text, then wait for cancel and return the
/// wall-clock duration without a result frame.
pub fn duration_cancel_runner() -> Arc<dyn Provider> {
    AsyncProvider::wrap(|_req, cx| async move {
        cx.sink.emit(AgentEvent::Text {
            text: "working on it".into(),
            uuid: String::new(),
            parent: String::new(),
        });
        cx.cancel.cancelled().await;
        Err(ProviderError::canceled().with_partial(TurnResult {
            duration_ms: 1500,
            session_id: "thread-1".into(),
            ..TurnResult::default()
        }))
    })
}

/// Go `countRunner`: counts runs, each a quick success.
pub fn count_runner(n: Arc<std::sync::atomic::AtomicUsize>) -> Arc<dyn Provider> {
    AsyncProvider::wrap(move |_req, _cx| {
        let n = Arc::clone(&n);
        async move {
            n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(TurnResult {
                subtype: "success".into(),
                text: "ran".into(),
                ..TurnResult::default()
            })
        }
    })
}

/// Go `slowUnwindRunner`: the first run waits for cancel and then `unwind`;
/// later runs succeed.
pub fn slow_unwind_runner(
    started: CancellationToken,
    unwind: CancellationToken,
) -> Arc<dyn Provider> {
    let n = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    AsyncProvider::wrap(move |_req, cx| {
        let (n, started, unwind) = (Arc::clone(&n), started.clone(), unwind.clone());
        async move {
            if n.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                started.cancel();
                cx.cancel.cancelled().await;
                unwind.cancelled().await;
                return Err(ProviderError::canceled());
            }
            Ok(TurnResult {
                subtype: "success".into(),
                session_id: "next".into(),
                text: "ok".into(),
                ..TurnResult::default()
            })
        }
    })
}

/// A test hook that signals `entered` and then waits for `proceed` (Go's
/// `close(blocked); <-proceed`), only on the calls `pick` selects.
pub fn gate_hook(
    pick: impl Fn(usize) -> Option<(CancellationToken, Option<CancellationToken>)>
    + Send
    + Sync
    + 'static,
) -> impl Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync + 'static
{
    let n = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    move || {
        let call = n.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let gate = pick(call);
        Box::pin(async move {
            if let Some((entered, proceed)) = gate {
                entered.cancel();
                if let Some(p) = proceed {
                    p.cancelled().await;
                }
            }
        })
    }
}

/// Go `holdFirstRunner` / `delayAfterCancelRunner`: the first run waits for
/// cancel (then `delay`), later runs replay a success.
pub fn hold_first_runner(
    c: &Arc<CaptureScript>,
    started: CancellationToken,
    delay: Duration,
) -> Arc<dyn Provider> {
    let (c, ok) = (Arc::clone(c), c.provider());
    AsyncProvider::wrap(move |req, cx| {
        let (c, ok, started) = (Arc::clone(&c), Arc::clone(&ok), started.clone());
        async move {
            if c.specs.lock().is_empty() {
                c.specs.lock().push(req);
                started.cancel();
                cx.cancel.cancelled().await;
                tokio::time::sleep(delay).await;
                return Err(ProviderError::canceled());
            }
            ok.run_turn(req, cx).await
        }
    })
}

/// Whether the inbox holds a row with this id (Go `inboxByID`).
pub fn inbox_has(te: &TestEngine, id: &str) -> bool {
    te.store
        .inbox()
        .unwrap_or_default()
        .iter()
        .any(|it| it.id == id)
}

/// The last `attention.updated` tabUnseen for one tab after event `from`.
pub fn last_tab_unseen(te: &TestEngine, from: usize, ws: &str, code: &str) -> Option<bool> {
    te.events
        .by_name("attention.updated")
        .into_iter()
        .skip(from)
        .rev()
        .find_map(|ev| match ev {
            crate::bus::EngineEvent::AttentionUpdated(a)
                if a.code == code && a.workspace_id == ws =>
            {
                Some(a.tab_unseen)
            }
            _ => None,
        })
}

/// The first `n` coder turns, polled for up to 3 s.
pub async fn wait_coder_turns(
    c: &crate::loops::test_support::CaptureScript,
    n: usize,
) -> Vec<recur_agent::TurnRequest> {
    for _ in 0..200 {
        let specs = c.with_role(AgentRole::Coder);
        if specs.len() >= n {
            return specs;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    panic!("timed out waiting for {n} coder turns");
}

/// Runs `pre` with each request before `inner` takes the turn (Go's
/// wrapping runners: `editingRunner`, `messageFromReportRunner`).
pub fn before_turn<F, Fut>(inner: Arc<dyn Provider>, pre: F) -> Arc<dyn Provider>
where
    F: Fn(TurnRequest) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let pre = Arc::new(pre);
    AsyncProvider::wrap(move |req, cx| {
        let (inner, pre) = (Arc::clone(&inner), Arc::clone(&pre));
        async move {
            pre(req.clone()).await;
            inner.run_turn(req, cx).await
        }
    })
}
