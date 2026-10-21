//! Recur's in-process coding agent over `POST /v1/responses` (Go
//! `harness.go`): rounds of model output and tool calls, read-only tools in
//! parallel, halting tools, steers between rounds.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;
use recur_agent::{
    AgentEvent, ContextUsageTracker, EventSink, HistorySource, ProviderError, TokenUsage,
    ToolCallMeta, ToolHost, ToolResultEvent, ToolStep, TurnContext, TurnRequest, TurnResult,
};
use recur_agent_tools::bare_name;
use recur_domain::CompatibleProvider;
use serde_json::Value;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::client::{Client, ClientError};
use crate::flags::apply_flags;
use crate::gojson::{marshal_sorted, valid};
use crate::history::{harness_history, harness_instructions};
use crate::tools::{tool_allowed_on_turn, tools_disabled};
use crate::types::{
    Event, FunctionCall, InputItem, Request, Tool, Usage, assistant_text, function_call_item,
    function_output_item, function_tool, user_text,
};

/// Read-only calls in one round run concurrently, at most this many at once.
pub const HARNESS_PARALLEL_LIMIT: usize = 8;

const PARALLEL_TOOL_NAMES: [&str; 10] = [
    "Read",
    "Glob",
    "Grep",
    "list_findings",
    "list_skills",
    "list_artifacts",
    "read_artifact",
    "list_terminals",
    "list_workspace_briefs",
    "get_workspace_context",
];

/// Every event runs through the context tracker and reaches the sink one at
/// a time, also from parallel tool tasks.
pub(crate) struct TurnEmitter {
    sink: Arc<dyn EventSink>,
    tracker: Mutex<ContextUsageTracker>,
}

impl TurnEmitter {
    pub(crate) fn new(sink: Arc<dyn EventSink>) -> Self {
        Self {
            sink,
            tracker: Mutex::new(ContextUsageTracker::default()),
        }
    }

    pub(crate) fn emit(&self, mut ev: AgentEvent) {
        let mut tracker = self.tracker.lock();
        tracker.observe(&mut ev);
        self.sink.emit(ev);
    }
}

/// One call's tool output as fed back to the model.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CallResult {
    pub text: String,
    pub halt: bool,
}

/// Everything one turn needs besides the request.
pub(crate) struct Harness<'a> {
    pub provider: &'a CompatibleProvider,
    pub client: &'a Client,
    pub data_root: Option<std::path::PathBuf>,
}

