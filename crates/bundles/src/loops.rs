//! `[[role]]`, `[[policy]]` and `[[loop]]` with `[[loop.phase]]`: the loop
//! model the engine runs (closed phase verbs, role slots, graph lint).

use std::collections::{BTreeMap, HashMap, HashSet};

use recur_domain::path;
use serde::{Deserialize, Serialize};

use crate::manifest::{BundleMeta, valid_bundle_id};
use crate::{Error, err};

/// Shipped first-party loop ids.
pub const DEFAULT_LOOP_ID: &str = "recur.loops.pr";
pub const CONVERGE_LOOP_ID: &str = "recur.loops.converge";
pub const CONVERGE_PR_LOOP_ID: &str = "recur.loops.converge-pr";
pub const DOCS_LOOP_ID: &str = "recur.loops.docs";
pub const SECURITY_LOOP_ID: &str = "recur.loops.security";
/// The engine-synthesized child loop for fan-out: never in a bundle, never
/// in the pickers.
pub const STEP_LOOP_ID: &str = "recur.loops.step";
pub const DEFAULT_POLICY_ID: &str = "recur.policies.balanced";
pub const WALK_STEP_CAP: usize = 32;

/// Chat / role modes a `[[role]]` may declare.
pub const ROLE_MODE_ASK: &str = "ask";
pub const ROLE_MODE_PLAN: &str = "plan";
pub const ROLE_MODE_AGENT: &str = "agent";
pub const ROLE_MODE_REVIEW: &str = "review";
pub const ROLE_MODE_LOOP: &str = "loop";

/// Closed phase verbs.
pub const VERB_PLAN: &str = "plan";
pub const VERB_IMPLEMENT: &str = "implement";
pub const VERB_REVIEW: &str = "review";
pub const VERB_CONVERGE: &str = "converge";
pub const VERB_VERIFY: &str = "verify";
pub const VERB_ENSURE_PR: &str = "ensure_pr";
pub const VERB_ARTIFACT: &str = "artifact";

/// The engine severity taxonomy, lowest to highest.
pub const SEVERITY_LEVELS: [&str; 5] = ["info", "low", "medium", "high", "critical"];

/// Reserved phase ids (cannot be author ids).
fn reserved_phase_id(s: &str) -> bool {
    matches!(s, "fail" | "escalate" | "skip" | "next")
}

/// One `[[role]]` entry after parse + prompt-path resolve.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Role {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub hint: String,
    pub code: String,
    pub mode: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    #[serde(
        rename(deserialize = "disabled_tools", serialize = "disabledTools"),
        skip_serializing_if = "Vec::is_empty"
    )]
    pub disabled_tools: Vec<String>,
    #[serde(
        rename(deserialize = "model_slot", serialize = "modelSlot"),
        skip_serializing_if = "String::is_empty"
    )]
    pub model_slot: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub prompt: String,

    #[serde(skip_deserializing, rename = "bundleName")]
    pub bundle_name: String,
    #[serde(skip_deserializing, rename = "bundleID")]
    pub bundle_id: String,
    #[serde(skip_deserializing, rename = "sourceRoot")]
    pub source_root: String,
    #[serde(skip_deserializing, rename = "bundleDir")]
    pub bundle_dir: String,
}

/// A per-severity cap map, or integer 0 (all five severities at 0).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct FindingsThreshold {
    #[serde(rename = "allZero", skip_serializing_if = "crate::manifest::is_false")]
    pub all_zero: bool,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub levels: BTreeMap<String, i64>,
}

/// One `[[policy]]` guard profile.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Policy {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub hint: String,
    #[serde(rename(deserialize = "max_rounds", serialize = "maxRounds"))]
    pub max_rounds: i64,
    #[serde(rename(deserialize = "on_max_rounds", serialize = "onMaxRounds"))]
    pub on_max_rounds: String,
    #[serde(skip_deserializing, rename = "findingsThreshold")]
    pub findings_threshold: FindingsThreshold,
    #[serde(rename(deserialize = "plan_approval", serialize = "planApproval"))]
    pub plan_approval: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub timeout: String,
    pub retries: i64,
    #[serde(rename(deserialize = "on_fail", serialize = "onFail"))]
    pub on_fail: String,
    #[serde(rename(deserialize = "extend_rounds", serialize = "extendRounds"))]
    pub extend_rounds: i64,
    #[serde(rename = "findings_threshold", skip_serializing)]
    pub findings_raw: Option<toml::Value>,

    #[serde(skip_deserializing, rename = "bundleName")]
    pub bundle_name: String,
    #[serde(skip_deserializing, rename = "bundleID")]
    pub bundle_id: String,
    #[serde(skip_deserializing, rename = "sourceRoot")]
    pub source_root: String,
    #[serde(skip_deserializing, rename = "bundleDir")]
    pub bundle_dir: String,
}

