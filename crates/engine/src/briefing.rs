//! The Briefing projection: one deterministic cross-workspace Now/Next view for
//! the Overview and Ask Recur, the bounded per-workspace context the
//! coordinator reads, and the activity watermarks (`mark_workspace_viewed`,
//! `touch_workspace_activity`) that drive "changed since you looked".

use std::collections::HashMap;

use chrono::{DateTime, Local};
use recur_domain::{
    Agent, AgentSummary, BLOCK_TEXT, BRIEF_ACTION_ANSWER, BRIEF_ACTION_APPROVE,
    BRIEF_ACTION_CONTINUE, BRIEF_ACTION_OPEN, BRIEF_ACTION_REVIEW, BRIEF_BUCKET_NEEDS,
    BRIEF_BUCKET_PICKUP, BRIEF_BUCKET_WORKING, BRIEF_REASON_ACTIVE_AGENT, BRIEF_REASON_DIRTY_IDLE,
    BRIEF_REASON_ESCALATION, BRIEF_REASON_FAILURE, BRIEF_REASON_FINDINGS, BRIEF_REASON_PERMISSION,
    BRIEF_REASON_PLAN_APPROVAL, BRIEF_REASON_PR_ATTENTION, BRIEF_REASON_QUESTION,
    BRIEF_REASON_RECENT_CHAT, BRIEF_REASON_RETRYABLE_FAILURE, BRIEF_REASON_UNFINISHED_PLAN,
    BRIEF_REASON_UNSEEN_CHAT, BriefAction, BriefingCounts, BriefingSnapshot, ChatHeader,
    ConversationLine, Escalation, Message, OpenAsk, PermissionRequest, Plan, Pr, PrSummary,
    REVIEW_STATE_CHANGES_REQUESTED, STEP_DOING, STEP_DONE, STEP_PENDING, STEP_SKIPPED, Session,
    UserQuestion, Workspace, WorkspaceBrief, WorkspaceContext, WorkspaceState,
};
use recur_store::Store;

use crate::app_chat::session_title;
use crate::bus::{BriefingUpdated, EngineEvent};
use crate::chat::children::is_live_agent_status;
use crate::codes::{MAESTRO_AGENT_CODE, is_chat_attention_code, loop_owner_code};
use crate::session_config::AgentConfig;
use crate::text::truncate;
use crate::{Engine, EngineError, Result};

/// Activity older than this reads "quiet for …".
pub const BRIEF_QUIET_AFTER_MS: i64 = 24 * 3600 * 1000;
pub const BRIEF_EXCERPT_MAX: usize = 8;
pub const BRIEF_LINE_MAX: usize = 240;

#[derive(Debug, Clone, Default)]
struct BriefReason {
    code: &'static str,
    rank: i32,
    now: String,
    next: String,
    action: BriefAction,
}

/// Everything one workspace's brief is computed from.
#[derive(Debug, Clone, Default)]
pub struct BriefInputs {
    pub ws: Workspace,
    pub project_name: String,
    pub agents: Vec<Agent>,
    pub headers: Vec<Session>,
    pub activity: i64,
    pub now_ms: i64,
    pub perm: Option<PermissionRequest>,
    pub question: Option<UserQuestion>,
    pub escalation: Option<Escalation>,
    pub pr: Option<Pr>,
    pub open_findings: i64,
}

fn action(label: &str, target: String, kind: &str) -> BriefAction {
    BriefAction {
        label: label.into(),
        target,
        kind: kind.into(),
    }
}

/// The workspace's last meaningful activity: the watermark, else (legacy
/// rows) its newest session, event or PR update.
pub fn effective_workspace_activity(
    ws: &Workspace,
    headers: &[Session],
    event_ts: i64,
    pr: Option<&Pr>,
) -> i64 {
    let mut ms = ws.last_activity_at;
    if ms > 0 {
        return ms;
    }
    for s in headers {
        ms = ms.max(s.updated_at);
    }
    ms = ms.max(event_ts);
    if let Some(p) = pr {
        ms = ms.max(parse_rfc3339_milli(&p.updated_at));
    }
    ms
}

fn parse_rfc3339_milli(s: &str) -> i64 {
    let s = s.trim();
    if s.is_empty() {
        return 0;
    }
    DateTime::parse_from_rfc3339(s).map_or(0, |t| t.timestamp_millis())
}

