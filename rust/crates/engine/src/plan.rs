//! Structured plans (Go `plan.go`): Plan-mode chats and the loop planner
//! file plans through `submit_plan`; the engine assigns step ids, owns
//! statuses, and mirrors `Plan::text()` into DefinitionOfDone so every prose
//! consumer stays coherent. Also the Plan panel's human edits and approval,
//! and the Cursor createPlan prose harvest.

use std::path::Path;

use recur_agent::display::PLAN_MAX_PERSIST_CHARS;
use recur_agent_tools::{PlanStepMeta, PlanSubmission};
use recur_domain::{
    FEED_CARD_PLAN, FeedItem, Plan, PlanAttachment, PlanStep, STEP_DOING, STEP_DONE, STEP_PENDING,
    STEP_SKIPPED,
};
use serde::Serialize;

use crate::Engine;
use crate::artifacts::list_artifacts;
use crate::codes::{is_pair_chat_code, normalize_pair_chat_code};
use crate::publish::run_progress;
use crate::text::{html_escape, truncate};

const MAX_PLAN_ATTACHMENTS: usize = 20;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlanCardStep {
    id: String,
    title: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    files: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    depends_on: Vec<String>,
}

/// The cockpit plan card payload: the summary and the checklist (status is
/// read live from the workspace plan by step id).
#[derive(Serialize)]
struct PlanCardData {
    summary: String,
    steps: Option<Vec<PlanCardStep>>,
    #[serde(skip_serializing_if = "String::is_empty")]
    source: String,
}

/// Who filed a plan: a Plan-mode pair chat, the human ("AR"), or the loop
/// planner under whatever role code the bundle gave it.
pub fn plan_source(agent_code: &str) -> &'static str {
    if is_pair_chat_code(agent_code) {
        "plan-chat"
    } else if agent_code == "AR" {
        "human"
    } else {
        "loop-planner"
    }
}

/// The pair-chat tab that filed the plan; human and Maestro filings keep
/// the previous tab so Run as loop still inherits its Simplify chip.
pub fn plan_author_chat_code(agent_code: &str, prev: Option<&Plan>) -> String {
    if is_pair_chat_code(agent_code) {
        return normalize_pair_chat_code(agent_code);
    }
    prev.map(|p| p.chat_code.trim().to_owned())
        .unwrap_or_default()
}

fn plural_s(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Resolves artifact ids and persisted chat-image ids; unknown ones come
/// back separately so the tool result can name them.
pub fn resolve_plan_attachments(
    data_dir: &Path,
    workspace_id: &str,
    ids: &[String],
) -> (Vec<PlanAttachment>, Vec<String>) {
    if ids.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let arts = list_artifacts(data_dir, workspace_id).unwrap_or_default();
    let mut resolved: Vec<PlanAttachment> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for raw in ids {
        let id = raw.trim();
        if id.is_empty() || seen.contains(&id) {
            continue;
        }
        seen.push(id);
        if resolved.len() + unknown.len() >= MAX_PLAN_ATTACHMENTS {
            break;
        }
        if let Some(a) = arts.iter().find(|a| a.id == id) {
            resolved.push(PlanAttachment {
                id: a.id.clone(),
                kind: a.kind.as_str().to_owned(),
                title: a.title.clone(),
                path: a.path.clone(),
            });
            continue;
        }
        if let Some(att) = lookup_chat_plan_attachment(data_dir, workspace_id, id) {
            resolved.push(att);
            continue;
        }
        unknown.push(id.to_owned());
    }
    (resolved, unknown)
}

fn lookup_chat_plan_attachment(
    data_dir: &Path,
    workspace_id: &str,
    id: &str,
) -> Option<PlanAttachment> {
    let id = recur_agent::attach::sanitize_attachment_id(id);
    if id.is_empty() {
        return None;
    }
    let dir = data_dir
        .join("workspaces")
        .join(workspace_id)
        .join("attachments");
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| !t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let prefix = format!("{id}.");
    let name = names
        .into_iter()
        .find(|n| *n == id || n.starts_with(&prefix))?;
    let ext = recur_bundles::go::ext(&name).trim_start_matches('.');
    let kind = match ext.to_lowercase().as_str() {
        "jpg" => "jpeg".to_owned(),
        "" => "image".to_owned(),
        k => k.to_owned(),
    };
    Some(PlanAttachment {
        id,
        kind,
        title: name.clone(),
        path: format!("workspaces/{workspace_id}/attachments/{name}"),
    })
}

/// The headline of a prose plan: the first non-empty line with leading
/// heading or list markers stripped.
pub fn first_paragraph(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        return String::new();
    }
    for line in s.split('\n') {
        let line = strip_leading_markdown(line.trim());
        if line.is_empty() {
            continue;
        }
        return truncate(&line, 400);
    }
    truncate(s, 400)
}

