//! Loop test doubles (Go `convergeRunner`, `verifyRunner`, `headlineRunner`,
//! the MCP posting helpers and `verifyDevProject`): providers that drive a
//! walk through the REAL MCP HTTP path (report_finding, list/update_finding,
//! attach_evidence, report_result) the production agents use, with
//! everything but the turn borrowed from the real Claude provider.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use parking_lot::Mutex;
use recur_agent::{
    AgentEvent, Availability, Flags, ModelChoice, Provider, ProviderCaps, ProviderError,
    ProviderStatus, ToolDisplay, ToolStep, TurnContext, TurnRequest, TurnResult,
};
use recur_domain::{AgentRole, Project, RunCommand, Settings};
use recur_store::Store;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::testing::add_run_project;
use crate::turn::test_support::claude;

type TurnFuture = Pin<Box<dyn Future<Output = Result<TurnResult, ProviderError>> + Send>>;
type TurnFn = Arc<dyn Fn(TurnRequest, TurnContext) -> TurnFuture + Send + Sync>;

/// Claude's id, caps and display around an async scripted turn.
pub struct AsyncProvider {
    inner: Arc<dyn Provider>,
    run: TurnFn,
}

impl AsyncProvider {
    pub fn wrap<F, Fut>(run: F) -> Arc<dyn Provider>
    where
        F: Fn(TurnRequest, TurnContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<TurnResult, ProviderError>> + Send + 'static,
    {
        Arc::new(Self {
            inner: claude(),
            run: Arc::new(move |req, cx| Box::pin(run(req, cx))),
        })
    }
}

#[async_trait::async_trait]
impl Provider for AsyncProvider {
    fn id(&self) -> &str {
        self.inner.id()
    }
    fn label(&self) -> &str {
        self.inner.label()
    }
    fn caps(&self) -> &ProviderCaps {
        self.inner.caps()
    }
    fn availability(&self, _s: &Settings) -> Availability {
        Availability::Ready
    }
    async fn status(&self, s: &Settings) -> ProviderStatus {
        self.inner.status(s).await
    }
    async fn models(&self, s: &Settings) -> Result<Vec<ModelChoice>, ProviderError> {
        self.inner.models(s).await
    }
    fn cached_models(&self, s: &Settings) -> Vec<ModelChoice> {
        self.inner.cached_models(s)
    }
    fn resolve_model(&self, m: &str, f: &Flags) -> String {
        self.inner.resolve_model(m, f)
    }
    fn context_window(&self, m: &str, f: &Flags) -> i64 {
        self.inner.context_window(m, f)
    }
    fn model_label(&self, m: &str) -> String {
        self.inner.model_label(m)
    }
    fn oneshot_flags(&self, m: &str) -> Flags {
        self.inner.oneshot_flags(m)
    }
    async fn run_turn(
        &self,
        req: TurnRequest,
        cx: TurnContext,
    ) -> Result<TurnResult, ProviderError> {
        (self.run)(req, cx).await
    }
    fn resume_handle_fits(&self, h: &str) -> bool {
        self.inner.resume_handle_fits(h)
    }
    fn display_tool(&self, n: &str, i: &str) -> ToolDisplay {
        self.inner.display_tool(n, i)
    }
    fn display_tool_result(&self, n: &str, r: &str) -> String {
        self.inner.display_tool_result(n, r)
    }
}

/// A success result for `session_id` with `text`.
pub fn success(session_id: &str, text: &str) -> TurnResult {
    TurnResult {
        subtype: "success".into(),
        session_id: session_id.into(),
        text: text.into(),
        ..TurnResult::default()
    }
}

/// Writes repo-relative files into the worktree (Go `ScriptedRunner.Edits`).
pub fn apply_edits(worktree: &Path, edits: &[(&str, &str)]) -> std::io::Result<()> {
    if worktree.as_os_str().is_empty() {
        return Ok(());
    }
    for (rel, body) in edits {
        let p = worktree.join(rel);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(p, body)?;
    }
    Ok(())
}

/// Emits one tool step (Go `emit(ParsedEvent{Kind: KindStep, ...})`).
pub fn emit_step(cx: &TurnContext, id: &str, name: &str, summary: &str, input: &str) {
    cx.sink.emit(AgentEvent::Step(ToolStep {
        id: id.into(),
        name: name.into(),
        summary: summary.into(),
        input: input.into(),
        ..ToolStep::default()
    }));
}

fn failed(e: impl std::fmt::Display) -> ProviderError {
    ProviderError::failed(e.to_string())
}

/// POSTs a tools/call to the MCP URL and returns the first text block
/// (Go `postMCPToolResult`). An `isError` result is an error.
pub async fn post_mcp_tool_result(url: &str, name: &str, args: Value) -> Result<String, String> {
    let body = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": name, "arguments": args},
    });
    let resp = reqwest::Client::new()
        .post(url)
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let v: Value = resp.json().await.map_err(|e| e.to_string())?;
    if let Some(err) = v.get("error") {
        return Err(format!(
            "mcp {name}: {}",
            err["message"].as_str().unwrap_or("")
        ));
    }
    let text = v["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    if v["result"]["isError"].as_bool() == Some(true) {
        return Err(format!("mcp {name}: {text}"));
    }
    Ok(text)
}

