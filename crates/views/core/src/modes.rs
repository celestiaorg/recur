//! Workspace run and lifecycle predicates the shell chrome reads (`lib/modes.ts`).

use std::collections::{BTreeMap, HashSet};

use recur_domain::{PermissionRequest, UserQuestion, Workspace};

const RUN_ACTIVE: [&str; 5] = ["queued", "planning", "coding", "reviewing", "verifying"];
const RUN_SETTLED_PAUSE: [&str; 4] = ["idle", "artifact", "phase missing", "loop refs missing"];

/// Whether the autopilot has (ever) been engaged (`domain.Workspace.HasRun`).
pub fn ws_has_run(w: &Workspace) -> bool {
    w.has_run()
}

/// The autopilot currently owns the worktree, plus a live paused/escalated goroutine that will
/// resume editing. Walker-idle pauses are settled: the human can commit/merge.
pub fn ws_run_active(w: Option<&Workspace>) -> bool {
    let Some(w) = w else { return false };
    let state = w.state.as_str();
    if state.is_empty() {
        return false;
    }
    if RUN_ACTIVE.contains(&state) || state == "escalated" {
        return true;
    }
    state == "paused" && !RUN_SETTLED_PAUSE.contains(&w.phase.as_str())
}

/// Amber pulse: any live agent (Maestro included) or an in-flight loop. Escalated is "needs
/// you", not a running pulse.
pub fn workspace_is_running(w: &Workspace, running: &HashSet<String>) -> bool {
    if !w.id.is_empty() && running.contains(&w.id) {
        return true;
    }
    let state = w.state.as_str();
    !state.is_empty() && state != "escalated" && RUN_ACTIVE.contains(&state)
}

/// Parked items that need the human, per workspace. GroupNeeds (retryable fail, interrupted
/// recover) counts once even without a prompt; hidden PR chat hosts stay off the sidebar.
pub fn parked_needs_you_by_workspace(
    workspaces: &[Workspace],
    questions: &[UserQuestion],
    permissions: &[PermissionRequest],
) -> BTreeMap<String, usize> {
    let mut n: BTreeMap<String, usize> = BTreeMap::new();
    let mut hidden = HashSet::new();
    for w in workspaces {
        if w.id.is_empty() {
            continue;
        }
        if w.archived || !w.host_pr_id.trim().is_empty() {
            hidden.insert(w.id.as_str());
            continue;
        }
        if w.state.as_str() == "escalated" || w.group.as_str() == "needs" {
            *n.entry(w.id.clone()).or_default() += 1;
        }
    }
    let ids = questions
        .iter()
        .map(|q| q.workspace_id.as_str())
        .chain(permissions.iter().map(|p| p.workspace_id.as_str()));
    for id in ids {
        if !id.is_empty() && !hidden.contains(id) {
            *n.entry(id.to_string()).or_default() += 1;
        }
    }
    n
}

/// The `title` of a sidebar unread chip.
pub fn sidebar_unread_title(unseen: usize, parked: usize) -> String {
    let n = unseen + parked;
    if n == 0 {
        return String::new();
    }
    if parked > 0 {
        return if n == 1 {
            "Needs you".into()
        } else {
            format!("{n} waiting")
        };
    }
    format!("{n} unseen agent response{}", if n == 1 { "" } else { "s" })
}

/// Autopilot finished: the engine's RemoveWorkspace allows these.
pub const WORKSPACE_SETTLED_STATES: [&str; 4] = ["merged", "stopped", "failed", "pr_open"];

pub fn workspace_is_settled(state: &str) -> bool {
    WORKSPACE_SETTLED_STATES.contains(&state)
}

/// Whether Remove is offered at all. Main checkouts, PR chat hosts and archived rows never get
/// the control; running loops still show it disabled.
pub fn workspace_shows_remove(w: Option<&Workspace>) -> bool {
    w.is_some_and(|w| !w.is_main && !w.archived && w.host_pr_id.trim().is_empty())
}