fn strip_leading_markdown(s: &str) -> String {
    let s = s.trim().trim_start_matches('#').trim();
    let b = s.as_bytes();
    if b.len() >= 2 && (b[0] == b'*' || b[0] == b'-') && b[1] == b' ' {
        return s[2..].trim().to_owned();
    }
    s.to_owned()
}

/// Carries status and note across a re-file: a previous step matches by id
/// (Plan panel retitle), then by exact title (agent re-file / reorder). Ids
/// are always S1…Sn in list order; retained dependencies follow their
/// target's new id and edges to dropped steps go with them.
pub fn merge_plan_steps(
    prev: &[PlanStep],
    titles: &[String],
    ids: &[String],
    meta: &[PlanStepMeta],
) -> Vec<PlanStep> {
    if titles.is_empty() {
        return Vec::new();
    }
    let mut used = vec![false; prev.len()];
    let mut src: Vec<Option<usize>> = vec![None; titles.len()];
    for (i, raw) in titles.iter().enumerate() {
        let t = raw.trim();
        if let Some(id) = ids.get(i).map(|s| s.trim())
            && let Some(j) = prev.iter().rposition(|s| !s.id.is_empty() && s.id == id)
            && !used[j]
        {
            src[i] = Some(j);
        }
        if src[i].is_none() {
            src[i] = prev
                .iter()
                .enumerate()
                .find(|(j, s)| s.title == t && !used[*j])
                .map(|(j, _)| j);
        }
        if let Some(j) = src[i] {
            used[j] = true;
        }
    }
    let renumber: Vec<(String, String)> = src
        .iter()
        .enumerate()
        .filter_map(|(i, j)| j.map(|j| (prev[j].id.clone(), format!("S{}", i + 1))))
        .collect();
    let mut out = Vec::with_capacity(titles.len());
    for (i, raw) in titles.iter().enumerate() {
        let t = raw.trim().to_owned();
        let new_id = format!("S{}", i + 1);
        let mut step = match src[i] {
            Some(j) => {
                let mut s = prev[j].clone();
                s.id = new_id.clone();
                s.title = t;
                s.depends_on = s
                    .depends_on
                    .iter()
                    .filter_map(|d| {
                        renumber
                            .iter()
                            .rev()
                            .find(|(old, _)| old == d.trim())
                            .map(|(_, nd)| nd.clone())
                    })
                    .filter(|nd| *nd != new_id)
                    .collect();
                s
            }
            None => PlanStep {
                id: new_id,
                title: t,
                status: STEP_PENDING.into(),
                ..PlanStep::default()
            },
        };
        // The object form is authoritative for its step, including empty
        // lists; a bare title keeps what was recorded.
        if let Some(m) = meta.get(i)
            && (m.explicit || !m.files.is_empty() || !m.depends_on.is_empty())
        {
            step.files = m.files.clone();
            step.depends_on = m.depends_on.clone();
        }
        out.push(step);
    }
    out
}

/// Shrinks Body (or Summary) so `Plan::text()` fits the persist cap: the
/// checklist is what Coders execute from and is never what gets cut.
pub fn cap_plan_for_persist(p: &mut Plan) {
    for _ in 0..8 {
        let len = p.text().len();
        if len <= PLAN_MAX_PERSIST_CHARS {
            return;
        }
        let trim = len - PLAN_MAX_PERSIST_CHARS + 8;
        if p.body.len() > 256 {
            let n = p.body.len().saturating_sub(trim).max(256);
            p.body = truncate(p.body.trim(), n);
            continue;
        }
        if p.summary.len() > 64 {
            let n = p.summary.len().saturating_sub(trim).max(64);
            p.summary = truncate(p.summary.trim(), n);
            continue;
        }
        return;
    }
}