/// One workspace's Overview card, or `None` when nothing calls for it.
pub fn build_workspace_brief(inp: &BriefInputs) -> Option<WorkspaceBrief> {
    let ws = &inp.ws;
    let reasons = collect_brief_reasons(inp);
    if reasons.is_empty() || settled_empty(inp) {
        return None;
    }
    let primary = &reasons[0];
    let bucket = bucket_for(primary.code, inp);
    if bucket.is_empty() {
        return None;
    }
    let mut codes = Vec::with_capacity(reasons.len());
    let mut secondary = Vec::new();
    let mut seen_act = vec![primary.action.target.clone()];
    for r in &reasons {
        codes.push(r.code.to_owned());
        if r.action.target.is_empty() || seen_act.contains(&r.action.target) {
            continue;
        }
        seen_act.push(r.action.target.clone());
        secondary.push(r.action.clone());
    }
    let mut title = workspace_title(ws);
    if ws.is_pr_chat_host()
        && let Some(p) = &inp.pr
        && !p.title.trim().is_empty()
    {
        title = p.title.clone();
    }
    let (plan_done, plan_total) = plan_progress(ws.plan.as_ref());
    let (latest_code, latest_title) = latest_chat(&inp.headers, &inp.agents);
    let (active_n, active_task, _) = first_active_agent(&inp.agents);
    let mut b = WorkspaceBrief {
        workspace_id: ws.id.clone(),
        project_id: ws.project_id.clone(),
        project_name: inp.project_name.clone(),
        title,
        branch: ws.branch.clone(),
        bucket: bucket.to_owned(),
        reasons: Some(codes),
        now: primary.now.clone(),
        next: primary.next.clone(),
        last_activity_at: inp.activity,
        last_viewed_at: ws.last_viewed_at,
        seen_activity_at: ws.seen_activity_at,
        // "Changed since you looked" needs a prior look: a never-viewed
        // workspace must not raise the cue off its own creation.
        changed_since_viewed: ws.seen_activity_at > 0 && inp.activity > ws.seen_activity_at,
        quiet_age: quiet_age(inp.activity, inp.now_ms),
        last_summary: ws.last.trim().to_owned(),
        state: ws.state.as_str().to_owned(),
        phase: ws.phase.clone(),
        plan_summary: plan_headline(ws.plan.as_ref()),
        plan_done,
        plan_total,
        active_agents: active_n,
        active_agent_task: active_task,
        latest_chat_code: latest_code,
        latest_chat_title: latest_title,
        diff_add: ws.diff.add,
        diff_del: ws.diff.del,
        is_pr_host: ws.is_pr_chat_host(),
        primary: primary.action.clone(),
        secondary,
        ..WorkspaceBrief::default()
    };
    if let Some(p) = &inp.pr {
        b.pr_id = p.id.clone();
        b.pr_number = p.number;
    }
    Some(b)
}

fn settled_empty(inp: &BriefInputs) -> bool {
    if matches!(
        inp.ws.state,
        WorkspaceState::Merged | WorkspaceState::Stopped
    ) && inp.perm.is_none()
        && inp.question.is_none()
        && !has_unseen_chat(&inp.agents)
        && !pr_needs_attention(inp.pr.as_ref())
    {
        return true;
    }
    inp.ws.is_main && !has_meaningful_work(inp)
}

fn has_meaningful_work(inp: &BriefInputs) -> bool {
    if inp.perm.is_some() || inp.question.is_some() || inp.escalation.is_some() {
        return true;
    }
    if has_unseen_chat(&inp.agents) || active_agent_count(&inp.agents) > 0 {
        return true;
    }
    if inp.ws.diff.add + inp.ws.diff.del > 0 {
        return true;
    }
    if let Some(p) = &inp.ws.plan
        && !p.steps.is_empty()
        && !plan_all_resolved(Some(p))
    {
        return true;
    }
    if !inp.headers.is_empty() {
        return true;
    }
    matches!(
        inp.ws.state,
        WorkspaceState::Queued
            | WorkspaceState::Planning
            | WorkspaceState::Coding
            | WorkspaceState::Reviewing
            | WorkspaceState::Verifying
            | WorkspaceState::Escalated
            | WorkspaceState::Failed
            | WorkspaceState::PrOpen
    )
}

