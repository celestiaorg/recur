//! OpenAI Responses SSE parsing and the non-stream JSON fallback (Go
//! `responses/sse.go`).

use serde_json::{Map, Value};

use crate::gojson::{Mismatch, array_field, as_struct, int_field, str_field, valid};
use crate::types::{Event, FunctionCall, Usage};

/// An incremental SSE reader. Unknown event types are ignored and a final
/// `[DONE]` frame ends the stream.
///
/// Local servers (LM Studio, llama.cpp) often emit `data: {json}\n` without
/// the blank line SSE uses as a dispatch boundary. Waiting for that blank
/// would concatenate later events and keep only the first object, so the chat
/// would freeze while the model is still generating; a data payload that is
/// already valid JSON is dispatched immediately.
#[derive(Debug, Default)]
pub struct SseParser {
    line: Vec<u8>,
    data: String,
}

impl SseParser {
    /// Feeds one chunk of the body.
    pub fn push(&mut self, chunk: &[u8], emit: &mut dyn FnMut(Event)) {
        let mut rest = chunk;
        while let Some(i) = rest.iter().position(|&b| b == b'\n') {
            self.line.extend_from_slice(&rest[..i]);
            let line = std::mem::take(&mut self.line);
            self.line_done(&line, emit);
            rest = &rest[i + 1..];
        }
        self.line.extend_from_slice(rest);
    }

    /// The body ended: a trailing unterminated line and pending data are
    /// dispatched.
    pub fn finish(&mut self, emit: &mut dyn FnMut(Event)) {
        let line = std::mem::take(&mut self.line);
        if !line.is_empty() {
            self.line_done(&line, emit);
        }
        self.flush(emit);
    }

    fn line_done(&mut self, raw: &[u8], emit: &mut dyn FnMut(Event)) {
        let line = String::from_utf8_lossy(raw);
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            self.flush(emit);
        } else if let Some(rest) = line.strip_prefix("data:") {
            if !self.data.is_empty() {
                self.data.push('\n');
            }
            self.data.push_str(rest.trim());
            if valid(self.data.trim()) {
                self.flush(emit);
            }
        }
    }

    fn flush(&mut self, emit: &mut dyn FnMut(Event)) {
        let data = std::mem::take(&mut self.data);
        let payload = data.trim();
        if payload.is_empty() || payload == "[DONE]" {
            return;
        }
        if let Some(ev) = decode_event(payload) {
            emit(ev);
        }
    }
}

/// Parses a whole SSE body (Go `ParseSSE` over an in-memory reader).
pub fn parse_sse(body: &[u8], emit: &mut dyn FnMut(Event)) {
    let mut p = SseParser::default();
    p.push(body, emit);
    p.finish(emit);
}

/// Decodes one SSE data payload; `None` for undecodable or untyped events.
pub(crate) fn decode_event(payload: &str) -> Option<Event> {
    let v: Value = serde_json::from_str(payload).ok()?;
    let head = as_struct(&v).ok()?.cloned().unwrap_or_default();
    let typ = str_field(&head, "type").ok()?;
    let text = str_field(&head, "text").ok()?;
    let item_id = str_field(&head, "item_id").ok()?;
    if typ.is_empty() {
        return None;
    }
    let mut delta = parse_delta(head.get("delta"));
    if delta.is_empty() {
        delta = text;
    }
    let mut ev = Event {
        r#type: typ.clone(),
        item_id,
        delta,
        ..Default::default()
    };
    let item = head.get("item");
    if ev.item_id.is_empty()
        && let Some(item) = item
        && let Ok(obj) = as_struct(item)
        && let Ok(id) = obj.map_or(Ok(String::new()), |o| str_field(o, "id"))
    {
        ev.item_id = id;
    }
    if let Some(resp) = head.get("response")
        && let Ok(resp) = decode_response_head(resp)
    {
        ev.response_id = resp.id;
        ev.status = resp.status;
        ev.usage = parse_usage(resp.usage.as_ref());
        if let Some(msg) = resp.error {
            ev.error_message = msg;
        }
        if typ == "response.completed" || typ == "response.failed" {
            ev.output = resp.output.clone();
            for raw in &resp.output {
                for sub in events_from_output_item(raw) {
                    if sub.call.is_some() && ev.call.is_none() {
                        ev.call = sub.call;
                    }
                    if !sub.output_text.is_empty() && ev.output_text.is_empty() {
                        ev.output_text = sub.output_text;
                    }
                }
            }
        }
    }
    if let Some(item) = item {
        if typ == "response.output_item.done" {
            ev.output = vec![item.clone()];
        }
        if let Some(call) = parse_function_call(item) {
            ev.call = Some(call);
        }
        if let Ok((kind, text)) = message_item(item)
            && kind == "message"
        {
            ev.output_text = text;
        }
    }
    if let Some(err) = head.get("error")
        && let Ok(obj) = as_struct(err)
        && let Ok(msg) = obj.map_or(Ok(String::new()), |o| str_field(o, "message"))
    {
        ev.error_message = msg;
    }
    Some(ev)
}

