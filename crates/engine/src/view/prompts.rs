//! Bundle `[[prompt]]` chat-action rules:
//! slash lookup, arg slots, the `/` menu gates, tab routing, the prompt
//! context and failure wording. Listing and running go through the engine
//! (`Engine::run_prompt_action`); the titlebar request store is app state.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use recur_bundles::{PromptAction, PromptArgSpec, PromptRunContext};
use recur_domain::Settings;
use regex::Regex;

use super::composer::{PromptChipSpec, parse_prompt_chip_values};
use super::js;
use super::mentions::{MentionQuery, skill_query_at};

/// A chat tab as prompt routing sees it.
pub trait PromptTab {
    fn code(&self) -> &str;
    fn prompt_group(&self) -> &str;
}

/// Where a mode-routed prompt lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptTabPick {
    Current,
    Select(String),
    Create,
}

/// [`pick_prompt_tab`]'s inputs.
pub struct PromptTabQuery<'a, T> {
    pub tabs: &'a [T],
    pub active_code: &'a str,
    pub mode: &'a str,
    pub group: &'a str,
    pub is_busy: &'a dyn Fn(&str) -> bool,
    pub tab_mode: &'a dyn Fn(&T) -> String,
    pub started: Option<&'a dyn Fn(&T) -> bool>,
    pub drafted: Option<&'a dyn Fn(&T) -> bool>,
    pub conductor_locked: Option<&'a dyn Fn(&str) -> bool>,
}

/// Picks the tab for a mode-routed prompt. Ungrouped: the nearest idle tab
/// in the mode, else create. Grouped: the nearest idle tab already in the
/// group (with no unsent draft), else an unused tab in the mode, else create.
/// MA and busy tabs are never picked.
pub fn pick_prompt_tab<T: PromptTab>(q: &PromptTabQuery<'_, T>) -> PromptTabPick {
    let mode_ok = |tab: &T| {
        tab.code() != crate::codes::MAESTRO_AGENT_CODE
            && !(q.is_busy)(tab.code())
            && (q.tab_mode)(tab) == q.mode
            && !(q.mode == "loop" && q.conductor_locked.is_some_and(|f| f(tab.code())))
    };
    let drafted = |tab: &T| q.drafted.is_some_and(|f| f(tab));
    let started = |tab: &T| q.started.is_some_and(|f| f(tab));
    let newest = |pred: &dyn Fn(&T) -> bool| q.tabs.iter().rev().find(|t| pred(t));
    let pick = |tab: &T| {
        if tab.code() == q.active_code {
            PromptTabPick::Current
        } else {
            PromptTabPick::Select(tab.code().to_owned())
        }
    };
    let current = q.tabs.iter().find(|t| t.code() == q.active_code);
    if !q.group.is_empty() {
        let in_group = |tab: &T| mode_ok(tab) && tab.prompt_group() == q.group && !drafted(tab);
        if current.is_some_and(in_group) {
            return PromptTabPick::Current;
        }
        if let Some(t) = newest(&in_group) {
            return pick(t);
        }
        let unused = |tab: &T| mode_ok(tab) && !started(tab) && !drafted(tab);
        if current.is_some_and(unused) {
            return PromptTabPick::Current;
        }
        return newest(&unused).map_or(PromptTabPick::Create, pick);
    }
    if current.is_some_and(mode_ok) {
        return PromptTabPick::Current;
    }
    newest(&mode_ok).map_or(PromptTabPick::Create, pick)
}

/// Prompt ids disabled in Settings → Bundles.
pub fn disabled_prompt_ids(set: &Settings) -> Vec<String> {
    set.disabled_bundle_prompts
        .iter()
        .filter(|id| !id.is_empty())
        .cloned()
        .collect()
}

pub fn prompt_enabled(set: &Settings, id: &str) -> bool {
    !set.disabled_bundle_prompts
        .iter()
        .any(|d| !d.is_empty() && d == id)
}

/// The catalog minus prompts disabled in Settings.
pub fn filter_enabled_prompts(set: &Settings, list: Vec<PromptAction>) -> Vec<PromptAction> {
    list.into_iter()
        .filter(|p| prompt_enabled(set, &p.id))
        .collect()
}

pub fn find_prompt_by_slash<'a>(slash: &str, list: &'a [PromptAction]) -> Option<&'a PromptAction> {
    let s = slash.strip_prefix('/').unwrap_or(slash).to_lowercase();
    list.iter().find(|p| p.slash == s)
}