fn collect_brief_reasons(inp: &BriefInputs) -> Vec<BriefReason> {
    let mut out = Vec::new();
    let ws = &inp.ws;
    if let Some(perm) = &inp.perm {
        let tool = match perm.tool.trim() {
            "" => "a tool",
            t => t,
        };
        out.push(BriefReason {
            code: BRIEF_REASON_PERMISSION,
            rank: 0,
            now: format!("An agent is waiting to use {tool}"),
            next: "Review the permission".into(),
            action: action(
                "Review",
                chat_or_workspace_target(&ws.id, &perm.agent_code),
                BRIEF_ACTION_REVIEW,
            ),
        });
    }
    if let Some(q) = &inp.question {
        let chat_name = chat_label(&inp.agents, &inp.headers, &q.agent_code);
        out.push(BriefReason {
            code: BRIEF_REASON_QUESTION,
            rank: 1,
            now: waiting_chat_now(&chat_name),
            next: truncate(q.question.trim(), 140),
            action: action("Answer", format!("question:{}", q.id), BRIEF_ACTION_ANSWER),
        });
    }
    if let Some(esc) = &inp.escalation
        && esc.answer.trim().is_empty()
    {
        out.push(BriefReason {
            code: BRIEF_REASON_ESCALATION,
            rank: 2,
            now: "Run is paused on a decision".into(),
            next: truncate(esc.question.trim(), 140),
            action: action("Answer", loop_chat_target(ws), BRIEF_ACTION_ANSWER),
        });
    }
    if ws.state == WorkspaceState::Failed {
        if ws.retryable {
            out.push(BriefReason {
                code: BRIEF_REASON_RETRYABLE_FAILURE,
                rank: 3,
                now: "The workflow stopped at a checkpoint".into(),
                next: "Continue in the workflow chat".into(),
                action: action("Open", loop_chat_target(ws), BRIEF_ACTION_OPEN),
            });
        } else {
            let reason = match ws.last_error.trim() {
                "" => "the persisted error",
                r => r,
            };
            out.push(BriefReason {
                code: BRIEF_REASON_FAILURE,
                rank: 4,
                now: format!("Run failed: {}", truncate(reason, 120)),
                next: "Open the workflow chat".into(),
                action: action("Open", loop_chat_target(ws), BRIEF_ACTION_OPEN),
            });
        }
    }
    if let Some(p) = &ws.plan
        && !p.approved
        && (!p.body.trim().is_empty() || !p.steps.is_empty())
    {
        out.push(BriefReason {
            code: BRIEF_REASON_PLAN_APPROVAL,
            rank: 5,
            now: "Plan is ready but not approved".into(),
            next: "Review and approve the plan".into(),
            action: action(
                "Review plan",
                format!("workspace:{}:plan", ws.id),
                BRIEF_ACTION_APPROVE,
            ),
        });
    }
    if inp.open_findings > 0 && !is_active_run(&ws.state) {
        let now = if inp.open_findings == 1 {
            "1 open finding".to_owned()
        } else {
            format!("{} open findings", inp.open_findings)
        };
        out.push(BriefReason {
            code: BRIEF_REASON_FINDINGS,
            rank: 6,
            now,
            next: "Review findings".into(),
            action: action(
                "Review",
                format!("workspace:{}:findings", ws.id),
                BRIEF_ACTION_REVIEW,
            ),
        });
    }
    if let Some(p) = inp.pr.as_ref().filter(|p| pr_needs_attention(Some(p))) {
        let label = if p.review_decision == REVIEW_STATE_CHANGES_REQUESTED
            || p.state.to_lowercase().contains("change")
        {
            "PR requests changes"
        } else {
            "PR is ready for review"
        };
        out.push(BriefReason {
            code: BRIEF_REASON_PR_ATTENTION,
            rank: 7,
            now: label.into(),
            next: "Review the PR".into(),
            action: action("Review", format!("pr:{}", p.id), BRIEF_ACTION_REVIEW),
        });
    }
    let (code, name) = first_unseen_chat(&inp.agents, &inp.headers);
    if !code.is_empty() {
        out.push(BriefReason {
            code: BRIEF_REASON_UNSEEN_CHAT,
            rank: 8,
            now: unread_chat_now(&name),
            next: "Read the response".into(),
            action: action("Read", format!("chat:{}:{code}", ws.id), BRIEF_ACTION_OPEN),
        });
    }
    let (n, task, code) = first_active_agent(&inp.agents);
    if n > 0 {
        let now = if task.is_empty() {
            status_now(ws)
        } else {
            format!("{} is {task}", agent_name(&inp.agents, &code))
        };
        let mut next = next_plan_step(ws.plan.as_ref());
        if next.is_empty() {
            next = "Open the chat".into();
        }
        out.push(BriefReason {
            code: BRIEF_REASON_ACTIVE_AGENT,
            rank: 9,
            now,
            next,
            action: action(
                "Open",
                agent_or_loop_chat_target(ws, &code),
                BRIEF_ACTION_OPEN,
            ),
        });
    } else if is_active_run(&ws.state) {
        let mut next = next_plan_step(ws.plan.as_ref());
        if next.is_empty() {
            next = "Open the workflow chat".into();
        }
        out.push(BriefReason {
            code: BRIEF_REASON_ACTIVE_AGENT,
            rank: 9,
            now: status_now(ws),
            next,
            action: action("Open", loop_chat_target(ws), BRIEF_ACTION_OPEN),
        });
    }
    if let Some(p) = &ws.plan
        && !p.steps.is_empty()
        && !plan_all_resolved(Some(p))
    {
        let (done, total) = plan_progress(Some(p));
        out.push(BriefReason {
            code: BRIEF_REASON_UNFINISHED_PLAN,
            rank: 11,
            now: format!("{done} of {total} plan steps are complete"),
            next: first_open_step(Some(p)),
            action: action(
                "Continue",
                format!("workspace:{}:plan", ws.id),
                BRIEF_ACTION_CONTINUE,
            ),
        });
    }
    if ws.diff.add + ws.diff.del > 0 && !is_active_run(&ws.state) {
        out.push(BriefReason {
            code: BRIEF_REASON_DIRTY_IDLE,
            rank: 12,
            now: "Branch has unlanded changes".into(),
            next: "Review the unlanded changes".into(),
            action: action(
                "Review",
                format!("workspace:{}:review", ws.id),
                BRIEF_ACTION_REVIEW,
            ),
        });
    }
    let (code, title) = latest_chat(&inp.headers, &inp.agents);
    if !code.is_empty()
        && !title.is_empty()
        && !is_active_run(&ws.state)
        && ws.state != WorkspaceState::Failed
    {
        out.push(BriefReason {
            code: BRIEF_REASON_RECENT_CHAT,
            rank: 13,
            now: continue_chat_now(&title),
            next: "Continue that chat".into(),
            action: action(
                "Continue",
                format!("chat:{}:{code}", ws.id),
                BRIEF_ACTION_CONTINUE,
            ),
        });
    }
    // Exchange sort, kept for its tie order.
    for i in 0..out.len() {
        for j in i + 1..out.len() {
            if out[j].rank < out[i].rank {
                out.swap(i, j);
            }
        }
    }
    out
}

