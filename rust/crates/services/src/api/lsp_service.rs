use async_trait::async_trait;

use super::{EnsureResult, LanguageServerInfo};
use crate::Result;

/// LSPService supervises language server processes for the Observatory editor.
///
/// Contract (mirrors TerminalService streaming):
///
/// ```text
/// EnsureLSP(projectID, workspaceID, languageID, relPath) → SessionInfo + URIs
/// WriteLSP(id, message)  // JSON-RPC body, no Content-Length headers
/// CloseLSP(id)
///   ...lsp.message events {id, message}...
///   ...lsp.exit events {id}...
/// ```
///
/// The frontend @codemirror/lsp-client owns protocol/UI; this service only
/// frames stdio and resolves worktree roots.
///
/// Rust port of Go `app.LSPService` (`frontend/bindings/recur/internal/app/lspservice.ts`).
#[async_trait]
pub trait LspService: Send + Sync {
    /// CloseLSP stops one language server session.
    async fn close_lsp(&self, id: &str) -> Result<()>;

    /// CloseWorkspaceLSP stops every language server for a workspace.
    async fn close_workspace_lsp(&self, workspace_id: &str) -> Result<()>;

    /// EnsureLSP starts or reuses a language server for the file's language.
    /// Soft-degrades (Available=false) when no server is registered or missing.
    async fn ensure_lsp(
        &self,
        project_id: &str,
        workspace_id: &str,
        language_id: &str,
        rel_path: &str,
    ) -> Result<EnsureResult>;

    /// ListLanguageServers returns merged Bundle + builtin language servers for Settings.
    async fn list_language_servers(&self) -> Result<Vec<LanguageServerInfo>>;

    /// WriteLSP sends one JSON-RPC message body to the session.
    async fn write_lsp(&self, id: &str, message: &str) -> Result<()>;
}
