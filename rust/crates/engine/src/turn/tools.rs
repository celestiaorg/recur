//! The in-process toolbox for `ToolTransport::InProcess` providers (Go
//! `harness.go` `responsesTools` / `execOne` / `harnessPermitter`): Recur
//! host tools plus the coding tools, run through the same `Tool::execute`
//! path MCP wraps, bound to one turn.

use std::sync::Arc;

use recur_agent::{ToolCallMeta, ToolHost, ToolOutcome, ToolSpec, TurnRequest};
use recur_agent_tools::{
    Call, Host, HostError, PermissionAsk, PermissionVerdict, Permitter, Registry, TodoStore, Tool,
    advertised_name, bare_name, coding_tools, halt_after, host_tools,
};
use recur_domain::PERMISSION_MODE_BYPASS;
use tokio_util::sync::CancellationToken;

const PERMISSION_REQUEST: &str = "permission_request";

/// `tools = Some("")` (blank) disables every tool on the turn.
pub fn tools_disabled(req: &TurnRequest) -> bool {
    req.tools.as_deref().is_some_and(|t| t.trim().is_empty())
}

/// Whether the turn may call `name`. `permission_request` never runs
/// in-process: the toolbox gates Bash itself.
pub fn tool_allowed_on_turn(name: &str, req: &TurnRequest) -> bool {
    if tools_disabled(req) {
        return false;
    }
    let bare = bare_name(name);
    if bare == PERMISSION_REQUEST {
        return false;
    }
    let Some(allowed) = &req.allowed_tools else {
        return true;
    };
    allowed.iter().any(|a| a == name || bare_name(a) == bare)
}

/// The agent code host tools attribute calls to: the session route's code,
/// else the agent id without its `<workspace>-` prefix.
pub fn in_process_agent_code(req: &TurnRequest) -> String {
    if !req.route.code.is_empty() {
        return req.route.code.clone();
    }
    let code = req
        .agent_id
        .strip_prefix(&format!("{}-", req.workspace_id))
        .unwrap_or(&req.agent_id);
    if code.is_empty() {
        return req.agent_id.clone();
    }
    code.to_owned()
}

/// One turn's in-process tools.
pub struct InProcessTools {
    registry: Registry,
    host: Option<Arc<dyn Host>>,
    todos: Arc<TodoStore>,
    req: TurnRequest,
}

impl InProcessTools {
    /// Host tools followed by the coding tools (Read, Edit, Write, Glob,
    /// Grep, Bash, TodoWrite).
    pub fn new(host: Option<Arc<dyn Host>>, todos: Arc<TodoStore>, req: &TurnRequest) -> Self {
        let mut tools: Vec<Arc<dyn Tool>> = host_tools();
        tools.extend(coding_tools());
        Self {
            registry: Registry::new(tools),
            host,
            todos,
            req: req.clone(),
        }
    }
}

#[async_trait::async_trait]
impl ToolHost for InProcessTools {
    fn tools(&self, allowed: Option<&[String]>) -> Vec<ToolSpec> {
        self.registry
            .list(allowed)
            .into_iter()
            .filter(|t| t.name() != PERMISSION_REQUEST)
            .map(|t| ToolSpec {
                name: advertised_name(t.as_ref(), allowed.unwrap_or_default()),
                description: t.description().to_owned(),
                schema: serde_json::from_str(&t.schema()).unwrap_or_default(),
            })
            .collect()
    }

    async fn call(&self, name: &str, args: &str, meta: ToolCallMeta) -> ToolOutcome {
        if !tool_allowed_on_turn(name, &self.req) {
            return error_outcome(format!("tool not allowed: {name}"));
        }
        let Some(t) = self.registry.get(name) else {
            return error_outcome(format!("unknown tool: {name}"));
        };
        let call = Call {
            cancel: meta.cancel,
            workspace_id: self.req.workspace_id.clone(),
            agent_code: in_process_agent_code(&self.req),
            chat_code: self.req.chat_code.clone(),
            worktree: self.req.worktree.to_string_lossy().into_owned(),
            session_id: self.req.recur_session_id.clone(),
            args: args.to_owned(),
            bypass_perms: self.req.permission_mode == PERMISSION_MODE_BYPASS,
            host: self.host.clone(),
            permitter: Some(Arc::new(HostPermitter {
                host: self.host.clone(),
            })),
            todos: Some(Arc::clone(&self.todos)),
        };
        match t.execute(call).await {
            Ok(text) => ToolOutcome {
                halt: halt_after(name, &text),
                text,
                is_error: false,
            },
            Err(e) => error_outcome(e.0),
        }
    }
}

fn error_outcome(text: String) -> ToolOutcome {
    ToolOutcome {
        text,
        is_error: true,
        halt: false,
    }
}

/// Bash prompts go to the host's permission broker.
struct HostPermitter {
    host: Option<Arc<dyn Host>>,
}

#[async_trait::async_trait]
impl Permitter for HostPermitter {
    async fn request_permission(
        &self,
        cancel: &CancellationToken,
        ws: &str,
        agent: &str,
        tool: &str,
        input: &str,
    ) -> Result<PermissionVerdict, HostError> {
        let Some(host) = &self.host else {
            return Ok(PermissionVerdict {
                allow: false,
                reason: "no permission broker".into(),
            });
        };
        host.request_permission(
            cancel,
            ws,
            agent,
            PermissionAsk {
                tool: tool.to_owned(),
                input: input.to_owned(),
                tool_use_id: String::new(),
            },
        )
        .await
    }
}