fn bucket_for(code: &str, inp: &BriefInputs) -> &'static str {
    match code {
        BRIEF_REASON_PERMISSION
        | BRIEF_REASON_QUESTION
        | BRIEF_REASON_ESCALATION
        | BRIEF_REASON_RETRYABLE_FAILURE
        | BRIEF_REASON_FAILURE
        | BRIEF_REASON_PLAN_APPROVAL
        | BRIEF_REASON_FINDINGS
        | BRIEF_REASON_PR_ATTENTION
        | BRIEF_REASON_UNSEEN_CHAT => BRIEF_BUCKET_NEEDS,
        BRIEF_REASON_ACTIVE_AGENT => BRIEF_BUCKET_WORKING,
        BRIEF_REASON_UNFINISHED_PLAN | BRIEF_REASON_DIRTY_IDLE | BRIEF_REASON_RECENT_CHAT => {
            BRIEF_BUCKET_PICKUP
        }
        _ if is_active_run(&inp.ws.state) || active_agent_count(&inp.agents) > 0 => {
            BRIEF_BUCKET_WORKING
        }
        _ => "",
    }
}

fn is_active_run(st: &WorkspaceState) -> bool {
    matches!(
        st,
        WorkspaceState::Queued
            | WorkspaceState::Planning
            | WorkspaceState::Coding
            | WorkspaceState::Reviewing
            | WorkspaceState::Verifying
    )
}

fn pr_needs_attention(p: Option<&Pr>) -> bool {
    let Some(p) = p else {
        return false;
    };
    if p.state.eq_ignore_ascii_case("Merged") || p.state.eq_ignore_ascii_case("Closed") {
        return false;
    }
    if p.review_requested || p.review_decision == REVIEW_STATE_CHANGES_REQUESTED {
        return true;
    }
    let st = p.state.to_lowercase();
    st.contains("review") || st.contains("ready")
}

fn has_unseen_chat(agents: &[Agent]) -> bool {
    agents
        .iter()
        .any(|a| a.unseen && is_chat_attention_code(&a.code))
}

fn first_unseen_chat(agents: &[Agent], headers: &[Session]) -> (String, String) {
    agents
        .iter()
        .find(|a| a.unseen && is_chat_attention_code(&a.code))
        .map(|a| (a.code.clone(), chat_label(agents, headers, &a.code)))
        .unwrap_or_default()
}

fn active_agent_count(agents: &[Agent]) -> i64 {
    agents
        .iter()
        .filter(|a| is_live_agent_status(&a.status))
        .count() as i64
}

/// (live count, the first live agent's task or status, its code).
fn first_active_agent(agents: &[Agent]) -> (i64, String, String) {
    let mut n = 0;
    let (mut task, mut code) = (String::new(), String::new());
    for a in agents.iter().filter(|a| is_live_agent_status(&a.status)) {
        n += 1;
        if code.is_empty() {
            code = a.code.clone();
            task = a.task.trim().to_owned();
            if task.is_empty() {
                task = a.status.clone();
            }
        }
    }
    (n, task, code)
}

/// The sidebar title: short, else the goal (capped), else the id.
pub fn workspace_title(ws: &Workspace) -> String {
    let s = ws.short.trim();
    if !s.is_empty() {
        return s.to_owned();
    }
    let g = ws.goal.trim();
    if !g.is_empty() {
        return truncate(g, 80);
    }
    ws.id.clone()
}