pub async fn post_mcp_tool(url: &str, name: &str, args: Value) -> Result<(), String> {
    post_mcp_tool_result(url, name, args).await.map(|_| ())
}

/// A report_finding through the real MCP endpoint.
pub async fn post_finding(
    url: &str,
    file: &str,
    line: i64,
    severity: &str,
    message: &str,
) -> Result<(), String> {
    post_mcp_tool(
        url,
        "report_finding",
        json!({"file": file, "line": line, "severity": severity, "message": message}),
    )
    .await
}

/// The finding ids in a list_findings result (JSON, or the legacy prose
/// `id=…` lines).
pub fn parse_finding_ids(text: &str) -> Vec<String> {
    if let Ok(v) = serde_json::from_str::<Value>(text)
        && let Some(arr) = v["findings"].as_array()
    {
        return arr
            .iter()
            .filter_map(|f| f["id"].as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
    }
    let mut ids = Vec::new();
    for line in text.split('\n') {
        let Some(i) = line.find("id=") else {
            continue;
        };
        let rest = &line[i + 3..];
        let end = rest.find([' ', '\t']).unwrap_or(rest.len());
        ids.push(rest[..end].to_owned());
    }
    ids
}

/// Lists open findings and resolves each (the Coder's checklist).
pub async fn close_open_findings_mcp(url: &str) -> Result<(), String> {
    let text = post_mcp_tool_result(url, "list_findings", json!({"status": "open"})).await?;
    for id in parse_finding_ids(&text) {
        post_mcp_tool(
            url,
            "update_finding",
            json!({"finding_id": id, "status": "resolved"}),
        )
        .await?;
    }
    Ok(())
}

/// An attach_evidence call (its result is ignored, as in Go).
pub async fn post_evidence(url: &str, step: &str, shot: &str, passed: bool) -> Result<(), String> {
    let _ = post_mcp_tool_result(
        url,
        "attach_evidence",
        json!({"step": step, "screenshot_path": shot, "passed": passed}),
    )
    .await;
    Ok(())
}

pub async fn post_report_result(url: &str, passed: bool) -> Result<(), String> {
    let _ = post_mcp_tool_result(
        url,
        "report_result",
        json!({"passed": passed, "summary": "verify"}),
    )
    .await;
    Ok(())
}

/// One verify evidence step.
#[derive(Debug, Clone)]
pub struct StepEvidence {
    pub step: &'static str,
    pub shot: &'static str,
    pub passed: bool,
}

/// Go `convergeRunner`'s counters.
#[derive(Debug, Default)]
pub struct ConvergeState {
    pub coder_runs: usize,
    pub review_runs: usize,
    pub qa_runs: usize,
    pub review_iter: usize,
    pub verify_run: usize,
    pub saw_reviewer: Vec<String>,
    pub saw_coder: Vec<String>,
    pub qa_playwright: bool,
    pub qa_tools: Vec<String>,
    pub specs: Vec<TurnRequest>,
}

/// Drives converge (and verify) through the real MCP path: the Coder edits
/// `file` and resolves open findings, the Reviewer files
/// `findings_per_iter[i]` findings on review i+1, QA files the run's
/// evidence and its report_result; role-less one-shots reply `commit_text`.
pub struct ConvergeRunner {
    pub findings_per_iter: Vec<usize>,
    pub evidence_per_run: Vec<Vec<StepEvidence>>,
    /// The same evidence on every verify run (headline runner).
    pub evidence_always: Vec<StepEvidence>,
    pub commit_text: String,
    pub file: &'static str,
    pub file_body: &'static str,
    pub severity: &'static str,
    pub message: &'static str,
    pub state: Mutex<ConvergeState>,
}

impl Default for ConvergeRunner {
    fn default() -> Self {
        Self {
            findings_per_iter: Vec::new(),
            evidence_per_run: Vec::new(),
            evidence_always: Vec::new(),
            commit_text: String::new(),
            file: "work.txt",
            file_body: "iteration edit\n",
            severity: "medium",
            message: "issue",
            state: Mutex::new(ConvergeState::default()),
        }
    }
}

impl ConvergeRunner {
    pub fn new(findings: &[usize]) -> Arc<Self> {
        Arc::new(Self {
            findings_per_iter: findings.to_vec(),
            ..Self::default()
        })
    }

    pub fn provider(self: &Arc<Self>) -> Arc<dyn Provider> {
        let me = Arc::clone(self);
        AsyncProvider::wrap(move |req, cx| {
            let me = Arc::clone(&me);
            async move { me.run(req, cx).await }
        })
    }

    pub fn counts(&self) -> (usize, usize) {
        let s = self.state.lock();
        (s.coder_runs, s.review_runs)
    }

    async fn run(&self, req: TurnRequest, cx: TurnContext) -> Result<TurnResult, ProviderError> {
        self.state.lock().specs.push(req.clone());
        match req.role {
            AgentRole::Coder => {
                {
                    let mut s = self.state.lock();
                    s.coder_runs += 1;
                    s.saw_coder.push(req.agent_id.clone());
                }
                apply_edits(&req.worktree, &[(self.file, self.file_body)]).map_err(failed)?;
                emit_step(
                    &cx,
                    &format!("tu-edit-{}", req.agent_id),
                    "Edit",
                    self.file,
                    &format!(r#"{{"file_path":"{}"}}"#, self.file),
                );
                if !req.mcp_url.is_empty() {
                    close_open_findings_mcp(&req.mcp_url)
                        .await
                        .map_err(failed)?;
                }
                Ok(success(&req.agent_id, ""))
            }
            AgentRole::Reviewer => {
                let iter = {
                    let mut s = self.state.lock();
                    s.review_runs += 1;
                    s.review_iter += 1;
                    s.saw_reviewer.push(req.agent_id.clone());
                    s.review_iter
                };
                let n = self.findings_per_iter.get(iter - 1).copied().unwrap_or(0);
                for i in 0..n {
                    post_finding(
                        &req.mcp_url,
                        self.file,
                        i as i64 + 1,
                        self.severity,
                        self.message,
                    )
                    .await
                    .map_err(failed)?;
                }
                Ok(success(&req.agent_id, ""))
            }
            AgentRole::Qa => {
                let run = {
                    let mut s = self.state.lock();
                    s.qa_runs += 1;
                    let r = s.verify_run;
                    s.verify_run += 1;
                    s.qa_playwright = req.playwright;
                    s.qa_tools = req.allowed_tools.clone().unwrap_or_default();
                    r
                };
                let steps = if self.evidence_always.is_empty() {
                    self.evidence_per_run.get(run).cloned().unwrap_or_default()
                } else {
                    self.evidence_always.clone()
                };
                let mut all_ok = !steps.is_empty();
                for s in &steps {
                    post_evidence(&req.mcp_url, s.step, s.shot, s.passed)
                        .await
                        .map_err(failed)?;
                    if !s.passed {
                        all_ok = false;
                    }
                }
                if !req.mcp_url.is_empty() {
                    post_report_result(&req.mcp_url, all_ok)
                        .await
                        .map_err(failed)?;
                }
                Ok(success(&req.agent_id, ""))
            }
            _ => Ok(success(&req.agent_id, &self.commit_text)),
        }
    }
}

/// Sets the Settings default policy.
pub fn set_default_policy(store: &Store, id: &str) {
    let mut set = store.settings();
    set.default_policy = id.into();
    store.set_settings(set);
}

#[allow(dead_code)]
/// A project whose dev command serves HTTP on $RECUR_PORT so verify can
/// boot (Go `verifyDevProject`). `None` when python3 is missing.
pub fn verify_dev_project(store: &Store, repo: &str) -> Option<String> {
    let has_python = std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !has_python {
        return None;
    }
    let p = Project {
        id: "proj".into(),
        name: "proj".into(),
        repo_path: repo.into(),
        base_branch: "main".into(),
        run_commands: vec![RunCommand {
            name: "dev".into(),
            cmd: r#"python3 -c 'import http.server as h, socketserver as s, sys; s.TCPServer.allow_reuse_address = True; s.TCPServer(("", int(sys.argv[1])), h.SimpleHTTPRequestHandler).serve_forever()' $RECUR_PORT"#.into(),
        }],
        app_url: "http://localhost:$RECUR_PORT".into(),
        ..Project::default()
    };
    Some(add_run_project(store, p).id)
}

/// A Claude NDJSON script that also writes `edits` (Go `ScriptedRunner{Edits,
/// Lines}`).
pub fn scripted_with_edits(lines: Vec<String>, edits: &[(&str, &str)]) -> Arc<dyn Provider> {
    Arc::new(
        recur_provider_claude::testing::NdjsonScript::new(lines)
            .with_edits(
                edits
                    .iter()
                    .map(|(p, b)| ((*p).to_owned(), (*b).to_owned())),
            )
            .imitating(claude()),
    )
}

/// Applies optional edits, emits one Bash step, then blocks until the gate
/// opens or the turn is cancelled (Go `blockingRunner`).
pub fn blocking_runner(
    gate: CancellationToken,
    edits: &'static [(&'static str, &'static str)],
) -> Arc<dyn Provider> {
    AsyncProvider::wrap(move |req, cx| {
        let gate = gate.clone();
        async move {
            apply_edits(&req.worktree, edits).map_err(failed)?;
            emit_step(&cx, "", "Bash", "working", "");
            tokio::select! {
                _ = gate.cancelled() => Ok(success(&req.agent_id, "")),
                _ = cx.cancel.cancelled() => Err(ProviderError::canceled()),
            }
        }
    })
}

/// Blocks until the turn is cancelled (Go `ctxRunner`).
pub fn ctx_runner() -> Arc<dyn Provider> {
    AsyncProvider::wrap(|_req, cx| async move {
        emit_step(&cx, "", "Bash", "long op", "");
        cx.cancel.cancelled().await;
        Err(ProviderError::canceled())
    })
}

/// The escalate request agent-tool tests file.
pub fn esc_req() -> recur_agent_tools::EscalationRequest {
    use recur_agent_tools::EscalationOption;
    recur_agent_tools::EscalationRequest {
        question: "Should I use approach A or B here?".into(),
        context: "Both are viable; the choice changes the public contract, so I won't guess."
            .into(),
        options: vec![
            EscalationOption {
                text: "Approach A".into(),
                tag: "simplest".into(),
                desc: "Less code, slightly less flexible.".into(),
            },
            EscalationOption {
                text: "Approach B".into(),
                tag: "recommended".into(),
                desc: "More robust, a bit more work.".into(),
            },
        ],
    }
}

/// A Coder that edits and fires the escalate tool on its running loop
/// (Go `escalatingRunner`); everything else succeeds.
pub struct EscalatingRunner {
    pub eng: Mutex<std::sync::Weak<crate::Engine>>,
}

impl EscalatingRunner {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            eng: Mutex::new(std::sync::Weak::new()),
        })
    }

    pub fn bind(&self, eng: &Arc<crate::Engine>) {
        *self.eng.lock() = Arc::downgrade(eng);
    }

    pub fn provider(self: &Arc<Self>) -> Arc<dyn Provider> {
        let me = Arc::clone(self);
        AsyncProvider::wrap(move |req, _cx| {
            let me = Arc::clone(&me);
            async move {
                if req.role == AgentRole::Coder {
                    apply_edits(&req.worktree, &[("work.txt", "edit\n")]).map_err(failed)?;
                    if let Some(e) = me.eng.lock().upgrade() {
                        e.escalate(&req.workspace_id, esc_req());
                    }
                }
                Ok(success(&req.agent_id, ""))
            }
        })
    }
}

