use async_trait::async_trait;

use super::{EditorSession, EditorSessionPatch};
use crate::Result;

/// EditorSessionService persists per-workspace editor UI state — open tabs,
/// selection, scratch docs, unsaved ("stashed") buffers, per-file caret /
/// scroll / Code|Preview|Diff, TextMate language-mode overrides, whether
/// the Observatory main column is Editor or Review, and left/right pane
/// kind plus visibility — as one gzipped JSON snapshot under the workspace
/// data dir:
///
/// ```text
/// <DataDir>/workspaces/<id>/editor-session.json.gz
/// ```
///
/// The state rides the same lifecycle as artifacts/attachments: workspace
/// deletion sweeps the directory,
/// and the write is a plain atomic temp+rename with no quota ceiling to trim to.
///
/// The wire protocol is patch-based so typing doesn't ship the whole session
/// over the bridge every debounce: UpdateEditorSession carries only what
/// changed (optional meta, upserted buffers/scratch docs keyed by path, and
/// explicit drops) and the merge happens here. Because absence in a patch means
/// "unchanged", deletions MUST be explicit: the UI sends a drop when a
/// save lands or a tab closes.
#[async_trait]
pub trait EditorSessionService: Send + Sync {
    /// ClearEditorSession deletes the stored session (last tab closed / discarded)
    /// and drops the in-memory cache entry so draft text is not retained.
    async fn clear_editor_session(&self, workspace_id: &str) -> Result<()>;

    /// GetEditorSession returns the stored session for a workspace, or nil when
    /// none exists. Called once when the Observatory opens a workspace root.
    async fn get_editor_session(&self, workspace_id: &str) -> Result<Option<EditorSession>>;

    /// UpdateEditorSession merges one patch into the stored session and writes it
    /// through. A session that ends up empty (no tabs, no buffers, no scratch, no
    /// language-mode maps, editor column) is deleted — an explicitly closed-out
    /// editor must not resurrect on restart. Language overrides and the Review
    /// column outlive the tab strip: closing the last tab keeps the file if
    /// syntax maps remain or Review is showing.
    async fn update_editor_session(
        &self,
        workspace_id: &str,
        patch: &EditorSessionPatch,
    ) -> Result<()>;
}
