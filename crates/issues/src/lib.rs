//! Issue trackers behind one [`IssueTracker`] trait (open-issue lists,
//! claiming, deep links, and the agent MCP wiring a tracker brings). GitHub
//! Issues ([`github::GitHubIssues`], over `gh`) and Linear ([`linear::Linear`],
//! GraphQL API) ship first.
//!
//! Task-prompt expansion stays in the shipped `bundles/Linear` and
//! `bundles/GitHub` prompt actions. Adding Jira is one module implementing
//! [`IssueTracker`] plus one registration line in `recur-services`.

pub mod github;
pub mod linear;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod view;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use recur_domain::{IssueRef, PrParticipant, Project, Settings};
use serde::{Deserialize, Serialize};

/// One open issue assigned to the user, or unassigned in their repos /
/// teams.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AssignedIssue {
    /// The tracker id (`github` | `linear`).
    pub source: String,
    /// Stable tracker id; matches `Workspace.issue.id`.
    pub id: String,
    /// `owner/repo#12` or `ENG-123`.
    pub r#ref: String,
    pub title: String,
    pub url: String,
    pub created_at: String,
    pub updated_at: String,
    /// `progress` | `todo` | `backlog`.
    pub group: String,
    pub state: String,
    pub state_color: String,
    /// Workflow order within a Linear team.
    pub state_position: f64,
    /// Linear: 0 none, 1 urgent … 4 low.
    pub priority: i64,
    pub priority_label: String,
    /// `owner/repo`, or the Linear project / team name.
    pub project: String,
    /// `None` serializes as `null`.
    pub labels: Option<Vec<IssueLabel>>,
    pub comments: i64,
    /// `None` serializes as `null`.
    pub assignees: Option<Vec<PrParticipant>>,
    /// The Recur project this issue belongs to, or empty.
    #[serde(rename = "projectId")]
    pub project_id: String,
    pub linear_project: String,
    /// Linear team key (`ENG`).
    pub linear_team: String,
}

/// A label chip; `color` is hex with or without `#`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct IssueLabel {
    pub name: String,
    pub color: String,
}

/// What a list call sees: settings (API keys), projects (repo/team links),
/// and the deadline for the whole listing.
#[derive(Debug, Clone, Copy)]
pub struct IssueQuery<'a> {
    pub settings: &'a Settings,
    pub projects: &'a [Project],
    pub deadline: tokio::time::Instant,
}

/// An HTTP MCP server a tracker attaches to agent turns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpEndpoint {
    pub url: String,
    pub headers: BTreeMap<String, String>,
}

/// A tracker call that failed; the message is what gets logged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueError(pub String);

impl std::fmt::Display for IssueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for IssueError {}

/// One issue tracker.
#[async_trait]
pub trait IssueTracker: Send + Sync + 'static {
    /// `IssueRef.source` / `AssignedIssue.source` (`github`, `linear`).
    fn id(&self) -> &str;
    /// User-facing name.
    fn label(&self) -> &str;
    /// Credentials are present (an unconfigured tracker lists nothing).
    fn configured(&self, settings: &Settings) -> bool;
    /// Open issues assigned to the user plus unassigned ones in their
    /// repos / teams, with `project_id` linked. Failures are logged and
    /// skipped, so a partial list still shows.
    async fn list_open(&self, q: IssueQuery<'_>) -> Vec<AssignedIssue>;
    /// Assigns the issue to the user when nobody owns it.
    async fn claim(&self, issue: &IssueRef, settings: &Settings) -> Result<(), IssueError>;
    /// The issue's web page when the issue list does not have it.
    fn issue_url(&self, _issue: &IssueRef) -> Option<String> {
        None
    }
    /// The tracker's desktop-app link for a web URL.
    fn app_url(&self, _web_url: &str) -> Option<String> {
        None
    }

    // ---- Agent wiring
    /// The Settings → MCP preset id this tracker provides.
    fn mcp_preset(&self) -> Option<&str> {
        None
    }
    /// The MCP server to attach for a turn in chat `mode`; `None` without
    /// credentials.
    fn agent_mcp(&self, _settings: &Settings, _mode: &str) -> Option<McpEndpoint> {
        None
    }
    /// System text appended when the MCP is attached.
    fn agent_hint(&self, _settings: &Settings) -> Option<String> {
        None
    }
    /// Env vars spawned agent CLIs must not inherit.
    fn secret_env(&self) -> &[&str] {
        &[]
    }
}

/// Every tracker's open issues, newest activity first. Trackers list
/// concurrently; each one stops at `q.deadline` with what it has.
pub async fn list_assigned(
    trackers: &[Arc<dyn IssueTracker>],
    q: IssueQuery<'_>,
) -> Vec<AssignedIssue> {
    let lists = futures::future::join_all(trackers.iter().map(|t| t.list_open(q))).await;
    let mut out: Vec<AssignedIssue> = lists.into_iter().flatten().collect();
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    out
}