/// Go `stepIDsIn`: the distinct `S<n>` step ids in a prompt, in order.
pub fn step_ids_in(prompt: &str) -> Vec<String> {
    let re = regex::Regex::new(r"\bS[0-9]+\b").expect("step id regex");
    let mut out: Vec<String> = Vec::new();
    for m in re.find_iter(prompt) {
        if !out.iter().any(|s| s == m.as_str()) {
            out.push(m.as_str().to_owned());
        }
    }
    out
}

/// Go `fanOutRunner` (+ `plannedFanOutRunner` when `plans`): each Coder
/// edits a file named after its worktree, closes findings, closes the plan
/// steps its prompt names and calls done with notes; a child whose prompt
/// names `fail_on` fails; `slow_on` sleeps first; `mark_on` titles record
/// their turn timings. A planner turn files a two-step structured plan.
#[derive(Default)]
pub struct FanOutRunner {
    pub parent_id: Mutex<String>,
    pub fail_on: String,
    pub slow_on: String,
    pub slow_delay: std::time::Duration,
    pub mark_on: Vec<&'static str>,
    pub plans: bool,
    pub state: Mutex<FanOutState>,
}

#[derive(Default)]
pub struct FanOutState {
    pub coder_work: Vec<String>,
    pub failed: usize,
    pub planned: usize,
    pub started: std::collections::HashMap<String, std::time::Instant>,
    pub finished: std::collections::HashMap<String, std::time::Instant>,
}

