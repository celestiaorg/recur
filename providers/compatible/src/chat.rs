//! The Chat Completions route (`POST {base}/v1/chat/completions`) for
//! endpoints that do not speak Responses (DeepSeek, many local servers).
//!
//! The harness keeps building Responses bodies and reading Responses events:
//! [`chat_body`] rewrites a request on the way out and [`ChatStream`] turns
//! the streamed chunks back into the Responses events the round loop reads.

use std::collections::{BTreeMap, HashSet};

use serde_json::{Map, Value, json};

use crate::types::{Event, FunctionCall, Usage};

/// Rewrites a Responses request body as a Chat Completions body: the
/// instructions become a system message, input items become messages (a
/// batch of function calls one assistant message followed by its tool
/// results, a reasoning item the next assistant message's
/// `reasoning_content`), and tools and flag wires move to their Chat
/// Completions names.
pub fn chat_body(responses_body: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(responses_body)
        .map_err(|e| format!("chat completions encode: {e}"))?;
    let Value::Object(req) = v else {
        return Err("chat completions encode: not an object".into());
    };
    let mut out = Map::new();
    out.insert(
        "model".into(),
        req.get("model").cloned().unwrap_or_default(),
    );
    let instructions = req
        .get("instructions")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let items = req
        .get("input")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    out.insert("messages".into(), messages(instructions, items).into());
    let tools: Vec<Value> = req
        .get("tools")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(chat_tool)
        .collect();
    if !tools.is_empty() {
        out.insert("tools".into(), tools.into());
        if let Some(choice) = req.get("tool_choice").filter(|c| c.is_string()) {
            out.insert("tool_choice".into(), choice.clone());
        }
    }
    if req.get("stream").and_then(Value::as_bool) == Some(true) {
        out.insert("stream".into(), true.into());
        out.insert("stream_options".into(), json!({"include_usage": true}));
    }
    if let Some(n) = req.get("max_output_tokens").filter(|n| n.is_number()) {
        out.insert("max_tokens".into(), n.clone());
    }
    if let Some(t) = req.get("temperature").filter(|t| t.is_number()) {
        out.insert("temperature".into(), t.clone());
    }
    if let Some(effort) = req
        .get("reasoning")
        .and_then(|r| r.get("effort"))
        .filter(|e| e.is_string())
    {
        out.insert("reasoning_effort".into(), effort.clone());
    }
    if let Some(verbosity) = req
        .get("text")
        .and_then(|t| t.get("verbosity"))
        .filter(|e| e.is_string())
    {
        out.insert("verbosity".into(), verbosity.clone());
    }
    Ok(Value::Object(out).to_string())
}

fn chat_tool(t: &Value) -> Option<Value> {
    let name = t.get("name")?.as_str()?;
    let mut f = Map::new();
    f.insert("name".into(), name.into());
    if let Some(d) = t.get("description").and_then(Value::as_str)
        && !d.is_empty()
    {
        f.insert("description".into(), d.into());
    }
    f.insert(
        "parameters".into(),
        t.get("parameters")
            .cloned()
            .filter(|p| !p.is_null())
            .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
    );
    Some(json!({"type": "function", "function": f}))
}

fn str_of<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn messages(instructions: &str, items: &[Value]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    if !instructions.trim().is_empty() {
        out.push(json!({"role": "system", "content": instructions}));
    }
    let mut reasoning: Option<String> = None;
    let mut i = 0;
    while i < items.len() {
        let kind = str_of(&items[i], "type");
        if kind == "reasoning" {
            let text = reasoning_text(&items[i]);
            if !text.is_empty() {
                reasoning.get_or_insert_default().push_str(&text);
            }
            i += 1;
            continue;
        }
        if kind == "function_call" || kind == "function_call_output" {
            let start = i;
            while i < items.len()
                && matches!(
                    str_of(&items[i], "type"),
                    "function_call" | "function_call_output"
                )
            {
                i += 1;
            }
            push_tool_batch(&mut out, &items[start..i], reasoning.take());
            continue;
        }
        if let Some(mut m) = message(&items[i]) {
            if let (Some(r), Value::Object(o)) = (reasoning.take(), &mut m)
                && o.get("role").and_then(Value::as_str) == Some("assistant")
            {
                o.insert("reasoning_content".into(), r.into());
            }
            out.push(m);
        }
        i += 1;
    }
    out
}