/// Whether Remove is allowed: a hand workspace or a settled loop, with no live agent turn.
pub fn workspace_can_remove(w: Option<&Workspace>, agent_running: bool) -> bool {
    let Some(w) = w.filter(|w| workspace_shows_remove(Some(w))) else {
        return false;
    };
    if agent_running {
        return false;
    }
    if !ws_has_run(w) || workspace_is_settled(w.state.as_str()) {
        return true;
    }
    w.state.as_str() == "paused" && RUN_SETTLED_PAUSE.contains(&w.phase.as_str())
}

/// Git worktree and/or setup_cmd still in flight.
pub fn ws_creating(w: Option<&Workspace>) -> bool {
    w.is_some_and(|w| {
        w.last_error.is_empty()
            && (w.phase == "creating"
                || (w.phase == "waiting for a slot" && w.worktree_path.is_empty()))
    })
}

/// The editor + files overlay: blocked until the worktree is on disk, or for good when a
/// create or allocate failed with no tree.
pub fn ws_main_blocked(w: Option<&Workspace>) -> bool {
    if let Some(w) = w
        && w.worktree_path.is_empty()
        && !w.last_error.is_empty()
        && (w.phase == "failed" || w.status_label == "Couldn't create")
    {
        return true;
    }
    if w.is_some_and(|w| !w.worktree_path.is_empty()) {
        return false;
    }
    ws_creating(w)
}

use recur_engine::view::modes as rules;
pub use recur_engine::view::modes::{
    CatalogModel, ModelPick, NEW_CHAT_TITLE, chat_has_started, default_model_for_mode,
    is_mode_default_selection, picker_after_mode_change, resolved_default_for_mode, ws_chat_held,
    ws_create_failed, ws_setup_status,
};

/// How a chat tab's agent may act. The mode belongs to the chat session; the rules over it
/// are `recur_engine::view::modes`, this adds the dock's icon and tint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChatMode {
    Ask,
    Plan,
    Agent,
    Review,
    Loop,
}

impl ChatMode {
    /// Unknown and empty modes degrade to agent.
    pub fn from_key(m: &str) -> ChatMode {
        match rules::normalize_chat_mode(m) {
            "ask" => ChatMode::Ask,
            "plan" => ChatMode::Plan,
            "review" => ChatMode::Review,
            "loop" => ChatMode::Loop,
            _ => ChatMode::Agent,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            ChatMode::Ask => "ask",
            ChatMode::Plan => "plan",
            ChatMode::Agent => "agent",
            ChatMode::Review => "review",
            ChatMode::Loop => "loop",
        }
    }

    fn meta(self) -> rules::ChatModeOption {
        rules::chat_mode_meta(self.key())
    }

