//! App-server approval requests routed to the human (Go
//! `handleCodexServerRequest` and helpers in `engine/codex.go`).
//!
//! With permissions not bypassed the thread runs `approval_policy =
//! on-request`, and sandbox exceptions arrive as server requests
//! (`item/commandExecution/requestApproval`,
//! `item/fileChange/requestApproval`, `item/permissions/requestApproval`,
//! legacy `execCommandApproval` / `applyPatchApproval`). They become the
//! same permission cards as Claude's prompt tool. MCP elicitations always
//! auto-accept so Recur's own tools never double-prompt. `dontAsk` (titles,
//! commit messages) declines without asking.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use recur_agent::PermissionAsker;
use recur_agent::permission::summary;
use recur_domain::{
    PERMISSION_MODE_BYPASS, PERMISSION_MODE_DENY, PermissionDecision, PermissionRequest,
};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::rpc::sort_keys;
use crate::server::auto_approve;

/// `fileChange` items by id from `item/started`: approval requests carry only
/// the item id, the patch lives on the item.
pub type FileItems = Arc<Mutex<HashMap<String, String>>>;

/// `never` for bypass and deny, `on-request` otherwise.
pub fn approval_policy(mode: &str) -> &'static str {
    match mode {
        PERMISSION_MODE_BYPASS | PERMISSION_MODE_DENY => "never",
        _ => "on-request",
    }
}

/// Whether the turn installs the human approval handler: never under bypass
/// (auto-accept stays), and only with someone to ask unless denying.
pub fn routes_to_human(mode: &str, has_asker: bool) -> bool {
    mode != PERMISSION_MODE_BYPASS && (mode == PERMISSION_MODE_DENY || has_asker)
}

/// The agent code inside an agent id (`<workspace>-<code>`), or the id.
pub fn agent_code(workspace_id: &str, agent_id: &str) -> String {
    match agent_id.strip_prefix(&format!("{workspace_id}-")) {
        Some(code) if !code.is_empty() => code.to_owned(),
        _ => agent_id.to_owned(),
    }
}

/// The permission-card tool for an approval method; `None` for requests the
/// human never sees.
pub fn approval_tool(method: &str) -> Option<&'static str> {
    match method {
        "item/commandExecution/requestApproval"
        | "commandExecution/requestApproval"
        | "execCommandApproval" => Some("Bash"),
        "item/fileChange/requestApproval" | "fileChange/requestApproval" | "applyPatchApproval" => {
            Some("Edit")
        }
        "item/permissions/requestApproval" => Some("Permissions"),
        _ => None,
    }
}

/// The app-server answer for a decision.
pub fn approval_response(method: &str, d: &PermissionDecision, params: &str) -> Value {
    if method == "item/permissions/requestApproval" {
        if !d.allow {
            return json!({"permissions": {}});
        }
        let scope = if d.remember { "session" } else { "turn" };
        let perms = serde_json::from_str::<Value>(params)
            .ok()
            .and_then(|v| v.get("permissions").cloned())
            .map_or_else(|| json!({}), sort_keys);
        return json!({"permissions": perms, "scope": scope});
    }
    if method == "execCommandApproval" {
        return json!({"decision": if d.allow { "approved" } else { "denied" }});
    }
    let decision = match (d.allow, d.remember) {
        (false, _) => "decline",
        (true, false) => "accept",
        (true, true) => "acceptForSession",
    };
    json!({"decision": decision})
}

/// Answers one server request for a turn.
pub async fn handle(
    method: &str,
    params: &str,
    req: &TurnTarget,
    asker: Option<&Arc<dyn PermissionAsker>>,
    cancel: &CancellationToken,
    file_items: &FileItems,
) -> Value {
    let Some(tool) = approval_tool(method) else {
        return auto_approve(method, params).unwrap_or_default();
    };
    if req.permission_mode == PERMISSION_MODE_DENY {
        return approval_response(method, &PermissionDecision::default(), params);
    }
    let Some(asker) = asker else {
        return auto_approve(method, params).unwrap_or_default();
    };
    let input = enrich_params(method, params, file_items);
    let ask = asker.ask(PermissionRequest {
        workspace_id: req.workspace_id.clone(),
        agent_code: agent_code(&req.workspace_id, &req.agent_id),
        tool: tool.into(),
        summary: summary(tool, &input),
        input,
        ..Default::default()
    });
    let decision = tokio::select! {
        d = ask => d.unwrap_or_default(),
        _ = cancel.cancelled() => PermissionDecision::default(),
    };
    approval_response(method, &decision, params)
}

/// What an approval needs from the turn.
#[derive(Debug, Clone, Default)]
pub struct TurnTarget {
    pub workspace_id: String,
    pub agent_id: String,
    pub permission_mode: String,
}