/// The `response` object of a stream event.
struct ResponseHead {
    id: String,
    status: String,
    usage: Option<Value>,
    error: Option<String>,
    output: Vec<Value>,
}

fn decode_response_head(v: &Value) -> Result<ResponseHead, Mismatch> {
    let Some(obj) = as_struct(v)? else {
        return Ok(ResponseHead {
            id: String::new(),
            status: String::new(),
            usage: None,
            error: None,
            output: Vec::new(),
        });
    };
    Ok(ResponseHead {
        id: str_field(obj, "id")?,
        status: str_field(obj, "status")?,
        usage: obj.get("usage").cloned(),
        error: error_field(obj)?,
        output: array_field(obj, "output")?.to_vec(),
    })
}

/// A `*struct{Message string}` field: `None` when absent or `null`.
fn error_field(obj: &Map<String, Value>) -> Result<Option<String>, Mismatch> {
    match obj.get("error") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(e)) => Ok(Some(str_field(e, "message")?)),
        Some(_) => Err(Mismatch),
    }
}

/// The decoded fields of a Responses envelope. Go decodes a `{response: …}`
/// wrap over the same struct, so a later decode only replaces the keys it
/// carries.
#[derive(Default)]
struct Envelope {
    object: String,
    id: String,
    status: String,
    error: Option<String>,
    usage: Option<Value>,
    output: Option<Value>,
    response: Option<Value>,
}

impl Envelope {
    fn overlay(&mut self, obj: &Map<String, Value>) -> Result<(), Mismatch> {
        for (key, slot) in [
            ("object", &mut self.object),
            ("id", &mut self.id),
            ("status", &mut self.status),
        ] {
            match obj.get(key) {
                None | Some(Value::Null) => {}
                Some(Value::String(s)) => *slot = s.clone(),
                Some(_) => return Err(Mismatch),
            }
        }
        match obj.get("error") {
            None => {}
            Some(Value::Null) => self.error = None,
            Some(Value::Object(e)) => match e.get("message") {
                None | Some(Value::Null) => {
                    self.error.get_or_insert_with(String::new);
                }
                Some(Value::String(s)) => self.error = Some(s.clone()),
                Some(_) => return Err(Mismatch),
            },
            Some(_) => return Err(Mismatch),
        }
        for (key, slot) in [
            ("usage", &mut self.usage),
            ("output", &mut self.output),
            ("response", &mut self.response),
        ] {
            if let Some(v) = obj.get(key) {
                *slot = Some(v.clone());
            }
        }
        Ok(())
    }
}

/// Parses a non-stream JSON body into the events a stream would have
/// produced (function calls, final text, usage).
///
/// The body must be a Responses object (or a `{response: …}` wrap): `{}`,
/// `null` and unrelated JSON are errors, not a synthetic completed turn.
/// Nonterminal statuses (queued, in_progress) error; incomplete is a real
/// terminal event so a token-capped ping passes while the harness still
/// rejects it.
pub fn decode_response(body: &[u8]) -> Result<Vec<Event>, String> {
    let text = String::from_utf8_lossy(body);
    let text = text.trim();
    if !text.starts_with('{') {
        return Err("not a Responses object".into());
    }
    let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let mismatch = |_| "json: cannot unmarshal into a Responses object".to_owned();
    let mut env = Envelope::default();
    if let Value::Object(obj) = &v {
        env.overlay(obj).map_err(mismatch)?;
    }
    if !env.output.as_ref().is_some_and(Value::is_array)
        && let Some(Value::Object(inner)) = env.response.clone()
    {
        env.overlay(&inner).map_err(mismatch)?;
    }
    let err_msg = env.error.clone().unwrap_or_default();
    let output_is_array = env.output.as_ref().is_some_and(Value::is_array);
    if !is_responses_object(&env.object, &env.id, &env.status, output_is_array, &err_msg) {
        return Err("not a Responses object".into());
    }
    if !err_msg.is_empty() {
        return Ok(vec![Event {
            r#type: "error".into(),
            response_id: env.id,
            status: env.status,
            error_message: err_msg,
            ..Default::default()
        }]);
    }
    let term_type = json_response_terminal(&env.status)?;
    let mut out = Vec::new();
    if let Some(Value::Array(items)) = &env.output {
        for raw in items {
            for mut ev in events_from_output_item(raw) {
                ev.response_id = env.id.clone();
                out.push(ev);
            }
        }
    }
    let mut term = Event {
        r#type: term_type.into(),
        response_id: env.id,
        status: env.status,
        usage: parse_usage(env.usage.as_ref()),
        ..Default::default()
    };
    if term.status.is_empty() && term_type == "response.completed" {
        term.status = "completed".into();
    }
    out.push(term);
    Ok(out)
}

