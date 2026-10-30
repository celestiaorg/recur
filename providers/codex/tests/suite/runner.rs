//! Codex turns and probes against a fake `codex app-server` shell script:
//! thread start/resume, notifications to events, approvals routed to the
//! human, steering, cancel, a crashed server, failed turns. Plus the live
//! smoke, env-gated.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use recur_agent::testing::EventRecorder;
use recur_agent::{
    AgentEvent, ErrorKind, PermissionAbandoned, PermissionAsker, Steer, SteerInbox, TurnContext,
    TurnRequest,
};
use recur_domain::{AgentRole, PermissionDecision, PermissionRequest};
use recur_provider_codex::runner::{RunConfig, run};
use recur_provider_codex::status::{ProbeEnv, probe};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Shell helpers: `id_of` reads a request id; `reply ID JSON` answers it;
/// `log` appends a line to the transcript.
const PRELUDE: &str = r#"id_of() { printf '%s' "$1" | sed -n 's/^{"method":"[^"]*","id":\([0-9][0-9]*\).*/\1/p'; }
reply() { printf '{"id":%s,"result":%s}\n' "$1" "$2"; }
log() { printf '%s\n' "$1" >> "$LOG"; }
next() { read -r line || exit 0; log "$line"; }
next; reply "$(id_of "$line")" '{}'
next
"#;

fn fake(dir: &Path, body: &str) -> PathBuf {
    let bin = dir.join("codex");
    let script = format!(
        "#!/bin/sh\nLOG='{}'\n{PRELUDE}{body}",
        dir.join("log").display()
    );
    recur_exec::write_executable(&bin, script, 0o755).unwrap();
    bin
}

fn rc(dir: &Path, bin: &Path) -> RunConfig {
    RunConfig {
        bin: bin.to_string_lossy().into_owned(),
        api_key: "sk-test".into(),
        home: dir.join("codex-home"),
        ..Default::default()
    }
}

fn logged(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("log"))
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

#[derive(Default)]
struct Approver {
    asked: Mutex<Vec<PermissionRequest>>,
}

#[async_trait::async_trait]
impl PermissionAsker for Approver {
    async fn ask(&self, req: PermissionRequest) -> Result<PermissionDecision, PermissionAbandoned> {
        self.asked.lock().push(req);
        Ok(PermissionDecision {
            allow: true,
            remember: true,
            ..Default::default()
        })
    }
}

const THREAD_START: &str = r#"next; reply "$(id_of "$line")" '{"thread":{"id":"thr-1"}}'
next; reply "$(id_of "$line")" '{"turn":{"id":"turn-1"}}'
"#;

#[tokio::test]
async fn turn_with_approval_and_steer() {
    let dir = tempfile::tempdir().unwrap();
    let body = format!(
        r#"{THREAD_START}echo '{{"method":"item/started","params":{{"item":{{"type":"commandExecution","id":"c1","command":"rm -rf build"}}}}}}'
echo '{{"method":"item/commandExecution/requestApproval","id":900,"params":{{"command":"rm -rf build"}}}}'
answered=0; steered=0
while [ $answered = 0 ] || [ $steered = 0 ]; do
  next
  case "$line" in
    *'"id":900'*) answered=1 ;;
    *'"method":"turn/steer"'*) reply "$(id_of "$line")" '{{"turnId":"turn-1"}}'; steered=1 ;;
  esac
