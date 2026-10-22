//! `LSPService` (Go `lsp_service.go`): language servers for the editor over
//! `recur_lsp`. This layer resolves the worktree root and the bundle roots;
//! framing and supervision live in the lsp crate.

use async_trait::async_trait;

use super::rt::{blocking, rt};
use super::{AppServices, err};
use crate::api::{EnsureResult, LanguageServerInfo, LspService};
use crate::{Error, Result};

impl AppServices {
    /// Settings-disabled servers and bundle `[[languageServer]]` overrides,
    /// re-read on every ensure and listing.
    fn refresh_lsp_registry(&self) {
        recur_lsp::service::refresh_registry(
            &self.lsp,
            &self.store().settings().disabled_language_servers,
            &self.config.bundles,
        );
    }

    /// The code browser's worktree-then-repo precedence, with this service's
    /// own error text.
    fn lsp_root(&self, project_id: &str, workspace_id: &str) -> Result<String> {
        let (mut project_id, mut wt) = (project_id.to_owned(), String::new());
        if !workspace_id.is_empty() {
            let ws = self
                .store()
                .workspace(workspace_id)
                .ok_or_else(|| Error::new("workspace not found"))?;
            wt = ws.worktree_path;
            if project_id.is_empty() {
                project_id = ws.project_id;
            }
        }
        let mut repo = String::new();
        if !project_id.is_empty() {
            repo = self
                .store()
                .project(&project_id)
                .ok_or_else(|| Error::new("project not found"))?
                .repo_path;
        }
        let (root, _) = recur_code::resolve_root_info(&repo, &wt).map_err(err)?;
        Ok(root.to_string_lossy().into_owned())
    }
}

#[async_trait]
impl LspService for AppServices {
    async fn close_lsp(&self, id: &str) -> Result<()> {
        rt!(self, id => |s| {
            s.lsp.close(&id);
            Ok(())
        })
    }

    async fn close_workspace_lsp(&self, workspace_id: &str) -> Result<()> {
        rt!(self, workspace_id => |s| {
            s.lsp.close_workspace(&workspace_id);
            Ok(())
        })
    }

    async fn ensure_lsp(
        &self,
        project_id: &str,
        workspace_id: &str,
        language_id: &str,
        rel_path: &str,
    ) -> Result<EnsureResult> {
        rt!(self, project_id, workspace_id, language_id, rel_path => |s| {
            let me = s.clone();
            blocking(move || {
                me.refresh_lsp_registry();
                Ok(())
            })
            .await?;
            let root = s.lsp_root(&project_id, &workspace_id)?;
            recur_lsp::service::ensure_lsp(&s.lsp, &workspace_id, &root, &language_id, &rel_path)
                .await
                .map_err(Error::new)
        })
    }

    async fn list_language_servers(&self) -> Result<Vec<LanguageServerInfo>> {
        rt!(self, => |s| {
            blocking(move || {
                s.refresh_lsp_registry();
                Ok(recur_lsp::service::list_language_servers(&s.lsp))
            })
            .await
        })
    }

    async fn write_lsp(&self, id: &str, message: &str) -> Result<()> {
        rt!(self, id, message => |s| {
            recur_lsp::service::write_lsp(&s.lsp, &id, &message)
                .await
                .map_err(Error::new)
        })
    }
}
