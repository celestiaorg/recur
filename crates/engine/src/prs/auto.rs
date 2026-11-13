//! PR automations: after each sweep, an open forge PR whose Recur workspace
//! is idle gets the prompt action a person would run there (Settings → PRs).
//! Merge conflicts run Update with base, then `/update` when git leaves
//! markers; failing checks run `/fix-checks`; new review activity runs
//! `/address-review`, whose script sends nothing when no comment needs a
//! response. The first sweep acts on what a PR already shows.

use std::collections::{HashMap, HashSet};

use recur_domain::{CHAT_MODE_AGENT, Pr, UPDATE_WORKSPACE_CONFLICT, normalize_chat_mode};
use recur_forge::model::BUCKET_PENDING;

use tokio_util::sync::CancellationToken;

use crate::Engine;
use crate::bus::{EngineEvent, WorkspaceHoisted};
use crate::codes::PAIR_AGENT_CODE;
use crate::composer::ExpandOpts;
use crate::prs::{is_forge_source, pr_settled};
use crate::text::html_escape;
use crate::view::prompts::{PromptTabPick, PromptTabQuery, pick_prompt_tab};

/// The shipped `/update`, `/fix-checks` and `/address-review` `tab_group`.
const AUTO_TAB_GROUP: &str = "ops";

/// Automatic turns per PR and kind before Recur stops and leaves it to you.
pub(crate) const MAX_AUTO_ATTEMPTS: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum AutoKind {
    Conflict,
    Checks,
    Review,
}

/// Which automations Settings turns on.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct AutoToggles {
    pub conflicts: bool,
    pub checks: bool,
    pub review: bool,
}

impl AutoToggles {
    fn on(self, kind: AutoKind) -> bool {
        match kind {
            AutoKind::Conflict => self.conflicts,
            AutoKind::Checks => self.checks,
            AutoKind::Review => self.review,
        }
    }
}

