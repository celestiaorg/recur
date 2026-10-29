//! Request, tool, input, usage and event types for `POST /v1/responses` (Go
//! `responses/types.go`). JSON names and `omitempty` rules match Go.

use serde::Serialize;
use serde_json::{Map, Value};

/// The JSON body for `POST /v1/responses`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Request {
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub instructions: String,
    pub input: Vec<InputItem>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tool_choice: String,
    #[serde(skip_serializing_if = "is_false")]
    pub stream: bool,
    pub store: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub parallel_tool_calls: bool,
    #[serde(skip_serializing_if = "is_zero")]
    pub max_output_tokens: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<TextConfig>,
}

/// The Responses reasoning configuration object.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Reasoning {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub effort: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub summary: String,
}

/// The Responses text configuration object.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TextConfig {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub verbosity: String,
}

/// A function tool advertised to the model.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Tool {
    pub r#type: String,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// The JSON schema; `None` is omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// One Responses input item (message, function_call, or output).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct InputItem {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub r#type: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub role: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub content: Vec<InputContent>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub call_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub arguments: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub output: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// A provider output item replayed verbatim (encrypted reasoning
    /// included) when the stateless ChatGPT route needs it on the next tool
    /// round. The other fields stay empty.
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub raw: Option<Map<String, Value>>,
}

/// One part of a message item.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct InputContent {
    pub r#type: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub image_url: String,
}

/// One completed function_call output item from the model.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FunctionCall {
    pub id: String,
    pub call_id: String,
    pub name: String,
    pub arguments: String,
}

/// Token accounting from a completed response.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub total_tokens: i64,
}

/// One parsed stream (or final JSON) event.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Event {
    /// Output items to replay: the `response.output_item.done` item, or a
    /// terminal response's whole output.
    pub output: Vec<Value>,
    pub r#type: String,
    pub response_id: String,
    pub delta: String,
    pub item_id: String,
    pub call: Option<FunctionCall>,
    pub usage: Option<Usage>,
    pub status: String,
    pub error_message: String,
    pub output_text: String,
}

/// A function tool descriptor; a missing schema becomes an empty object
/// schema.
pub fn function_tool(name: &str, description: &str, schema: Option<Value>) -> Tool {
    let schema = schema
        .filter(|s| !s.is_null())
        .unwrap_or_else(|| serde_json::json!({"type": "object", "properties": {}}));
    Tool {
        r#type: "function".into(),
        name: name.into(),
        description: description.into(),
        parameters: Some(schema),
    }
}

/// A user message with plain text.
pub fn user_text(text: &str) -> InputItem {
    InputItem {
        role: "user".into(),
        content: vec![InputContent {
            r#type: "input_text".into(),
            text: text.into(),
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// An assistant message with output text.
pub fn assistant_text(text: &str) -> InputItem {
    InputItem {
        role: "assistant".into(),
        content: vec![InputContent {
            r#type: "output_text".into(),
            text: text.into(),
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// A prior model function_call to replay in input.
pub fn function_call_item(call_id: &str, name: &str, arguments: &str) -> InputItem {
    InputItem {
        r#type: "function_call".into(),
        call_id: call_id.into(),
        name: name.into(),
        arguments: arguments.into(),
        ..Default::default()
    }
}

/// The tool result for a prior function_call.
pub fn function_output_item(call_id: &str, output: &str) -> InputItem {
    InputItem {
        r#type: "function_call_output".into(),
        call_id: call_id.into(),
        output: output.into(),
        ..Default::default()
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}
