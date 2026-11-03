//! Chat modes: the dock's mode list (labels,
//! hints, icons, tints), mode cycling, each mode's Settings default model,
//! the picker rule after a mode switch, and the create/setup predicates the
//! composer reads. The workspace run and Remove predicates live in
//! `recur_view_core::modes`.

use recur_agent::catalog::FlagDimension;
use recur_domain::{
    CHAT_MODE_AGENT, CHAT_MODE_ASK, CHAT_MODE_LOOP, CHAT_MODE_PLAN, CHAT_MODE_REVIEW, Flags,
    Settings, Workspace,
};

/// One dock selector row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatModeOption {
    pub value: &'static str,
    pub label: &'static str,
    /// One-line description for the selector row / tooltip.
    pub hint: &'static str,
    /// Settings → Models title when richer than the dock label.
    pub settings_title: &'static str,
    /// Settings → Models description when richer than the dock hint.
    pub settings_desc: &'static str,
    pub icon: &'static str,
    pub tint: &'static str,
}

/// The selector rows in display order. Hints are terse: they render
/// inline. Plan's yellow is a literal (no palette var is a true yellow).
pub const CHAT_MODES: [ChatModeOption; 5] = [
    ChatModeOption {
        value: CHAT_MODE_ASK,
        label: "Ask",
        hint: "Q&A and reports",
        settings_title: "",
        settings_desc: "",
        icon: "message-question",
        tint: "var(--emerald)",
    },
    ChatModeOption {
        value: CHAT_MODE_PLAN,
        label: "Plan",
        hint: "Investigate and plan",
        settings_title: "",
        settings_desc: "",
        icon: "notebook",
        tint: "#facc15",
    },
    ChatModeOption {
        value: CHAT_MODE_AGENT,
        label: "Agent",
        hint: "Read and edit files",
        settings_title: "Agent / Coder",
        settings_desc: "Writes and edits code in Agent chats and loop coding turns",
        icon: "message-code",
        tint: "var(--cyan)",
    },
    ChatModeOption {
        value: CHAT_MODE_REVIEW,
        label: "Review",
        hint: "Review the diff",
        settings_title: "",
        settings_desc: "",
        icon: "git-compare",
        tint: "var(--indigo)",
    },
    ChatModeOption {
        value: CHAT_MODE_LOOP,
        label: "Loop",
        hint: "Autopilot to a PR",
        settings_title: "",
        settings_desc: "",
        icon: "refresh-cw",
        tint: "var(--amber)",
    },
];

/// Placeholder tab label until the first user turn.
pub const NEW_CHAT_TITLE: &str = "New chat";

/// Unknown and empty modes degrade to agent.
pub fn normalize_chat_mode(m: &str) -> &'static str {
    recur_domain::normalize_chat_mode(m)
}

/// One mode's metadata (default agent).
pub fn chat_mode_meta(m: &str) -> ChatModeOption {
    let mode = normalize_chat_mode(m);
    CHAT_MODES
        .iter()
        .find(|o| o.value == mode)
        .copied()
        .unwrap_or(CHAT_MODES[2])
}

fn mode_rows(exclude_loop: bool, exclude_agent: bool) -> Vec<ChatModeOption> {
    CHAT_MODES
        .iter()
        .filter(|o| {
            (!exclude_loop || o.value != CHAT_MODE_LOOP)
                && (!exclude_agent || o.value != CHAT_MODE_AGENT)
        })
        .copied()
        .collect()
}

/// The next mode in selector order (Ask → Plan → Agent → Review → Loop →
/// Ask). `exclude_loop` skips Loop (the main working copy can never engage
/// the autopilot); `exclude_agent` skips Agent (PR chat hosts).
pub fn cycle_chat_mode(m: &str, exclude_loop: bool, exclude_agent: bool) -> &'static str {
    let modes = mode_rows(exclude_loop, exclude_agent);
    let cur = normalize_chat_mode(m);
    let next = modes
        .iter()
        .position(|o| o.value == cur)
        .map_or(0, |i| i + 1);
    modes[next % modes.len()].value
}

/// The dock selector's options.
pub fn chat_mode_options(exclude_loop: bool, exclude_agent: bool) -> Vec<ChatModeOption> {
    mode_rows(exclude_loop, exclude_agent)
}

/// One picker row of the flattened model catalog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CatalogModel {
    pub r#ref: String,
    pub label: String,
    pub group: String,
    pub provider: String,
    pub flags: Vec<FlagDimension>,
}

/// A selected model and flags.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelPick {
    pub model: String,
    pub flags: Flags,
}

fn model_provider(r#ref: &str) -> &str {
    match r#ref.find('/') {
        Some(i) if i > 0 => &r#ref[..i],
        _ => recur_domain::PROVIDER_CLAUDE_CODE,
    }
}

fn provider_enabled(set: Option<&Settings>, slug: &str) -> bool {
    !set.is_some_and(|s| s.disabled_providers.iter().any(|p| p == slug))
}

/// Missing dims filled from the catalog defaults; dims the model no longer
/// has dropped.
fn normalize_flags(dims: &[FlagDimension], cur: &Flags) -> Flags {
    let mut out = Flags::new();
    for d in dims {
        if let Some(v) = cur.get(&d.id)
            && !v.is_empty()
            && d.options.iter().any(|o| &o.value == v)
        {
            out.insert(d.id.clone(), v.clone());
            continue;
        }
        if let Some(def) = d.options.iter().find(|o| o.default).or(d.options.first()) {
            out.insert(d.id.clone(), def.value.clone());
        }
    }
    out
}

