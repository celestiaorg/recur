//! One turn's frames folded into agent events and a result.
//!
//! Cursor streams differently from Claude Code, and each fold below exists
//! because the naive mapping broke a transcript:
//!
//! - A tool call is a RUN of frames (`edit` sends running, running,
//!   completed): one Step and one ToolResult per call id.
//! - A failed call gets NO terminal frame; open calls are closed at the next
//!   reasoning or reply frame (the model cannot speak until its tool results
//!   are back) and at the end of the run, stating only that no result came.
//! - Assistant frames are chunks: one Text block per contiguous run, a
//!   TextDelta per chunk.
//! - Reasoning is a run of chunks ended by an empty frame: one Thinking
//!   block; ThinkingDelta only with partials.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use recur_agent::display::{create_plan_body, summarize_tool_input};
use recur_agent::{
    AgentEvent, EventSink, Flags, TokenUsage as MeterUsage, ToolResultEvent, ToolStep, TurnResult,
};
use recur_domain::{COMPACT_DONE, COMPACT_STARTED};
use serde::Deserialize;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::context::resolve_context_window;
use crate::events::{
    DELTA_SUMMARY, DELTA_SUMMARY_COMPLETED, DELTA_SUMMARY_STARTED, DELTA_TURN_ENDED, MSG_ASSISTANT,
    MSG_STATUS, MSG_THINKING, MSG_TOOL_CALL, MSG_USAGE, RUN_CANCELLED, RUN_ERROR, RUN_FINISHED,
    RunError, RunResult, RunStreamEvent, STATUS_ERROR, SdkMessage, TOOL_COMPLETED, TOOL_RUNNING,
    TokenUsage, parse_interaction_type, sub_usage,
};
use crate::tools::display_tool;

/// What a call shows when Cursor never reported a result. States only what
/// is known: the provider gives no reason.
pub(crate) const UNREPORTED_RESULT_TEXT: &str = "No result reported. Cursor sends no terminal frame for a tool call that fails, so the reason is not available — the agent's own reply usually explains it.";

/// The synthetic result when a steer CancelRun interrupts in-flight tools:
/// cancelled, not failed.
pub(crate) const STEER_CANCELLED_CALL_TEXT: &str =
    "Interrupted by a Maestro steer — this call was cancelled, not failed.";

/// The turn inputs the fold reads.
#[derive(Debug, Clone, Default)]
pub(crate) struct TurnSpec {
    pub model: String,
    pub flags: Flags,
    pub include_partials: bool,
}

#[derive(Default)]
struct RunIds {
    ids: BTreeSet<String>,
    parent: String,
    /// Drop a CANCELLED result during a steer (D49).
    suppress_result: bool,
}

#[derive(Default)]
pub(crate) struct Fold {
    pub(crate) have_result: bool,
    pub(crate) res: RunResult,
    /// The reason from the last status:ERROR frame: the proto RunResult has
    /// no error, so this fills a bare ERROR's stderr.
    status_error: String,
    pub(crate) last_text: String,
    pub(crate) prior_duration_ms: i64,
    pub(crate) usage: Option<TokenUsage>,
    /// The last frame was a single-call footprint (turn-ended).
    usage_per_call: bool,
    /// The last run-cumulative usage frame (turn-ended may overwrite `usage`).
    pub(crate) usage_cumulative: Option<TokenUsage>,
    /// Cumulative usage at summary-started: later frames still include
    /// pre-compact billing, so the meter subtracts it.
    pub(crate) compact_baseline: Option<TokenUsage>,
    /// The scaled pre-compact fill shown on the compact card.
    pub(crate) compact_pre_fill: i64,
    pub(crate) compact_summary: String,
    compact_emitted: bool,
    /// Tool batches separated by model output. Cursor's usage sums every
    /// model call in the loop; waves + 1 ≈ model calls, so the meter can
    /// scale cumulative billing back to a last-call fill.
    pub(crate) tool_waves: i64,
    pub(crate) wave_open: bool,
    thinking: String,
    text: String,
    /// Call ids that already produced a Step.
    pub(crate) stepped_calls: Option<HashSet<String>>,
    /// The richest createPlan args seen per call id.
    pub(crate) create_plan_args: Option<HashMap<String, String>>,
    /// Calls with a Step but no terminal frame, in call order.
    pub(crate) open_calls: Vec<String>,
    pub(crate) open_tool: Option<HashMap<String, String>>,
    /// Hold the TurnEnd until finish so a live steer's User event precedes
    /// it.
    defer_result: bool,
    result_held: bool,
}

