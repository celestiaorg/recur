//! Role tool sets: the closed tool set of each `[[role]].mode`, bundle role
//! allowlists, slot → role resolution, and model slot mapping. Allowlist syntax
//! is Claude's (`Bash(git diff:*)`, `mcp__<server>__*`); each provider
//! translates.

use std::collections::BTreeMap;

use recur_agent_tools::names::mcp;
use recur_bundles::{
    ROLE_MODE_AGENT, ROLE_MODE_ASK, ROLE_MODE_LOOP, ROLE_MODE_PLAN, ROLE_MODE_REVIEW, Role,
    find_role,
};
use recur_domain::Settings;

use super::{
    EDIT_TOOLS, GIT_READ_TOOLS, MCP_PLAYWRIGHT_ALL, READ_EXPLORE_TOOLS, RoleSpec, SKILL_MCP_TOOLS,
};
use crate::loop_handle::RoleView;

fn own(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| (*s).to_owned()).collect()
}

/// The closed tool set for a role mode. Specialists never get interactive
/// tools (switch_mode, ask_user, rename_workspace) but do get the skill
/// tools; the conductor (loop) gets the renames and loop-flow hands.
pub fn mode_tools(mode: &str) -> Vec<String> {
    let mut ask = own(&READ_EXPLORE_TOOLS);
    ask.extend(own(&GIT_READ_TOOLS));
    ask.push(mcp::LIST_FINDINGS.to_owned());
    ask.extend(own(&SKILL_MCP_TOOLS));
    match mode.trim().to_lowercase().as_str() {
        ROLE_MODE_PLAN => {
            ask.push(mcp::SUBMIT_PLAN.to_owned());
            ask
        }
        ROLE_MODE_AGENT => {
            ask.extend(own(&EDIT_TOOLS));
            ask.push(mcp::UPDATE_FINDING.to_owned());
            ask
        }
        ROLE_MODE_REVIEW => {
            ask.extend(own(&["Bash", mcp::REPORT_FINDING, mcp::UPDATE_FINDING]));
            ask
        }
        ROLE_MODE_LOOP => {
            ask.extend(own(&[
                mcp::PLAN,
                mcp::EDIT_PLAN,
                mcp::SUBMIT_PLAN,
                mcp::UPDATE_PLAN_STEP,
                mcp::DISPATCH_SPECIALIST,
                mcp::REENTER_PHASE,
                mcp::RUN_PHASE,
                mcp::EXTEND_ROUNDS,
                mcp::MESSAGE_CHILD,
                mcp::CREATE_ARTIFACT,
                mcp::LIST_ARTIFACTS,
                mcp::READ_ARTIFACT,
                mcp::PRESENT_ARTIFACT,
                mcp::LIST_TERMINALS,
                mcp::READ_TERMINAL,
                mcp::RENAME_SESSION,
                mcp::RENAME_WORKSPACE,
                mcp::RENAME_BRANCH,
            ]));
            ask
        }
        _ => ask,
    }
}

/// done / escalate / list_findings: on every non-loop role.
pub fn loop_flow_tools() -> Vec<String> {
    own(&[mcp::DONE, mcp::ESCALATE, mcp::LIST_FINDINGS])
}

/// mode_tools ∪ loop_flow ∪ tools − disabled_tools. The reserved token
/// "playwright" turns on the browser MCP server rather than naming a tool.
pub fn role_allowlist(role: &Role) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |t: &str| {
        let t = t.trim();
        if t.is_empty() || out.iter().any(|x| x == t) {
            return;
        }
        out.push(t.to_owned());
    };
    for t in mode_tools(&role.mode) {
        add(&t);
    }
    if role.mode != ROLE_MODE_LOOP {
        for t in loop_flow_tools() {
            add(&t);
        }
    }
    let mut playwright = false;
    for raw in &role.tools {
        if raw.trim() == "playwright" {
            playwright = true;
            continue;
        }
        add(&map_role_tool(raw));
    }
    if playwright {
        add(MCP_PLAYWRIGHT_ALL);
    }
    let mut deny: Vec<String> = Vec::new();
    for d in &role.disabled_tools {
        deny.push(map_role_tool(d));
        deny.push(d.trim().to_owned());
    }
    out.retain(|t| !deny.iter().any(|d| d == t));
    out
}

/// Whether the role listed the reserved playwright token.
pub fn role_wants_playwright(role: &Role) -> bool {
    role.tools.iter().any(|t| t.trim() == "playwright")
}

/// A bundle tool name as an allowlist entry: built-ins and explicit rules
/// pass through; bare Recur names gain the `mcp__recur__` prefix.
pub fn map_role_tool(name: &str) -> String {
    let name = name.trim();
    if name.is_empty() || name == "playwright" {
        return String::new();
    }
    if name.starts_with("mcp__") || name.contains('(') {
        return name.to_owned();
    }
    match name {
        "Read" | "Glob" | "Grep" | "LSP" | "WebFetch" | "WebSearch" | "Edit" | "Write" | "Bash"
        | "NotebookEdit" => name.to_owned(),
        _ => format!("mcp__recur__{name}"),
    }
}

fn phase_role_id(view: &RoleView, slot: &str) -> String {
    if let Some(roles) = &view.phase_roles
        && let Some(id) = roles.get(slot)
        && !id.trim().is_empty()
    {
        return id.trim().to_owned();
    }
    lookup(&view.loop_roles, slot)
}

fn lookup(roles: &BTreeMap<String, String>, slot: &str) -> String {
    roles
        .get(slot)
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

/// Resolves a phase slot to a role spec: the catalog role's allowlist,
/// code, name, model slot and mode when the run names one; else `fallback`.
pub fn slot_role_spec(view: &RoleView, slot: &str, fallback: RoleSpec) -> RoleSpec {
    let id = phase_role_id(view, slot);
    if id.is_empty() {
        return fallback;
    }
    let Some(role) = find_role(&view.catalog_roles, &id) else {
        return fallback;
    };
    let mut spec = fallback;
    if !role.code.is_empty() {
        spec.code = role.code.clone();
    }
    if !role.name.is_empty() {
        spec.name = role.name.clone();
    }
    spec.allowed_tools = role_allowlist(role);
    spec.model_slot = role.model_slot.clone();
    spec.mode = role.mode.clone();
    spec
}

/// The Settings model + flags for a role's model slot (defaulting from its
/// mode); the fallback when neither is set or names no slot.
pub fn model_for_slot(
    set: &Settings,
    rs: &RoleSpec,
    fallback_model: &str,
    fallback_flags: &BTreeMap<String, String>,
) -> (String, BTreeMap<String, String>) {
    let mut slot = rs.model_slot.trim();
    if slot.is_empty() {
        slot = rs.mode.trim();
    }
    if slot.is_empty() {
        return (fallback_model.to_owned(), fallback_flags.clone());
    }
    match slot.to_lowercase().as_str() {
        ROLE_MODE_PLAN => (set.plan_model().to_string(), set.plan_flags()),
        ROLE_MODE_AGENT => (set.agent_model().to_string(), set.agent_flags()),
        ROLE_MODE_REVIEW => (set.review_model().to_string(), set.review_flags()),
        ROLE_MODE_LOOP => (set.loop_model().to_string(), set.loop_flags()),
        ROLE_MODE_ASK => (set.ask_model().to_string(), set.ask_flags()),
        _ => (fallback_model.to_owned(), fallback_flags.clone()),
    }
}