fn status_now(ws: &Workspace) -> String {
    let p = ws.phase.trim();
    if !p.is_empty() {
        return format!("Run is {p}");
    }
    let s = ws.status_label.trim();
    if !s.is_empty() {
        return s.to_owned();
    }
    if !ws.state.is_empty() {
        return format!("Run is {}", ws.state.as_str());
    }
    "Work is in progress".into()
}

fn plan_headline(p: Option<&Plan>) -> String {
    p.map(|p| p.summary.trim().to_owned()).unwrap_or_default()
}

fn plan_progress(p: Option<&Plan>) -> (i64, i64) {
    let Some(p) = p else {
        return (0, 0);
    };
    let done = p
        .steps
        .iter()
        .filter(|s| s.status == STEP_DONE || s.status == STEP_SKIPPED)
        .count() as i64;
    (done, p.steps.len() as i64)
}

fn plan_all_resolved(p: Option<&Plan>) -> bool {
    match p {
        None => true,
        Some(p) if p.steps.is_empty() => true,
        Some(p) => p
            .steps
            .iter()
            .all(|s| s.status == STEP_DONE || s.status == STEP_SKIPPED),
    }
}

fn next_plan_step(p: Option<&Plan>) -> String {
    let Some(plan) = p else {
        return String::new();
    };
    if let Some(s) = plan.steps.iter().find(|s| s.status == STEP_DOING) {
        return s.title.clone();
    }
    first_open_step(p)
}

fn first_open_step(p: Option<&Plan>) -> String {
    p.and_then(|p| {
        p.steps
            .iter()
            .find(|s| s.status == STEP_DOING || s.status == STEP_PENDING)
    })
    .map(|s| s.title.clone())
    .unwrap_or_default()
}

/// The most recently updated chat-tab session (code, title), else the
/// first chat agent.
fn latest_chat(headers: &[Session], agents: &[Agent]) -> (String, String) {
    let mut best_at = -1;
    let (mut code, mut title) = (String::new(), String::new());
    for s in headers {
        let cfg = AgentConfig::parse(&s.config_json);
        if cfg.code.is_empty() || !is_chat_attention_code(&cfg.code) {
            continue;
        }
        if s.updated_at >= best_at {
            best_at = s.updated_at;
            title = session_title(&cfg);
            code = cfg.code;
        }
    }
    if !code.is_empty() {
        return (code, title);
    }
    agents
        .iter()
        .find(|a| is_chat_attention_code(&a.code))
        .map(|a| (a.code.clone(), a.name.clone()))
        .unwrap_or_default()
}

/// The chat's identity in Now copy: generic tab labels collapse to "chat",
/// real titles are quoted.
pub fn chat_noun(name: &str) -> String {
    let name = name.trim();
    match name.to_lowercase().as_str() {
        "" | "new chat" | "chat" | "agent" => return "chat".into(),
        "maestro" | "loop" => return name.to_owned(),
        _ => {}
    }
    if is_chat_attention_code(name) {
        return "chat".into();
    }
    format!("“{}”", truncate(name, 80))
}

fn continue_chat_now(title: &str) -> String {
    match chat_noun(title).as_str() {
        "chat" => "Continue the latest chat".into(),
        n => format!("Continue the {n} chat"),
    }
}

fn unread_chat_now(name: &str) -> String {
    match chat_noun(name).as_str() {
        "chat" => "New reply in chat".into(),
        n => format!("New reply in {n}"),
    }
}

fn waiting_chat_now(name: &str) -> String {
    match chat_noun(name).as_str() {
        "chat" => "A chat is waiting for an answer".into(),
        n => format!("{n} is waiting for an answer"),
    }
}

fn chat_label(agents: &[Agent], headers: &[Session], code: &str) -> String {
    if code.is_empty() {
        return "Chat".into();
    }
    if let Some(a) = agents.iter().find(|a| a.code == code) {
        let t = a.name.trim();
        if !t.is_empty() {
            return t.to_owned();
        }
    }
    for s in headers {
        let cfg = AgentConfig::parse(&s.config_json);
        if cfg.code == code {
            let t = session_title(&cfg);
            if !t.is_empty() {
                return t;
            }
        }
    }
    if code == MAESTRO_AGENT_CODE {
        return "Maestro".into();
    }
    code.to_owned()
}

fn agent_name(agents: &[Agent], code: &str) -> String {
    agents
        .iter()
        .find(|a| a.code == code && !a.name.trim().is_empty())
        .map_or_else(|| code.to_owned(), |a| a.name.clone())
}

fn chat_or_workspace_target(workspace_id: &str, code: &str) -> String {
    if is_chat_attention_code(code) {
        format!("chat:{workspace_id}:{code}")
    } else {
        format!("workspace:{workspace_id}")
    }
}

fn loop_chat_target(ws: &Workspace) -> String {
    let mut code = loop_owner_code(ws);
    if code.is_empty() {
        code = MAESTRO_AGENT_CODE.into();
    }
    format!("chat:{}:{code}", ws.id)
}

