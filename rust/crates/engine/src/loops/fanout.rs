//! Fan-out (Go `fanout.go`): every ready plan step runs in a CHILD
//! WORKSPACE branched off this walk's branch, converging under the parent's
//! policy, and merges back the moment it settles. Ready steps are grouped by
//! file overlap (sharers ride together; file-less steps share one child).
//! Children never fan out. A WAVE is the set of children started together:
//! a record of a dispatch, not a barrier.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::time::Duration;

use recur_agent::display::PLAN_MAX_PERSIST_CHARS;
use recur_bundles::STEP_LOOP_ID;
use recur_domain::{
    ChildFanOut, FEED_CARD_FAN_OUT, FeedItem, Group, Plan, PlanStep, STEP_DOING, STEP_DONE,
    STEP_PENDING, STEP_SKIPPED, Workspace, WorkspaceState, finding_loc,
};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::Engine;
use crate::attention::chat_inbox::loop_settled;
use crate::lifecycle::PHASE_INTERRUPTED;
use crate::loops::Walk;
use crate::publish::{run_progress, status_label};
use crate::text::{html_escape, truncate};

/// How often a waiting parent re-checks its children.
pub const DEFAULT_FAN_OUT_POLL: Duration = Duration::from_secs(2);

fn null_if_empty<S: serde::Serializer>(v: &[String], s: S) -> Result<S::Ok, S::Error> {
    if v.is_empty() {
        s.serialize_none()
    } else {
        v.serialize(s)
    }
}

/// One child row of a wave card (`domain.FeedCardFanOut`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct FanOutChild {
    pub id: String,
    pub title: String,
    /// Go's nil slice: `null` when empty.
    #[serde(serialize_with = "null_if_empty")]
    pub steps: Vec<String>,
    /// A chat child's chat mode chip.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mode: String,
    /// "" while running; merged | failed | conflict: …
    #[serde(skip_serializing_if = "String::is_empty")]
    pub state: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// A wave card payload.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct FanOutWave {
    pub wave: i64,
    /// A chat-child card header; loops show "Wave N".
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// Parent steps whose landing started this wave.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
    pub children: Vec<FanOutChild>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub done: bool,
}

/// What one fan-out call ended on.
#[derive(Debug, Default)]
pub(crate) struct FanOutResult {
    pub merged: i64,
    /// Paths the last merge left conflicted (merge in progress).
    pub conflicts: Vec<String>,
    pub stopped: bool,
    /// Ready steps but no loop slot for a child.
    pub no_slot: bool,
    /// Merged children's done notes ("S2: …") for the replan turn.
    pub handoffs: Vec<String>,
}

/// Pending steps whose dependencies are settled, minus excluded child ids
/// (and stale step ids).
pub fn ready_steps(p: &Plan, exclude: &HashSet<String>) -> Vec<PlanStep> {
    p.steps
        .iter()
        .filter(|s| {
            p.ready(s)
                && !exclude.contains(&s.id)
                && (s.workspace.is_empty() || !exclude.contains(&s.workspace))
        })
        .cloned()
        .collect()
}

/// Partitions steps into children: steps sharing any declared file join one
/// group (transitively); file-less steps share one group. Groups keep plan
/// order by their first step.
pub fn group_by_files(steps: &[PlanStep]) -> Vec<Vec<PlanStep>> {
    let n = steps.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    fn union(parent: &mut [usize], a: usize, b: usize) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        parent[ra] = rb;
    }
    let mut owner: HashMap<String, usize> = HashMap::new();
    let mut no_files: Option<usize> = None;
    for (i, s) in steps.iter().enumerate() {
        if s.files.is_empty() {
            match no_files {
                None => no_files = Some(i),
                Some(j) => union(&mut parent, i, j),
            }
            continue;
        }
        for f in &s.files {
            let f = f.trim().to_owned();
            match owner.get(&f) {
                Some(&j) => union(&mut parent, i, j),
                None => {
                    owner.insert(f, i);
                }
            }
        }
    }
    let mut by_root: BTreeMap<usize, Vec<PlanStep>> = BTreeMap::new();
    for (i, s) in steps.iter().enumerate() {
        let r = find(&mut parent, i);
        by_root.entry(r).or_default().push(s.clone());
    }
    // Go sorts the roots; a BTreeMap iterates them ascending.
    by_root.into_values().collect()
}

