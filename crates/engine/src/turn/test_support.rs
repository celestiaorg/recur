//! Turn-pipeline test doubles and fixtures: a provider that records every
//! request and replays per-call Claude stream-json through the real parser,
//! with every other behavior borrowed from the real Claude provider.

use std::sync::Arc;

use parking_lot::Mutex;
use recur_agent::{
    AgentEvent, Availability, Flags, ModelChoice, Provider, ProviderCaps, ProviderError,
    ProviderStatus, TokenUsage, ToolDisplay, TurnContext, TurnRequest, TurnResult,
};
use recur_domain::{AutonomyLevel, Group, Settings, Workspace, WorkspaceState};
use recur_provider_claude::testing::NdjsonScript;
use recur_provider_claude::{ClaudeConfig, ClaudeProvider};

use crate::Engine;

/// The real Claude provider (display, labels, context windows).
pub fn claude() -> Arc<dyn Provider> {
    Arc::new(ClaudeProvider::new(ClaudeConfig::default()))
}

/// Replays `lines` under the Claude rules.
pub fn script<I, S>(lines: I) -> Arc<dyn Provider>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    Arc::new(
        NdjsonScript::new(
            lines
                .into_iter()
                .map(|l| l.as_ref().as_bytes().to_vec())
                .collect::<Vec<_>>(),
        )
        .imitating(claude()),
    )
}

/// init → assistant text → success.
pub fn maestro_fixture(session_id: &str, reply: &str) -> Vec<String> {
    vec![
        format!(
            r#"{{"type":"system","subtype":"init","session_id":"{session_id}","model":"claude-opus-4-6","mcp_servers":[]}}"#
        ),
        format!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"{reply}"}}]}}}}"#
        ),
        format!(
            r#"{{"type":"result","subtype":"success","session_id":"{session_id}","duration_ms":900,"is_error":false,"num_turns":1,"result":"{reply}","total_cost_usd":0.05}}"#
        ),
    ]
}

/// init → text + Read tool_use → tool_result → text → success.
pub fn maestro_tool_fixture(session_id: &str) -> Vec<String> {
    vec![
        format!(
            r#"{{"type":"system","subtype":"init","session_id":"{session_id}","model":"claude-opus-4-6","mcp_servers":[]}}"#
        ),
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Let me check."},{"type":"tool_use","id":"toolu_read1","name":"Read","input":{"file_path":"auth.go"}}]}}"#.into(),
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_read1","is_error":false,"content":"package auth"}]}}"#.into(),
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Auth looks fine."}]}}"#.into(),
        format!(
            r#"{{"type":"result","subtype":"success","session_id":"{session_id}","duration_ms":900,"is_error":false,"num_turns":1,"result":"Auth looks fine.","total_cost_usd":0.05}}"#
        ),
    ]
}

/// Records every request and returns a benign success.
pub struct Capture {
    reqs: Mutex<Vec<TurnRequest>>,
    inner: Arc<dyn Provider>,
}

impl Capture {
    pub fn empty() -> Arc<Self> {
        Arc::new(Self {
            reqs: Mutex::new(Vec::new()),
            inner: claude(),
        })
    }

    pub fn requests(&self) -> Vec<TurnRequest> {
        self.reqs.lock().clone()
    }
}

#[async_trait::async_trait]
impl Provider for Capture {
    fn id(&self) -> &str {
        self.inner.id()
    }
    fn label(&self) -> &str {
        self.inner.label()
    }
    fn caps(&self) -> &ProviderCaps {
        self.inner.caps()
    }
    fn availability(&self, _s: &Settings) -> Availability {
        Availability::Ready
    }
    async fn status(&self, s: &Settings) -> ProviderStatus {
        self.inner.status(s).await
    }
    async fn models(&self, s: &Settings) -> Result<Vec<ModelChoice>, ProviderError> {
        self.inner.models(s).await
    }
    fn cached_models(&self, s: &Settings) -> Vec<ModelChoice> {
        self.inner.cached_models(s)
    }
    fn resolve_model(&self, m: &str, f: &Flags) -> String {
        self.inner.resolve_model(m, f)
    }
    fn context_window(&self, m: &str, f: &Flags) -> i64 {
        self.inner.context_window(m, f)
    }
    fn model_label(&self, m: &str) -> String {
        self.inner.model_label(m)
    }
    fn oneshot_flags(&self, m: &str) -> Flags {
        self.inner.oneshot_flags(m)
    }
    async fn run_turn(
        &self,
        req: TurnRequest,
        cx: TurnContext,
    ) -> Result<TurnResult, ProviderError> {
        self.reqs.lock().push(req.clone());
        script(Vec::<String>::new()).run_turn(req, cx).await
    }
    fn resume_handle_fits(&self, h: &str) -> bool {
        self.inner.resume_handle_fits(h)
    }
    fn display_tool(&self, n: &str, i: &str) -> ToolDisplay {
        self.inner.display_tool(n, i)
    }
    fn display_tool_result(&self, n: &str, r: &str) -> String {
        self.inner.display_tool_result(n, r)
    }
}