done
echo '{{"method":"item/completed","params":{{"item":{{"type":"commandExecution","id":"c1","aggregatedOutput":"ok","exitCode":0}}}}}}'
echo '{{"method":"item/agentMessage/delta","params":{{"delta":"Hel"}}}}'
echo '{{"method":"item/agentMessage/delta","params":{{"delta":"lo"}}}}'
echo '{{"method":"thread/tokenUsage/updated","params":{{"tokenUsage":{{"total":{{"totalTokens":120,"inputTokens":100,"cachedInputTokens":40,"outputTokens":20}},"last":{{"totalTokens":120,"inputTokens":100,"cachedInputTokens":40,"outputTokens":20}},"modelContextWindow":272000}}}}}}'
echo '{{"method":"item/completed","params":{{"item":{{"type":"agentMessage","id":"m1","text":"Hello"}}}}}}'
echo '{{"method":"turn/completed","params":{{"turn":{{"id":"turn-1","status":"completed"}}}}}}'
while next; do :; done
"#
    );
    let bin = fake(dir.path(), &body);
    let rec = EventRecorder::default();
    let cancel = CancellationToken::new();
    let mut cx = TurnContext::new(cancel.clone(), Arc::new(rec.clone()));
    let approver = Arc::new(Approver::default());
    cx.permissions = Some(approver.clone());
    let (tx, rx) = mpsc::channel(4);
    cx.steer = Some(SteerInbox(rx));
    let (steer, ack) = Steer::new("also run vet", Some("Also run vet".into()));
    tx.send(steer).await.unwrap();

    let req = TurnRequest {
        workspace_id: "W1".into(),
        agent_id: "W1-PA".into(),
        role: AgentRole::Coder,
        model: "gpt-5.6-sol".into(),
        flags: [("effort".to_owned(), "high".to_owned())].into(),
        system_prompt: "You are the Coder.".into(),
        append_system_prompt: "This chat is in AGENT mode.".into(),
        prompt: "fix it".into(),
        permission_mode: "default".into(),
        allowed_tools: Some(vec!["Read".into(), "Edit".into(), "Bash".into()]),
        worktree: dir.path().into(),
        mcp_url: "http://127.0.0.1:9/tok/mcp".into(),
        include_partials: true,
        ..Default::default()
    };
    let res = tokio::time::timeout(Duration::from_secs(10), run(rc(dir.path(), &bin), req, cx))
        .await
        .expect("turn hung")
        .unwrap();
    assert_eq!(
        (
            res.session_id.as_str(),
            res.subtype.as_str(),
            res.text.as_str()
        ),
        ("thr-1", "success", "Hello")
    );
    assert!(!res.is_error && res.duration_ms >= 1);
    assert_eq!(
        (
            res.usage.input_tokens,
            res.usage.cache_read_tokens,
            res.usage.context_window
        ),
        (60, 40, 272_000)
    );
    assert!(ack.await.unwrap(), "steer delivered");

    let asked = approver.asked.lock().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(
        (
            asked[0].workspace_id.as_str(),
            asked[0].agent_code.as_str(),
            asked[0].tool.as_str()
        ),
        ("W1", "PA", "Bash")
    );
    assert_eq!(asked[0].summary, "Bash: rm -rf build");

    let evs = rec.take();
    let kinds: Vec<_> = evs.iter().map(AgentEvent::kind).collect();
    assert_eq!(kinds.first(), Some(&"init"));
    assert_eq!(kinds.last(), Some(&"result"));
    for k in ["step", "tool_result", "text_delta", "usage", "user", "text"] {
        assert!(kinds.contains(&k), "missing {k}: {kinds:?}");
    }
    assert!(evs.contains(&AgentEvent::User {
        text: "also run vet".into(),
        display: "Also run vet".into(),
        live_steer: true
    }));
    let AgentEvent::Init { mcp_servers, .. } = &evs[0] else {
        panic!()
    };
    assert_eq!(mcp_servers[0].name, "recur");

    let log = logged(dir.path());
    let find = |m: &str| {
        log.iter()
            .find(|l| l.contains(&format!(r#""method":"{m}""#)))
            .cloned()
            .unwrap_or_default()
    };
    assert_eq!(
        find("thread/start"),
        format!(
            r#"{{"method":"thread/start","id":2,"params":{{"model":"gpt-5.6-sol","cwd":"{}","approvalPolicy":"on-request","sandbox":"workspace-write","developerInstructions":"You are the Coder.","serviceName":"recur"}}}}"#,
            dir.path().display()
        )
    );
    assert_eq!(
        find("turn/start"),
        format!(
            r#"{{"method":"turn/start","id":3,"params":{{"threadId":"thr-1","input":[{{"type":"text","text":"fix it\n\nThis chat is in AGENT mode."}}],"model":"gpt-5.6-sol","cwd":"{}","approvalPolicy":"on-request","sandboxPolicy":{{"type":"workspaceWrite"}},"effort":"high"}}}}"#,
            dir.path().display()
        )
    );
    assert!(
        find("turn/steer").ends_with(
            r#""params":{"threadId":"thr-1","input":[{"type":"text","text":"also run vet"}]}}"#
        ),
        "{log:?}"
    );
    assert!(log.contains(&r#"{"id":900,"result":{"decision":"acceptForSession"}}"#.to_owned()));
    let trust = std::fs::read_to_string(dir.path().join("codex-home/config.toml")).unwrap();
    assert!(trust.contains("trust_level = \"trusted\""));
}

#[tokio::test]
async fn resume_falls_back_and_reattaches_standing() {
    let dir = tempfile::tempdir().unwrap();
    let body = r#"next; printf '{"id":%s,"error":{"code":-1,"message":"no rollout"}}\n' "$(id_of "$line")"
next; reply "$(id_of "$line")" '{"thread":{"id":"thr-new"}}'
next; reply "$(id_of "$line")" '{"turn":{"id":"turn-1"}}'
echo '{"method":"turn/completed","params":{"turn":{"id":"turn-1","status":"completed"}}}'
while next; do :; done
"#;
    let bin = fake(dir.path(), body);
    let rec = EventRecorder::default();
    let cx = TurnContext::new(CancellationToken::new(), Arc::new(rec.clone()));
    let req = TurnRequest {
        prompt: "again".into(),
        append_system_prompt: "PLAN mode.".into(),
        resume_session: "thr-gone".into(),
        worktree: dir.path().into(),
        role: AgentRole::Reviewer,
        ..Default::default()
    };
    let res = run(rc(dir.path(), &bin), req, cx).await.unwrap();
    assert_eq!(res.session_id, "thr-new");
    let log = logged(dir.path());
    assert!(
        log[2].starts_with(
            r#"{"method":"thread/resume","id":2,"params":{"approvalPolicy":"on-request","cwd":"#
        ),
        "{log:?}"
    );
    assert!(
        log[2].contains(r#""sandbox":"read-only","threadId":"thr-gone""#),
        "{}",
        log[2]
    );
    assert!(
        log[4].contains(r#""text":"again\n\nPLAN mode.""#),
        "fresh thread re-attaches: {}",
        log[4]
    );
    assert!(log[4].contains(r#""sandboxPolicy":{"type":"readOnly","networkAccess":true}"#));
}

#[tokio::test]
async fn failed_turn_returns_error_result_without_turn_end_duration() {
    let dir = tempfile::tempdir().unwrap();
    let body = format!(
        r#"{THREAD_START}echo '{{"method":"turn/completed","params":{{"turn":{{"id":"turn-1","status":"failed","error":{{"message":"quota exceeded"}}}}}}}}'
while next; do :; done
"#
    );
    let bin = fake(dir.path(), &body);
    let rec = EventRecorder::default();
    let cx = TurnContext::new(CancellationToken::new(), Arc::new(rec.clone()));
    let req = TurnRequest {
        prompt: "x".into(),
        worktree: dir.path().into(),
        ..Default::default()
    };
    let res = run(rc(dir.path(), &bin), req, cx).await.unwrap();
    assert!(res.is_error);
    assert_eq!(
        (res.subtype.as_str(), res.text.as_str()),
        ("error_during_execution", "quota exceeded")
    );
    assert!(res.duration_ms >= 1);
    let Some(AgentEvent::TurnEnd(frame)) = rec.take().pop() else {
        panic!("no result frame")
    };
    assert_eq!(
        frame.duration_ms, 0,
        "no session.turn_end for an error frame"
    );
    assert!(frame.is_error);
}

#[tokio::test]
async fn server_exit_mid_turn() {
    let dir = tempfile::tempdir().unwrap();
    let body = format!("{THREAD_START}echo 'panic: boom' >&2\nexit 1\n");
    let bin = fake(dir.path(), &body);
    let rec = EventRecorder::default();
    let cx = TurnContext::new(CancellationToken::new(), Arc::new(rec.clone()));
    let req = TurnRequest {
        prompt: "x".into(),
        worktree: dir.path().into(),
        ..Default::default()
    };
    let err = tokio::time::timeout(Duration::from_secs(10), run(rc(dir.path(), &bin), req, cx))
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert_eq!(
        err.message,
        "codex app-server exited before turn completed\npanic: boom"
    );
    assert_eq!(err.partial.session_id, "thr-1");
    assert_eq!(err.partial.stderr, "panic: boom");
    assert!(err.partial.is_error);
    assert!(rec.take().iter().all(|e| e.kind() != "result"));
}

#[tokio::test]
async fn cancel_interrupts_and_detaches() {
    let dir = tempfile::tempdir().unwrap();
    let body = format!("{THREAD_START}while next; do :; done\n");
    let bin = fake(dir.path(), &body);
    let rec = EventRecorder::default();
    let cancel = CancellationToken::new();
    let cx = TurnContext::new(cancel.clone(), Arc::new(rec.clone()));
    let req = TurnRequest {
        prompt: "x".into(),
        worktree: dir.path().into(),
        ..Default::default()
    };
    let rc = rc(dir.path(), &bin);
    let task = tokio::spawn(async move { run(rc, req, cx).await });
    while !logged(dir.path()).iter().any(|l| l.contains("turn/start")) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    cancel.cancel();
    let err = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("cancel did not end the turn")
        .unwrap()
        .unwrap_err();
    assert!(err.is_canceled());
    assert_eq!(err.partial.text, "context canceled");
    assert!(rec.take().iter().all(|e| e.kind() != "result"));
}

#[tokio::test]
async fn deny_mode_declines_without_asking() {
    let dir = tempfile::tempdir().unwrap();
    let body = format!(
        r#"{THREAD_START}echo '{{"method":"item/fileChange/requestApproval","id":901,"params":{{"itemId":"fc1"}}}}'
next
echo '{{"method":"turn/completed","params":{{"turn":{{"id":"turn-1","status":"completed"}}}}}}'
while next; do :; done
"#
    );
    let bin = fake(dir.path(), &body);
    let mut cx = TurnContext::new(CancellationToken::new(), Arc::new(EventRecorder::default()));
    let approver = Arc::new(Approver::default());
    cx.permissions = Some(approver.clone());
    let req = TurnRequest {
        prompt: "title".into(),
        worktree: dir.path().into(),
        permission_mode: "dontAsk".into(),
        ..Default::default()
    };
    run(rc(dir.path(), &bin), req, cx).await.unwrap();
    assert!(approver.asked.lock().is_empty());
    assert!(
        logged(dir.path()).contains(&r#"{"id":901,"result":{"decision":"decline"}}"#.to_owned())
    );
}

#[tokio::test]
async fn probe_reads_account_and_models() {
    let dir = tempfile::tempdir().unwrap();
    let body = r#"next; reply "$(id_of "$line")" '{"account":{"type":"chatgpt","email":" dev@example.com ","planType":"pro"},"requiresOpenaiAuth":true}'
next; reply "$(id_of "$line")" '{"data":[{"id":"gpt-5.6-sol","displayName":"GPT-5.6-Sol","supportedReasoningEfforts":null}]}'
while next; do :; done
"#;
    let bin = fake(dir.path(), body);
    let env = ProbeEnv {
        home: dir.path().join("codex-home"),
        data_dir: dir.path().into(),
        env_scrub: Vec::new(),
        seed_auth_from: None,
    };
    let p = probe(&env, &bin.to_string_lossy(), "", false)
        .await
        .unwrap();
    assert_eq!(
        (p.login_method.as_str(), p.email.as_str(), p.plan.as_str()),
        ("ChatGPT account", "dev@example.com", "Pro")
    );
    let models = p.models.unwrap();
    assert_eq!(models[0].display_name, "GPT-5.6-Sol");
    let log = logged(dir.path());
    assert_eq!(
        log[2],
        r#"{"method":"account/read","id":2,"params":{"refreshToken":false}}"#
    );
    assert_eq!(
        log[3],
        r#"{"method":"model/list","id":3,"params":{"includeHidden":false,"limit":50}}"#
    );

    let logged_out = r#"next; reply "$(id_of "$line")" '{"account":null}'
while next; do :; done
"#;
    let bin = fake(dir.path(), logged_out);
    assert_eq!(
        probe(&env, &bin.to_string_lossy(), "sk", false)
            .await
            .unwrap_err(),
        "not authenticated: sign in with ChatGPT in Settings or set an API key"
    );
}

/// Live app-server smoke. Needs a native binary and auth:
///
/// ```text
/// RECUR_CODEX_E2E=1 RECUR_CODEX=/path/to/codex cargo test -p recur-provider-codex --test suite -- codex_e2e_smoke
/// ```
#[tokio::test]
async fn codex_e2e_smoke() {
    if std::env::var("RECUR_CODEX_E2E").as_deref() != Ok("1") {
        eprintln!("set RECUR_CODEX_E2E=1 to run live Codex smoke");
        return;
    }
    let bin = recur_provider_codex::locate::locate("").unwrap_or_else(|e| {
        panic!("locate codex: {e}: install `brew install --cask codex` or set RECUR_CODEX to the native binary (full package layout, not a lone copied file)")
    });
    let wt = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    assert!(
        std::process::Command::new("git")
            .arg("init")
            .arg(wt.path())
            .status()
            .unwrap()
            .success()
    );
    let rc = RunConfig {
        bin,
        chatgpt_auth: true,
        home: home.path().into(),
        seed_auth_from: recur_provider_codex::server::default_seed_auth_path(),
        ..Default::default()
    };
    let rec = EventRecorder::default();
    let cx = TurnContext::new(CancellationToken::new(), Arc::new(rec.clone()));
    let req = TurnRequest {
        worktree: wt.path().into(),
        model: "gpt-5.6-luna".into(),
        role: AgentRole::Coder,
        system_prompt: "You are a concise coding agent.".into(),
        prompt: "Create a file named hello-recur.txt containing exactly: ok\nThen stop. Do not do anything else.".into(),
        ..Default::default()
    };
    let res = tokio::time::timeout(Duration::from_secs(180), run(rc, req, cx))
        .await
        .unwrap()
        .unwrap_or_else(|e| panic!("Run: {e}\nstderr: {}", e.partial.stderr));
    assert!(!res.session_id.is_empty(), "empty session/thread id");
    assert!(res.duration_ms > 0);
    let steps = rec.take().iter().filter(|e| e.kind() == "step").count();
    let body = std::fs::read_to_string(wt.path().join("hello-recur.txt")).unwrap_or_else(|e| {
        panic!(
            "expected worktree edit: {e} (text={:?} steps={steps})",
            res.text
        )
    });
    assert!(body == "ok\n" || body == "ok", "file body {body:?}");
    assert!(
        !wt.path().join(".codex").exists(),
        "worktree must not contain .codex/"
    );
}
