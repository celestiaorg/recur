//! What a pane needs from the session layer: the `TerminalService` calls a pane makes
//! (`AttachTerminal`, `DetachTerminal`, `AckTerminal`, `WriteTerminal`, `ResizeTerminal`,
//! `ClearTerminal`). Implemented for [`recur_term::Manager`]; the app can wrap the services layer
//! instead.

/// Session calls a terminal pane makes. They may block briefly (a PTY write, an ioctl), so the
/// view runs them off the UI thread.
pub trait TerminalBackend: Send + Sync + 'static {
    /// Starts streaming and returns the raw VT replay to repaint from.
    fn attach(&self, id: &str) -> Result<Vec<u8>, String>;
    /// Stops streaming without touching the shell.
    fn detach(&self, id: &str);
    /// The client parsed `n` more bytes.
    fn ack(&self, id: &str, n: usize);
    /// Keystrokes, pasted text, mouse reports and terminal replies.
    fn write(&self, id: &str, data: &[u8]);
    fn resize(&self, id: &str, cols: u16, rows: u16);
    /// Drops the replay ring after the user clears the screen.
    fn clear_replay(&self, id: &str);
}

// The pane ignored every failure but attach (`.catch(() => {})`): the only realistic cause is the
// session already being gone.
impl TerminalBackend for recur_term::Manager {
    fn attach(&self, id: &str) -> Result<Vec<u8>, String> {
        recur_term::Manager::attach(self, id).map_err(|e| e.to_string())
    }

    fn detach(&self, id: &str) {
        let _ = recur_term::Manager::detach(self, id);
    }

    fn ack(&self, id: &str, n: usize) {
        let _ = recur_term::Manager::ack(self, id, n);
    }

    fn write(&self, id: &str, data: &[u8]) {
        let _ = recur_term::Manager::write(self, id, data);
    }

    fn resize(&self, id: &str, cols: u16, rows: u16) {
        let _ = recur_term::Manager::resize(self, id, i32::from(cols), i32::from(rows));
    }

    fn clear_replay(&self, id: &str) {
        let _ = recur_term::Manager::clear_replay(self, id);
    }
}