impl Engine {
    /// Host `submit_plan`: persists the filing as the workspace plan and
    /// publishes its card.
    pub fn apply_submitted_plan(
        &self,
        workspace_id: &str,
        agent_code: &str,
        sub: PlanSubmission,
    ) -> String {
        let Some(ws) = self.store.workspace(workspace_id) else {
            return "Refused: that workspace no longer exists.".into();
        };
        let source = plan_source(agent_code);
        let prev = ws
            .plan
            .as_ref()
            .map(|p| p.steps.as_slice())
            .unwrap_or_default();
        let steps = merge_plan_steps(prev, &sub.steps, &sub.step_ids, &sub.step_meta);
        let (atts, unknown) =
            resolve_plan_attachments(&self.data_dir, workspace_id, &sub.attachment_ids);
        let mut plan = Plan {
            summary: sub.summary.trim().to_owned(),
            body: sub.body.trim().to_owned(),
            steps,
            attachments: atts,
            source: source.to_owned(),
            chat_code: plan_author_chat_code(agent_code, ws.plan.as_ref()),
            ..Plan::default()
        };
        cap_plan_for_persist(&mut plan);
        self.set_workspace_plan(workspace_id, &plan);
        let author = if source == "human" { "You" } else { "Agent" };
        self.emit_plan_card(workspace_id, agent_code, author, &plan);
        self.touch_workspace_activity(workspace_id, "Filed a structured plan", self.now());
        let mut msg = format!("Plan recorded ({} steps).", plan.steps.len());
        let n = plan.attachments.len();
        if n > 0 {
            msg = format!(
                "Plan recorded ({} steps, {n} attachment{}).",
                plan.steps.len(),
                plural_s(n)
            );
        }
        if !unknown.is_empty() {
            msg.push_str(&format!(" Unknown attachment ids: {}.", unknown.join(", ")));
        }
        msg.push_str(" The human reviews and approves it in the Plan panel.");
        msg
    }

    /// One compact card per filing (a replan shows as its own row).
    fn emit_plan_card(&self, workspace_id: &str, agent_code: &str, author: &str, plan: &Plan) {
        let d = PlanCardData {
            summary: plan.summary.trim().to_owned(),
            steps: (!plan.steps.is_empty()).then(|| {
                plan.steps
                    .iter()
                    .map(|s| PlanCardStep {
                        id: s.id.clone(),
                        title: s.title.clone(),
                        files: s.files.clone(),
                        depends_on: s.depends_on.clone(),
                    })
                    .collect()
            }),
            source: plan.source.clone(),
        };
        let Ok(data) = recur_domain::json::to_string(&d) else {
            return;
        };
        let nanos = self.now().timestamp_nanos_opt().unwrap_or_default();
        self.emit_workspace_card(
            workspace_id,
            FeedItem {
                id: format!("plan-{workspace_id}-{nanos}"),
                agent_code: agent_code.to_owned(),
                name: author.to_owned(),
                text: format!(
                    "Filed a structured plan: {} step(s) — {}",
                    plan.steps.len(),
                    html_escape(&truncate(&plan.summary, 160))
                ),
                card: FEED_CARD_PLAN.into(),
                data,
                ..FeedItem::default()
            },
        );
    }

    /// Host `update_plan_step`: the engine owns the transition (statuses
    /// were vocabulary-checked at the MCP boundary).
    pub fn apply_plan_step_update(
        &self,
        workspace_id: &str,
        _agent_code: &str,
        step_id: &str,
        status: &str,
        note: &str,
    ) -> String {
        let Some(ws) = self.store.workspace(workspace_id) else {
            return "Refused: that workspace no longer exists.".into();
        };
        if ws.plan.as_ref().is_none_or(|p| p.steps.is_empty()) {
            return "Refused: this workspace has no structured plan steps.".into();
        }
        let mut title = None;
        let updated = self.store.update_workspace(workspace_id, |l| {
            let Some(plan) = l.plan.as_mut() else {
                return;
            };
            let Some(s) = plan.steps.iter_mut().find(|s| s.id == step_id) else {
                return;
            };
            s.status = status.to_owned();
            if !note.trim().is_empty() {
                s.note = truncate(note.trim(), 300);
            }
            title = Some(s.title.clone());
            l.definition_of_done = truncate(&plan.text(), PLAN_MAX_PERSIST_CHARS);
            if let Some(p) = run_progress(&l.state, l.plan.as_ref()) {
                l.progress = p;
            }
        });
        let Some(title) = title else {
            return format!(
                "Refused: no plan step with id {}.",
                recur_bundles::go::quote(step_id)
            );
        };
        if let Some(u) = updated {
            self.publish_loop_updated(&u.id, u.state.clone(), &u.phase, u.iteration);
        }
        self.touch_workspace_activity(
            workspace_id,
            &format!("Updated plan step {step_id}"),
            self.now(),
        );
        format!("Step {step_id} ({}) → {status}.", truncate(&title, 60))
    }