/// An open forge PR the viewer opened, with a live, non-loop workspace.
#[derive(Debug, Clone, Default)]
pub(crate) struct AutoCandidate {
    pub pr_id: String,
    pub number: i64,
    pub ws_id: String,
    /// No live chat, question, loop or base update in the workspace.
    pub idle: bool,
    pub conflicting: bool,
    /// GitHub reports the branch merges cleanly.
    pub mergeable: bool,
    pub checks_failing: bool,
    /// Checks ran and every one passed (none failing or pending).
    pub checks_green: bool,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AutoAction {
    pub kind: AutoKind,
    pub pr_id: String,
    pub number: i64,
    pub ws_id: String,
    /// This attempt reaches [`MAX_AUTO_ATTEMPTS`].
    pub last: bool,
}

#[derive(Debug, Clone, Default)]
struct AutoPr {
    /// Conflict / Checks: this occurrence was handled; cleared when the
    /// condition clears, so the next occurrence runs again.
    active: bool,
    /// Tries since the PR was last clean (checks green, branch mergeable),
    /// so a fix that never lands stops at the cap.
    attempts: u32,
    /// Review: the PR `updated_at` last consumed.
    seen: String,
}

// simplify: attempts live in memory, so a restart allows another
// MAX_AUTO_ATTEMPTS per PR; persist them per PR if that retries too much.
#[derive(Debug, Default)]
pub(crate) struct AutoState {
    prs: HashMap<(String, AutoKind), AutoPr>,
    /// Workspaces with an automation in flight.
    busy: HashSet<String>,
}

/// One sweep's decisions: at most one action per workspace, conflicts
/// first (a conflicting branch often fails checks too), then checks, then
/// review. A busy workspace leaves its condition unconsumed for the next sweep;
/// entries of PRs that are no longer candidates are dropped.
pub(crate) fn auto_actions(
    cands: &[AutoCandidate],
    on: AutoToggles,
    st: &mut AutoState,
) -> Vec<AutoAction> {
    let live: HashSet<&str> = cands.iter().map(|c| c.pr_id.as_str()).collect();
    st.prs.retain(|(id, _), _| live.contains(id.as_str()));
    let mut out = Vec::new();
    for c in cands {
        for kind in [AutoKind::Conflict, AutoKind::Checks, AutoKind::Review] {
            if !on.on(kind) {
                continue;
            }
            let (cond, clean) = match kind {
                AutoKind::Conflict => (c.conflicting, c.mergeable),
                AutoKind::Checks => (c.checks_failing, c.checks_green),
                AutoKind::Review => (false, false),
            };
            let e = st.prs.entry((c.pr_id.clone(), kind)).or_default();
            if clean {
                e.attempts = 0;
            }
            let due = match kind {
                AutoKind::Conflict | AutoKind::Checks => {
                    if !cond {
                        e.active = false;
                    }
                    cond && !e.active
                }
                AutoKind::Review => c.updated_at != e.seen,
            };
            if !due || e.attempts >= MAX_AUTO_ATTEMPTS || !c.idle || st.busy.contains(&c.ws_id) {
                continue;
            }
            match kind {
                AutoKind::Conflict | AutoKind::Checks => {
                    e.active = true;
                    e.attempts += 1;
                }
                // Counted by `note_review_turn` only when a turn ran.
                AutoKind::Review => e.seen = c.updated_at.clone(),
            }
            st.busy.insert(c.ws_id.clone());
            out.push(AutoAction {
                kind,
                pr_id: c.pr_id.clone(),
                number: c.number,
                ws_id: c.ws_id.clone(),
                last: kind != AutoKind::Review && e.attempts == MAX_AUTO_ATTEMPTS,
            });
        }
    }
    out
}

impl Engine {
    /// Runs the PR automations Settings turns on (called after each sweep).
    pub async fn run_pr_automations(&self) {
        let s = self.settings();
        let on = AutoToggles {
            conflicts: s.auto_update_conflicts,
            checks: s.auto_fix_checks,
            review: s.auto_address_review,
        };
        if !(on.conflicts || on.checks || on.review) || self.is_shutting_down() {
            return;
        }
        let cands: Vec<AutoCandidate> = self
            .store
            .prs()
            .iter()
            .filter_map(|p| self.auto_candidate(p))
            .collect();
        let actions = auto_actions(&cands, on, &mut self.prs.auto.lock());
        for a in actions {
            let eng = self.arc();
            self.tasks.schedules.spawn(async move {
                eng.run_pr_automation(&a).await;
                eng.prs.auto.lock().busy.remove(&a.ws_id);
            });
        }
    }

    fn auto_candidate(&self, p: &Pr) -> Option<AutoCandidate> {
        // Someone else's PR (a workspace made from it) is theirs to fix.
        if !is_forge_source(&p.source) || pr_settled(p) || !p.viewer_is_author() {
            return None;
        }
        let ws_id = self.live_pr_workspace_id(&p.workspace_id);
        let ws = self.store.workspace(&ws_id)?;
        // A loop's Maestro owns its PR (send-back), so it is left alone.
        if ws.worktree_path.trim().is_empty() || ws.has_run() {
            return None;
        }
        Some(AutoCandidate {
            pr_id: p.id.clone(),
            number: p.number,
            idle: p.updating_base.is_empty() && !self.scheduled_chat_active(&ws_id),
            ws_id,
            conflicting: p.mergeable.eq_ignore_ascii_case("CONFLICTING"),
            mergeable: p.mergeable.eq_ignore_ascii_case("MERGEABLE"),
            checks_failing: p.has_checks && !p.checks_ok,
            checks_green: p.has_checks
                && p.checks_ok
                && p.checks
                    .iter()
                    .flatten()
                    .all(|c| c.status != BUCKET_PENDING),
            updated_at: p.updated_at.clone(),
        })
    }

