//! Display rules for Linear MCP tool cards in chat (`lib/lineartools.ts`):
//! friendly labels and parsed result rows instead of JSON dumps.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};

/// The interesting arguments of a Linear tool call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinearArgs {
    pub query: String,
    pub team: String,
    pub id: String,
    pub issue: String,
    pub name: String,
    pub title: String,
    pub project: String,
    pub user: String,
    pub url_or_id: String,
}

/// One row of a Linear result card.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinearItem {
    pub title: String,
    pub identifier: String,
    pub url: String,
    pub summary: String,
    pub body: String,
    pub color: String,
    pub chips: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinearViewKind {
    List,
    Item,
}

/// A parsed Linear tool result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinearView {
    pub kind: LinearViewKind,
    /// What the list holds (`projects`, `release notes`), or empty.
    pub noun: String,
    pub items: Vec<LinearItem>,
    pub has_next_page: bool,
}

const COLLECTION_KEYS: [&str; 23] = [
    "issues",
    "projects",
    "teams",
    "labels",
    "users",
    "comments",
    "cycles",
    "documents",
    "initiatives",
    "milestones",
    "notifications",
    "templates",
    "diffs",
    "releases",
    "releaseNotes",
    "release_notes",
    "releasePipelines",
    "release_pipelines",
    "agentSkills",
    "agent_skills",
    "statusUpdates",
    "status_updates",
    "attachments",
];

const ENTITY_KEYS: [&str; 15] = [
    "issue",
    "project",
    "team",
    "comment",
    "document",
    "initiative",
    "milestone",
    "user",
    "cycle",
    "release",
    "attachment",
    "template",
    "diff",
    "notification",
    "label",
];

fn verbs(verb: &str) -> Option<(&'static str, &'static str)> {
    Some(match verb {
        "list" => ("Listing", "Listed"),
        "get" => ("Looking up", "Looked up"),
        "save" => ("Saving", "Saved"),
        "delete" => ("Deleting", "Deleted"),
        "create" => ("Creating", "Created"),
        "update" => ("Updating", "Updated"),
        "search" => ("Searching", "Searched"),
        "mark" => ("Marking", "Marked"),
        "share" => ("Sharing", "Shared"),
        "unshare" => ("Unsharing", "Unshared"),
        "merge" => ("Merging", "Merged"),
        "submit" => ("Submitting", "Submitted"),
        "resolve" => ("Resolving", "Resolved"),
        "retire" => ("Retiring", "Retired"),
        "restore" => ("Restoring", "Restored"),
        "extract" => ("Extracting", "Extracted"),
        "prepare" => ("Preparing", "Prepared"),
        _ => return None,
    })
}

fn re(p: &str) -> Regex {
    Regex::new(p).expect("lineartools pattern")
}