type TurnFn = Arc<dyn Fn(&TurnContext) -> Result<TurnResult, ProviderError> + Send + Sync>;

/// Claude's id, caps and display around a synchronous scripted `run_turn`
/// for ad hoc test providers.
pub struct FnProvider {
    inner: Arc<dyn Provider>,
    run: TurnFn,
}

impl FnProvider {
    pub fn wrap(
        run: impl Fn(&TurnContext) -> Result<TurnResult, ProviderError> + Send + Sync + 'static,
    ) -> Arc<dyn Provider> {
        Arc::new(Self {
            inner: claude(),
            run: Arc::new(run),
        })
    }
}

#[async_trait::async_trait]
impl Provider for FnProvider {
    fn id(&self) -> &str {
        self.inner.id()
    }
    fn label(&self) -> &str {
        self.inner.label()
    }
    fn caps(&self) -> &ProviderCaps {
        self.inner.caps()
    }
    fn availability(&self, _s: &Settings) -> Availability {
        Availability::Ready
    }
    async fn status(&self, s: &Settings) -> ProviderStatus {
        self.inner.status(s).await
    }
    async fn models(&self, s: &Settings) -> Result<Vec<ModelChoice>, ProviderError> {
        self.inner.models(s).await
    }
    fn cached_models(&self, s: &Settings) -> Vec<ModelChoice> {
        self.inner.cached_models(s)
    }
    fn resolve_model(&self, m: &str, f: &Flags) -> String {
        self.inner.resolve_model(m, f)
    }
    fn context_window(&self, m: &str, f: &Flags) -> i64 {
        self.inner.context_window(m, f)
    }
    fn model_label(&self, m: &str) -> String {
        self.inner.model_label(m)
    }
    fn oneshot_flags(&self, m: &str) -> Flags {
        self.inner.oneshot_flags(m)
    }
    async fn run_turn(
        &self,
        _req: TurnRequest,
        cx: TurnContext,
    ) -> Result<TurnResult, ProviderError> {
        (self.run)(&cx)
    }
    fn resume_handle_fits(&self, h: &str) -> bool {
        self.inner.resume_handle_fits(h)
    }
    fn display_tool(&self, n: &str, i: &str) -> ToolDisplay {
        self.inner.display_tool(n, i)
    }
    fn display_tool_result(&self, n: &str, r: &str) -> String {
        self.inner.display_tool_result(n, r)
    }
}

/// A minimal running-ish loop record straight in the store,
/// so a turn can target a workspace without a walk.
pub async fn started_loop(eng: &Engine, project_id: &str) -> String {
    let id = eng.new_workspace_id(project_id, None).await;
    eng.store
        .add_workspace(Workspace {
            id: id.clone(),
            project_id: project_id.to_owned(),
            goal: "Add OAuth provider config".into(),
            state: WorkspaceState::Coding,
            phase: "iteration 2".into(),
            iteration: 2,
            group: Group::Active,
            autonomy: AutonomyLevel::Balanced,
            ..Workspace::default()
        })
        .expect("add workspace");
    id
}

/// A hand-driven workspace row.
pub fn add_solo_workspace(eng: &Engine, id: &str) {
    eng.store
        .add_workspace(Workspace {
            id: id.to_owned(),
            project_id: "p1".into(),
            r#type: recur_domain::WORKSPACE_SOLO.into(),
            short: "hand".into(),
            ..Workspace::default()
        })
        .expect("add workspace");
}

/// Text, then a duration-less result carrying usage, then an error whose
/// result has the duration.
pub fn duration_error() -> Arc<dyn recur_agent::Provider> {
    FnProvider::wrap(|cx| {
        cx.sink.emit(AgentEvent::Text {
            text: "partial…".into(),
            uuid: String::new(),
            parent: String::new(),
        });
        let res = TurnResult {
            duration_ms: 1500,
            is_error: true,
            session_id: "thread-1".into(),
            usage: TokenUsage {
                input_tokens: 200,
                output_tokens: 50,
                cache_read_tokens: 15000,
                context_window: 200_000,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut usage = res.clone();
        usage.duration_ms = 0;
        cx.sink.emit(AgentEvent::TurnEnd(usage));
        Err(
            ProviderError::failed("codex app-server exited before turn completed")
                .with_partial(res),
        )
    })
}

/// The result frame already stamped the duration before the error.
pub fn result_then_error() -> Arc<dyn recur_agent::Provider> {
    FnProvider::wrap(|cx| {
        cx.sink.emit(AgentEvent::Text {
            text: "partial…".into(),
            uuid: String::new(),
            parent: String::new(),
        });
        let res = TurnResult {
            duration_ms: 1500,
            is_error: true,
            session_id: "thread-1".into(),
            ..Default::default()
        };
        cx.sink.emit(AgentEvent::TurnEnd(res.clone()));
        Err(ProviderError::failed("cursor run: boom").with_partial(res))
    })
}