    async fn run_pr_automation(&self, a: &AutoAction) {
        let note = |text: String| {
            self.emit_workspace_event(&a.ws_id, "MA", "Maestro", &html_escape(&text));
        };
        let n = a.number;
        match a.kind {
            AutoKind::Conflict => {
                note(format!(
                    "PR #{n} has merge conflicts. Updating with base automatically."
                ));
                match self.update_pr_with_base(&a.pr_id, "").await {
                    Ok(r) if r.status == UPDATE_WORKSPACE_CONFLICT => {
                        self.send_auto_prompt(&a.ws_id, "/update").await;
                    }
                    Ok(_) => {}
                    Err(e) => note(format!("Automatic update with base failed: {}", e.0)),
                }
            }
            AutoKind::Checks => {
                note(format!(
                    "Checks failed on PR #{n}. Running Fix checks automatically."
                ));
                self.send_auto_prompt(&a.ws_id, "/fix-checks").await;
            }
            AutoKind::Review => {
                if self
                    .send_auto_prompt(&a.ws_id, "/address-review")
                    .await
                    .is_empty()
                {
                    return;
                }
                let last = self.note_review_turn(&a.pr_id);
                note(format!(
                    "Ran Address review automatically for new review feedback on PR #{n}."
                ));
                if last {
                    note(format!(
                        "Stopped automatic Address review for PR #{n} after {MAX_AUTO_ATTEMPTS} tries."
                    ));
                }
                return;
            }
        }
        if a.last {
            let what = if a.kind == AutoKind::Conflict {
                "conflict fixes"
            } else {
                "Fix checks"
            };
            note(format!(
                "Stopped automatic {what} for PR #{n} after {MAX_AUTO_ATTEMPTS} tries."
            ));
        }
    }

    /// Counts an `/address-review` that ran a turn; true at the cap.
    fn note_review_turn(&self, pr_id: &str) -> bool {
        let mut st = self.prs.auto.lock();
        let e = st
            .prs
            .entry((pr_id.to_owned(), AutoKind::Review))
            .or_default();
        e.attempts += 1;
        e.attempts == MAX_AUTO_ATTEMPTS
    }