/// The fold for one Cursor turn, shared by the Send stream, the steer
/// listener and the cancel watcher.
pub(crate) struct Turn {
    spec: TurnSpec,
    sink: Option<Arc<dyn EventSink>>,
    agent_id: String,
    ids: Mutex<RunIds>,
    first_id: Notify,
    pub(crate) fold: Mutex<Fold>,
}

impl Turn {
    pub(crate) fn new(
        spec: TurnSpec,
        sink: Option<Arc<dyn EventSink>>,
        agent_id: &str,
        defer_result: bool,
    ) -> Self {
        Self {
            spec,
            sink,
            agent_id: agent_id.to_owned(),
            ids: Mutex::new(RunIds::default()),
            first_id: Notify::new(),
            fold: Mutex::new(Fold {
                defer_result,
                ..Default::default()
            }),
        }
    }

    fn emit(&self, ev: AgentEvent) {
        if let Some(s) = &self.sink {
            s.emit(ev);
        }
    }

    // ---- run ids (CancelRun targets)

    /// Records a provider run id; the first one wakes waiters.
    pub(crate) fn note_run_id(&self, id: &str) {
        if id.is_empty() {
            return;
        }
        let became_parent = {
            let mut g = self.ids.lock();
            let became = g.parent.is_empty();
            if became {
                g.parent = id.to_owned();
            }
            g.ids.insert(id.to_owned());
            became
        };
        if became_parent {
            self.first_id.notify_waiters();
        }
    }

    /// Every run id seen (parent and Task sub-agents), sorted.
    pub(crate) fn all_run_ids(&self) -> Vec<String> {
        self.ids.lock().ids.iter().cloned().collect()
    }

    pub(crate) fn parent_run(&self) -> String {
        self.ids.lock().parent.clone()
    }

    /// Drops the steer target so the next Send's first id becomes it; run
    /// ids stay so CancelRun still reaches recovery leftovers.
    pub(crate) fn clear_steer_target(&self) {
        self.ids.lock().parent.clear();
    }

    /// Waits until a run id is known, `d` elapses, or `cancel` fires.
    pub(crate) async fn wait_run_id(&self, cancel: &CancellationToken, d: Duration) -> String {
        let deadline = tokio::time::Instant::now() + d;
        loop {
            let notified = self.first_id.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let id = self.parent_run();
            if !id.is_empty() {
                return id;
            }
            tokio::select! {
                _ = &mut notified => {}
                _ = cancel.cancelled() => return String::new(),
                _ = tokio::time::sleep_until(deadline) => return self.parent_run(),
            }
        }
    }

    pub(crate) fn set_suppress_result(&self, v: bool) {
        self.ids.lock().suppress_result = v;
    }

    pub(crate) fn suppressing_result(&self) -> bool {
        self.ids.lock().suppress_result
    }

    pub(crate) fn have_result(&self) -> bool {
        self.fold.lock().have_result
    }

    // ---- steer follow-up

    /// Resets per-run state before a follow-up Send on the same agent. The
    /// suppress flag stays: a steer landing in the take/follow-up window
    /// would otherwise emit error_cancelled and drop the follow-up.
    pub(crate) fn prepare_steer_follow_up(&self) {
        let mut f = self.fold.lock();
        self.finish_steer_cancel_locked(&mut f);
        // A steer interrupt can return Send before the CANCELLED frame is
        // read: bank mid-stream duration here (idempotent).
        f.bank_suppressed_duration();
        f.have_result = false;
        f.res = RunResult::default();
        f.tool_waves = 0;
        f.wave_open = false;
        f.compact_baseline = None;
        f.compact_pre_fill = 0;
        f.compact_summary.clear();
        f.stepped_calls = None;
        f.create_plan_args = None;
        f.open_calls.clear();
        f.open_tool = None;
        f.usage = None;
        f.usage_cumulative = None;
        f.usage_per_call = false;
        drop(f);
        let mut g = self.ids.lock();
        g.ids.clear();
        g.parent.clear();
    }

