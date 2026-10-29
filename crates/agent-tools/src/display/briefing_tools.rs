//! Ask Recur briefing tool cards (`lib/briefingtools.ts`): list_workspace_briefs,
//! get_workspace_context and present_workspace, parsed from their JSON results.

use recur_domain::{BriefAction, WorkspaceBrief};
use serde_json::{Map, Value};

use super::{is_tool, js_space, js_trim, starts_with_ci, str_of};

pub fn is_list_workspace_briefs_tool(tool: Option<&str>) -> bool {
    is_tool(tool, "list_workspace_briefs")
}

pub fn is_get_workspace_context_tool(tool: Option<&str>) -> bool {
    is_tool(tool, "get_workspace_context")
}

pub fn is_present_workspace_tool(tool: Option<&str>) -> bool {
    is_tool(tool, "present_workspace")
}

pub fn is_briefing_tool(tool: Option<&str>) -> bool {
    is_list_workspace_briefs_tool(tool)
        || is_get_workspace_context_tool(tool)
        || is_present_workspace_tool(tool)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BriefingCountsView {
    pub needs: i64,
    pub working: i64,
    pub pickup: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListedWorkspaceBriefs {
    pub counts: BriefingCountsView,
    pub workspaces: Vec<WorkspaceBrief>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanStepRow {
    pub id: String,
    pub title: String,
    pub status: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentRow {
    pub code: String,
    pub name: String,
    pub status: String,
    pub task: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpenAskRow {
    pub kind: String,
    pub id: String,
    pub summary: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatRow {
    pub code: String,
    pub title: String,
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrRow {
    pub id: String,
    pub number: i64,
    pub title: String,
    pub state: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConversationRow {
    pub role: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceContextView {
    pub brief: WorkspaceBrief,
    pub plan_steps: Vec<PlanStepRow>,
    pub agents: Vec<AgentRow>,
    pub open_asks: Vec<OpenAskRow>,
    pub recent_chats: Vec<ChatRow>,
    pub pr: Option<PrRow>,
    pub conversation: Vec<ConversationRow>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PresentWorkspaceView {
    pub ok: bool,
    pub target: String,
    pub kind: String,
    pub brief: Option<WorkspaceBrief>,
    pub error: String,
}

/// A finite number field, else 0. Fractions truncate (Go writes integers).
fn num(v: Option<&Value>) -> i64 {
    v.and_then(Value::as_f64)
        .filter(|f| f.is_finite())
        .map_or(0, |f| f as i64)
}

/// `str(a) || str(b)`.
fn str_or(o: &Map<String, Value>, a: &str, b: &str) -> String {
    let s = str_of(o.get(a));
    if s.is_empty() { str_of(o.get(b)) } else { s }
}

/// `num(a) || num(b)`.
fn num_or(o: &Map<String, Value>, a: &str, b: &str) -> i64 {
    match num(o.get(a)) {
        0 => num(o.get(b)),
        n => n,
    }
}

/// The JSON object starting at the first `{`, if the rest parses as one.
fn parse_json_object(raw: &str) -> Option<Map<String, Value>> {
    let t = js_trim(raw);
    let i = t.find('{')?;
    match serde_json::from_str(&t[i..]) {
        Ok(Value::Object(o)) => Some(o),
        _ => None,
    }
}

/// JavaScript truthiness of a JSON value.
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

fn parse_action(raw: Option<&Value>) -> BriefAction {
    let Some(Value::Object(o)) = raw else {
        return BriefAction::default();
    };
    BriefAction {
        label: str_of(o.get("label")),
        target: str_of(o.get("target")),
        kind: str_of(o.get("kind")),
    }
}

/// One brief, accepting camelCase or snake_case keys. `None` when it has
/// neither an id nor a title.
pub fn parse_workspace_brief(raw: Option<&Value>) -> Option<WorkspaceBrief> {
    let Some(Value::Object(o)) = raw else {
        return None;
    };
    let workspace_id = str_or(o, "workspaceId", "workspace_id");
    let title = str_of(o.get("title"));
    if workspace_id.is_empty() && title.is_empty() {
        return None;
    }
    let reasons = match o.get("reasons") {
        Some(Value::Array(r)) => r
            .iter()
            .filter_map(Value::as_str)
            .filter(|s| !js_trim(s).is_empty())
            .map(String::from)
            .collect(),
        _ => Vec::new(),
    };
    let secondary = match o.get("secondary") {
        Some(Value::Array(s)) => s
            .iter()
            .map(|a| parse_action(Some(a)))
            .filter(|a| !a.target.is_empty() && !a.label.is_empty())
            .collect(),
        _ => Vec::new(),
    };
    let changed = |k: &str| o.get(k) == Some(&Value::Bool(true));
    Some(WorkspaceBrief {
        title: if title.is_empty() {
            workspace_id.clone()
        } else {
            title
        },
        workspace_id,
        project_id: str_or(o, "projectId", "project_id"),
        project_name: str_or(o, "projectName", "project_name"),
        branch: str_of(o.get("branch")),
        bucket: str_of(o.get("bucket")),
        reasons: Some(reasons),
        now: str_of(o.get("now")),
        next: str_of(o.get("next")),
        quiet_age: str_or(o, "quietAge", "quiet_age"),
        plan_summary: str_or(o, "planSummary", "plan_summary"),
        plan_done: num_or(o, "planDone", "plan_done"),
        plan_total: num_or(o, "planTotal", "plan_total"),
        active_agents: num_or(o, "activeAgents", "active_agents"),
        latest_chat_title: str_or(o, "latestChatTitle", "latest_chat_title"),
        diff_add: num_or(o, "diffAdd", "diff_add"),
        diff_del: num_or(o, "diffDel", "diff_del"),
        pr_number: num_or(o, "prNumber", "pr_number"),
        changed_since_viewed: changed("changedSinceViewed") || changed("changed_since_viewed"),
        primary: parse_action(o.get("primary")),
        secondary,
        ..Default::default()
    })
}

/// The list_workspace_briefs envelope; `None` without a `workspaces` array.
pub fn parse_list_workspace_briefs(result: &str) -> Option<ListedWorkspaceBriefs> {
    let o = parse_json_object(result)?;
    let Some(Value::Array(rows)) = o.get("workspaces") else {
        return None;
    };
    let c = match o.get("counts") {
        Some(Value::Object(c)) => c.clone(),
        _ => Map::new(),
    };
    Some(ListedWorkspaceBriefs {
        counts: BriefingCountsView {
            needs: num(c.get("needs")),
            working: num(c.get("working")),
            pickup: num(c.get("pickup")),
        },
        workspaces: rows
            .iter()
            .filter_map(|r| parse_workspace_brief(Some(r)))
            .collect(),
    })
}

fn array<'a>(o: &'a Map<String, Value>, a: &str, b: &str) -> &'a [Value] {
    match (o.get(a), o.get(b)) {
        (Some(Value::Array(v)), _) | (_, Some(Value::Array(v))) => v,
        _ => &[],
    }
}

fn opt(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

/// The get_workspace_context result: the nested brief (or the top level as a
/// brief) and the bounded extras.
pub fn parse_workspace_context(result: &str) -> Option<WorkspaceContextView> {
    let o = parse_json_object(result)?;
    let top = Value::Object(o.clone());
    let brief =
        parse_workspace_brief(o.get("brief")).or_else(|| parse_workspace_brief(Some(&top)))?;
    let objects = |a: &str, b: &str| -> Vec<Map<String, Value>> {
        array(&o, a, b)
            .iter()
            .filter_map(|v| v.as_object().cloned())
            .collect()
    };
    let pr = match o.get("pr") {
        Some(Value::Object(p)) if !str_of(p.get("id")).is_empty() || num(p.get("number")) != 0 => {
            Some(PrRow {
                id: str_of(p.get("id")),
                number: num(p.get("number")),
                title: str_of(p.get("title")),
                state: opt(str_of(p.get("state"))),
            })
        }
        _ => None,
    };
    Some(WorkspaceContextView {
        brief,
        plan_steps: objects("planSteps", "plan_steps")
            .iter()
            .filter(|r| !str_of(r.get("title")).is_empty())
            .map(|r| PlanStepRow {
                id: str_of(r.get("id")),
                title: str_of(r.get("title")),
                status: str_of(r.get("status")),
                note: opt(str_of(r.get("note"))),
            })
            .collect(),
        agents: objects("agents", "agents")
            .iter()
            .filter(|r| !str_of(r.get("code")).is_empty())
            .map(|r| {
                let code = str_of(r.get("code"));
                let name = str_of(r.get("name"));
                AgentRow {
                    name: if name.is_empty() { code.clone() } else { name },
                    code,
                    status: str_of(r.get("status")),
                    task: opt(str_of(r.get("task"))),
                }
            })
            .collect(),
        open_asks: objects("openAsks", "open_asks")
            .iter()
            .filter(|r| !str_of(r.get("summary")).is_empty())
            .map(|r| OpenAskRow {
                kind: str_of(r.get("kind")),
                id: str_of(r.get("id")),
                summary: str_of(r.get("summary")),
            })
            .collect(),
        recent_chats: objects("recentChats", "recent_chats")
            .iter()
            .filter(|r| !str_of(r.get("code")).is_empty())
            .map(|r| {
                let code = str_of(r.get("code"));
                let title = str_of(r.get("title"));
                ChatRow {
                    title: if title.is_empty() {
                        code.clone()
                    } else {
                        title
                    },
                    code,
                    mode: opt(str_of(r.get("mode"))),
                }
            })
            .collect(),
        pr,
        conversation: objects("conversation", "conversation")
            .iter()
            .filter(|r| !str_of(r.get("text")).is_empty())
            .map(|r| {
                let role = str_of(r.get("role"));
                ConversationRow {
                    role: if role.is_empty() {
                        "assistant".into()
                    } else {
                        role
                    },
                    text: str_of(r.get("text")),
                }
            })
            .collect(),
    })
}

/// The present_workspace target from its args.
pub fn parse_present_workspace_target(raw: &Value) -> String {
    str_of(raw.get("target"))
}

/// `/^<word>\s+(\S+)\.?$/i`, the capture with one trailing '.' removed.
fn legacy_target(r: &str, word: &str) -> Option<String> {
    if !starts_with_ci(r, word) {
        return None;
    }
    let rest = &r[word.len()..];
    let target = rest.trim_start_matches(js_space);
    if target.len() == rest.len() || target.is_empty() || target.chars().any(js_space) {
        return None;
    }
    Some(target.strip_suffix('.').unwrap_or(target).to_string())
}

/// The present_workspace card: the JSON payload, or the legacy
/// "Opening <target>." / "Presented <target>." strings, or a refusal.
pub fn parse_present_workspace(result: &str, input_target: &str) -> PresentWorkspaceView {
    let r = js_trim(result);
    let failed = |error: &str| PresentWorkspaceView {
        ok: false,
        target: input_target.into(),
        kind: target_kind(input_target),
        brief: None,
        error: error.into(),
    };
    if let Some(o) = parse_json_object(r)
        && (o.get("ok").is_some_and(Value::is_boolean)
            || truthy(o.get("target"))
            || truthy(o.get("brief"))
            || truthy(o.get("error")))
    {
        let target = match str_of(o.get("target")) {
            t if t.is_empty() => input_target.to_string(),
            t => t,
        };
        let brief = parse_workspace_brief(o.get("brief"));
        let error = str_of(o.get("error"));
        let ok = o.get("ok") == Some(&Value::Bool(true))
            || (error.is_empty() && (!target.is_empty() || brief.is_some()));
        let kind = match str_of(o.get("kind")) {
            k if k.is_empty() => target_kind(&target),
            k => k,
        };
        let error = match (error.is_empty(), ok) {
            (false, _) => error,
            (true, true) => String::new(),
            (true, false) => r.into(),
        };
        return PresentWorkspaceView {
            ok: ok && error.is_empty(),
            target,
            kind,
            brief,
            error,
        };
    }
    for word in ["Opening", "Presented"] {
        if let Some(target) = legacy_target(r, word) {
            return PresentWorkspaceView {
                ok: true,
                kind: target_kind(&target),
                target,
                brief: None,
                error: String::new(),
            };
        }
    }
    if starts_with_ci(r, "Refused:") {
        return failed(r);
    }
    if r.is_empty() {
        return failed("");
    }
    failed(r)
}

/// The kind segment of a target (`loop` reads as `workspace`).
pub fn target_kind(target: &str) -> String {
    let k = js_trim(target.split(':').next().unwrap_or(""));
    if k == "loop" {
        "workspace".into()
    } else {
        k.into()
    }
}

/// The workspace id in a present_workspace target; empty for pr/question.
pub fn workspace_id_from_present_target(target: &str) -> String {
    let t = js_trim(target);
    let Some((kind, rest)) = t.split_once(':') else {
        return t.into();
    };
    let rest = js_trim(rest);
    match kind {
        "workspace" | "loop" | "chat" => js_trim(rest.split(':').next().unwrap_or("")).into(),
        _ => String::new(),
    }
}

/// A workspace as the hydrator needs it.
#[derive(Debug, Clone, Default)]
pub struct HydrateWorkspace {
    pub id: String,
    pub short: String,
    pub goal: String,
    pub branch: String,
    pub project_id: String,
}

#[derive(Debug, Clone, Default)]
pub struct HydrateProject {
    pub id: String,
    pub name: String,
}

/// Fills a missing brief from the live overview or workspace list so the pin
/// is a real card.
pub fn hydrate_present_brief(
    card: &PresentWorkspaceView,
    overview: &[WorkspaceBrief],
    workspaces: &[HydrateWorkspace],
    projects: &[HydrateProject],
) -> PresentWorkspaceView {
    if card.brief.is_some() {
        return card.clone();
    }
    let primary = BriefAction {
        label: "Open".into(),
        target: card.target.clone(),
        kind: if card.kind.is_empty() {
            "open".into()
        } else {
            card.kind.clone()
        },
    };
    let id = workspace_id_from_present_target(&card.target);
    if !id.is_empty() {
        if let Some(b) = overview.iter().find(|w| w.workspace_id == id) {
            return PresentWorkspaceView {
                brief: Some(b.clone()),
                ..card.clone()
            };
        }
        if let Some(ws) = workspaces.iter().find(|w| w.id == id) {
            let first = [&ws.short, &ws.goal, &ws.id]
                .into_iter()
                .find(|s| !s.is_empty())
                .map_or("", |s| s.as_str());
            let title = match js_trim(first) {
                "" => id.clone(),
                t => t.to_string(),
            };
            let project_name = projects
                .iter()
                .find(|p| !ws.project_id.is_empty() && p.id == ws.project_id)
                .map(|p| p.name.clone())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| ws.project_id.clone());
            return PresentWorkspaceView {
                brief: Some(WorkspaceBrief {
                    workspace_id: ws.id.clone(),
                    project_id: ws.project_id.clone(),
                    project_name,
                    title,
                    branch: ws.branch.clone(),
                    reasons: Some(Vec::new()),
                    primary,
                    ..Default::default()
                }),
                ..card.clone()
            };
        }
    }
    if card.target.is_empty() {
        return card.clone();
    }
    PresentWorkspaceView {
        brief: Some(WorkspaceBrief {
            title: if id.is_empty() {
                card.target.clone()
            } else {
                id.clone()
            },
            workspace_id: id,
            reasons: Some(Vec::new()),
            primary,
            ..Default::default()
        }),
        ..card.clone()
    }
}

pub fn briefing_counts_meta(counts: &BriefingCountsView) -> String {
    let mut parts = Vec::new();
    if counts.needs != 0 {
        parts.push(format!("{} need you", counts.needs));
    }
    if counts.working != 0 {
        parts.push(format!("{} working", counts.working));
    }
    if counts.pickup != 0 {
        parts.push(format!("{} pick up", counts.pickup));
    }
    parts.join(" · ")
}

pub fn list_workspace_briefs_label(
    pending: bool,
    listed: Option<&ListedWorkspaceBriefs>,
) -> String {
    if pending {
        return "Listing workspaces".into();
    }
    let Some(listed) = listed else {
        return "Listed workspaces".into();
    };
    match listed.workspaces.len() {
        0 => "Listed workspaces · none".into(),
        1 => "Listed 1 workspace".into(),
        n => format!("Listed {n} workspaces"),
    }
}

pub fn get_workspace_context_label(pending: bool, title: &str, failed: bool) -> String {
    if failed {
        return "Workspace lookup failed".into();
    }
    match (pending, title.is_empty()) {
        (true, false) => format!("Looking up {title}"),
        (true, true) => "Looking up workspace".into(),
        (false, false) => format!("Looked up {title}"),
        (false, true) => "Looked up workspace".into(),
    }
}

pub fn list_workspace_briefs_summary(listed: &ListedWorkspaceBriefs) -> String {
    match listed.workspaces.as_slice() {
        [] => "Nothing in flight.".into(),
        [b] => {
            let detail = if b.now.is_empty() { &b.next } else { &b.now };
            [b.title.as_str(), detail.as_str()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" — ")
        }
        many => match briefing_counts_meta(&listed.counts) {
            m if m.is_empty() => format!("{} workspaces", many.len()),
            m => m,
        },
    }
}

pub fn workspace_context_summary(ctx: &WorkspaceContextView) -> String {
    let b = &ctx.brief;
    [&b.now, &b.next, &b.title]
        .into_iter()
        .find(|s| !s.is_empty())
        .cloned()
        .unwrap_or_default()
}

pub fn present_workspace_label(pending: bool, card: Option<&PresentWorkspaceView>) -> String {
    let title = card
        .and_then(|c| c.brief.as_ref())
        .map(|b| b.title.as_str())
        .filter(|t| !t.is_empty());
    let target = card.map(|c| c.target.as_str()).filter(|t| !t.is_empty());
    if pending {
        return match title.or(target) {
            Some(t) => format!("Presenting {t}"),
            None => "Presenting workspace".into(),
        };
    }
    if card.is_some_and(|c| !c.error.is_empty() || !c.ok) {
        return "Present workspace failed".into();
    }
    if let Some(t) = title {
        return format!("Presented {t}");
    }
    if let Some(t) = target {
        return format!("Presented {t}");
    }
    "Presented workspace".into()
}

// Port of briefingtools.test.ts.
#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn brief() -> Value {
        json!({
            "workspaceId": "recur.zesty-quiche",
            "projectName": "recur",
            "title": "Account linking architecture",
            "branch": "mycodecrafting/multi-account-linking",
            "bucket": "needs",
            "reasons": ["plan_approval", "unfinished_plan", "recent_chat"],
            "now": "Plan is ready but not approved",
            "next": "Review and approve the plan",
            "primary": {"label": "Review plan", "target": "workspace:recur.zesty-quiche"},
        })
    }

    fn with(base: Value, patch: Value) -> Value {
        let (Value::Object(mut a), Value::Object(b)) = (base, patch) else {
            unreachable!()
        };
        a.extend(b);
        Value::Object(a)
    }

    #[test]
    fn detectors_match_recur_mcp_and_bare_names() {
        assert!(is_list_workspace_briefs_tool(Some(
            "mcp__recur__list_workspace_briefs"
        )));
        assert!(is_get_workspace_context_tool(Some(
            "mcp__recur__get_workspace_context"
        )));
        assert!(is_present_workspace_tool(Some(
            "mcp__recur__present_workspace"
        )));
        assert!(is_present_workspace_tool(Some("present_workspace")));
        assert!(!is_list_workspace_briefs_tool(Some(
            "mcp__recur__list_findings"
        )));
        assert!(is_briefing_tool(Some("get_workspace_context")));
    }

    fn two() -> String {
        json!({
            "counts": {"needs": 1, "working": 0, "pickup": 2},
            "workspaces": [brief(), with(brief(), json!({"workspaceId": "w2", "title": "Fix build", "bucket": "pickup"}))],
        })
        .to_string()
    }

    #[test]
    fn reads_counts_and_cards_without_dumping_json() {
        let listed = parse_list_workspace_briefs(&two()).unwrap();
        assert_eq!(listed.counts.needs, 1);
        assert_eq!(listed.workspaces.len(), 2);
        assert_eq!(listed.workspaces[0].title, "Account linking architecture");
        assert_eq!(
            list_workspace_briefs_label(false, Some(&listed)),
            "Listed 2 workspaces"
        );
        assert_eq!(
            list_workspace_briefs_label(true, Some(&listed)),
            "Listing workspaces"
        );
        assert_eq!(
            briefing_counts_meta(&listed.counts),
            "1 need you · 2 pick up"
        );
    }

    #[test]
    fn rejects_a_non_envelope() {
        assert!(parse_list_workspace_briefs(r#"{"error":"nope"}"#).is_none());
        assert!(parse_list_workspace_briefs("Refused: no").is_none());
    }

    #[test]
    fn lifts_the_nested_brief_and_extras() {
        let ctx = parse_workspace_context(
            &json!({
                "brief": brief(),
                "planSteps": [{"id": "S1", "title": "Model identities", "status": "done"}],
                "openAsks": [{"kind": "question", "id": "q1", "summary": "Burst limit?"}],
                "recentChats": [{"code": "PA", "title": "Multi-account linking model"}],
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(ctx.brief.title, "Account linking architecture");
        assert_eq!(ctx.plan_steps[0].title, "Model identities");
        assert_eq!(ctx.open_asks[0].summary, "Burst limit?");
        let label = get_workspace_context_label(false, &ctx.brief.title, false);
        assert_eq!(label, "Looked up Account linking architecture");
    }

    #[test]
    fn reads_the_json_card_payload() {
        let card = parse_present_workspace(
            &json!({"ok": true, "target": "workspace:recur.zesty-quiche", "kind": "workspace", "brief": brief()})
                .to_string(),
            "",
        );
        assert!(card.ok);
        assert_eq!(
            card.brief.as_ref().unwrap().title,
            "Account linking architecture"
        );
        assert_eq!(
            present_workspace_label(false, Some(&card)),
            "Presented Account linking architecture"
        );
        assert_eq!(
            present_workspace_label(true, Some(&card)),
            "Presenting Account linking architecture"
        );
    }

    #[test]
    fn understands_legacy_opening_and_refusal() {
        let legacy = parse_present_workspace("Opening workspace:recur.zesty-quiche.", "");
        assert!(legacy.ok);
        assert_eq!(legacy.target, "workspace:recur.zesty-quiche");
        let refused = parse_present_workspace(
            "Refused: that workspace is not available",
            "workspace:missing",
        );
        assert!(!refused.ok);
        assert_eq!(
            present_workspace_label(false, Some(&refused)),
            "Present workspace failed"
        );
    }

    #[test]
    fn reads_the_target_from_args() {
        let t = parse_present_workspace_target(&json!({"target": "workspace:recur.zesty-quiche"}));
        assert_eq!(t, "workspace:recur.zesty-quiche");
    }

    #[test]
    fn hydrates_a_missing_brief_from_the_overview() {
        let card = parse_present_workspace("Opening workspace:recur.zesty-quiche.", "");
        assert!(card.brief.is_none());
        assert_eq!(
            workspace_id_from_present_target(&card.target),
            "recur.zesty-quiche"
        );
        assert_eq!(
            workspace_id_from_present_target("workspace:recur.zesty-quiche:plan"),
            "recur.zesty-quiche"
        );
        let overview = vec![parse_workspace_brief(Some(&brief())).unwrap()];
        let hydrated = hydrate_present_brief(&card, &overview, &[], &[]);
        assert_eq!(
            hydrated.brief.as_ref().unwrap().title,
            "Account linking architecture"
        );
        assert_eq!(
            present_workspace_label(false, Some(&hydrated)),
            "Presented Account linking architecture"
        );
    }

    #[test]
    fn hydrates_from_the_workspace_list() {
        let card = parse_present_workspace("Opening workspace:W1.", "");
        let hydrated = hydrate_present_brief(
            &card,
            &[],
            &[HydrateWorkspace {
                id: "W1".into(),
                short: "Auth rewrite".into(),
                branch: "feat/auth".into(),
                project_id: "storefront".into(),
                ..Default::default()
            }],
            &[HydrateProject {
                id: "storefront".into(),
                name: "storefront".into(),
            }],
        );
        let b = hydrated.brief.unwrap();
        assert_eq!(
            (b.title.as_str(), b.project_name.as_str(), b.branch.as_str()),
            ("Auth rewrite", "storefront", "feat/auth")
        );
    }

    #[test]
    fn collapses_a_list_to_a_titled_line() {
        let listed = parse_list_workspace_briefs(&two()).unwrap();
        assert_eq!(
            list_workspace_briefs_summary(&listed),
            "1 need you · 2 pick up"
        );
        assert_eq!(
            list_workspace_briefs_summary(&ListedWorkspaceBriefs::default()),
            "Nothing in flight."
        );
        let one = ListedWorkspaceBriefs {
            counts: BriefingCountsView {
                needs: 1,
                ..Default::default()
            },
            workspaces: vec![parse_workspace_brief(Some(&brief())).unwrap()],
        };
        assert_eq!(
            list_workspace_briefs_summary(&one),
            "Account linking architecture — Plan is ready but not approved"
        );
    }

    #[test]
    fn collapses_context_to_now_copy() {
        let ctx = parse_workspace_context(&json!({"brief": brief()}).to_string()).unwrap();
        assert_eq!(
            workspace_context_summary(&ctx),
            "Plan is ready but not approved"
        );
    }
}