    /// Writes a plan and its prose mirror and publishes so the Plan panel
    /// refreshes.
    pub fn set_workspace_plan(&self, workspace_id: &str, plan: &Plan) {
        let Some(u) = self.store.update_workspace(workspace_id, |l| {
            let mut p = plan.clone();
            cap_plan_for_persist(&mut p);
            l.definition_of_done = truncate(&p.text(), PLAN_MAX_PERSIST_CHARS);
            l.plan = Some(p);
            if let Some(prog) = run_progress(&l.state, l.plan.as_ref()) {
                l.progress = prog;
            }
        }) else {
            return;
        };
        self.publish_loop_updated(&u.id, u.state.clone(), &u.phase, u.iteration);
    }

    /// Files a summary-only plan from free-form prose (Cursor createPlan,
    /// which Recur cannot disable). Never overwrites a plan that already has
    /// structured steps.
    pub fn adopt_prose_plan(&self, workspace_id: &str, agent_code: &str, body: &str) -> bool {
        let body = body.trim();
        if body.is_empty() {
            return false;
        }
        let Some(ws) = self.store.workspace(workspace_id) else {
            return false;
        };
        if ws.plan.as_ref().is_some_and(|p| !p.steps.is_empty()) {
            return false;
        }
        self.set_workspace_plan(
            workspace_id,
            &Plan {
                summary: first_paragraph(body),
                body: body.to_owned(),
                source: plan_source(agent_code).to_owned(),
                chat_code: plan_author_chat_code(agent_code, ws.plan.as_ref()),
                ..Plan::default()
            },
        );
        self.emit_workspace_event(
            workspace_id,
            agent_code,
            "Agent",
            "Captured plan from createPlan (prose only — no structured steps).",
        );
        true
    }

    /// The Plan panel's edit, on the human's behalf (source human). Empty
    /// steps are dropped; `step_ids` aligned with `steps` keep status across
    /// a retitle. A summary-only plan is allowed; attachments are kept.
    pub fn submit_human_plan(
        &self,
        workspace_id: &str,
        summary: &str,
        body: &str,
        steps: &[String],
        step_ids: &[String],
    ) -> String {
        let mut clean = Vec::with_capacity(steps.len());
        let mut ids = Vec::with_capacity(steps.len());
        for (i, s) in steps.iter().enumerate() {
            let t = s.trim();
            if t.is_empty() {
                continue;
            }
            clean.push(t.to_owned());
            ids.push(
                step_ids
                    .get(i)
                    .map(|s| s.trim().to_owned())
                    .unwrap_or_default(),
            );
        }
        if summary.trim().is_empty() {
            return "Refused: the plan needs a summary.".into();
        }
        let att_ids: Vec<String> = self
            .store
            .workspace(workspace_id)
            .and_then(|ws| ws.plan)
            .map(|p| {
                p.attachments
                    .iter()
                    .map(|a| a.id.trim().to_owned())
                    .filter(|id| !id.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        self.apply_submitted_plan(
            workspace_id,
            "AR",
            PlanSubmission {
                summary: summary.to_owned(),
                body: body.to_owned(),
                steps: clean,
                step_ids: ids,
                attachment_ids: att_ids,
                step_meta: Vec::new(),
            },
        )
    }

    /// A Plan panel checkbox: the same validated path as update_plan_step.
    pub fn update_human_plan_step(
        &self,
        workspace_id: &str,
        step_id: &str,
        status: &str,
    ) -> String {
        if ![STEP_PENDING, STEP_DOING, STEP_DONE, STEP_SKIPPED].contains(&status) {
            return format!(
                "Refused: unknown step status {}.",
                recur_bundles::go::quote(status)
            );
        }
        self.apply_plan_step_update(workspace_id, "AR", step_id, status, "")
    }

    /// The Plan panel's Approve.
    pub fn approve_plan(&self, workspace_id: &str) -> String {
        let Some(ws) = self.store.workspace(workspace_id) else {
            return "Refused: that workspace no longer exists.".into();
        };
        let Some(plan) = &ws.plan else {
            return "Refused: this workspace has no structured plan to approve.".into();
        };
        if plan.approved {
            return "The plan is already approved.".into();
        }
        let updated = self.store.update_workspace(workspace_id, |l| {
            if let Some(p) = l.plan.as_mut() {
                p.approved = true;
            }
        });
        self.emit_workspace_event(workspace_id, "AR", "You", "Approved the plan.");
        self.touch_workspace_activity(workspace_id, "Approved the plan", self.now());
        let u = updated.unwrap_or_default();
        self.publish_loop_updated(&u.id, u.state.clone(), &u.phase, u.iteration);
        "Plan approved.".into()
    }
}

#[cfg(test)]
mod tests;
