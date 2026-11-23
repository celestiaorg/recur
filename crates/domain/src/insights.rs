//! The Insights report: how agents spent their time over a range.

/// Every duration is milliseconds of agent time: five agents busy for an hour is 5h.
/// `None` means the data is not recorded for that metric, so the view shows a dash.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InsightsReport {
    /// Progress (0 to 1) of indexing older history; while `Some`, the rest is empty.
    pub preparing: Option<f32>,
    pub agent_ms: i64,
    /// `agent_ms` over as long a stretch from the start of the previous period.
    pub prev_agent_ms: i64,
    pub tasks: TaskStats,
    /// The most agents running at the same moment, and when.
    pub peak_agents: i64,
    pub peak_at_ms: i64,
    pub asks: AskStats,
    pub merged: MergeStats,
    /// One entry per local day, oldest first, for the heatmap.
    pub days: Vec<DayHours>,
    pub split: TimeSplit,
    /// Time lost by cause, largest first. Only causes that occurred.
    pub lost: Vec<TimeLost>,
    /// Agent time by local hour of day (0 to 23).
    pub by_hour: [i64; 24],
    /// Average agent time per day by weekday, Monday first.
    pub by_weekday: [i64; 7],
    /// Models by agent time, largest first.
    pub models: Vec<ModelStats>,
    /// Commands by total time, largest first.
    pub commands: Vec<CommandStats>,
    pub handoffs: HandoffStats,
    pub context: ContextStats,
    pub tokens: TokenStats,
    pub changes: ChangeStats,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TaskStats {
    pub total: i64,
    pub finished: i64,
    pub failed: i64,
    /// Failures by reason ("plan limit", "provider error", …), largest first.
    pub failed_by: Vec<(String, i64)>,
    pub stopped: i64,
    pub running: i64,
    pub from_you: i64,
    pub from_agent: i64,
    pub from_schedule: i64,
    pub projects: i64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AskStats {
    /// Questions agents asked the human. Permission prompts are not recorded.
    pub count: i64,
    pub typical_wait_ms: Option<i64>,
    pub longest_wait_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MergeStats {
    /// Workspaces whose work landed on their base, by PR or directly.
    pub total: i64,
    pub by_pr: i64,
    pub direct: i64,
    pub open_prs: i64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DayHours {
    /// Local date, `YYYY-MM-DD`.
    pub date: String,
    pub agent_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TimeSplit {
    pub commands_ms: i64,
    pub model_ms: i64,
    pub waiting_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LostKind {
    BuildLock,
    RepeatedFailure,
    OvernightAsk,
    ProviderError,
    ReviewNotSettling,
}

impl LostKind {
    pub fn label(self) -> &'static str {
        match self {
            LostKind::BuildLock => "Cargo build lock",
            LostKind::RepeatedFailure => "Re-running failed commands",
            LostKind::OvernightAsk => "Overnight questions",
            LostKind::ProviderError => "Provider errors, retries",
            LostKind::ReviewNotSettling => "Review loops not settling",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimeLost {
    pub kind: LostKind,
    pub ms: i64,
    pub count: i64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelStats {
    /// Display name, e.g. "opus 5.5".
    pub model: String,
    pub provider: String,
    pub agent_ms: i64,
    pub runs: i64,
    /// `None` below the minimum sample (10 tasks).
    pub typical_task_ms: Option<i64>,
    /// Agent time per ask; `None` below the minimum sample or with no asks.
    pub ms_per_ask: Option<i64>,
    pub commands: i64,
    pub failed_commands: i64,
    /// `None` when the provider reports no usage.
    pub tokens_in: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CommandStats {
    /// Program or tool group, e.g. "cargo test", "Read / Edit files".
    pub name: String,
    pub total_ms: i64,
    pub calls: i64,
    pub p50_ms: i64,
    pub max_ms: i64,
    pub failed: i64,
    /// Searches that matched nothing (not failures).
    pub no_match: i64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HandoffStats {
    pub to_reviewer: i64,
    pub to_child: i64,
    pub review_loops: i64,
    pub review_settled: i64,
    pub unfinished: i64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ContextStats {
    pub compactions: i64,
    pub turns_over_90: i64,
    pub largest: i64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TokenStats {
    pub input: i64,
    pub cached: i64,
    pub output: i64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChangeStats {
    pub added: i64,
    pub removed: i64,
    pub files: i64,
}