static ISSUE_ID: LazyLock<Regex> = LazyLock::new(|| re(r"^[A-Z][A-Z0-9]+-[0-9]+$"));
static UUID: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"));
static HEX_COLOR: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^#([0-9a-f]{3}|[0-9a-f]{6}|[0-9a-f]{8})$"));
static TRUNCATED: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)\(truncated\b|use `get_[a-z_]+` for full"));

fn as_object(v: &Value) -> Option<&Map<String, Value>> {
    v.as_object()
}

fn str_of(v: Option<&Value>) -> String {
    v.and_then(Value::as_str)
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

fn get(o: &Map<String, Value>, k: &str) -> String {
    str_of(o.get(k))
}

fn first(parts: &[String]) -> String {
    parts
        .iter()
        .find(|s| !s.is_empty())
        .cloned()
        .unwrap_or_default()
}

fn named(v: Option<&Value>) -> String {
    let s = str_of(v);
    if !s.is_empty() {
        return s;
    }
    let Some(o) = v.and_then(as_object) else {
        return String::new();
    };
    first(&[
        get(o, "name"),
        get(o, "key"),
        get(o, "displayName"),
        get(o, "title"),
        get(o, "email"),
    ])
}

fn is_truncated_blob(s: &str) -> bool {
    TRUNCATED.is_match(s)
}

/// Whitespace collapsed, capped at `max` characters with an ellipsis.
fn first_line(s: &str, max: usize) -> String {
    let t = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() <= max {
        return t;
    }
    format!("{}…", t.chars().take(max - 1).collect::<String>())
}

/// `releaseNotes` / `release_notes` → `release notes`.
fn human_key(k: &str) -> String {
    let mut out = String::new();
    let mut prev_lower = false;
    for c in k.chars() {
        if c == '.' || c == '_' {
            out.push(' ');
            prev_lower = false;
            continue;
        }
        if c.is_ascii_uppercase() && prev_lower {
            out.push(' ');
        }
        prev_lower = c.is_ascii_lowercase();
        out.push(c);
    }
    out.to_lowercase()
}

fn singular(noun: &str) -> String {
    if let Some(stem) = noun.strip_suffix("ies") {
        return format!("{stem}y");
    }
    if let Some(stem) = noun.strip_suffix("ses") {
        return format!("{stem}s");
    }
    if noun.ends_with('s') && !noun.ends_with("ss") {
        return noun[..noun.len() - 1].to_owned();
    }
    noun.to_owned()
}

fn linear_server(tool: &str) -> String {
    let Some(rest) = tool.strip_prefix("mcp__") else {
        return String::new();
    };
    match rest.rfind("__") {
        Some(i) if i > 0 => rest[..i].to_lowercase(),
        _ => String::new(),
    }
}

/// Recur's `mcp__linear__*` tools and plugin servers whose id contains
/// `linear`.
pub fn is_linear_tool(tool: &str) -> bool {
    let t = tool.trim();
    t.starts_with("mcp__") && linear_server(t).contains("linear")
}

/// The bare tool name (`list_projects`).
pub fn linear_tool_name(tool: &str) -> &str {
    match tool.rsplit("__").next() {
        Some(n) if !n.is_empty() => n,
        _ => tool,
    }
}

fn parse_json_value(raw: &str) -> Option<Value> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    let parse = |s: &str| {
        serde_json::from_str::<Value>(s)
            .ok()
            .filter(|v| !v.is_null())
    };
    if let Some(v) = parse(t) {
        return Some(v);
    }
    let i = t.find(['{', '['])?;
    parse(&t[i..])
}

pub fn parse_linear_args(raw: &Value) -> LinearArgs {
    let empty = Map::new();
    let o = as_object(raw).unwrap_or(&empty);
    LinearArgs {
        query: get(o, "query"),
        team: first(&[get(o, "team"), get(o, "teamId")]),
        id: get(o, "id"),
        issue: first(&[get(o, "issue"), get(o, "issueId")]),
        name: get(o, "name"),
        title: get(o, "title"),
        project: first(&[get(o, "project"), get(o, "projectId")]),
        user: get(o, "user"),
        url_or_id: first(&[get(o, "urlOrId"), get(o, "url_or_id")]),
    }
}

/// What the call is about: query, id, issue, name, title, url, project,
/// team or user (first set).
pub fn linear_subject(a: &LinearArgs) -> String {
    first(&[
        a.query.clone(),
        a.id.clone(),
        a.issue.clone(),
        a.name.clone(),
        a.title.clone(),
        a.url_or_id.clone(),
        a.project.clone(),
        a.team.clone(),
        a.user.clone(),
    ])
}

/// Markdown an argument carries (comment or issue body).
pub fn linear_input_body(raw: &Value) -> String {
    let Some(o) = as_object(raw) else {
        return String::new();
    };
    first(&[
        get(o, "body"),
        get(o, "description"),
        get(o, "content"),
        get(o, "markdown"),
    ])
}

fn parse_item(raw: &Value) -> Option<LinearItem> {
    let o = as_object(raw)?;
    let id = first(&[get(o, "identifier"), get(o, "id")]);
    let identifier = if ISSUE_ID.is_match(&id) {
        id.clone()
    } else if ISSUE_ID.is_match(&get(o, "identifier")) {
        get(o, "identifier")
    } else {
        String::new()
    };
    let title = first(&[get(o, "name"), get(o, "title"), identifier.clone()]);
    let url = get(o, "url");
    let body = first(&[
        get(o, "description"),
        get(o, "body"),
        get(o, "content"),
        get(o, "markdown"),
    ]);
    let mut summary = first(&[get(o, "summary"), get(o, "subtitle")]);
    if summary.is_empty() && !body.is_empty() && !is_truncated_blob(&body) {
        summary = first_line(&body, 88);
    } else if !summary.is_empty() {
        summary = first_line(&summary, 88);
    }
    if is_truncated_blob(&summary) {
        summary.clear();
    }
    let color = get(o, "color");
    let color = if HEX_COLOR.is_match(&color) {
        color
    } else {
        String::new()
    };
    let mut chips: Vec<String> = Vec::new();
    let mut push = |v: String| {
        if !v.is_empty() && v != title && v != identifier && !chips.contains(&v) {
            chips.push(v);
        }
    };
    push(named(o.get("status")));
    push(named(o.get("priority")));
    push(first(&[
        named(o.get("assignee")),
        named(o.get("lead")),
        named(o.get("owner")),
        named(o.get("author")),
        named(o.get("creator")),
    ]));
    push(first(&[named(o.get("team")), named(o.get("leadTeam"))]));
    push(get(o, "visibility"));
    push(first(&[get(o, "state"), named(o.get("state"))]));
    if title.is_empty() && url.is_empty() && body.is_empty() && chips.is_empty() && color.is_empty()
    {
        return None;
    }
    if title.is_empty() && UUID.is_match(&id) && url.is_empty() && body.is_empty() {
        return None;
    }
    let title = if !title.is_empty() {
        title
    } else if !url.is_empty() {
        "Linear".to_owned()
    } else {
        first(&[
            first_line(&body, 88),
            identifier.clone(),
            "Untitled".to_owned(),
        ])
    };
    Some(LinearItem {
        title,
        identifier,
        url,
        summary,
        body: if is_truncated_blob(&body) {
            String::new()
        } else {
            body
        },
        color,
        chips,
    })
}

fn item_view(item: LinearItem, noun: String) -> LinearView {
    LinearView {
        kind: LinearViewKind::Item,
        noun,
        items: vec![item],
        has_next_page: false,
    }
}

/// A tool result as card rows, or `None` when it is not an entity or list.
pub fn parse_linear_result(result: &str) -> Option<LinearView> {
    let v = parse_json_value(result)?;
    if let Some(arr) = v.as_array() {
        let items: Vec<LinearItem> = arr.iter().filter_map(parse_item).collect();
        if items.is_empty() {
            return None;
        }
        let kind = if items.len() == 1 {
            LinearViewKind::Item
        } else {
            LinearViewKind::List
        };
        return Some(LinearView {
            kind,
            noun: "results".into(),
            items,
            has_next_page: false,
        });
    }
    let o = as_object(&v)?;
    let has_next_page = o.get("hasNextPage") == Some(&Value::Bool(true))
        || o.get("pageInfo").and_then(|p| p.get("hasNextPage")) == Some(&Value::Bool(true));
    let entity = [
        "identifier",
        "title",
        "name",
        "url",
        "description",
        "body",
        "content",
        "markdown",
    ]
    .iter()
    .any(|k| !get(o, k).is_empty());
    if entity && let Some(item) = parse_item(&v) {
        return Some(item_view(item, String::new()));
    }
    for key in COLLECTION_KEYS {
        let Some(arr) = o.get(key).and_then(Value::as_array) else {
            continue;
        };
        return Some(LinearView {
            kind: LinearViewKind::List,
            noun: human_key(key),
            items: arr.iter().filter_map(parse_item).collect(),
            has_next_page,
        });
    }
    for key in ENTITY_KEYS {
        if let Some(item) = o.get(key).and_then(parse_item) {
            return Some(item_view(item, human_key(key)));
        }
    }
    parse_item(&v).map(|item| item_view(item, String::new()))
}

fn split_verb(name: &str) -> (&str, String) {
    match name.find('_') {
        Some(i) if i > 0 => (&name[..i], name[i + 1..].replace('_', " ")),
        _ => (name, String::new()),
    }
}

/// Card label: tense plus subject (`Listing projects`, `Looked up ENG-1`).
pub fn linear_tool_label(
    tool: &str,
    pending: bool,
    failed: bool,
    args: &LinearArgs,
    view: Option<&LinearView>,
) -> String {
    let name = linear_tool_name(tool);
    let (verb, rest) = split_verb(name);
    if failed {
        let noun = name.replace('_', " ");
        let mut chars = noun.chars();
        return match chars.next() {
            Some(c) => format!("{}{} failed", c.to_uppercase(), chars.as_str()),
            None => "Linear tool failed".to_owned(),
        };
    }
    let mut subject = linear_subject(args);
    if subject.is_empty()
        && let Some(v) = view.filter(|v| v.kind == LinearViewKind::Item)
        && let Some(item) = v.items.first()
    {
        subject = first(&[item.identifier.clone(), item.title.clone()]);
    }
    let Some((doing, done)) = verbs(verb) else {
        return name.replace('_', " ");
    };
    let word = if pending { doing } else { done };
    let with_rest = || {
        if rest.is_empty() {
            word.to_owned()
        } else {
            format!("{word} {rest}")
        }
    };
    match verb {
        "list" => with_rest(),
        "get" | "search" if !subject.is_empty() => format!("{word} {subject}"),
        "save" | "update" | "create" | "delete" if !subject.is_empty() && rest.is_empty() => {
            format!("{word} {subject}")
        }
        _ => with_rest(),
    }
}

/// Card meta line: the query (quoted), team or subject, then a count or the
/// first chips; nothing the label already says.
pub fn linear_tool_meta(args: &LinearArgs, view: Option<&LinearView>, label: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    let subject = linear_subject(args);
    if !args.query.is_empty() {
        parts.push(format!("\"{}\"", args.query));
    } else if !args.team.is_empty() && !label.contains(&args.team) {
        parts.push(args.team.clone());
    } else if !subject.is_empty() && !label.contains(&subject) {
        parts.push(subject);
    }
    match view {
        Some(v) if v.kind == LinearViewKind::List => {
            parts.push(if v.items.is_empty() {
                "none".to_owned()
            } else {
                v.items.len().to_string()
            });
        }
        Some(v) => {
            for c in v
                .items
                .first()
                .map(|i| i.chips.iter().take(2))
                .into_iter()
                .flatten()
            {
                if !label.contains(c.as_str()) && !parts.contains(c) {
                    parts.push(c.clone());
                }
            }
        }
        None => {}
    }
    parts.join(" · ")
}

/// `1 project`, `3 projects`, `2 results`.
pub fn linear_count_label(v: &LinearView) -> String {
    let noun = if v.noun.is_empty() { "result" } else { &v.noun };
    match v.items.len() {
        1 => format!("1 {}", singular(noun)),
        n => format!("{n} {noun}"),
    }
}

/// The single item's URL, or empty.
pub fn linear_item_url(v: Option<&LinearView>) -> String {
    match v {
        Some(v) if v.kind == LinearViewKind::Item => {
            v.items.first().map(|i| i.url.clone()).unwrap_or_default()
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn apollo() -> Value {
        json!({
            "id": "712025ee-a70e-4000-8000-000000000001",
            "name": "Apollo",
            "summary": "QA and pre-launch testing",
            "description": "there's a lot going on in the app, so the immediate need is simply more people using it hard and filing clear bug reports.\n\n## What Apollo is\n\nA browser app… (truncated, use `get_project` for full description)",
            "url": "https://linear.app/acme/project/apollo-qa-and-pre-launch-testing-712025eea70e",
            "createdAt": "2026-05-29T10:31:35.161Z",
            "updatedAt": "2026-05-29T10:31:35.161Z",
            "startedAt": null,
            "completedAt": null,
            "canceledAt": null,
            "startDate": null,
            "targetDate": "2026-06-16",
            "priority": {"value": 1, "name": "Urgent"},
            "status": {"name": "In Progress", "type": "started"},
        })
    }

    fn acme() -> Value {
        json!({
            "id": "e61f6417-68c4-41e3-a2b1-a55a334a8d79",
            "icon": "Robot",
            "name": "Acme",
            "visibility": "public",
            "createdAt": "2026-06-16T12:31:54.884Z",
            "updatedAt": "2026-09-14T04:10:51.227Z",
        })
    }

    #[test]
    fn linear_tool_names() {
        assert!(is_linear_tool("mcp__linear__list_projects"));
        assert!(is_linear_tool("mcp__plugin_linear_linear__list_teams"));
        assert!(!is_linear_tool("mcp__recur__list_findings"));
        assert!(!is_linear_tool("list_projects"));
        assert!(!is_linear_tool("WebSearch"));
    }

    #[test]
    fn labels_use_tense_and_query() {
        let args = parse_linear_args(&json!({"query": "Apollo"}));
        assert_eq!(linear_subject(&args), "Apollo");
        assert_eq!(
            linear_tool_label("mcp__linear__list_projects", true, false, &args, None),
            "Listing projects"
        );
        let view =
            parse_linear_result(&json!({"projects": [apollo()], "hasNextPage": false}).to_string());
        let label = linear_tool_label(
            "mcp__linear__list_projects",
            false,
            false,
            &args,
            view.as_ref(),
        );
        assert_eq!(label, "Listed projects");
        assert_eq!(
            linear_tool_label("mcp__linear__list_projects", false, true, &args, None),
            "List projects failed"
        );
        let meta = linear_tool_meta(&args, view.as_ref(), &label);
        assert!(
            meta.contains("\"Apollo\"") && !meta.contains("createdAt") && !meta.contains('{'),
            "{meta}"
        );
        assert_eq!(meta, "\"Apollo\" · 1");
        let args = parse_linear_args(&json!({"id": "ACME-114"}));
        assert_eq!(
            linear_tool_label("mcp__linear__get_issue", false, false, &args, None),
            "Looked up ACME-114"
        );
    }

    #[test]
    fn list_projects_rows() {
        let view =
            parse_linear_result(&json!({"projects": [apollo()], "hasNextPage": false}).to_string())
                .unwrap();
        assert_eq!(
            (view.kind, view.noun.as_str(), view.items.len()),
            (LinearViewKind::List, "projects", 1)
        );
        let row = &view.items[0];
        assert_eq!(row.title, "Apollo");
        assert!(row.url.contains("linear.app"));
        assert!(
            row.chips.contains(&"Urgent".to_owned())
                && row.chips.contains(&"In Progress".to_owned())
        );
        assert_eq!(row.summary, "QA and pre-launch testing");
        assert_eq!(row.body, "");
        assert_eq!(linear_count_label(&view), "1 project");
    }

    #[test]
    fn teams_and_labels() {
        let view =
            parse_linear_result(&json!({"teams": [acme()], "hasNextPage": false}).to_string())
                .unwrap();
        assert_eq!(view.items[0].title, "Acme");
        assert!(view.items[0].chips.contains(&"public".to_owned()));
        let view = parse_linear_result(
            &json!({"labels": [{"id": "lab-1", "name": "Bug", "color": "#EB5757", "description": "Something is broken"}]}).to_string(),
        )
        .unwrap();
        let item = &view.items[0];
        assert_eq!(
            (
                item.title.as_str(),
                item.color.as_str(),
                item.summary.as_str()
            ),
            ("Bug", "#EB5757", "Something is broken")
        );
    }

    #[test]
    fn get_issue_item() {
        let view = parse_linear_result(
            &json!({
                "id": "ACME-114", "identifier": "ACME-114", "title": "Apollo transcript cards",
                "description": "## Context\n\nRender Linear MCP results as cards.",
                "url": "https://linear.app/acme/issue/ACME-114",
                "status": {"name": "In Progress"}, "assignee": {"name": "Josh"},
            })
            .to_string(),
        );
        let v = view.as_ref().unwrap();
        assert_eq!(v.kind, LinearViewKind::Item);
        assert_eq!(
            (v.items[0].identifier.as_str(), v.items[0].title.as_str()),
            ("ACME-114", "Apollo transcript cards")
        );
        assert!(v.items[0].body.contains("Render Linear MCP results"));
        assert_eq!(v.items[0].chips, ["In Progress", "Josh"]);
        assert!(linear_item_url(view.as_ref()).contains("ACME-114"));

        let view = parse_linear_result(
            &json!({
                "id": "712025ee-a70e-4000-8000-000000000114", "identifier": "ACME-114", "title": "Apollo transcript cards",
                "description": "## Context\n\nRender Linear MCP results as cards.",
                "url": "https://linear.app/acme/issue/ACME-114", "status": {"name": "In Progress"}, "labels": ["Bug"],
                "attachments": [{"id": "att-1", "title": "shot.png"}], "documents": [{"id": "doc-1", "title": "Spec"}],
                "project": apollo(), "team": acme(),
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            (view.kind, view.noun.as_str(), view.items.len()),
            (LinearViewKind::Item, "", 1)
        );
        assert_eq!(view.items[0].identifier, "ACME-114");
        assert!(view.items[0].body.contains("Render Linear MCP results"));
    }

    #[test]
    fn rejects_non_entities() {
        assert_eq!(parse_linear_result(r#"{"error":"nope"}"#), None);
        assert_eq!(parse_linear_result("Refused: no"), None);
        assert_eq!(parse_linear_result(""), None);
        assert!(
            parse_linear_result(r#"note: [{"name":"A"},{"name":"B"}]"#)
                .is_some_and(|v| v.items.len() == 2)
        );
    }

    #[test]
    fn input_body_and_keys() {
        assert_eq!(
            linear_input_body(&json!({"body": "## Hello\n\nWorld"})),
            "## Hello\n\nWorld"
        );
        assert_eq!(linear_input_body(&json!({"query": "Apollo"})), "");
        assert_eq!(human_key("releaseNotes"), "release notes");
        assert_eq!(human_key("release_notes"), "release notes");
        assert_eq!(singular("issues"), "issue");
        assert_eq!(singular("statuses"), "status");
        assert_eq!(singular("class"), "class");
        assert_eq!(first_line("a\n  b", 88), "a b");
        assert_eq!(
            first_line(&"x".repeat(100), 10),
            format!("{}…", "x".repeat(9))
        );
    }
}
