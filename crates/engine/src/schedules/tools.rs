//! The create_schedule, list_schedules and delete_schedule host tools: agents
//! manage the project's scheduled prompts through the same validation and
//! store as the Schedules view, scoped to their workspace's project.

use chrono::DateTime;
use recur_agent_tools::names::mcp;
use recur_agent_tools::{HostError, ScheduleRequest};
use recur_base::tz::Tz;
use recur_domain::{ScheduledPrompt, next_scheduled_time};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::Engine;
use crate::view::modes::default_model_for_mode;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScheduleCard<'a> {
    id: &'a str,
    name: &'a str,
    prompt: &'a str,
    cadence: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    cron: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    date: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    time: &'a str,
    time_zone: &'a str,
    mode: &'a str,
    model: &'a str,
    base_branch: &'a str,
    paused: bool,
    /// Local wall time in the schedule's zone; empty when none is due.
    next_run: String,
    #[serde(skip_serializing_if = "str::is_empty")]
    last_run: &'a str,
}

fn card(p: &ScheduledPrompt) -> ScheduleCard<'_> {
    let next_run = match (
        p.time_zone.parse::<Tz>(),
        DateTime::from_timestamp_millis(p.next_run_at),
    ) {
        (Ok(tz), Some(t)) if p.next_run_at > 0 => {
            t.with_timezone(&tz).format("%Y-%m-%d %H:%M %Z").to_string()
        }
        _ => String::new(),
    };
    ScheduleCard {
        id: &p.id,
        name: &p.name,
        prompt: &p.prompt,
        cadence: &p.cadence,
        cron: &p.cron,
        date: &p.date,
        time: &p.time,
        time_zone: &p.time_zone,
        mode: &p.mode,
        model: &p.model,
        base_branch: &p.base_branch,
        paused: p.paused,
        next_run,
        last_run: p.last_run.as_ref().map_or("", |r| r.status.as_str()),
    }
}

fn json<T: Serialize>(v: &T) -> Result<String, HostError> {
    serde_json::to_string(v).map_err(|e| HostError(e.to_string()))
}

impl Engine {
    fn workspace_project(&self, workspace_id: &str) -> Result<recur_domain::Project, HostError> {
        self.store
            .workspace(workspace_id)
            .and_then(|w| self.store.project(&w.project_id))
            .ok_or_else(|| HostError::new("workspace not found"))
    }

    fn project_schedules(&self, project_id: &str) -> Result<Vec<ScheduledPrompt>, HostError> {
        Ok(self
            .store
            .scheduled_prompts()
            .map_err(|e| HostError(e.to_string()))?
            .into_iter()
            .filter(|p| p.project_id == project_id)
            .collect())
    }

    /// Host `create_schedule`: defaults filled, the timing checked before the
    /// approval prompt, then saved like the Schedules view saves.
    pub async fn create_schedule_from_chat(
        &self,
        cancel: &CancellationToken,
        workspace_id: &str,
        chat: &str,
        req: ScheduleRequest,
    ) -> Result<String, HostError> {
        let proj = self.workspace_project(workspace_id)?;
        let mut p = ScheduledPrompt {
            project_id: proj.id,
            name: req.name,
            prompt: req.prompt,
            cadence: req.cadence,
            date: req.date,
            time: req.time,
            cron: req.cron,
            time_zone: req.time_zone,
            mode: req.mode,
            model: req.model,
            base_branch: req.base_branch,
            ..ScheduledPrompt::default()
        };
        if p.time_zone.is_empty() {
            p.time_zone = iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".into());
        }
        if p.base_branch.is_empty() {
            p.base_branch = proj.base_branch;
        }
        if p.model.is_empty() {
            let d = default_model_for_mode(&p.mode, Some(&self.settings()));
            (p.model, p.flags) = (d.model, d.flags);
        }
        next_scheduled_time(&p, self.now().to_utc()).map_err(|e| HostError(e.to_string()))?;
        let input = serde_json::json!({
            "name": p.name,
            "prompt": p.prompt,
            "cadence": p.cadence,
            "cron": p.cron,
            "date": p.date,
            "time": p.time,
            "time_zone": p.time_zone,
            "mode": p.mode,
            "model": p.model,
        });
        self.machine_permission(
            cancel,
            workspace_id,
            chat,
            mcp::CREATE_SCHEDULE,
            input,
            "schedules",
        )
        .await?;
        let saved = self
            .save_scheduled_prompt(p)
            .await
            .map_err(|e| HostError(e.0))?;
        json(&card(&saved))
    }

    /// Host `list_schedules`: this workspace's project only.
    pub fn list_schedules_from_chat(&self, workspace_id: &str) -> Result<String, HostError> {
        let proj = self.workspace_project(workspace_id)?;
        let all = self.project_schedules(&proj.id)?;
        let cards: Vec<_> = all.iter().map(card).collect();
        json(&serde_json::json!({"count": cards.len(), "schedules": cards}))
    }

    /// Host `delete_schedule`: only this project's schedules.
    pub async fn delete_schedule_from_chat(
        &self,
        cancel: &CancellationToken,
        workspace_id: &str,
        chat: &str,
        id: &str,
    ) -> Result<String, HostError> {
        let proj = self.workspace_project(workspace_id)?;
        let Some(p) = self
            .project_schedules(&proj.id)?
            .into_iter()
            .find(|p| p.id == id)
        else {
            return Err(HostError(format!(
                "schedule {id:?} not found in this project: call list_schedules"
            )));
        };
        self.machine_permission(
            cancel,
            workspace_id,
            chat,
            mcp::DELETE_SCHEDULE,
            serde_json::json!({"id": p.id, "name": p.name}),
            "schedules",
        )
        .await?;
        self.delete_scheduled_prompt(id)
            .await
            .map_err(|e| HostError(e.0))?;
        json(&serde_json::json!({"id": p.id, "name": p.name, "deleted": true}))
    }
}

#[cfg(test)]
#[path = "tools_tests.rs"]
mod tests;