impl FanOutRunner {
    pub fn provider(self: &Arc<Self>) -> Arc<dyn Provider> {
        let me = Arc::clone(self);
        AsyncProvider::wrap(move |req, _cx| {
            let me = Arc::clone(&me);
            async move { me.run(req).await }
        })
    }

    fn stamp(&self, text: &str, finished: bool) {
        let mut s = self.state.lock();
        for t in &self.mark_on {
            if text.contains(t) {
                let m = if finished {
                    &mut s.finished
                } else {
                    &mut s.started
                };
                m.entry((*t).to_owned())
                    .or_insert_with(std::time::Instant::now);
            }
        }
    }

    async fn run(&self, req: TurnRequest) -> Result<TurnResult, ProviderError> {
        let allowed = req.allowed_tools.clone().unwrap_or_default().join(",");
        if self.plans
            && req.role == AgentRole::Maestro
            && allowed.contains("submit_plan")
            && !req.system_prompt.contains("Implementer handoff:")
        {
            self.state.lock().planned += 1;
            post_mcp_tool(
                &req.mcp_url,
                "submit_plan",
                json!({
                    "summary": "Importer", "body": "Parser and writer are independent.",
                    "steps": [
                        {"title": "Write the parser", "files": ["parser.go"]},
                        {"title": "Write the writer", "files": ["writer.go"]},
                    ],
                }),
            )
            .await
            .map_err(failed)?;
            return Ok(success(&req.agent_id, ""));
        }
        if req.role != AgentRole::Coder {
            return Ok(success(&req.agent_id, ""));
        }
        let worktree = req.worktree.to_string_lossy().into_owned();
        let text = format!("{} {}", req.system_prompt, req.prompt);
        let fail = {
            let mut s = self.state.lock();
            s.coder_work.push(worktree.clone());
            let parent = self.parent_id.lock().clone();
            let child = !parent.is_empty() && req.workspace_id != parent;
            let fail = !self.fail_on.is_empty()
                && child
                && format!("{}{}", req.system_prompt, req.prompt).contains(&self.fail_on);
            if fail {
                s.failed += 1;
            }
            fail
        };
        if fail {
            return Ok(TurnResult {
                subtype: "error".into(),
                is_error: true,
                session_id: req.agent_id.clone(),
                ..TurnResult::default()
            });
        }
        self.stamp(&text, false);
        if !self.slow_on.is_empty() && text.contains(&self.slow_on) {
            tokio::time::sleep(self.slow_delay).await;
        }
        let name = format!(
            "{}.txt",
            Path::new(&worktree)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        );
        let body = format!("done by {}\n", req.workspace_id);
        let out = async {
            apply_edits(&req.worktree, &[(name.as_str(), body.as_str())]).map_err(failed)?;
            if !req.mcp_url.is_empty() {
                close_open_findings_mcp(&req.mcp_url).await.map_err(failed)?;
                let mut ids = step_ids_in(&text);
                if ids.is_empty() {
                    ids = vec!["S1".into()];
                }
                for id in ids {
                    let _ = post_mcp_tool(
                        &req.mcp_url,
                        "update_plan_step",
                        json!({"step_id": id, "status": "done"}),
                    )
                    .await;
                }
                let _ = post_mcp_tool(
                    &req.mcp_url,
                    "done",
                    json!({"summary": "Step done", "notes": format!("no deviations for {}", req.workspace_id)}),
                )
                .await;
            }
            Ok(success(&req.agent_id, ""))
        }
        .await;
        self.stamp(&text, true);
        out
    }
}