fn agent_or_loop_chat_target(ws: &Workspace, code: &str) -> String {
    if is_chat_attention_code(code) {
        format!("chat:{}:{code}", ws.id)
    } else {
        loop_chat_target(ws)
    }
}

fn quiet_age(activity_ms: i64, now_ms: i64) -> String {
    if activity_ms <= 0 {
        return String::new();
    }
    let d = now_ms - activity_ms;
    if d < BRIEF_QUIET_AFTER_MS {
        return String::new();
    }
    format!("quiet for {}", format_age(d))
}

/// Hours under two days, else days, from milliseconds.
pub fn format_age(d_ms: i64) -> String {
    let h = d_ms.max(0) / 3_600_000;
    if h < 48 {
        return format!("{h}h");
    }
    format!("{}d", h / 24)
}

/// Exchange sort with bucket, recency and id tie-breaks.
pub fn sort_briefs(briefs: &mut [WorkspaceBrief]) {
    let rank = |b: &WorkspaceBrief| match b.bucket.as_str() {
        BRIEF_BUCKET_NEEDS => 0,
        BRIEF_BUCKET_WORKING => 1,
        _ => 2,
    };
    let needs_rank = |b: &WorkspaceBrief| match b.reasons.as_deref().and_then(<[String]>::first) {
        None => 99,
        Some(r) => match r.as_str() {
            BRIEF_REASON_PERMISSION
            | BRIEF_REASON_QUESTION
            | BRIEF_REASON_ESCALATION
            | BRIEF_REASON_RETRYABLE_FAILURE
            | BRIEF_REASON_FAILURE
            | BRIEF_REASON_PLAN_APPROVAL
            | BRIEF_REASON_FINDINGS => 0,
            BRIEF_REASON_PR_ATTENTION => 1,
            BRIEF_REASON_UNSEEN_CHAT => 2,
            _ => 3,
        },
    };
    let queued = |b: &WorkspaceBrief| b.state == WorkspaceState::Queued.as_str();
    for i in 0..briefs.len() {
        for j in i + 1..briefs.len() {
            let (a, b) = (&briefs[i], &briefs[j]);
            let swap = if rank(b) != rank(a) {
                rank(b) < rank(a)
            } else {
                match a.bucket.as_str() {
                    BRIEF_BUCKET_NEEDS => {
                        b.last_activity_at > a.last_activity_at
                            || (b.last_activity_at == a.last_activity_at
                                && (needs_rank(b) < needs_rank(a)
                                    || (needs_rank(b) == needs_rank(a)
                                        && b.workspace_id < a.workspace_id)))
                    }
                    BRIEF_BUCKET_WORKING => {
                        (queued(a) && !queued(b))
                            || (queued(a) == queued(b)
                                && (b.last_activity_at > a.last_activity_at
                                    || (b.last_activity_at == a.last_activity_at
                                        && b.workspace_id < a.workspace_id)))
                    }
                    _ => {
                        (b.changed_since_viewed && !a.changed_since_viewed)
                            || (b.changed_since_viewed == a.changed_since_viewed
                                && (b.last_viewed_at > a.last_viewed_at
                                    || (b.last_viewed_at == a.last_viewed_at
                                        && (b.last_activity_at > a.last_activity_at
                                            || (b.last_activity_at == a.last_activity_at
                                                && b.workspace_id < a.workspace_id)))))
                    }
                }
            };
            if swap {
                briefs.swap(i, j);
            }
        }
    }
}

fn message_plain_text(m: &Message) -> String {
    let mut parts: Vec<String> = Vec::new();
    for c in m.blocks() {
        if c.r#type != BLOCK_TEXT {
            continue;
        }
        let Some(p) = c.as_text() else {
            continue;
        };
        let mut t = p.text.trim().to_owned();
        if t.is_empty() && !p.display_text.is_empty() {
            t = p.display_text.trim().to_owned();
        }
        if !t.is_empty() {
            parts.push(t);
        }
    }
    parts.join("\n")
}

/// The last few user/assistant text lines of a session, oldest first.
fn conversation_excerpt(st: &Store, session_id: &str) -> Vec<ConversationLine> {
    if session_id.is_empty() {
        return Vec::new();
    }
    let Ok(Some(msgs)) = st.session_transcript(session_id) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for m in msgs.iter().rev() {
        if out.len() >= BRIEF_EXCERPT_MAX {
            break;
        }
        if m.role != "user" && m.role != "assistant" {
            continue;
        }
        let text = message_plain_text(m);
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        out.push(ConversationLine {
            role: m.role.clone(),
            text: truncate(text, BRIEF_LINE_MAX),
            at: m.created_at,
        });
    }
    out.reverse();
    out
}

