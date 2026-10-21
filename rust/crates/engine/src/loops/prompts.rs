//! Phase prompts and roles (Go `loop_prompts.go`): phase prompt slots, the
//! slot → role id lookup, bundle role prompts as specialist system prompts,
//! slot expansion through the composer, and the findings-bar briefing.

use recur_bundles::{FindingsThreshold, SEVERITY_LEVELS, find_role};
use tokio_util::sync::CancellationToken;

use crate::composer::ExpandOpts;
use crate::loops::Walk;
use crate::loops::failure::LoopError;

impl Walk {
    /// A phase prompt slot's text ("" when unset).
    pub(crate) fn phase_slot(&self, slot: &str) -> String {
        self.walk_phase()
            .and_then(|ph| ph.prompts.get(slot).map(|s| s.trim().to_owned()))
            .unwrap_or_default()
    }

    /// The role id a slot names on the current phase, else on the loop.
    pub(crate) fn phase_role_id(&self, slot: &str) -> String {
        if let Some(ph) = self.walk_phase()
            && let Some(id) = ph.roles.get(slot)
            && !id.trim().is_empty()
        {
            return id.trim().to_owned();
        }
        self.guard
            .loop_def
            .roles
            .get(slot)
            .map(|s| s.trim().to_owned())
            .unwrap_or_default()
    }

    fn catalog_role_prompt(&mut self, role_id: &str) -> String {
        let role_id = role_id.trim();
        if role_id.is_empty() {
            return String::new();
        }
        let cat = self.loop_catalog();
        find_role(&cat.roles, role_id)
            .map(|r| r.prompt.trim().to_owned())
            .unwrap_or_default()
    }

    fn bundle_mission(&self) -> String {
        self.guard
            .loop_def
            .prompts
            .get("mission")
            .map(|s| s.trim().to_owned())
            .unwrap_or_default()
    }

    /// The role's bundle prompt plus the task and plan extras; `fallback`
    /// when the role has no body. The loop mission is the conductor's
    /// charter and is never pasted onto specialists.
    pub(crate) fn bundle_system_prompt(
        &mut self,
        role_id: &str,
        fallback: &str,
        task: &str,
        plan_extra: &str,
    ) -> String {
        let body = self.catalog_role_prompt(role_id);
        if body.is_empty() {
            return fallback.to_owned();
        }
        let mut b = body;
        let task = task.trim();
        if !task.is_empty() && task != self.bundle_mission() {
            b.push_str("\n\nTask: ");
            b.push_str(task);
        }
        if !plan_extra.trim().is_empty() {
            b.push_str(plan_extra);
        }
        b
    }

    /// Expands the phase slot (or `fallback`) through the composer. The
    /// display is the prompt action's label when the slot is a slash action;
    /// without a slot it is empty so callers keep the goal display.
    pub(crate) async fn expand_phase_prompt(
        &mut self,
        cancel: &CancellationToken,
        slot: &str,
        fallback: &str,
    ) -> Result<(String, String), LoopError> {
        let s = self.phase_slot(slot);
        let from_slot = !s.is_empty();
        let raw = if from_slot { s } else { fallback.to_owned() };
        let cat = self.loop_catalog();
        let catalog = (!cat.prompts.is_empty() || !cat.roles.is_empty()).then_some(cat);
        let res = self
            .eng
            .expand_composer(
                cancel,
                &raw,
                ExpandOpts {
                    worktree: self.worktree.clone(),
                    workspace_id: self.id.clone(),
                    ignore_disabled: true,
                    catalog,
                },
            )
            .await
            .map_err(|e| LoopError::msg(e.to_string()))?;
        if res.prompt.trim().is_empty() {
            return Ok((fallback.to_owned(), String::new()));
        }
        if !from_slot {
            return Ok((res.prompt, String::new()));
        }
        Ok((res.prompt, res.display))
    }

    /// The findings bar the reviewer is briefed with.
    pub(crate) fn findings_threshold_briefing(&self) -> String {
        let th = match self.walk_phase() {
            Some(ph) => self.guard.phase_threshold(&ph),
            None => self.guard.policy.findings_threshold.clone(),
        };
        format_findings_threshold(&th)
    }
}

/// The reviewer's briefing for a findings bar ("" for no named levels).
pub fn format_findings_threshold(th: &FindingsThreshold) -> String {
    if th.all_zero {
        return "Phase findings threshold: every open finding blocks (all severities at 0). Do not file a ship-blocker as low.".into();
    }
    if th.levels.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = SEVERITY_LEVELS
        .iter()
        .filter_map(|lv| th.levels.get(*lv).map(|cap| format!("{lv} ≤ {cap}")))
        .collect();
    if parts.is_empty() {
        return String::new();
    }
    format!(
        "Phase findings threshold (engine-enforced; named levels only): {}. Do not file a ship-blocker as low to sneak it through.",
        parts.join(", ")
    )
}
