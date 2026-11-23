//! The deterministic `run` verb: one command in the worktree, exit 0 passes.
//! A failure is a phase error (retries, `on_fail`), or with `on_reject` sends
//! the work back with the output tail for the next implementer turn.

use std::path::Path;
use std::time::Duration;

use recur_bundles::{Command, LoopPhase, RunContext, RunCtx};
use recur_domain::WorkspaceState;
use tokio_util::sync::CancellationToken;

use crate::loops::Walk;
use crate::loops::failure::LoopError;
use crate::text::html_escape;

/// Wall-clock limit when neither the phase nor the policy sets `timeout`.
pub const RUN_DEFAULT_TIMEOUT: Duration = Duration::from_secs(30 * 60);

const TAIL_LINES: usize = 40;

/// The last [`TAIL_LINES`] lines: stdout's tail then stderr's, with stderr
/// held to a quarter when stdout needs the room, so progress noise on stderr
/// (cargo's `Compiling`/`Running`) cannot push out stdout's failures.
pub fn output_tail(stdout: &str, stderr: &str) -> String {
    let out: Vec<&str> = stdout.trim().lines().collect();
    let err: Vec<&str> = stderr.trim().lines().collect();
    let err_n = err
        .len()
        .min((TAIL_LINES / 4).max(TAIL_LINES.saturating_sub(out.len())));
    let out_n = out.len().min(TAIL_LINES - err_n);
    out[out.len() - out_n..]
        .iter()
        .chain(&err[err.len() - err_n..])
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}

/// What a `run` phase runs, for labels: its script path, else its first line.
pub fn command_label(ph: &LoopPhase) -> &str {
    if ph.script.is_empty() {
        ph.command.lines().next().unwrap_or_default()
    } else {
        &ph.script
    }
}

impl Walk {
    /// Runs the phase command. A failure leaves its report in
    /// `pending_rejection` for `on_reject`.
    pub(crate) async fn run_command_phase(
        &mut self,
        cancel: &CancellationToken,
        ph: &LoopPhase,
    ) -> Result<(), LoopError> {
        let label = command_label(ph).to_owned();
        self.set_state(WorkspaceState::Verifying, &format!("running {}", ph.id));
        let cmd = Command {
            id: ph.id.clone(),
            body: ph.command.clone(),
            bundle_dir: self.guard.loop_def.bundle_dir.clone(),
            ..Command::default()
        };
        let rc = RunContext {
            work_dir: self.worktree.clone(),
            project_directory: self.worktree.clone(),
            ..RunContext::default()
        };
        let timeout = self.phase_timeout(ph).unwrap_or(RUN_DEFAULT_TIMEOUT);
        let ctx = RunCtx::with_cancel(cancel.clone()).with_timeout(timeout);
        let (code, tail, why) = match recur_bundles::run(&ctx, &cmd, rc).await {
            Ok(r) => (
                r.exit_code,
                output_tail(&r.stdout, &r.stderr),
                format!("exited {}", r.exit_code),
            ),
            Err(e) => (
                -1,
                output_tail(&e.partial.stdout, &e.partial.stderr),
                e.message,
            ),
        };
        let mut html = format!(
            "Ran <code>{}</code>: {}.",
            html_escape(&label),
            html_escape(&why)
        );
        if !tail.is_empty() {
            html.push_str(&format!("<pre><code>{}</code></pre>", html_escape(&tail)));
        }
        self.emit_feed("MA", "Maestro", &html);
        if code == 0 {
            self.pending_rejection.clear();
            return Ok(());
        }
        let rerun = if ph.script.is_empty() {
            String::new()
        } else {
            let path = Path::new(&self.guard.loop_def.bundle_dir).join(&ph.script);
            format!(
                " Rerun it with `{}` from the worktree root.",
                path.display()
            )
        };
        self.pending_rejection = format!(
            "The {} check `{label}` failed ({why}). Make it pass.{rerun}\n\nOutput tail:\n```\n{tail}\n```",
            ph.id
        );
        Err(LoopError::msg(format!("{label}: {why}")))
    }
}
