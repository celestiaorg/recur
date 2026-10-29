//! Snapshot of every host-tool case (`golden/host_calls.json`) against the Rust
//! tools: result text (JSON as values), error text (decode wording excepted)
//! and the host calls. Edit the fixture by hand when the behavior changes on
//! purpose.

use crate::common;

use std::sync::{Arc, Mutex};

use recur_agent_tools::{
    AskUserRequest, CreateArtifactRequest, CreateWorkspaceRequest, EscalationOption,
    EscalationRequest, Evidence, Finding, Host, HostError, PermissionAsk, PermissionVerdict,
    PlanSubmission, TerminalInfo, host_tools,
};
use recur_domain::{
    AgentSkill, Artifact, ArtifactKind, BriefAction, BriefingCounts, BriefingSnapshot,
    Finding as StoredFinding, FindingNote, PresentWorkspaceResult, Severity, WorkspaceBrief,
    WorkspaceContext,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Rec {
    calls: Mutex<Vec<Value>>,
}

impl Rec {
    fn rec(&self, v: Value) {
        self.calls.lock().unwrap().push(v);
    }
}

fn opts(o: &[EscalationOption]) -> Value {
    o.iter()
        .map(|x| json!({"text": x.text, "tag": x.tag, "desc": x.desc}))
        .collect()
}

#[async_trait::async_trait]
impl Host for Rec {
    fn report_finding(&self, ws: &str, a: &str, f: Finding) -> StoredFinding {
        self.rec(json!({"method": "ReportFinding", "ws": ws, "agent": a, "file": f.file, "line": f.line, "endLine": f.end_line, "severity": f.severity.as_str(), "message": f.message, "summary": f.summary}));
        StoredFinding {
            id: "uuid-1".into(),
            short_id: "find_1".into(),
            workspace_id: ws.into(),
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

    fn done(&self, ws: &str, a: &str, summary: &str, notes: &str) -> Result<(), HostError> {
        self.rec(
            json!({"method": "Done", "ws": ws, "agent": a, "summary": summary, "notes": notes}),
        );
        if summary == "blocked" {
            return Err(HostError::new("2 findings are still open"));
        }
        Ok(())
    }

    fn list_findings(
        &self,
        ws: &str,
        a: &str,
        status: &str,
    ) -> Result<Vec<StoredFinding>, HostError> {
        self.rec(json!({"method": "ListFindings", "ws": ws, "agent": a, "status": status}));
        match status {
            "invalid" => return Err(HostError::new("store offline")),
            "ignored" => return Ok(Vec::new()),
            _ => {}
        }
        Ok(vec![
            StoredFinding {
                id: "uuid-a".into(),
                short_id: "find_a".into(),
                file: "a.go".into(),
                line: 3,
                end_line: 9,
                severity: Severity::High,
                message: "x < y && z".into(),
                summary: "sum".into(),
                status: "open".into(),
                updated_by: "coder".into(),
                notes: vec![FindingNote {
                    id: "n1".into(),
                    author: "human".into(),
                    body: "look <here>".into(),
                    created_at: "2026-01-02T03:04:05Z".into(),
                }],
                ..Default::default()
            },
            StoredFinding {
                id: "uuid-b".into(),
                file: "b.go".into(),
                severity: Severity::Low,
                message: "m".into(),
                status: "resolved".into(),
                status_reason: "fixed".into(),
                ..Default::default()
            },
        ])
    }

    fn update_finding(
        &self,
        ws: &str,
        a: &str,
        id: &str,
        status: &str,
        reason: &str,
    ) -> Result<StoredFinding, HostError> {
        self.rec(json!({"method": "UpdateFinding", "ws": ws, "agent": a, "id": id, "status": status, "reason": reason}));
        if id == "missing" {
            return Err(HostError::new(format!("finding \"{id}\" not found")));
        }
        let mut f = StoredFinding {
            id: format!("uuid-{id}"),
            short_id: id.into(),
            file: "c.go".into(),
            line: 7,
            severity: Severity::Medium,
            message: "msg".into(),
            status: status.into(),
            status_reason: reason.into(),
            updated_by: "coder".into(),
            ..Default::default()
        };
        if id == "noted" {
            f.notes = vec![FindingNote {
                id: "n2".into(),
                author: "human".into(),
                body: "b".into(),
                created_at: "t".into(),
            }];
        }
        Ok(f)
    }

    fn report_result(&self, ws: &str, a: &str, passed: bool, summary: &str) {
        self.rec(json!({"method": "ReportResult", "ws": ws, "agent": a, "passed": passed, "summary": summary}));
    }

    fn plan(&self, ws: &str, a: &str, summary: &str) {
        self.rec(json!({"method": "Plan", "ws": ws, "agent": a, "summary": summary}));
    }

    fn escalate(&self, ws: &str, a: &str, e: EscalationRequest) {
        self.rec(json!({"method": "Escalate", "ws": ws, "agent": a, "question": e.question, "context": e.context, "options": opts(&e.options)}));
    }

    fn attach_evidence(&self, ws: &str, a: &str, e: Evidence) {
        self.rec(json!({"method": "AttachEvidence", "ws": ws, "agent": a, "step": e.step, "screenshotPath": e.screenshot_path, "passed": e.passed}));
    }

    fn note_tool_args(&self, ws: &str, a: &str, tool: &str, args: &str) {
        self.rec(
            json!({"method": "NoteToolArgs", "ws": ws, "agent": a, "tool": tool, "args": args}),
        );
    }

    async fn ask_user(&self, ws: &str, a: &str, r: AskUserRequest) -> String {
        self.rec(json!({"method": "AskUser", "ws": ws, "agent": a, "question": r.question, "context": r.context, "options": opts(&r.options)}));
        "asked".into()
    }

    async fn switch_mode(&self, ws: &str, a: &str, mode: &str) -> String {
        self.rec(json!({"method": "SwitchMode", "ws": ws, "agent": a, "mode": mode}));
        format!("Switched to {mode}; remounting")
    }

    async fn create_artifact(
        &self,
        ws: &str,
        a: &str,
        r: CreateArtifactRequest,
    ) -> Result<Artifact, HostError> {
        self.rec(json!({"method": "CreateArtifact", "ws": ws, "agent": a, "title": r.title, "kind": r.kind, "content": r.content}));
        if r.title == "fail" {
            return Err(HostError::new("create_artifact: disk full"));
        }
        Ok(Artifact {
            id: "art-1".into(),
            workspace_id: ws.into(),
            title: r.title,
            kind: ArtifactKind::from(r.kind.as_str()),
            path: format!("workspaces/{ws}/artifacts/art-1.{}", r.kind),
            created_at: "2026-01-01T00:00:00Z".into(),
            created_by: a.into(),
        })
    }

    async fn list_artifacts(&self, ws: &str, a: &str) -> Result<Vec<Artifact>, HostError> {
        self.rec(json!({"method": "ListArtifacts", "ws": ws, "agent": a}));
        match ws {
            "none" => Ok(Vec::new()),
            "err" => Err(HostError::new("artifacts unavailable")),
            _ => Ok(vec![Artifact {
                id: "art-1".into(),
                workspace_id: ws.into(),
                title: "<Chart> & co".into(),
                kind: ArtifactKind::Svg,
                path: "p".into(),
                created_at: "t".into(),
                created_by: String::new(),
            }]),
        }
    }

    async fn read_artifact(&self, ws: &str, a: &str, id: &str) -> Result<String, HostError> {
        self.rec(json!({"method": "ReadArtifact", "ws": ws, "agent": a, "id": id}));
        if id == "missing" {
            return Err(HostError::new(format!("artifact not found: {id}")));
        }
        Ok(format!("body of {id}"))
    }

    async fn present_artifact(&self, ws: &str, a: &str, id: &str) -> String {
        self.rec(json!({"method": "PresentArtifact", "ws": ws, "agent": a, "id": id}));
        format!("Presented {id}")
    }

    async fn rename_session(&self, ws: &str, a: &str, owner: &str, title: &str) -> String {
        self.rec(json!({"method": "RenameSession", "ws": ws, "agent": a, "owner": owner, "title": title}));
        format!("Renamed to {title}")
    }

    async fn rename_workspace(&self, ws: &str, a: &str, title: &str) -> String {
        self.rec(json!({"method": "RenameWorkspace", "ws": ws, "agent": a, "title": title}));
        format!("Renamed workspace to {title}")
    }

    async fn rename_branch(&self, ws: &str, a: &str, name: &str) -> String {
        self.rec(json!({"method": "RenameBranch", "ws": ws, "agent": a, "name": name}));
        format!("Renamed branch to {name}")
    }

    async fn open_pr(&self, ws: &str, a: &str, title: &str, body: &str, draft: bool) -> String {
        self.rec(json!({"method": "OpenPR", "ws": ws, "agent": a, "title": title, "body": body, "draft": draft}));
        format!("Opened PR: {title}")
    }

    async fn create_workspace(&self, ws: &str, a: &str, r: CreateWorkspaceRequest) -> String {
        self.rec(json!({"method": "CreateWorkspace", "ws": ws, "agent": a, "projectId": r.project_id, "title": r.title, "prompt": r.prompt, "baseBranch": r.base_branch, "mode": r.mode, "loopId": r.loop_id, "policyId": r.policy_id, "standalone": r.standalone}));
        format!("created {}", r.title)
    }

    async fn merge_child(&self, ws: &str, a: &str, child: &str) -> String {
        self.rec(json!({"method": "MergeChild", "ws": ws, "agent": a, "child": child}));
        format!("merged {child}")
    }

    async fn read_child(&self, ws: &str, a: &str, child: &str) -> String {
        self.rec(json!({"method": "ReadChild", "ws": ws, "agent": a, "child": child}));
        format!("report {child}")
    }

    async fn archive_child(&self, ws: &str, a: &str, child: &str) -> String {
        self.rec(json!({"method": "ArchiveChild", "ws": ws, "agent": a, "child": child}));
        format!("archived {child}")
    }

    async fn message_child(&self, ws: &str, a: &str, child: &str, msg: &str) -> String {
        self.rec(
            json!({"method": "MessageChild", "ws": ws, "agent": a, "child": child, "message": msg}),
        );
        format!("steered {child}")
    }

    async fn read_child_artifact(
        &self,
        ws: &str,
        a: &str,
        child: &str,
        id: &str,
    ) -> Result<String, HostError> {
        self.rec(
            json!({"method": "ReadChildArtifact", "ws": ws, "agent": a, "child": child, "id": id}),
        );
        if id == "missing" {
            return Err(HostError::new("child artifact not found"));
        }
        Ok(format!("child body {id}"))
    }

    async fn request_permission(
        &self,
        _cancel: &CancellationToken,
        ws: &str,
        a: &str,
        ask: PermissionAsk,
    ) -> Result<PermissionVerdict, HostError> {
        self.rec(json!({"method": "RequestPermission", "ws": ws, "agent": a, "tool": ask.tool, "input": ask.input, "toolUseId": ask.tool_use_id}));
        match ask.tool.as_str() {
            "Deny" => Ok(PermissionVerdict {
                allow: false,
                reason: "  not here  ".into(),
            }),
            "Blank" => Ok(PermissionVerdict::default()),
            "Err" => Err(HostError::new("broker down")),
            _ => Ok(PermissionVerdict {
                allow: true,
                reason: String::new(),
            }),
        }
    }

    async fn send_back(&self, ws: &str, a: &str, instr: &str) -> String {
        self.rec(json!({"method": "SendBack", "ws": ws, "agent": a, "instruction": instr}));
        if instr == "empty" {
            return String::new();
        }
        "Sent back.".into()
    }

    async fn edit_plan(&self, ws: &str, a: &str, plan: &str) -> String {
        self.rec(json!({"method": "EditPlan", "ws": ws, "agent": a, "plan": plan}));
        "Plan updated.".into()
    }

    async fn rerun_review(&self, ws: &str, a: &str) -> String {
        self.rec(json!({"method": "RerunReview", "ws": ws, "agent": a}));
        "Reviewer dispatched.".into()
    }

    async fn dispatch_specialist(&self, ws: &str, a: &str, role: &str, instr: &str) -> String {
        self.rec(json!({"method": "DispatchSpecialist", "ws": ws, "agent": a, "role": role, "instruction": instr}));
        "Specialist dispatched.".into()
    }

    async fn extend_iterations(&self, ws: &str, a: &str, n: i64) -> String {
        self.rec(json!({"method": "ExtendIterations", "ws": ws, "agent": a, "n": n}));
        format!("Granted {n}.")
    }

    async fn extend_rounds(&self, ws: &str, a: &str, n: i64) -> String {
        self.rec(json!({"method": "ExtendRounds", "ws": ws, "agent": a, "n": n}));
        format!("Rounds {n}.")
    }

    async fn reenter_phase(&self, ws: &str, a: &str, phase: &str) -> String {
        self.rec(json!({"method": "ReenterPhase", "ws": ws, "agent": a, "phase": phase}));
        format!("Reentered {phase}")
    }

    async fn run_phase(&self, ws: &str, a: &str, phase: &str) -> String {
        self.rec(json!({"method": "RunPhase", "ws": ws, "agent": a, "phase": phase}));
        format!("Running {phase}")
    }

    async fn resume_loop(&self, ws: &str, a: &str) -> String {
        self.rec(json!({"method": "ResumeLoop", "ws": ws, "agent": a}));
        "Loop restarting.".into()
    }

    async fn submit_plan(&self, ws: &str, a: &str, p: PlanSubmission) -> String {
        let meta: Vec<Value> = p
            .step_meta
            .iter()
            .map(|m| json!({"files": m.files, "dependsOn": m.depends_on, "explicit": m.explicit}))
            .collect();
        self.rec(json!({"method": "SubmitPlan", "ws": ws, "agent": a, "summary": p.summary, "body": p.body, "steps": p.steps, "stepIds": p.step_ids, "attachments": p.attachment_ids, "stepMeta": meta}));
        "Plan recorded.".into()
    }

    async fn update_plan_step(
        &self,
        ws: &str,
        a: &str,
        id: &str,
        status: &str,
        note: &str,
    ) -> String {
        self.rec(json!({"method": "UpdatePlanStep", "ws": ws, "agent": a, "id": id, "status": status, "note": note}));
        format!("Step {id} → {status}.")
    }

    async fn list_skills(&self, ws: &str, a: &str) -> Result<Vec<AgentSkill>, HostError> {
        self.rec(json!({"method": "ListSkills", "ws": ws, "agent": a}));
        if ws == "err" {
            return Err(HostError::new("skills unavailable"));
        }
        Ok(vec![
            AgentSkill {
                name: "go-review".into(),
                description: "Review <Go> & tests".into(),
                scope: "user".into(),
                ..Default::default()
            },
            AgentSkill {
                name: "bare".into(),
                scope: "workspace".into(),
                ..Default::default()
            },
        ])
    }

    async fn activate_skill(&self, ws: &str, a: &str, name: &str) -> Result<String, HostError> {
        self.rec(json!({"method": "ActivateSkill", "ws": ws, "agent": a, "name": name}));
        if name == "missing" {
            return Err(HostError::new(format!("unknown skill \"{name}\"")));
        }
        Ok(format!(
            "<skill_content name=\"{name}\">body</skill_content>"
        ))
    }

    async fn list_terminals(&self, ws: &str, a: &str) -> Option<Vec<TerminalInfo>> {
        self.rec(json!({"method": "ListTerminals", "ws": ws, "agent": a}));
        if ws == "none" {
            return None;
        }
        Some(vec![
            TerminalInfo {
                id: "t1".into(),
                title: "zsh".into(),
                cwd: "/w".into(),
                shell: "/bin/zsh".into(),
                active: true,
                ..Default::default()
            },
            TerminalInfo {
                id: "t2".into(),
                title: "dev <srv>".into(),
                cwd: "/w".into(),
                exited: true,
                exit_code: Some(0),
                ..Default::default()
            },
        ])
    }

    async fn read_terminal(
        &self,
        ws: &str,
        a: &str,
        id: &str,
        tail: i64,
    ) -> Result<String, HostError> {
        self.rec(json!({"method": "ReadTerminal", "ws": ws, "agent": a, "id": id, "tail": tail}));
        if id == "missing" {
            return Err(HostError::new(format!("terminal \"{id}\" not found")));
        }
        Ok("out\n".into())
    }

    async fn list_workspace_briefs(&self) -> BriefingSnapshot {
        self.rec(json!({"method": "ListWorkspaceBriefs"}));
        BriefingSnapshot {
            counts: BriefingCounts {
                needs: 1,
                pickup: 2,
                ..Default::default()
            },
            workspaces: Some(vec![WorkspaceBrief {
                workspace_id: "WS-1".into(),
                title: "Needs <a> look".into(),
                bucket: "needs".into(),
                reasons: Some(vec!["plan_approval".into()]),
                primary: BriefAction {
                    label: "Open".into(),
                    target: "workspace:WS-1".into(),
                    ..Default::default()
                },
                ..Default::default()
            }]),
            ..Default::default()
        }
    }

    async fn workspace_context(&self, id: &str) -> Result<WorkspaceContext, HostError> {
        self.rec(json!({"method": "WorkspaceContext", "id": id}));
        if id == "missing" {
            return Err(HostError::new("workspace not found"));
        }
        Ok(WorkspaceContext {
            brief: WorkspaceBrief {
                workspace_id: id.into(),
                title: "ctx & more".into(),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn present_workspace(&self, target: &str) -> String {
        self.rec(json!({"method": "PresentWorkspace", "target": target}));
        serde_json::to_string(&PresentWorkspaceResult {
            ok: true,
            target: target.into(),
            ..Default::default()
        })
        .unwrap()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Case {
    tool: String,
    agent: String,
    ws: String,
    chat: String,
    args: String,
    #[serde(default)]
    no_host: bool,
    result: String,
    error: String,
    is_error: bool,
    calls: Vec<Value>,
}

/// Host calls with JSON-text fields (raw tool input) decoded, so compacted
/// and verbatim input compare equal.
pub fn embedded_json(v: Value) -> Value {
    match v {
        Value::String(s) if s.starts_with(['{', '[']) => {
            serde_json::from_str(&s).unwrap_or(Value::String(s))
        }
        Value::Array(a) => a.into_iter().map(embedded_json).collect(),
        Value::Object(m) => m.into_iter().map(|(k, v)| (k, embedded_json(v))).collect(),
        other => other,
    }
}

/// Go `encoding/json` quirks Recur does not keep: Unicode case folding of
/// keys, duplicate keys merging, and trailing data after the arguments.
const GO_ONLY_ARGS: &[&str] = &[
    r#"{"ſeverity":"low","file":"k.go","message":"x"}"#,
    r#"{"file":"a.go","file":null,"severity":"low","message":"x"}"#,
    r#"{"file":"a.go","severity":"low","message":"x"} trailing"#,
    r#"{"question":"q","options":[{"text":"a","tag":"t"}],"options":[{"text":"b"}]}"#,
];

#[tokio::test]
async fn host_tools_match_go() {
    let cases: Vec<Case> = serde_json::from_str(include_str!("../golden/host_calls.json")).unwrap();
    let tools = host_tools();
    let mut failures = Vec::new();
    for c in cases
        .iter()
        .filter(|c| !GO_ONLY_ARGS.contains(&c.args.as_str()))
    {
        let tool = tools
            .iter()
            .find(|t| t.name() == c.tool)
            .expect("tool in catalog");
        let rec = Arc::new(Rec::default());
        let host: Option<Arc<dyn Host>> = if c.no_host { None } else { Some(rec.clone()) };
        let got = tool
            .execute(recur_agent_tools::Call {
                workspace_id: c.ws.clone(),
                agent_code: c.agent.clone(),
                chat_code: c.chat.clone(),
                args: c.args.clone(),
                host,
                ..Default::default()
            })
            .await;
        let (result, error) = match got {
            Ok(r) => (r, String::new()),
            Err(e) => (String::new(), e.0),
        };
        let calls = rec.calls.lock().unwrap().clone();
        if !common::same_result(&result, &c.result)
            || !common::same_error(&error, &c.error)
            || error.is_empty() == c.is_error
            || embedded_json(calls.clone().into()) != embedded_json(c.calls.clone().into())
        {
            failures.push(format!(
                "{} {}\n  want result={:?} error={:?} calls={}\n  got  result={:?} error={:?} calls={}",
                c.tool,
                c.args,
                c.result,
                c.error,
                Value::from(c.calls.clone()),
                result,
                error,
                Value::from(calls)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases differ:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}
