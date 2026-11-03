use async_trait::async_trait;
use recur_domain::PermissionRequest;

use crate::Result;

/// PermissionsService answers the tool-permission prompts agents raise while
/// Settings.BypassPermissions is off. Unlike an escalation these are not
/// persisted: an agent subprocess is holding an open request while it waits, so
/// nothing here outlives the turn that asked.
#[async_trait]
pub trait PermissionsService: Send + Sync {
    /// AnswerPermission delivers the human's verdict to the waiting agent. It errors
    /// when the request is already gone (answered in another window, timed out, or
    /// its turn ended), so the UI can drop a stale prompt instead of appearing to
    /// have done something.
    async fn answer_permission(
        &self,
        id: &str,
        allow: bool,
        reason: &str,
        remember: bool,
    ) -> Result<()>;

    /// ListPendingPermissions rebuilds the prompt queue after a UI reload, which
    /// matters because the requests only ever arrived as events.
    async fn list_pending_permissions(&self) -> Result<Vec<PermissionRequest>>;
}