/// Best effort with a 20 s bound; failures are logged.
pub async fn claim(trackers: &[Arc<dyn IssueTracker>], issue: &IssueRef, settings: &Settings) {
    let Some(t) = trackers.iter().find(|t| t.id() == issue.source) else {
        return;
    };
    let res = match tokio::time::timeout(Duration::from_secs(20), t.claim(issue, settings)).await {
        Ok(res) => res,
        Err(_) => Err(IssueError("context deadline exceeded".into())),
    };
    if let Err(e) = res {
        tracing::warn!("recur: assign {} issue {}: {e}", issue.source, issue.key);
    }
}

/// `env` without the variables trackers keep from agent processes.
pub fn scrub_env(
    env: Vec<(String, String)>,
    trackers: &[Arc<dyn IssueTracker>],
) -> Vec<(String, String)> {
    env.into_iter()
        .filter(|(k, _)| {
            !trackers
                .iter()
                .any(|t| t.secret_env().contains(&k.as_str()))
        })
        .collect()
}

/// The `Identifier:` and `Title:` header lines an expanded issue goal
/// carries (shipped prompt-action convention), each trimmed; empty when
/// missing.
pub fn goal_meta(goal: &str) -> (String, String) {
    let (mut ident, mut title) = (String::new(), String::new());
    for line in goal.split('\n') {
        let line = line.trim();
        if ident.is_empty()
            && let Some(rest) = line.strip_prefix("Identifier:")
        {
            ident = rest.trim().to_owned();
        }
        if title.is_empty()
            && let Some(rest) = line.strip_prefix("Title:")
        {
            title = rest.trim().to_owned();
        }
        if !ident.is_empty() && !title.is_empty() {
            break;
        }
    }
    (ident, title)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeTracker;

    #[test]
    fn goal_meta_reads_headers() {
        assert_eq!(
            goal_meta("Linear issue\n  Identifier: ENG-12 \nTitle:  Fix login\nIdentifier: ENG-99"),
            ("ENG-12".to_owned(), "Fix login".to_owned())
        );
        assert_eq!(goal_meta("Title: only"), (String::new(), "only".to_owned()));
        assert_eq!(goal_meta("plain goal"), (String::new(), String::new()));
    }

    #[test]
    fn issue_json_matches_go_shape() {
        let got = serde_json::to_value(AssignedIssue {
            source: "linear".into(),
            state_position: 2.0,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(got["source"], "linear");
        assert_eq!(got["labels"], serde_json::Value::Null);
        assert_eq!(got["projectId"], "");
        assert_eq!(got["ref"], "");
        assert_eq!(got["statePosition"], 2.0);
    }

    #[tokio::test]
    async fn list_assigned_merges_newest_first_and_claims_by_source() {
        let a = Arc::new(FakeTracker::new("github"));
        a.set_issues(vec![
            AssignedIssue {
                id: "1".into(),
                updated_at: "2026-08-01T00:00:00Z".into(),
                ..Default::default()
            },
            AssignedIssue {
                id: "3".into(),
                updated_at: "2026-08-03T00:00:00Z".into(),
                ..Default::default()
            },
        ]);
        let b = Arc::new(FakeTracker::new("linear"));
        b.set_issues(vec![AssignedIssue {
            id: "2".into(),
            updated_at: "2026-08-02T00:00:00Z".into(),
            ..Default::default()
        }]);
        let trackers: Vec<Arc<dyn IssueTracker>> = vec![a.clone(), b.clone()];
        let settings = Settings::default();
        let q = IssueQuery {
            settings: &settings,
            projects: &[],
            deadline: tokio::time::Instant::now() + Duration::from_secs(15),
        };
        let ids: Vec<String> = list_assigned(&trackers, q)
            .await
            .into_iter()
            .map(|i| i.id)
            .collect();
        assert_eq!(ids, ["3", "2", "1"]);

        let issue = IssueRef {
            source: "linear".into(),
            id: "uuid-157".into(),
            key: "BOTFUN-157".into(),
        };
        claim(&trackers, &issue, &settings).await;
        assert_eq!(b.claimed(), std::slice::from_ref(&issue));
        assert!(a.claimed().is_empty());
    }

    #[test]
    fn scrub_env_drops_tracker_secrets() {
        let trackers: Vec<Arc<dyn IssueTracker>> = vec![Arc::new(linear::Linear::new())];
        let env = vec![
            ("PATH".to_owned(), "/bin".to_owned()),
            ("LINEAR_API_KEY".to_owned(), "lin_x".to_owned()),
        ];
        assert_eq!(
            scrub_env(env, &trackers),
            [("PATH".to_owned(), "/bin".to_owned())]
        );
    }
}
