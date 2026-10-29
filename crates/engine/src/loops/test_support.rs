//! Loop test doubles and MCP posting helpers: providers that drive a
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

/// Writes repo-relative files into the worktree.
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

/// Emits one tool step.
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
/// An `isError` result is an error.
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

/// An attach_evidence call (its result is ignored).
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

/// The converge runner's counters.
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

/// A project whose dev command serves HTTP on $RECUR_PORT so verify can
/// boot. `None` when python3 is missing.
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

/// A Claude NDJSON script that also writes `edits`.
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
/// opens or the turn is cancelled.
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

/// Blocks until the turn is cancelled.
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
/// Everything else succeeds.
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

/// The distinct `S<n>` step ids in a prompt, in order.
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

/// A fan-out runner (planning too when `plans`): each Coder
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
    /// With `slow_on`: the slow turn ends as soon as this marked title has started (still
    /// at most `slow_delay`), so ordering tests do not race the machine's speed.
    pub slow_until: Option<&'static str>,
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
            let deadline = tokio::time::Instant::now() + self.slow_delay;
            while tokio::time::Instant::now() < deadline
                && self
                    .slow_until
                    .is_none_or(|t| !self.state.lock().started.contains_key(t))
            {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
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

/// Every turn fails with a provider error result.
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

/// A shebang `[[prompt]]` bundle under the default bundles root.
pub fn write_prompt_bundle(t: &crate::testing::TestEngine, slash: &str, body: &str, id: &str) {
    let dir = Path::new(&t.engine.bundle_roots().default_dir).join("ChatTest");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let script = dir.join("src").join("act.sh");
    recur_exec::write_executable(&script, body, 0o755).unwrap();
    std::fs::write(
        dir.join("bundle.toml"),
        format!(
            "[bundle]\nname = \"ChatTest\"\nid = \"recur.chattest\"\nversion = \"1.0.0\"\n\n[[prompt]]\nid = \"{id}\"\nname = \"Act\"\nslash = \"{slash}\"\ninterpreter = \"shebang\"\nscript = \"src/act.sh\"\n"
        ),
    )
    .unwrap();
}

/// The first user text bubble of a session.
pub async fn first_user_bubble(eng: &crate::Engine, ws: &str, code: &str) -> String {
    crate::turn::transcript::content_items(&eng.workspace_chat_items(ws, code).await)
        .into_iter()
        .find(|it| it.role == "user" && it.kind == "text")
        .map(|it| it.text)
        .unwrap_or_default()
}

type LinesFor = Arc<dyn Fn(usize) -> Vec<String> + Send + Sync>;

/// Records every request and replays per-call Claude stream-json through
/// the real parser.
pub struct CaptureScript {
    pub specs: Mutex<Vec<TurnRequest>>,
    lines_for: Option<LinesFor>,
}

impl CaptureScript {
    pub fn new(lines_for: impl Fn(usize) -> Vec<String> + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            specs: Mutex::new(Vec::new()),
            lines_for: Some(Arc::new(lines_for)),
        })
    }

    pub fn provider(self: &Arc<Self>) -> Arc<dyn Provider> {
        let me = Arc::clone(self);
        AsyncProvider::wrap(move |req, cx| {
            let me = Arc::clone(&me);
            async move {
                let call = {
                    let mut s = me.specs.lock();
                    s.push(req.clone());
                    s.len() - 1
                };
                let lines = me.lines_for.as_ref().map(|f| f(call)).unwrap_or_default();
                let p =
                    recur_provider_claude::testing::NdjsonScript::new(lines).imitating(claude());
                p.run_turn(req, cx).await
            }
        })
    }

    pub fn spec_at(&self, i: usize) -> Option<TurnRequest> {
        self.specs.lock().get(i).cloned()
    }

    pub fn with_role(&self, role: AgentRole) -> Vec<TurnRequest> {
        self.specs
            .lock()
            .iter()
            .filter(|s| s.role == role)
            .cloned()
            .collect()
    }
}

/// Maestro turns replay `maestro`; every other role emits a Bash step and
/// blocks until the gate opens or the turn is cancelled.
pub fn role_router_runner(gate: CancellationToken, maestro: Vec<String>) -> Arc<dyn Provider> {
    AsyncProvider::wrap(move |req, cx| {
        let (gate, maestro) = (gate.clone(), maestro.clone());
        async move {
            if req.role == AgentRole::Maestro {
                let p =
                    recur_provider_claude::testing::NdjsonScript::new(maestro).imitating(claude());
                return p.run_turn(req, cx).await;
            }
            emit_step(&cx, "", "Bash", "working", "");
            tokio::select! {
                _ = gate.cancelled() => Ok(success(&req.agent_id, "")),
                _ = cx.cancel.cancelled() => Err(ProviderError::canceled()),
            }
        }
    })
}

