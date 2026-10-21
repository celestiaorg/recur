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

/// Chat-bouncing status copy while the worktree allocates (`wsSetupStatus`).
pub fn ws_setup_status(w: Option<&Workspace>) -> &'static str {
    let Some(w) = w.filter(|w| w.last_error.is_empty()) else {
        return "";
    };
    match w.phase.as_str() {
        "creating" => "Setting up the worktree…",
        "waiting for a slot" => "Waiting for a loop slot…",
        _ => "",
    }
}

/// A create that failed before the workspace ever ran (`wsCreateFailed`).
pub fn ws_create_failed(w: Option<&Workspace>) -> bool {
    let Some(w) = w else { return false };
    if w.last_error.is_empty() || ws_has_run(w) {
        return false;
    }
    w.status_label == "Couldn't create" || w.phase == "failed"
}

/// Composer send queues until git/setup finish; main and failed creates do not hold
/// (`wsChatHeld`).
pub fn ws_chat_held(w: Option<&Workspace>) -> bool {
    let Some(ws) = w else { return false };
    if ws.is_main || ws_create_failed(w) {
        return false;
    }
    ws_creating(w) || ws.worktree_path.trim().is_empty()
}

/// How a chat tab's agent may act (`ChatMode`). The mode belongs to the chat session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChatMode {
    Ask,
    Plan,
    Agent,
    Review,
    Loop,
}

/// The selector rows, in display order (`CHAT_MODES`).
pub const CHAT_MODES: [ChatMode; 5] = [
    ChatMode::Ask,
    ChatMode::Plan,
    ChatMode::Agent,
    ChatMode::Review,
    ChatMode::Loop,
];

impl ChatMode {
    pub fn key(self) -> &'static str {
        match self {
            ChatMode::Ask => "ask",
            ChatMode::Plan => "plan",
            ChatMode::Agent => "agent",
            ChatMode::Review => "review",
            ChatMode::Loop => "loop",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ChatMode::Ask => "Ask",
            ChatMode::Plan => "Plan",
            ChatMode::Agent => "Agent",
            ChatMode::Review => "Review",
            ChatMode::Loop => "Loop",
        }
    }

    /// The one-line description in the selector row.
    pub fn hint(self) -> &'static str {
        match self {
            ChatMode::Ask => "Q&A and reports",
            ChatMode::Plan => "Investigate and plan",
            ChatMode::Agent => "Read and edit files",
            ChatMode::Review => "Review the diff",
            ChatMode::Loop => "Autopilot to a PR",
        }
    }

    /// Settings → Models title.
    pub fn settings_title(self) -> &'static str {
        match self {
            ChatMode::Agent => "Agent / Coder",
            m => m.label(),
        }
    }

    /// Settings → Models description.
    pub fn settings_desc(self) -> &'static str {
        match self {
            ChatMode::Agent => "Writes and edits code in Agent chats and loop coding turns",
            m => m.hint(),
        }
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

    /// The accent: Loop amber (Maestro), Agent cyan (coding agent), Review indigo (Reviewer),
    /// Ask emerald, and Plan's literal yellow (no palette var is a true yellow).
    pub fn tint(self, t: &recur_ui::Tokens) -> recur_ui::Color {
        match self {
            ChatMode::Ask => t.emerald,
            ChatMode::Plan => recur_ui::hex(0xfacc15),
            ChatMode::Agent => t.cyan,
            ChatMode::Review => t.indigo,
            ChatMode::Loop => t.amber,
        }
    }
}

/// `normalizeChatMode`: unknown and empty degrade to agent.
pub fn normalize_chat_mode(m: &str) -> ChatMode {
    match m {
        "ask" => ChatMode::Ask,
        "plan" => ChatMode::Plan,
        "review" => ChatMode::Review,
        "loop" => ChatMode::Loop,
        _ => ChatMode::Agent,
    }
}

/// `chatModeMeta` keyed by the stored string.
pub fn chat_mode_meta(m: &str) -> ChatMode {
    normalize_chat_mode(m)
}

fn mode_offered(m: ChatMode, exclude_loop: bool, exclude_agent: bool) -> bool {
    !(exclude_loop && m == ChatMode::Loop) && !(exclude_agent && m == ChatMode::Agent)
}

