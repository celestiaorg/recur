//! Port of `server_test.go`, `tools_briefing_test.go` and `wrap_test.go`.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use recur_agent_tools::{
    AskUserRequest, CreateArtifactRequest, CreateWorkspaceRequest, EscalationRequest, Evidence,
    Finding, Host, HostError, PermissionAsk, PermissionVerdict, PlanSubmission, TerminalInfo,
    host_tools,
};
use recur_domain::{
    APP_AGENT_CODE, AgentSkill, Artifact, ArtifactKind, BriefingCounts, BriefingSnapshot,
    Finding as StoredFinding, PresentWorkspaceResult, Severity, WorkspaceBrief, WorkspaceContext,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::tools::{all_tools, append_permission_request, names_from_allowed_tools};

type OnAsk = Arc<
    dyn Fn(CancellationToken) -> tokio::task::JoinHandle<Result<PermissionVerdict, HostError>>
        + Send
        + Sync,
>;

#[derive(Clone, Debug, PartialEq)]
struct Recorded {
    ws: String,
    agent: String,
    summary: String,
}

fn rec(ws: &str, agent: &str, summary: &str) -> Recorded {
    Recorded {
        ws: ws.into(),
        agent: agent.into(),
        summary: summary.into(),
    }
}

/// Records every typed callback so tests assert a tools/call became the
/// right engine callback (not prose parsing). Safe for concurrent use.
#[derive(Default)]
struct Capture {
    findings: Mutex<Vec<(String, String, Finding)>>,
    dones: Mutex<Vec<Recorded>>,
    done_notes: Mutex<Vec<String>>,
    results: Mutex<Vec<(String, String, bool, String)>>,
    plans: Mutex<Vec<Recorded>>,
    escalations: Mutex<Vec<(String, String, EscalationRequest)>>,
    evidence: Mutex<Vec<(String, String, Evidence)>>,
    send_backs: Mutex<Vec<Recorded>>,
    submitted_plans: Mutex<Vec<Recorded>>,
    last_plan: Mutex<PlanSubmission>,
    step_updates: Mutex<Vec<Recorded>>,
    asks: Mutex<Vec<(String, String, PermissionAsk)>>,
    /// Answers a permission request; `None` allows at once.
    on_ask: Option<OnAsk>,
    rename_owner: Mutex<String>,
}

impl Capture {
    fn counts(&self) -> (usize, usize, usize, usize) {
        (
            self.findings.lock().unwrap().len(),
            self.dones.lock().unwrap().len(),
            self.results.lock().unwrap().len(),
            self.plans.lock().unwrap().len(),
        )
    }
}

#[async_trait::async_trait]
impl Host for Capture {
    fn report_finding(&self, ws: &str, agent: &str, f: Finding) -> StoredFinding {
        self.findings
            .lock()
            .unwrap()
            .push((ws.into(), agent.into(), f.clone()));
        StoredFinding {
            id: format!("F-test-{}", f.file),
            file: f.file,
            line: f.line,
            end_line: f.end_line,
            severity: f.severity,
            message: f.message,
            summary: f.summary,
            status: "open".into(),
            ..Default::default()
        }
    }

    fn done(&self, ws: &str, agent: &str, summary: &str, notes: &str) -> Result<(), HostError> {
        self.dones.lock().unwrap().push(rec(ws, agent, summary));
        self.done_notes.lock().unwrap().push(notes.into());
        Ok(())
    }

    fn update_finding(
        &self,
        _ws: &str,
        _a: &str,
        id: &str,
        status: &str,
        reason: &str,
    ) -> Result<StoredFinding, HostError> {
        Ok(StoredFinding {
            id: id.into(),
            status: status.into(),
            status_reason: reason.into(),
            ..Default::default()
        })
    }

    fn report_result(&self, ws: &str, agent: &str, passed: bool, summary: &str) {
        self.results
            .lock()
            .unwrap()
            .push((ws.into(), agent.into(), passed, summary.into()));
    }

    fn plan(&self, ws: &str, agent: &str, summary: &str) {
        self.plans.lock().unwrap().push(rec(ws, agent, summary));
    }

    fn escalate(&self, ws: &str, agent: &str, e: EscalationRequest) {
        self.escalations
            .lock()
            .unwrap()
            .push((ws.into(), agent.into(), e));
    }

    fn attach_evidence(&self, ws: &str, agent: &str, e: Evidence) {
        self.evidence
            .lock()
            .unwrap()
            .push((ws.into(), agent.into(), e));
    }

    async fn request_permission(
        &self,
        cancel: &CancellationToken,
        ws: &str,
        agent: &str,
        ask: PermissionAsk,
    ) -> Result<PermissionVerdict, HostError> {
        self.asks
            .lock()
            .unwrap()
            .push((ws.into(), agent.into(), ask));
        match &self.on_ask {
            None => Ok(PermissionVerdict {
                allow: true,
                reason: String::new(),
            }),
            Some(f) => f(cancel.clone()).await.unwrap(),
        }
    }

    async fn send_back(&self, ws: &str, agent: &str, instruction: &str) -> String {
        self.send_backs
            .lock()
            .unwrap()
            .push(rec(ws, agent, instruction));
        "Sent back to the loop.".into()
    }

    async fn edit_plan(&self, _ws: &str, _agent: &str, _plan: &str) -> String {
        "Plan updated.".into()
    }

    async fn rerun_review(&self, _ws: &str, _agent: &str) -> String {
        "Reviewer dispatched.".into()
    }

    async fn dispatch_specialist(
        &self,
        _ws: &str,
        _agent: &str,
        _role: &str,
        _instruction: &str,
    ) -> String {
        "Specialist dispatched.".into()
    }

    async fn resume_loop(&self, _ws: &str, _agent: &str) -> String {
        "Loop restarting.".into()
    }

    async fn extend_iterations(&self, _ws: &str, _agent: &str, _n: i64) -> String {
        "Extension granted.".into()
    }

    async fn extend_rounds(&self, _ws: &str, _agent: &str, _n: i64) -> String {
        "Extension granted.".into()
    }

    async fn reenter_phase(&self, _ws: &str, _agent: &str, phase: &str) -> String {
        format!("Reentered {phase}")
    }

    async fn run_phase(&self, _ws: &str, _agent: &str, phase: &str) -> String {
        format!("Running {phase}")
    }

    async fn submit_plan(&self, ws: &str, agent: &str, p: PlanSubmission) -> String {
        self.submitted_plans.lock().unwrap().push(rec(
            ws,
            agent,
            &format!("{} | {}", p.summary, p.steps.join("; ")),
        ));
        *self.last_plan.lock().unwrap() = p;
        "Plan recorded.".into()
    }

    async fn update_plan_step(
        &self,
        ws: &str,
        agent: &str,
        step_id: &str,
        status: &str,
        _note: &str,
    ) -> String {
        self.step_updates
            .lock()
            .unwrap()
            .push(rec(ws, agent, &format!("{step_id}→{status}")));
        "Step updated.".into()
    }

    async fn list_skills(&self, _ws: &str, _agent: &str) -> Result<Vec<AgentSkill>, HostError> {
        Ok(vec![AgentSkill {
            name: "demo-skill".into(),
            description: "A demo".into(),
            scope: "user".into(),
            path: "/tmp/demo-skill/SKILL.md".into(),
            ..Default::default()
        }])
    }

    async fn activate_skill(
        &self,
        _ws: &str,
        _agent: &str,
        name: &str,
    ) -> Result<String, HostError> {
        if name != "demo-skill" {
            return Err(HostError::new(format!("unknown skill \"{name}\"")));
        }
        Ok(r#"<skill_content name="demo-skill">demo body</skill_content>"#.into())
    }

    async fn list_terminals(&self, _ws: &str, _agent: &str) -> Option<Vec<TerminalInfo>> {
        Some(vec![TerminalInfo {
            id: "term-1".into(),
            title: "zsh".into(),
            cwd: "/tmp".into(),
            active: true,
            ..Default::default()
        }])
    }

    async fn read_terminal(
        &self,
        _ws: &str,
        _agent: &str,
        id: &str,
        _tail: i64,
    ) -> Result<String, HostError> {
        if !id.is_empty() && id != "term-1" {
            return Err(HostError::new(format!("terminal \"{id}\" not found")));
        }
        Ok("error: boom\n".into())
    }

    async fn ask_user(&self, _ws: &str, _agent: &str, _req: AskUserRequest) -> String {
        "Question delivered to the user. Stop now and wait.".into()
    }

    async fn switch_mode(&self, _ws: &str, _agent: &str, mode: &str) -> String {
        format!("Switched to {mode}")
    }

    async fn create_artifact(
        &self,
        ws: &str,
        _agent: &str,
        req: CreateArtifactRequest,
    ) -> Result<Artifact, HostError> {
        Ok(Artifact {
            id: "a1".into(),
            workspace_id: ws.into(),
            title: req.title,
            kind: ArtifactKind::from(req.kind.as_str()),
            ..Default::default()
        })
    }

    async fn read_artifact(&self, _ws: &str, _agent: &str, id: &str) -> Result<String, HostError> {
        if id.is_empty() || id == "missing" {
            return Err(HostError::new(format!("artifact not found: {id}")));
        }
        Ok(format!(
            r##"{{"id":"{id}","title":"Notes","kind":"md","content":"# hi\n"}}"##
        ))
    }

    async fn present_artifact(&self, _ws: &str, _agent: &str, id: &str) -> String {
        format!("Presented {id}")
    }

    async fn rename_session(&self, _ws: &str, _agent: &str, owner: &str, title: &str) -> String {
        *self.rename_owner.lock().unwrap() = owner.into();
        format!("Renamed to {title}")
    }

    async fn rename_workspace(&self, _ws: &str, _agent: &str, title: &str) -> String {
        format!("Renamed workspace to {title}")
    }

    async fn rename_branch(&self, _ws: &str, _agent: &str, name: &str) -> String {
        format!("Renamed branch to {name}")
    }

    async fn merge_child(&self, _ws: &str, _agent: &str, _child: &str) -> String {
        "merged".into()
    }

    async fn read_child(&self, _ws: &str, _agent: &str, _child: &str) -> String {
        "report".into()
    }

    async fn archive_child(&self, _ws: &str, _agent: &str, _child: &str) -> String {
        "archived".into()
    }

    async fn message_child(&self, _ws: &str, _agent: &str, _child: &str, _msg: &str) -> String {
        "steered".into()
    }

    async fn read_child_artifact(
        &self,
        _ws: &str,
        _a: &str,
        _c: &str,
        _id: &str,
    ) -> Result<String, HostError> {
        Ok("{}".into())
    }

    async fn create_workspace(
        &self,
        _ws: &str,
        _agent: &str,
        _req: CreateWorkspaceRequest,
    ) -> String {
        "created".into()
    }

    async fn open_pr(
        &self,
        _ws: &str,
        _agent: &str,
        title: &str,
        _body: &str,
        draft: bool,
    ) -> String {
        if draft {
            return format!("Opened draft PR: {title}");
        }
        format!("Opened PR: {title}")
    }

    async fn list_workspace_briefs(&self) -> BriefingSnapshot {
        BriefingSnapshot {
            counts: BriefingCounts {
                needs: 1,
                ..Default::default()
            },
            workspaces: Some(vec![WorkspaceBrief {
                workspace_id: "WS-1".into(),
                title: "Needs a look".into(),
                bucket: "needs".into(),
                ..Default::default()
            }]),
            ..Default::default()
        }
    }

    async fn workspace_context(&self, id: &str) -> Result<WorkspaceContext, HostError> {
        if id.is_empty() {
            return Err(HostError::new("workspace_id is required"));
        }
        Ok(WorkspaceContext {
            brief: WorkspaceBrief {
                workspace_id: id.into(),
                title: "ctx".into(),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn present_workspace(&self, target: &str) -> String {
        recur_domain::json::to_string(&PresentWorkspaceResult {
            ok: true,
            target: target.into(),
            ..Default::default()
        })
        .unwrap()
    }
}

fn server(cap: &Arc<Capture>) -> McpServer {
    McpServer::new(Some(cap.clone()))
}

fn binding(ws: &str, agent: &str) -> Binding {
    Binding {
        workspace_id: ws.into(),
        agent_code: agent.into(),
        chat_code: String::new(),
        allowed: None,
        active: false,
        stable: false,
    }
}

fn tools_call(id: &str, name: &str, args: &str) -> RpcRequest {
    RpcRequest {
        jsonrpc: "2.0".into(),
        id: id.into(),
        method: "tools/call".into(),
        params: format!(r#"{{"name":{},"arguments":{args}}}"#, json!(name)),
    }
}

/// tools/call straight through dispatch (no HTTP), so framing and handler are
/// tested in isolation.
async fn call_ctx(
    s: &McpServer,
    cancel: &CancellationToken,
    b: &Binding,
    name: &str,
    args: Value,
) -> CallToolResult {
    let resp = s
        .dispatch(cancel, &tools_call("1", name, &args.to_string()), b)
        .await
        .expect("tools/call is not a notification");
    match resp.body {
        Ok(RpcResult::CallTool(r)) => r,
        other => panic!("tools/call {name}: {other:?}"),
    }
}

async fn call(s: &McpServer, b: &Binding, name: &str, args: Value) -> CallToolResult {
    call_ctx(s, &CancellationToken::new(), b, name, args).await
}

fn text(r: &CallToolResult) -> &str {
    assert_eq!(r.content.len(), 1, "one content block");
    &r.content[0].text
}

#[tokio::test]
async fn submit_plan_object_steps() {
    let cap = Arc::new(Capture::default());
    let s = server(&cap);
    let b = binding("L3", "MA");
    let res = call(
        &s,
        &b,
        "submit_plan",
        json!({
            "summary": "Importer", "body": "Parser and writer.",
            "steps": [
                {"title": "Write the parser", "files": ["parser.go", " "]},
                "Plain step",
                {"title": "Wire it", "files": ["main.go"], "depends_on": ["S1", "S2"]},
                {"title": "   "},
            ],
        }),
    )
    .await;
    assert!(!res.is_error, "{res:?}");
    let p = cap.last_plan.lock().unwrap().clone();
    assert_eq!(p.steps.join(";"), "Write the parser;Plain step;Wire it");
    assert_eq!(p.step_meta.len(), 3);
    assert_eq!(p.step_meta[0].files.join(","), "parser.go");
    assert!(p.step_meta[1].files.is_empty());
    assert_eq!(p.step_meta[2].depends_on.join(","), "S1,S2");
    let res = call(
        &s,
        &b,
        "submit_plan",
        json!({"summary": "bad", "body": "x", "steps": [{"name": "wrong key"}]}),
    )
    .await;
    assert!(res.is_error, "an object step without title is a tool error");
}

#[tokio::test]
async fn report_finding_fires_typed_callback() {
    let cap = Arc::new(Capture::default());
    let s = server(&cap);
    let res = call(
        &s,
        &binding("L1", "RV"),
        "report_finding",
        json!({"file": "main.go", "line": 42, "severity": "high", "message": "nil deref", "summary": "nil deref on boot path"}),
    )
    .await;
    assert!(!res.is_error, "{res:?}");
    assert_eq!(cap.counts().0, 1);
    let (ws, agent, f) = cap.findings.lock().unwrap()[0].clone();
    assert_eq!((ws.as_str(), agent.as_str()), ("L1", "RV"));
    assert_eq!(
        (f.file.as_str(), f.line, f.severity, f.message.as_str()),
        ("main.go", 42, Severity::High, "nil deref")
    );
    assert_eq!(f.summary, "nil deref on boot path");
    let out: Value = serde_json::from_str(text(&res)).unwrap();
    assert!(!out["id"].as_str().unwrap().is_empty());
    assert_eq!(out["file"], "main.go");
    assert_eq!(out["summary"], "nil deref on boot path");
    assert_eq!(out["status"], "open");
}

#[tokio::test]
async fn report_finding_range() {
    let cap = Arc::new(Capture::default());
    let s = server(&cap);
    let res = call(
        &s,
        &binding("L1", "RV"),
        "report_finding",
        json!({"file": "main.go", "line": 10, "end_line": 20, "severity": "medium", "message": "span"}),
    )
    .await;
    assert!(!res.is_error);
    let f = cap.findings.lock().unwrap()[0].2.clone();
    assert_eq!((f.line, f.end_line), (10, 20));
}

#[tokio::test]
async fn done_and_report_result_and_plan() {
    let cap = Arc::new(Capture::default());
    let s = server(&cap);
    let b = binding("L2", "CO");
    call(
        &s,
        &b,
        "done",
        json!({"summary": "implemented feature", "notes": "S2 was too big — split it"}),
    )
    .await;
    call(
        &s,
        &b,
        "report_result",
        json!({"passed": true, "summary": "all green"}),
    )
    .await;
    call(
        &s,
        &binding("L2", "MA"),
        "plan",
        json!({"summary": "do X then Y"}),
    )
    .await;
    let (_, nd, nr, np) = cap.counts();
    assert_eq!((nd, nr, np), (1, 1, 1));
    assert_eq!(
        cap.dones.lock().unwrap()[0],
        rec("L2", "CO", "implemented feature")
    );
    assert_eq!(
        cap.done_notes.lock().unwrap()[0],
        "S2 was too big — split it"
    );
    let r = cap.results.lock().unwrap()[0].clone();
    assert!(r.2 && r.3 == "all green");
    assert_eq!(cap.plans.lock().unwrap()[0].summary, "do X then Y");
}

#[tokio::test]
async fn report_result_false_recorded() {
    let cap = Arc::new(Capture::default());
    let s = server(&cap);
    call(
        &s,
        &binding("L", "TE"),
        "report_result",
        json!({"passed": false, "summary": "tests failed"}),
    )
    .await;
    assert_eq!(cap.counts().2, 1);
    assert!(!cap.results.lock().unwrap()[0].2);
}

#[tokio::test]
async fn escalate_fires_typed_callback() {
    let cap = Arc::new(Capture::default());
    let s = server(&cap);
    let res = call(
        &s,
        &binding("L9", "MA"),
        "escalate",
        json!({
            "question": "When a client goes over the rate limit, what should happen?",
            "context": "The ticket doesn't define over-limit behavior; I won't guess on the public contract.",
            "options": [
                {"text": "Reject with 429", "tag": "simplest", "desc": "Clear but harsh."},
                {"text": "Token bucket", "tag": "recommended", "desc": "Fair, standard."},
            ],
        }),
    )
    .await;
    assert!(!res.is_error);
    let esc = cap.escalations.lock().unwrap();
    assert_eq!(esc.len(), 1);
    let (ws, agent, req) = &esc[0];
    assert_eq!((ws.as_str(), agent.as_str()), ("L9", "MA"));
    assert!(!req.question.is_empty() && req.options.len() == 2);
    assert_eq!(
        (req.options[1].text.as_str(), req.options[1].tag.as_str()),
        ("Token bucket", "recommended")
    );
}

#[tokio::test]
async fn escalate_requires_question() {
    let cap = Arc::new(Capture::default());
    let s = server(&cap);
    let res = call(&s, &binding("L", "MA"), "escalate", json!({"context": "x"})).await;
    assert!(res.is_error);
    assert!(cap.escalations.lock().unwrap().is_empty());
}

#[tokio::test]
async fn attach_evidence_fires_typed_callback() {
    let cap = Arc::new(Capture::default());
    let s = server(&cap);
    let res = call(
        &s,
        &binding("L5", "QA"),
        "attach_evidence",
        json!({"step": "Opened /checkout and signed in with Apple", "screenshot_path": "/data/workspaces/L5/screenshots/01.png", "passed": true}),
    )
    .await;
    assert!(!res.is_error);
    let ev = cap.evidence.lock().unwrap();
    assert_eq!(ev.len(), 1);
    let (ws, agent, e) = &ev[0];
    assert_eq!((ws.as_str(), agent.as_str()), ("L5", "QA"));
    assert_eq!(e.step, "Opened /checkout and signed in with Apple");
    assert_eq!(e.screenshot_path, "/data/workspaces/L5/screenshots/01.png");
    assert!(e.passed);
}

#[tokio::test]
async fn attach_evidence_failing_step() {
    let cap = Arc::new(Capture::default());
    let s = server(&cap);
    let res = call(
        &s,
        &binding("L6", "QA"),
        "attach_evidence",
        json!({"step": "Sign in with Google returned a 500", "passed": false}),
    )
    .await;
    assert!(!res.is_error);
    let ev = cap.evidence.lock().unwrap();
    assert_eq!(ev.len(), 1);
    assert!(!ev[0].2.passed);
    assert!(ev[0].2.screenshot_path.is_empty());
}

#[tokio::test]
async fn bad_input_rejected() {
    let cap = Arc::new(Capture::default());
    let s = server(&cap);
    let b = binding("L", "RV");
    let cases = [
        (
            "missing file",
            "report_finding",
            json!({"severity": "high", "message": "x"}),
        ),
        (
            "missing message",
            "report_finding",
            json!({"file": "a.go", "severity": "high"}),
        ),
        (
            "bad severity",
            "report_finding",
            json!({"file": "a.go", "severity": "fatal", "message": "x"}),
        ),
        (
            "negative line",
            "report_finding",
            json!({"file": "a.go", "line": -1, "severity": "low", "message": "x"}),
        ),
        (
            "end before start",
            "report_finding",
            json!({"file": "a.go", "line": 10, "end_line": 5, "severity": "low", "message": "x"}),
        ),
        (
            "end without start",
            "report_finding",
            json!({"file": "a.go", "end_line": 5, "severity": "low", "message": "x"}),
        ),
        (
            "unknown field",
            "report_finding",
            json!({"file": "a.go", "severity": "low", "message": "x", "bogus": 1}),
        ),
        ("done no summary", "done", json!({})),
        ("result no passed", "report_result", json!({"summary": "x"})),
        ("plan no summary", "plan", json!({})),
        ("escalate no question", "escalate", json!({"context": "x"})),
        (
            "escalate empty option text",
            "escalate",
            json!({"question": "q", "options": [{"text": "  "}]}),
        ),
        (
            "evidence no step",
            "attach_evidence",
            json!({"passed": true}),
        ),
        (
            "evidence no passed",
            "attach_evidence",
            json!({"step": "did a thing"}),
        ),
        (
            "evidence unknown field",
            "attach_evidence",
            json!({"step": "x", "passed": true, "bogus": 1}),
        ),
        ("send_back no instruction", "send_back", json!({})),
    ];
    for (name, tool, args) in cases {
        let res = call(&s, &b, tool, args).await;
        assert!(
            res.is_error,
            "{name}: expected a tool-level error, got {res:?}"
        );
    }
    assert_eq!(
        cap.counts(),
        (0, 0, 0, 0),
        "rejected calls fire no callbacks"
    );
    assert!(cap.evidence.lock().unwrap().is_empty());
}

#[tokio::test]
async fn unknown_tool_is_rpc_error() {
    let s = server(&Arc::new(Capture::default()));
    let resp = s
        .dispatch(
            &CancellationToken::new(),
            &tools_call("7", "no_such_tool", "{}"),
            &binding("L", "RV"),
        )
        .await
        .unwrap();
    assert!(
        matches!(
            resp.body,
            Err(RpcError {
                code: CODE_METHOD_NOT_FOUND,
                ..
            })
        ),
        "{resp:?}"
    );
}

// The wire shapes below are what Claude Code 2.1 sends to, and expects back
// from, a --permission-prompt-tool. Getting them wrong fails open or hangs the
// agent.
#[tokio::test]
async fn permission_request_allow_echoes_input() {
    let cap = Arc::new(Capture::default());
    let s = server(&cap);
    let input = r#"{"file_path":"/tmp/x.go","content":"package x"}"#;
    let args = format!(r#"{{"tool_name":"Write","input":{input},"tool_use_id":"toolu_1"}}"#);
    let resp = s
        .dispatch(
            &CancellationToken::new(),
            &tools_call("1", "permission_request", &args),
            &binding("L", "CD"),
        )
        .await
        .unwrap();
    let Ok(RpcResult::CallTool(res)) = resp.body else {
        panic!()
    };
    assert!(!res.is_error, "{res:?}");
    let got: Value = serde_json::from_str(text(&res)).unwrap();
    assert_eq!(got["behavior"], "allow");
    // updatedInput is required on allow; Recur never rewrites arguments.
    assert_eq!(
        text(&res),
        format!(r#"{{"behavior":"allow","updatedInput":{input}}}"#)
    );
    let asks = cap.asks.lock().unwrap();
    assert_eq!(asks.len(), 1);
    assert_eq!(
        (asks[0].2.tool.as_str(), asks[0].1.as_str()),
        ("Write", "CD")
    );
    assert_eq!(asks[0].2.tool_use_id, "toolu_1");
}

#[tokio::test]
async fn permission_request_deny_carries_reason() {
    let cap = Arc::new(Capture {
        on_ask: Some(Arc::new(|_| {
            tokio::spawn(async {
                Ok(PermissionVerdict {
                    allow: false,
                    reason: "not on this branch".into(),
                })
            })
        })),
        ..Default::default()
    });
    let s = server(&cap);
    let res = call(
        &s,
        &binding("L", "CD"),
        "permission_request",
        json!({"tool_name": "Bash", "input": {"command": "rm -rf /"}}),
    )
    .await;
    // A refusal is a successful verdict, not a tool error.
    assert!(!res.is_error, "{res:?}");
    let got: Value = serde_json::from_str(text(&res)).unwrap();
    assert_eq!(
        got,
        json!({"behavior": "deny", "message": "not on this branch"})
    );
}

#[tokio::test]
async fn permission_request_blocks_and_honors_cancellation() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let entered2 = entered.clone();
    let cap = Arc::new(Capture {
        on_ask: Some(Arc::new(move |cancel: CancellationToken| {
            let entered = entered2.clone();
            tokio::spawn(async move {
                entered.notify_one();
                cancel.cancelled().await; // the agent went away
                Err(HostError::new("context canceled"))
            })
        })),
        ..Default::default()
    });
    let s = Arc::new(server(&cap));
    let cancel = CancellationToken::new();
    let task = {
        let (s, cancel) = (s.clone(), cancel.clone());
        tokio::spawn(async move {
            call_ctx(
                &s,
                &cancel,
                &binding("L", "CD"),
                "permission_request",
                json!({"tool_name": "Bash", "input": {"command": "sleep 1"}}),
            )
            .await
        })
    };
    entered.notified().await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        !task.is_finished(),
        "the handler returned without waiting for a verdict"
    );
    cancel.cancel();
    let res = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("cancelling the request did not release the handler")
        .unwrap();
    assert!(
        res.is_error,
        "an unreachable verdict surfaces as a tool error"
    );
}

#[tokio::test]
async fn permission_request_rejects_missing_tool_name() {
    let s = server(&Arc::new(Capture::default()));
    let res = call(
        &s,
        &binding("L", "CD"),
        "permission_request",
        json!({"input": {}}),
    )
    .await;
    assert!(res.is_error);
}

#[tokio::test]
async fn permission_request_tolerates_unknown_fields() {
    // The payload comes from the CLI, not the model: a field added by a future
    // Claude Code must not break every permission request.
    let s = server(&Arc::new(Capture::default()));
    let res = call(
        &s,
        &binding("L", "CD"),
        "permission_request",
        json!({"tool_name": "Read", "input": {"file_path": "/tmp/x"}, "permission_id": "something-new"}),
    )
    .await;
    assert!(!res.is_error, "{}", text(&res));
}

async fn list_names(s: &McpServer, b: &Binding) -> HashSet<String> {
    let req = RpcRequest {
        jsonrpc: "2.0".into(),
        id: "1".into(),
        method: "tools/list".into(),
        params: String::new(),
    };
    match s
        .dispatch(&CancellationToken::new(), &req, b)
        .await
        .unwrap()
        .body
    {
        Ok(RpcResult::ListTools(tools)) => tools.into_iter().map(|t| t.name).collect(),
        other => panic!("tools/list: {other:?}"),
    }
}

#[tokio::test]
async fn initialize_and_tools_list() {
    let s = server(&Arc::new(Capture::default()));
    let req = RpcRequest {
        jsonrpc: "2.0".into(),
        id: "1".into(),
        method: "initialize".into(),
        params: r#"{"protocolVersion":"2025-06-18"}"#.into(),
    };
    let resp = s
        .dispatch(&CancellationToken::new(), &req, &binding("", ""))
        .await
        .unwrap();
    let Ok(RpcResult::Initialize(ir)) = resp.body else {
        panic!("{resp:?}")
    };
    assert_eq!(ir.protocol_version, "2025-06-18");
    assert_eq!(ir.server_info.name, SERVER_NAME);
    assert!(ir.capabilities.tools.is_some());

    let req = RpcRequest {
        jsonrpc: "2.0".into(),
        id: "2".into(),
        method: "tools/list".into(),
        params: String::new(),
    };
    let Ok(RpcResult::ListTools(tools)) = s
        .dispatch(&CancellationToken::new(), &req, &binding("", ""))
        .await
        .unwrap()
        .body
    else {
        panic!()
    };
    for t in &tools {
        assert!(!t.input_schema.is_empty(), "{} has an empty schema", t.name);
        let probe: Value = serde_json::from_str(&t.input_schema).unwrap();
        assert!(probe.is_object(), "{}", t.name);
    }
    let names: HashSet<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    for want in [
        "report_finding",
        "list_findings",
        "update_finding",
        "done",
        "report_result",
        "plan",
        "escalate",
        "attach_evidence",
        "send_back",
        "list_skills",
        "activate_skill",
        "permission_request",
        "extend_rounds",
        "reenter_phase",
        "run_phase",
    ] {
        assert!(names.contains(want), "tools/list missing {want}");
    }
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn names_from_allowed_tools_scopes() {
    let got = names_from_allowed_tools(&strings(&[
        "Read",
        "Edit",
        "mcp__recur__list_findings",
        "mcp__recur__update_finding",
        "mcp__recur__list_skills",
        "mcp__playwright__browser_navigate",
    ]));
    assert_eq!(
        got,
        strings(&["list_findings", "update_finding", "list_skills"])
    );
    assert!(
        !got.contains(&"permission_request".to_string()),
        "never injected here"
    );
    assert!(!got.contains(&"done".to_string()));
    let with_perm = append_permission_request(got);
    assert!(with_perm.contains(&"permission_request".to_string()));
    assert_eq!(
        append_permission_request(with_perm.clone()).len(),
        with_perm.len(),
        "idempotent"
    );
    // No mcp__recur__ names: empty (scope to nothing), not unrestricted.
    assert!(names_from_allowed_tools(&strings(&["Read", "Glob", "Grep"])).is_empty());
}

const AGENT_TOOLS: [&str; 6] = [
    "Read",
    "Edit",
    "mcp__recur__list_findings",
    "mcp__recur__update_finding",
    "mcp__recur__list_skills",
    "mcp__recur__activate_skill",
];

#[tokio::test]
async fn claude_binding_advertises_and_allows_permission_request() {
    let cap = Arc::new(Capture::default());
    let s = server(&cap);
    let names = append_permission_request(names_from_allowed_tools(&strings(&AGENT_TOOLS)));
    let (token, _) = s.register("W1", "PA", Some(&names));
    let b = s.lookup(&token).expect("registered");
    let listed = list_names(&s, &b).await;
    for want in [
        "list_findings",
        "update_finding",
        "list_skills",
        "activate_skill",
        "permission_request",
    ] {
        assert!(listed.contains(want), "missing {want}: {listed:?}");
    }
    for banned in ["done", "escalate", "plan", "report_finding"] {
        assert!(!listed.contains(banned), "must not advertise {banned}");
    }
    assert!(b.allows("permission_request"));
    let res = call(&s, &b, "done", json!({"summary": "should not work"})).await;
    assert!(res.is_error);
    assert!(cap.dones.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cursor_binding_omits_permission_request() {
    let cap = Arc::new(Capture::default());
    let s = server(&cap);
    let names = names_from_allowed_tools(&strings(&AGENT_TOOLS));
    let (token, _) = s.register("W2", "PA", Some(&names));
    let b = s.lookup(&token).expect("registered");
    let listed = list_names(&s, &b).await;
    assert!(!listed.contains("permission_request"));
    assert!(!b.allows("permission_request"));
    let res = call(
        &s,
        &b,
        "permission_request",
        json!({"tool_name": "Write", "input": {}}),
    )
    .await;
    assert!(res.is_error);
    for want in ["list_findings", "update_finding", "list_skills"] {
        assert!(listed.contains(want), "missing {want}");
    }
}

#[tokio::test]
async fn empty_scope_advertises_nothing_and_refuses_calls() {
    // Cursor PR-responder shape: explore tools only. Must not fall through to
    // unrestricted.
    let cap = Arc::new(Capture::default());
    let s = server(&cap);
    let empty = names_from_allowed_tools(&strings(&["Read", "Glob", "Grep"]));
    let (token, _) = s.register("W3", "PR", Some(&empty));
    let b = s.lookup(&token).expect("registered");
    assert!(
        b.allowed.is_some(),
        "empty scope is restricted, not unrestricted"
    );
    assert!(list_names(&s, &b).await.is_empty());
    let res = call(&s, &b, "done", json!({"summary": "leak"})).await;
    assert!(res.is_error);
    assert!(cap.dones.lock().unwrap().is_empty());
}

#[tokio::test]
async fn list_and_activate_skills() {
    let s = server(&Arc::new(Capture::default()));
    let b = binding("L1", "PA");
    let res = call(&s, &b, "list_skills", json!({})).await;
    assert!(!res.is_error);
    let t = text(&res);
    assert!(
        t.contains("<available_skills>")
            && t.contains("<name>demo-skill</name>")
            && t.contains("<description>A demo</description>"),
        "{t}"
    );
    assert!(
        !t.contains("<location>"),
        "location is omitted when activate_skill exists"
    );
    let res = call(&s, &b, "activate_skill", json!({"name": "demo-skill"})).await;
    assert!(!res.is_error);
    assert!(text(&res).contains(r#"<skill_content name="demo-skill">"#));
    let res = call(&s, &b, "activate_skill", json!({"name": "missing"})).await;
    assert!(res.is_error);
}

#[tokio::test]
async fn list_and_read_terminals() {
    let s = server(&Arc::new(Capture::default()));
    let b = binding("L1", "PA");
    let res = call(&s, &b, "list_terminals", json!({})).await;
    assert!(!res.is_error);
    assert!(text(&res).contains(r#""id":"term-1""#) && text(&res).contains(r#""title":"zsh""#));
    let res = call(&s, &b, "read_terminal", json!({})).await;
    assert!(!res.is_error);
    assert!(text(&res).contains("error: boom"));
    let res = call(&s, &b, "read_terminal", json!({"id": "missing"})).await;
    assert!(res.is_error);
}

#[tokio::test]
async fn read_artifact() {
    let s = server(&Arc::new(Capture::default()));
    let b = binding("L1", "PA");
    let res = call(&s, &b, "read_artifact", json!({"id": "a1"})).await;
    assert!(!res.is_error);
    assert!(text(&res).contains(r#""id":"a1""#) && text(&res).contains("# hi"));
    let res = call(&s, &b, "read_artifact", json!({"id": "missing"})).await;
    assert!(res.is_error);
}

#[tokio::test]
async fn send_back_fires_typed_callback() {
    let cap = Arc::new(Capture::default());
    let s = server(&cap);
    let res = call(
        &s,
        &binding("L-sb", "MA"),
        "send_back",
        json!({"instruction": "rebase onto main and fix conflicts"}),
    )
    .await;
    assert!(!res.is_error);
    assert_eq!(
        *cap.send_backs.lock().unwrap(),
        vec![rec("L-sb", "MA", "rebase onto main and fix conflicts")]
    );
}

#[tokio::test]
async fn send_back_requires_instruction() {
    let s = server(&Arc::new(Capture::default()));
    assert!(
        call(&s, &binding("L", "MA"), "send_back", json!({}))
            .await
            .is_error
    );
}

#[tokio::test]
async fn notification_gets_no_response() {
    let s = server(&Arc::new(Capture::default()));
    let req = RpcRequest {
        jsonrpc: "2.0".into(),
        method: "notifications/initialized".into(),
        ..Default::default()
    };
    assert!(
        s.dispatch(&CancellationToken::new(), &req, &binding("", ""))
            .await
            .is_none()
    );
}

fn call_envelope(name: &str, args: Value) -> String {
    json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args}}).to_string()
}

async fn http_post(url: &str, body: &str) -> u16 {
    let rest = url.strip_prefix("http://127.0.0.1:").unwrap();
    let (port, path) = rest.split_once('/').unwrap();
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port.parse::<u16>().unwrap()))
        .await
        .unwrap();
    let req = format!(
        "POST /{path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).await.unwrap();
    raw.split(' ').nth(1).unwrap().parse().unwrap()
}

// The full HTTP path: attribution by token, unknown token 404, Unregister 404
// until re-Register (same token), Drop revokes.
#[tokio::test]
async fn http_round_trip_with_token_attribution() {
    let cap = Arc::new(Capture::default());
    let s = server(&cap);
    s.start().await.unwrap();
    let (token, url) = s.register("LOOP-9", "RV", None);
    let body = call_envelope(
        "report_finding",
        json!({"file": "x.go", "line": 1, "severity": "medium", "message": "smell"}),
    );
    assert_eq!(http_post(&url, &body).await, 200);
    assert_eq!(cap.counts().0, 1);
    let f = cap.findings.lock().unwrap()[0].clone();
    assert_eq!((f.0.as_str(), f.1.as_str()), ("LOOP-9", "RV"));

    let unknown = format!("http://127.0.0.1:{}/deadbeef/mcp", s.port());
    assert_eq!(http_post(&unknown, &body).await, 404);

    s.unregister(&token);
    assert_eq!(http_post(&url, &body).await, 404, "unregister deactivates");
    let (tok2, url2) = s.register("LOOP-9", "RV", None);
    assert_eq!(
        (tok2.as_str(), url2.as_str()),
        (token.as_str(), url.as_str()),
        "token reused"
    );
    assert_eq!(http_post(&url, &body).await, 200, "re-register reactivates");

    s.drop_token(&token);
    assert_eq!(http_post(&url, &body).await, 404, "dropped token");
    s.close().await.unwrap();
}

#[tokio::test]
async fn register_reuses_token_for_same_agent() {
    let s = server(&Arc::new(Capture::default()));
    s.start().await.unwrap();
    let (tok1, url1) = s.register("W1", "PA", Some(&strings(&["list_skills"])));
    s.unregister(&tok1);
    let (tok2, url2) = s.register("W1", "PA", Some(&strings(&["list_skills", "submit_plan"])));
    assert_eq!((&tok1, &url1), (&tok2, &url2));
    let b = s.lookup(&tok2).unwrap();
    assert!(
        b.active && b.allows("submit_plan"),
        "re-register reactivates and replaces the allowlist"
    );
    let (other, _) = s.register("W1", "PA2", Some(&strings(&["list_skills"])));
    assert_ne!(other, tok1, "different agents never share a token");
    s.drop_agent("W1", "PA");
    let (tok3, _) = s.register("W1", "PA", Some(&strings(&["list_skills"])));
    assert_ne!(tok3, tok1, "drop_agent mints a new token");
    s.close().await.unwrap();
}

#[tokio::test]
async fn register_overlap_mints_distinct_token() {
    let s = server(&Arc::new(Capture::default()));
    s.start().await.unwrap();
    let (tok_chat, _) = s.register("W1", "MA", Some(&strings(&["ask_user"])));
    let (tok_plan, _) = s.register("W1", "MA", Some(&strings(&["escalate"])));
    assert_ne!(
        tok_chat, tok_plan,
        "overlapping register mints an extra token"
    );
    let chat = s.lookup(&tok_chat).unwrap();
    let plan = s.lookup(&tok_plan).unwrap();
    assert!(chat.allows("ask_user") && !chat.allows("escalate"));
    assert!(plan.allows("escalate") && !plan.allows("ask_user"));

    assert_eq!(s.unregister(&tok_plan), Some(("W1".into(), "MA".into())));
    assert!(
        s.lookup(&tok_plan).is_none(),
        "the extra token is dropped on unregister"
    );
    s.unregister(&tok_chat);
    let chat = s.lookup(&tok_chat).expect("canonical token kept");
    assert!(!chat.active);
    let (tok3, _) = s.register("W1", "MA", Some(&strings(&["ask_user"])));
    assert_eq!(tok3, tok_chat);
    assert_eq!(s.unregister("nope"), None);
    s.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_tool_calls_keep_attribution() {
    let cap = Arc::new(Capture::default());
    let s = server(&cap);
    s.start().await.unwrap();
    let (_, url_a) = s.register("LA", "RV", None);
    let (_, url_b) = s.register("LB", "RV", None);
    let body = call_envelope(
        "report_finding",
        json!({"file": "f.go", "severity": "low", "message": "x"}),
    );
    let mut tasks = Vec::new();
    for _ in 0..25 {
        for url in [url_a.clone(), url_b.clone()] {
            let body = body.clone();
            tasks.push(tokio::spawn(async move { http_post(&url, &body).await }));
        }
    }
    for t in tasks {
        assert_eq!(t.await.unwrap(), 200);
    }
    let workspaces: Vec<String> = cap
        .findings
        .lock()
        .unwrap()
        .iter()
        .map(|f| f.0.clone())
        .collect();
    assert_eq!(workspaces.len(), 50);
    let la = workspaces.iter().filter(|w| *w == "LA").count();
    let lb = workspaces.iter().filter(|w| *w == "LB").count();
    assert_eq!((la, lb), (25, 25));
    s.close().await.unwrap();
}

#[tokio::test]
async fn submit_plan_and_update_step() {
    let cap = Arc::new(Capture::default());
    let s = server(&cap);
    let b = binding("L3", "PA");
    let res = call(
        &s,
        &b,
        "submit_plan",
        json!({
            "summary": "Add dark mode",
            "body": "Add a toggle in Settings.svelte and persist the preference.",
            "steps": ["Add the toggle to Settings.svelte", "  ", "Persist the preference"],
        }),
    )
    .await;
    assert!(!res.is_error);
    {
        let plans = cap.submitted_plans.lock().unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(
            (plans[0].ws.as_str(), plans[0].agent.as_str()),
            ("L3", "PA")
        );
        // Blank steps are dropped before the callback.
        assert!(plans[0].summary.contains("Add the toggle") && !plans[0].summary.contains("  ;"));
    }
    let res = call(
        &s,
        &b,
        "submit_plan",
        json!({"summary": "no body", "body": "  ", "steps": ["one"]}),
    )
    .await;
    assert!(res.is_error);
    let res = call(
        &s,
        &b,
        "submit_plan",
        json!({"summary": "no steps", "steps": []}),
    )
    .await;
    assert!(res.is_error);
    assert_eq!(
        cap.submitted_plans.lock().unwrap().len(),
        1,
        "failed submits fire no callback"
    );

    let res = call(
        &s,
        &b,
        "update_plan_step",
        json!({"step_id": "S1", "status": "done"}),
    )
    .await;
    assert!(!res.is_error);
    assert_eq!(
        *cap.step_updates.lock().unwrap(),
        vec![rec("L3", "PA", "S1→done")]
    );
    let res = call(
        &s,
        &b,
        "update_plan_step",
        json!({"step_id": "S1", "status": "finished"}),
    )
    .await;
    assert!(res.is_error);
}

#[tokio::test]
async fn rename_workspace_and_branch_tools() {
    let cap = Arc::new(Capture::default());
    let s = server(&cap);
    let b = binding("L1", "PA");
    let res = call(
        &s,
        &b,
        "rename_workspace",
        json!({"title": "Dark mode toggle"}),
    )
    .await;
    assert!(!res.is_error && text(&res).contains("Dark mode toggle"));
    let res = call(&s, &b, "rename_branch", json!({"name": "add-dark-mode"})).await;
    assert!(!res.is_error && text(&res).contains("add-dark-mode"));
    assert!(call(&s, &b, "rename_workspace", json!({})).await.is_error);
    assert!(call(&s, &b, "rename_branch", json!({})).await.is_error);
    let pl = Binding {
        chat_code: "PA2".into(),
        ..binding("L1", "PL")
    };
    let res = call(&s, &pl, "rename_session", json!({"title": "Dark mode"})).await;
    assert!(!res.is_error);
    assert_eq!(*cap.rename_owner.lock().unwrap(), "PA2");
}

#[tokio::test]
async fn open_pr_tool() {
    let s = server(&Arc::new(Capture::default()));
    let b = binding("L1", "PA");
    let res = call(
        &s,
        &b,
        "open_pr",
        json!({"title": "Agent-written GitHub PR descriptions", "body": "The titlebar Open PR action now invokes /open-pr."}),
    )
    .await;
    assert!(!res.is_error && text(&res).contains("Agent-written GitHub PR descriptions"));
    assert!(
        call(&s, &b, "open_pr", json!({"title": "Only title"}))
            .await
            .is_error
    );
    assert!(
        call(&s, &b, "open_pr", json!({"body": "Only body"}))
            .await
            .is_error
    );
    let draft = call(
        &s,
        &b,
        "open_pr",
        json!({"title": "WIP gateway", "body": "Not ready for review.", "draft": true}),
    )
    .await;
    assert!(!draft.is_error && text(&draft).contains("draft"));
}

// tools_briefing_test.go

#[tokio::test]
async fn briefing_tools_refuse_non_app() {
    let s = server(&Arc::new(Capture::default()));
    let b = binding("", "MA");
    for name in [
        "list_workspace_briefs",
        "get_workspace_context",
        "present_workspace",
    ] {
        let res = call(
            &s,
            &b,
            name,
            json!({"workspace_id": "WS-1", "target": "workspace:WS-1"}),
        )
        .await;
        assert!(
            res.is_error,
            "{name}: expected a tool-level error for a non-APP agent"
        );
        assert!(text(&res).contains("Ask Recur"), "{name}: {res:?}");
    }
}

#[tokio::test]
async fn briefing_tools_app_happy_path() {
    let s = server(&Arc::new(Capture::default()));
    let b = binding("", APP_AGENT_CODE);
    let list = call(&s, &b, "list_workspace_briefs", json!({})).await;
    assert!(!list.is_error && text(&list).contains("WS-1"), "{list:?}");
    let ctx = call(
        &s,
        &b,
        "get_workspace_context",
        json!({"workspace_id": "WS-9"}),
    )
    .await;
    assert!(!ctx.is_error && text(&ctx).contains("WS-9"), "{ctx:?}");
    let present = call(
        &s,
        &b,
        "present_workspace",
        json!({"target": "workspace:WS-1"}),
    )
    .await;
    assert!(
        !present.is_error && text(&present).contains("workspace:WS-1"),
        "{present:?}"
    );
}

// wrap_test.go

#[test]
fn all_tools_wraps_host_tools() {
    let host = host_tools();
    let specs = all_tools();
    assert_eq!(specs.len(), host.len());
    for (spec, t) in specs.iter().zip(&host) {
        assert_eq!(spec.name, t.name());
    }
}

// Rust-side guarantees of the Go contract: an HTTP client that disconnects
// mid-wait cancels the permission wait (Go: request context), and start/close
// lifecycle text.

#[tokio::test]
async fn client_disconnect_cancels_permission_wait() {
    let released = Arc::new(tokio::sync::Notify::new());
    let entered = Arc::new(tokio::sync::Notify::new());
    let (released2, entered2) = (released.clone(), entered.clone());
    let cap = Arc::new(Capture {
        on_ask: Some(Arc::new(move |cancel: CancellationToken| {
            let (released, entered) = (released2.clone(), entered2.clone());
            tokio::spawn(async move {
                entered.notify_one();
                cancel.cancelled().await;
                released.notify_one();
                Err(HostError::new("context canceled"))
            })
        })),
        ..Default::default()
    });
    let s = server(&cap);
    s.start().await.unwrap();
    let (_, url) = s.register("L", "CD", None);
    let body = call_envelope(
        "permission_request",
        json!({"tool_name": "Bash", "input": {}}),
    );
    let path = url
        .split_once(&format!("{}", s.port()))
        .unwrap()
        .1
        .to_string();
    let mut conn = tokio::net::TcpStream::connect(("127.0.0.1", s.port()))
        .await
        .unwrap();
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    conn.write_all(req.as_bytes()).await.unwrap();
    entered.notified().await;
    drop(conn);
    tokio::time::timeout(Duration::from_secs(2), released.notified())
        .await
        .expect("a dead client releases the permission waiter");
    s.close().await.unwrap();
}

#[tokio::test]
async fn start_reports_bind_failure_and_close_is_idempotent() {
    let a = server(&Arc::new(Capture::default()));
    a.start().await.unwrap();
    let taken = format!("127.0.0.1:{}", a.port());
    let b = McpServer::new(None).with_addr(taken.clone());
    let err = b.start().await.unwrap_err();
    assert!(
        err.0
            .starts_with(&format!("mcp listen: listen tcp {taken}: bind: ")),
        "{err}"
    );
    assert_eq!(b.port(), 0);
    assert_eq!(b.url_for("tok"), "http://127.0.0.1:0/tok/mcp");
    a.close().await.unwrap();
    a.close().await.unwrap();
    assert!(
        McpServer::new(None).close().await.is_ok(),
        "close before start"
    );
}