/// One `[[loop]]` after scalar normalize + graph lint.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LoopDef {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub hint: String,
    pub entry: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reenter: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub roles: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub prompts: BTreeMap<String, String>,
    #[serde(
        rename(deserialize = "phase", serialize = "phases"),
        skip_serializing_if = "Vec::is_empty"
    )]
    pub phases: Vec<LoopPhase>,
    /// Scalar sugar for `roles.conductor` (normalized away).
    #[serde(skip_serializing)]
    pub conductor: String,
    /// Scalar sugar for `prompts.mission` (normalized away).
    #[serde(skip_serializing)]
    pub prompt: String,

    #[serde(skip_deserializing, rename = "bundleName")]
    pub bundle_name: String,
    #[serde(skip_deserializing, rename = "bundleID")]
    pub bundle_id: String,
    #[serde(skip_deserializing, rename = "sourceRoot")]
    pub source_root: String,
    #[serde(skip_deserializing, rename = "bundleDir")]
    pub bundle_dir: String,
}

/// One `[[loop.phase]]`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LoopPhase {
    pub id: String,
    pub uses: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub dispatch: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub when: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub gate: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub next: String,
    #[serde(
        rename(deserialize = "on_reject", serialize = "onReject"),
        skip_serializing_if = "String::is_empty"
    )]
    pub on_reject: String,
    #[serde(
        rename(deserialize = "on_fail", serialize = "onFail"),
        skip_serializing_if = "String::is_empty"
    )]
    pub on_fail: String,
    #[serde(
        rename(deserialize = "on_max_rounds", serialize = "onMaxRounds"),
        skip_serializing_if = "String::is_empty"
    )]
    pub on_max_rounds: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub start: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub timeout: String,
    #[serde(
        rename(deserialize = "max_rounds", serialize = "maxRounds"),
        skip_serializing_if = "Option::is_none"
    )]
    pub max_rounds: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retries: Option<i64>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub roles: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub prompts: BTreeMap<String, String>,
    #[serde(skip_serializing)]
    pub role: String,
    #[serde(skip_serializing)]
    pub prompt: String,
    #[serde(rename = "findings_threshold", skip_serializing)]
    pub findings_raw: Option<toml::Value>,
    #[serde(
        skip_deserializing,
        rename = "findingsThreshold",
        skip_serializing_if = "Option::is_none"
    )]
    pub findings: Option<FindingsThreshold>,
    #[serde(skip_serializing)]
    pub optional: bool,
}

/// Role and prompt slots a phase verb accepts.
struct PhaseSlotSpec {
    role_slots: &'static [&'static str],
    prompt_slots: &'static [&'static str],
    required_roles: &'static [&'static str],
}

fn verb_slots(uses: &str) -> Option<PhaseSlotSpec> {
    let spec = |role_slots, prompt_slots| PhaseSlotSpec {
        role_slots,
        prompt_slots,
        required_roles: role_slots,
    };
    Some(match uses {
        VERB_PLAN => spec(&["planner"], &["plan"]),
        VERB_IMPLEMENT => spec(&["implementer"], &["implement"]),
        VERB_REVIEW => spec(&["reviewer"], &["review"]),
        VERB_CONVERGE => spec(
            &["implementer", "reviewer"],
            &["implement", "review", "fix", "rereview"],
        ),
        VERB_VERIFY => spec(&["verifier"], &["verify"]),
        VERB_ENSURE_PR | VERB_ARTIFACT => spec(&[], &[]),
        _ => return None,
    })
}

