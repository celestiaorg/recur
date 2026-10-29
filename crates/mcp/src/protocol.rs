//! JSON-RPC 2.0 framing and the MCP payloads Recur reads and sends.

use recur_agent_tools::json::{Shape, Ty, compact, field, unmarshal};
use serde::Serialize;

/// The MCP protocol version Recur advertises when the client names none. A
/// client's requested version is echoed: the shapes Recur uses (initialize,
/// tools/list, tools/call) are stable across these revisions.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const SERVER_NAME: &str = "recur";
pub const SERVER_VERSION: &str = "1.0.0";
/// The only JSON-RPC version accepted.
pub const JSONRPC_VERSION: &str = "2.0";

pub const CODE_PARSE: i64 = -32700;
pub const CODE_INVALID_REQUEST: i64 = -32600;
pub const CODE_METHOD_NOT_FOUND: i64 = -32601;
pub const CODE_INVALID_PARAMS: i64 = -32602;
pub const CODE_INTERNAL: i64 = -32603;

pub(crate) const INSTRUCTIONS: &str =
    "Use these tools to report structured signals back to the Recur orchestrator.";

static RPC_REQUEST: Shape = Shape::anon(&[
    field("jsonrpc", Ty::Str),
    field("id", Ty::Raw),
    field("method", Ty::Str),
    field("params", Ty::Raw),
]);

/// One incoming request, or a notification when `id` is empty or null. `id`
/// and `params` keep their exact JSON text so the id echoes verbatim.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RpcRequest {
    pub jsonrpc: String,
    pub id: String,
    pub method: String,
    pub params: String,
}

impl RpcRequest {
    /// Go `json.Unmarshal` into `rpcRequest`.
    pub fn parse(body: &str) -> Result<RpcRequest, String> {
        let a = unmarshal(body, &RPC_REQUEST)?;
        Ok(RpcRequest {
            jsonrpc: a.str("jsonrpc").into(),
            id: a.str("id").into(),
            method: a.str("method").into(),
            params: a.str("params").into(),
        })
    }

    pub fn is_notification(&self) -> bool {
        self.id.is_empty() || self.id == "null"
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct RpcError {
    pub code: i64,
    pub message: String,
}

/// One response: exactly one of result or error.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RpcResponse {
    pub id: String,
    pub body: Result<RpcResult, RpcError>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RpcResult {
    Initialize(InitializeResult),
    ListTools(Vec<ToolEntry>),
    CallTool(CallToolResult),
    /// `{}` (ping).
    Empty,
}

pub(crate) fn new_result(id: &str, result: RpcResult) -> RpcResponse {
    RpcResponse {
        id: id.into(),
        body: Ok(result),
    }
}

pub(crate) fn new_error(id: &str, code: i64, message: impl Into<String>) -> RpcResponse {
    RpcResponse {
        id: id.into(),
        body: Err(RpcError {
            code,
            message: message.into(),
        }),
    }
}

fn to_json<T: Serialize + ?Sized>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

impl RpcResponse {
    /// The JSON-RPC response: the id compacted, result or error.
    pub fn encode(&self) -> String {
        let id = if self.id.is_empty() {
            "null".to_string()
        } else {
            compact(&self.id).unwrap_or_else(|_| "null".into())
        };
        match &self.body {
            Ok(r) => format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{}}}"#, r.encode()),
            Err(e) => format!(r#"{{"jsonrpc":"2.0","id":{id},"error":{}}}"#, to_json(e)),
        }
    }
}

impl RpcResult {
    fn encode(&self) -> String {
        match self {
            RpcResult::Initialize(r) => to_json(r),
            RpcResult::ListTools(tools) => {
                let entries: Vec<String> = tools
                    .iter()
                    .map(|t| {
                        format!(
                            r#"{{"name":{},"description":{},"inputSchema":{}}}"#,
                            to_json(&t.name),
                            to_json(&t.description),
                            compact(&t.input_schema).unwrap_or_else(|_| "null".into())
                        )
                    })
                    .collect();
                format!(r#"{{"tools":[{}]}}"#, entries.join(","))
            }
            RpcResult::CallTool(r) => to_json(r),
            RpcResult::Empty => "{}".into(),
        }
    }
}

/// The server's `initialize` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InitializeResult {
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    pub server_info: Implementation,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub instructions: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ServerCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolsCapability {
    pub list_changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Implementation {
    pub name: String,
    pub version: String,
}

/// One tools/list entry (MCP Tool shape). `input_schema` is raw JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolEntry {
    pub name: String,
    pub description: String,
    pub input_schema: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TextContent {
    #[serde(rename = "type")]
    pub kind: String,
    pub text: String,
}

/// A tools/call result. A tool-level failure is `is_error` plus a text
/// block, not a JSON-RPC error, so the model can read it and self-correct.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CallToolResult {
    pub content: Vec<TextContent>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
}

pub(crate) fn tool_text(s: impl Into<String>) -> CallToolResult {
    CallToolResult {
        content: vec![TextContent {
            kind: "text".into(),
            text: s.into(),
        }],
        is_error: false,
    }
}

pub(crate) fn tool_error(s: impl Into<String>) -> CallToolResult {
    CallToolResult {
        is_error: true,
        ..tool_text(s)
    }
}