impl Harness<'_> {
    pub(crate) async fn run(
        &self,
        req: TurnRequest,
        mut cx: TurnContext,
    ) -> Result<TurnResult, ProviderError> {
        let start = Instant::now();
        let emitter = Arc::new(TurnEmitter::new(Arc::clone(&cx.sink)));
        let mut model = req.model.clone();
        if model.is_empty()
            && let Some(m) = self.provider.models.first()
        {
            model = m.id.clone();
        }
        let tools: Vec<Tool> = match (&cx.tools, tools_disabled(&req)) {
            (Some(host), false) => host
                .tools(req.allowed_tools.as_deref())
                .into_iter()
                .map(|t| function_tool(&t.name, &t.description, Some(t.schema)))
                .collect(),
            _ => Vec::new(),
        };
        let mut input = self.build_input(&req, cx.history.as_ref()).await;
        let instructions = harness_instructions(&req);

        let mut usage = Usage::default();
        let mut text = String::new();
        let mut last_id = String::new();
        let mut rounds: u32 = 0;

        emitter.emit(AgentEvent::Init {
            session_id: req.recur_session_id.clone(),
            model: model.clone(),
            mcp_servers: Vec::new(),
        });

        loop {
            if cx.cancel.is_cancelled() {
                return Err(self.canceled(&req, start, &text, usage, &last_id));
            }
            rounds += 1;
            if req.max_turns > 0 && rounds > req.max_turns {
                let mut res = self.result(&req, start, &text, usage, &last_id);
                res.is_error = true;
                res.subtype = "error_max_turns".into();
                emitter.emit(AgentEvent::TurnEnd(res.clone()));
                return Ok(res);
            }
            let round = self
                .round(
                    &req,
                    &cx.cancel,
                    &model,
                    &instructions,
                    &input,
                    &tools,
                    &emitter,
                )
                .await;
            let round = match round {
                Ok(r) => r,
                Err(RoundError::Canceled) => {
                    return Err(self.canceled(&req, start, &text, usage, &last_id));
                }
                Err(RoundError::Failed(msg)) => {
                    return Err(ProviderError::failed(msg)
                        .with_partial(self.result(&req, start, &text, usage, &last_id)));
                }
            };
            if !round.resp_id.is_empty() {
                last_id = round.resp_id;
            }
            if round.usage.input_tokens > 0 || round.usage.output_tokens > 0 {
                usage = round.usage;
            }
            text.push_str(&round.text);
            if !round.text.is_empty() {
                input.push(assistant_text(&round.text));
            }

            let mut halt = false;
            if !round.calls.is_empty() {
                let results =
                    exec_calls(&req, &cx.cancel, cx.tools.as_ref(), &round.calls, &emitter).await;
                if cx.cancel.is_cancelled() {
                    return Err(self.canceled(&req, start, &text, usage, &last_id));
                }
                for (call, res) in round.calls.iter().zip(&results) {
                    input.push(function_call_item(
                        &call.call_id,
                        &call.name,
                        &call.arguments,
                    ));
                    input.push(function_output_item(&call.call_id, &res.text));
                    halt |= res.halt;
                }
            }
            if halt {
                break;
            }
            let steers = cx.steer.as_mut().map(|s| s.drain()).unwrap_or_default();
            if steers.is_empty() {
                if round.calls.is_empty() {
                    break;
                }
                continue;
            }
            for s in steers {
                let (prompt, display) = if s.is_plain() {
                    let (p, d) = s.plain_parts();
                    let d = if d == p { String::new() } else { d };
                    (p, d)
                } else {
                    (s.prompt.clone(), String::new())
                };
                emitter.emit(AgentEvent::User {
                    text: prompt.clone(),
                    display,
                    live_steer: true,
                });
                input.push(user_text(&prompt));
                s.ack(true);
            }
        }

        let res = self.result(&req, start, &text, usage, &last_id);
        emitter.emit(AgentEvent::TurnEnd(res.clone()));
        Ok(res)
    }

    async fn build_input(
        &self,
        req: &TurnRequest,
        history: Option<&Arc<dyn HistorySource>>,
    ) -> Vec<InputItem> {
        let mut msgs = Vec::new();
        let mut after = req.history_after_id.clone();
        if let Some(h) = history
            && !req.recur_session_id.is_empty()
        {
            msgs = h.transcript(&req.recur_session_id).await;
            if after.is_empty() {
                after = h
                    .history_after(&req.recur_session_id)
                    .await
                    .trim()
                    .to_owned();
            }
        }
        let req = TurnRequest {
            history_after_id: after,
            ..req.clone()
        };
        harness_history(&msgs, &req, self.data_root.as_deref())
    }

    #[allow(clippy::too_many_arguments)]
    async fn round(
        &self,
        req: &TurnRequest,
        cancel: &CancellationToken,
        model: &str,
        instructions: &str,
        input: &[InputItem],
        tools: &[Tool],
        emitter: &TurnEmitter,
    ) -> Result<Round, RoundError> {
        let body = Request {
            model: model.into(),
            instructions: instructions.into(),
            input: input.to_vec(),
            tools: tools.to_vec(),
            tool_choice: "auto".into(),
            stream: true,
            store: false,
            parallel_tool_calls: true,
            ..Default::default()
        };
        let mut body =
            serde_json::to_value(&body).map_err(|e| RoundError::Failed(e.to_string()))?;
        if let (Some(m), Value::Object(map)) = (self.provider.model_by_id(model), &mut body) {
            apply_flags(map, &m.flags, &req.flags)
                .map_err(|e| RoundError::Failed(e.to_string()))?;
        }
        let payload = marshal_sorted(&body);

        let mut st = RoundState::default();
        let streamed = self
            .client
            .stream_json(cancel, payload, &mut |ev| {
                st.on_event(ev, req.include_partials, emitter)
            })
            .await;
        let (text, think) = st.flush_items();
        if let Some(fail) = st.fail {
            return Err(RoundError::Failed(fail));
        }
        match streamed {
            Err(ClientError::Canceled) => return Err(RoundError::Canceled),
            Err(ClientError::Failed(e)) => return Err(RoundError::Failed(e)),
            Ok(()) => {}
        }
        terminal_response_error(&st.status, st.saw_completed).map_err(RoundError::Failed)?;
        if !text.is_empty() {
            emitter.emit(AgentEvent::Text {
                text: text.clone(),
                uuid: String::new(),
                parent: String::new(),
            });
        }
        if !think.is_empty() {
            emitter.emit(AgentEvent::Thinking {
                text: think,
                signature: String::new(),
                uuid: String::new(),
                parent: String::new(),
            });
        }
        let calls = st
            .order
            .iter()
            .filter_map(|id| st.pending.get(id))
            .filter(|c| !c.name.is_empty())
            .cloned()
            .collect();
        Ok(Round {
            calls,
            text,
            usage: st.usage,
            resp_id: st.resp_id,
        })
    }

    fn result(
        &self,
        req: &TurnRequest,
        start: Instant,
        text: &str,
        usage: Usage,
        resp_id: &str,
    ) -> TurnResult {
        let window = self
            .provider
            .model_by_id(&req.model)
            .map_or(0, |m| m.context_window);
        TurnResult {
            session_id: if resp_id.is_empty() {
                req.recur_session_id.clone()
            } else {
                resp_id.to_owned()
            },
            subtype: "success".into(),
            text: text.to_owned(),
            duration_ms: start.elapsed().as_millis() as i64,
            usage: TokenUsage {
                input_tokens: responses_fresh_input(usage),
                output_tokens: usage.output_tokens,
                cache_read_tokens: usage.cache_read_tokens,
                context_window: window,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn canceled(
        &self,
        req: &TurnRequest,
        start: Instant,
        text: &str,
        usage: Usage,
        resp_id: &str,
    ) -> ProviderError {
        ProviderError::canceled().with_partial(self.result(req, start, text, usage, resp_id))
    }
}

/// Fresh prompt tokens: input minus cache reads (input as is when the
/// provider reports more cached than total).
pub fn responses_fresh_input(u: Usage) -> i64 {
    let fresh = u.input_tokens - u.cache_read_tokens;
    if fresh < 0 { u.input_tokens } else { fresh }
}

/// A round only counts when the response reached a terminal success state.
pub fn terminal_response_error(status: &str, saw_completed: bool) -> Result<(), String> {
    match status {
        "failed" | "cancelled" | "canceled" | "incomplete" => {
            return Err(format!("responses status {status}"));
        }
        _ => {}
    }
    if saw_completed || status == "completed" {
        return Ok(());
    }
    Err("responses: stream ended before completion".into())
}

enum RoundError {
    Canceled,
    Failed(String),
}

struct Round {
    calls: Vec<FunctionCall>,
    text: String,
    usage: Usage,
    resp_id: String,
}

/// One Responses output item's streamed vs final text.
#[derive(Default)]
struct HarnessItem {
    text_stream: String,
    think_stream: String,
    text_final: String,
    think_final: String,
}

#[derive(Default)]
struct RoundState {
    pending: HashMap<String, FunctionCall>,
    order: Vec<String>,
    items: HashMap<String, HarnessItem>,
    item_order: Vec<String>,
    anon: usize,
    status: String,
    saw_completed: bool,
    fail: Option<String>,
    usage: Usage,
    resp_id: String,
}

impl RoundState {
    fn item(&mut self, ev: &Event) -> &mut HarnessItem {
        let mut id = ev.item_id.clone();
        if id.is_empty()
            && self
                .items
                .get("")
                .is_some_and(|b| !b.text_final.is_empty() || !b.think_final.is_empty())
        {
            self.anon += 1;
            id = format!("anon-{}", self.anon);
        }
        if !self.items.contains_key(&id) {
            self.item_order.push(id.clone());
        }
        self.items.entry(id).or_default()
    }

    fn on_event(&mut self, ev: Event, partials: bool, emitter: &TurnEmitter) {
        if !ev.response_id.is_empty() {
            self.resp_id = ev.response_id.clone();
        }
        if !ev.status.is_empty() {
            self.status = ev.status.clone();
        }
        if ev.r#type == "response.completed" {
            self.saw_completed = true;
        }
        if ev.r#type == "response.incomplete" {
            self.status = "incomplete".into();
        }
        if !ev.error_message.is_empty() || ev.r#type == "error" || ev.r#type == "response.failed" {
            let msg = if ev.error_message.is_empty() {
                &ev.r#type
            } else {
                &ev.error_message
            };
            self.fail = Some(format!("responses: {msg}"));
            return;
        }
        let t = ev.r#type.as_str();
        if t == "response.output_text.delta" || t.ends_with("output_text.delta") {
            if ev.delta.is_empty() {
                return;
            }
            self.item(&ev).text_stream.push_str(&ev.delta);
            if partials {
                emitter.emit(AgentEvent::TextDelta {
                    text: ev.delta.clone(),
                    parent: String::new(),
                });
            }
        } else if t == "response.reasoning_summary_text.delta"
            || t == "response.reasoning_text.delta"
        {
            if ev.delta.is_empty() {
                return;
            }
            self.item(&ev).think_stream.push_str(&ev.delta);
            if partials {
                emitter.emit(AgentEvent::ThinkingDelta {
                    text: ev.delta.clone(),
                    parent: String::new(),
                });
            }
        } else if t == "response.reasoning_summary_text.done" || t == "response.reasoning_text.done"
        {
            let b = self.item(&ev);
            if b.think_stream.is_empty() {
                b.think_final = ev.delta.clone();
            }
        } else if t == "response.output_text.done" {
            let b = self.item(&ev);
            if b.text_stream.is_empty() {
                b.text_final = ev.delta.clone();
            }
        } else if (t == "response.output_item.done" || t.ends_with("output_item.done"))
            && !ev.output_text.is_empty()
        {
            let b = self.item(&ev);
            if b.text_stream.is_empty() {
                b.text_final = ev.output_text.clone();
            }
        } else if let Some(call) = &ev.call {
            let mut c = call.clone();
            let mut id = if c.id.is_empty() {
                c.call_id.clone()
            } else {
                c.id.clone()
            };
            if id.is_empty() {
                id = format!("call-{}", self.order.len());
            }
            if c.call_id.is_empty() {
                c.call_id = id.clone();
            }
            if !self.pending.contains_key(&id) {
                self.order.push(id.clone());
            }
            self.pending.insert(id, c);
        } else if t == "response.function_call_arguments.delta"
            && !ev.delta.is_empty()
            && let Some(p) = self.pending.get_mut(&ev.item_id)
        {
            p.arguments.push_str(&ev.delta);
        }
        if let Some(u) = ev.usage {
            self.usage = u;
        }
    }

    fn flush_items(&self) -> (String, String) {
        let mut text = String::new();
        let mut think = String::new();
        for id in &self.item_order {
            let Some(b) = self.items.get(id) else {
                continue;
            };
            text.push_str(if b.text_stream.is_empty() {
                &b.text_final
            } else {
                &b.text_stream
            });
            think.push_str(if b.think_stream.is_empty() {
                &b.think_final
            } else {
                &b.think_stream
            });
        }
        (text, think)
    }
}