/// Child workspace id → the steps it owns (status doing + a child id), in
/// plan order.
pub fn in_flight_children(p: &Plan) -> (Vec<String>, HashMap<String, Vec<PlanStep>>) {
    let mut ids = Vec::new();
    let mut steps: HashMap<String, Vec<PlanStep>> = HashMap::new();
    for s in &p.steps {
        let id = s.workspace.trim();
        if s.status != STEP_DOING || id.is_empty() {
            continue;
        }
        if !steps.contains_key(id) {
            ids.push(id.to_owned());
        }
        steps.entry(id.to_owned()).or_default().push(s.clone());
    }
    (ids, steps)
}

pub fn step_ids(steps: &[PlanStep]) -> String {
    steps
        .iter()
        .map(|s| s.id.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// A child that did not finish its steps (failed, stopped, or idled on a
/// broken guard).
pub fn child_incomplete(ws: &Workspace) -> bool {
    match ws.state {
        WorkspaceState::Failed | WorkspaceState::Stopped => true,
        WorkspaceState::Paused => ws.phase == "loop refs missing" || ws.phase == "phase missing",
        _ => false,
    }
}

/// The child's brief: its steps, their files, and the parent plan as
/// context. Pre-approved so the child never gates on a human.
pub fn child_step_plan(parent: &Workspace, steps: &[PlanStep]) -> Plan {
    let mut b = String::new();
    b.push_str("You are working a slice of a larger plan in your own workspace, branched from the parent's ");
    b.push_str("branch. Do only the steps below. Sibling workspaces are working other steps at the same time, ");
    b.push_str("so stay inside the files listed; if a step cannot be done without touching another file, do ");
    b.push_str("the minimum and say so in your done notes.\n\nYour steps:\n");
    let mut files: BTreeSet<String> = BTreeSet::new();
    let mut child_steps = Vec::with_capacity(steps.len());
    for (i, s) in steps.iter().enumerate() {
        b.push_str(&format!("- {}: {}\n", s.id, s.title));
        for f in &s.files {
            files.insert(f.clone());
        }
        child_steps.push(PlanStep {
            id: format!("S{}", i + 1),
            title: format!("{}: {}", s.id, s.title),
            status: STEP_PENDING.into(),
            files: s.files.clone(),
            ..PlanStep::default()
        });
    }
    if !files.is_empty() {
        b.push_str(&format!(
            "Files you own: {}\n",
            files.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    b.push_str(&format!("\nParent task: {}\n", parent.goal.trim()));
    if let Some(p) = &parent.plan {
        let sum = p.summary.trim();
        if !sum.is_empty() {
            b.push_str(&format!("Parent plan: {sum}\n"));
        }
        let body = p.body.trim();
        if !body.is_empty() {
            b.push_str("\nParent plan detail (context only — do not work its other steps):\n");
            b.push_str(body);
        }
    }
    Plan {
        summary: group_title(steps),
        body: b,
        steps: child_steps,
        source: "loop-planner".into(),
        approved: true,
        ..Plan::default()
    }
}

fn group_title(steps: &[PlanStep]) -> String {
    if steps.len() > 1 {
        format!("{} (+{} more)", steps[0].title, steps.len() - 1)
    } else {
        steps[0].title.clone()
    }
}

impl Engine {
    /// Whether a child's walk is over: no task and a terminal or idle row.
    /// Queued, escalated or mid-walk children are not settled.
    pub fn child_settled(&self, id: &str) -> bool {
        let Some(ws) = self.store.workspace(id) else {
            return true;
        };
        if self.running_loop_by_id(id).is_some() {
            return false;
        }
        match ws.state {
            WorkspaceState::Stopped
            | WorkspaceState::Failed
            | WorkspaceState::Merged
            | WorkspaceState::PrOpen => true,
            WorkspaceState::Paused => {
                loop_settled(&ws.state, &ws.phase)
                    || ws.phase == "loop refs missing"
                    || ws.phase == "phase missing"
            }
            _ => false,
        }
    }

    /// Marks a merged child and archives it (history kept, worktree torn
    /// down in the background). A refusal is logged, not fatal.
    pub async fn finish_child(&self, id: &str) {
        if let Some(updated) = self.store.update_workspace(id, |w| {
            w.state = WorkspaceState::Merged;
            w.status_label = status_label(&WorkspaceState::Merged);
            w.phase = "merged into parent".into();
            w.group = Group::Ready;
            w.checkpoint = None;
        }) {
            self.publish_loop_updated(
                &updated.id,
                updated.state.clone(),
                &updated.phase,
                updated.iteration,
            );
        }
        if let Err(e) = self.remove_workspace(id).await {
            tracing::warn!("fan-out: archive child {id}: {e}");
        }
    }

    /// The parent waits this long between child checks.
    pub(crate) fn fan_out_poll(&self) -> Duration {
        *self.fan_out_poll.lock()
    }
}

impl Walk {
    pub(crate) fn fan_out_eligible(&self) -> bool {
        self.plan_structured && self.parent_id.is_empty()
    }

    /// This parent's children of wave `n`, in creation order (archived rows
    /// included: merged children are kept for history).
    fn wave_children(&self, n: i64) -> Vec<Workspace> {
        self.eng
            .store
            .workspaces()
            .into_iter()
            .filter(|c| c.parent_id == self.id && c.fan_out.as_ref().is_some_and(|f| f.wave == n))
            .collect()
    }

    /// (Re)publishes wave `n`'s card under its stable id, rebuilt from the
    /// children's rows.
    pub(crate) fn emit_wave_card(&self, n: i64) {
        let kids = self.wave_children(n);
        if n <= 0 || kids.is_empty() {
            return;
        }
        let mut w = FanOutWave {
            wave: n,
            after: kids[0]
                .fan_out
                .as_ref()
                .map(|f| f.after.clone())
                .unwrap_or_default(),
            done: true,
            ..FanOutWave::default()
        };
        let mut merged = 0;
        for c in &kids {
            let fo = c.fan_out.clone().unwrap_or_default();
            let row = FanOutChild {
                id: c.id.clone(),
                title: c.goal.clone(),
                steps: c.parent_step_ids.clone(),
                state: fo.outcome,
                note: fo.note,
                ..FanOutChild::default()
            };
            if row.state.is_empty() {
                w.done = false;
            }
            if row.state == "merged" {
                merged += 1;
            }
            w.children.push(row);
        }
        let Ok(data) = serde_json::to_string(&w) else {
            return;
        };
        let mut b = format!("Wave {n}: {} child workspace(s)", kids.len());
        if !w.after.is_empty() {
            b.push_str(&format!(" after {}", html_escape(&w.after.join(", "))));
        }
        if w.done {
            b.push_str(&format!(", {merged} merged"));
        }
        b.push_str(" — ");
        for (i, c) in w.children.iter().enumerate() {
            if i > 0 {
                b.push_str(" · ");
            }
            b.push_str(&html_escape(&c.steps.join(", ")));
        }
        self.eng.emit_workspace_card(
            &self.id,
            FeedItem {
                id: format!("fanout-{}-{n}", self.id),
                agent_code: "MA".into(),
                name: "Maestro".into(),
                text: b,
                chat_code: self.chat_code(),
                card: FEED_CARD_FAN_OUT.into(),
                data,
                ..FeedItem::default()
            },
        );
    }

    /// Records what the parent made of a settled child on its row (the wave
    /// card reads it) before the child is archived.
    fn set_child_outcome(&self, child_id: &str, outcome: &str, note: &str) {
        let (outcome, note) = (outcome.to_owned(), truncate(note.trim(), 300));
        self.eng.store.update_workspace(child_id, |w| {
            let fo = w.fan_out.get_or_insert_with(ChildFanOut::default);
            fo.outcome = outcome;
            fo.note = note;
        });
    }

    fn child_wave(&self, child_id: &str) -> i64 {
        self.eng
            .store
            .workspace(child_id)
            .and_then(|c| c.fan_out)
            .map_or(0, |f| f.wave)
    }

    /// A child whose merge is already in progress: the branch owning
    /// MERGE_HEAD, else a recorded conflict outcome.
    async fn pending_conflict_child(&self) -> String {
        if let Some(git) = &self.git
            && git.merge_head(&self.worktree).await
        {
            for c in self.eng.store.workspaces() {
                if c.parent_id != self.id || c.branch.trim().is_empty() {
                    continue;
                }
                if git.merge_head_equals(&self.worktree, &c.branch).await {
                    return c.id;
                }
            }
        }
        self.eng
            .store
            .workspaces()
            .into_iter()
            .find(|c| {
                c.parent_id == self.id
                    && c.fan_out
                        .as_ref()
                        .is_some_and(|f| f.outcome.starts_with("conflict"))
            })
            .map(|c| c.id)
            .unwrap_or_default()
    }

    /// Drives the board until an event the caller handles: handoffs (the
    /// Maestro replans while the rest run), a conflicted merge, a stop,
    /// ready steps with no slot and nothing running, or an idle board.
    /// In-flight children are read from the plan each pass, so a call after
    /// an event or a restart picks up exactly where the board was.
    pub(crate) async fn fan_out_steps(
        &mut self,
        cancel: &CancellationToken,
        exclude: &mut HashSet<String>,
    ) -> FanOutResult {
        let mut res = FanOutResult::default();
        let mut first = true;
        loop {
            let Some(ws) = self.eng.store.workspace(&self.id) else {
                return res;
            };
            let Some(plan) = ws.plan.clone() else {
                return res;
            };
            let (mut ids, mut owned) = in_flight_children(&plan);
            if first {
                first = false;
                // A resumed parent picks its interrupted children back up and
                // refreshes the cards of the waves still in flight.
                self.resume_interrupted_children(&ids).await;
                let mut seen = HashSet::new();
                for id in &ids {
                    let n = self.child_wave(id);
                    if n > 0 && seen.insert(n) {
                        self.emit_wave_card(n);
                    }
                }
            }
            let settled: Vec<String>;
            let conflict = self.pending_conflict_child().await;
            if !conflict.is_empty() {
                if !owned.contains_key(&conflict) {
                    let s = self.steps_owned_by(&conflict);
                    owned.insert(conflict.clone(), s);
                }
                settled = vec![conflict];
            } else {
                let ready = ready_steps(&plan, exclude);
                if !ready.is_empty() {
                    let free = self.eng.gov.free_loops();
                    if free == 0 && ids.is_empty() {
                        res.no_slot = true;
                        return res;
                    }
                    if free > 0 {
                        let mut groups = group_by_files(&ready);
                        let free = usize::try_from(free).unwrap_or(usize::MAX);
                        groups.truncate(free);
                        let wave_no = ws.fan_out_waves + 1;
                        let after = self.board_unblocked.clone();
                        let mut started = 0;
                        for g in groups {
                            match self.start_step_child(&ws, &g, wave_no, &after).await {
                                Ok(id) => {
                                    ids.push(id.clone());
                                    owned.insert(id, g);
                                    started += 1;
                                }
                                Err(e) => self.emit_feed(
                                    "MA",
                                    "Maestro",
                                    &format!(
                                        "Could not start a child workspace for {}: {}",
                                        html_escape(&step_ids(&g)),
                                        html_escape(&e.0)
                                    ),
                                ),
                            }
                        }
                        if started > 0 {
                            self.eng
                                .store
                                .update_workspace(&self.id, |w| w.fan_out_waves = wave_no);
                            self.board_unblocked.clear();
                            self.emit_wave_card(wave_no);
                        } else if ids.is_empty() {
                            // Nothing started and nothing runs: the board
                            // falls back.
                            return res;
                        }
                    }
                }
                if ids.is_empty() {
                    return res;
                }
                self.set_board_status(&ids);
                match self.wait_any_child(cancel, &ids).await {
                    Some(s) => settled = s,
                    None => {
                        res.stopped = true;
                        return res;
                    }
                }
            }
            for id in &settled {
                let steps = owned.get(id).cloned().unwrap_or_default();
                let (conflicts, merged, handoff, failed) = self.merge_child(id, &steps).await;
                self.emit_wave_card(self.child_wave(id));
                if merged {
                    res.merged += 1;
                }
                if merged && !failed {
                    self.board_unblocked
                        .extend(steps.iter().map(|s| s.id.clone()));
                }
                if !handoff.is_empty() {
                    res.handoffs
                        .push(format!("{}: {handoff}", step_ids(&steps)));
                }
                if failed {
                    // Stable child id: step keys go stale after a replan.
                    exclude.insert(id.clone());
                }
                if !conflicts.is_empty() {
                    res.conflicts = conflicts;
                    return res;
                }
            }
            if !res.handoffs.is_empty() {
                return res;
            }
        }
    }

    /// The parent's status line while children run.
    fn set_board_status(&self, inflight: &[String]) {
        let (mut running, mut needs) = (0, 0);
        for id in inflight {
            if self
                .eng
                .store
                .workspace(id)
                .is_some_and(|c| c.state == WorkspaceState::Escalated)
            {
                needs += 1;
            } else {
                running += 1;
            }
        }
        let (mut done, mut total) = (0, 0);
        if let Some(plan) = self.eng.store.workspace(&self.id).and_then(|w| w.plan) {
            for s in &plan.steps {
                total += 1;
                if s.status == STEP_DONE || s.status == STEP_SKIPPED {
                    done += 1;
                }
            }
        }
        let label = if needs > 0 {
            format!(
                "board: {needs} child workspace(s) need you, {running} running, {done} of {total} steps done"
            )
        } else {
            format!("board: {running} child workspace(s) running, {done} of {total} steps done")
        };
        self.set_state(WorkspaceState::Coding, &label);
    }

    /// Blocks at checkpoints until a child settled; `None` when the parent
    /// stopped or was cancelled (a human Stop also stops the children).
    async fn wait_any_child(
        &mut self,
        cancel: &CancellationToken,
        ids: &[String],
    ) -> Option<Vec<String>> {
        loop {
            if !self.checkpoint(cancel).await {
                self.stop_running_children(ids).await;
                return None;
            }
            let settled: Vec<String> = ids
                .iter()
                .filter(|id| self.eng.child_settled(id))
                .cloned()
                .collect();
            if !settled.is_empty() {
                return Some(settled);
            }
            tokio::select! {
                _ = cancel.cancelled() => {
                    self.stop_running_children(ids).await;
                    return None;
                }
                _ = tokio::time::sleep(self.eng.fan_out_poll()) => {}
            }
        }
    }

    /// Creates and dispatches a child for one step group: a loop derived
    /// from this walk's converge phase, this walk's policy, based on this
    /// walk's branch, scoped by its plan. The parent steps become doing with
    /// the child's id. `dispatch_mu` is held across insert → parent metadata
    /// → dispatch so the drain cannot promote a half-built row.
    async fn start_step_child(
        &self,
        pws: &Workspace,
        steps: &[PlanStep],
        wave: i64,
        after: &[String],
    ) -> crate::Result<String> {
        let title = group_title(steps);
        let _d = self.eng.dispatch_mu.lock().await;
        let (mut rec, acquired) = self
            .eng
            .queue_start_workspace(
                &self.project_id,
                &truncate(&title, 200),
                "",
                &self.branch(),
                &self.autonomy,
                STEP_LOOP_ID,
                &self.policy_id,
                None,
            )
            .await?;
        self.eng.run_child_queue_hook();
        let child_plan = child_step_plan(pws, steps);
        rec.parent_id = self.id.clone();
        if let Some(ph) = self.walk_phase() {
            rec.parent_phase_id = ph.id;
        }
        rec.parent_step_ids
            .extend(steps.iter().map(|s| s.id.clone()));
        rec.definition_of_done = truncate(&child_plan.text(), PLAN_MAX_PERSIST_CHARS);
        rec.plan = Some(child_plan);
        rec.fan_out = Some(ChildFanOut {
            wave,
            after: after.to_vec(),
            ..ChildFanOut::default()
        });
        let r = rec.clone();
        self.eng.store.update_workspace(&rec.id, |w| {
            w.parent_id = r.parent_id;
            w.parent_step_ids = r.parent_step_ids;
            w.parent_phase_id = r.parent_phase_id;
            w.plan = r.plan;
            w.definition_of_done = r.definition_of_done;
            w.fan_out = r.fan_out;
        });
        for s in steps {
            self.set_step(&s.id, STEP_DOING, "", &rec.id);
        }
        if acquired && let Err(e) = self.eng.dispatch(rec.clone()).await {
            self.eng.gov.release_loop();
            for s in steps {
                self.set_step(
                    &s.id,
                    STEP_PENDING,
                    &format!("child dispatch failed: {e}"),
                    "",
                );
            }
            return Err(e);
        }
        Ok(rec.id)
    }

    /// Updates one parent step (the engine owns step transitions; this is
    /// the fan-out path, not an agent call).
    fn set_step(&self, step_id: &str, status: &str, note: &str, child_id: &str) {
        let note = note.trim();
        let Some(updated) = self.eng.store.update_workspace(&self.id, |w| {
            let Some(p) = w.plan.as_mut() else {
                return;
            };
            for s in p.steps.iter_mut().filter(|s| s.id == step_id) {
                s.status = status.to_owned();
                if !note.is_empty() {
                    s.note = truncate(note, 300);
                }
                if !child_id.is_empty() {
                    s.workspace = child_id.to_owned();
                }
            }
            w.definition_of_done = truncate(&p.text(), PLAN_MAX_PERSIST_CHARS);
            if let Some(prog) = run_progress(&w.state, w.plan.as_ref()) {
                w.progress = prog;
            }
        }) else {
            return;
        };
        self.eng.publish_loop_updated(
            &updated.id,
            updated.state.clone(),
            &updated.phase,
            updated.iteration,
        );
    }

    /// Merges a settled child's branch into this worktree and closes its
    /// steps. A failed or stopped child's partial commits still merge, but
    /// its steps stay pending and it is reported failed so the parent
    /// finishes them. Conflicts come back with the merge left in progress.
    async fn merge_child(
        &self,
        child_id: &str,
        steps: &[PlanStep],
    ) -> (Vec<String>, bool, String, bool) {
        let Some(child) = self.eng.store.workspace(child_id) else {
            for s in steps {
                self.set_step(
                    &s.id,
                    STEP_PENDING,
                    "child workspace vanished — running here",
                    "",
                );
            }
            return (Vec::new(), false, String::new(), true);
        };
        let mut handoff = child.handoff.trim().to_owned();
        let open = self.eng.store.open_findings(&child.id).unwrap_or_default();
        if !open.is_empty() {
            // Findings the child's review could not close: hand them up so
            // the aggregate Reviewer knows where to look.
            let mut b = format!(
                "{} unresolved finding(s) from the child's review:",
                open.len()
            );
            for (i, f) in open.iter().enumerate() {
                if i == 5 {
                    b.push_str(" …");
                    break;
                }
                b.push_str(&format!(
                    " [{}] {} — {};",
                    f.severity.as_str(),
                    finding_loc(&f.file, f.line, f.end_line),
                    truncate(&f.message, 120)
                ));
            }
            handoff = format!("{handoff}\n{b}").trim().to_owned();
        }
        let failed = child_incomplete(&child);
        let ids = step_ids(steps);
        let already = child
            .fan_out
            .as_ref()
            .is_some_and(|f| f.outcome.starts_with("conflict"));
        let Some(git) = &self.git else {
            return (Vec::new(), false, handoff, true);
        };
        if git.merge_head(&self.worktree).await {
            if !git.merge_head_equals(&self.worktree, &child.branch).await {
                return (Vec::new(), false, String::new(), false);
            }
            let mut conflicts = git.unmerged_paths(&self.worktree).await;
            if conflicts.is_empty() {
                conflicts = vec![".".into()];
            }
            if !already {
                let note = conflict_note(&handoff, &conflicts);
                for s in steps {
                    self.set_step(&s.id, STEP_DOING, &note, &child.id);
                }
                self.set_child_outcome(
                    &child.id,
                    &format!("conflict: {}", conflicts.join(", ")),
                    &handoff,
                );
            }
            return (conflicts, true, handoff, failed);
        }
        if already {
            self.close_merged_child(&child, steps, failed, &handoff)
                .await;
            return (Vec::new(), true, handoff, failed);
        }
        if let Err(e) = self.preserve_child_work(&child).await {
            self.emit_feed(
                "MA",
                "Maestro",
                &format!(
                    "Could not save uncommitted work in child <code>{}</code> — keeping the worktree ({}).",
                    html_escape(&child.id),
                    html_escape(&e)
                ),
            );
            for s in steps {
                self.set_step(
                    &s.id,
                    STEP_PENDING,
                    &format!("child work not saved: {e}"),
                    "",
                );
            }
            self.set_child_outcome(&child.id, "failed", &format!("preserve failed: {e}"));
            return (Vec::new(), false, handoff, true);
        }
        let merged = match recur_git::identity(&self.worktree).await {
            Ok((name, email)) => {
                let msg = format!(
                    "Merge {ids} from {}: {}",
                    child.branch,
                    truncate(
                        &steps.first().map(|s| s.title.clone()).unwrap_or_default(),
                        60
                    )
                );
                git.merge_into(&self.worktree, &child.branch, &msg, &name, &email)
                    .await
                    .map_err(|e| e.to_string())
            }
            Err(e) => Err(e.to_string()),
        };
        let conflicts = match merged {
            Ok(c) => c,
            Err(e) => {
                self.emit_feed(
                    "MA",
                    "Maestro",
                    &format!(
                        "Could not merge child branch <code>{}</code> ({}) — {} run here.",
                        html_escape(&child.branch),
                        html_escape(&e),
                        html_escape(&ids)
                    ),
                );
                for s in steps {
                    self.set_step(&s.id, STEP_PENDING, &format!("child merge failed: {e}"), "");
                }
                self.set_child_outcome(&child.id, "failed", &format!("merge failed: {e}"));
                self.eng.finish_child(&child.id).await;
                return (Vec::new(), false, handoff, true);
            }
        };
        if !conflicts.is_empty() {
            let note = conflict_note(&handoff, &conflicts);
            for s in steps {
                self.set_step(&s.id, STEP_DOING, &note, &child.id);
            }
            self.set_child_outcome(
                &child.id,
                &format!("conflict: {}", conflicts.join(", ")),
                &handoff,
            );
            self.emit_feed(
                "MA",
                "Maestro",
                &format!(
                    "Merged {} from <code>{}</code> with {} conflicted file(s) — sending to the Coder.",
                    html_escape(&ids),
                    html_escape(&child.branch),
                    conflicts.len()
                ),
            );
            return (conflicts, true, handoff, failed);
        }
        self.close_merged_child(&child, steps, failed, &handoff)
            .await;
        (Vec::new(), true, handoff, failed)
    }

    async fn close_merged_child(
        &self,
        child: &Workspace,
        steps: &[PlanStep],
        failed: bool,
        handoff: &str,
    ) {
        let mut status = STEP_DONE;
        let mut note = if handoff.is_empty() {
            "merged from child workspace".to_owned()
        } else {
            handoff.to_owned()
        };
        let mut outcome = "merged";
        if failed {
            status = STEP_PENDING;
            note = format!(
                "child {} — partial work merged; finish here. {note}",
                child.state.as_str()
            );
            outcome = "failed";
        }
        for s in steps {
            self.set_step(&s.id, status, &note, "");
        }
        self.set_child_outcome(&child.id, outcome, handoff);
        self.eng.finish_child(&child.id).await;
    }

    /// Commits a settled child's dirty files so teardown cannot drop them
    /// (failed/stopped children never reach the engine commit).
    async fn preserve_child_work(&self, child: &Workspace) -> Result<(), String> {
        let Some(git) = &self.git else {
            return Ok(());
        };
        if child.worktree_path.trim().is_empty() {
            return Ok(());
        }
        let (name, email) = recur_git::identity(&child.worktree_path)
            .await
            .map_err(|e| format!("preserve child {} identity: {e}", child.id))?;
        git.commit(
            &child.worktree_path,
            "WIP: uncommitted child work",
            &name,
            &email,
        )
        .await
        .map_err(|e| format!("preserve child {} work: {e}", child.id))?;
        Ok(())
    }

    /// Stops the children a parent leaves behind (Stop, a failed resolve
    /// turn, the board cap). Not on app shutdown: Recover marks them
    /// interrupted and the parent's resume re-dispatches them.
    async fn stop_running_children(&self, ids: &[String]) {
        if self.eng.is_shutting_down() {
            return;
        }
        for id in ids {
            self.eng.stop(id).await;
        }
    }

    /// Re-dispatches in-flight children a restart left interrupted; a child
    /// the human paused is left alone.
    async fn resume_interrupted_children(&self, ids: &[String]) {
        for id in ids {
            let Some(c) = self.eng.store.workspace(id) else {
                continue;
            };
            if c.state != WorkspaceState::Paused || c.phase != PHASE_INTERRUPTED {
                continue;
            }
            if self.eng.running_loop_by_id(id).is_some() {
                continue;
            }
            self.eng.resume(id).await;
        }
    }

    pub(crate) async fn stop_in_flight_children(&self) {
        let Some(plan) = self.eng.store.workspace(&self.id).and_then(|w| w.plan) else {
            return;
        };
        let (ids, _) = in_flight_children(&plan);
        self.stop_running_children(&ids).await;
    }

    /// Closes children whose merge conflicted once the resolve turn
    /// concluded the merge.
    pub(crate) async fn mark_conflicts_resolved(&self) {
        let mut waves = BTreeSet::new();
        for c in self.eng.store.workspaces() {
            if c.parent_id != self.id {
                continue;
            }
            let Some(fo) = c.fan_out.clone() else {
                continue;
            };
            if !fo.outcome.starts_with("conflict") {
                continue;
            }
            let note = format!("{} (conflicts resolved by the parent)", fo.note)
                .trim()
                .to_owned();
            let failed = child_incomplete(&c);
            let steps = self.steps_owned_by(&c.id);
            self.close_merged_child(&c, &steps, failed, &note).await;
            waves.insert(fo.wave);
        }
        for n in waves {
            self.emit_wave_card(n);
        }
    }

    /// The parent's current steps owned by a child (a replan may have
    /// renumbered them).
    fn steps_owned_by(&self, child_id: &str) -> Vec<PlanStep> {
        self.eng
            .store
            .workspace(&self.id)
            .and_then(|w| w.plan)
            .map(|p| {
                p.steps
                    .into_iter()
                    .filter(|s| s.workspace == child_id)
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn conflict_note(handoff: &str, conflicts: &[String]) -> String {
    let base = if handoff.is_empty() {
        "merged from child workspace"
    } else {
        handoff
    };
    format!("{base} (merge conflicts: {})", conflicts.join(", "))
}
