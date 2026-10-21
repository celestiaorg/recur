//! The tool contract shared by the MCP server and the in-process harness.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parking_lot::RwLock;
use tokio_util::sync::CancellationToken;

use crate::coding::todos::TodoStore;
use crate::host::{Host, HostError, PermissionVerdict};
use crate::names::{ASK_USER, RECUR_MCP_PREFIX, SWITCH_MODE};

/// A failed tool call. Its text is the tool-level error the model reads.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ToolError(pub String);

impl From<String> for ToolError {
    fn from(s: String) -> Self {
        ToolError(s)
    }
}

impl From<&str> for ToolError {
    fn from(s: &str) -> Self {
        ToolError(s.to_string())
    }
}

impl From<HostError> for ToolError {
    fn from(e: HostError) -> Self {
        ToolError(e.0)
    }
}

/// A first-class Recur tool. MCP wraps `execute`; the compatible harness calls
/// it in-process.
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    /// The JSON input schema exactly as Go writes it (tools/list compacts it).
    fn schema(&self) -> String;
    /// Whether a successful call ends the calling turn (ask_user, switch_mode).
    fn halts(&self) -> bool {
        false
    }
    async fn execute(&self, call: Call) -> Result<String, ToolError>;
}

/// Asks the human whether a mutating coding tool may run.
#[async_trait::async_trait]
pub trait Permitter: Send + Sync {
    async fn request_permission(
        &self,
        cancel: &CancellationToken,
        ws: &str,
        agent: &str,
        tool: &str,
        input: &str,
    ) -> Result<PermissionVerdict, HostError>;
}

/// One tool invocation.
#[derive(Clone, Default)]
pub struct Call {
    /// The turn (harness) or HTTP request (MCP); cancels long calls.
    pub cancel: CancellationToken,
    pub workspace_id: String,
    pub agent_code: String,
    /// The conductor tab (PA*, MA) for a loop specialist turn. Empty for
    /// interactive chats, where `agent_code` is already the tab.
    pub chat_code: String,
    pub worktree: String,
    pub session_id: String,
    /// Raw JSON arguments; empty when the call carried none.
    pub args: String,
    pub bypass_perms: bool,
    /// The engine seam. `None` for coding-only calls.
    pub host: Option<Arc<dyn Host>>,
    /// Gates Bash when `bypass_perms` is false.
    pub permitter: Option<Arc<dyn Permitter>>,
    pub todos: Option<Arc<TodoStore>>,
}

impl Call {
    pub(crate) fn host(&self) -> Result<&dyn Host, ToolError> {
        self.host
            .as_deref()
            .ok_or_else(|| ToolError::from("no host reporter"))
    }
}

/// Short tool names to implementations, in registration order.
#[derive(Default)]
pub struct Registry {
    inner: RwLock<RegistryInner>,
}

#[derive(Default)]
struct RegistryInner {
    tools: HashMap<String, Arc<dyn Tool>>,
    order: Vec<String>,
}

impl Registry {
    /// Later tools with the same name replace earlier ones; unnamed tools are skipped.
    pub fn new(tools: impl IntoIterator<Item = Arc<dyn Tool>>) -> Registry {
        let r = Registry::default();
        for t in tools {
            if !t.name().is_empty() {
                r.add(t);
            }
        }
        r
    }

    pub fn add(&self, t: Arc<dyn Tool>) {
        let mut inner = self.inner.write();
        let name = t.name().to_string();
        if !inner.tools.contains_key(&name) {
            inner.order.push(name.clone());
        }
        inner.tools.insert(name, t);
    }

    /// The tool with the given short or mcp-prefixed name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.inner.read().tools.get(bare_name(name)).cloned()
    }

    /// Tools in registration order, filtered by a runner allowlist (`Read`,
    /// `mcp__recur__ask_user`, …). `None` means all.
    pub fn list(&self, allowed: Option<&[String]>) -> Vec<Arc<dyn Tool>> {
        let inner = self.inner.read();
        let Some(allowed) = allowed else {
            return inner.order.iter().map(|n| inner.tools[n].clone()).collect();
        };
        let mut want: HashSet<&str> = HashSet::new();
        for a in allowed.iter().filter(|a| !a.is_empty()) {
            want.insert(a);
            want.insert(bare_name(a));
        }
        inner
            .order
            .iter()
            .filter(|n| {
                want.contains(n.as_str())
                    || want.contains(format!("{RECUR_MCP_PREFIX}{n}").as_str())
            })
            .map(|n| inner.tools[n].clone())
            .collect()
    }
}

/// The name the model should see for `t` given the turn allowlist.
pub fn advertised_name(t: &dyn Tool, allowed: &[String]) -> String {
    let short = t.name();
    let prefixed = format!("{RECUR_MCP_PREFIX}{short}");
    for a in allowed {
        if *a == prefixed {
            return prefixed;
        }
        if a == short {
            return short.to_string();
        }
    }
    short.to_string()
}

/// Strips `mcp__recur__` if present.
pub fn bare_name(name: &str) -> &str {
    name.strip_prefix(RECUR_MCP_PREFIX).unwrap_or(name)
}

/// Host tools that park or remount the turn.
pub const HALT_NAMES: [&str; 2] = [ASK_USER, SWITCH_MODE];

/// Whether a successful call should end the native loop. switch_mode only
/// halts when the host actually parked and remounted the turn.
pub fn halt_after(name: &str, result: &str) -> bool {
    match bare_name(name) {
        ASK_USER => true,
        SWITCH_MODE => result.contains("remounting"),
        _ => false,
    }
}