/// The modes a role slot may fill for `uses` (empty when unconstrained).
pub fn allowed_role_modes(uses: &str, slot: &str) -> &'static [&'static str] {
    match (uses, slot) {
        (VERB_PLAN, "planner") => &[ROLE_MODE_PLAN],
        (VERB_IMPLEMENT | VERB_CONVERGE, "implementer") => &[ROLE_MODE_AGENT],
        (VERB_REVIEW | VERB_CONVERGE, "reviewer") => &[ROLE_MODE_REVIEW],
        (VERB_VERIFY, "verifier") => &[ROLE_MODE_ASK, ROLE_MODE_REVIEW, ROLE_MODE_AGENT],
        _ => &[],
    }
}

pub(crate) fn parse_roles(
    bundle_dir: &str,
    meta: &BundleMeta,
    raw: Vec<Role>,
) -> Result<Vec<Role>, Error> {
    let mut out = Vec::with_capacity(raw.len());
    let mut seen_code: HashMap<String, String> = HashMap::new();
    for (i, mut r) in raw.into_iter().enumerate() {
        r.id = r.id.trim().to_string();
        if r.id.is_empty() {
            return Err(err!("role[{i}]: id is required"));
        }
        if !valid_bundle_id(&r.id) {
            return Err(err!("role[{i}]: id is not a safe id"));
        }
        let qid = format!("{:?}", r.id);
        if r.name.trim().is_empty() {
            return Err(err!("role {qid}: name is required"));
        }
        r.mode = crate::manifest::normalize_mode(&r.mode);
        if r.mode.is_empty() {
            return Err(err!(
                "role {qid}: mode is required (ask|plan|agent|review|autopilot)"
            ));
        }
        if !valid_role_mode(&r.mode) {
            return Err(err!(
                "role {qid}: unknown mode {:?} (use ask|plan|agent|review|autopilot)",
                r.mode
            ));
        }
        r.code = r.code.trim().to_uppercase();
        if r.code.len() != 2 || !r.code.bytes().all(|c| c.is_ascii_uppercase()) {
            return Err(err!("role {qid}: code must be 2 letters"));
        }
        if let Some(other) = seen_code.get(&r.code) {
            return Err(err!(
                "role {qid}: code {:?} collides with {:?}",
                r.code,
                other
            ));
        }
        seen_code.insert(r.code.clone(), r.id.clone());
        if r.model_slot.trim().is_empty() {
            r.model_slot = r.mode.clone();
        }
        resolve_prompt_field(bundle_dir, &r.id, "role", &mut r.prompt)?;
        r.bundle_name = meta.name.clone();
        r.bundle_id = meta.id.clone();
        r.bundle_dir = bundle_dir.to_string();
        out.push(r);
    }
    Ok(out)
}

pub(crate) fn parse_policies(
    bundle_dir: &str,
    meta: &BundleMeta,
    raw: Vec<Policy>,
) -> Result<Vec<Policy>, Error> {
    let mut out = Vec::with_capacity(raw.len());
    for (i, mut p) in raw.into_iter().enumerate() {
        p.id = p.id.trim().to_string();
        if p.id.is_empty() {
            return Err(err!("policy[{i}]: id is required"));
        }
        if !valid_bundle_id(&p.id) {
            return Err(err!("policy[{i}]: id is not a safe id"));
        }
        let qid = format!("{:?}", p.id);
        if p.name.trim().is_empty() {
            return Err(err!("policy {qid}: name is required"));
        }
        if p.max_rounds == 0 {
            p.max_rounds = 4;
        }
        if p.max_rounds < 1 {
            return Err(err!("policy {qid}: max_rounds must be >= 1"));
        }
        if p.on_max_rounds.is_empty() {
            p.on_max_rounds = "escalate".to_string();
        }
        if !valid_on_max_rounds(&p.on_max_rounds) {
            return Err(err!(
                "policy {qid}: on_max_rounds must be fail|escalate|proceed"
            ));
        }
        if p.plan_approval.is_empty() {
            p.plan_approval = "if_structured".to_string();
        }
        if !valid_plan_approval(&p.plan_approval) {
            return Err(err!(
                "policy {qid}: plan_approval must be always|if_structured|never"
            ));
        }
        if p.on_fail.is_empty() {
            p.on_fail = "fail".to_string();
        }
        if !valid_on_fail(&p.on_fail) {
            return Err(err!("policy {qid}: on_fail must be fail|escalate|skip"));
        }
        if p.extend_rounds < 0 {
            return Err(err!("policy {qid}: extend_rounds must be >= 0"));
        }
        p.findings_threshold = parse_findings_threshold(p.findings_raw.take().as_ref(), false)
            .map_err(|e| err!("policy {qid}: {e}"))?;
        p.bundle_name = meta.name.clone();
        p.bundle_id = meta.id.clone();
        p.bundle_dir = bundle_dir.to_string();
        out.push(p);
    }
    Ok(out)
}