impl Engine {
    /// The current cross-workspace projection.
    pub fn briefing_snapshot(&self) -> BriefingSnapshot {
        let now = self.now();
        let workspaces = self.store.workspaces();
        let proj_name: HashMap<String, String> = self
            .store
            .projects()
            .unwrap_or_default()
            .into_iter()
            .map(|p| {
                let name = match p.name.trim() {
                    "" => p.id.clone(),
                    n => n.to_owned(),
                };
                (p.id, name)
            })
            .collect();
        let headers = self
            .store
            .session_headers_by_workspace()
            .unwrap_or_default();
        let event_ts = self
            .store
            .latest_event_ts_by_workspace()
            .unwrap_or_default();
        let prs = self.store.prs();
        let mut pr_by_ws: HashMap<&str, &Pr> = HashMap::new();
        let mut pr_by_id: HashMap<&str, &Pr> = HashMap::new();
        for p in &prs {
            pr_by_id.insert(&p.id, p);
            if !p.workspace_id.is_empty() {
                pr_by_ws.insert(&p.workspace_id, p);
            }
        }
        let mut esc: HashMap<String, Escalation> = HashMap::new();
        for ws in &workspaces {
            if let Some(e) = self.store.escalation(&ws.id) {
                esc.insert(ws.id.clone(), e);
            }
        }
        let mut perm_by_ws: HashMap<String, PermissionRequest> = HashMap::new();
        for p in self.pending_permissions() {
            perm_by_ws.entry(p.workspace_id.clone()).or_insert(p);
        }
        let mut q_by_ws: HashMap<String, UserQuestion> = HashMap::new();
        for q in self.pending_ask_users() {
            q_by_ws.entry(q.workspace_id.clone()).or_insert(q);
        }
        let mut briefs = Vec::new();
        let mut seen_pr: Vec<String> = Vec::new();
        for ws in &workspaces {
            if ws.archived {
                continue;
            }
            let hs = headers.get(&ws.id).cloned().unwrap_or_default();
            let mut pr = pr_by_ws.get(ws.id.as_str()).map(|p| (*p).clone());
            let activity = effective_workspace_activity(
                ws,
                &hs,
                event_ts.get(&ws.id).copied().unwrap_or(0),
                pr.as_ref(),
            );
            if ws.is_pr_chat_host()
                && let Some(p) = pr_by_id.get(ws.host_pr_id.as_str())
            {
                pr = Some((*p).clone());
            }
            let inp = BriefInputs {
                ws: ws.clone(),
                project_name: proj_name.get(&ws.project_id).cloned().unwrap_or_default(),
                agents: self.store.agents(&ws.id),
                headers: hs,
                activity,
                now_ms: now.timestamp_millis(),
                perm: perm_by_ws.get(&ws.id).cloned(),
                question: q_by_ws.get(&ws.id).cloned(),
                escalation: esc.get(&ws.id).cloned(),
                pr,
                open_findings: self.store.open_findings_count(&ws.id) as i64,
            };
            let Some(b) = build_workspace_brief(&inp) else {
                continue;
            };
            if !b.pr_id.is_empty() {
                if seen_pr.contains(&b.pr_id) {
                    continue;
                }
                seen_pr.push(b.pr_id.clone());
            }
            briefs.push(b);
        }
        sort_briefs(&mut briefs);
        let mut counts = BriefingCounts::default();
        for b in &briefs {
            match b.bucket.as_str() {
                BRIEF_BUCKET_NEEDS => counts.needs += 1,
                BRIEF_BUCKET_WORKING => counts.working += 1,
                BRIEF_BUCKET_PICKUP => counts.pickup += 1,
                _ => {}
            }
        }
        BriefingSnapshot {
            generated_at: now.timestamp_millis(),
            counts,
            workspaces: (!briefs.is_empty()).then_some(briefs),
        }
    }