/// `cycleChatMode`: Ask → Plan → Agent → Review → Loop → Ask. `exclude_loop` for the main
/// working copy; `exclude_agent` for PR chat hosts.
pub fn cycle_chat_mode(m: &str, exclude_loop: bool, exclude_agent: bool) -> ChatMode {
    let modes: Vec<ChatMode> = CHAT_MODES
        .into_iter()
        .filter(|&o| mode_offered(o, exclude_loop, exclude_agent))
        .collect();
    let cur = normalize_chat_mode(m);
    let next = modes.iter().position(|&o| o == cur).map_or(0, |i| i + 1);
    modes[next % modes.len()]
}

/// The modes the dock selector offers (`chatModeOptions`).
pub fn chat_mode_options(exclude_loop: bool, exclude_agent: bool) -> Vec<ChatMode> {
    CHAT_MODES
        .into_iter()
        .filter(|&o| mode_offered(o, exclude_loop, exclude_agent))
        .collect()
}

const FALLBACK_MODEL: &str = "claude-code/claude-sonnet-5-5";

/// The raw Settings default for a chat mode (`defaultModelForMode`).
pub fn default_model_for_mode(
    mode: &str,
    set: Option<&recur_domain::Settings>,
) -> (String, crate::models::FlagMap) {
    let pick = |m: Option<&String>| {
        m.filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(|| FALLBACK_MODEL.to_string())
    };
    let Some(s) = set else {
        return (FALLBACK_MODEL.to_string(), Default::default());
    };
    match normalize_chat_mode(mode) {
        ChatMode::Ask => (pick(Some(&s.model_ask)), s.flags_ask.clone()),
        ChatMode::Plan => (pick(Some(&s.model_plan)), s.flags_plan.clone()),
        ChatMode::Review => {
            let m = if s.model_review.is_empty() {
                &s.model_loop
            } else {
                &s.model_review
            };
            (pick(Some(m)), s.flags_review.clone())
        }
        ChatMode::Loop => (pick(Some(&s.model_loop)), s.flags_loop.clone()),
        ChatMode::Agent => (pick(Some(&s.model_agent)), s.flags_agent.clone()),
    }
}

/// `resolvedDefaultForMode`: the Settings default with missing flag dimensions filled; a slot
/// naming a turned-off provider (or a ref the catalog no longer lists) falls to the first
/// catalog model whose provider is on.
pub fn resolved_default_for_mode(
    mode: &str,
    set: Option<&recur_domain::Settings>,
    catalog: &[crate::models::CatalogModel],
) -> (String, crate::models::FlagMap) {
    use crate::models::{flags_for_model, model_provider, normalize_flags, provider_enabled};
    let (mut model, mut flags) = default_model_for_mode(mode, set);
    let disabled: &[String] = set.map_or(&[], |s| &s.disabled_providers);
    let enabled = provider_enabled(disabled, model_provider(&model));
    let listed = catalog.iter().any(|m| m.model_ref == model);
    if !catalog.is_empty()
        && (!enabled || !listed)
        && let Some(first) = catalog.iter().find(|m| {
            let p = if m.provider.is_empty() {
                model_provider(&m.model_ref)
            } else {
                &m.provider
            };
            provider_enabled(disabled, p)
        })
    {
        model = first.model_ref.clone();
        flags = Default::default();
    }
    let flags = normalize_flags(&flags_for_model(catalog, &model), Some(&flags));
    (model, flags)
}

/// `isModeDefaultSelection`: the dock still shows this mode's Settings default.
pub fn is_mode_default_selection(
    mode: &str,
    model: &str,
    flags: &crate::models::FlagMap,
    set: Option<&recur_domain::Settings>,
    catalog: &[crate::models::CatalogModel],
) -> bool {
    let (m, f) = resolved_default_for_mode(mode, set, catalog);
    model == m && *flags == f
}

/// The tab label until the first user turn (Go `chatTabTitle`).
pub const NEW_CHAT_TITLE: &str = "New chat";

/// `chatHasStarted`: transcript rows (not queued), or a tab title other than the placeholder.
pub fn chat_has_started(title: &str, unqueued_rows: bool) -> bool {
    if unqueued_rows {
        return true;
    }
    let t = title.trim();
    !t.is_empty() && t != NEW_CHAT_TITLE
}