    /// Flushes everything still buffered at the end of a turn.
    pub(crate) fn finish(&self) {
        let mut f = self.fold.lock();
        self.flush_thinking(&mut f);
        self.close_open_calls(&mut f, UNREPORTED_RESULT_TEXT, true);
        self.flush_text(&mut f);
        if f.result_held {
            f.result_held = false;
            self.emit_result(&f);
        }
    }

    fn finish_steer_cancel_locked(&self, f: &mut Fold) {
        self.flush_thinking(f);
        self.close_open_calls(f, STEER_CANCELLED_CALL_TEXT, false);
        self.flush_text(f);
    }

    /// One event outside the stream (recovery notices).
    pub(crate) fn fire(&self, ev: AgentEvent) {
        let _g = self.fold.lock();
        self.emit(ev);
    }

    /// User rows for a follow-up Send or a live delivered steer. A live steer
    /// first flushes the reasoning and reply it interrupts.
    pub(crate) fn fire_users(&self, users: Vec<AgentEvent>, live: bool) {
        if users.is_empty() {
            return;
        }
        let mut f = self.fold.lock();
        if live {
            self.flush_thinking(&mut f);
            self.flush_text(&mut f);
        }
        for u in users {
            self.emit(u);
        }
    }

    fn emit_result(&self, f: &Fold) {
        // Like Claude's stream: the TurnEnd lets the mirror stamp duration and
        // the live UI show the turn footer.
        self.emit(AgentEvent::TurnEnd(self.result_of(f)));
    }

    // ---- frames

    pub(crate) fn on_event(&self, ev: RunStreamEvent) {
        let mut f = self.fold.lock();
        // Every frame's run ids, so CancelRun reaches Task sub-agent runs too.
        if let Some(r) = &ev.result {
            self.note_run_id(&r.run_id);
            if let Some(rr) = &r.result {
                self.note_run_id(&rr.run_id);
            }
        }
        if let Some(m) = &ev.sdk_message {
            self.note_run_id(&m.run_id);
        }

        let has_result = ev.result.as_ref().is_some_and(|r| r.result.is_some());
        if !ev.interaction_update.is_empty() {
            self.on_interaction(&mut f, &ev.interaction_update);
            if ev.sdk_message.is_none() && !has_result {
                return;
            }
        }

        if let Some(rr) = ev.result.and_then(|r| r.result) {
            self.flush_thinking(&mut f);
            f.res = rr;
            if f.res.is_error() && f.res.error.is_none() && !f.status_error.is_empty() {
                f.res.error = Some(RunError {
                    message: f.status_error.clone(),
                    code: String::new(),
                });
            }
            f.have_result = true;
            // A steer CancelRun ends the stream CANCELLED: emitting it would
            // publish turn_end and fail the turn. FINISHED still wins (late
            // steer); ERROR is not suppressed (it must surface).
            if self.suppressing_result() && f.res.status == RUN_CANCELLED {
                self.close_open_calls(&mut f, STEER_CANCELLED_CALL_TEXT, false);
                self.flush_text(&mut f);
                f.bank_suppressed_duration();
                f.have_result = false;
                return;
            }
            self.close_open_calls(&mut f, UNREPORTED_RESULT_TEXT, true);
            self.flush_text(&mut f);
            if f.defer_result {
                f.result_held = true;
                return;
            }
            self.emit_result(&f);
            return;
        }
        let Some(m) = ev.sdk_message else {
            return;
        };

        match m.kind.as_str() {
            MSG_STATUS if m.status == STATUS_ERROR && !m.message.is_empty() => {
                f.status_error = expand_cursor_run_error(&m.message);
            }
            MSG_ASSISTANT => {
                let chunk = m
                    .content
                    .as_ref()
                    .map(|c| c.text_content())
                    .unwrap_or_default();
                if chunk.is_empty() {
                    return;
                }
                // The model cannot speak again until its tool results are
                // back, so any open call is finished.
                self.close_open_calls(&mut f, UNREPORTED_RESULT_TEXT, true);
                self.flush_thinking(&mut f);
                f.text.push_str(&chunk);
                // Each chunk streams live; the mirror suppresses the duplicate
                // publish when the accumulated Text lands.
                self.emit(AgentEvent::TextDelta {
                    text: chunk,
                    parent: String::new(),
                });
            }
            MSG_TOOL_CALL => {
                self.flush_thinking(&mut f);
                self.flush_text(&mut f);
                // Raw provider data flows through: Cursor's name, verbatim
                // args and result envelope. Rendering is the display
                // boundary's job.
                if m.status == TOOL_RUNNING {
                    self.emit_step_once(&mut f, &m);
                    return;
                }
                // ANY non-running status is terminal, and a terminal frame
                // with no running one still gets its step.
                self.emit_step_once(&mut f, &m);
                f.resolve_call(&m.call_id);
                let is_error = m.status != TOOL_COMPLETED || m.tool_result_is_error();
                self.emit(AgentEvent::ToolResult(ToolResultEvent {
                    id: m.call_id,
                    name: m.name,
                    text: m.result,
                    is_error,
                    truncated: m.truncated.result,
                    ..Default::default()
                }));
            }
            MSG_USAGE => {
                // Mid-stream usage: a run that dies before its result still
                // reports what it spent, and the meter updates live.
                if let Some(u) = m.usage {
                    self.emit_cursor_usage(&mut f, u, false);
                }
            }
            MSG_THINKING => {
                if !m.text.is_empty() {
                    // Same boundary as a reply.
                    self.close_open_calls(&mut f, UNREPORTED_RESULT_TEXT, true);
                    self.flush_text(&mut f);
                    f.thinking.push_str(&m.text);
                    // Reasoning streams to the thinking pane, never as a text
                    // delta (that would splice it into the visible reply).
                    if self.spec.include_partials {
                        self.emit(AgentEvent::ThinkingDelta {
                            text: m.text,
                            parent: String::new(),
                        });
                    }
                    return;
                }
                self.flush_thinking(&mut f);
            }
            _ => {}
        }
    }