pub(crate) fn parse_loops(
    bundle_dir: &str,
    meta: &BundleMeta,
    raw: Vec<LoopDef>,
) -> Result<Vec<LoopDef>, Error> {
    let mut out = Vec::with_capacity(raw.len());
    for (i, mut lp) in raw.into_iter().enumerate() {
        lp.id = lp.id.trim().to_string();
        if lp.id.is_empty() {
            return Err(err!("workflow[{i}]: id is required"));
        }
        if !valid_bundle_id(&lp.id) {
            return Err(err!("workflow[{i}]: id is not a safe id"));
        }
        let qid = format!("{:?}", lp.id);
        if lp.name.trim().is_empty() {
            return Err(err!("workflow {qid}: name is required"));
        }
        normalize_loop_slots(&mut lp).map_err(|e| err!("workflow {qid}: {e}"))?;
        let id = lp.id.clone();
        for (slot, val) in lp.prompts.iter_mut() {
            resolve_prompt_field(bundle_dir, &id, &format!("prompts.{slot}"), val)
                .map_err(|e| err!("workflow {qid}: {e}"))?;
        }
        if lp.entry.trim().is_empty() {
            return Err(err!("workflow {qid}: entry is required"));
        }
        let mut seen_phase = HashSet::new();
        for (j, ph) in lp.phases.iter_mut().enumerate() {
            normalize_phase(ph, bundle_dir).map_err(|e| err!("workflow {qid} phase[{j}]: {e}"))?;
            if !seen_phase.insert(ph.id.clone()) {
                return Err(err!("workflow {qid}: duplicate phase id {:?}", ph.id));
            }
        }
        lint_loop_graph(&lp).map_err(|e| err!("workflow {qid}: {e}"))?;
        lp.bundle_name = meta.name.clone();
        lp.bundle_id = meta.id.clone();
        lp.bundle_dir = bundle_dir.to_string();
        out.push(lp);
    }
    Ok(out)
}

fn normalize_loop_slots(lp: &mut LoopDef) -> Result<(), String> {
    if !lp.conductor.is_empty() {
        if lp
            .roles
            .get("conductor")
            .is_some_and(|c| !c.is_empty() && *c != lp.conductor)
        {
            return Err("conductor and roles.conductor both set".to_string());
        }
        lp.roles
            .insert("conductor".to_string(), lp.conductor.trim().to_string());
        lp.conductor.clear();
    }
    if !lp.prompt.is_empty() {
        if lp
            .prompts
            .get("mission")
            .is_some_and(|m| !m.is_empty() && *m != lp.prompt)
        {
            return Err("prompt and prompts.mission both set".to_string());
        }
        lp.prompts
            .insert("mission".to_string(), std::mem::take(&mut lp.prompt));
    }
    if lp
        .roles
        .get("conductor")
        .is_none_or(|c| c.trim().is_empty())
    {
        return Err("roles.conductor is required".to_string());
    }
    Ok(())
}

