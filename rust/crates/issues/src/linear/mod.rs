//! Linear through its GraphQL API: assigned and unassigned open issues,
//! claiming, `linear://` app links, and the Linear MCP preset for agents.

pub mod tools;

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use recur_domain::{
    IssueRef, PrParticipant, Project, SETTING_LINEAR_API_KEY, SETTING_LINEAR_TEAM_KEY, Settings,
    linear_mcp_url,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{AssignedIssue, IssueError, IssueLabel, IssueQuery, IssueTracker, McpEndpoint};

pub const GRAPHQL_URL: &str = "https://api.linear.app/graphql";

const ISSUE_FIELDS: &str = "id identifier title url createdAt updatedAt priority priorityLabel assignee { name displayName avatarUrl } state { name type color position } labels(first: 10) { nodes { name color } } project { name } team { key name }";
const OPEN_FILTER: &str = r#"state: {type: {nin: ["completed", "canceled", "duplicate"]}}"#;
const TEAMS_QUERY: &str = "query { viewer { teams(first: 50) { nodes { id } } } }";
const CLAIM_QUERY: &str =
    "query($id: String!) { viewer { id } issue(id: $id) { assignee { id } } }";
const CLAIM_MUTATION: &str = "mutation($id: String!, $a: String!) { issueUpdate(id: $id, input: {assigneeId: $a}) { success } }";

// simplify: unassigned Linear issues stop at 300 (3 pages). Raise it, or page
// lazily from the view, if teams carry bigger unassigned backlogs.
const UNASSIGNED_MAX: usize = 300;

pub fn assigned_query() -> String {
    format!(
        "query {{ viewer {{ assignedIssues(first: 100, filter: {{{OPEN_FILTER}}}) {{ nodes {{ {ISSUE_FIELDS} }} }} }} }}"
    )
}

pub fn unassigned_query() -> String {
    format!(
        "query($teams: [ID!], $after: String) {{ issues(first: 100, after: $after, filter: {{team: {{id: {{in: $teams}}}}, assignee: {{null: true}}, {OPEN_FILTER}}}) {{ nodes {{ {ISSUE_FIELDS} }} pageInfo {{ hasNextPage endCursor }} }} }}"
    )
}

/// Reads an environment variable (tests substitute their own).
pub type EnvFn = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

pub struct Linear {
    endpoint: String,
    env: EnvFn,
    client: reqwest::Client,
}

impl Default for Linear {
    fn default() -> Self {
        Self::new()
    }
}

impl Linear {
    pub fn new() -> Self {
        Self {
            endpoint: GRAPHQL_URL.into(),
            env: Arc::new(|k| std::env::var(k).ok()),
            client: reqwest::Client::new(),
        }
    }

    /// Another GraphQL endpoint (tests).
    pub fn with_endpoint(mut self, url: impl Into<String>) -> Self {
        self.endpoint = url.into();
        self
    }

    /// Another environment (tests).
    pub fn with_env(
        mut self,
        env: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        self.env = Arc::new(env);
        self
    }

    fn env(&self, key: &str) -> String {
        (self.env)(key).unwrap_or_default().trim().to_owned()
    }

    /// The API key: the Linear bundle setting, else `LINEAR_API_KEY`.
    pub fn api_key(&self, settings: &Settings) -> String {
        let k = settings.bundle_value(SETTING_LINEAR_API_KEY);
        if k.is_empty() {
            self.env("LINEAR_API_KEY")
        } else {
            k
        }
    }

    /// The default team key: the bundle setting, else `LINEAR_TEAM_KEY`.
    pub fn team_key(&self, settings: &Settings) -> String {
        let k = settings.bundle_value(SETTING_LINEAR_TEAM_KEY);
        if k.is_empty() {
            self.env("LINEAR_TEAM_KEY")
        } else {
            k
        }
    }

    /// Go `linearGraphQL`: one query; the decoded `data`.
    async fn graphql<T: for<'de> Deserialize<'de> + Default>(
        &self,
        key: &str,
        query: &str,
        vars: Option<BTreeMap<&str, Value>>,
    ) -> Result<T, IssueError> {
        let vars = vars.map(|v| json!(v));
        let body = recur_domain::json::to_string(&BTreeMap::from([
            ("query", json!(query)),
            ("variables", vars.unwrap_or(Value::Null)),
        ]))
        .unwrap_or_default();
        let resp = self
            .client
            .post(&self.endpoint)
            .header("Authorization", key)
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|e| IssueError(e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp
            .text()
            .await
            .map_err(|e| IssueError(format!("HTTP {status}: {e}")))?;
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Envelope {
            data: Option<Value>,
            errors: Vec<Message>,
        }
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Message {
            message: String,
        }
        let res: Envelope = serde_json::Deserializer::from_str(&text)
            .into_iter::<Value>()
            .next()
            .unwrap_or_else(|| serde_json::from_str::<Value>(""))
            .and_then(recur_forge::json::decode_value)
            .map_err(|e| IssueError(format!("HTTP {status}: {e}")))?;
        if let Some(e) = res.errors.first() {
            return Err(IssueError(e.message.clone()));
        }
        if status != 200 {
            return Err(IssueError(format!("HTTP {status}")));
        }
        match res.data {
            Some(data) => {
                recur_forge::json::decode_value(data).map_err(|e| IssueError(e.to_string()))
            }
            None => Ok(T::default()),
        }
    }

    async fn assigned(&self, key: &str) -> Vec<AssignedIssue> {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Data {
            viewer: Viewer,
        }
        #[derive(Default, Deserialize)]
        #[serde(default, rename_all = "camelCase")]
        struct Viewer {
            assigned_issues: Nodes,
        }
        match self.graphql::<Data>(key, &assigned_query(), None).await {
            Ok(d) => to_issues(d.viewer.assigned_issues.nodes),
            Err(e) => {
                tracing::warn!("recur: linear assigned issues: {e}");
                Vec::new()
            }
        }
    }

    /// Unassigned open issues in the user's teams: one flat paged query
    /// (nesting issues under teams multiplies Linear's complexity cost).
    async fn unassigned(&self, key: &str) -> Vec<AssignedIssue> {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Teams {
            viewer: TeamViewer,
        }
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct TeamViewer {
            teams: TeamNodes,
        }
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct TeamNodes {
            nodes: Vec<Id>,
        }
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Id {
            id: String,
        }
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Page {
            issues: PageIssues,
        }
        #[derive(Default, Deserialize)]
        #[serde(default, rename_all = "camelCase")]
        struct PageIssues {
            nodes: Vec<IssueNode>,
            page_info: PageInfo,
        }
        #[derive(Default, Deserialize)]
        #[serde(default, rename_all = "camelCase")]
        struct PageInfo {
            has_next_page: bool,
            end_cursor: String,
        }
        let teams = match self.graphql::<Teams>(key, TEAMS_QUERY, None).await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("recur: linear teams: {e}");
                return Vec::new();
            }
        };
        let ids: Vec<String> = teams.viewer.teams.nodes.into_iter().map(|t| t.id).collect();
        if ids.is_empty() {
            return Vec::new();
        }
        let query = unassigned_query();
        let mut nodes = Vec::new();
        let mut after = String::new();
        while nodes.len() < UNASSIGNED_MAX {
            let mut vars = BTreeMap::from([("teams", json!(ids))]);
            if !after.is_empty() {
                vars.insert("after", json!(after));
            }
            let page = match self.graphql::<Page>(key, &query, Some(vars)).await {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!("recur: linear unassigned issues: {e}");
                    break;
                }
            };
            nodes.extend(page.issues.nodes);
            if !page.issues.page_info.has_next_page || page.issues.page_info.end_cursor.is_empty() {
                break;
            }
            after = page.issues.page_info.end_cursor;
        }
        to_issues(nodes)
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Nodes {
    nodes: Vec<IssueNode>,
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct IssueNode {
    id: String,
    identifier: String,
    title: String,
    url: String,
    created_at: String,
    updated_at: String,
    priority: i64,
    priority_label: String,
    assignee: Option<Assignee>,
    state: State,
    labels: Option<LabelNodes>,
    project: Option<Named>,
    team: Team,
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Assignee {
    name: String,
    display_name: String,
    avatar_url: String,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct State {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    color: String,
    position: f64,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct LabelNodes {
    nodes: Option<Vec<IssueLabel>>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Named {
    name: String,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Team {
    key: String,
    name: String,
}

/// Go `linearToIssues`.
fn to_issues(nodes: Vec<IssueNode>) -> Vec<AssignedIssue> {
    nodes
        .into_iter()
        .map(|n| {
            let (mut project, mut linear_project) = (n.team.name.clone(), String::new());
            if let Some(p) = n.project.filter(|p| !p.name.is_empty()) {
                project = p.name.clone();
                linear_project = p.name;
            }
            let assignees = n.assignee.map(|a| {
                vec![PrParticipant {
                    name: if a.display_name.is_empty() {
                        a.name
                    } else {
                        a.display_name
                    },
                    avatar_url: a.avatar_url,
                    ..Default::default()
                }]
            });
            AssignedIssue {
                source: "linear".into(),
                id: n.id,
                r#ref: n.identifier,
                title: n.title,
                url: n.url,
                created_at: n.created_at,
                updated_at: n.updated_at,
                group: group(&n.state.kind).into(),
                state: n.state.name,
                state_color: n.state.color,
                state_position: n.state.position,
                priority: n.priority,
                priority_label: n.priority_label,
                project,
                labels: n.labels.and_then(|l| l.nodes),
                assignees,
                linear_project,
                linear_team: n.team.key,
                ..Default::default()
            }
        })
        .collect()
}

/// Board column for a Linear state type.
pub fn group(state_type: &str) -> &'static str {
    match state_type {
        "started" => "progress",
        "backlog" | "triage" => "backlog",
        _ => "todo",
    }
}

/// Sets `project_id` on Linear issues: a project's `linear_projects` entries
/// match the Linear project name first, then the team key.
pub fn link_projects(issues: &mut [AssignedIssue], projects: &[Project]) {
    let mut by_linear: BTreeMap<String, String> = BTreeMap::new();
    for p in projects {
        for tok in p.linear_projects.split(',') {
            let tok = tok.trim().to_lowercase();
            if !tok.is_empty() {
                by_linear.entry(tok).or_insert_with(|| p.id.clone());
            }
        }
    }
    for it in issues.iter_mut().filter(|i| i.source == "linear") {
        if let Some(id) = by_linear
            .get(&it.linear_project.to_lowercase())
            .filter(|_| !it.linear_project.is_empty())
        {
            it.project_id = id.clone();
        } else if !it.linear_team.is_empty() {
            it.project_id = by_linear
                .get(&it.linear_team.to_lowercase())
                .cloned()
                .unwrap_or_default();
        }
    }
}

/// `linear://…` for a `https://linear.app/…` URL (path and query kept).
pub fn app_url(raw: &str) -> Option<String> {
    let s = raw.trim();
    let (scheme, rest) = s.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let rest = rest.split('#').next().unwrap_or_default();
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if host != "linear.app" {
        return None;
    }
    let tail = &rest[end..];
    let (path, query) = match tail.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (tail, None),
    };
    let mut uri = escape_path(path);
    if uri.is_empty() {
        uri.push('/');
    }
    if let Some(q) = query.filter(|q| !q.is_empty()) {
        uri.push('?');
        uri.push_str(q);
    }
    Some(format!("linear:/{uri}"))
}

/// Keeps a path's existing escapes and escapes what a URL path cannot hold.
fn escape_path(p: &str) -> String {
    let mut out = String::with_capacity(p.len());
    for b in p.bytes() {
        let keep = b.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/%".contains(&b);
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[async_trait]
impl IssueTracker for Linear {
    fn id(&self) -> &str {
        "linear"
    }

    fn label(&self) -> &str {
        "Linear"
    }

    fn configured(&self, settings: &Settings) -> bool {
        !self.api_key(settings).is_empty()
    }

    async fn list_open(&self, q: IssueQuery<'_>) -> Vec<AssignedIssue> {
        let key = self.api_key(q.settings);
        if key.is_empty() {
            return Vec::new();
        }
        let (a, u) = tokio::join!(
            tokio::time::timeout_at(q.deadline, self.assigned(&key)),
            tokio::time::timeout_at(q.deadline, self.unassigned(&key))
        );
        let mut out: Vec<AssignedIssue> = a
            .unwrap_or_default()
            .into_iter()
            .chain(u.unwrap_or_default())
            .collect();
        link_projects(&mut out, q.projects);
        out
    }

    /// Go `claimLinearIssue`: assigns to the viewer when unassigned.
    async fn claim(&self, issue: &IssueRef, settings: &Settings) -> Result<(), IssueError> {
        let key = self.api_key(settings);
        if key.is_empty() {
            return Ok(());
        }
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Data {
            viewer: Id,
            issue: ClaimIssue,
        }
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct ClaimIssue {
            assignee: Option<Id>,
        }
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Id {
            id: String,
        }
        let d: Data = self
            .graphql(
                &key,
                CLAIM_QUERY,
                Some(BTreeMap::from([("id", json!(issue.id))])),
            )
            .await?;
        if d.issue.assignee.is_some() || d.viewer.id.is_empty() {
            return Ok(());
        }
        let _: Value = self
            .graphql(
                &key,
                CLAIM_MUTATION,
                Some(BTreeMap::from([
                    ("a", json!(d.viewer.id)),
                    ("id", json!(issue.id)),
                ])),
            )
            .await?;
        Ok(())
    }

    fn app_url(&self, web_url: &str) -> Option<String> {
        app_url(web_url)
    }

    fn mcp_preset(&self) -> Option<&str> {
        Some(recur_domain::MCP_PRESET_LINEAR)
    }

    /// The official Linear MCP: readonly for Ask/Plan/Review, write for
    /// Agent and Loop; bearer key from settings or `LINEAR_API_KEY`.
    fn agent_mcp(&self, settings: &Settings, mode: &str) -> Option<McpEndpoint> {
        let key = self.api_key(settings);
        if key.is_empty() {
            return None;
        }
        Some(McpEndpoint {
            url: linear_mcp_url(mode).to_owned(),
            headers: BTreeMap::from([("Authorization".to_owned(), format!("Bearer {key}"))]),
        })
    }

    fn agent_hint(&self, settings: &Settings) -> Option<String> {
        let team = self.team_key(settings);
        (!team.is_empty())
            .then(|| format!("Linear MCP is connected. Default team key: {team}. Prefer that team unless the user names another."))
    }

    fn secret_env(&self) -> &[&str] {
        &["LINEAR_API_KEY"]
    }
}

#[cfg(test)]
mod tests;