    /// The call's Step the first time it is seen. createPlan streams its
    /// markdown across running frames and a Step freezes args on first emit,
    /// so createPlan waits for the terminal frame and uses the richest args.
    fn emit_step_once(&self, f: &mut Fold, m: &SdkMessage) {
        let stepped = f.stepped_calls.get_or_insert_with(HashSet::new);
        let mut args = m.args.clone();
        if m.name == "createPlan" {
            let plans = f.create_plan_args.get_or_insert_with(HashMap::new);
            let prev = plans.get(&m.call_id).map(String::as_str).unwrap_or("");
            if create_plan_body(&m.args).len() >= create_plan_body(prev).len() {
                plans.insert(m.call_id.clone(), m.args.clone());
            }
            if m.status == TOOL_RUNNING {
                return;
            }
            if let Some(richest) = plans.get(&m.call_id).filter(|r| !r.is_empty()) {
                args = richest.clone();
            }
        }
        // A blank id cannot be correlated, so it is never deduped: better a
        // duplicate row than a swallowed call.
        if !m.call_id.is_empty() {
            if !stepped.insert(m.call_id.clone()) {
                return;
            }
            // The first tool after a model boundary starts a wave.
            if !f.wave_open {
                f.tool_waves += 1;
                f.wave_open = true;
            }
            f.open_calls.push(m.call_id.clone());
            f.open_tool
                .get_or_insert_with(HashMap::new)
                .insert(m.call_id.clone(), m.name.clone());
        }
        let d = display_tool(&m.name, &args);
        let summary = summarize_tool_input(&d.name, &d.input);
        self.emit(AgentEvent::Step(ToolStep {
            id: m.call_id.clone(),
            name: m.name.clone(),
            summary,
            input: args.clone(),
            ..Default::default()
        }));
        // The plan-mode harvest reads this instead of the tool name.
        if m.name == "createPlan" {
            let body = create_plan_body(&args);
            if !body.is_empty() {
                self.emit(AgentEvent::PlanProposal { body });
            }
        }
    }