/// `pickerAfterModeChange`: the model and flags to keep after a mode switch. Unhydrated
/// pickers take the new mode's default; an unused chat still on the old default follows the
/// new one; a started session or an explicit pick keeps its model.
#[allow(clippy::too_many_arguments)]
pub fn picker_after_mode_change(
    from_mode: &str,
    to_mode: &str,
    model: &str,
    flags: &crate::models::FlagMap,
    set: Option<&recur_domain::Settings>,
    catalog: &[crate::models::CatalogModel],
    picker_hydrated: bool,
    session_started: bool,
) -> (String, crate::models::FlagMap) {
    if session_started && picker_hydrated {
        return (model.to_string(), flags.clone());
    }
    if !picker_hydrated || is_mode_default_selection(from_mode, model, flags, set, catalog) {
        return resolved_default_for_mode(to_mode, set, catalog);
    }
    (model.to_string(), flags.clone())
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

    fn settings() -> recur_domain::Settings {
        let flags = |v: &str| {
            let mut m = crate::models::FlagMap::new();
            m.insert("effort".into(), v.into());
            m
        };
        recur_domain::Settings {
            model_ask: "claude-code/haiku".into(),
            model_plan: "claude-code/opus".into(),
            model_agent: "claude-code/sonnet".into(),
            model_review: "claude-code/fable".into(),
            model_loop: "claude-code/opus".into(),
            flags_ask: flags("low"),
            flags_plan: flags("high"),
            flags_agent: flags("medium"),
            flags_review: flags("high"),
            flags_loop: flags("high"),
            ..Default::default()
        }
    }

    fn effort(default: &str) -> recur_services::api::FlagDimension {
        use recur_services::api::{FlagDimension, FlagOption};
        FlagDimension {
            id: "effort".into(),
            label: "Effort".into(),
            options: ["low", "medium", "high"]
                .into_iter()
                .map(|v| FlagOption {
                    value: v.into(),
                    label: v.into(),
                    default: v == default,
                })
                .collect(),
        }
    }

    fn model(r: &str, provider: &str, flags: Vec<recur_services::api::FlagDimension>) -> crate::models::CatalogModel {
        crate::models::CatalogModel {
            model_ref: r.into(),
            label: r.into(),
            group: provider.into(),
            provider: provider.into(),
            flags,
        }
    }

    fn catalog() -> Vec<crate::models::CatalogModel> {
        vec![
            model("claude-code/opus", "claude-code", vec![effort("high")]),
            model("claude-code/sonnet", "claude-code", vec![effort("medium")]),
            model("claude-code/fable", "claude-code", vec![effort("high")]),
            model("claude-code/haiku", "claude-code", vec![effort("low")]),
        ]
    }

    fn cursor_catalog() -> Vec<crate::models::CatalogModel> {
        use recur_services::api::{FlagDimension, FlagOption};
        let fast = FlagDimension {
            id: "fast".into(),
            label: "Fast".into(),
            options: vec![
                FlagOption { value: "true".into(), label: "On".into(), default: true },
                FlagOption { value: "false".into(), label: "Off".into(), default: false },
            ],
        };
        vec![model("cursor/composer-2.5", "cursor", vec![fast])]
    }

    fn fm(pairs: &[(&str, &str)]) -> crate::models::FlagMap {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    // modes.test.ts
    #[test]
    fn default_model_per_mode() {
        let s = settings();
        assert_eq!(default_model_for_mode("ask", Some(&s)).0, "claude-code/haiku");
        assert_eq!(default_model_for_mode("plan", Some(&s)).0, "claude-code/opus");
        assert_eq!(default_model_for_mode("agent", Some(&s)).0, "claude-code/sonnet");
        assert_eq!(default_model_for_mode("review", Some(&s)).0, "claude-code/fable");
        assert_eq!(default_model_for_mode("loop", Some(&s)).0, "claude-code/opus");
    }

    #[test]
    fn resolved_default_skips_a_disabled_provider() {
        let mut off = settings();
        off.disabled_providers = vec!["claude-code".into()];
        let mut mixed = catalog();
        mixed.extend(cursor_catalog());
        let (m, f) = resolved_default_for_mode("agent", Some(&off), &mixed);
        assert_eq!(m, "cursor/composer-2.5");
        assert_eq!(f, fm(&[("fast", "true")]));
        let mut remapped = off.clone();
        remapped.model_agent = "cursor/composer-2.5".into();
        remapped.flags_agent = fm(&[("fast", "false")]);
        let (m, f) = resolved_default_for_mode("agent", Some(&remapped), &cursor_catalog());
        assert_eq!(m, "cursor/composer-2.5");
        assert_eq!(f, fm(&[("fast", "false")]));
    }

    #[test]
    fn picker_after_mode_change_rules() {
        let s = settings();
        let c = catalog();
        let want = resolved_default_for_mode("review", Some(&s), &c);
        assert_eq!(
            picker_after_mode_change("agent", "review", &s.model_agent, &fm(&[]), Some(&s), &c, false, false),
            want
        );
        let from = resolved_default_for_mode("agent", Some(&s), &c);
        assert!(is_mode_default_selection("agent", &from.0, &from.1, Some(&s), &c));
        assert_eq!(
            picker_after_mode_change("agent", "review", &from.0, &from.1, Some(&s), &c, true, false),
            want
        );
        assert_eq!(
            picker_after_mode_change("agent", "ask", &from.0, &from.1, Some(&s), &c, true, true),
            from
        );
        let low = fm(&[("effort", "low")]);
        assert_eq!(
            picker_after_mode_change("agent", "review", "claude-code/opus", &low, Some(&s), &c, true, false),
            ("claude-code/opus".to_string(), low.clone())
        );
        assert_eq!(
            picker_after_mode_change("agent", "plan", "claude-code/opus", &low, Some(&s), &c, true, true),
            ("claude-code/opus".to_string(), low)
        );
        for to in ["plan", "ask"] {
            assert_eq!(
                picker_after_mode_change("agent", to, &s.model_agent, &fm(&[]), Some(&s), &c, false, false),
                resolved_default_for_mode(to, Some(&s), &c)
            );
        }
    }

    #[test]
    fn chat_started() {
        assert!(!chat_has_started("", false));
        assert!(!chat_has_started("New chat", false));
        assert!(chat_has_started("New chat", true));
        assert!(chat_has_started("Fix the dock", false));
        assert!(chat_has_started("Image", false));
    }

    #[test]
    fn setup_status_and_hold() {
        let st = |f: fn(&mut Workspace)| ws_setup_status(Some(&w(f)));
        assert_eq!(st(|w| w.phase = "creating".into()), "Setting up the worktree…");
        assert_eq!(
            st(|w| {
                w.phase = "creating".into();
                w.worktree_path = "/tmp/wt".into();
            }),
            "Setting up the worktree…"
        );
        assert_eq!(st(|w| w.phase = "waiting for a slot".into()), "Waiting for a loop slot…");
        assert_eq!(
            st(|w| {
                w.phase = "starting".into();
                w.worktree_path = "/tmp/wt".into();
            }),
            ""
        );
        let held = |f: fn(&mut Workspace)| ws_chat_held(Some(&w(f)));
        assert!(held(|w| w.phase = "creating".into()));
        assert!(held(|w| {
            w.phase = "creating".into();
            w.worktree_path = "/tmp/wt".into();
        }));
        assert!(held(|w| w.phase = "editing".into()));
        assert!(!held(|w| {
            w.phase = "editing".into();
            w.worktree_path = "/tmp/wt".into();
        }));
        assert!(!held(|w| {
            w.is_main = true;
            w.phase = "creating".into();
        }));
        assert!(!held(|w| {
            w.phase = "failed".into();
            w.last_error = "git failed".into();
            w.status_label = "Couldn't create".into();
            w.r#type = "solo".into();
        }));
    }

    #[test]
    fn create_failed_rules() {
        let failed = |f: fn(&mut Workspace)| ws_create_failed(Some(&w(f)));
        assert!(failed(|w| {
            w.phase = "failed".into();
            w.last_error = "setup command failed".into();
            w.worktree_path = "/tmp/wt".into();
            w.status_label = "Couldn't create".into();
            w.r#type = "solo".into();
        }));
        assert!(!failed(|w| {
            w.phase = "failed".into();
            w.last_error = "agent died".into();
            w.worktree_path = "/tmp/wt".into();
            w.status_label = "Failed".into();
            w.run_started = true;
        }));
        assert!(!failed(|w| {
            w.phase = "failed".into();
            w.last_error = "dispatch failed".into();
            w.status_label = "Couldn't create".into();
            w.run_started = true;
        }));
    }

    #[test]
    fn cycle_and_options() {
        assert_eq!(cycle_chat_mode("ask", false, false), ChatMode::Plan);
        assert_eq!(cycle_chat_mode("loop", false, false), ChatMode::Ask);
        assert_eq!(cycle_chat_mode("review", true, false), ChatMode::Ask);
        assert_eq!(cycle_chat_mode("plan", false, true), ChatMode::Review);
        assert_eq!(cycle_chat_mode("bogus", false, false), ChatMode::Review);
        assert_eq!(chat_mode_options(true, true).len(), 3);
        assert_eq!(chat_mode_meta("").label(), "Agent");
        assert_eq!(chat_mode_meta("plan").hint(), "Investigate and plan");
    }
}