fn normalize_phase(ph: &mut LoopPhase, bundle_dir: &str) -> Result<(), Error> {
    ph.id = ph.id.trim().to_string();
    if ph.id.is_empty() {
        return Err(err!("id is required"));
    }
    if reserved_phase_id(&ph.id) {
        return Err(err!("phase id {:?} is reserved", ph.id));
    }
    ph.uses = ph.uses.trim().to_string();
    let uses = format!("{:?}", ph.uses);
    let Some(spec) = verb_slots(&ph.uses) else {
        return Err(err!("unknown uses {uses}"));
    };
    if ph.optional {
        return Err(err!(
            "optional is reserved; omit it or use dispatch = \"on_demand\""
        ));
    }
    if ph.dispatch.is_empty() {
        ph.dispatch = "graph".to_string();
    }
    if ph.dispatch != "graph" && ph.dispatch != "on_demand" {
        return Err(err!("dispatch must be graph|on_demand"));
    }
    if !ph.role.is_empty() {
        if spec.role_slots.len() != 1 {
            return Err(err!(
                "scalar role = is only legal when uses {uses} has exactly one role slot"
            ));
        }
        let slot = spec.role_slots[0];
        if ph
            .roles
            .get(slot)
            .is_some_and(|r| !r.is_empty() && *r != ph.role)
        {
            return Err(err!("role and roles.{slot} both set"));
        }
        ph.roles
            .insert(slot.to_string(), ph.role.trim().to_string());
        ph.role.clear();
    }
    if !ph.prompt.is_empty() {
        if spec.prompt_slots.len() != 1 {
            return Err(err!(
                "scalar prompt = is only legal when uses {uses} has exactly one prompt slot (use prompts)"
            ));
        }
        let slot = spec.prompt_slots[0];
        if ph
            .prompts
            .get(slot)
            .is_some_and(|p| !p.is_empty() && *p != ph.prompt)
        {
            return Err(err!("prompt and prompts.{slot} both set"));
        }
        ph.prompts
            .insert(slot.to_string(), std::mem::take(&mut ph.prompt));
    }
    for k in ph.roles.keys() {
        if k.is_empty() {
            return Err(err!("empty role slot for uses {uses}"));
        }
        if !spec.role_slots.contains(&k.as_str()) {
            return Err(err!("unknown role slot {:?} for uses {uses}", k));
        }
    }
    for k in ph.prompts.keys() {
        if !spec.prompt_slots.contains(&k.as_str()) {
            return Err(err!("unknown prompt slot {:?} for uses {uses}", k));
        }
    }
    for slot in spec.required_roles {
        if ph.roles.get(*slot).is_none_or(|r| r.trim().is_empty()) {
            return Err(err!("roles.{slot} is required for uses {uses}"));
        }
    }
    if !ph.start.is_empty() {
        if ph.uses != VERB_CONVERGE {
            return Err(err!("start is only legal on converge"));
        }
        if ph.start != "implement" && ph.start != "review" {
            return Err(err!("start must be implement|review"));
        }
    }
    if !ph.gate.is_empty() && ph.gate != "plan_approval" {
        return Err(err!("unknown gate {:?}", ph.gate));
    }
    if !ph.on_fail.is_empty() && !valid_on_fail(&ph.on_fail) {
        return Err(err!("on_fail must be fail|escalate|skip"));
    }
    if !ph.on_max_rounds.is_empty() && !valid_on_max_rounds(&ph.on_max_rounds) {
        return Err(err!("on_max_rounds must be fail|escalate|proceed"));
    }
    if ph.max_rounds.is_some_and(|n| n < 1) {
        return Err(err!("max_rounds must be >= 1"));
    }
    if let Some(raw) = ph.findings_raw.take() {
        ph.findings = Some(parse_findings_threshold(Some(&raw), true).map_err(Error)?);
    }
    let id = ph.id.clone();
    for (slot, val) in ph.prompts.iter_mut() {
        resolve_prompt_field(bundle_dir, &id, &format!("prompts.{slot}"), val)?;
    }
    Ok(())
}

fn lint_loop_graph(lp: &LoopDef) -> Result<(), String> {
    let by_id: HashMap<&str, &LoopPhase> = lp.phases.iter().map(|p| (p.id.as_str(), p)).collect();
    let entry = format!("{:?}", lp.entry);
    match by_id.get(lp.entry.as_str()) {
        None => return Err(format!("entry {entry} is not a phase")),
        Some(ph) if ph.dispatch == "on_demand" => {
            return Err(format!("entry {entry} is on-demand"));
        }
        _ => {}
    }
    for id in &lp.reenter {
        match by_id.get(id.as_str()) {
            None => return Err(format!("reenter {:?} is not a phase", id)),
            Some(ph) if ph.dispatch == "on_demand" => {
                return Err(format!("reenter cannot list on-demand id {:?}", id));
            }
            _ => {}
        }
    }
    // Inbound next onto an on-demand phase is illegal.
    let mut inbound: HashSet<&str> = HashSet::new();
    for ph in &lp.phases {
        for edge in [&ph.next, &ph.on_reject] {
            if edge.is_empty() {
                continue;
            }
            if reserved_phase_id(edge) {
                return Err(format!(
                    "phase {:?}: {:?} is reserved (not a phase id)",
                    ph.id, edge
                ));
            }
            if !by_id.contains_key(edge.as_str()) {
                return Err(format!("phase {:?} names missing id {:?}", ph.id, edge));
            }
            inbound.insert(edge);
        }
    }
    for ph in &lp.phases {
        if ph.dispatch == "on_demand" && inbound.contains(ph.id.as_str()) {
            return Err(format!("on-demand phase {:?} has inbound next", ph.id));
        }
    }
    // Success-next DAG.
    let mut adj: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for ph in &lp.phases {
        if ph.dispatch == "on_demand" || ph.next.is_empty() {
            continue;
        }
        adj.entry(&ph.id).or_default().push(&ph.next);
    }
    match graph_cycle(&adj) {
        Some(cycle) => Err(format!("next graph contains a cycle ({cycle})")),
        None => Ok(()),
    }
}

