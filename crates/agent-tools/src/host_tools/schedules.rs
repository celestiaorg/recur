use super::{EMPTY_OBJECT_SCHEMA, strict};
use crate::display::schedule_tools::clamp_wakeup_delay;
use crate::host::ScheduleRequest;
use crate::json::{Shape, Ty, field};
use crate::names;
use crate::tool::{Call, Tool, ToolError};

const SCHEDULE_WAKEUP_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "delay_seconds": {"type": "integer", "description": "Seconds from now, clamped to 60..86400 (1 minute to 24 hours)"},
    "reason": {"type": "string", "description": "What to check, shown to the user and handed back to you when it fires"}
  },
  "required": ["delay_seconds", "reason"]
}"#;

static SCHEDULE_WAKEUP_IN: Shape =
    Shape::anon(&[field("delay_seconds", Ty::Int), field("reason", Ty::Str)]);

pub struct ScheduleWakeupTool;

#[async_trait::async_trait]
impl Tool for ScheduleWakeupTool {
    fn name(&self) -> &'static str {
        names::SCHEDULE_WAKEUP
    }

    fn description(&self) -> &'static str {
        "Check back later in this same chat: after delay_seconds (clamped to 60..86400, 1 minute \
to 24 hours) this chat gets a new turn carrying reason. Use it to follow a deploy, CI run or \
background process instead of sleeping. One-shot: to keep checking, call it again from that turn. \
A chat has at most one pending wakeup, so a new call replaces the earlier one. It survives an app \
restart (an overdue wakeup fires once on launch); closing the chat or archiving the workspace \
cancels it."
    }

    fn schema(&self) -> String {
        SCHEDULE_WAKEUP_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &SCHEDULE_WAKEUP_IN)?;
        let reason = a.str("reason").trim();
        if reason.is_empty() {
            return Err("reason is required".into());
        }
        let delay = clamp_wakeup_delay(a.int("delay_seconds"));
        Ok(
            h.schedule_wakeup(&c.workspace_id, &c.agent_code, delay, reason)
                .await?,
        )
    }
}

const CREATE_SCHEDULE_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "name": {"type": "string", "description": "Short name shown in the Schedules view"},
    "prompt": {"type": "string", "description": "What the new workspace's agent is asked to do"},
    "cadence": {"type": "string", "enum": ["once", "daily", "weekdays", "cron"], "description": "once needs date and time; daily and weekdays need time; cron needs cron"},
    "date": {"type": "string", "description": "YYYY-MM-DD, for once"},
    "time": {"type": "string", "description": "HH:MM (24-hour), for once, daily and weekdays"},
    "cron": {"type": "string", "description": "Cron expression, for cron (5 fields, e.g. \"0 9 * * MON-FRI\")"},
    "time_zone": {"type": "string", "description": "IANA zone the times are in (default: this machine's zone)"},
    "mode": {"type": "string", "enum": ["ask", "plan", "agent", "review"], "description": "Chat mode of the run"},
    "model": {"type": "string", "description": "provider/model ref (default: the Settings default for mode)"},
    "base_branch": {"type": "string", "description": "Branch the new workspace starts from (default: the project's base branch)"}
  },
  "required": ["name", "prompt", "cadence", "mode"]
}"#;

static CREATE_SCHEDULE_IN: Shape = Shape::anon(&[
    field("name", Ty::Str),
    field("prompt", Ty::Str),
    field("cadence", Ty::Str),
    field("date", Ty::Str),
    field("time", Ty::Str),
    field("cron", Ty::Str),
    field("time_zone", Ty::Str),
    field("mode", Ty::Str),
    field("model", Ty::Str),
    field("base_branch", Ty::Str),
]);

pub struct CreateScheduleTool;

#[async_trait::async_trait]
impl Tool for CreateScheduleTool {
    fn name(&self) -> &'static str {
        names::CREATE_SCHEDULE
    }

    fn description(&self) -> &'static str {
        "Create a scheduled prompt in this workspace's project. At each occurrence Recur starts a \
new, independent workspace that runs prompt in mode; it does not come back to this chat (use \
schedule_wakeup for that). Times are in time_zone. The user sees it in the Schedules view and \
can pause, edit or delete it there. Every occurrence is a paid agent run, so Recur asks the user \
first unless permissions are bypassed. Returns the schedule with its id and next run."
    }

    fn schema(&self) -> String {
        CREATE_SCHEDULE_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &CREATE_SCHEDULE_IN)?;
        let s = |k: &str| a.str(k).trim().to_owned();
        let req = ScheduleRequest {
            name: s("name"),
            prompt: s("prompt"),
            cadence: s("cadence"),
            date: s("date"),
            time: s("time"),
            cron: s("cron"),
            time_zone: s("time_zone"),
            mode: s("mode"),
            model: s("model"),
            base_branch: s("base_branch"),
        };
        if req.name.is_empty() || req.prompt.is_empty() {
            return Err("name and prompt are required".into());
        }
        Ok(
            h.create_schedule(&c.cancel, &c.workspace_id, &c.agent_code, req)
                .await?,
        )
    }
}

pub struct ListSchedulesTool;

#[async_trait::async_trait]
impl Tool for ListSchedulesTool {
    fn name(&self) -> &'static str {
        names::LIST_SCHEDULES
    }

    fn description(&self) -> &'static str {
        "List this workspace's project's scheduled prompts: id, name, cadence (cron or date/time), \
time zone, mode, model, base branch, paused, next run and the last run's status."
    }

    fn schema(&self) -> String {
        EMPTY_OBJECT_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        Ok(h.list_schedules(&c.workspace_id, &c.agent_code).await?)
    }
}

const DELETE_SCHEDULE_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "id": {"type": "string", "description": "Schedule id from list_schedules or create_schedule"}
  },
  "required": ["id"]
}"#;

static DELETE_SCHEDULE_IN: Shape = Shape::anon(&[field("id", Ty::Str)]);

pub struct DeleteScheduleTool;

#[async_trait::async_trait]
impl Tool for DeleteScheduleTool {
    fn name(&self) -> &'static str {
        names::DELETE_SCHEDULE
    }

    fn description(&self) -> &'static str {
        "Delete one of this project's scheduled prompts. Future runs stop; workspaces it already \
started stay. Recur asks the user first unless permissions are bypassed."
    }

    fn schema(&self) -> String {
        DELETE_SCHEDULE_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &DELETE_SCHEDULE_IN)?;
        let id = a.str("id").trim();
        if id.is_empty() {
            return Err("id is required".into());
        }
        Ok(
            h.delete_schedule(&c.cancel, &c.workspace_id, &c.agent_code, id)
                .await?,
        )
    }
}