fn flags_for_model<'a>(catalog: &'a [CatalogModel], r#ref: &str) -> &'a [FlagDimension] {
    catalog
        .iter()
        .find(|m| m.r#ref == r#ref)
        .map_or(&[], |m| &m.flags)
}

/// The raw Settings default for a mode (one slot per mode).
pub fn default_model_for_mode(mode: &str, set: Option<&Settings>) -> ModelPick {
    let pick = |m: Option<&str>, f: Option<&Flags>| ModelPick {
        model: match m {
            Some(m) if !m.is_empty() => m.to_owned(),
            _ => recur_domain::providers::FALLBACK_CLAUDE_MODEL.to_owned(),
        },
        flags: f.cloned().unwrap_or_default(),
    };
    match normalize_chat_mode(mode) {
        CHAT_MODE_ASK => pick(set.map(|s| s.model_ask.as_str()), set.map(|s| &s.flags_ask)),
        CHAT_MODE_PLAN => pick(
            set.map(|s| s.model_plan.as_str()),
            set.map(|s| &s.flags_plan),
        ),
        CHAT_MODE_REVIEW => pick(
            set.map(|s| {
                if s.model_review.is_empty() {
                    s.model_loop.as_str()
                } else {
                    s.model_review.as_str()
                }
            }),
            set.map(|s| &s.flags_review),
        ),
        CHAT_MODE_LOOP => pick(
            set.map(|s| s.model_loop.as_str()),
            set.map(|s| &s.flags_loop),
        ),
        _ => pick(
            set.map(|s| s.model_agent.as_str()),
            set.map(|s| &s.flags_agent),
        ),
    }
}

/// The catalog-resolved default (missing flag dims filled). A slot naming
/// a turned-off provider, or a ref the catalog no longer lists, falls to
/// the first catalog model whose provider is on.
pub fn resolved_default_for_mode(
    mode: &str,
    set: Option<&Settings>,
    catalog: &[CatalogModel],
) -> ModelPick {
    let d = default_model_for_mode(mode, set);
    let (mut model, mut flags) = (d.model, d.flags);
    let enabled = provider_enabled(set, model_provider(&model));
    let listed = catalog.iter().any(|m| m.r#ref == model);
    if !catalog.is_empty()
        && (!enabled || !listed)
        && let Some(first) = catalog.iter().find(|m| {
            let p = if m.provider.is_empty() {
                model_provider(&m.r#ref)
            } else {
                &m.provider
            };
            provider_enabled(set, p)
        })
    {
        model = first.r#ref.clone();
        flags = Flags::new();
    }
    let flags = normalize_flags(flags_for_model(catalog, &model), &flags);
    ModelPick { model, flags }
}

/// The dock still shows this mode's Settings default (no override).
pub fn is_mode_default_selection(
    mode: &str,
    model: &str,
    flags: &Flags,
    set: Option<&Settings>,
    catalog: &[CatalogModel],
) -> bool {
    let d = resolved_default_for_mode(mode, set, catalog);
    model == d.model && flags == &d.flags
}

/// A chat has had a turn: a non-queued transcript row, or a tab title other
/// than the placeholder (the title survives reload before history loads).
pub fn chat_has_started(title: Option<&str>, queued_rows: Option<&[bool]>) -> bool {
    if queued_rows.is_some_and(|rows| rows.iter().any(|queued| !queued)) {
        return true;
    }
    let t = title.unwrap_or_default().trim();
    !t.is_empty() && t != NEW_CHAT_TITLE
}

/// The model and flags to keep after a mode switch. Unhydrated pickers
/// always take the new mode's default (their coder fallback is not an
/// override); an unused chat still on the old mode's default follows the
/// new one; a started session keeps its model (mode is tool policy, not a
/// mid-chat model swap); an explicit pick is kept.
#[allow(clippy::too_many_arguments)]
pub fn picker_after_mode_change(
    from_mode: &str,
    to_mode: &str,
    model: &str,
    flags: &Flags,
    set: Option<&Settings>,
    catalog: &[CatalogModel],
    picker_hydrated: bool,
    session_started: bool,
) -> ModelPick {
    if session_started && picker_hydrated {
        return ModelPick {
            model: model.to_owned(),
            flags: flags.clone(),
        };
    }
    if !picker_hydrated || is_mode_default_selection(from_mode, model, flags, set, catalog) {
        return resolved_default_for_mode(to_mode, set, catalog);
    }
    ModelPick {
        model: model.to_owned(),
        flags: flags.clone(),
    }
}

/// The chat's bouncing status while the worktree is set up or a loop slot
/// is awaited.
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

/// A create (not a loop dispatch) failed.
pub fn ws_create_failed(w: Option<&Workspace>) -> bool {
    let Some(w) = w else {
        return false;
    };
    if w.last_error.is_empty() || w.has_run() {
        return false;
    }
    w.status_label == "Couldn't create" || w.phase == "failed"
}

/// Composer sends queue until git and setup finish. The main working copy
/// and failed creates do not hold.
pub fn ws_chat_held(w: Option<&Workspace>) -> bool {
    let Some(ws) = w else {
        return false;
    };
    if ws.is_main || ws_create_failed(w) {
        return false;
    }
    let creating = ws.last_error.is_empty()
        && (ws.phase == "creating"
            || (ws.phase == "waiting for a slot" && ws.worktree_path.is_empty()));
    creating || ws.worktree_path.trim().is_empty()
}

#[cfg(test)]
#[path = "modes_tests.rs"]
mod tests;