/// Whether picking from `/` inserts arg slots instead of invoking.
pub fn prompt_takes_args(info: Option<&PromptAction>) -> bool {
    info.is_some_and(|p| p.takes_args || !p.args.is_empty())
}

/// The composer slots of a takes-args prompt (`args` is synthesized when the
/// manifest names none).
pub fn prompt_arg_specs(info: Option<&PromptAction>) -> Vec<PromptArgSpec> {
    let Some(p) = info else {
        return Vec::new();
    };
    let listed: Vec<PromptArgSpec> = p
        .args
        .iter()
        .filter(|a| !a.name.is_empty())
        .map(|a| PromptArgSpec {
            name: a.name.clone(),
            placeholder: if a.placeholder.is_empty() {
                p.arg_placeholder.clone()
            } else {
                a.placeholder.clone()
            },
            optional: a.optional,
        })
        .collect();
    if !listed.is_empty() {
        return listed;
    }
    if p.takes_args {
        return vec![PromptArgSpec {
            name: "args".into(),
            placeholder: p.arg_placeholder.clone(),
            optional: false,
        }];
    }
    Vec::new()
}

/// Whether every slot is optional (an empty Enter invokes).
pub fn prompt_args_optional(info: Option<&PromptAction>) -> bool {
    let specs = prompt_arg_specs(info);
    !specs.is_empty() && specs.iter().all(|s| s.optional)
}

/// Whether the focused slot (or the first) is optional.
pub fn prompt_arg_slot_optional(info: Option<&PromptAction>, arg: &str) -> bool {
    let specs = prompt_arg_specs(info);
    let focused = (!arg.is_empty())
        .then(|| specs.iter().find(|s| s.name == arg))
        .flatten();
    focused.or(specs.first()).is_some_and(|s| s.optional)
}

/// Whether every required slot has a value. No specs (catalog loading, or
/// not a takes-args prompt) is not invokable.
pub fn prompt_required_args_filled(info: Option<&PromptAction>, values: &[String]) -> bool {
    let specs = prompt_arg_specs(info);
    !specs.is_empty()
        && specs
            .iter()
            .enumerate()
            .all(|(i, s)| s.optional || values.get(i).is_some_and(|v| !js::trim(v).is_empty()))
}

/// Named values from the composer remainder.
pub fn params_from_args(info: Option<&PromptAction>, args: &str) -> BTreeMap<String, String> {
    let specs = prompt_arg_specs(info);
    if specs.is_empty() {
        return BTreeMap::new();
    }
    let spec = PromptChipSpec {
        slash: info.map(|p| p.slash.clone()).unwrap_or_default(),
        args: specs.clone(),
    };
    let values = parse_prompt_chip_values(args, &spec);
    specs
        .into_iter()
        .enumerate()
        .map(|(i, s)| (s.name, values.get(i).cloned().unwrap_or_default()))
        .collect()
}

/// The `/` menu gate for prompt actions: the slash token at offset 0 with
/// only whitespace after it (callers also require no attachments).
pub fn prompt_action_query_at(text: &str, caret: usize) -> Option<MentionQuery> {
    let q = skill_query_at(text, caret)?;
    if q.start != 0 || !text[q.end..].chars().all(js::is_space) {
        return None;
    }
    Some(q)
}

static COMMITTED_PROMPT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^/([a-z][a-z0-9-]*)(?:[{}]+([\s\S]*))?$",
        js::WS_CLASS
    ))
    .expect("committed prompt re")
});

static PROMPT_ARGS_LINE: LazyLock<Regex> = LazyLock::new(|| {
    let ws = js::WS_CLASS;
    Regex::new(&format!(r"^([{ws}]*)/([a-z][a-z0-9-]*)([{ws}]+)([\s\S]*)$"))
        .expect("prompt args re")
});

/// A whole-composer `/slash` or `/slash args` (trimmed), for Enter/Send when
/// the menu is not handling the key. Mid-line slashes are `None`.
pub fn parse_committed_prompt_line(text: &str) -> Option<(String, String)> {
    let c = COMMITTED_PROMPT.captures(js::trim(text))?;
    Some((
        c[1].to_owned(),
        js::trim(c.get(2).map_or("", |m| m.as_str())).to_owned(),
    ))
}

