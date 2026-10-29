use async_trait::async_trait;

use super::{IssuesView, ShellState};
use crate::Result;

/// ShellService is the read/write surface for ShellState. SetShellState updates
/// the in-memory snapshot immediately and coalesces disk writes; Flush (window
/// close) commits any pending snapshot so a quit inside the debounce window
/// still restores the last view.
#[async_trait]
pub trait ShellService: Send + Sync {
    /// Flush writes any coalesced snapshot to disk. Called from the window-close
    /// hook so ⌘Q does not depend on a frontend pagehide IPC. The lock is held
    /// across the write so overlapping flushes cannot share shell.json.tmp. A
    /// failed write keeps pending and retries with exponential backoff; only the
    /// first failure (and later recovery) is logged.
    async fn flush(&self) -> Result<()>;

    /// GetExpandedSchedules returns which schedule dashboard rows are expanded.
    /// Lives next to shell.json so accordion state survives a webview storage wipe.
    async fn get_expanded_schedules(&self) -> Result<Vec<String>>;

    /// GetIssuesView returns the saved Issues view, or a zero value when none
    /// exists or the file is corrupt.
    async fn get_issues_view(&self) -> Result<IssuesView>;

    /// GetShellState returns the last persisted shell, or a zero value when none
    /// exists (first run / corrupt file). The frontend treats an empty View as
    /// "stay on the default Inbox" unless first-run routing sends Welcome.
    async fn get_shell_state(&self) -> Result<ShellState>;

    /// SetExpandedSchedules records expanded schedule dashboard rows.
    async fn set_expanded_schedules(&self, ids: &[String]) -> Result<()>;

    /// SetIssuesView records the Issues view next to shell.json.
    async fn set_issues_view(&self, v: &IssuesView) -> Result<()>;

    /// SetShellState records the current shell. Unknown views (including welcome)
    /// are ignored so a future client cannot wipe a good shell.json. The disk write
    /// is coalesced; call Flush on shutdown so the last snapshot is durable.
    async fn set_shell_state(&self, st: &ShellState) -> Result<()>;
}
