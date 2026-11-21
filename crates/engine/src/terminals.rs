//! The Observatory terminals as the engine reads them: `@terminal`
//! mentions and the terminal host tools reach live shells through this seam
//! instead of reaching into `recur-term` directly.

pub use recur_term::Info as TerminalSession;
pub use recur_term::OpenOptions as SpawnOptions;

/// Called with a session's final info and whether a stop (tool, user, workspace
/// removal, quit) ended it rather than the process itself.
pub type ExitHook = Box<dyn Fn(&TerminalSession, bool) + Send + Sync>;

/// List, focus and plain-text tail of a workspace's PTY sessions, plus the
/// background processes agents start.
pub trait TerminalSource: Send + Sync {
    /// Every session of `workspace_id`, in tab order.
    fn list(&self, workspace_id: &str) -> Vec<TerminalSession>;
    /// The focused session id, or "".
    fn active_id(&self, workspace_id: &str) -> String;
    /// ANSI-stripped recent output. `tail_lines` ≤ 0 means the default tail.
    /// Err is the user-facing text.
    fn read_plain(&self, id: &str, tail_lines: i64) -> Result<String, String>;
    /// A mark of the output produced so far, for [`Self::read_plain_since`].
    fn output_written(&self, _id: &str) -> u64 {
        0
    }
    /// Plain output produced after the `since` mark (at most the longest tail).
    fn read_plain_since(&self, id: &str, _since: u64) -> Result<String, String> {
        self.read_plain(id, 0)
    }
    /// Starts a session in `cwd` with `env` (`KEY=VALUE`) over the host's.
    fn spawn(
        &self,
        _workspace_id: &str,
        _cwd: &str,
        _env: &[String],
        _opts: SpawnOptions,
    ) -> Result<TerminalSession, String> {
        Err("background processes are unavailable here".into())
    }
    /// Terminates a session's process tree and drops it.
    fn stop(&self, _id: &str) -> Result<(), String> {
        Err("background processes are unavailable here".into())
    }
    /// Registers the exit hook; it runs off the async runtime and must not block.
    fn on_exit(&self, _hook: ExitHook) {}
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

    fn output_written(&self, id: &str) -> u64 {
        recur_term::Manager::output_written(self, id).unwrap_or(0)
    }

    fn read_plain_since(&self, id: &str, since: u64) -> Result<String, String> {
        recur_term::Manager::read_plain_since(self, id, since, recur_term::MAX_TAIL_LINES)
            .map_err(|e| e.to_string())
    }

    fn spawn(
        &self,
        workspace_id: &str,
        cwd: &str,
        env: &[String],
        opts: SpawnOptions,
    ) -> Result<TerminalSession, String> {
        self.open_with(workspace_id, cwd, 120, 40, env, opts)
            .map_err(|e| e.to_string())
    }

    fn stop(&self, id: &str) -> Result<(), String> {
        self.close(id).map_err(|e| e.to_string())
    }

    fn on_exit(&self, hook: ExitHook) {
        self.set_on_exit(hook);
    }
}
