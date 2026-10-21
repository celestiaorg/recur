//! Test doubles (Go `ScriptedRunner`). Never a production provider.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use recur_domain::Settings;

use crate::catalog::ModelChoice;
use crate::display::{ToolDisplay, canonical_claude_tool};
use crate::error::ProviderError;
use crate::event::{AgentEvent, ContextUsageTracker, TurnResult};
use crate::provider::{
    Availability, Flags, HistoryMode, McpTransports, PermissionRoute, Provider, ProviderCaps,
    ProviderStatus, SteerSupport, ToolTransport,
};
use crate::quota::UsageProbe;
use crate::turn::{TurnContext, TurnRequest};

type OnRun = Arc<dyn Fn(&TurnRequest) + Send + Sync>;

/// An [`crate::EventSink`] that keeps every event, in order.
#[derive(Clone, Default)]
pub struct EventRecorder(Arc<std::sync::Mutex<Vec<AgentEvent>>>);

impl EventRecorder {
    /// The events so far, leaving the recorder empty.
    pub fn take(&self) -> Vec<AgentEvent> {
        std::mem::take(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl crate::turn::EventSink for EventRecorder {
    fn emit(&self, ev: AgentEvent) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).push(ev);
    }
}

/// Replays a canned event script, optionally after writing scripted file
/// edits into the worktree, so the engine can be exercised without a real
/// agent. The script is a list of frames (one per provider line);
/// cancellation is checked between frames. Each event runs through a
/// [`ContextUsageTracker`] before it is emitted. When steering is armed, one
/// queued steer is taken after each tool result and held unacked until the
/// turn returns, like Go's ScriptedRunner draining `Inject`.
///
/// Every call replays the same script. With [`ScriptedProvider::imitating`]
/// everything but the turn itself (id, caps, models, context window,
/// display) comes from a real provider.
#[derive(Clone)]
pub struct ScriptedProvider {
    frames: Vec<Vec<AgentEvent>>,
    edits: BTreeMap<String, String>,
    on_run: Option<OnRun>,
    imitate: Option<Arc<dyn Provider>>,
}

const CAPS: ProviderCaps = ProviderCaps {
    permission: PermissionRoute::McpPromptTool,
    steer: SteerSupport::Live,
    history: HistoryMode::Native,
    tool_transport: ToolTransport::Mcp,
    native_fork: true,
    images: true,
    can_disable_tools: true,
    max_turns: true,
    partials: true,
    user_mcp: McpTransports {
        http: true,
        stdio: true,
    },
};

impl ScriptedProvider {
    pub fn new(frames: Vec<Vec<AgentEvent>>) -> Self {
        Self {
            frames,
            edits: BTreeMap::new(),
            on_run: None,
            imitate: None,
        }
    }

    /// One frame per event.
    pub fn from_events(events: Vec<AgentEvent>) -> Self {
        Self::new(events.into_iter().map(|e| vec![e]).collect())
    }

    /// Repo-relative path → contents, written before the script replays.
    pub fn with_edits(mut self, edits: impl IntoIterator<Item = (String, String)>) -> Self {
        self.edits.extend(edits);
        self
    }

    /// Called once at the start of every turn with the request.
    pub fn on_run(mut self, f: impl Fn(&TurnRequest) + Send + Sync + 'static) -> Self {
        self.on_run = Some(Arc::new(f));
        self
    }

    /// Delegates everything but `run_turn` to `provider`.
    pub fn imitating(mut self, provider: Arc<dyn Provider>) -> Self {
        self.imitate = Some(provider);
        self
    }

