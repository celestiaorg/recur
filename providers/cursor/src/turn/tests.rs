//! Turn frame and result tests and the frame-fold cases.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use recur_agent::AgentEvent;
use serde_json::json;

use super::*;
use crate::events::decode_frame;
use crate::steer::follow_up_users;
use crate::tools::tests::*;

#[derive(Clone, Default)]
pub(crate) struct Rec(Arc<Mutex<Vec<AgentEvent>>>);

impl Rec {
    pub(crate) fn all(&self) -> Vec<AgentEvent> {
        self.0.lock().clone()
    }

    pub(crate) fn sink(&self) -> Arc<dyn EventSink> {
        let r = self.0.clone();
        Arc::new(move |ev: AgentEvent| r.lock().push(ev))
    }
}

pub(crate) fn kinds(evs: &[AgentEvent]) -> Vec<&'static str> {
    evs.iter().map(AgentEvent::kind).collect()
}

fn spec(model: &str, partials: bool) -> TurnSpec {
    TurnSpec {
        model: model.into(),
        flags: Default::default(),
        include_partials: partials,
    }
}

/// Replays frames through a turn; `emit` is immediate, as in a
/// default turn.
fn feed(spec: TurnSpec, frames: &[&str]) -> (Turn, Rec) {
    let rec = Rec::default();
    let t = Turn::new(spec, Some(rec.sink()), "agent-1", false);
    for f in frames {
        t.on_event(decode_frame(f.as_bytes()).unwrap_or_else(|| panic!("decode {f}")));
    }
    (t, rec)
}

const F_ASSISTANT: &str = r#"{"sdkMessage":{"type":"assistant","message":{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}}}"#;
const F_TOOL_RUN: &str = r#"{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c1","name":"mcp","status":"running","args":{"providerIdentifier":"recur","toolName":"done","args":{"summary":"finished the work"}}}}}"#;
const F_TOOL_OK: &str = r#"{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c1","name":"mcp","status":"completed","args":{"providerIdentifier":"recur","toolName":"done"},"result":{"status":"success","value":{"isError":false}}}}}"#;
const F_TOOL_BAD: &str = r#"{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c2","name":"shell","status":"completed","args":{"command":"go test ./..."},"result":{"status":"success","value":{"isError":true}}}}}"#;
const F_THINKING: &str =
    r#"{"sdkMessage":{"type":"thinking","message":{"type":"thinking","text":"hmm"}}}"#;
pub(crate) const F_USAGE: &str = r#"{"sdkMessage":{"type":"usage","message":{"type":"usage","usage":{"inputTokens":1000,"outputTokens":500,"cacheReadTokens":2000,"cacheWriteTokens":0,"totalTokens":3500}}}}"#;
pub(crate) const F_RESULT_OK: &str = r#"{"result":{"status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-1","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"all good","durationMs":"1500","usage":{"inputTokens":"1000","outputTokens":"500","cacheReadTokens":"2000","totalTokens":"3500"}}}}"#;
const F_RESULT_ERR: &str = r#"{"result":{"status":"RUN_LIFECYCLE_STATUS_ERROR","result":{"runId":"run-1","status":"RUN_LIFECYCLE_STATUS_ERROR","error":{"message":"model overloaded"}}}}"#;
const F_RESULT_CANCELLED: &str = r#"{"result":{"status":"RUN_LIFECYCLE_STATUS_CANCELLED","result":{"runId":"run-1","status":"RUN_LIFECYCLE_STATUS_CANCELLED"}}}"#;
const F_TASK_SUMMARY: &str = r#"{"sdkMessage":{"type":"task","message":{"type":"task","text":"older turns folded into a short brief"}}}"#;

fn tool_frame_id(id: &str, name: &str, status: &str, args: &str, result: &str) -> String {
    let mut p = json!({"type": "tool_call", "call_id": id, "name": name, "status": status});
    if !args.is_empty() {
        p["args"] = serde_json::from_str(args).unwrap();
    }
    if !result.is_empty() {
        p["result"] = serde_json::from_str(result).unwrap();
    }
    json!({"sdkMessage": {"type": "tool_call", "message": p}}).to_string()
}

fn tool_frame(name: &str, status: &str, args: &str, result: &str) -> String {
    tool_frame_id("c1", name, status, args, result)
}

fn thinking_frame(text: &str, duration_ms: i64) -> String {
    let mut p = json!({"type": "thinking", "text": text});
    if duration_ms > 0 {
        p["thinking_duration_ms"] = duration_ms.into();
    }
    json!({"sdkMessage": {"type": "thinking", "message": p}}).to_string()
}

fn assistant_frame(text: &str) -> String {
    json!({"sdkMessage": {"type": "assistant", "message": {"type": "assistant", "message": {
        "role": "assistant", "content": [{"type": "text", "text": text}]
    }}}})
    .to_string()
}

fn steps(evs: &[AgentEvent]) -> Vec<&recur_agent::ToolStep> {
    evs.iter()
        .filter_map(|e| match e {
            AgentEvent::Step(s) => Some(s),
            _ => None,
        })
        .collect()
}

fn results(evs: &[AgentEvent]) -> Vec<&recur_agent::ToolResultEvent> {
    evs.iter()
        .filter_map(|e| match e {
            AgentEvent::ToolResult(r) => Some(r),
            _ => None,
        })
        .collect()
}