    /// Gives every call without a terminal frame a synthetic result, and
    /// always ends the tool wave (callers are at a model boundary).
    fn close_open_calls(&self, f: &mut Fold, text: &str, is_error: bool) {
        f.wave_open = false;
        if f.open_calls.is_empty() {
            return;
        }
        let open = std::mem::take(&mut f.open_calls);
        for id in open {
            let name = f
                .open_tool
                .as_ref()
                .and_then(|t| t.get(&id))
                .cloned()
                .unwrap_or_default();
            self.emit(AgentEvent::ToolResult(ToolResultEvent {
                id,
                name,
                text: text.to_owned(),
                is_error,
                ..Default::default()
            }));
        }
    }

    /// The accumulated reply chunks as ONE text block.
    fn flush_text(&self, f: &mut Fold) {
        if f.text.is_empty() {
            return;
        }
        f.wave_open = false;
        let text = std::mem::take(&mut f.text);
        // The result carries the whole reply, so accumulate across runs.
        f.last_text.push_str(&text);
        self.emit(AgentEvent::Text {
            text,
            uuid: String::new(),
            parent: String::new(),
        });
    }

    /// The accumulated reasoning as one thinking block.
    fn flush_thinking(&self, f: &mut Fold) {
        if f.thinking.is_empty() {
            return;
        }
        f.wave_open = false;
        let text = std::mem::take(&mut f.thinking);
        self.emit(AgentEvent::Thinking {
            text,
            signature: String::new(),
            uuid: String::new(),
            parent: String::new(),
        });
    }

    // ---- usage and compaction

    /// A live meter frame. `per_call` means a single conversation-turn
    /// footprint (turn-ended), not run-cumulative billing.
    fn emit_cursor_usage(&self, f: &mut Fold, raw: TokenUsage, per_call: bool) {
        f.usage = Some(raw);
        f.usage_per_call = per_call;
        if !per_call {
            f.usage_cumulative = Some(raw);
        }
        let partial = self.meter(f, &raw, per_call);
        if partial.context_used() > 0 {
            self.emit(AgentEvent::Usage(partial));
        }
    }

    fn meter(&self, f: &Fold, raw: &TokenUsage, per_call: bool) -> MeterUsage {
        let used = match (&f.compact_baseline, per_call) {
            (Some(base), false) => sub_usage(raw, base),
            _ => *raw,
        };
        let mut out = apply_cursor_usage(&used);
        let window = resolve_context_window(&self.spec.model, &self.spec.flags);
        if per_call {
            if window > 0 && out.context_used() > window {
                scale_cursor_meter_usage(&mut out, window, 1);
            }
        } else {
            scale_cursor_meter_usage(&mut out, window, f.model_rounds());
        }
        out
    }

    fn on_interaction(&self, f: &mut Fold, raw: &str) {
        let (kind, body) = parse_interaction_type(raw);
        match kind.as_str() {
            DELTA_SUMMARY_STARTED => self.begin_compact(f),
            DELTA_SUMMARY_COMPLETED => self.finish_compact(f),
            DELTA_SUMMARY => {
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct P {
                    #[serde(deserialize_with = "crate::wire::null_default")]
                    summary: String,
                }
                if let Ok(p) = crate::wire::decode_struct::<P>(&body)
                    && !p.summary.trim().is_empty()
                {
                    f.compact_summary = p.summary;
                }
            }
            DELTA_TURN_ENDED => {
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct P {
                    usage: Option<TokenUsage>,
                }
                let Ok(P { usage: Some(u) }) = crate::wire::decode_struct::<P>(&body) else {
                    return;
                };
                let window = resolve_context_window(&self.spec.model, &self.spec.flags);
                let used = u.prompt_tokens();
                // A total that fits the window is the per-call footprint;
                // larger is run-cumulative billing.
                self.emit_cursor_usage(f, u, window > 0 && used > 0 && used <= window);
            }
            _ => {}
        }
    }