fn is_responses_object(
    object: &str,
    id: &str,
    status: &str,
    output_is_array: bool,
    err_msg: &str,
) -> bool {
    if !err_msg.is_empty() || object == "response" || is_response_status(status) {
        return true;
    }
    !id.is_empty() && output_is_array
}

fn is_response_status(status: &str) -> bool {
    matches!(
        status,
        "completed" | "failed" | "cancelled" | "canceled" | "incomplete" | "in_progress" | "queued"
    )
}

fn json_response_terminal(status: &str) -> Result<&'static str, String> {
    match status {
        "" | "completed" => Ok("response.completed"),
        "incomplete" => Ok("response.incomplete"),
        "failed" | "cancelled" | "canceled" => Ok("response.failed"),
        other => Err(format!("responses status {other}")),
    }
}

/// A `message` item's type and concatenated output text.
fn message_item(raw: &Value) -> Result<(String, String), Mismatch> {
    let Some(obj) = as_struct(raw)? else {
        return Ok((String::new(), String::new()));
    };
    let kind = str_field(obj, "type")?;
    let mut text = String::new();
    for c in array_field(obj, "content")? {
        let Some(c) = as_struct(c)? else { continue };
        let ctype = str_field(c, "type")?;
        let ctext = str_field(c, "text")?;
        if ctype == "output_text" || ctype == "text" {
            text.push_str(&ctext);
        }
    }
    Ok((kind, text))
}

fn events_from_output_item(raw: &Value) -> Vec<Event> {
    let Ok(Some(obj)) = as_struct(raw) else {
        return Vec::new();
    };
    let Ok(kind) = str_field(obj, "type") else {
        return Vec::new();
    };
    let call = function_call_fields(obj);
    let message = message_item(raw);
    let (Ok(call), Ok((_, text))) = (call, message) else {
        return Vec::new();
    };
    match kind.as_str() {
        "function_call" => vec![Event {
            r#type: "response.output_item.done".into(),
            call: Some(call),
            ..Default::default()
        }],
        "message" if !text.is_empty() => vec![Event {
            r#type: "response.output_text.delta".into(),
            delta: text.clone(),
            output_text: text,
            ..Default::default()
        }],
        _ => Vec::new(),
    }
}

fn function_call_fields(obj: &Map<String, Value>) -> Result<FunctionCall, Mismatch> {
    Ok(FunctionCall {
        id: str_field(obj, "id")?,
        call_id: str_field(obj, "call_id")?,
        name: str_field(obj, "name")?,
        arguments: str_field(obj, "arguments")?,
    })
}

fn parse_function_call(raw: &Value) -> Option<FunctionCall> {
    let obj = as_struct(raw).ok()??;
    if str_field(obj, "type").ok()? != "function_call" {
        return None;
    }
    function_call_fields(obj).ok()
}

fn parse_delta(raw: Option<&Value>) -> String {
    match raw {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(o)) => str_field(o, "text").unwrap_or_default(),
        _ => String::new(),
    }
}

