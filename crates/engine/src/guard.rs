//! Run guards: resolve a run's `[[loop]]` and `[[policy]]` from the bundle
//! catalog (or the shipped synthetic defaults), derive a fan-out child's loop
//! from its parent's converge phase, and the findings bar math. The catalog is
//! re-read on every call.

use std::collections::{BTreeMap, HashMap};

use recur_bundles::{
    Catalog, DEFAULT_LOOP_ID, DEFAULT_POLICY_ID, FindingsThreshold, LoopDef, LoopPhase, Policy,
    STEP_LOOP_ID, VERB_ARTIFACT, VERB_CONVERGE, VERB_ENSURE_PR, VERB_PLAN, find_loop, find_policy,
    find_role,
};
use recur_domain::{AutonomyLevel, Settings, Workspace};

use crate::Engine;
use crate::text::plural;

/// The policy and loop applied to a run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolvedGuard {
    pub policy: Policy,
    pub loop_def: LoopDef,
    pub from_bundle: bool,
}

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn levels(pairs: &[(&str, i64)]) -> BTreeMap<String, i64> {
    pairs.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect()
}

fn contains_id(ids: &[String], id: &str) -> bool {
    ids.iter().any(|x| x == id)
}

impl Engine {
    /// The bundle catalog from the user and default bundle roots.
    pub fn discover_loop_catalog(&self) -> Result<Catalog, recur_bundles::Error> {
        recur_bundles::discover_catalog(&self.bundle_roots)
    }

    /// The refusal when loop/policy cannot be engaged (missing, disabled,
    /// or a referenced role is disabled). Empty is OK.
    pub fn guard_reason(&self, loop_id: &str, policy_id: &str, autonomy: &AutonomyLevel) -> String {
        match self.resolve_guard(loop_id, policy_id, autonomy) {
            Ok(_) => String::new(),
            Err(reason) => reason,
        }
    }

    /// Resolves a run's guard. Empty ids fall back to the Settings defaults,
    /// then the shipped To a PR loop and the autonomy's policy.
    pub fn resolve_guard(
        &self,
        loop_id: &str,
        policy_id: &str,
        autonomy: &AutonomyLevel,
    ) -> Result<ResolvedGuard, String> {
        let settings = self.store.settings();
        let mut loop_id = loop_id.trim().to_owned();
        let mut policy_id = policy_id.trim().to_owned();
        if loop_id.is_empty() {
            loop_id = settings.default_loop.trim().to_owned();
        }
        if loop_id.is_empty() {
            loop_id = DEFAULT_LOOP_ID.to_owned();
        }
        if policy_id.is_empty() {
            policy_id = settings.default_policy.trim().to_owned();
        }
        if policy_id.is_empty() {
            policy_id = recur_bundles::policy_from_autonomy(autonomy.as_str()).to_owned();
        }
        if contains_id(&settings.disabled_bundle_loops, &loop_id) {
            return Err(format!("loop {loop_id} is disabled"));
        }
        if contains_id(&settings.disabled_bundle_policies, &policy_id) {
            return Err(format!("policy {policy_id} is disabled"));
        }

        let cat = self
            .discover_loop_catalog()
            .map_err(|e| format!("could not load bundles: {e}"))?;
        let mut g = ResolvedGuard {
            policy: synthetic_policy(&policy_id, autonomy),
            loop_def: synthetic_pr_loop(),
            from_bundle: false,
        };
        if let Some(p) = find_policy(&cat.policies, &policy_id) {
            g.policy = p.clone();
            g.from_bundle = true;
        }
        if let Some(lp) = find_loop(&cat.loops, &loop_id) {
            if let Some(reason) = loop_roles_ok(&cat, lp, &settings) {
                return Err(reason);
            }
            g.loop_def = lp.clone();
            g.from_bundle = true;
        } else if loop_id == STEP_LOOP_ID {
            g.loop_def = synthetic_step_loop(LoopPhase {
                roles: map(&[
                    ("implementer", "recur.roles.coder"),
                    ("reviewer", "recur.roles.reviewer"),
                ]),
                ..LoopPhase::default()
            });
        } else if loop_id != DEFAULT_LOOP_ID {
            return Err(format!("loop {loop_id} not found"));
        }
        if policy_id != DEFAULT_POLICY_ID
            && policy_id != "recur.policies.cautious"
            && policy_id != "recur.policies.aggressive"
            && find_policy(&cat.policies, &policy_id).is_none()
        {
            return Err(format!("policy {policy_id} not found"));
        }
        Ok(g)
    }

