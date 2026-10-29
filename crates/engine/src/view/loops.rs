//! The `[[loop]]` + `[[policy]]` picker rules:
//! shipped ids, the Settings enable/default rules, a workspace's effective
//! pick, select options, the legacy autonomy mapping and the shipped
//! fallback catalog. The service calls (`ListLoops`, `CreateWorkspaceWith`,
//! `RememberChatLoopPolicy`, `EngageLoopWith`) are direct engine calls in
//! `recur-app`.

use recur_domain::{Settings, Workspace};

pub const DEFAULT_LOOP_ID: &str = "recur.loops.pr";
pub const CONVERGE_LOOP_ID: &str = "recur.loops.converge";
pub const CONVERGE_PR_LOOP_ID: &str = "recur.loops.converge-pr";
pub const DOCS_LOOP_ID: &str = "recur.loops.docs";
pub const SECURITY_LOOP_ID: &str = "recur.loops.security";
pub const DEFAULT_POLICY_ID: &str = "recur.policies.balanced";

/// A bundle `[[loop]]` in a picker.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoopInfo {
    pub id: String,
    pub name: String,
    pub hint: String,
}

/// A bundle `[[policy]]` in a picker.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PolicyInfo {
    pub id: String,
    pub name: String,
    pub hint: String,
}

/// One picker row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
    pub hint: String,
}

pub fn enabled_loops(list: &[LoopInfo], set: Option<&Settings>) -> Vec<LoopInfo> {
    let disabled = set
        .map(|s| s.disabled_bundle_loops.as_slice())
        .unwrap_or_default();
    list.iter()
        .filter(|l| !l.id.is_empty() && !disabled.contains(&l.id))
        .cloned()
        .collect()
}

pub fn enabled_policies(list: &[PolicyInfo], set: Option<&Settings>) -> Vec<PolicyInfo> {
    let disabled = set
        .map(|s| s.disabled_bundle_policies.as_slice())
        .unwrap_or_default();
    list.iter()
        .filter(|p| !p.id.is_empty() && !disabled.contains(&p.id))
        .cloned()
        .collect()
}

fn trimmed_or(v: Option<&str>, fallback: &str) -> String {
    match v.map(super::js::trim) {
        Some(s) if !s.is_empty() => s.to_owned(),
        _ => fallback.to_owned(),
    }
}

pub fn default_loop_id(set: Option<&Settings>) -> String {
    trimmed_or(set.map(|s| s.default_loop.as_str()), DEFAULT_LOOP_ID)
}

pub fn default_policy_id(set: Option<&Settings>) -> String {
    trimmed_or(set.map(|s| s.default_policy.as_str()), DEFAULT_POLICY_ID)
}

pub fn workspace_loop_id(ws: Option<&Workspace>, set: Option<&Settings>) -> String {
    trimmed_or(ws.map(|w| w.loop_id.as_str()), &default_loop_id(set))
}

pub fn workspace_policy_id(ws: Option<&Workspace>, set: Option<&Settings>) -> String {
    trimmed_or(ws.map(|w| w.policy_id.as_str()), &default_policy_id(set))
}

fn option(id: &str, name: &str, hint: &str) -> SelectOption {
    SelectOption {
        value: id.to_owned(),
        label: if name.is_empty() { id } else { name }.to_owned(),
        hint: hint.to_owned(),
    }
}

pub fn loop_select_options(list: &[LoopInfo]) -> Vec<SelectOption> {
    list.iter()
        .map(|l| option(&l.id, &l.name, &l.hint))
        .collect()
}

pub fn policy_select_options(list: &[PolicyInfo]) -> Vec<SelectOption> {
    list.iter()
        .map(|p| option(&p.id, &p.name, &p.hint))
        .collect()
}

/// A shipped (or similarly named) policy as the legacy autonomy field.
pub fn autonomy_from_policy_id(id: &str) -> &'static str {
    let s = id.to_lowercase();
    if s.contains("cautious") {
        "cautious"
    } else if s.contains("aggressive") {
        "aggressive"
    } else {
        "balanced"
    }
}

fn info(id: &str, name: &str, hint: &str) -> (String, String, String) {
    (id.to_owned(), name.to_owned(), hint.to_owned())
}

/// The shipped loops, for when the catalog call fails.
pub fn fallback_loops() -> Vec<LoopInfo> {
    [
        info(
            DEFAULT_LOOP_ID,
            "To a PR",
            "Plan, implement, review, open a PR",
        ),
        info(
            CONVERGE_LOOP_ID,
            "Review & fix",
            "Review and fix the current diff until the bar passes",
        ),
        info(
            CONVERGE_PR_LOOP_ID,
            "Review & fix to a PR",
            "Review and fix the current diff, then open a PR",
        ),
        info(
            DOCS_LOOP_ID,
            "Docs",
            "Plan, write, and edit documentation — no PR",
        ),
        info(
            SECURITY_LOOP_ID,
            "Security review & fix",
            "Review and fix auth, secrets, and permissions until the bar passes",
        ),
    ]
    .into_iter()
    .map(|(id, name, hint)| LoopInfo { id, name, hint })
    .collect()
}

/// The shipped policies, for when the catalog call fails.
pub fn fallback_policies() -> Vec<PolicyInfo> {
    [
        info(
            "recur.policies.cautious",
            "Cautious",
            "Close every finding; escalate after 2 rounds",
        ),
        info(
            DEFAULT_POLICY_ID,
            "Balanced",
            "Fix medium and up; escalate after 4 rounds",
        ),
        info(
            "recur.policies.aggressive",
            "Aggressive",
            "Ship-blockers only; proceed after 6 rounds",
        ),
    ]
    .into_iter()
    .map(|(id, name, hint)| PolicyInfo { id, name, hint })
    .collect()
}

#[cfg(test)]
#[path = "loops_tests.rs"]
mod tests;