    /// Runs a prompt action once and, when it has something to send, sends
    /// it to the tab the dock would pick for an "ops" action (an idle ops
    /// tab, else an unused agent tab, else a new one), so the user's working
    /// conversation keeps its session. Empty when the action had nothing to
    /// send: no tab is picked or created and no turn runs.
    async fn send_auto_prompt(&self, ws_id: &str, slash: &str) -> String {
        let Some(ws) = self.store.workspace(ws_id) else {
            return String::new();
        };
        let Some(p) = self.lookup_prompt_action(slash.trim_start_matches('/'), false, None) else {
            return String::new();
        };
        let opts = ExpandOpts {
            worktree: ws.worktree_path.clone(),
            workspace_id: ws_id.to_owned(),
            ..ExpandOpts::default()
        };
        let out = match self
            .run_prompt_action(&CancellationToken::new(), &p, &opts)
            .await
        {
            Ok(out) if !out.prompt.trim().is_empty() => out,
            Ok(_) => return String::new(),
            Err(e) => {
                let msg = format!("Automatic {slash} couldn't run: {e}");
                self.emit_workspace_event(ws_id, "MA", "Maestro", &html_escape(&msg));
                return String::new();
            }
        };
        let tabs = self.list_workspace_chats(ws_id);
        let pick = pick_prompt_tab(&PromptTabQuery {
            tabs: &tabs,
            active_code: "",
            mode: CHAT_MODE_AGENT,
            group: AUTO_TAB_GROUP,
            is_busy: &|code| self.live_chat_turn(ws_id, code).is_some(),
            tab_mode: &|t| normalize_chat_mode(&t.mode).to_owned(),
            started: Some(&|t| self.chat_has_user_message(ws_id, &t.code)),
            drafted: None,
            conductor_locked: None,
        });
        let code = match pick {
            PromptTabPick::Select(code) => code,
            PromptTabPick::Current | PromptTabPick::Create => {
                let code = self.create_workspace_chat(ws_id).await.code;
                if code.is_empty() {
                    return String::new();
                }
                // The open workspace view re-lists its tab strip.
                self.bus
                    .publish(EngineEvent::WorkspaceHoisted(WorkspaceHoisted {
                        chats_changed: true,
                        workspace_id: ws_id.to_owned(),
                        ..WorkspaceHoisted::default()
                    }));
                code
            }
        };
        let (mut model, mut flags, _) = self.workspace_chat_selection(ws_id, &code);
        if model.trim().is_empty() {
            (model, flags, _) = self.workspace_chat_selection(ws_id, PAIR_AGENT_CODE);
        }
        let display = if out.display.is_empty() {
            slash
        } else {
            &out.display
        };
        let reply = self
            .send_workspace_turn(
                ws_id,
                &code,
                &out.prompt,
                &model,
                Some(&flags),
                &[],
                CHAT_MODE_AGENT,
                display,
                p.fresh_session,
            )
            .await;
        self.remember_workspace_chat_prompt_group(ws_id, &code, AUTO_TAB_GROUP)
            .await;
        reply
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An idle PR with nothing wrong.
    fn cand(id: &str) -> AutoCandidate {
        AutoCandidate {
            pr_id: id.into(),
            number: 7,
            ws_id: format!("ws-{id}"),
            idle: true,
            updated_at: "t0".into(),
            ..Default::default()
        }
    }

    const ALL: AutoToggles = AutoToggles {
        conflicts: true,
        checks: true,
        review: true,
    };

    const CHECKS: AutoToggles = AutoToggles {
        conflicts: false,
        checks: true,
        review: false,
    };

    fn kinds(a: &[AutoAction]) -> Vec<AutoKind> {
        a.iter().map(|a| a.kind).collect()
    }

    /// A sweep that also lets the in-flight automation finish.
    fn sweep(c: &AutoCandidate, on: AutoToggles, st: &mut AutoState) -> Vec<AutoKind> {
        let out = kinds(&auto_actions(std::slice::from_ref(c), on, st));
        st.busy.clear();
        out
    }

    /// State after Recur has swept a clean `c` once (its review consumed).
    fn seen(c: &AutoCandidate, on: AutoToggles) -> AutoState {
        let mut st = AutoState::default();
        sweep(c, on, &mut st);
        assert!(sweep(c, on, &mut st).is_empty());
        st
    }

    #[test]
    fn the_first_sweep_acts_on_what_a_pr_already_shows() {
        let mut c = cand("a");
        c.conflicting = true;
        c.checks_failing = true;
        let mut st = AutoState::default();
        assert_eq!(sweep(&c, ALL, &mut st), [AutoKind::Conflict]);
        assert_eq!(sweep(&c, ALL, &mut st), [AutoKind::Checks]);
        assert_eq!(sweep(&c, ALL, &mut st), [AutoKind::Review]);
        assert!(
            sweep(&c, ALL, &mut st).is_empty(),
            "still the same occurrence"
        );
    }

    #[test]
    fn toggles_gate_their_kind() {
        let mut c = cand("a");
        let mut st = seen(&c, ALL);
        c.conflicting = true;
        c.checks_failing = true;
        assert!(sweep(&c, AutoToggles::default(), &mut st).is_empty());
        assert_eq!(sweep(&c, CHECKS, &mut st), [AutoKind::Checks]);
    }

    #[test]
    fn a_toggle_turned_on_acts_on_what_it_sees() {
        let mut c = cand("a");
        let mut st = seen(&c, AutoToggles::default());
        c.checks_failing = true;
        assert_eq!(sweep(&c, CHECKS, &mut st), [AutoKind::Checks]);
    }

    #[test]
    fn one_action_per_workspace_conflicts_first() {
        let mut c = cand("a");
        let mut st = seen(&c, ALL);
        c.conflicting = true;
        c.checks_failing = true;
        c.updated_at = "t1".into();
        assert_eq!(sweep(&c, ALL, &mut st), [AutoKind::Conflict]);
        assert_eq!(sweep(&c, ALL, &mut st), [AutoKind::Checks]);
        assert_eq!(sweep(&c, ALL, &mut st), [AutoKind::Review]);
        assert!(sweep(&c, ALL, &mut st).is_empty());
    }

    #[test]
    fn a_rerun_that_fails_again_runs_again() {
        let mut c = cand("a");
        let mut st = seen(&c, CHECKS);
        c.checks_failing = true;
        assert_eq!(sweep(&c, CHECKS, &mut st), [AutoKind::Checks]);
        assert!(sweep(&c, CHECKS, &mut st).is_empty());
        // The re-run is pending (nothing failed yet), then fails.
        c.checks_failing = false;
        assert!(sweep(&c, CHECKS, &mut st).is_empty());
        c.checks_failing = true;
        assert_eq!(sweep(&c, CHECKS, &mut st), [AutoKind::Checks]);
    }

    #[test]
    fn busy_waits_then_fires() {
        let on = AutoToggles {
            conflicts: true,
            review: true,
            ..Default::default()
        };
        let mut c = cand("a");
        let mut st = seen(&c, on);
        c.conflicting = true;
        c.updated_at = "t1".into();
        c.idle = false;
        assert!(auto_actions(std::slice::from_ref(&c), on, &mut st).is_empty());
        c.idle = true;
        assert_eq!(
            kinds(&auto_actions(std::slice::from_ref(&c), on, &mut st)),
            [AutoKind::Conflict]
        );
        // Review stayed unconsumed while busy and the workspace is in
        // flight now, so it waits for the next sweep.
        assert!(auto_actions(std::slice::from_ref(&c), on, &mut st).is_empty());
        st.busy.clear();
        assert_eq!(sweep(&c, on, &mut st), [AutoKind::Review]);
    }

    #[test]
    fn review_fires_on_updated_at_changes_only() {
        let on = AutoToggles {
            review: true,
            ..Default::default()
        };
        let mut c = cand("a");
        let mut st = seen(&c, on);
        assert!(sweep(&c, on, &mut st).is_empty());
        c.updated_at = "t1".into();
        assert_eq!(sweep(&c, on, &mut st), [AutoKind::Review]);
        assert!(sweep(&c, on, &mut st).is_empty());
    }

    #[test]
    fn stops_at_the_cap_until_checks_pass() {
        let mut c = cand("a");
        let mut st = seen(&c, CHECKS);
        let mut lasts = Vec::new();
        let mut fail_then_pending = |c: &mut AutoCandidate, st: &mut AutoState| {
            c.checks_failing = true;
            let a = auto_actions(std::slice::from_ref(c), CHECKS, st);
            lasts.extend(a.iter().map(|a| a.last));
            st.busy.clear();
            c.checks_failing = false;
            sweep(c, CHECKS, st);
        };
        for _ in 0..5 {
            fail_then_pending(&mut c, &mut st);
        }
        // Green resets the count: the next failure runs again.
        c.checks_green = true;
        sweep(&c, CHECKS, &mut st);
        c.checks_green = false;
        fail_then_pending(&mut c, &mut st);
        assert_eq!(lasts, [false, false, true, false]);
    }

    #[test]
    fn a_clean_merge_resets_the_conflict_cap() {
        let on = AutoToggles {
            conflicts: true,
            ..Default::default()
        };
        let mut c = cand("a");
        let mut st = seen(&c, on);
        for _ in 0..MAX_AUTO_ATTEMPTS {
            c.conflicting = true;
            assert_eq!(sweep(&c, on, &mut st), [AutoKind::Conflict]);
            c.conflicting = false;
            sweep(&c, on, &mut st);
        }
        c.conflicting = true;
        assert!(sweep(&c, on, &mut st).is_empty(), "capped");
        c.conflicting = false;
        c.mergeable = true;
        sweep(&c, on, &mut st);
        c.mergeable = false;
        c.conflicting = true;
        assert_eq!(sweep(&c, on, &mut st), [AutoKind::Conflict]);
    }

    #[test]
    fn review_counts_only_turns_that_ran() {
        let on = AutoToggles {
            review: true,
            ..Default::default()
        };
        let mut c = cand("a");
        let mut st = seen(&c, on);
        if let Some(e) = st.prs.get_mut(&("a".to_owned(), AutoKind::Review)) {
            e.attempts = MAX_AUTO_ATTEMPTS;
        }
        c.updated_at = "t9".into();
        assert!(sweep(&c, on, &mut st).is_empty());
    }

    #[test]
    fn a_pr_that_drops_out_is_forgotten() {
        let mut c = cand("a");
        c.checks_failing = true;
        let mut st = AutoState::default();
        assert_eq!(sweep(&c, CHECKS, &mut st), [AutoKind::Checks]);
        assert!(auto_actions(&[], CHECKS, &mut st).is_empty());
        assert!(st.prs.is_empty());
        assert_eq!(sweep(&c, CHECKS, &mut st), [AutoKind::Checks]);
    }
}