fn graph_cycle(adj: &BTreeMap<&str, Vec<&str>>) -> Option<String> {
    #[derive(Clone, Copy, PartialEq)]
    enum Color {
        Gray,
        Black,
    }
    fn walk<'a>(
        n: &'a str,
        adj: &BTreeMap<&'a str, Vec<&'a str>>,
        color: &mut HashMap<&'a str, Color>,
    ) -> Option<String> {
        color.insert(n, Color::Gray);
        for m in adj.get(n).into_iter().flatten() {
            match color.get(m) {
                Some(Color::Gray) => return Some(format!("{n} → {m}")),
                None => {
                    if let Some(c) = walk(m, adj, color) {
                        return Some(c);
                    }
                }
                Some(Color::Black) => {}
            }
        }
        color.insert(n, Color::Black);
        None
    }
    let mut color = HashMap::new();
    for n in adj.keys() {
        if !color.contains_key(n)
            && let Some(c) = walk(n, adj, &mut color)
        {
            return Some(c);
        }
    }
    None
}

fn parse_findings_threshold(
    raw: Option<&toml::Value>,
    phase: bool,
) -> Result<FindingsThreshold, String> {
    let Some(raw) = raw else {
        return Ok(FindingsThreshold::default());
    };
    match raw {
        toml::Value::Integer(v) => {
            if *v != 0 {
                if phase {
                    return Err(format!(
                        "phase findings_threshold integer must be 0 (got {v})"
                    ));
                }
                return Err("integer findings_threshold must be 0".to_string());
            }
            Ok(FindingsThreshold {
                all_zero: true,
                ..Default::default()
            })
        }
        toml::Value::Float(v) => {
            if *v != 0.0 {
                return Err("integer findings_threshold must be 0".to_string());
            }
            Ok(FindingsThreshold {
                all_zero: true,
                ..Default::default()
            })
        }
        toml::Value::Table(t) => {
            let mut levels = BTreeMap::new();
            for (k, val) in t {
                let k = k.trim().to_lowercase();
                if !SEVERITY_LEVELS.contains(&k.as_str()) {
                    return Err(format!("unknown severity {:?}", k));
                }
                let n = any_to_int(val).map_err(|e| format!("findings_threshold.{k}: {e}"))?;
                if n < 0 {
                    return Err(format!("findings_threshold.{k} must be >= 0"));
                }
                levels.insert(k, n);
            }
            Ok(FindingsThreshold {
                all_zero: false,
                levels,
            })
        }
        _ => Err("findings_threshold must be a map or 0".to_string()),
    }
}

fn any_to_int(v: &toml::Value) -> Result<i64, String> {
    match v {
        toml::Value::Integer(n) => Ok(*n),
        toml::Value::Float(f) => {
            if f.fract() != 0.0 || !f.is_finite() {
                return Err("not an integer".to_string());
            }
            Ok(*f as i64)
        }
        toml::Value::String(s) => s.parse::<i64>().map_err(|e| e.to_string()),
        _ => Err("not an integer".to_string()),
    }
}

/// Whether a prompt field value should be resolved as a bundle-relative file.
pub fn looks_like_prompt_path(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() || s.starts_with('@') || s.starts_with('/') {
        return false;
    }
    if s.contains([' ', '\t', '\n']) {
        return false;
    }
    s.contains('/') || s.to_lowercase().ends_with(".md")
}

