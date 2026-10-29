//! The Observatory terminals as the engine reads them (R16): `@terminal`
//! mentions and the list/read_terminal host tools snapshot live shells
//! through this seam instead of reaching into `recur-term` directly.

pub use recur_term::Info as TerminalSession;

/// List, focus and plain-text tail of a workspace's PTY sessions.
pub trait TerminalSource: Send + Sync {
    /// Every session of `workspace_id`, in tab order.
    fn list(&self, workspace_id: &str) -> Vec<TerminalSession>;
    /// The focused session id, or "".
    fn active_id(&self, workspace_id: &str) -> String;
    /// ANSI-stripped recent output. `tail_lines` ≤ 0 means the default tail
    /// (Go `TailPlain` clamps the same way). Err is the user-facing text.
    fn read_plain(&self, id: &str, tail_lines: i64) -> Result<String, String>;
}

impl TerminalSource for recur_term::Manager {
    fn list(&self, workspace_id: &str) -> Vec<TerminalSession> {
        recur_term::Manager::list(self, workspace_id)
    }

    fn active_id(&self, workspace_id: &str) -> String {
        recur_term::Manager::active_id(self, workspace_id)
    }

    fn read_plain(&self, id: &str, tail_lines: i64) -> Result<String, String> {
        let n = usize::try_from(tail_lines).unwrap_or(0);
        recur_term::Manager::read_plain(self, id, n).map_err(|e| e.to_string())
    }
}