/// A Responses reasoning item's raw chain-of-thought text.
fn reasoning_text(item: &Value) -> String {
    item.get("content")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|p| str_of(p, "type") == "reasoning_text")
        .map(|p| str_of(p, "text"))
        .collect()
}

/// One run of function calls and outputs: every call goes on one assistant
/// message (the preceding assistant text when it is last), then each call's
/// result in call order. A call without a result gets an empty one and a
/// result without a call is dropped, since Chat Completions rejects both.
/// The round's `reasoning` goes on the assistant message unless the text
/// it merges into already carries it.
fn push_tool_batch(out: &mut Vec<Value>, run: &[Value], reasoning: Option<String>) {
    let calls: Vec<&Value> = run
        .iter()
        .filter(|it| str_of(it, "type") == "function_call")
        .collect();
    if calls.is_empty() {
        return;
    }
    let tool_calls: Vec<Value> = calls
        .iter()
        .map(|c| {
            let args = str_of(c, "arguments");
            json!({
                "id": str_of(c, "call_id"),
                "type": "function",
                "function": {
                    "name": str_of(c, "name"),
                    "arguments": if args.is_empty() { "{}" } else { args },
                },
            })
        })
        .collect();
    let merge = out.last().is_some_and(|m| {
        str_of(m, "role") == "assistant"
            && m.get("tool_calls").is_none()
            && m["content"].is_string()
    });
    if merge && let Some(Value::Object(last)) = out.last_mut() {
        last.insert("tool_calls".into(), tool_calls.into());
        if let Some(r) = reasoning {
            last.insert("reasoning_content".into(), r.into());
        }
    } else {
        let mut m = json!({"role": "assistant", "content": null, "tool_calls": tool_calls});
        if let Some(r) = reasoning {
            m["reasoning_content"] = r.into();
        }
        out.push(m);
    }
    let mut answered = HashSet::new();
    for c in &calls {
        let id = str_of(c, "call_id");
        if !answered.insert(id) {
            continue;
        }
        let result = run
            .iter()
            .find(|it| str_of(it, "type") == "function_call_output" && str_of(it, "call_id") == id)
            .map(|it| str_of(it, "output"))
            .unwrap_or_default();
        out.push(json!({"role": "tool", "tool_call_id": id, "content": result}));
    }
}

/// A Responses message item as a chat message. Text parts join with
/// newlines; a user message with images becomes a content-part array.
fn message(item: &Value) -> Option<Value> {
    let role = match str_of(item, "role") {
        "" => return None,
        "developer" => "system",
        r => r,
    };
    let parts = item
        .get("content")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut text = Vec::new();
    let mut images = Vec::new();
    for p in parts {
        match str_of(p, "type") {
            "input_text" | "output_text" | "text" => text.push(str_of(p, "text")),
            "input_image" if role == "user" => images.push(str_of(p, "image_url")),
            _ => {}
        }
    }
    if images.is_empty() {
        if text.is_empty() {
            return None;
        }
        return Some(json!({"role": role, "content": text.join("\n")}));
    }
    let mut content: Vec<Value> = text
        .iter()
        .map(|t| json!({"type": "text", "text": t}))
        .collect();
    content.extend(
        images
            .iter()
            .map(|url| json!({"type": "image_url", "image_url": {"url": url}})),
    );
    Some(json!({"role": role, "content": content}))
}

/// Folds streamed `chat.completion.chunk` payloads into Responses events:
/// text and reasoning deltas as they come, then the finished tool calls and
/// one terminal event when the body ends. A stream that never reports a
/// finish reason gets no terminal event, so the harness rejects it.
#[derive(Debug, Default)]
pub struct ChatStream {
    id: String,
    calls: BTreeMap<u64, FunctionCall>,
    finish: String,
    usage: Option<Usage>,
}

impl ChatStream {
    /// One SSE data payload.
    pub fn on_payload(&mut self, payload: &str, emit: &mut dyn FnMut(Event)) {
        let Ok(v) = serde_json::from_str::<Value>(payload) else {
            return;
        };
        self.on_chunk(&v, emit);
    }