fn resolve_prompt_field(
    bundle_dir: &str,
    owner: &str,
    field: &str,
    slot: &mut String,
) -> Result<(), Error> {
    let s = slot.trim().to_string();
    if s.is_empty() || !looks_like_prompt_path(&s) {
        *slot = s;
        return Ok(());
    }
    let escapes = || err!("{owner} {field}: path {:?} escapes the bundle", s);
    if path::is_abs(&s) || s.split('/').any(|part| part == "..") {
        return Err(escapes());
    }
    let p = path::join(&[bundle_dir, &s]);
    match path::rel(bundle_dir, &p) {
        Some(rel) if !rel.starts_with("..") => {}
        _ => return Err(escapes()),
    }
    let body = std::fs::read(&p).map_err(|_| err!("{owner} {field}: missing path {:?}", s))?;
    *slot = String::from_utf8_lossy(&body).into_owned();
    Ok(())
}

fn valid_role_mode(s: &str) -> bool {
    matches!(
        s,
        ROLE_MODE_ASK | ROLE_MODE_PLAN | ROLE_MODE_AGENT | ROLE_MODE_REVIEW | ROLE_MODE_LOOP
    )
}

fn valid_on_max_rounds(s: &str) -> bool {
    matches!(s, "fail" | "escalate" | "proceed")
}

fn valid_on_fail(s: &str) -> bool {
    matches!(s, "fail" | "escalate" | "skip")
}

fn valid_plan_approval(s: &str) -> bool {
    matches!(s, "always" | "if_structured" | "never")
}

impl LoopDef {
    /// The phase with the given id.
    pub fn phase_by_id(&self, id: &str) -> Option<&LoopPhase> {
        self.phases.iter().find(|ph| ph.id == id)
    }

    /// Non-on-demand, non-terminal phases (the default reenter list).
    pub fn graph_phase_ids(&self) -> Vec<String> {
        self.phases
            .iter()
            .filter(|ph| {
                ph.dispatch != "on_demand" && ph.uses != VERB_ENSURE_PR && ph.uses != VERB_ARTIFACT
            })
            .map(|ph| ph.id.clone())
            .collect()
    }

    /// The author's reenter list, or every graph non-terminal when omitted.
    pub fn reenter_ids(&self) -> Vec<String> {
        if self.reenter.is_empty() {
            self.graph_phase_ids()
        } else {
            self.reenter.clone()
        }
    }
}

/// Whether open findings fail the threshold. Omitted map levels do not
/// participate; `all_zero` means every severity at 0.
pub fn findings_block(th: &FindingsThreshold, counts: &HashMap<String, i64>) -> bool {
    let count = |lv: &str| counts.get(lv).copied().unwrap_or(0);
    if th.all_zero {
        return SEVERITY_LEVELS.iter().any(|lv| count(lv) > 0);
    }
    th.levels.iter().any(|(lv, cap)| count(lv) > *cap)
}

/// Whether a phase threshold is the same as or stricter than the policy's.
pub fn same_or_stricter(
    phase: &FindingsThreshold,
    policy: &FindingsThreshold,
) -> Result<(), String> {
    if phase.all_zero {
        return Ok(());
    }
    if policy.all_zero {
        if phase.levels.values().any(|n| *n > 0) {
            return Err("phase findings_threshold is looser than policy (all-zero)".to_string());
        }
        return Ok(());
    }
    for (lv, p_cap) in &policy.levels {
        match phase.levels.get(lv) {
            None => return Err(format!("phase drops policy level {lv}")),
            Some(ph_cap) if ph_cap > p_cap => {
                return Err(format!("phase raises {lv} cap from {p_cap} to {ph_cap}"));
            }
            _ => {}
        }
    }
    Ok(())
}

/// The catalog role by id.
pub fn find_role<'a>(roles: &'a [Role], id: &str) -> Option<&'a Role> {
    roles.iter().find(|r| r.id == id)
}

/// The catalog loop by id.
pub fn find_loop<'a>(loops: &'a [LoopDef], id: &str) -> Option<&'a LoopDef> {
    loops.iter().find(|l| l.id == id)
}

/// The catalog policy by id.
pub fn find_policy<'a>(policies: &'a [Policy], id: &str) -> Option<&'a Policy> {
    policies.iter().find(|p| p.id == id)
}

/// Maps a stored autonomy level onto a shipped policy id.
pub fn policy_from_autonomy(a: &str) -> &'static str {
    match a.trim().to_lowercase().as_str() {
        "cautious" => "recur.policies.cautious",
        "aggressive" => "recur.policies.aggressive",
        _ => DEFAULT_POLICY_ID,
    }
}