    fn begin_compact(&self, f: &mut Fold) {
        // Flush first so the marker lands after the reply that streamed
        // before auto-compaction.
        self.flush_thinking(f);
        self.flush_text(f);
        f.snapshot_compact_baseline();
        f.compact_emitted = false;
        f.compact_summary.clear();
        // Scale with the current waves before they reset for the
        // post-summary loop.
        f.compact_pre_fill = self.compact_card_tokens(f, f.compact_baseline);
        f.tool_waves = 0;
        f.wave_open = false;
        self.emit(AgentEvent::Compact {
            status: COMPACT_STARTED.into(),
            trigger: "auto".into(),
            summary: String::new(),
            pre_tokens: f.compact_pre_fill,
            uuid: String::new(),
            parent: String::new(),
        });
    }

    fn finish_compact(&self, f: &mut Fold) {
        if f.compact_emitted {
            return;
        }
        if f.compact_baseline.is_none() {
            f.snapshot_compact_baseline();
            f.compact_pre_fill = self.compact_card_tokens(f, f.compact_baseline);
        }
        f.compact_emitted = true;
        self.emit(AgentEvent::Compact {
            status: COMPACT_DONE.into(),
            trigger: "auto".into(),
            summary: f.compact_summary.clone(),
            pre_tokens: f.compact_pre_fill,
            uuid: String::new(),
            parent: String::new(),
        });
        // Pre-compact turn-ended footprints describe discarded context:
        // re-meter from cumulative − baseline (0 until the next usage frame).
        if f.compact_baseline.is_none() {
            return;
        }
        f.usage_per_call = false;
        if let Some(c) = f.usage_cumulative {
            self.emit_cursor_usage(f, c, false);
        }
    }

    fn compact_card_tokens(&self, f: &Fold, u: Option<TokenUsage>) -> i64 {
        let Some(u) = u else {
            return 0;
        };
        let mut out = apply_cursor_usage(&u);
        scale_cursor_meter_usage(
            &mut out,
            resolve_context_window(&self.spec.model, &self.spec.flags),
            f.model_rounds(),
        );
        out.context_used()
    }

    // ---- result

    pub(crate) fn result(&self) -> TurnResult {
        self.result_of(&self.fold.lock())
    }

    fn result_of(&self, f: &Fold) -> TurnResult {
        // The meter prefers the latest mid-stream usage frame; Cursor's
        // result usage is often cumulative across the tool loop.
        let (meter_usage, per_call) = match &f.usage {
            Some(u) => (Some(*u), f.usage_per_call),
            None => (f.res.usage, false),
        };
        let text = if f.res.result.is_empty() {
            f.last_text.clone()
        } else {
            f.res.result.clone()
        };
        let is_error = !f.have_result || f.res.is_error();
        let mut out = TurnResult {
            // Persisted so the next turn can ResumeAgent.
            session_id: self.agent_id.clone(),
            subtype: cursor_subtype(&f.res, f.have_result).into(),
            is_error,
            duration_ms: f.res.duration_ms + f.prior_duration_ms,
            text,
            ..Default::default()
        };
        if let Some(u) = meter_usage {
            let m = self.meter(f, &u, per_call);
            out.usage.input_tokens = m.input_tokens;
            out.usage.output_tokens = m.output_tokens;
            out.usage.cache_creation_tokens = m.cache_creation_tokens;
            out.usage.cache_read_tokens = m.cache_read_tokens;
        }
        if let Some(e) = &f.res.error {
            out.stderr = e.message.clone();
        } else if is_error && !f.status_error.is_empty() {
            out.stderr = f.status_error.clone();
        }
        out
    }

    #[cfg(test)]
    pub(crate) fn model_rounds(&self) -> i64 {
        self.fold.lock().model_rounds()
    }
}

impl Fold {
    fn resolve_call(&mut self, id: &str) {
        if id.is_empty() {
            return;
        }
        if let Some(i) = self.open_calls.iter().position(|c| c == id) {
            self.open_calls.remove(i);
        }
    }

    /// Banks wall time across a cancelled steer and drops usage so the
    /// follow-up result does not reuse this run's snapshot.
    pub(crate) fn bank_suppressed_duration(&mut self) {
        self.prior_duration_ms += self.res.duration_ms;
        self.usage = None;
        self.usage_cumulative = None;
        self.usage_per_call = false;
        self.res.usage = None;
        self.res.duration_ms = 0;
    }

