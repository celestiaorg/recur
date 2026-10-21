//! Shared E4 test scaffolding: a temp project with a scripted provider, the
//! Go `captureRunner` lookups, and small store/registry pokes Go tests did
//! through package internals.

use std::sync::Arc;
use std::time::Duration;

use recur_agent::error::ErrorKind;
use recur_agent::{Provider, ProviderError, TurnRequest};
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

impl Fx {
    pub fn new(provider: Option<Arc<dyn Provider>>) -> Self {
        let te = TestEngine::new(provider);
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
