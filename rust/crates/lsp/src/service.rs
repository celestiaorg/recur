//! The editor-facing LSP API (Go `app/lsp_service.go`). `recur-services`
//! resolves the workspace root (worktree, else repo: Go
//! `code.ResolveRootInfo`) and the bundle roots, then calls these.
//!
//! Contract: `ensure_lsp` → [`EnsureResult`]; `write_lsp(id, body)` sends a
//! JSON-RPC body without headers; `close_lsp(id)`; the manager's emitter
//! delivers `lsp.message {id, message}` and `lsp.exit {id}`. The editor's
//! LSP client owns protocol and UI; this layer frames stdio and resolves
//! roots.

use serde::{Deserialize, Serialize};

use recur_bundles::DiscoverRoots;

use crate::manager::{Manager, SessionInfo};
use crate::uri::{canon_path, clean, path_to_uri};

/// One builtin or bundle language server for Settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LanguageServerInfo {
    pub id: String,
    /// `None` is Go's nil slice (`null`).
    pub languages: Option<Vec<String>>,
    pub command: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub managed: String,
    /// `bundle` or `builtin`.
    pub source: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub bundle_name: String,
    pub available: bool,
}

/// The answer to opening a file that may need a language server.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EnsureResult {
    #[serde(flatten)]
    pub session: SessionInfo,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub file_uri: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub language_id: String,
    /// The other servers that claim the language.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<SessionInfo>,
}

/// Reloads Settings-disabled servers and bundle `[[languageServer]]`
/// declarations. A discovery failure keeps the previous bundle list.
pub fn refresh_registry(mgr: &Manager, disabled: &[String], roots: &DiscoverRoots) {
    mgr.registry().set_disabled_servers(disabled);
    if let Ok(cat) = recur_bundles::discover_catalog(roots) {
        mgr.registry().set_bundle_servers(&cat.language_servers);
    }
}

/// Starts or reuses the servers for a file's language under `root`. Soft
/// degrades (`available: false`) when none is registered or installed.
pub async fn ensure_lsp(
    mgr: &Manager,
    workspace_id: &str,
    root: &str,
    language_id: &str,
    rel_path: &str,
) -> Result<EnsureResult, String> {
    let rel = rel_path.trim();
    let mut lang = language_id.trim().to_owned();
    let root = canon_path(root);
    let infos = mgr.ensure_all(workspace_id, &root, &lang, rel).await?;
    if lang.is_empty()
        && let Some(first) = mgr.registry().lookup_all("", rel).into_iter().next()
        && let Some(l) = first.languages.first()
    {
        lang.clone_from(l);
    }
    let mut infos = infos.into_iter();
    let session = infos.next().unwrap_or_else(|| SessionInfo {
        reason: format!("no language server registered for {lang}"),
        ..Default::default()
    });
    let mut out = EnsureResult {
        session,
        language_id: lang,
        extra: infos.collect(),
        ..Default::default()
    };
    if !rel.is_empty() {
        let abs = safe_join(&root, rel)?;
        out.file_uri = path_to_uri(&canon_path(&abs));
    }
    Ok(out)
}

/// Merged bundle + builtin servers for Settings. Availability is PATH
/// presence only: managed servers may still install on ensure.
pub fn list_language_servers(mgr: &Manager) -> Vec<LanguageServerInfo> {
    mgr.registry()
        .all_servers()
        .into_iter()
        .map(|s| LanguageServerInfo {
            available: s.available(),
            languages: (!s.languages.is_empty()).then_some(s.languages),
            id: s.key,
            command: s.command,
            managed: s.managed,
            source: s.source,
            bundle_name: s.bundle_name,
        })
        .collect()
}

/// Sends one JSON-RPC body to the session.
pub async fn write_lsp(mgr: &Manager, id: &str, message: &str) -> Result<(), String> {
    if id.trim().is_empty() {
        return Err("missing lsp session id".into());
    }
    mgr.write(id, message).await
}

/// Go `code.SafeJoin`: `rel` (forward slashes) under `root`, refusing `..`
/// and anything that escapes.
pub fn safe_join(root: &str, rel: &str) -> Result<String, String> {
    let root_abs = abs(root);
    let rel = rel.strip_prefix('/').unwrap_or(rel);
    if rel.is_empty() || rel == "." {
        return Ok(root_abs);
    }
    if rel.contains("..") {
        return Err("path escapes root".into());
    }
    let full = clean(&format!("{root_abs}/{rel}"));
    if full != root_abs && !full.starts_with(&format!("{root_abs}/")) {
        return Err("path escapes root".into());
    }
    Ok(full)
}

fn abs(p: &str) -> String {
    if p.starts_with('/') {
        return clean(p);
    }
    let cwd = std::env::current_dir()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    clean(&format!("{cwd}/{p}"))
}
