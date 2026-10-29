//! Recur's MCP host server: a small HTTP server the agent CLIs connect to. Its
//! tools are not a way to run code: each tool call becomes a typed engine
//! callback on [`recur_agent_tools::Host`], so structured signals re-enter the
//! engine as events and the engine enforces its rules at the tool boundary.

mod protocol;
mod server;
mod tools;

pub use protocol::{
    CODE_INTERNAL, CODE_INVALID_PARAMS, CODE_INVALID_REQUEST, CODE_METHOD_NOT_FOUND, CODE_PARSE,
    JSONRPC_VERSION, PROTOCOL_VERSION, SERVER_NAME, SERVER_VERSION,
};
pub use server::{McpError, McpServer};
pub use tools::{append_permission_request, names_from_allowed_tools};