/// The args region of a whole-composer `/slash args` with the caret in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptArgsQuery {
    pub slash: String,
    pub args: String,
    pub args_start: usize,
    /// The active `[[prompt.arg]]` name when the caret is in a labeled slot.
    pub arg: String,
}

/// [`PromptArgsQuery`] while the caret is past `/slash `. Unlike
/// [`prompt_action_query_at`] this stays open once args start, for
/// parameter completion.
pub fn prompt_args_query_at(
    text: &str,
    caret: usize,
    takes_args: &dyn Fn(&str) -> bool,
) -> Option<PromptArgsQuery> {
    if caret > text.len() {
        return None;
    }
    let c = PROMPT_ARGS_LINE.captures(text)?;
    let slash = &c[2];
    if !takes_args(slash) {
        return None;
    }
    let prefix_len = c[1].len() + 1 + slash.len();
    if caret < prefix_len {
        return None;
    }
    Some(PromptArgsQuery {
        slash: slash.to_owned(),
        args: js::trim_end(&c[4]).to_owned(),
        args_start: prefix_len + c[3].len(),
        arg: String::new(),
    })
}

/// The workspace fields a prompt context needs.
#[derive(Debug, Clone, Default)]
pub struct PromptWorkspace {
    pub id: String,
    pub project_id: String,
    pub worktree_path: String,
    pub port: i64,
}

/// The editor state a prompt sees (`Partial<EditorContext>`: unset fields
/// take the defaults below).
#[derive(Debug, Clone, Default)]
pub struct PromptEditor {
    pub path: Option<String>,
    pub selected_text: Option<String>,
    pub document: Option<String>,
    pub current_line: Option<String>,
    pub line_number: Option<i64>,
    pub column: Option<i64>,
    pub current_word: Option<String>,
    pub scope: Option<String>,
    pub clipboard: Option<String>,
    pub tab_size: Option<i64>,
    pub soft_tabs: Option<bool>,
}

/// The run context for a composer prompt.
/// `surface` defaults to "workspace".
pub fn build_prompt_context(
    ws: &PromptWorkspace,
    editor: Option<&PromptEditor>,
    surface: &str,
    args: &str,
    params: BTreeMap<String, String>,
) -> PromptRunContext {
    let e = editor.cloned().unwrap_or_default();
    let path = e.path.unwrap_or_default();
    PromptRunContext {
        workspace_id: ws.id.clone(),
        project_id: ws.project_id.clone(),
        worktree: ws.worktree_path.clone(),
        work_dir: ws.worktree_path.clone(),
        port: ws.port,
        selected_text: e.selected_text.unwrap_or_default(),
        document: e.document.unwrap_or_default(),
        current_line: e.current_line.unwrap_or_default(),
        line_number: e.line_number.unwrap_or_default(),
        column: e.column.unwrap_or_default(),
        current_word: e.current_word.unwrap_or_default(),
        file_name: path.rsplit('/').next().unwrap_or_default().to_owned(),
        file_path: path,
        scope: e.scope.unwrap_or_default(),
        clipboard: e.clipboard.unwrap_or_default(),
        tab_size: e.tab_size.unwrap_or(4),
        soft_tabs: e.soft_tabs.unwrap_or(true),
        surface: if surface.is_empty() {
            "workspace".into()
        } else {
            surface.into()
        },
        args: args.into(),
        params,
        ..PromptRunContext::default()
    }
}

static EXITED_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"(?i)^prompt "[^"]+" exited [0-9]+:[{}]*"#,
        js::WS_CLASS
    ))
    .expect("exited re")
});

/// The human message of a prompt failure: a RuntimeError JSON blob is
/// unwrapped and the shebang `prompt "id" exited N:` prefix dropped.
pub fn prompt_action_error_message(err: &str) -> String {
    let mut raw = err.to_owned();
    if raw.starts_with('{')
        && let Some(o) = js::parse_object(&raw)
        && let Some(m) = o.get("message").and_then(|m| m.as_str())
        && !m.is_empty()
    {
        raw = m.to_owned();
    }
    let stripped = EXITED_PREFIX.replace(&raw, "");
    let stripped = js::trim(&stripped);
    if !stripped.is_empty() {
        return stripped.to_owned();
    }
    let t = js::trim(&raw);
    if t.is_empty() {
        "Prompt failed".into()
    } else {
        t.to_owned()
    }
}

#[cfg(test)]
#[path = "prompts_tests.rs"]
mod tests;
