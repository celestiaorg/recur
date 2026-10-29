//! Language server client: process lifecycle, JSON-RPC framing, and server
//! resolution from builtins, bundles and pinned installs, with the frontend LSP
//! client's transport rules.
//!
//! The editor talks to servers through a [`Manager`]: [`service::ensure_lsp`]
//! starts or reuses sessions for a file, [`Manager::write`] sends JSON-RPC
//! bodies, and the manager's [`Emitter`] receives [`LspEvent`]s in stdout
//! order. [`protocol`] holds the client-side message rules.

pub mod frame;
pub mod managed;
pub mod manager;
pub mod protocol;
pub mod servers;
pub mod service;
pub mod session;
pub mod uri;

pub use manager::{Manager, SessionInfo};
pub use servers::{ServerRegistry, ServerSpec, builtin_servers};
pub use service::{EnsureResult, LanguageServerInfo};
pub use session::{Emitter, LspEvent, Session};
pub use uri::{canon_path, path_to_uri, uri_to_path};

#[cfg(test)]
mod golden_tests;