    /// A run's guard. A fan-out child walks its parent's converge phase as
    /// a two-phase loop; everything else resolves by id.
    pub fn guard_for_workspace(&self, ws: &Workspace) -> Result<ResolvedGuard, String> {
        if ws.parent_id.trim().is_empty() {
            return self.resolve_guard(&ws.loop_id, &ws.policy_id, &ws.autonomy);
        }
        let Some(parent) = self.store.workspace(&ws.parent_id) else {
            return self.resolve_guard(&ws.loop_id, &ws.policy_id, &ws.autonomy);
        };
        let mut g = self.resolve_guard(&parent.loop_id, &ws.policy_id, &ws.autonomy)?;
        let mut ph = g
            .loop_def
            .phase_by_id(ws.parent_phase_id.trim())
            .filter(|p| p.uses == VERB_CONVERGE)
            .cloned();
        if ph.is_none() {
            ph = g
                .loop_def
                .phases
                .iter()
                .find(|p| p.uses == VERB_CONVERGE && p.dispatch != "on_demand")
                .cloned();
        }
        let ph = ph.unwrap_or_else(|| LoopPhase {
            roles: map(&[
                ("implementer", "recur.roles.coder"),
                ("reviewer", "recur.roles.reviewer"),
            ]),
            ..LoopPhase::default()
        });
        g.loop_def = child_loop_def(&g.loop_def, &ph);
        Ok(g)
    }

    /// Open findings per severity ("" counts as medium).
    pub fn open_findings_counts(&self, workspace_id: &str) -> HashMap<String, i64> {
        let mut counts = HashMap::new();
        for f in self.store.findings(workspace_id) {
            if f.status != "open" {
                continue;
            }
            let mut lv = f.severity.as_str().trim().to_lowercase();
            if lv.is_empty() {
                lv = "medium".to_owned();
            }
            *counts.entry(lv).or_insert(0) += 1;
        }
        counts
    }

    /// Whether open findings fail the bar, with "N countable finding(s)".
    pub fn findings_bar_blocks(
        &self,
        workspace_id: &str,
        th: &FindingsThreshold,
    ) -> Option<String> {
        let counts = self.open_findings_counts(workspace_id);
        if !recur_bundles::findings_block(th, &counts) {
            return None;
        }
        let n = countable_findings(th, &counts);
        Some(plural(n, "countable finding", "countable findings"))
    }
}

fn loop_roles_ok(cat: &Catalog, lp: &LoopDef, settings: &Settings) -> Option<String> {
    let mut ids: Vec<&String> = lp.roles.values().collect();
    for ph in &lp.phases {
        ids.extend(ph.roles.values());
    }
    ids.sort();
    ids.dedup();
    for id in ids {
        if contains_id(&settings.disabled_bundle_roles, id) {
            return Some(format!("role {id} is disabled"));
        }
        if find_role(&cat.roles, id).is_none() {
            if id.starts_with("recur.roles.") {
                continue;
            }
            return Some(format!("role {id} not found"));
        }
    }
    None
}

/// The shipped policy for an id (autonomy picks one when empty).
pub fn synthetic_policy(id: &str, autonomy: &AutonomyLevel) -> Policy {
    let id = if id.is_empty() {
        recur_bundles::policy_from_autonomy(autonomy.as_str())
    } else {
        id
    };
    match id {
        "recur.policies.cautious" => Policy {
            id: id.to_owned(),
            name: "Cautious".into(),
            max_rounds: 2,
            on_max_rounds: "escalate".into(),
            findings_threshold: FindingsThreshold {
                all_zero: true,
                levels: BTreeMap::new(),
            },
            plan_approval: "always".into(),
            extend_rounds: 1,
            on_fail: "fail".into(),
            ..Policy::default()
        },
        "recur.policies.aggressive" => Policy {
            id: id.to_owned(),
            name: "Aggressive".into(),
            max_rounds: 6,
            on_max_rounds: "proceed".into(),
            findings_threshold: FindingsThreshold {
                all_zero: false,
                levels: levels(&[("critical", 0), ("high", 0)]),
            },
            plan_approval: "never".into(),
            extend_rounds: 3,
            on_fail: "fail".into(),
            ..Policy::default()
        },
        _ => Policy {
            id: DEFAULT_POLICY_ID.to_owned(),
            name: "Balanced".into(),
            max_rounds: 4,
            on_max_rounds: "escalate".into(),
            findings_threshold: FindingsThreshold {
                all_zero: false,
                levels: levels(&[("critical", 0), ("high", 0), ("medium", 0)]),
            },
            plan_approval: "if_structured".into(),
            extend_rounds: 2,
            on_fail: "fail".into(),
            ..Policy::default()
        },
    }
}