    fn on_chunk(&mut self, v: &Value, emit: &mut dyn FnMut(Event)) {
        if let Some(msg) = error_message(v) {
            emit(Event {
                r#type: "error".into(),
                error_message: msg,
                ..Default::default()
            });
            return;
        }
        if let Some(id) = v.get("id").and_then(Value::as_str)
            && !id.is_empty()
        {
            self.id = id.to_owned();
        }
        if let Some(u) = chat_usage(v.get("usage")) {
            self.usage = Some(u);
        }
        let choices = v
            .get("choices")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for choice in choices.iter().filter(|c| index_of(c) == 0) {
            let delta = choice.get("delta").or_else(|| choice.get("message"));
            if let Some(d) = delta {
                self.on_delta(d, emit);
            }
            if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str)
                && !reason.is_empty()
            {
                self.finish = reason.to_owned();
            }
        }
    }

    fn on_delta(&mut self, d: &Value, emit: &mut dyn FnMut(Event)) {
        for key in ["reasoning_content", "reasoning"] {
            let think = str_of(d, key);
            if !think.is_empty() {
                emit(Event {
                    r#type: "response.reasoning_text.delta".into(),
                    item_id: "reasoning".into(),
                    delta: think.to_owned(),
                    ..Default::default()
                });
                break;
            }
        }
        let text = str_of(d, "content");
        if !text.is_empty() {
            emit(Event {
                r#type: "response.output_text.delta".into(),
                item_id: "message".into(),
                delta: text.to_owned(),
                ..Default::default()
            });
        }
        let calls = d
            .get("tool_calls")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for (pos, tc) in calls.iter().enumerate() {
            let index = tc
                .get("index")
                .and_then(Value::as_u64)
                .unwrap_or(pos as u64);
            let slot = self.calls.entry(index).or_default();
            let id = str_of(tc, "id");
            if !id.is_empty() {
                slot.call_id = id.to_owned();
                slot.id = id.to_owned();
            }
            if let Some(f) = tc.get("function") {
                let name = str_of(f, "name");
                if !name.is_empty() {
                    slot.name = name.to_owned();
                }
                slot.arguments.push_str(str_of(f, "arguments"));
            }
        }
    }

    /// The body ended: the finished calls, then the terminal event.
    pub fn finish(&mut self, emit: &mut dyn FnMut(Event)) {
        if self.finish.is_empty() {
            return;
        }
        for (i, mut call) in std::mem::take(&mut self.calls) {
            if call.name.is_empty() {
                continue;
            }
            if call.call_id.is_empty() {
                call.call_id = format!("call_{i}");
                call.id.clone_from(&call.call_id);
            }
            emit(Event {
                r#type: "response.output_item.done".into(),
                call: Some(call),
                ..Default::default()
            });
        }
        let (kind, status) = match self.finish.as_str() {
            "length" => ("response.incomplete", "incomplete"),
            "content_filter" => ("response.failed", "failed"),
            _ => ("response.completed", "completed"),
        };
        emit(Event {
            r#type: kind.into(),
            response_id: self.id.clone(),
            status: status.into(),
            usage: self.usage,
            error_message: if status == "failed" {
                "chat completions: the response was filtered".into()
            } else {
                String::new()
            },
            ..Default::default()
        });
    }
}

fn index_of(choice: &Value) -> u64 {
    choice.get("index").and_then(Value::as_u64).unwrap_or(0)
}

fn error_message(v: &Value) -> Option<String> {
    match v.get("error")? {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        e => Some(
            e.get("message")
                .and_then(Value::as_str)
                .map_or_else(|| e.to_string(), str::to_owned),
        ),
    }
}

/// Chat Completions usage as Responses usage: prompt tokens include cache
/// hits (OpenAI `prompt_tokens_details.cached_tokens`, DeepSeek
/// `prompt_cache_hit_tokens`).
fn chat_usage(raw: Option<&Value>) -> Option<Usage> {
    let u = raw?.as_object()?;
    let int = |k: &str| u.get(k).and_then(Value::as_i64).unwrap_or(0);
    let input = int("prompt_tokens");
    let output = int("completion_tokens");
    let cache = u
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| int("prompt_cache_hit_tokens"));
    if input == 0 && output == 0 {
        return None;
    }
    Some(Usage {
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache,
        total_tokens: int("total_tokens"),
    })
}