    /// Estimated model calls: one per tool wave plus the final reply.
    fn model_rounds(&self) -> i64 {
        (self.tool_waves + 1).max(1)
    }

    /// The last run-cumulative usage, never a turn-ended per-call footprint.
    fn snapshot_compact_baseline(&mut self) {
        let src = self.usage_cumulative.or(if self.usage_per_call {
            None
        } else {
            self.usage
        });
        if let Some(s) = src {
            self.compact_baseline = Some(s);
        }
    }
}

/// Cursor usage on the meter shape. Cache writes occupy context like fresh
/// input. A total-only frame is stored as input, overstating fill until a
/// breakdown arrives.
pub(crate) fn apply_cursor_usage(u: &TokenUsage) -> MeterUsage {
    let mut out = MeterUsage {
        input_tokens: u.input_tokens,
        output_tokens: u.output_tokens,
        cache_read_tokens: u.cache_read_tokens,
        cache_creation_tokens: u.cache_write_tokens,
        ..Default::default()
    };
    if out.context_used() == 0 {
        out.input_tokens = u.total_tokens;
    }
    out
}

/// Rewrites prompt-side fields when the usage is cumulative across the tool
/// loop. Several model calls use an arithmetic-series last-call estimate
/// (`last ≈ used * 2 / (rounds+1)`) clamped to the window; output tokens are
/// left alone.
pub(crate) fn scale_cursor_meter_usage(out: &mut MeterUsage, window: i64, rounds: i64) {
    let used = out.context_used();
    if used <= 0 {
        return;
    }
    let window = if window <= 0 { 200_000 } else { window };
    let rounds = rounds.max(1);
    let mut target = used;
    if rounds > 1 {
        target = used * 2 / (rounds + 1);
    }
    target = target.min(window).max(1);
    if target == used {
        return;
    }
    // Scale together so the popover split stays meaningful.
    out.input_tokens = out.input_tokens * target / used;
    out.cache_read_tokens = out.cache_read_tokens * target / used;
    out.cache_creation_tokens = out.cache_creation_tokens * target / used;
    let got = out.context_used();
    if got != target && got > 0 {
        out.input_tokens = (out.input_tokens + target - got).max(0);
    }
}

/// Hints for Connect codes Cursor returns with no detail ("[resource_exhausted]
/// Error" reads like a Recur bug).
const CURSOR_RUN_ERROR_HINTS: &[(&str, &str)] = &[
    (
        "resource_exhausted",
        "Cursor's model provider is rate-limited or at capacity. Wait a moment and retry.",
    ),
    (
        "unavailable",
        "Cursor could not reach the model provider. Usually temporary; retry.",
    ),
    (
        "deadline_exceeded",
        "Cursor's model provider timed out. Usually temporary; retry.",
    ),
    (
        "internal",
        "Cursor-side fault, not a defect in the work. Retry.",
    ),
];

/// Appends the hint to a bare "[code] …" status message.
pub(crate) fn expand_cursor_run_error(msg: &str) -> String {
    let Some(rest) = msg.strip_prefix('[') else {
        return msg.to_owned();
    };
    let Some(end) = rest.find(']') else {
        return msg.to_owned();
    };
    let code = rest[..end].to_lowercase();
    match CURSOR_RUN_ERROR_HINTS.iter().find(|(c, _)| *c == code) {
        Some((_, hint)) => format!("{msg}. {hint}"),
        None => msg.to_owned(),
    }
}

/// A Cursor run status in the engine's subtype vocabulary.
pub(crate) fn cursor_subtype(r: &RunResult, have_result: bool) -> &'static str {
    if !have_result {
        return "error_during_execution";
    }
    match r.status.as_str() {
        RUN_FINISHED if r.error.is_some() => "error_during_execution",
        RUN_FINISHED => "success",
        RUN_CANCELLED => "error_cancelled",
        RUN_ERROR => "error_during_execution",
        _ => "success",
    }
}

#[cfg(test)]
pub(crate) mod tests;