fn texts(evs: &[AgentEvent], kind: &str) -> Vec<String> {
    evs.iter()
        .filter(|e| e.kind() == kind)
        .filter_map(|e| match e {
            AgentEvent::Text { text, .. }
            | AgentEvent::TextDelta { text, .. }
            | AgentEvent::Thinking { text, .. }
            | AgentEvent::ThinkingDelta { text, .. }
            | AgentEvent::Notice { text }
            | AgentEvent::User { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn turn_ends(evs: &[AgentEvent]) -> Vec<&recur_agent::TurnResult> {
    evs.iter()
        .filter_map(|e| match e {
            AgentEvent::TurnEnd(r) => Some(r),
            _ => None,
        })
        .collect()
}

// ---- cursor

#[test]
fn task_message_is_not_compact() {
    // SDKTaskMessage is task status; summaries arrive on onDelta.
    let (_, rec) = feed(
        spec("composer-2.5", false),
        &[F_USAGE, F_TASK_SUMMARY, F_RESULT_OK],
    );
    assert!(!kinds(&rec.all()).contains(&"compact"));
}

fn compacts(evs: &[AgentEvent]) -> Vec<(String, String)> {
    evs.iter()
        .filter_map(|e| match e {
            AgentEvent::Compact {
                status, summary, ..
            } => Some((status.clone(), summary.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn interaction_summary_is_compact() {
    let (_, rec) = feed(
        spec("composer-2.5", false),
        &[
            F_USAGE,
            r#"{"interactionUpdate":{"type":"summary-started"}}"#,
            r#"{"interactionUpdate":{"type":"summary","update":{"type":"summary","summary":"older turns folded into a short brief"}}}"#,
            r#"{"interactionUpdate":{"type":"summary-completed"}}"#,
            F_RESULT_OK,
        ],
    );
    assert_eq!(
        compacts(&rec.all()),
        [
            ("started".to_owned(), String::new()),
            (
                "done".to_owned(),
                "older turns folded into a short brief".to_owned()
            )
        ]
    );
}

#[test]
fn second_compact_clears_prior_summary() {
    let (_, rec) = feed(
        spec("composer-2.5", false),
        &[
            F_USAGE,
            r#"{"interactionUpdate":{"type":"summary-started"}}"#,
            r#"{"interactionUpdate":{"type":"summary","update":{"type":"summary","summary":"first fold"}}}"#,
            r#"{"interactionUpdate":{"type":"summary-completed"}}"#,
            r#"{"interactionUpdate":{"type":"summary-started"}}"#,
            r#"{"interactionUpdate":{"type":"summary-completed"}}"#,
            F_RESULT_OK,
        ],
    );
    let done: Vec<String> = compacts(&rec.all())
        .into_iter()
        .filter(|(s, _)| s == "done")
        .map(|(_, t)| t)
        .collect();
    assert_eq!(done, ["first fold", ""]);
}

#[test]
fn collects_run_ids_for_cancel() {
    let (t, _) = feed(
        spec("", false),
        &[
            r#"{"sdkMessage":{"type":"status","message":{"type":"status","agent_id":"agent-1","run_id":"run-parent","status":"RUNNING"}}}"#,
            r#"{"sdkMessage":{"type":"status","message":{"type":"status","agent_id":"agent-1","run_id":"run-child","status":"RUNNING"}}}"#,
            F_RESULT_OK,
        ],
    );
    assert_eq!(t.all_run_ids(), ["run-1", "run-child", "run-parent"]);
    assert_eq!(t.parent_run(), "run-parent");
}

fn bare_turn() -> Arc<Turn> {
    Arc::new(Turn::new(spec("", false), None, "", false))
}

#[tokio::test]
async fn wait_run_id() {
    let t = bare_turn();
    let t2 = t.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        t2.note_run_id("run-late");
    });
    assert_eq!(
        t.wait_run_id(&CancellationToken::new(), Duration::from_secs(1))
            .await,
        "run-late"
    );
}

#[tokio::test]
async fn wait_run_id_and_prepare_follow_up_no_race() {
    let t = bare_turn();
    let t2 = t.clone();
    let w = tokio::spawn(async move {
        t2.wait_run_id(&CancellationToken::new(), Duration::from_millis(100))
            .await
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    t.prepare_steer_follow_up();
    t.note_run_id("run-follow");
    assert_eq!(w.await.unwrap(), "run-follow");
}

#[tokio::test]
async fn wait_run_id_after_prepare_follow_up() {
    let t = bare_turn();
    t.note_run_id("run-old");
    t.prepare_steer_follow_up();
    let t2 = t.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        t2.note_run_id("run-follow");
    });
    assert_eq!(
        t.wait_run_id(&CancellationToken::new(), Duration::from_secs(1))
            .await,
        "run-follow"
    );
}

#[tokio::test]
async fn wait_run_id_times_out_and_cancels() {
    let t = bare_turn();
    assert_eq!(
        t.wait_run_id(&CancellationToken::new(), Duration::from_millis(10))
            .await,
        ""
    );
    let c = CancellationToken::new();
    c.cancel();
    assert_eq!(t.wait_run_id(&c, Duration::from_secs(5)).await, "");
}

#[test]
fn emits_step_and_tool_result() {
    let (_, rec) = feed(spec("", false), &[F_TOOL_RUN, F_TOOL_OK]);
    let evs = rec.all();
    assert_eq!(kinds(&evs), ["step", "tool_result"]);
    let s = steps(&evs)[0];
    // Raw on the event; common at the display boundary.
    assert_eq!(s.name, "mcp");
    assert_eq!(
        crate::tools::display_tool(&s.name, &s.input).name,
        "mcp__recur__done"
    );
    assert_eq!(s.summary, "finished the work");
    assert_eq!(s.id, "c1");
    assert!(!results(&evs)[0].is_error);
}

#[test]
fn flags_failed_tool() {
    // A lone terminal frame yields the call AND its result.
    let (_, rec) = feed(spec("", false), &[F_TOOL_BAD]);
    let evs = rec.all();
    assert_eq!(kinds(&evs), ["step", "tool_result"]);
    let r = results(&evs)[0];
    assert!(r.is_error, "isError under a protocol success");
    assert_eq!(r.name, "shell");
    assert_eq!(
        crate::tools::display_tool(&steps(&evs)[0].name, "").name,
        "Bash"
    );
}

#[test]
fn emits_assistant_text() {
    let (t, rec) = feed(spec("", false), &[F_ASSISTANT]);
    t.finish();
    assert_eq!(texts(&rec.all(), "text"), ["done"]);
}

#[test]
fn thinking_only_with_partials() {
    assert!(feed(spec("", false), &[F_THINKING]).1.all().is_empty());
    assert_eq!(
        kinds(&feed(spec("", true), &[F_THINKING]).1.all()),
        ["thinking_delta"]
    );
}

#[test]
fn emits_mid_turn_usage() {
    let (_, rec) = feed(spec("", false), &[F_USAGE]);
    let evs = rec.all();
    let AgentEvent::Usage(u) = &evs[0] else {
        panic!("want usage, got {evs:?}");
    };
    assert_eq!((u.input_tokens, u.cache_read_tokens), (1000, 2000));
}

#[test]
fn result_success() {
    let (t, _) = feed(
        spec("composer-2.5", false),
        &[F_ASSISTANT, F_USAGE, F_RESULT_OK],
    );
    let r = t.result();
    assert!(!r.is_error && r.subtype == "success", "{r:?}");
    assert_eq!(r.text, "all good");
    assert_eq!(r.duration_ms, 1500, "string-encoded int64");
    assert_eq!(r.session_id, "agent-1", "the resume handle");
    assert_eq!(
        (
            r.usage.input_tokens,
            r.usage.output_tokens,
            r.usage.cache_read_tokens
        ),
        (1000, 500, 2000)
    );
}

#[test]
fn suppresses_cancelled_result_when_steering() {
    let rec = Rec::default();
    let t = Turn::new(spec("", false), Some(rec.sink()), "agent-1", false);
    t.set_suppress_result(true);
    t.on_event(decode_frame(F_RESULT_CANCELLED.as_bytes()).unwrap());
    assert!(
        turn_ends(&rec.all()).is_empty(),
        "CANCELLED emitted during a steer"
    );
    assert!(!t.have_result(), "the runner must be able to follow up");
}

#[test]
fn emits_finished_result_even_when_steer_pending() {
    let rec = Rec::default();
    let t = Turn::new(
        spec("composer-2.5", false),
        Some(rec.sink()),
        "agent-1",
        false,
    );
    t.set_suppress_result(true);
    t.on_event(decode_frame(F_RESULT_OK.as_bytes()).unwrap());
    assert_eq!(turn_ends(&rec.all()).len(), 1);
    assert!(t.have_result(), "a late steer must not follow up");
}

#[test]
fn result_error() {
    let (t, _) = feed(spec("", false), &[F_RESULT_ERR]);
    let r = t.result();
    assert!(r.is_error);
    assert_eq!(r.subtype, "error_during_execution");
    assert!(r.stderr.contains("model overloaded"));
}

/// The real bridge never puts `error` on the wire RunResult; the reason
/// arrives earlier as a status ERROR frame and the terminal frame is bare.
#[test]
fn status_error_fills_bare_result() {
    let (t, _) = feed(
        spec("", false),
        &[
            r#"{"sdkMessage":{"type":"status","message":{"type":"status","agent_id":"agent-1","run_id":"run-1","status":"ERROR","message":"[resource_exhausted] Error"}}}"#,
            r#"{"result":{"status":"RUN_LIFECYCLE_STATUS_ERROR","result":{"runId":"run-1","status":"RUN_LIFECYCLE_STATUS_ERROR"}}}"#,
        ],
    );
    let r = t.result();
    assert!(r.is_error && r.subtype == "error_during_execution");
    assert!(
        r.stderr.starts_with("[resource_exhausted] Error. ") && r.stderr.contains("rate-limited"),
        "{}",
        r.stderr
    );
    let (reason, transient) = recur_agent::error::classify(None, &r);
    assert!(transient);
    assert_eq!(reason, r.stderr);
}

#[test]
fn missing_result_is_error() {
    let (t, _) = feed(spec("", false), &[F_ASSISTANT]);
    t.finish();
    let r = t.result();
    assert!(r.is_error && r.subtype == "error_during_execution");
    assert_eq!(r.text, "done", "text before the stream died is kept");
}

#[test]
fn uses_mid_stream_usage_when_result_missing() {
    let (t, _) = feed(spec("composer-2.5", false), &[F_USAGE]);
    let r = t.result();
    assert!(r.usage.input_tokens != 0 || r.usage.cache_read_tokens != 0);
}

#[test]
fn prepare_follow_up_resets_per_run_state() {
    let (t, rec) = feed(spec("composer-2.5", false), &[F_TOOL_RUN]);
    {
        let mut f = t.fold.lock();
        f.tool_waves = 6;
        f.wave_open = true;
        f.compact_baseline = Some(TokenUsage {
            input_tokens: 9000,
            ..Default::default()
        });
        f.compact_pre_fill = 42;
        f.compact_summary = "old".into();
        f.usage = Some(TokenUsage {
            input_tokens: 1000,
            ..Default::default()
        });
        f.usage_cumulative = f.usage;
        f.have_result = true;
        f.res = RunResult {
            result: "partial".into(),
            ..Default::default()
        };
    }
    t.note_run_id("run-old");
    assert_eq!(t.model_rounds(), 7);

    t.prepare_steer_follow_up();

    {
        let f = t.fold.lock();
        assert!(f.tool_waves == 0 && !f.wave_open);
        assert!(
            f.compact_baseline.is_none() && f.compact_pre_fill == 0 && f.compact_summary.is_empty()
        );
        assert!(f.stepped_calls.is_none() && f.create_plan_args.is_none());
        assert!(f.open_calls.is_empty() && f.open_tool.is_none());
        assert!(f.usage.is_none() && f.usage_cumulative.is_none() && !f.have_result);
    }
    assert_eq!(
        t.model_rounds(),
        1,
        "the follow-up must not inherit run 1 waves"
    );
    assert!(t.all_run_ids().is_empty());

    // The same call id on the follow-up run mints a new Step.
    let before = rec.all().len();
    t.on_event(decode_frame(F_TOOL_RUN.as_bytes()).unwrap());
    assert!(steps(&rec.all()[before..]).iter().any(|s| s.id == "c1"));
}

#[test]
fn prepare_follow_up_keeps_suppress_result() {
    let t = bare_turn();
    t.set_suppress_result(true);
    t.prepare_steer_follow_up();
    assert!(
        t.suppressing_result(),
        "a steer in the take/follow-up window would emit error_cancelled"
    );
}

#[test]
fn prepare_follow_up_banks_duration_without_cancelled_frame() {
    let t = Turn::new(spec("composer-2.5", false), None, "", false);
    {
        let mut f = t.fold.lock();
        f.usage = Some(TokenUsage {
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_tokens: 2000,
            ..Default::default()
        });
        f.res.duration_ms = 1500;
    }
    t.prepare_steer_follow_up();
    assert!(t.fold.lock().usage.is_none());
    assert_eq!(t.fold.lock().prior_duration_ms, 1500);
    assert_eq!(t.result().duration_ms, 1500);
}

#[test]
fn bank_suppressed_duration_does_not_double_count() {
    let t = Turn::new(spec("composer-2.5", false), None, "", false);
    {
        let mut f = t.fold.lock();
        f.usage = Some(TokenUsage {
            input_tokens: 1000,
            ..Default::default()
        });
        f.res.duration_ms = 1500;
        f.bank_suppressed_duration();
        assert_eq!(f.prior_duration_ms, 1500);
        assert!(f.usage.is_none());
        f.res = RunResult::default();
        f.bank_suppressed_duration();
        assert_eq!(f.prior_duration_ms, 1500);
    }
    assert_eq!(t.result().duration_ms, 1500);
}

#[test]
fn does_not_suppress_provider_error() {
    let rec = Rec::default();
    let t = Turn::new(spec("", false), Some(rec.sink()), "agent-1", false);
    t.set_suppress_result(true);
    t.on_event(decode_frame(F_RESULT_ERR.as_bytes()).unwrap());
    let evs = rec.all();
    let ends = turn_ends(&evs);
    assert_eq!(ends.len(), 1);
    assert!(ends[0].stderr.contains("model overloaded"));
    assert!(t.have_result(), "ERROR must surface, not start another run");
}

#[test]
fn prepare_follow_up_closes_open_calls_as_cancelled() {
    let (t, rec) = feed(spec("", false), &[F_TOOL_RUN]);
    t.prepare_steer_follow_up();
    let evs = rec.all();
    let r = results(&evs);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].id, "c1");
    assert!(!r[0].is_error, "cancelled is not a failure");
    assert_eq!(r[0].text, STEER_CANCELLED_CALL_TEXT);
}

#[test]
fn deferred_result_waits_for_finish() {
    let rec = Rec::default();
    let t = Turn::new(spec("", false), Some(rec.sink()), "agent-1", true);
    t.on_event(decode_frame(F_RESULT_OK.as_bytes()).unwrap());
    assert!(turn_ends(&rec.all()).is_empty());
    t.fire_users(
        vec![AgentEvent::User {
            text: "late".into(),
            display: String::new(),
            live_steer: true,
        }],
        true,
    );
    t.finish();
    assert_eq!(kinds(&rec.all()), ["user", "result"]);
    t.finish();
    assert_eq!(turn_ends(&rec.all()).len(), 1, "held once");
}

// ---- cursor steer: user rows

#[test]
fn fire_users_uses_display_text() {
    let rec = Rec::default();
    let t = Turn::new(spec("", false), Some(rec.sink()), "", false);
    let steer = recur_agent::Steer::new("WIRE", Some("Fix findings".into())).0;
    t.fire_users(
        follow_up_users(std::slice::from_ref(&steer), "WIRE", true),
        true,
    );
    assert_eq!(
        rec.all(),
        [AgentEvent::User {
            text: "WIRE".into(),
            display: "Fix findings".into(),
            live_steer: true
        }]
    );
}

#[test]
fn live_fire_flushes_text() {
    let rec = Rec::default();
    let t = Turn::new(spec("", false), Some(rec.sink()), "", false);
    {
        let mut f = t.fold.lock();
        f.thinking.push_str("thought");
        f.text.push_str("chunk A");
    }
    let steer = recur_agent::Steer::new("steer now", None).0;
    t.fire_users(
        follow_up_users(std::slice::from_ref(&steer), "steer now", true),
        true,
    );
    assert!(t.fold.lock().thinking.is_empty() && t.fold.lock().text.is_empty());
    let evs = rec.all();
    assert_eq!(kinds(&evs), ["thinking", "text", "user"]);
    assert_eq!(texts(&evs, "thinking"), ["thought"]);
    assert_eq!(texts(&evs, "text"), ["chunk A"]);
    assert!(
        matches!(&evs[2], AgentEvent::User { live_steer: true, text, .. } if text == "steer now")
    );
}

// ---- cursortools: frames

#[test]
fn emits_raw_provider_tool_data() {
    let (_, rec) = feed(
        spec("", false),
        &[
            &tool_frame("shell", "running", ARGS_SHELL, ""),
            &tool_frame("shell", "completed", ARGS_SHELL, RES_SHELL),
        ],
    );
    let evs = rec.all();
    assert_eq!(evs.len(), 2);
    let s = steps(&evs)[0];
    assert_eq!(s.name, "shell");
    assert!(
        s.input.contains(r#""timeout""#),
        "verbatim args: {}",
        s.input
    );
    assert!(
        results(&evs)[0].text.contains(r#""status""#),
        "verbatim envelope"
    );
    // The feed's one-line summary is a display concern: already common.
    assert_eq!(s.summary, "echo hi");
}

#[test]
fn mcp_tool_frames() {
    let args = r#"{"providerIdentifier":"recur","toolName":"done","args":{"summary":"all done"}}"#;
    let (_, rec) = feed(
        spec("", false),
        &[
            &tool_frame("mcp", "running", args, ""),
            &tool_frame("mcp", "completed", args, RES_MCP),
        ],
    );
    let evs = rec.all();
    let s = steps(&evs)[0];
    assert_eq!(s.name, "mcp");
    assert!(s.input.contains("providerIdentifier"));
    let d = crate::tools::display_tool(&s.name, &s.input);
    assert_eq!(d.name, "mcp__recur__done");
    assert!(d.input.contains(r#""summary""#) && !d.input.contains("providerIdentifier"));
}

#[test]
fn thinking_becomes_one_block() {
    let (_, rec) = feed(
        spec("", false),
        &[
            &thinking_frame("I should ", 0),
            &thinking_frame("read the file.", 0),
            &thinking_frame("", 42),
        ],
    );
    assert_eq!(texts(&rec.all(), "thinking"), ["I should read the file."]);
}

#[test]
fn thinking_flushes_before_tool_call() {
    let (_, rec) = feed(
        spec("", false),
        &[
            &thinking_frame("let me look", 0),
            &tool_frame("read", "running", ARGS_READ, ""),
        ],
    );
    let evs = rec.all();
    assert_eq!(kinds(&evs)[..2], ["thinking", "step"]);
    assert_eq!(steps(&evs)[0].name, "read");
    assert_eq!(crate::tools::display_tool("read", ARGS_READ).name, "Read");
}

#[test]
fn thinking_flushed_on_result() {
    let (_, rec) = feed(
        spec("", false),
        &[&thinking_frame("half a thought", 0), F_RESULT_OK],
    );
    assert_eq!(texts(&rec.all(), "thinking"), ["half a thought"]);
}

#[test]
fn thinking_deltas_only_with_partials() {
    let frames = [thinking_frame("hmm", 0), thinking_frame("", 1)];
    let frames: Vec<&str> = frames.iter().map(String::as_str).collect();
    let quiet = feed(spec("", false), &frames).1.all();
    assert!(
        !kinds(&quiet)
            .iter()
            .any(|k| *k == "text_delta" || *k == "thinking_delta")
    );
    let chat = feed(spec("", true), &frames).1.all();
    assert!(kinds(&chat).contains(&"thinking_delta"));
    assert!(
        !kinds(&chat).contains(&"text_delta"),
        "reasoning must not stream as reply text"
    );
    assert_eq!(
        texts(
            &feed(spec("", true), &[&thinking_frame("weighing options", 0)])
                .1
                .all(),
            "thinking_delta"
        ),
        ["weighing options"]
    );
}

#[test]
fn repeated_running_yields_one_step() {
    let (_, rec) = feed(
        spec("", false),
        &[
            &tool_frame("edit", "running", ARGS_EDIT, ""),
            &tool_frame("edit", "running", ARGS_EDIT, ""),
            &tool_frame("edit", "completed", ARGS_EDIT, RES_EDIT),
        ],
    );
    let evs = rec.all();
    assert_eq!((steps(&evs).len(), results(&evs).len()), (1, 1));
}

const GROK_IDS: [&str; 3] = [
    "call-9812a46f-b258-4ed8-8d09-55441deb1733-5\nfc_acab39d7-9d73-9fd7-9c87-51495bec8eb8_0",
    "call-9812a46f-b258-4ed8-8d09-55441deb1733-6\nfc_acab39d7-9d73-9fd7-9c87-51495bec8eb8_1",
    "call-9812a46f-b258-4ed8-8d09-55441deb1733-7\nfc_acab39d7-9d73-9fd7-9c87-51495bec8eb8_2",
];

#[test]
fn grok46_newline_id_repeated_running_yields_one_step() {
    let args = r#"{"globPattern":"TODO.md"}"#;
    let id = GROK_IDS[0];
    let frames = [
        tool_frame_id(id, "glob", "running", args, ""),
        tool_frame_id(id, "glob", "running", args, ""),
        tool_frame_id(id, "glob", "running", args, ""),
        tool_frame_id(id, "glob", "completed", args, RES_GLOB),
    ];
    let frames: Vec<&str> = frames.iter().map(String::as_str).collect();
    let evs = feed(spec("", false), &frames).1.all();
    let s = steps(&evs);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].id, id);
}

#[test]
fn grok46_parallel_globs_each_get_a_step() {
    let args = [
        r#"{"globPattern":"TODO.md"}"#,
        r#"{"globPattern":"README*"}"#,
        r#"{"globPattern":"Makefile"}"#,
    ];
    let mut frames: Vec<String> = GROK_IDS
        .iter()
        .zip(args)
        .map(|(id, a)| tool_frame_id(id, "glob", "running", a, ""))
        .collect();
    frames.extend(
        GROK_IDS
            .iter()
            .zip(args)
            .map(|(id, a)| tool_frame_id(id, "glob", "completed", a, RES_GLOB)),
    );
    let frames: Vec<&str> = frames.iter().map(String::as_str).collect();
    let evs = feed(spec("", false), &frames).1.all();
    let ids: std::collections::HashSet<&str> = steps(&evs).iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids.len(), 3);
}

#[test]
fn parallel_calls_each_get_a_step() {
    let mut frames: Vec<String> = ["a", "b", "c"]
        .iter()
        .map(|id| tool_frame_id(id, "read", "running", ARGS_READ, ""))
        .collect();
    frames.extend(
        ["a", "b", "c"]
            .iter()
            .map(|id| tool_frame_id(id, "read", "completed", ARGS_READ, RES_READ)),
    );
    let frames: Vec<&str> = frames.iter().map(String::as_str).collect();
    let evs = feed(spec("", false), &frames).1.all();
    assert_eq!((steps(&evs).len(), results(&evs).len()), (3, 3));
}

#[test]
fn unknown_terminal_status_still_produces_result() {
    for status in ["failed", "aborted", "rejected", "error", "cancelled"] {
        let evs = feed(
            spec("", false),
            &[
                &tool_frame("read", "running", ARGS_READ, ""),
                &tool_frame("read", status, ARGS_READ, RES_READ),
            ],
        )
        .1
        .all();
        let r = results(&evs);
        assert_eq!(r.len(), 1, "{status}");
        assert!(r[0].is_error, "{status} not flagged as an error");
    }
}

#[test]
fn terminal_without_running_still_emits_step() {
    let evs = feed(
        spec("", false),
        &[&tool_frame("read", "completed", ARGS_READ, RES_READ)],
    )
    .1
    .all();
    assert_eq!(kinds(&evs), ["step", "tool_result"]);
    assert_eq!(steps(&evs)[0].id, results(&evs)[0].id);
}

#[test]
fn completed_is_not_an_error() {
    let evs = feed(
        spec("", false),
        &[
            &tool_frame("read", "running", ARGS_READ, ""),
            &tool_frame("read", "completed", ARGS_READ, RES_READ),
        ],
    )
    .1
    .all();
    assert!(results(&evs).iter().all(|r| !r.is_error));
}

#[test]
fn assistant_chunks_become_one_block() {
    // Chunk boundaries VERBATIM from a real session: a fence opened in one
    // frame and closed in the next.
    let frames = [
        assistant_frame("CLI Tetris is ready. Run it with:\n\n```bash\ngo"),
        assistant_frame(" run .\n```\n\n**Controls:** arrows move"),
        assistant_frame(" · Q quit\n\nTests pass."),
    ];
    let mut f: Vec<&str> = frames.iter().map(String::as_str).collect();
    f.push(F_RESULT_OK);
    let evs = feed(spec("", false), &f).1.all();
    let blocks = texts(&evs, "text");
    assert_eq!(
        blocks,
        [
            "CLI Tetris is ready. Run it with:\n\n```bash\ngo run .\n```\n\n**Controls:** arrows move · Q quit\n\nTests pass."
        ]
    );
    assert_eq!(blocks[0].matches("```").count(), 2);
    assert_eq!(texts(&evs, "text_delta").len(), 3, "one delta per chunk");
}

#[test]
fn result_text_is_the_whole_reply() {
    let frames = [
        assistant_frame("The"),
        assistant_frame(" O piece has empty left columns."),
    ];
    let (t, _) = feed(spec("", false), &[&frames[0], &frames[1]]);
    t.finish();
    assert_eq!(
        t.fold.lock().last_text,
        "The O piece has empty left columns."
    );
}

#[test]
fn text_flushes_around_tool_calls() {
    let evs = feed(
        spec("", false),
        &[
            &assistant_frame("Reading the file"),
            &assistant_frame(" first."),
            &tool_frame("read", "running", ARGS_READ, ""),
            &tool_frame("read", "completed", ARGS_READ, RES_READ),
            &assistant_frame("Done."),
            F_RESULT_OK,
        ],
    )
    .1
    .all();
    assert_eq!(texts(&evs, "text"), ["Reading the file first.", "Done."]);
    let order: Vec<&str> = kinds(&evs)
        .into_iter()
        .filter(|k| *k != "text_delta")
        .collect();
    assert_eq!(order, ["text", "step", "tool_result", "text", "result"]);
}

#[test]
fn text_and_thinking_interleave_in_order() {
    let evs = feed(
        spec("", false),
        &[
            &assistant_frame("Let me check."),
            &thinking_frame("the O piece", 0),
            &thinking_frame(" has empty columns", 0),
            &assistant_frame("Fixing it."),
            F_RESULT_OK,
        ],
    )
    .1
    .all();
    let seq: Vec<String> = evs
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Text { text, .. } => Some(format!("text:{text}")),
            AgentEvent::Thinking { text, .. } => Some(format!("think:{text}")),
            _ => None,
        })
        .collect();
    assert_eq!(
        seq,
        [
            "text:Let me check.",
            "think:the O piece has empty columns",
            "text:Fixing it."
        ]
    );
}

#[test]
fn create_plan_waits_for_terminal_args() {
    let evs = feed(
        spec("", false),
        &[
            &tool_frame("createPlan", "running", ARGS_PLAN_EMPTY, ""),
            &tool_frame(
                "createPlan",
                "running",
                r#"{"plan":"Summary: only the opening line"}"#,
                "",
            ),
            &tool_frame("createPlan", "completed", ARGS_PLAN, RES_PLAN),
        ],
    )
    .1
    .all();
    let s = steps(&evs);
    assert_eq!(s.len(), 1);
    let body = recur_agent::display::create_plan_body(&s[0].input);
    assert!(body.contains("touch work.txt"), "{}", s[0].input);
    assert_eq!(
        crate::tools::display_tool(&s[0].name, &s[0].input).name,
        "CreatePlan"
    );
    // The plan-mode harvest reads the proposal, not the tool name.
    assert!(
        evs.iter()
            .any(|e| matches!(e, AgentEvent::PlanProposal { body: b } if b == &body))
    );
    assert_eq!(kinds(&evs), ["step", "plan_proposal", "tool_result"]);
}

#[test]
fn create_plan_keeps_richest_args() {
    let evs = feed(
        spec("", false),
        &[
            &tool_frame("createPlan", "running", ARGS_PLAN, ""),
            &tool_frame(
                "createPlan",
                "completed",
                r#"{"plan":"Summary: stub"}"#,
                RES_PLAN,
            ),
        ],
    )
    .1
    .all();
    assert!(
        recur_agent::display::create_plan_body(&steps(&evs)[0].input).contains("touch work.txt")
    );
}

#[test]
fn empty_create_plan_proposes_nothing() {
    let evs = feed(
        spec("", false),
        &[&tool_frame(
            "createPlan",
            "completed",
            ARGS_PLAN_EMPTY,
            RES_PLAN,
        )],
    )
    .1
    .all();
    assert!(!kinds(&evs).contains(&"plan_proposal"));
}

#[test]
fn unreported_call_is_closed_as_error() {
    let (t, rec) = feed(
        spec("", false),
        &[&tool_frame("read", "running", ARGS_READ, ""), F_RESULT_OK],
    );
    t.finish();
    let evs = rec.all();
    let r = results(&evs);
    assert_eq!(r.len(), 1);
    assert!(r[0].is_error);
    assert_eq!(r[0].id, "c1");
    assert_eq!(r[0].text, UNREPORTED_RESULT_TEXT);
    assert_eq!(r[0].name, "read");
}

#[test]
fn unreported_call_closes_before_final_text() {
    let (t, rec) = feed(
        spec("", false),
        &[
            &tool_frame("read", "running", ARGS_READ, ""),
            &assistant_frame("Those files do not exist."),
            F_RESULT_OK,
        ],
    );
    t.finish();
    let order: Vec<&str> = kinds(&rec.all())
        .into_iter()
        .filter(|k| *k == "tool_result" || *k == "text")
        .collect();
    assert_eq!(order, ["tool_result", "text"]);
}

#[test]
fn resolved_call_is_not_double_closed() {
    let (t, rec) = feed(
        spec("", false),
        &[
            &tool_frame("read", "running", ARGS_READ, ""),
            &tool_frame("read", "completed", ARGS_READ, RES_READ),
            F_RESULT_OK,
        ],
    );
    t.finish();
    assert_eq!(results(&rec.all()).len(), 1);
}

#[test]
fn all_unreported_calls_are_closed_in_order() {
    let frames: Vec<String> = ["a", "b", "c"]
        .iter()
        .map(|id| tool_frame_id(id, "read", "running", ARGS_READ, ""))
        .collect();
    let mut f: Vec<&str> = frames.iter().map(String::as_str).collect();
    f.push(F_RESULT_OK);
    let (t, rec) = feed(spec("", false), &f);
    t.finish();
    let evs = rec.all();
    let ids: Vec<&str> = results(&evs).iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["a", "b", "c"]);
}

#[test]
fn unreported_call_closed_on_finish() {
    let (t, rec) = feed(
        spec("", false),
        &[&tool_frame("read", "running", ARGS_READ, "")],
    );
    t.finish();
    assert_eq!(results(&rec.all()).iter().filter(|r| r.is_error).count(), 1);
}

#[test]
fn error_status_is_terminal_failure() {
    let evs = feed(
        spec("", false),
        &[
            &tool_frame("read", "running", ARGS_READ, ""),
            &tool_frame(
                "read",
                "error",
                ARGS_READ,
                r#"{"status":"error","value":{"message":"ENOENT"}}"#,
            ),
        ],
    )
    .1
    .all();
    let r = results(&evs);
    assert!(r[0].is_error);
    assert!(
        r[0].text.contains("ENOENT"),
        "the provider's payload is kept verbatim"
    );
}

#[test]
fn error_status_resolves_the_call() {
    let (t, rec) = feed(
        spec("", false),
        &[
            &tool_frame("read", "running", ARGS_READ, ""),
            &tool_frame(
                "read",
                "error",
                ARGS_READ,
                r#"{"status":"error","value":{}}"#,
            ),
            F_RESULT_OK,
        ],
    );
    t.finish();
    assert_eq!(results(&rec.all()).len(), 1);
}

#[test]
fn truncated_result_is_flagged() {
    let frame = |trunc: Option<&str>| {
        let mut p = json!({
            "type": "tool_call", "call_id": "c1", "name": "read", "status": "completed",
            "args": serde_json::from_str::<serde_json::Value>(ARGS_READ).unwrap(),
            "result": serde_json::from_str::<serde_json::Value>(RES_READ).unwrap(),
        });
        if let Some(t) = trunc {
            p["truncated"] = serde_json::from_str(t).unwrap();
        }
        json!({"sdkMessage": {"type": "tool_call", "message": p}}).to_string()
    };
    let clipped = feed(spec("", false), &[&frame(Some(r#"{"result":true}"#))])
        .1
        .all();
    let r = results(&clipped)[0];
    assert!(
        r.truncated && !r.is_error,
        "truncation is flagged and is not a failure"
    );
    let whole = feed(spec("", false), &[&frame(None)]).1.all();
    assert!(!results(&whole)[0].truncated);
}

#[test]
fn unreported_call_closes_at_next_thinking() {
    let evs = feed(
        spec("", false),
        &[
            &tool_frame("read", "running", ARGS_READ, ""),
            &thinking_frame("go.mod does not exist.", 0),
        ],
    )
    .1
    .all();
    let idx = evs
        .iter()
        .position(|e| e.kind() == "tool_result")
        .expect("closed at the next thinking frame");
    assert!(results(&evs)[0].is_error);
    assert!(!evs[..idx].iter().any(|e| e.kind() == "thinking"));
}

#[test]
fn unreported_call_closes_at_next_reply() {
    let evs = feed(
        spec("", false),
        &[
            &tool_frame("read", "running", ARGS_READ, ""),
            &assistant_frame("That file does not exist."),
        ],
    )
    .1
    .all();
    assert_eq!(results(&evs).iter().filter(|r| r.is_error).count(), 1);
}

#[test]
fn reported_call_survives_next_thinking() {
    let (t, rec) = feed(
        spec("", false),
        &[
            &tool_frame("read", "running", ARGS_READ, ""),
            &tool_frame("read", "completed", ARGS_READ, RES_READ),
            &thinking_frame("now I know the contents", 0),
            F_RESULT_OK,
        ],
    );
    t.finish();
    let evs = rec.all();
    let r = results(&evs);
    assert_eq!(r.len(), 1);
    assert!(!r[0].is_error && r[0].text.contains("alpha"));
}

#[test]
fn closing_does_not_affect_later_calls() {
    let (t, rec) = feed(
        spec("", false),
        &[
            &tool_frame_id("a", "read", "running", ARGS_READ, ""),
            &thinking_frame("that one failed", 0),
            &tool_frame_id("b", "read", "running", ARGS_READ, ""),
            &tool_frame_id("b", "read", "completed", ARGS_READ, RES_READ),
            F_RESULT_OK,
        ],
    );
    t.finish();
    let evs = rec.all();
    let r = results(&evs);
    assert_eq!(r.len(), 2);
    assert!(r.iter().find(|x| x.id == "a").unwrap().is_error);
    assert!(!r.iter().find(|x| x.id == "b").unwrap().is_error);
}

#[test]
fn result_carries_duration_and_emits_turn_end() {
    let (t, rec) = feed(spec("", false), &[F_RESULT_OK]);
    assert_eq!(t.result().duration_ms, 1500);
    let evs = rec.all();
    let ends = turn_ends(&evs);
    assert_eq!(ends.len(), 1);
    assert_eq!(
        (ends[0].duration_ms, ends[0].text.as_str()),
        (1500, "all good")
    );
}

#[test]
fn expand_and_subtype() {
    assert_eq!(
        expand_cursor_run_error("[resource_exhausted] Error"),
        "[resource_exhausted] Error. Cursor's model provider is rate-limited or at capacity. Wait a moment and retry."
    );
    assert_eq!(expand_cursor_run_error("plain"), "plain");
    assert_eq!(
        cursor_subtype(&RunResult::default(), false),
        "error_during_execution"
    );
    let r = |s: &str| RunResult {
        status: s.into(),
        ..Default::default()
    };
    assert_eq!(cursor_subtype(&r(RUN_FINISHED), true), "success");
    assert_eq!(cursor_subtype(&r(RUN_CANCELLED), true), "error_cancelled");
    assert_eq!(
        cursor_subtype(&r(RUN_ERROR), true),
        "error_during_execution"
    );
    assert_eq!(
        cursor_subtype(&r("RUN_LIFECYCLE_STATUS_EXPIRED"), true),
        "success"
    );
}

// ---- the composer meter from Cursor usage frames.

fn tool_wave(id: &str, name: &str, args: &str) -> [String; 2] {
    [
        format!(
            r#"{{"sdkMessage":{{"type":"tool_call","message":{{"type":"tool_call","call_id":"{id}","name":"{name}","status":"running","args":{args}}}}}}}"#
        ),
        format!(
            r#"{{"sdkMessage":{{"type":"tool_call","message":{{"type":"tool_call","call_id":"{id}","name":"{name}","status":"completed","args":{args},"result":{{"status":"success","value":{{"isError":false}}}}}}}}}}"#
        ),
    ]
}

const M_THINK: &str =
    r#"{"sdkMessage":{"type":"thinking","message":{"type":"thinking","text":"next"}}}"#;
const M_THINK_END: &str =
    r#"{"sdkMessage":{"type":"thinking","message":{"type":"thinking","text":""}}}"#;
const M_CUMUL_300K: &str = r#"{"sdkMessage":{"type":"usage","message":{"type":"usage","usage":{"inputTokens":150000,"outputTokens":900,"cacheReadTokens":150000,"cacheWriteTokens":0,"totalTokens":300900}}}}"#;
const M_FIN_300K: &str = r#"{"result":{"status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-1","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"ok","durationMs":"900","usage":{"inputTokens":"150000","outputTokens":"900","cacheReadTokens":"150000","totalTokens":"300900"}}}}"#;
const M_ENDED_20K: &str = r#"{"interactionUpdate":{"type":"turn-ended","usage":{"inputTokens":12000,"outputTokens":400,"cacheReadTokens":8000,"cacheWriteTokens":0}}}"#;
const M_PRE: &str = r#"{"sdkMessage":{"type":"usage","message":{"type":"usage","usage":{"inputTokens":150000,"outputTokens":200,"cacheReadTokens":150000,"cacheWriteTokens":0,"totalTokens":300200}}}}"#;
const M_SUMMARY_START: &str = r#"{"interactionUpdate":{"type":"summary-started"}}"#;
const M_SUMMARY_DONE: &str = r#"{"interactionUpdate":{"type":"summary-completed"}}"#;
const M_POST: &str = r#"{"sdkMessage":{"type":"usage","message":{"type":"usage","usage":{"inputTokens":165000,"outputTokens":260,"cacheReadTokens":165000,"cacheWriteTokens":0,"totalTokens":330260}}}}"#;
const M_FIN_POST: &str = r#"{"result":{"status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-1","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"ok","durationMs":"900","usage":{"inputTokens":"165000","outputTokens":"260","cacheReadTokens":"165000","totalTokens":"330260"}}}}"#;

fn feed_owned(frames: &[String]) -> (Turn, Rec) {
    let refs: Vec<&str> = frames.iter().map(String::as_str).collect();
    feed(spec("composer-2.5", false), &refs)
}

fn frames(parts: &[&[String]]) -> Vec<String> {
    parts.iter().flat_map(|p| p.iter().cloned()).collect()
}

fn s(f: &str) -> Vec<String> {
    vec![f.to_string()]
}

fn compact_tokens(evs: &[AgentEvent]) -> Vec<(String, i64)> {
    evs.iter()
        .filter_map(|e| match e {
            AgentEvent::Compact {
                status, pre_tokens, ..
            } => Some((status.clone(), *pre_tokens)),
            _ => None,
        })
        .collect()
}

#[test]
fn cursor_result_prefers_mid_stream_usage_for_meter() {
    let mid = r#"{"sdkMessage":{"type":"usage","message":{"type":"usage","usage":{"inputTokens":3000,"outputTokens":100,"cacheReadTokens":70000,"cacheWriteTokens":0,"totalTokens":73100}}}}"#;
    let fin = r#"{"result":{"status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-1","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"ok","durationMs":"900","usage":{"inputTokens":"9000","outputTokens":"400","cacheReadTokens":"200000","totalTokens":"209400"}}}}"#;
    let (t, _) = feed(spec("composer-2.5", false), &[mid, fin]);
    let u = t.result().usage;
    assert_eq!((u.input_tokens, u.cache_read_tokens), (3000, 70000));
}

#[test]
fn cursor_meter_scales_cumulative_turn_usage() {
    let (t, _) = feed_owned(&frames(&[
        &tool_wave("c1", "shell", r#"{"command":"ls"}"#),
        &s(M_THINK),
        &s(M_THINK_END),
        &tool_wave("c2", "shell", r#"{"command":"pwd"}"#),
        &s(M_CUMUL_300K),
        &s(M_FIN_300K),
    ]));
    let u = t.result().usage;
    assert_eq!(u.context_used(), 150_000, "300000 * 2 / 4");
    assert_eq!((u.input_tokens, u.cache_read_tokens), (75_000, 75_000));
}

#[test]
fn cursor_meter_serial_sibling_tools_share_one_wave() {
    let (t, _) = feed_owned(&frames(&[
        &tool_wave("c1", "shell", r#"{"command":"ls"}"#),
        &tool_wave("c2", "shell", r#"{"command":"pwd"}"#),
        &s(M_CUMUL_300K),
        &s(M_FIN_300K),
    ]));
    assert_eq!(t.fold.lock().tool_waves, 1, "serial siblings share a wave");
    assert_eq!(
        t.result().usage.context_used(),
        200_000,
        "300000 * 2 / 3 clamped to the window"
    );
}

#[test]
fn cursor_meter_subtracts_usage_at_summary() {
    let (t, rec) = feed(
        spec("composer-2.5", false),
        &[M_PRE, M_SUMMARY_START, M_SUMMARY_DONE, M_POST, M_FIN_POST],
    );
    let c = compact_tokens(&rec.all());
    assert!(c.iter().any(|(s, _)| s == "started") && c.iter().any(|(s, _)| s == "done"));
    for (_, tokens) in &c {
        assert_eq!(*tokens, 200_000, "300k cumulative clamped to the window");
    }
    assert_eq!(
        t.result().usage.context_used(),
        30_000,
        "post-summary delta"
    );
}

#[test]
fn cursor_meter_scales_cumulative_usage_under_window() {
    let usage = r#"{"sdkMessage":{"type":"usage","message":{"type":"usage","usage":{"inputTokens":61155,"outputTokens":763,"cacheReadTokens":53344,"cacheWriteTokens":0,"totalTokens":115262}}}}"#;
    let fin = r#"{"result":{"status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-1","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"ok","durationMs":"7725","usage":{"inputTokens":"61155","outputTokens":"763","cacheReadTokens":"53344","totalTokens":"115262"}}}}"#;
    let (t, _) = feed_owned(&frames(&[
        &tool_wave("c1", "glob", r#"{"glob_pattern":"*"}"#),
        &s(M_THINK),
        &s(M_THINK_END),
        &tool_wave("c2", "read", r#"{"path":"main.go"}"#),
        &tool_wave("c3", "glob", r#"{"glob_pattern":"README*"}"#),
        &s(M_THINK),
        &s(M_THINK_END),
        &tool_wave("c4", "mcp", r#"{"toolName":"rename_workspace"}"#),
        &tool_wave("c5", "mcp", r#"{"toolName":"rename_branch"}"#),
        &tool_wave("c6", "mcp", r#"{"toolName":"rename_session"}"#),
        &s(usage),
        &s(fin),
    ]));
    assert_eq!(t.fold.lock().tool_waves, 3, "glob | read+glob | three mcp");
    let u = t.result().usage;
    assert_eq!(u.context_used(), 45_799, "114499 * 2 / 5");
    assert_eq!(u.input_tokens + u.cache_read_tokens, 45_799);
}

#[test]
fn cursor_turn_ended_footprint_not_divided_by_waves() {
    let cumul = r#"{"sdkMessage":{"type":"usage","message":{"type":"usage","usage":{"inputTokens":61155,"outputTokens":763,"cacheReadTokens":53344,"totalTokens":115262}}}}"#;
    let fin = r#"{"result":{"status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-1","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"ok","durationMs":"7725","usage":{"inputTokens":"61155","outputTokens":"763","cacheReadTokens":"53344","totalTokens":"115262"}}}}"#;
    let (t, _) = feed_owned(&frames(&[
        &tool_wave("c1", "glob", r#"{"glob_pattern":"*"}"#),
        &s(M_THINK),
        &s(M_THINK_END),
        &tool_wave("c2", "read", r#"{"path":"main.go"}"#),
        &s(cumul),
        &s(M_ENDED_20K),
        &s(fin),
    ]));
    assert_eq!(t.result().usage.context_used(), 20_000);
}

#[test]
fn cursor_turn_ended_usage_is_per_call_footprint() {
    let fin = r#"{"result":{"status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-1","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"ok","durationMs":"400","usage":{"inputTokens":"150000","outputTokens":"900","cacheReadTokens":"150000","totalTokens":"300900"}}}}"#;
    let (t, _) = feed(
        spec("composer-2.5", false),
        &[M_CUMUL_300K, M_ENDED_20K, fin],
    );
    assert_eq!(t.result().usage.context_used(), 20_000);
}

#[test]
fn cursor_compact_baseline_skips_per_call_footprint() {
    let (t, _) = feed(
        spec("composer-2.5", false),
        &[
            M_PRE,
            M_ENDED_20K,
            M_SUMMARY_START,
            M_SUMMARY_DONE,
            M_POST,
            M_FIN_POST,
        ],
    );
    assert_eq!(
        t.result().usage.context_used(),
        30_000,
        "cumulative minus cumulative, not the 20k footprint"
    );
}

#[test]
fn cursor_compact_clears_per_call_meter() {
    let fin = r#"{"result":{"status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-1","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"ok","durationMs":"900","usage":{"inputTokens":"150000","outputTokens":"200","cacheReadTokens":"150000","totalTokens":"300200"}}}}"#;
    let (t, rec) = feed(
        spec("composer-2.5", false),
        &[M_PRE, M_ENDED_20K, M_SUMMARY_START, M_SUMMARY_DONE, fin],
    );
    assert!(!t.fold.lock().usage_per_call);
    assert_eq!(t.result().usage.context_used(), 0);
    let evs = rec.all();
    let done = evs
        .iter()
        .position(|e| matches!(e, AgentEvent::Compact { status, .. } if status == "done"))
        .expect("compact done");
    assert!(
        !evs[done + 1..]
            .iter()
            .any(|e| matches!(e, AgentEvent::Usage(_))),
        "no usage after compact-done with a zero delta"
    );
}

#[test]
fn cursor_compact_flushes_text_first() {
    let (_, rec) = feed(spec("composer-2.5", false), &[F_ASSISTANT, M_SUMMARY_START]);
    let k: Vec<&str> = kinds(&rec.all())
        .into_iter()
        .filter(|k| matches!(*k, "text" | "compact"))
        .collect();
    assert_eq!(&k[..2], ["text", "compact"]);
}

#[test]
fn cursor_result_carries_duration() {
    let (t, _) = feed(spec("", false), &[F_RESULT_OK]);
    assert_eq!(t.result().duration_ms, 1500);
}