/// Runs one round's calls: read-only tools first, in parallel up to
/// [`HARNESS_PARALLEL_LIMIT`], then the rest in order, stopping after a
/// halting tool or cancellation. Unrun calls keep an empty result.
pub(crate) async fn exec_calls(
    req: &TurnRequest,
    cancel: &CancellationToken,
    tools: Option<&Arc<dyn ToolHost>>,
    calls: &[FunctionCall],
    emitter: &Arc<TurnEmitter>,
) -> Vec<CallResult> {
    let mut out = vec![CallResult::default(); calls.len()];
    let parallel_names: HashSet<&str> = PARALLEL_TOOL_NAMES.into_iter().collect();
    let (parallel, serial): (Vec<usize>, Vec<usize>) =
        (0..calls.len()).partition(|&i| parallel_names.contains(bare_name(&calls[i].name)));
    if cancel.is_cancelled() {
        return out;
    }
    if !parallel.is_empty() {
        let sem = Arc::new(Semaphore::new(HARNESS_PARALLEL_LIMIT));
        let mut set = tokio::task::JoinSet::new();
        for i in parallel {
            let permit = tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                p = Arc::clone(&sem).acquire_owned() => match p {
                    Ok(p) => p,
                    Err(_) => break,
                },
            };
            let (req, cancel, tools, call, emitter) = (
                req.clone(),
                cancel.clone(),
                tools.cloned(),
                calls[i].clone(),
                Arc::clone(emitter),
            );
            set.spawn(async move {
                let _permit = permit;
                (
                    i,
                    exec_one(&req, &cancel, tools.as_ref(), &call, &emitter).await,
                )
            });
        }
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok((i, r)) => out[i] = r,
                Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
                Err(_) => {}
            }
        }
    }
    for i in serial {
        if cancel.is_cancelled() {
            break;
        }
        out[i] = exec_one(req, cancel, tools, &calls[i], emitter).await;
        if out[i].halt || cancel.is_cancelled() {
            break;
        }
    }
    out
}

