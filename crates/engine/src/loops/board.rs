//! The board: the first coding pass of a converge entry works a structured plan
//! continuously. Every ready step runs in a child workspace (fan-out, grouped
//! by file overlap), a child merges back the moment it settles, whatever that
//! unblocked starts at once while slower siblings keep running, and the Maestro
//! may replan off each handoff. The parent codes only to resolve a merge
//! conflict or, as a fallback, to finish steps a child failed. The review then
//! judges the aggregate diff.

use std::collections::HashSet;

use recur_domain::{Plan, STEP_DOING, STEP_DONE, STEP_PENDING, STEP_SKIPPED, WorkspaceState};
use tokio_util::sync::CancellationToken;

use crate::loops::Walk;
use crate::loops::failure::LoopError;
use crate::loops::fanout::in_flight_children;

/// Bounds the events (handoff, conflict, fallback) one board pass handles:
/// a safety net, not a budget a healthy plan reaches.
pub const BOARD_MAX_ROUNDS: usize = 64;

/// The resting phase of a board that hit [`BOARD_MAX_ROUNDS`] (R14).
pub const BOARD_CAP_PHASE: &str = "board event cap";

pub fn has_pending_steps(p: &Plan) -> bool {
    p.steps
        .iter()
        .any(|s| s.status == STEP_PENDING || s.status == STEP_DOING)
}

impl Walk {
    /// Drives the board for the entry's first coding pass. `ran` = it did
    /// this iteration's coding (the caller reviews the aggregate diff);
    /// a state = the parent stopped or paused.
    pub(crate) async fn run_board(
        &mut self,
        cancel: &CancellationToken,
        iter: i64,
    ) -> Result<(bool, Option<WorkspaceState>), LoopError> {
        if !self.fan_out_eligible() {
            return Ok((false, None));
        }
        let out = self.board_pass(cancel, iter).await;
        // Success leaves nothing in flight; error/stop/pause must not leak
        // a sibling whose dispatch outlives this walk.
        // simplify: retries > 0 re-enter and merge these as failed; to let a
        // sibling finish across a failed attempt, skip this while
        // run_loop_phase still has tries.
        self.stop_in_flight_children().await;
        out
    }

    async fn board_pass(
        &mut self,
        cancel: &CancellationToken,
        iter: i64,
    ) -> Result<(bool, Option<WorkspaceState>), LoopError> {
        self.start_board_handoff();
        // Child ids whose child failed once: the parent finishes them.
        let mut exclude: HashSet<String> = HashSet::new();
        let mut ran = false;
        for _ in 0..self.board_max_rounds {
            let res = self.fan_out_steps(cancel, &mut exclude).await;
            if res.stopped {
                return Ok((ran, Some(WorkspaceState::Stopped)));
            }
            if res.merged > 0 {
                ran = true;
            }
            // Record the handoffs on the parent and let the Maestro revise
            // the rest of the board while the other children keep running.
            if !res.handoffs.is_empty() {
                self.record_handoff("MA", "Maestro", &res.handoffs.join("\n"));
                if self.checkpoint(cancel).await {
                    let h = self.last_handoff.clone();
                    self.run_replanner(cancel, &h).await;
                }
            }
            if !res.conflicts.is_empty() {
                self.pending_conflicts = res.conflicts;
                self.set_state(
                    WorkspaceState::Coding,
                    &format!("resolving merge conflicts (iteration {iter})"),
                );
                self.set_iteration(iter);
                self.run_coder(cancel, iter, Vec::new()).await?;
                if let Some(git) = &self.git
                    && git.merge_head(&self.worktree).await
                {
                    return Err(LoopError::msg(
                        "merge still in progress after the resolve turn",
                    ));
                }
                self.mark_conflicts_resolved().await;
                ran = true;
                continue;
            }
            if !res.handoffs.is_empty() {
                continue;
            }
            let Some(plan) = self.eng.store.workspace(&self.id).and_then(|w| w.plan) else {
                return Ok((ran, None));
            };
            if !has_pending_steps(&plan) {
                if ran {
                    self.emit_feed(
                        "MA",
                        "Maestro",
                        "Every step landed — reviewing the aggregate diff.",
                    );
                }
                return Ok((ran, None));
            }
            // Pending steps but nothing more can fan out: one plain Coder
            // turn over the whole plan finishes what is left.
            let why = if res.no_slot {
                "no free loop slot for a child workspace"
            } else if !exclude.is_empty() {
                "a child workspace failed on them"
            } else {
                "steps blocked by unknown dependencies"
            };
            self.emit_feed(
                "MA",
                "Maestro",
                &format!("Finishing the remaining steps here — {why}."),
            );
            self.set_state(WorkspaceState::Coding, &format!("iteration {iter}"));
            self.set_iteration(iter);
            self.run_coder(cancel, iter, Vec::new()).await?;
            ran = true;
            // The parent's own handoff may revise the board too.
            if !self.last_handoff.is_empty() && self.checkpoint(cancel).await {
                let h = self.last_handoff.clone();
                if self.run_replanner(cancel, &h).await {
                    continue;
                }
            }
            return Ok((true, None));
        }
        self.set_state(WorkspaceState::Paused, BOARD_CAP_PHASE);
        self.emit_feed(
            "MA",
            "Maestro",
            "Board paused — too many events with work still open.",
        );
        Ok((ran, Some(WorkspaceState::Paused)))
    }

    /// Resets the notes for a new coding round. A resumed board keeps (or
    /// rebuilds) its landed children's notes so an empty sibling cannot
    /// drop them.
    fn start_board_handoff(&mut self) {
        if self.board_pass_in_progress() {
            if self.board_handoff.trim().is_empty() {
                self.board_handoff = self.landed_board_handoff();
            }
            return;
        }
        self.reset_board_handoff();
    }

    fn board_pass_in_progress(&self) -> bool {
        let Some(plan) = self.eng.store.workspace(&self.id).and_then(|w| w.plan) else {
            return false;
        };
        let (ids, _) = in_flight_children(&plan);
        if !ids.is_empty() {
            return true;
        }
        plan.steps
            .iter()
            .any(|s| s.status == STEP_DONE || s.status == STEP_SKIPPED)
    }

    fn landed_board_handoff(&self) -> String {
        let mut parts = Vec::new();
        for c in self.eng.store.workspaces() {
            if c.parent_id != self.id {
                continue;
            }
            let Some(fo) = &c.fan_out else {
                continue;
            };
            if fo.outcome != "merged" && !fo.outcome.starts_with("conflict") {
                continue;
            }
            let note = fo.note.trim();
            if note.is_empty() {
                continue;
            }
            let ids = c.parent_step_ids.join(", ");
            if ids.is_empty() {
                parts.push(note.to_owned());
            } else {
                parts.push(format!("{ids}: {note}"));
            }
        }
        parts.join("\n")
    }
}