    pub fn label(self) -> &'static str {
        self.meta().label
    }

    pub fn hint(self) -> &'static str {
        self.meta().hint
    }

    pub fn icon(self) -> recur_ui::IconName {
        use recur_ui::IconName;
        match self {
            ChatMode::Ask => IconName::MessageQuestion,
            ChatMode::Plan => IconName::Notebook,
            ChatMode::Agent => IconName::MessageCode,
            ChatMode::Review => IconName::GitCompare,
            ChatMode::Loop => IconName::RefreshCw,
        }
    }

    /// The mode's `tint` (`var(--emerald)`, Plan's literal `#facc15`, …) as a color.
    pub fn tint(self, t: &recur_ui::Tokens) -> recur_ui::Color {
        match self {
            ChatMode::Ask => t.emerald,
            ChatMode::Plan => recur_ui::hex(0xfacc15),
            ChatMode::Agent => t.cyan,
            ChatMode::Review => t.indigo,
            ChatMode::Loop => t.amber,
        }
    }

    /// `cycleChatMode`.
    pub fn cycle(self, exclude_loop: bool, exclude_agent: bool) -> ChatMode {
        ChatMode::from_key(rules::cycle_chat_mode(
            self.key(),
            exclude_loop,
            exclude_agent,
        ))
    }

    /// `chatModeOptions`.
    pub fn options(exclude_loop: bool, exclude_agent: bool) -> Vec<ChatMode> {
        rules::chat_mode_options(exclude_loop, exclude_agent)
            .iter()
            .map(|o| ChatMode::from_key(o.value))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(f: impl FnOnce(&mut Workspace)) -> Workspace {
        let mut w = Workspace::default();
        f(&mut w);
        w
    }

    fn set(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn main_blocked() {
        fn b(f: impl FnOnce(&mut Workspace)) -> bool {
            ws_main_blocked(Some(&w(f)))
        }
        assert!(b(|w| w.phase = "creating".into()));
        assert!(b(|w| w.phase = "waiting for a slot".into()));
        assert!(!b(|w| {
            w.phase = "waiting for a slot".into();
            w.worktree_path = "/tmp/wt".into();
        }));
        assert!(!b(|w| {
            w.phase = "creating".into();
            w.worktree_path = "/tmp/wt".into();
        }));
        assert!(ws_creating(Some(&w(|w| {
            w.phase = "creating".into();
            w.worktree_path = "/tmp/wt".into();
        }))));
        assert!(b(|w| {
            w.phase = "failed".into();
            w.last_error = "git failed".into();
            w.status_label = "Couldn't create".into();
            w.r#type = "solo".into();
        }));
        assert!(!b(|w| {
            w.phase = "editing".into();
            w.last_error = "later".into();
            w.worktree_path = "/tmp/wt".into();
        }));
        assert!(b(|w| {
            w.phase = "failed".into();
            w.last_error = "allocate worktree: bad base".into();
            w.status_label = "Couldn't create".into();
            w.run_started = true;
        }));
        assert!(!b(|w| {
            w.phase = "editing".into();
            w.worktree_path = "/tmp/wt".into();
        }));
        assert!(!ws_main_blocked(None));
    }

    #[test]
    fn run_active() {
        let a = |state: &str, phase: &str| {
            ws_run_active(Some(&w(|w| {
                w.state = state.into();
                w.phase = phase.into();
            })))
        };
        assert!(a("coding", "") && a("queued", ""));
        assert!(a("escalated", "reviewing iteration 2") && a("paused", "paused"));
        for p in ["idle", "artifact", "phase missing", "loop refs missing"] {
            assert!(!a("paused", p));
        }
        assert!(!a("pr_open", "") && !a("stopped", ""));
    }

    #[test]
    fn is_running() {
        let r = |id: &str, state: &str, phase: &str, running: &[&str]| {
            workspace_is_running(
                &w(|w| {
                    w.id = id.into();
                    w.state = state.into();
                    w.phase = phase.into();
                }),
                &set(running),
            )
        };
        assert!(r("child", "coding", "", &[]));
        assert!(r("ws", "paused", "idle", &["ws"]));
        assert!(!r("ws", "paused", "idle", &["other"]));
        assert!(!r("ws", "escalated", "", &[]));
        assert!(r("ws", "escalated", "", &["ws"]));
        assert!(!r("ws", "paused", "paused", &[]));
        assert!(!r("ws", "paused", "board event cap", &[]));
        assert!(r("ws", "paused", "board event cap", &["ws"]));
    }

    fn q(ws: &str) -> UserQuestion {
        UserQuestion {
            workspace_id: ws.into(),
            ..UserQuestion::default()
        }
    }

    fn p(ws: &str) -> PermissionRequest {
        PermissionRequest {
            workspace_id: ws.into(),
            ..PermissionRequest::default()
        }
    }

    #[test]
    fn parked() {
        let ws = |id: &str, state: &str, group: &str| {
            w(|w| {
                w.id = id.into();
                w.state = state.into();
                w.group = group.into();
            })
        };
        let n = parked_needs_you_by_workspace(
            &[ws("ask", "escalated", "needs"), ws("idle", "paused", "")],
            &[q("idle"), q("idle")],
            &[p("ask")],
        );
        assert_eq!(n.get("ask"), Some(&2));
        assert_eq!(n.get("idle"), Some(&2));
        let n = parked_needs_you_by_workspace(
            &[
                ws("fail", "failed", "needs"),
                ws("cut", "paused", "needs"),
                ws("rest", "paused", "active"),
            ],
            &[],
            &[],
        );
        assert_eq!((n.get("fail"), n.get("cut")), (Some(&1), Some(&1)));
        assert!(!n.contains_key("rest"));
        let gone = w(|w| {
            w.id = "gone".into();
            w.state = "escalated".into();
            w.archived = true;
        });
        assert!(parked_needs_you_by_workspace(&[gone], &[q("gone")], &[]).is_empty());
        let host = w(|w| {
            w.id = "host".into();
            w.host_pr_id = "pr-1".into();
            w.group = "needs".into();
        });
        let n = parked_needs_you_by_workspace(
            &[host, ws("task", "", "needs")],
            &[q("host"), q("task")],
            &[],
        );
        assert!(!n.contains_key("host"));
        assert_eq!(n.get("task"), Some(&2));
    }

    #[test]
    fn unread_title() {
        assert_eq!(sidebar_unread_title(0, 1), "Needs you");
        assert_eq!(sidebar_unread_title(2, 1), "3 waiting");
        assert_eq!(sidebar_unread_title(2, 0), "2 unseen agent responses");
        assert_eq!(sidebar_unread_title(1, 0), "1 unseen agent response");
        assert_eq!(sidebar_unread_title(0, 0), "");
    }

    #[test]
    fn remove_rules() {
        fn shows(f: impl FnOnce(&mut Workspace)) -> bool {
            workspace_shows_remove(Some(&w(f)))
        }
        assert!(shows(|w| w.r#type = "solo".into()));
        assert!(shows(|w| {
            w.run_started = true;
            w.state = "coding".into();
        }));
        assert!(shows(|w| w.state = "merged".into()));
        assert!(!shows(|w| w.is_main = true));
        assert!(!shows(|w| w.host_pr_id = "482".into()));
        assert!(!shows(|w| w.archived = true));
        assert!(!workspace_shows_remove(None));

        let solo = w(|w| w.r#type = "solo".into());
        let run = |state: &str, phase: &str| {
            w(|w| {
                w.run_started = true;
                w.state = state.into();
                w.phase = phase.into();
            })
        };
        let can = |w: &Workspace, agent| workspace_can_remove(Some(w), agent);
        assert!(can(&solo, false));
        assert!(can(&run("pr_open", ""), false));
        assert!(can(&w(|w| w.state = "merged".into()), false));
        assert!(can(&run("paused", "idle"), false));
        assert!(can(&run("paused", "artifact"), false));
        assert!(!can(&w(|w| w.is_main = true), false));
        assert!(!can(&run("coding", ""), false));
        assert!(!can(&run("paused", "paused"), false));
        assert!(!can(&run("paused", "interrupted"), false));
        assert!(!can(&solo, true));
        assert!(!can(&run("paused", "idle"), true));
        assert!(!workspace_can_remove(None, false));
    }

    #[test]
    fn modes_follow_the_engine_rules() {
        assert_eq!(ChatMode::from_key(""), ChatMode::Agent);
        assert_eq!(ChatMode::Review.cycle(false, false), ChatMode::Loop);
        assert_eq!(ChatMode::Review.cycle(true, false), ChatMode::Ask);
        assert_eq!(ChatMode::options(true, true).len(), 3);
        assert_eq!(ChatMode::Agent.label(), "Agent");
    }
}
