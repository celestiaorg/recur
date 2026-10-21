//! Go `internal/lsp` types the LSP service returns. They live here until
//! `recur-lsp` is ported (slice P3), which should own them; this module then
//! re-exports its types.

use serde::{Deserialize, Serialize};

/// SessionInfo is the frontend-facing handle for an LSP session.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    pub server_key: String,
    pub root_uri: String,
    pub root_path: String,
    pub available: bool,
    /// Reason explains soft-degrade when Available is false.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// VuePluginPath is the @vue/language-server package dir for TS initialize.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub vue_plugin_path: String,
}