/// Usage from a response object; `None` when absent, undecodable, or all
/// zero.
fn parse_usage(raw: Option<&Value>) -> Option<Usage> {
    let obj = as_struct(raw?).ok()?.cloned().unwrap_or_default();
    let input = int_field(&obj, "input_tokens").ok()?;
    let output = int_field(&obj, "output_tokens").ok()?;
    let total = int_field(&obj, "total_tokens").ok()?;
    let mut cache = int_field(&obj, "cache_read_input_tokens").ok()?;
    let details = match obj.get("input_tokens_details") {
        None | Some(Value::Null) => None,
        Some(Value::Object(d)) => Some(int_field(d, "cached_tokens").ok()?),
        Some(_) => return None,
    };
    if cache == 0
        && let Some(cached) = details
    {
        cache = cached;
    }
    if input == 0 && output == 0 && cache == 0 {
        return None;
    }
    Some(Usage {
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache,
        total_tokens: total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(raw: &str) -> Vec<Event> {
        let mut out = Vec::new();
        parse_sse(raw.as_bytes(), &mut |e| out.push(e));
        out
    }

    #[test]
    fn json_lines_without_blank() {
        let raw = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"a\"}\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"b\"}\n";
        let got: String = collect(raw).into_iter().map(|e| e.delta).collect();
        assert_eq!(got, "ab");
    }

    #[test]
    fn object_delta() {
        let raw = "data: {\"type\":\"response.output_text.delta\",\"delta\":{\"text\":\"hi\"}}\n\n";
        assert_eq!(collect(raw)[0].delta, "hi");
    }

    #[test]
    fn text_and_function_call() {
        let raw = [
            r#"event: response.created"#,
            r#"data: {"type":"response.created","response":{"id":"resp_1"}}"#,
            "",
            r#"event: response.output_text.delta"#,
            r#"data: {"type":"response.output_text.delta","delta":"hi"}"#,
            "",
            r#"event: response.output_item.done"#,
            r#"data: {"type":"response.output_item.done","item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"Read","arguments":"{\"file_path\":\"a.go\"}"}}"#,
            "",
            r#"event: response.completed"#,
            r#"data: {"type":"response.completed","response":{"id":"resp_1","usage":{"input_tokens":10,"output_tokens":4}}}"#,
            "",
        ]
        .join("\n");
        let evs = collect(&raw);
        assert_eq!(evs.len(), 4);
        assert_eq!(evs[0].response_id, "resp_1");
        assert_eq!(evs[1].delta, "hi");
        let call = evs[2].call.as_ref().unwrap();
        assert_eq!(
            (call.name.as_str(), call.call_id.as_str()),
            ("Read", "call_1")
        );
        assert_eq!(evs[3].usage.unwrap().input_tokens, 10);
    }

    #[test]
    fn chunk_boundaries_do_not_matter() {
        let raw = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"é\"}\n\n";
        let mut p = SseParser::default();
        let mut out = Vec::new();
        for b in raw.as_bytes() {
            p.push(std::slice::from_ref(b), &mut |e| out.push(e));
        }
        p.finish(&mut |e| out.push(e));
        assert_eq!(out[0].delta, "é");
    }

    #[test]
    fn decode_json_fallback() {
        let body = br#"{"id":"resp_2","status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"done"}]}],"usage":{"input_tokens":3,"output_tokens":1}}"#;
        let evs = decode_response(body).unwrap();
        let text: String = evs.iter().map(|e| e.delta.as_str()).collect();
        assert_eq!(text, "done");
        assert_eq!(evs.iter().find_map(|e| e.usage).unwrap().input_tokens, 3);
    }

    #[test]
    fn decode_ignores_unknown() {
        assert!(decode_event(r#"{"foo":1}"#).is_none());
    }

    #[test]
    fn decode_rejects_malformed_envelope() {
        for body in [r#"{}"#, "null", "[]", r#"{"foo":1}"#, r#"{"id":"r"}"#] {
            assert!(decode_response(body.as_bytes()).is_err(), "{body}");
        }
    }

    #[test]
    fn decode_rejects_nonterminal_status() {
        for status in ["queued", "in_progress"] {
            let body = format!(r#"{{"id":"r","status":"{status}","output":[]}}"#);
            let err = decode_response(body.as_bytes()).unwrap_err();
            assert!(err.contains(status), "{err}");
        }
    }

    #[test]
    fn decode_incomplete_is_not_completed() {
        let body = br#"{"id":"r","status":"incomplete","output":[{"type":"message","content":[{"type":"output_text","text":"hi"}]}],"usage":{"input_tokens":1,"output_tokens":1}}"#;
        let evs = decode_response(body).unwrap();
        assert!(!evs.iter().any(|e| e.r#type == "response.completed"));
        assert!(evs.iter().any(|e| e.r#type == "response.incomplete"));
    }
}
