use async_trait::async_trait;
use recur_term::Info as TermInfo;

use super::TerminalDockChrome;
use crate::Result;

/// TerminalService backs the Observatory's terminal dock: real shells running in
/// the workspace's worktree, one PTY per tab.
///
/// The service is a thin shell over term.Manager — it resolves a workspace to a
/// directory and otherwise forwards. The interesting behaviour (output coalescing,
/// ack-based flow control, replay on re-attach) lives in the term package, which is
/// deliberately free of any UI or engine dependency.
///
/// The frontend contract, per tab:
///
/// ```text
/// OpenTerminal      -> session id
/// RestoreWorkspaceTerminals -> prior tabs after an app restart (or empty)
/// ReorderTerminals  -> persist a new tab-strip order
/// AttachTerminal    -> base64 replay to repaint from; output starts streaming
///   ...term.data events...   AckTerminal(n) after each is parsed
/// DetachTerminal      -> streaming stops, shell keeps running
/// InterruptTerminal   -> foreground group dies; shell stays
/// CloseTerminal       -> shell dies
/// term.busy / term.idle  -> a command took / left the foreground (shell stays)
/// ```
///
/// Acking is not optional. A session stops reading its PTY once too many unacked
/// bytes are outstanding, so a client that streams without acking will wedge its
/// own shell after the first few hundred KB.
#[async_trait]
pub trait TerminalService: Send + Sync {
    /// AckTerminal reports that n more bytes have been parsed by the renderer, which is
    /// what releases flow control.
    async fn ack_terminal(&self, id: &str, n: i64) -> Result<()>;

    /// AttachTerminal starts streaming and returns the replay buffer (base64 raw VT) so
    /// the client can repaint the screen it missed.
    async fn attach_terminal(&self, id: &str) -> Result<String>;

    /// ClearTerminal drops the replay buffer after the user clears the screen, so
    /// re-attaching doesn't bring back the output they just wiped.
    async fn clear_terminal(&self, id: &str) -> Result<()>;

    /// CloseTerminal terminates the session's process tree.
    async fn close_terminal(&self, id: &str) -> Result<()>;

    /// DetachTerminal stops streaming without killing the shell.
    async fn detach_terminal(&self, id: &str) -> Result<()>;

    /// InterruptTerminal stops the session's foreground process (the running
    /// dev server) without killing the shell, so the tab stays and can be reused.
    async fn interrupt_terminal(&self, id: &str) -> Result<()>;

    /// ListTerminalDockChrome returns dock layouts that are not the closed default,
    /// so the frontend can restore open/maximized/editor-hidden before first paint.
    async fn list_terminal_dock_chrome(&self) -> Result<Vec<TerminalDockChrome>>;

    /// ListTerminals returns the workspace's live sessions in tab order. The dock calls
    /// this on mount to re-adopt shells that outlived a navigation.
    async fn list_terminals(&self, workspace_id: &str) -> Result<Vec<TermInfo>>;

    /// OpenTerminal starts a shell for the workspace and returns the new session.
    ///
    /// The shell runs in the workspace's git worktree, so a terminal opened next to a
    /// running loop is already sitting on that loop's branch and changes — which is the
    /// whole point of putting one here. Workspaces with an empty WorktreePath
    /// (archived, or not yet created) fall back to the project checkout. A recorded
    /// path whose directory is gone is an error — a shell in the checkout would
    /// target the wrong branch.
    async fn open_terminal(&self, workspace_id: &str, cols: i64, rows: i64) -> Result<TermInfo>;

    /// ReorderTerminals persists a new dock tab-strip order. ids must be a
    /// permutation of the workspace's current session ids.
    async fn reorder_terminals(&self, workspace_id: &str, ids: &[String]) -> Result<()>;

    /// ResizeTerminal updates the PTY window size so full-screen programs redraw.
    async fn resize_terminal(&self, id: &str, cols: i64, rows: i64) -> Result<()>;

    /// RestoreWorkspaceTerminals respawns the workspace's last live tabs after an
    /// app restart: same titles and working directories, with the persisted replay
    /// buffer painted before the new shell's prompt. No-op when sessions are already
    /// running (drawer remount) or nothing was saved. Does not spawn a default tab —
    /// the dock does that when restore returns empty.
    async fn restore_workspace_terminals(
        &self,
        workspace_id: &str,
        cols: i64,
        rows: i64,
    ) -> Result<Vec<TermInfo>>;

    /// SetActiveTerminal records which dock tab is focused for workspaceID. Bare
    /// @terminal and read_terminal with no id resolve to this session.
    async fn set_active_terminal(&self, workspace_id: &str, id: &str) -> Result<()>;

    /// SetTerminalDockChrome records the Observatory dock layout for a workspace.
    /// Disk write is the same debounce as tab snapshots (flushed on quit).
    async fn set_terminal_dock_chrome(
        &self,
        workspace_id: &str,
        chrome: &TerminalDockChrome,
    ) -> Result<()>;

    /// SetTerminalTitle records the OSC / dock tab label for a session so chat
    /// @terminal chips and MCP list_terminals show the name the human sees.
    async fn set_terminal_title(&self, id: &str, title: &str) -> Result<()>;

    /// WriteTerminal forwards keystrokes to the shell.
    async fn write_terminal(&self, id: &str, data: &str) -> Result<()>;
}