    fn apply_edits(&self, worktree: &Path) -> std::io::Result<()> {
        if self.edits.is_empty() || worktree.as_os_str().is_empty() {
            return Ok(());
        }
        for (rel, content) in &self.edits {
            let path = worktree.join(rel);
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, content)?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl Provider for ScriptedProvider {
    fn id(&self) -> &str {
        self.imitate
            .as_ref()
            .map_or(recur_domain::PROVIDER_CLAUDE_CODE, |p| p.id())
    }

    fn label(&self) -> &str {
        self.imitate.as_ref().map_or("Scripted", |p| p.label())
    }

    fn caps(&self) -> &ProviderCaps {
        self.imitate.as_ref().map_or(&CAPS, |p| p.caps())
    }

    fn availability(&self, settings: &Settings) -> Availability {
        self.imitate
            .as_ref()
            .map_or(Availability::Ready, |p| p.availability(settings))
    }

    async fn status(&self, settings: &Settings) -> ProviderStatus {
        match &self.imitate {
            Some(p) => p.status(settings).await,
            None => ProviderStatus {
                installed: true,
                authenticated: true,
                ..Default::default()
            },
        }
    }

    async fn models(&self, settings: &Settings) -> Result<Vec<ModelChoice>, ProviderError> {
        match &self.imitate {
            Some(p) => p.models(settings).await,
            None => Ok(Vec::new()),
        }
    }

    fn cached_models(&self, settings: &Settings) -> Vec<ModelChoice> {
        self.imitate
            .as_ref()
            .map_or_else(Vec::new, |p| p.cached_models(settings))
    }

    fn resolve_model(&self, model: &str, flags: &Flags) -> String {
        self.imitate
            .as_ref()
            .map_or_else(|| model.to_owned(), |p| p.resolve_model(model, flags))
    }

    fn context_window(&self, model: &str, flags: &Flags) -> i64 {
        self.imitate
            .as_ref()
            .map_or(0, |p| p.context_window(model, flags))
    }

    fn model_label(&self, model: &str) -> String {
        self.imitate
            .as_ref()
            .map_or_else(|| model.to_owned(), |p| p.model_label(model))
    }

    fn oneshot_flags(&self, model: &str) -> Flags {
        self.imitate
            .as_ref()
            .map_or_else(Flags::new, |p| p.oneshot_flags(model))
    }

    async fn run_turn(
        &self,
        req: TurnRequest,
        cx: TurnContext,
    ) -> Result<TurnResult, ProviderError> {
        if let Some(f) = &self.on_run {
            f(&req);
        }
        self.apply_edits(&req.worktree)
            .map_err(|e| ProviderError::failed(e.to_string()))?;
        let mut steer = cx.steer;
        let mut held = Vec::new();
        let mut tracker = ContextUsageTracker::default();
        let mut result = None;
        for frame in &self.frames {
            if cx.cancel.is_cancelled() {
                return Err(ProviderError::canceled().with_partial(result.unwrap_or_default()));
            }
            for ev in frame {
                let mut ev = ev.clone();
                tracker.observe(&mut ev);
                if let AgentEvent::TurnEnd(r) = &ev {
                    result = Some(r.clone());
                }
                let tool_result = matches!(ev, AgentEvent::ToolResult(_));
                cx.sink.emit(ev);
                if tool_result && let Some(s) = steer.as_mut().and_then(|s| s.try_next()) {
                    held.push(s);
                }
            }
        }
        drop(held);
        // A script without a result line still returns a benign success so
        // the engine can advance.
        Ok(result.unwrap_or_else(|| TurnResult {
            subtype: "success".into(),
            ..Default::default()
        }))
    }

    fn resume_handle_fits(&self, handle: &str) -> bool {
        self.imitate
            .as_ref()
            .is_none_or(|p| p.resume_handle_fits(handle))
    }

    fn display_tool(&self, name: &str, input: &str) -> ToolDisplay {
        match &self.imitate {
            Some(p) => p.display_tool(name, input),
            None => ToolDisplay {
                name: canonical_claude_tool(name).to_owned(),
                input: input.to_owned(),
            },
        }
    }

    fn display_tool_result(&self, name: &str, result: &str) -> String {
        match &self.imitate {
            Some(p) => p.display_tool_result(name, result),
            None => result.to_owned(),
        }
    }

    fn usage_probe(&self) -> Option<Arc<dyn UsageProbe>> {
        self.imitate.as_ref().and_then(|p| p.usage_probe())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{TokenUsage, ToolResultEvent};
    use crate::turn::{Steer, SteerInbox};
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    type Recorder = EventRecorder;

    fn cx(rec: &Recorder) -> TurnContext {
        TurnContext::new(CancellationToken::new(), Arc::new(rec.clone()))
    }

    fn tool_result() -> AgentEvent {
        AgentEvent::ToolResult(ToolResultEvent {
            id: "t1".into(),
            ..Default::default()
        })
    }

    #[tokio::test]
    async fn replays_and_prefers_last_usage() {
        let usage = AgentEvent::Usage(TokenUsage {
            input_tokens: 200,
            cache_read_tokens: 90_000,
            ..Default::default()
        });
        let end = AgentEvent::TurnEnd(TurnResult {
            subtype: "success".into(),
            duration_ms: 800,
            usage: TokenUsage {
                input_tokens: 250,
                cache_read_tokens: 100_000,
                ..Default::default()
            },
            ..Default::default()
        });
        let p = ScriptedProvider::new(vec![vec![usage], vec![end]]);
        let rec = Recorder::default();
        let res = p.run_turn(TurnRequest::default(), cx(&rec)).await.unwrap();
        assert_eq!(
            (res.usage.input_tokens, res.usage.cache_read_tokens),
            (200, 90_000)
        );
        let evs = rec.take();
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[1], AgentEvent::TurnEnd(res));
    }

    #[tokio::test]
    async fn no_result_is_success() {
        let rec = Recorder::default();
        let res = ScriptedProvider::new(vec![])
            .run_turn(TurnRequest::default(), cx(&rec))
            .await
            .unwrap();
        assert_eq!(res.subtype, "success");
    }

    #[tokio::test]
    async fn edits_and_on_run() {
        let dir = tempfile::tempdir().unwrap();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let s2 = seen.clone();
        let p = ScriptedProvider::new(vec![])
            .with_edits([("a/b.txt".to_owned(), "hi\n".to_owned())])
            .on_run(move |r| s2.lock().unwrap().push(r.agent_id.clone()));
        let req = TurnRequest {
            agent_id: "W-CO".into(),
            worktree: dir.path().into(),
            ..Default::default()
        };
        p.run_turn(req, cx(&Recorder::default())).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a/b.txt")).unwrap(),
            "hi\n"
        );
        assert_eq!(*seen.lock().unwrap(), ["W-CO"]);
    }

    #[tokio::test]
    async fn cancel_between_frames_keeps_partial() {
        let end = AgentEvent::TurnEnd(TurnResult {
            subtype: "success".into(),
            session_id: "s".into(),
            ..Default::default()
        });
        let p = ScriptedProvider::new(vec![vec![end], vec![tool_result()]]);
        let rec = Recorder::default();
        let c = cx(&rec);
        c.cancel.cancel();
        let err = p.run_turn(TurnRequest::default(), c).await.unwrap_err();
        assert!(err.is_canceled());
        assert!(rec.take().is_empty());
    }

    #[tokio::test]
    async fn one_steer_drained_per_tool_result() {
        let (tx, rx) = mpsc::channel(4);
        let (s1, a1) = Steer::new("one", None);
        let (s2, a2) = Steer::new("two", None);
        tx.send(s1).await.unwrap();
        tx.send(s2).await.unwrap();
        let rec = Recorder::default();
        let mut c = cx(&rec);
        c.steer = Some(SteerInbox(rx));
        let p = ScriptedProvider::from_events(vec![tool_result()]);
        p.run_turn(TurnRequest::default(), c).await.unwrap();
        assert!(a1.await.is_err(), "drained steer is never acked");
        assert!(a2.await.is_err(), "inbox dropped with the turn");
    }
}