async fn exec_one(
    req: &TurnRequest,
    cancel: &CancellationToken,
    tools: Option<&Arc<dyn ToolHost>>,
    call: &FunctionCall,
    emitter: &TurnEmitter,
) -> CallResult {
    let name = call.name.as_str();
    let args = if valid(&call.arguments) {
        call.arguments.as_str()
    } else {
        "{}"
    };
    emitter.emit(AgentEvent::Step(ToolStep {
        id: call.call_id.clone(),
        name: name.into(),
        summary: args.into(),
        input: args.into(),
        ..Default::default()
    }));
    let outcome = if !tool_allowed_on_turn(name, req) {
        None
    } else if let Some(host) = tools {
        Some(
            host.call(
                name,
                args,
                ToolCallMeta {
                    call_id: call.call_id.clone(),
                    cancel: cancel.clone(),
                },
            )
            .await,
        )
    } else {
        Some(recur_agent::ToolOutcome {
            text: format!("unknown tool: {name}"),
            is_error: true,
            halt: false,
        })
    };
    let outcome = outcome.unwrap_or_else(|| recur_agent::ToolOutcome {
        text: format!("tool not allowed: {name}"),
        is_error: true,
        halt: false,
    });
    emitter.emit(AgentEvent::ToolResult(ToolResultEvent {
        id: call.call_id.clone(),
        name: name.into(),
        text: outcome.text.clone(),
        is_error: outcome.is_error,
        ..Default::default()
    }));
    CallResult {
        text: outcome.text,
        halt: !outcome.is_error && outcome.halt,
    }
}
