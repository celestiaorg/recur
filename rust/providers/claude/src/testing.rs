//! Test doubles that replay Claude stream-json (Go `ScriptedRunner`).

use recur_agent::testing::ScriptedProvider;

use crate::stream::parse_line;

/// Replays raw stream-json lines through the real parser, one frame per
/// line (malformed lines yield nothing, as Go's runner skips them), so a
/// scripted test also exercises the parser end to end. Chain
/// [`ScriptedProvider`]'s builders for edits, `on_run` and `imitating`.
pub struct NdjsonScript;

impl NdjsonScript {
    #[allow(clippy::new_ret_no_self)]
    pub fn new<I>(lines: I) -> ScriptedProvider
    where
        I: IntoIterator,
        I::Item: AsRef<[u8]>,
    {
        ScriptedProvider::new(
            lines
                .into_iter()
                .map(|l| parse_line(l.as_ref()).unwrap_or_default())
                .collect(),
        )
    }
}

/// The canonical happy-path transcript (init, a Write tool_use, its
/// tool_result, a success result) engine tests replay (Go `fixtureLines`).
/// `cost` renders as Go's fixture did: 0.05, 0.10 and 0.25 verbatim, anything
/// else as 0.
pub fn fixture_lines(
    session_id: &str,
    file_path: &str,
    final_text: &str,
    cost: f64,
) -> Vec<String> {
    let cost = if cost == 0.05 {
        "0.05"
    } else if cost == 0.10 {
        "0.10"
    } else if cost == 0.25 {
        "0.25"
    } else {
        "0"
    };
    vec![
        format!(
            r#"{{"type":"system","subtype":"init","session_id":"{session_id}","model":"claude-sonnet-4-6","mcp_servers":[]}}"#
        ),
        format!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"t1","name":"Write","input":{{"file_path":"{file_path}"}}}}]}}}}"#
        ),
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":false}]}}"#.to_owned(),
        format!(
            r#"{{"type":"result","subtype":"success","session_id":"{session_id}","duration_ms":1000,"is_error":false,"num_turns":1,"result":"{final_text}","total_cost_usd":{cost}}}"#
        ),
    ]
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use recur_agent::testing::EventRecorder;
    use recur_agent::{AgentEvent, Provider, TurnContext, TurnRequest};
    use tokio_util::sync::CancellationToken;

    use super::*;

    async fn run(p: &ScriptedProvider) -> (recur_agent::TurnResult, Vec<AgentEvent>) {
        let rec = EventRecorder::default();
        let cx = TurnContext::new(CancellationToken::new(), Arc::new(rec.clone()));
        let res = p.run_turn(TurnRequest::default(), cx).await.unwrap();
        (res, rec.take())
    }

    #[tokio::test]
    async fn fixture_replays_through_the_parser() {
        let (res, evs) = run(&NdjsonScript::new(fixture_lines(
            "sess-1", "a.txt", "done", 0.05,
        )))
        .await;
        let kinds: Vec<_> = evs.iter().map(AgentEvent::kind).collect();
        assert_eq!(kinds, ["init", "step", "tool_result", "result"]);
        assert_eq!(
            (res.session_id.as_str(), res.text.as_str()),
            ("sess-1", "done")
        );
        assert!(fixture_lines("s", "f", "t", 0.10)[3].ends_with(r#""total_cost_usd":0.10}"#));
        assert!(fixture_lines("s", "f", "t", 0.3)[3].ends_with(r#""total_cost_usd":0}"#));
    }

    /// Go `TestLastAssistantUsageWinsOverCumulativeResult` (provider half):
    /// result usage sums every inference and can exceed the window; the
    /// meter stamps the last assistant footprint instead.
    #[tokio::test]
    async fn last_assistant_usage_wins_over_cumulative_result() {
        let p = NdjsonScript::new([
            r#"{"type":"system","subtype":"init","session_id":"sess-ctx","model":"claude-sonnet-5","mcp_servers":[]}"#,
            r#"{"type":"assistant","message":{"usage":{"input_tokens":50,"output_tokens":10,"cache_read_input_tokens":10000,"cache_creation_input_tokens":0},"content":[{"type":"text","text":"step1"},{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file_path":"a.go"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"ok"}]}}"#,
            r#"{"type":"assistant","message":{"usage":{"input_tokens":200,"output_tokens":40,"cache_read_input_tokens":90000,"cache_creation_input_tokens":0},"content":[{"type":"text","text":"done"}]}}"#,
            "{not json",
            r#"{"type":"result","subtype":"success","session_id":"sess-ctx","duration_ms":800,"is_error":false,"num_turns":2,"result":"done","total_cost_usd":0.12,"usage":{"input_tokens":250,"output_tokens":50,"cache_creation_input_tokens":0,"cache_read_input_tokens":100000},"modelUsage":{"claude-sonnet-5":{"contextWindow":1000000,"inputTokens":250,"outputTokens":50,"cacheReadInputTokens":100000,"cacheCreationInputTokens":0}}}"#,
        ]);
        let (res, evs) = run(&p).await;
        assert_eq!(
            (res.usage.input_tokens, res.usage.cache_read_tokens),
            (200, 90_000)
        );
        assert_eq!(res.usage.context_used(), 90_200);
        assert_eq!(res.usage.context_window, 1_000_000);
        assert_eq!(evs.last(), Some(&AgentEvent::TurnEnd(res)));
    }
}