/// Every turn fails with a provider error result (Go `failAgentRunner`).
pub fn fail_agent_runner() -> Arc<dyn Provider> {
    AsyncProvider::wrap(|_req, _cx| async move {
        Ok(TurnResult {
            subtype: "error_during_execution".into(),
            is_error: true,
            text: "provider timeout".into(),
            ..TurnResult::default()
        })
    })
}

/// A shebang `[[prompt]]` bundle under the default bundles root (Go
/// `writePromptBundle`).
pub fn write_prompt_bundle(t: &crate::testing::TestEngine, slash: &str, body: &str, id: &str) {
    let dir = Path::new(&t.engine.bundle_roots().default_dir).join("ChatTest");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let script = dir.join("src").join("act.sh");
    std::fs::write(&script, body).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(
        dir.join("bundle.toml"),
        format!(
            "[bundle]\nname = \"ChatTest\"\nid = \"recur.chattest\"\nversion = \"1.0.0\"\n\n[[prompt]]\nid = \"{id}\"\nname = \"Act\"\nslash = \"{slash}\"\ninterpreter = \"shebang\"\nscript = \"src/act.sh\"\n"
        ),
    )
    .unwrap();
}

/// The first user text bubble of a session (Go `contentItems(...)` scan).
pub async fn first_user_bubble(eng: &crate::Engine, ws: &str, code: &str) -> String {
    crate::turn::transcript::content_items(&eng.workspace_chat_items(ws, code).await)
        .into_iter()
        .find(|it| it.role == "user" && it.kind == "text")
        .map(|it| it.text)
        .unwrap_or_default()
}
