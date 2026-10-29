//! [`FakeTracker`]: an [`IssueTracker`] with canned issues that records
//! claims, for engine and services tests.

use async_trait::async_trait;
use parking_lot::Mutex;
use recur_domain::{IssueRef, Settings};

use crate::{AssignedIssue, IssueError, IssueQuery, IssueTracker, McpEndpoint};

#[derive(Default)]
struct State {
    issues: Vec<AssignedIssue>,
    claimed: Vec<IssueRef>,
    claim_error: Option<String>,
    mcp: Option<McpEndpoint>,
    hint: Option<String>,
}

pub struct FakeTracker {
    id: String,
    state: Mutex<State>,
}

impl FakeTracker {
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            state: Mutex::default(),
        }
    }

    /// Issues `list_open` returns.
    pub fn set_issues(&self, issues: Vec<AssignedIssue>) {
        self.state.lock().issues = issues;
    }

    /// Makes `claim` fail with this message.
    pub fn fail_claims(&self, message: &str) {
        self.state.lock().claim_error = Some(message.to_owned());
    }

    /// The MCP endpoint and hint returned for every mode.
    pub fn set_agent_mcp(&self, mcp: Option<McpEndpoint>, hint: Option<String>) {
        let mut s = self.state.lock();
        s.mcp = mcp;
        s.hint = hint;
    }

    /// Issues passed to `claim`, in order.
    pub fn claimed(&self) -> Vec<IssueRef> {
        self.state.lock().claimed.clone()
    }
}

#[async_trait]
impl IssueTracker for FakeTracker {
    fn id(&self) -> &str {
        &self.id
    }

    fn label(&self) -> &str {
        &self.id
    }

    fn configured(&self, _settings: &Settings) -> bool {
        true
    }

    async fn list_open(&self, _q: IssueQuery<'_>) -> Vec<AssignedIssue> {
        self.state.lock().issues.clone()
    }

    async fn claim(&self, issue: &IssueRef, _settings: &Settings) -> Result<(), IssueError> {
        let mut s = self.state.lock();
        s.claimed.push(issue.clone());
        match &s.claim_error {
            Some(e) => Err(IssueError(e.clone())),
            None => Ok(()),
        }
    }

    fn mcp_preset(&self) -> Option<&str> {
        Some(&self.id)
    }

    fn agent_mcp(&self, _settings: &Settings, _mode: &str) -> Option<McpEndpoint> {
        self.state.lock().mcp.clone()
    }

    fn agent_hint(&self, _settings: &Settings) -> Option<String> {
        self.state.lock().hint.clone()
    }
}