/// A non-streamed `chat.completion` body as the events a stream would have
/// produced. Anything without a `choices` array (or an error) fails.
pub fn decode_chat_completion(body: &[u8]) -> Result<Vec<Event>, String> {
    let v: Value =
        serde_json::from_slice(body).map_err(|_| "not a Chat Completions body".to_owned())?;
    if let Some(msg) = error_message(&v) {
        return Ok(vec![Event {
            r#type: "error".into(),
            error_message: msg,
            ..Default::default()
        }]);
    }
    if !v.get("choices").is_some_and(Value::is_array) {
        return Err("not a Chat Completions body".into());
    }
    let mut out = Vec::new();
    let mut st = ChatStream::default();
    st.on_chunk(&v, &mut |e| out.push(e));
    if st.finish.is_empty() {
        st.finish = "stop".into();
    }
    st.finish(&mut |e| out.push(e));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::sse::SseParser;
    use crate::types::{
        Request, assistant_text, function_call_item, function_output_item, function_tool,
        reasoning_item, user_text,
    };

    fn body(req: &Request) -> Value {
        serde_json::from_str(&chat_body(&serde_json::to_string(req).unwrap()).unwrap()).unwrap()
    }

    #[test]
    fn request_maps_messages_tools_and_flags() {
        let mut req = Request {
            model: "deepseek-chat".into(),
            instructions: "be brief".into(),
            input: vec![
                user_text("list files"),
                assistant_text("Looking."),
                function_call_item("call_1", "Bash", r#"{"command":"ls"}"#),
                function_call_item("call_2", "Read", ""),
                function_output_item("call_1", "a.txt"),
                function_output_item("call_2", "hi"),
                function_output_item("orphan", "dropped"),
                user_text("thanks"),
            ],
            tools: vec![function_tool("Bash", "Run a command", None)],
            tool_choice: "auto".into(),
            stream: true,
            parallel_tool_calls: true,
            max_output_tokens: 64,
            temperature: Some(0.5),
            ..Default::default()
        };
        req.reasoning = Some(crate::types::Reasoning {
            effort: "high".into(),
            summary: "auto".into(),
        });
        let got = body(&req);
        assert_eq!(
            got,
            json!({
                "model": "deepseek-chat",
                "messages": [
                    {"role": "system", "content": "be brief"},
                    {"role": "user", "content": "list files"},
                    {"role": "assistant", "content": "Looking.", "tool_calls": [
                        {"id": "call_1", "type": "function",
                         "function": {"name": "Bash", "arguments": "{\"command\":\"ls\"}"}},
                        {"id": "call_2", "type": "function",
                         "function": {"name": "Read", "arguments": "{}"}},
                    ]},
                    {"role": "tool", "tool_call_id": "call_1", "content": "a.txt"},
                    {"role": "tool", "tool_call_id": "call_2", "content": "hi"},
                    {"role": "user", "content": "thanks"},
                ],
                "tools": [{"type": "function", "function": {
                    "name": "Bash", "description": "Run a command",
                    "parameters": {"type": "object", "properties": {}},
                }}],
                "tool_choice": "auto",
                "stream": true,
                "stream_options": {"include_usage": true},
                "max_tokens": 64,
                "temperature": 0.5,
                "reasoning_effort": "high",
            })
        );
    }

    #[test]
    fn request_passes_reasoning_back_on_assistant_messages() {
        let got = body(&Request {
            model: "deepseek-flash".into(),
            input: vec![
                user_text("hello"),
                reasoning_item("Look first."),
                assistant_text("Looking."),
                function_call_item("c1", "Bash", "{}"),
                function_output_item("c1", "rejected"),
                reasoning_item("Try reading."),
                function_call_item("c2", "Read", "{}"),
                function_output_item("c2", "rejected"),
            ],
            ..Default::default()
        });
        assert_eq!(
            got["messages"],
            json!([
                {"role": "user", "content": "hello"},
                {"role": "assistant", "content": "Looking.", "reasoning_content": "Look first.",
                 "tool_calls": [
                    {"id": "c1", "type": "function", "function": {"name": "Bash", "arguments": "{}"}},
                ]},
                {"role": "tool", "tool_call_id": "c1", "content": "rejected"},
                {"role": "assistant", "content": null, "reasoning_content": "Try reading.",
                 "tool_calls": [
                    {"id": "c2", "type": "function", "function": {"name": "Read", "arguments": "{}"}},
                ]},
                {"role": "tool", "tool_call_id": "c2", "content": "rejected"},
            ])
        );
    }

    #[test]
    fn request_answers_unanswered_calls_and_keeps_images() {
        let mut img = user_text("see");
        img.content.push(crate::types::InputContent {
            r#type: "input_image".into(),
            image_url: "data:image/png;base64,AA==".into(),
            ..Default::default()
        });
        let got = body(&Request {
            model: "m".into(),
            input: vec![img, function_call_item("c1", "Bash", "{}")],
            ..Default::default()
        });
        assert_eq!(
            got["messages"],
            json!([
                {"role": "user", "content": [
                    {"type": "text", "text": "see"},
                    {"type": "image_url", "image_url": {"url": "data:image/png;base64,AA=="}},
                ]},
                {"role": "assistant", "content": null, "tool_calls": [
                    {"id": "c1", "type": "function", "function": {"name": "Bash", "arguments": "{}"}},
                ]},
                {"role": "tool", "tool_call_id": "c1", "content": ""},
            ])
        );
        assert!(got.get("stream").is_none() && got.get("tools").is_none());
    }

    fn stream(raw: &str) -> Vec<Event> {
        let mut out = Vec::new();
        let mut sse = SseParser::default();
        let mut chat = ChatStream::default();
        sse.push_payloads(raw.as_bytes(), &mut |p| {
            chat.on_payload(p, &mut |e| out.push(e))
        });
        sse.finish_payloads(&mut |p| chat.on_payload(p, &mut |e| out.push(e)));
        chat.finish(&mut |e| out.push(e));
        out
    }

    #[test]
    fn stream_text_reasoning_calls_and_usage() {
        let raw = [
            r#"{"id":"c1","choices":[{"index":0,"delta":{"role":"assistant","reasoning_content":"hmm"}}]}"#,
            r#"{"id":"c1","choices":[{"index":0,"delta":{"content":"Hi"}}]}"#,
            r#"{"id":"c1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"Bash","arguments":""}}]}}]}"#,
            r#"{"id":"c1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"command\":"}}]}}]}"#,
            r#"{"id":"c1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"ls\"}"}}]}}]}"#,
            r#"{"id":"c1","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
            r#"{"id":"c1","choices":[],"usage":{"prompt_tokens":12,"completion_tokens":5,"total_tokens":17,"prompt_cache_hit_tokens":8}}"#,
            "[DONE]",
        ]
        .map(|f| format!("data: {f}\n\n"))
        .concat();
        let evs = stream(&raw);
        let kinds: Vec<&str> = evs.iter().map(|e| e.r#type.as_str()).collect();
        assert_eq!(
            kinds,
            [
                "response.reasoning_text.delta",
                "response.output_text.delta",
                "response.output_item.done",
                "response.completed",
            ]
        );
        assert_eq!(
            (evs[0].delta.as_str(), evs[1].delta.as_str()),
            ("hmm", "Hi")
        );
        let call = evs[2].call.clone().unwrap();
        assert_eq!(
            (
                call.call_id.as_str(),
                call.name.as_str(),
                call.arguments.as_str()
            ),
            ("call_a", "Bash", r#"{"command":"ls"}"#)
        );
        assert_eq!(evs[3].response_id, "c1");
        assert_eq!(
            evs[3].usage,
            Some(Usage {
                input_tokens: 12,
                output_tokens: 5,
                cache_read_tokens: 8,
                total_tokens: 17,
            })
        );
    }

    #[test]
    fn stream_without_finish_has_no_terminal_and_length_is_incomplete() {
        let cut = stream("data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n");
        assert!(
            !cut.iter()
                .any(|e| e.r#type.starts_with("response.completed"))
        );
        let long = stream("data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\n");
        assert_eq!(long.last().unwrap().r#type, "response.incomplete");
        let err = stream("data: {\"error\":{\"message\":\"Insufficient Balance\"}}\n\n");
        assert_eq!(err[0].error_message, "Insufficient Balance");
    }

    #[test]
    fn decode_non_streamed_completion() {
        let evs = decode_chat_completion(
            br#"{"id":"c2","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"pong"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":1}}"#,
        )
        .unwrap();
        assert_eq!(evs[0].delta, "pong");
        assert_eq!(evs.last().unwrap().status, "completed");
        assert!(decode_chat_completion(br#"{"id":"r","output":[]}"#).is_err());
        assert!(decode_chat_completion(b"<html>").is_err());
        let err = decode_chat_completion(br#"{"error":{"message":"bad key"}}"#).unwrap();
        assert_eq!(err[0].error_message, "bad key");
    }
}