/// The parent converge phase as a two-phase loop: converge (the parent's
/// roles, prompts, rounds, bar, cap, timeout, retries, on_fail, with
/// start/gate/dispatch cleared so the child implements first), then an
/// artifact phase so it never opens a PR.
pub fn child_loop_def(parent: &LoopDef, ph: &LoopPhase) -> LoopDef {
    let mut conv = ph.clone();
    conv.id = "converge".into();
    conv.uses = VERB_CONVERGE.into();
    conv.next = "done".into();
    conv.dispatch.clear();
    conv.when.clear();
    conv.gate.clear();
    conv.start.clear();
    conv.on_reject.clear();
    conv.optional = false;
    let mut roles = parent.roles.clone();
    if roles.get("conductor").is_none_or(|c| c.is_empty()) {
        roles.insert("conductor".into(), "recur.roles.maestro".into());
    }
    LoopDef {
        id: STEP_LOOP_ID.into(),
        name: format!("{} step", parent.name.trim()),
        entry: "converge".into(),
        reenter: vec!["converge".into()],
        roles,
        prompts: map(&[(
            "mission",
            "You conduct one child workspace that owns a slice of a parent plan. \
It implements and is reviewed under the parent's policy; the parent merges this branch back and \
reviews the aggregate diff. Answer where the slice stands; do not widen scope beyond its steps.",
        )]),
        phases: vec![
            conv,
            LoopPhase {
                id: "done".into(),
                uses: VERB_ARTIFACT.into(),
                ..LoopPhase::default()
            },
        ],
        bundle_name: parent.bundle_name.clone(),
        bundle_id: parent.bundle_id.clone(),
        source_root: parent.source_root.clone(),
        bundle_dir: parent.bundle_dir.clone(),
        ..LoopDef::default()
    }
}

/// The child loop when the parent cannot be consulted: coder + reviewer
/// converge, then artifact.
pub fn synthetic_step_loop(ph: LoopPhase) -> LoopDef {
    child_loop_def(
        &LoopDef {
            name: "Plan".into(),
            ..LoopDef::default()
        },
        &ph,
    )
}

/// The shipped To a PR loop: plan (gated by plan approval) → converge → PR.
pub fn synthetic_pr_loop() -> LoopDef {
    LoopDef {
        id: DEFAULT_LOOP_ID.into(),
        name: "To a PR".into(),
        entry: "planning".into(),
        reenter: vec!["planning".into(), "converge".into()],
        roles: map(&[("conductor", "recur.roles.maestro")]),
        phases: vec![
            LoopPhase {
                id: "planning".into(),
                uses: VERB_PLAN.into(),
                gate: "plan_approval".into(),
                next: "converge".into(),
                roles: map(&[("planner", "recur.roles.planner")]),
                ..LoopPhase::default()
            },
            LoopPhase {
                id: "converge".into(),
                uses: VERB_CONVERGE.into(),
                next: "pr".into(),
                roles: map(&[
                    ("implementer", "recur.roles.coder"),
                    ("reviewer", "recur.roles.reviewer"),
                ]),
                ..LoopPhase::default()
            },
            LoopPhase {
                id: "pr".into(),
                uses: VERB_ENSURE_PR.into(),
                ..LoopPhase::default()
            },
        ],
        ..LoopDef::default()
    }
}

/// How many open findings the bar counts: all of them for an all-zero bar,
/// else only the named severities.
pub fn countable_findings(th: &FindingsThreshold, counts: &HashMap<String, i64>) -> i64 {
    if th.all_zero {
        return counts.values().sum();
    }
    th.levels
        .keys()
        .map(|lv| counts.get(lv).copied().unwrap_or(0))
        .sum()
}

impl ResolvedGuard {
    /// The phase's findings bar when it is at least as strict as the
    /// policy's; otherwise the policy's.
    pub fn phase_threshold(&self, ph: &LoopPhase) -> FindingsThreshold {
        let pol = &self.policy.findings_threshold;
        match &ph.findings {
            Some(f) if recur_bundles::same_or_stricter(f, pol).is_ok() => f.clone(),
            _ => pol.clone(),
        }
    }

    pub fn phase_max_rounds(&self, ph: &LoopPhase) -> i64 {
        if let Some(n) = ph.max_rounds
            && n > 0
        {
            return n;
        }
        if self.policy.max_rounds > 0 {
            return self.policy.max_rounds;
        }
        4
    }

    pub fn phase_on_max_rounds(&self, ph: &LoopPhase) -> String {
        if !ph.on_max_rounds.is_empty() {
            return ph.on_max_rounds.clone();
        }
        if !self.policy.on_max_rounds.is_empty() {
            return self.policy.on_max_rounds.clone();
        }
        "escalate".into()
    }
}

/// Whether the plan-approval gate is skipped: an approved plan, a "never"
/// policy, or (if_structured) a plan with no steps.
pub fn skip_plan_approval(pol: &Policy, ws: &Workspace) -> bool {
    if ws.plan.as_ref().is_some_and(|p| p.approved) {
        return true;
    }
    match pol.plan_approval.as_str() {
        "never" => true,
        "always" => false,
        _ => ws.plan.as_ref().is_none_or(|p| p.steps.is_empty()),
    }
}

#[cfg(test)]
mod tests;
