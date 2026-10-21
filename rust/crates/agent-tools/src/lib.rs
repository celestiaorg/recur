//! Host tools exposed to agents: the engine callback seam ([`Host`]), the
//! Recur host tools served over MCP, and the coding tools the in-process
//! harness runs (Go `internal/agenttools`).

pub mod catalog;
pub mod coding;
pub mod display;
pub mod gostr;
pub mod host;
pub mod host_tools;
pub mod json;
pub mod names;
pub mod path;
pub mod tool;

pub use catalog::{coding_tools, host_tools};
pub use coding::todos::{TodoItem, TodoStore};
pub use host::*;
pub use names::RECUR_MCP_PREFIX;
pub use tool::{
    Call, HALT_NAMES, Permitter, Registry, Tool, ToolError, advertised_name, bare_name, halt_after,
};