/// Copies the matching `fileChange` item's changes (kind, path, diff,
/// movePath) onto an Edit approval, whose params only carry
/// itemId/reason/grantRoot.
pub fn enrich_params(method: &str, params: &str, file_items: &FileItems) -> String {
    if approval_tool(method) != Some("Edit") {
        return params.to_owned();
    }
    let head: Map<String, Value> = serde_json::from_str(params).unwrap_or_default();
    let field = |k: &str| head.get(k).and_then(Value::as_str).unwrap_or_default();
    if !field("path").is_empty() || !field("file_path").is_empty() || !field("diff").is_empty() {
        return params.to_owned();
    }
    let Some(item) = file_items.lock().get(field("itemId")).cloned() else {
        return params.to_owned();
    };
    let Some(extra) = file_change_permission_input(&item) else {
        return params.to_owned();
    };
    let mut out = head;
    out.extend(extra);
    serde_json::to_string(&sort_keys(Value::Object(out))).unwrap_or_else(|_| params.to_owned())
}

/// `{changes, path, file_path, diff}` for a permission card from a
/// `fileChange` item; `None` without changes.
pub fn file_change_permission_input(item: &str) -> Option<Map<String, Value>> {
    let parsed: Value = serde_json::from_str(item).ok()?;
    let changes = parsed.get("changes")?.as_array()?;
    if changes.is_empty() {
        return None;
    }
    let mut paths = Vec::new();
    let mut diffs = String::new();
    for c in changes {
        if let Some(p) = c
            .get("path")
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty())
        {
            paths.push(p);
        }
        if let Some(d) = c.get("diff").and_then(Value::as_str) {
            diffs.push_str(d);
        }
    }
    let mut out = Map::new();
    out.insert("changes".into(), Value::Array(changes.clone()));
    if !paths.is_empty() {
        let joined = paths.join(", ");
        out.insert("path".into(), joined.clone().into());
        out.insert("file_path".into(), joined.into());
    }
    if !diffs.is_empty() {
        out.insert("diff".into(), diffs.into());
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use recur_agent::PermissionAbandoned;

    use super::*;

    struct Recorder {
        asked: Mutex<Vec<PermissionRequest>>,
        answer: PermissionDecision,
    }

    #[async_trait::async_trait]
    impl PermissionAsker for Recorder {
        async fn ask(
            &self,
            req: PermissionRequest,
        ) -> Result<PermissionDecision, PermissionAbandoned> {
            self.asked.lock().push(req);
            Ok(self.answer.clone())
        }
    }

    fn recorder(allow: bool, remember: bool) -> Arc<Recorder> {
        Arc::new(Recorder {
            asked: Mutex::default(),
            answer: PermissionDecision {
                allow,
                remember,
                ..Default::default()
            },
        })
    }

    fn decision(allow: bool, remember: bool) -> PermissionDecision {
        PermissionDecision {
            allow,
            remember,
            ..Default::default()
        }
    }

    #[test]
    fn policy_and_routing() {
        assert_eq!(approval_policy("bypassPermissions"), "never");
        assert_eq!(approval_policy("default"), "on-request");
        assert_eq!(approval_policy(""), "on-request");
        assert_eq!(approval_policy("dontAsk"), "never");
        assert!(!routes_to_human("bypassPermissions", true));
        assert!(routes_to_human("dontAsk", false));
        assert!(routes_to_human("default", true));
        assert!(!routes_to_human("default", false));
        assert_eq!(agent_code("W1", "W1-PA"), "PA");
        assert_eq!(agent_code("W1", "W1-"), "W1-");
        assert_eq!(agent_code("W1", "X"), "X");
    }

    #[test]
    fn responses() {
        let cmd = "item/commandExecution/requestApproval";
        assert_eq!(
            approval_response(cmd, &decision(true, true), "")["decision"],
            "acceptForSession"
        );
        assert_eq!(
            approval_response(cmd, &decision(true, false), "")["decision"],
            "accept"
        );
        assert_eq!(
            approval_response(cmd, &decision(false, false), "")["decision"],
            "decline"
        );
        let params = r#"{"permissions":{"fileSystem":{"write":["/tmp"]}}}"#;
        let perms = "item/permissions/requestApproval";
        let got = approval_response(perms, &decision(true, true), params);
        assert_eq!(got["scope"], "session");
        assert_eq!(got["permissions"]["fileSystem"]["write"][0], "/tmp");
        assert_eq!(
            approval_response(perms, &decision(true, false), params)["scope"],
            "turn"
        );
        assert_eq!(
            approval_response(perms, &decision(false, false), params),
            json!({"permissions": {}})
        );
        assert_eq!(
            approval_response("execCommandApproval", &decision(false, false), "")["decision"],
            "denied"
        );
    }

    fn target(ws: &str, agent: &str, mode: &str) -> TurnTarget {
        TurnTarget {
            workspace_id: ws.into(),
            agent_id: agent.into(),
            permission_mode: mode.into(),
        }
    }

    #[tokio::test]
    async fn asks_human() {
        let rec = recorder(true, true);
        let asker: Arc<dyn PermissionAsker> = rec.clone();
        let got = handle(
            "item/commandExecution/requestApproval",
            r#"{"command":"curl localhost:3000"}"#,
            &target("W1", "W1-PA", ""),
            Some(&asker),
            &CancellationToken::new(),
            &FileItems::default(),
        )
        .await;
        let asked = rec.asked.lock()[0].clone();
        assert_eq!(
            (
                asked.workspace_id.as_str(),
                asked.agent_code.as_str(),
                asked.tool.as_str()
            ),
            ("W1", "PA", "Bash")
        );
        assert!(asked.summary.contains("curl localhost:3000"));
        assert_eq!(got["decision"], "acceptForSession");
    }

    #[tokio::test]
    async fn elicitation_auto_accepts_and_deny_mode_declines() {
        let rec = recorder(true, false);
        let asker: Arc<dyn PermissionAsker> = rec.clone();
        let items = FileItems::default();
        let cancel = CancellationToken::new();
        let got = handle(
            "mcpServer/elicitation/request",
            "",
            &target("", "", ""),
            Some(&asker),
            &cancel,
            &items,
        )
        .await;
        assert_eq!(got["action"], "accept");
        let deny = target("W1", "", "dontAsk");
        let got = handle(
            "item/fileChange/requestApproval",
            r#"{"itemId":"fc1"}"#,
            &deny,
            Some(&asker),
            &cancel,
            &items,
        )
        .await;
        assert_eq!(got["decision"], "decline");
        let got = handle(
            "mcpServer/elicitation/request",
            "",
            &deny,
            Some(&asker),
            &cancel,
            &items,
        )
        .await;
        assert_eq!(got["action"], "accept");
        assert!(
            rec.asked.lock().is_empty(),
            "no prompt in dontAsk or for elicitations"
        );
        let got = handle(
            "item/commandExecution/requestApproval",
            "{}",
            &target("W1", "", ""),
            None,
            &cancel,
            &items,
        )
        .await;
        assert_eq!(
            got["decision"], "acceptForSession",
            "nobody to ask auto-accepts"
        );
    }

    #[tokio::test]
    async fn cancelled_turn_declines() {
        struct Never;
        #[async_trait::async_trait]
        impl PermissionAsker for Never {
            async fn ask(
                &self,
                _: PermissionRequest,
            ) -> Result<PermissionDecision, PermissionAbandoned> {
                std::future::pending().await
            }
        }
        let asker: Arc<dyn PermissionAsker> = Arc::new(Never);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let got = handle(
            "item/commandExecution/requestApproval",
            "{}",
            &target("W1", "W1-C", ""),
            Some(&asker),
            &cancel,
            &FileItems::default(),
        )
        .await;
        assert_eq!(got["decision"], "decline");
    }

    #[tokio::test]
    async fn file_change_includes_patch() {
        let items = FileItems::default();
        items.lock().insert(
            "fc1".into(),
            r#"{"id":"fc1","type":"fileChange","changes":[{"path":"/tmp/a.ts","kind":{"type":"update"},"diff":"@@ -1 +1 @@\n-a\n+b\n"}]}"#.into(),
        );
        let rec = recorder(true, false);
        let asker: Arc<dyn PermissionAsker> = rec.clone();
        let params =
            r#"{"threadId":"thr","turnId":"turn","itemId":"fc1","reason":"sandbox write"}"#;
        let got = handle(
            "item/fileChange/requestApproval",
            params,
            &target("W1", "W1-C", ""),
            Some(&asker),
            &CancellationToken::new(),
            &items,
        )
        .await;
        let asked = rec.asked.lock()[0].clone();
        assert_eq!(asked.tool, "Edit");
        assert!(asked.summary.contains("/tmp/a.ts"), "{}", asked.summary);
        assert!(asked.input.contains("/tmp/a.ts") && asked.input.contains("@@"));
        assert!(asked.input.contains(r#""kind""#) && asked.input.contains(r#""update""#));
        assert_eq!(got["decision"], "accept");
        assert_eq!(
            asked.input,
            r#"{"changes":[{"diff":"@@ -1 +1 @@\n-a\n+b\n","kind":{"type":"update"},"path":"/tmp/a.ts"}],"diff":"@@ -1 +1 @@\n-a\n+b\n","file_path":"/tmp/a.ts","itemId":"fc1","path":"/tmp/a.ts","reason":"sandbox write","threadId":"thr","turnId":"turn"}"#
        );
    }

    #[test]
    fn permission_input_keeps_kinds() {
        let item = r#"{"changes":[
            {"path":"/tmp/old.ts","kind":{"type":"update","movePath":"/tmp/new.ts"},"diff":"@@ -1 +1 @@\n-a\n+b\n"},
            {"path":"/tmp/a.ts","kind":{"type":"add"},"diff":"hello\n"},
            {"path":"/tmp/gone.ts","kind":{"type":"delete"}}
        ]}"#;
        let got = file_change_permission_input(item).unwrap();
        assert_eq!(got["path"], "/tmp/old.ts, /tmp/a.ts, /tmp/gone.ts");
        let s = serde_json::to_string(&got["changes"]).unwrap();
        assert!(s.contains(r#""type":"update""#) && s.contains(r#""movePath":"/tmp/new.ts""#));
        assert!(s.contains(r#""type":"add""#) && s.contains(r#""type":"delete""#));
        assert!(file_change_permission_input("{}").is_none());
    }
}