    /// Bounded coordinator detail for one live workspace.
    pub fn workspace_context(&self, id: &str) -> Result<WorkspaceContext> {
        let id = id.trim();
        if id.is_empty() {
            return Err(EngineError::new("workspace not found"));
        }
        let ws = self
            .store
            .workspace(id)
            .filter(|w| !w.archived)
            .ok_or_else(|| EngineError(format!("workspace {id:?} not found")))?;
        let found = self
            .briefing_snapshot()
            .workspaces
            .unwrap_or_default()
            .into_iter()
            .find(|b| b.workspace_id == id);
        let brief = match found {
            Some(b) => b,
            None => {
                let mut project_name = ws.project_id.clone();
                if let Some(p) = self.store.project(&ws.project_id)
                    && !p.name.trim().is_empty()
                {
                    project_name = p.name;
                }
                let inp = BriefInputs {
                    ws: ws.clone(),
                    project_name,
                    agents: self.store.agents(&ws.id),
                    now_ms: self.now().timestamp_millis(),
                    open_findings: self.store.open_findings_count(&ws.id) as i64,
                    ..BriefInputs::default()
                };
                let mut b = build_workspace_brief(&inp).unwrap_or_default();
                b.workspace_id = ws.id.clone();
                b.title = workspace_title(&ws);
                b
            }
        };
        let mut ctx = WorkspaceContext {
            brief,
            ..WorkspaceContext::default()
        };
        if let Some(p) = &ws.plan {
            ctx.plan_steps = p.steps.clone();
        }
        let agents = self.store.agents(&ws.id);
        ctx.agents = agents
            .iter()
            .map(|a| AgentSummary {
                code: a.code.clone(),
                name: a.name.clone(),
                status: a.status.clone(),
                task: a.task.clone(),
                unseen: a.unseen,
            })
            .collect();
        if let Some(p) = self
            .pending_permissions()
            .into_iter()
            .find(|p| p.workspace_id == ws.id)
        {
            ctx.open_asks.push(OpenAsk {
                kind: "permission".into(),
                id: p.id,
                agent_code: p.agent_code,
                summary: p.summary,
            });
        }
        if let Some(q) = self
            .pending_ask_users()
            .into_iter()
            .find(|q| q.workspace_id == ws.id)
        {
            ctx.open_asks.push(OpenAsk {
                kind: "question".into(),
                id: q.id,
                agent_code: q.agent_code,
                summary: q.question,
            });
        }
        if let Some(esc) = self.store.escalation(&ws.id)
            && esc.answer.trim().is_empty()
        {
            ctx.open_asks.push(OpenAsk {
                kind: "escalation".into(),
                id: esc.id,
                summary: esc.question,
                ..OpenAsk::default()
            });
        }
        let sessions = self.store.sessions_for_loop(&ws.id).unwrap_or_default();
        for sess in &sessions {
            let cfg = AgentConfig::parse(&sess.config_json);
            if cfg.code.is_empty() {
                continue;
            }
            let unseen = self
                .store
                .agents(&ws.id)
                .iter()
                .find(|a| a.code == cfg.code)
                .is_some_and(|a| a.unseen);
            ctx.recent_chats.push(ChatHeader {
                title: session_title(&cfg),
                code: cfg.code,
                mode: cfg.mode,
                updated_at: sess.updated_at,
                unseen,
            });
            if ctx.recent_chats.len() >= 6 {
                break;
            }
        }
        if let Some(p) = self
            .store
            .prs()
            .into_iter()
            .find(|p| p.workspace_id == ws.id)
        {
            ctx.pr = Some(PrSummary {
                id: p.id,
                number: p.number,
                title: p.title,
                state: p.state,
                review_decision: p.review_decision,
            });
        }
        let latest = &ctx.brief.latest_chat_code;
        let sess = self.store.session_for_loop_agent(&ws.id, latest);
        if sess.is_some() || latest.is_empty() {
            let sess = sess.or_else(|| sessions.last().cloned());
            if let Some(s) = sess {
                ctx.conversation = conversation_excerpt(&self.store, &s.id);
            }
        }
        Ok(ctx)
    }

    /// The shell rendered this workspace through `seen_through_activity_at`:
    /// LastViewedAt is now; SeenActivityAt only advances to what the UI
    /// showed (max, so a racing event stays new).
    pub fn mark_workspace_viewed(&self, workspace_id: &str, seen_through_activity_at: i64) {
        if workspace_id.trim().is_empty() {
            return;
        }
        let now = self.now().timestamp_millis();
        if self
            .store
            .update_workspace(workspace_id, |l| {
                if now > l.last_viewed_at {
                    l.last_viewed_at = now;
                }
                if seen_through_activity_at > l.seen_activity_at {
                    l.seen_activity_at = seen_through_activity_at;
                }
            })
            .is_some()
        {
            self.publish_briefing_invalidated(workspace_id);
        }
    }

    /// Moves Last and LastActivityAt forward monotonically and invalidates
    /// the Briefing. Call only at meaningful boundaries.
    pub fn touch_workspace_activity(&self, workspace_id: &str, summary: &str, at: DateTime<Local>) {
        if workspace_id.trim().is_empty() {
            return;
        }
        let ms = at.timestamp_millis();
        let summary = summary.trim();
        let mut changed = false;
        let ok = self
            .store
            .update_workspace(workspace_id, |l| {
                if ms > l.last_activity_at {
                    l.last_activity_at = ms;
                    changed = true;
                }
                if !summary.is_empty() && summary != l.last {
                    l.last = truncate(summary, 160);
                    changed = true;
                }
            })
            .is_some();
        if ok && changed {
            self.publish_briefing_invalidated(workspace_id);
        }
    }

    pub(crate) fn publish_briefing_invalidated(&self, workspace_id: &str) {
        self.bus
            .publish(EngineEvent::BriefingUpdated(BriefingUpdated {
                workspace_id: workspace_id.to_owned(),
            }));
    }
}

#[cfg(test)]
mod tests;