/// Records specs; specialists emit a step, signal `gated`, block on the gate,
/// then emit the tool result and take one queued steer (Claude's D49 timing).
#[derive(Default)]
pub struct SteerCapture {
    pub specs: Mutex<Vec<TurnRequest>>,
    pub injected: Mutex<Vec<String>>,
    pub gate: CancellationToken,
    pub gated: tokio::sync::Notify,
}

impl SteerCapture {
    pub fn provider(self: &Arc<Self>) -> Arc<dyn Provider> {
        let me = Arc::clone(self);
        AsyncProvider::wrap(move |req, cx| {
            let me = Arc::clone(&me);
            async move {
                let mut cx = cx;
                me.specs.lock().push(req.clone());
                if req.role == AgentRole::Maestro {
                    return Ok(success(&req.agent_id, ""));
                }
                emit_step(&cx, "t1", "Bash", "working", "");
                me.gated.notify_one();
                tokio::select! {
                    _ = me.gate.cancelled() => {}
                    _ = cx.cancel.cancelled() => return Err(ProviderError::canceled()),
                }
                cx.sink
                    .emit(AgentEvent::ToolResult(recur_agent::ToolResultEvent {
                        id: "t1".into(),
                        text: "ok".into(),
                        ..recur_agent::ToolResultEvent::default()
                    }));
                if let Some(inbox) = cx.steer.as_mut()
                    && let Some(s) = inbox.try_next()
                {
                    me.injected.lock().push(s.prompt.clone());
                }
                Ok(success(&req.agent_id, ""))
            }
        })
    }
}

/// Counts Coder turns per workspace (a double dispatch runs the Coder twice)
/// and optionally holds each Coder turn on a gate.
#[derive(Default)]
pub struct CountingRunner {
    pub counts: Mutex<std::collections::HashMap<String, usize>>,
    pub gate: Option<CancellationToken>,
}

impl CountingRunner {
    pub fn provider(self: &Arc<Self>) -> Arc<dyn Provider> {
        let me = Arc::clone(self);
        AsyncProvider::wrap(move |req, cx| {
            let me = Arc::clone(&me);
            async move {
                if req.role == AgentRole::Coder {
                    *me.counts
                        .lock()
                        .entry(req.workspace_id.clone())
                        .or_default() += 1;
                    let body = format!("done by {}\n", req.workspace_id);
                    apply_edits(&req.worktree, &[("work.txt", body.as_str())]).map_err(failed)?;
                }
                emit_step(&cx, "", "Write", "work.txt", "");
                if let Some(gate) = &me.gate
                    && req.role == AgentRole::Coder
                {
                    tokio::select! {
                        _ = gate.cancelled() => {}
                        _ = cx.cancel.cancelled() => return Err(ProviderError::canceled()),
                    }
                }
                Ok(success(&req.agent_id, ""))
            }
        })
    }

    /// The workspace with the most Coder turns and its count.
    pub fn max_count(&self) -> (String, usize) {
        self.counts
            .lock()
            .iter()
            .max_by_key(|(_, n)| **n)
            .map(|(k, n)| (k.clone(), *n))
            .unwrap_or_default()
    }
}

/// Fills every loop slot so an engaged run parks queued.
pub fn take_slots(t: &crate::testing::TestEngine) -> usize {
    let mut n = 0;
    while t.engine.governor().try_acquire_loop() {
        n += 1;
    }
    n
}

pub fn release_slots(t: &crate::testing::TestEngine, n: usize) {
    for _ in 0..n {
        t.engine.governor().release_loop();
    }
}

/// Linear on in Settings with an API key; the engine needs [`linear_trackers`]
/// to build it.
pub fn enable_linear_mcp(store: &Store) {
    let mut set = store.settings();
    set.mcp_servers = vec![recur_domain::McpServer {
        id: recur_domain::MCP_ID_LINEAR.into(),
        enabled: true,
        ..recur_domain::McpServer::default()
    }];
    set.bundle_values.insert(
        recur_domain::SETTING_LINEAR_API_KEY.into(),
        "lin_api_x".into(),
    );
    store.set_settings(set);
}

pub fn linear_trackers() -> Vec<Arc<dyn recur_issues::IssueTracker>> {
    vec![Arc::new(
        recur_issues::linear::Linear::new().with_env(|_| None),
    )]
}

/// E4's `CloseWorkspaceChat` as far as the walk sees it: the tab's agent
/// row goes and it stops owning the workspace's loop.
pub fn close_tab(store: &Store, ws: &str, code: &str) {
    store.update_workspace(ws, |w| {
        if w.loop_chat_code == code {
            w.loop_chat_code.clear();
        }
    });
    store.delete_agent(ws, code);
}
