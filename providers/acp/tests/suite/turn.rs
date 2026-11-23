//! ACP turns and probes against a fake agent shell script: handshake, model
//! and mode config, streaming, permissions, resume and load, cancel, a
//! crashed agent, auth errors, the probe cache. Plus an ignored live smoke
//! against `opencode acp`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use recur_agent::testing::EventRecorder;
use recur_agent::{
    AgentEvent, ErrorKind, PermissionAbandoned, PermissionAsker, Provider, TurnContext,
    TurnRequest, TurnResult,
};
use recur_domain::{AcpAgent, PermissionDecision, PermissionRequest, Settings, acp_agent};
use recur_provider_acp::launch::Launch;
use recur_provider_acp::{AcpConfig, AcpProvider, agent, turn};
use tokio_util::sync::CancellationToken;

/// `id_of` reads a request id; `reply ID JSON` answers it; `upd JSON` sends a
/// session/update; `next` reads and logs one line. The prelude answers
/// `initialize` with `$INIT`.
const PRELUDE: &str = r#"id_of() { printf '%s' "$1" | sed -n 's/^{"id":\([0-9][0-9]*\),.*/\1/p'; }
reply() { printf '{"jsonrpc":"2.0","id":%s,"result":%s}\n' "$1" "$2"; }
upd() { printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":%s}}\n' "$1"; }
next() { read -r line || exit 0; printf '%s\n' "$line" >> "$LOG"; }
next; reply "$(id_of "$line")" "$INIT"
"#;

const CAPS: &str = r#"{"protocolVersion":1,"agentCapabilities":{"loadSession":true,"promptCapabilities":{"image":true},"sessionCapabilities":{"resume":{},"close":{}}},"agentInfo":{"name":"Fake","version":"9.9.9"}}"#;

const OPTIONS: &str = r#"{"sessionId":"s1","configOptions":[{"id":"model","category":"model","currentValue":"fake/a","options":[{"value":"fake/a","name":"Fake A"},{"value":"fake/b","name":"Fake B"}]},{"id":"effort","name":"Effort","category":"thought_level","currentValue":"low","options":[{"value":"low","name":"Low"},{"value":"high","name":"High"}]},{"id":"mode","category":"mode","currentValue":"build","options":[{"value":"build","name":"build"},{"value":"plan","name":"plan"}]}]}"#;

fn opencode() -> &'static AcpAgent {
    acp_agent("opencode").unwrap()
}

fn fake(dir: &Path, caps: &str, body: &str) -> PathBuf {
    let bin = dir.join("agent");
    let script = format!(
        "#!/bin/sh\nLOG='{}'\nINIT='{caps}'\n{PRELUDE}{body}",
        dir.join("log").display()
    );
    recur_exec::write_executable(&bin, script, 0o755).unwrap();
    bin
}

fn logged(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("log"))
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

fn find(log: &[String], method: &str) -> String {
    log.iter()
        .find(|l| l.contains(&format!(r#""method":"{method}""#)))
        .cloned()
        .unwrap_or_default()
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
            ..Default::default()
        })
    }
}

async fn run(
    bin: &Path,
    req: TurnRequest,
    cx: TurnContext,
) -> Result<TurnResult, recur_agent::ProviderError> {
    tokio::time::timeout(
        Duration::from_secs(10),
        turn::run(opencode(), &Launch::plain(opencode(), bin), req, cx),
    )
    .await
    .expect("turn hung")
}

#[tokio::test]
async fn turn_streams_tools_and_asks() {
    let dir = tempfile::tempdir().unwrap();
    let body = r#"next; reply "$(id_of "$line")" "$(cat "$OPTS")"
next; reply "$(id_of "$line")" '{}'
next; reply "$(id_of "$line")" '{}'
next; p=$(id_of "$line")
upd '{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"Thinking"}}'
upd '{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Let me "}}'
upd '{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"look."}}'
upd '{"sessionUpdate":"tool_call","toolCallId":"t1","title":"bash","kind":"execute","status":"pending","rawInput":{}}'
printf '%s\n' '{"jsonrpc":"2.0","id":901,"method":"fs/read_text_file","params":{"path":"/x"}}'
printf '%s\n' '{"jsonrpc":"2.0","id":900,"method":"session/request_permission","params":{"sessionId":"s1","toolCall":{"toolCallId":"t1","title":"rm -rf build","kind":"execute","rawInput":{"command":"rm -rf build"}},"options":[{"optionId":"once","name":"Allow","kind":"allow_once"},{"optionId":"always","name":"Always","kind":"allow_always"},{"optionId":"no","name":"Reject","kind":"reject_once"}]}}'
a=0; b=0
while [ $a = 0 ] || [ $b = 0 ]; do
  next
  case "$line" in *'"id":900'*) a=1 ;; *'"id":901'*) b=1 ;; esac
done
upd '{"sessionUpdate":"tool_call_update","toolCallId":"t1","status":"completed","content":[{"type":"content","content":{"type":"text","text":"removed"}}]}'
upd '{"sessionUpdate":"usage_update","used":1200,"size":200000}'
upd '{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Done."}}'
reply "$p" '{"stopReason":"end_turn","usage":{"inputTokens":40,"outputTokens":7,"cachedReadTokens":1100}}'
while next; do :; done
"#;
    std::fs::write(dir.path().join("opts"), OPTIONS).unwrap();
    let bin = fake(
        dir.path(),
        CAPS,
        &format!("OPTS='{}'\n{body}", dir.path().join("opts").display()),
    );
    let rec = EventRecorder::default();
    let mut cx = TurnContext::new(CancellationToken::new(), Arc::new(rec.clone()));
    let approver = Arc::new(Approver::default());
    cx.permissions = Some(approver.clone());
    let req = TurnRequest {
        workspace_id: "W1".into(),
        agent_id: "W1-PA".into(),
        model: "fake/b".into(),
        flags: [("effort".to_owned(), "high".to_owned())].into(),
        system_prompt: "You are the Coder.".into(),
        prompt: "clean up".into(),
        permission_mode: "default".into(),
        worktree: dir.path().into(),
        mcp_url: "http://127.0.0.1:9/tok/mcp".into(),
        include_partials: true,
        images: vec![recur_agent::UserImage {
            id: "img1".into(),
            mime: "image/png".into(),
            data: "AAAA".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let res = run(&bin, req, cx).await.unwrap();
    assert_eq!(
        (
            res.session_id.as_str(),
            res.subtype.as_str(),
            res.text.as_str()
        ),
        ("opencode:s1", "success", "Done.")
    );
    assert!(!res.is_error && res.duration_ms >= 1);
    assert_eq!(
        (
            res.usage.input_tokens,
            res.usage.cache_read_tokens,
            res.usage.context_window
        ),
        (40, 1100, 200_000)
    );

    let asked = approver.asked.lock().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(
        (
            asked[0].agent_code.as_str(),
            asked[0].tool.as_str(),
            asked[0].summary.as_str()
        ),
        ("PA", "Bash", "Bash: rm -rf build")
    );

    let evs = rec.take();
    let kinds: Vec<_> = evs.iter().map(AgentEvent::kind).collect();
    assert_eq!(
        kinds,
        [
            "init",
            "thinking_delta",
            "thinking",
            "text_delta",
            "text_delta",
            "text",
            "step",
            "tool_result",
            "usage",
            "text_delta",
            "text",
            "result"
        ]
    );
    let AgentEvent::Init { mcp_servers, .. } = &evs[0] else {
        panic!()
    };
    assert_eq!(mcp_servers[0].name, "recur");
    assert!(evs.contains(&AgentEvent::Text {
        text: "Let me look.".into(),
        uuid: String::new(),
        parent: String::new()
    }));
    let Some(AgentEvent::TurnEnd(end)) = evs.last() else {
        panic!()
    };
    assert_eq!(
        end.usage.context_used(),
        1200,
        "the meter takes the last usage_update footprint"
    );
    let AgentEvent::Step(s) = &evs[6] else {
        panic!()
    };
    assert_eq!(
        (s.id.as_str(), s.name.as_str(), s.input.as_str()),
        ("t1", "execute", r#"{"command":"rm -rf build"}"#)
    );
    let AgentEvent::ToolResult(r) = &evs[7] else {
        panic!()
    };
    assert_eq!(r.text, "removed");

    let log = logged(dir.path());
    assert!(log[0].starts_with(r#"{"id":1,"jsonrpc":"2.0","method":"initialize","params":{"clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false}"#), "{}", log[0]);
    assert_eq!(
        find(&log, "session/new"),
        format!(
            r#"{{"id":2,"jsonrpc":"2.0","method":"session/new","params":{{"cwd":"{}","mcpServers":[{{"headers":[],"name":"recur","type":"http","url":"http://127.0.0.1:9/tok/mcp"}}]}}}}"#,
            dir.path().display()
        )
    );
    let sets: Vec<_> = log
        .iter()
        .filter(|l| l.contains("session/set_config_option"))
        .collect();
    assert_eq!(sets.len(), 2, "model and effort; build mode is left alone");
    assert!(sets[0].contains(r#""configId":"model","sessionId":"s1","value":"fake/b""#));
    assert!(sets[1].contains(r#""configId":"effort","sessionId":"s1","value":"high""#));
    assert_eq!(
        find(&log, "session/prompt"),
        r#"{"id":5,"jsonrpc":"2.0","method":"session/prompt","params":{"prompt":[{"text":"<role-instructions>\nYou are the Coder.\n</role-instructions>\n\nclean up","type":"text"},{"data":"AAAA","mimeType":"image/png","type":"image"}],"sessionId":"s1"}}"#
    );
    assert!(log.contains(
        &r#"{"id":900,"jsonrpc":"2.0","result":{"outcome":{"optionId":"once","outcome":"selected"}}}"#.to_owned()
    ));
    assert!(
        log.iter()
            .any(|l| l.starts_with(r#"{"error":{"code":-32601"#) && l.contains(r#""id":901"#)),
        "fs/* answers method-not-found"
    );
}

#[tokio::test]
async fn resume_read_only_plans_and_rejects_edits() {
    let dir = tempfile::tempdir().unwrap();
    let body = r#"next; reply "$(id_of "$line")" '{"configOptions":[{"id":"mode","category":"mode","currentValue":"build","options":[{"value":"build"},{"value":"plan"}]}]}'
next; reply "$(id_of "$line")" '{}'
next; p=$(id_of "$line")
printf '%s\n' '{"jsonrpc":"2.0","id":900,"method":"session/request_permission","params":{"sessionId":"s1","toolCall":{"toolCallId":"t1","title":"write","kind":"edit","rawInput":{"filePath":"/a","content":"x"}},"options":[{"optionId":"once","kind":"allow_once"},{"optionId":"no","kind":"reject_once"}]}}'
next
printf '%s\n' '{"jsonrpc":"2.0","id":902,"method":"session/request_permission","params":{"sessionId":"s1","toolCall":{"toolCallId":"t2","name":"mcp_recur_report_finding","kind":"other","rawInput":{"title":"t"}},"options":[{"optionId":"once","kind":"allow_once"},{"optionId":"no","kind":"reject_once"}]}}'
next
reply "$p" '{"stopReason":"end_turn"}'
while next; do :; done
"#;
    let bin = fake(dir.path(), CAPS, body);
    let mut cx = TurnContext::new(CancellationToken::new(), Arc::new(EventRecorder::default()));
    let approver = Arc::new(Approver::default());
    cx.permissions = Some(approver.clone());
    let req = TurnRequest {
        system_prompt: "You are the Coder.".into(),
        prompt: "look".into(),
        mcp_url: "http://127.0.0.1:1/t/mcp".into(),
        allowed_tools: Some(vec!["Read".into(), "Grep".into()]),
        permission_mode: "default".into(),
        resume_session: "opencode:s1".into(),
        worktree: dir.path().into(),
        ..Default::default()
    };
    let res = run(&bin, req, cx).await.unwrap();
    assert_eq!(res.session_id, "opencode:s1");
    assert!(approver.asked.lock().is_empty());
    let log = logged(dir.path());
    assert!(find(&log, "session/resume").contains(r#""sessionId":"s1""#));
    assert!(
        find(&log, "session/set_config_option")
            .contains(r#""configId":"mode","sessionId":"s1","value":"plan""#)
    );
    assert!(
        find(&log, "session/prompt").contains(r#""text":"look""#),
        "resumed: no role"
    );
    assert!(log.contains(
        &r#"{"id":900,"jsonrpc":"2.0","result":{"outcome":{"optionId":"no","outcome":"selected"}}}"#.to_owned()
    ));
    assert!(
        log.contains(
            &r#"{"id":902,"jsonrpc":"2.0","result":{"outcome":{"optionId":"once","outcome":"selected"}}}"#.to_owned()
        ),
        "Recur's own MCP tools are approved without asking"
    );
}

/// Antigravity shapes: string request ids echoed as is, only allow_once and
/// reject_once on offer, models from configOptions over `models`.
#[tokio::test]
async fn antigravity_string_ids_and_once_only_options() {
    let dir = tempfile::tempdir().unwrap();
    let body = r#"next; reply "$(id_of "$line")" '{"sessionId":"s1","models":{"currentModelId":"g-high","availableModels":[{"modelId":"g-high"}]},"configOptions":[{"id":"model","category":"model","currentValue":"g","options":[{"value":"g"}]}]}'
next; p=$(id_of "$line")
printf '%s\n' '{"jsonrpc":"2.0","method":"session/request_permission","params":{"sessionId":"s1","toolCall":{"toolCallId":"c","name":"run_command","kind":"execute","status":"pending","title":"echo hi","rawInput":{"CommandLine":"echo hi"}},"options":[{"optionId":"allow","kind":"allow_once"},{"optionId":"deny","kind":"reject_once"}]},"id":"1"}'
next
reply "$p" '{"stopReason":"end_turn"}'
while next; do :; done
"#;
    let bin = fake(dir.path(), CAPS, body);
    let rec = EventRecorder::default();
    let mut cx = TurnContext::new(CancellationToken::new(), Arc::new(rec.clone()));
    cx.permissions = Some(Arc::new(Approver::default()));
    let req = TurnRequest {
        model: "g".into(),
        prompt: "x".into(),
        permission_mode: "bypassPermissions".into(),
        worktree: dir.path().into(),
        ..Default::default()
    };
    run(&bin, req, cx).await.unwrap();
    let log = logged(dir.path());
    assert!(log.contains(
        &r#"{"id":"1","jsonrpc":"2.0","result":{"outcome":{"optionId":"allow","outcome":"selected"}}}"#.to_owned()
    ));
    assert!(
        find(&log, "session/set_config_option").is_empty()
            && find(&log, "session/set_model").is_empty(),
        "the configOptions model is already current"
    );
    let step = rec.take().into_iter().find_map(|e| match e {
        AgentEvent::Step(s) => Some(s),
        _ => None,
    });
    assert_eq!(step.unwrap().name, "run_command");
}

#[tokio::test]
async fn load_drops_replay_and_failed_resume_starts_fresh() {
    let caps = r#"{"protocolVersion":1,"agentCapabilities":{"loadSession":true}}"#;
    let dir = tempfile::tempdir().unwrap();
    let body = r#"next
upd '{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"old history"}}'
reply "$(id_of "$line")" '{}'
next; p=$(id_of "$line")
upd '{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"new"}}'
reply "$p" '{"stopReason":"end_turn"}'
while next; do :; done
"#;
    let bin = fake(dir.path(), caps, body);
    let rec = EventRecorder::default();
    let cx = TurnContext::new(CancellationToken::new(), Arc::new(rec.clone()));
    let req = TurnRequest {
        prompt: "go".into(),
        resume_session: "opencode:s1".into(),
        worktree: dir.path().into(),
        ..Default::default()
    };
    let res = run(&bin, req, cx).await.unwrap();
    assert_eq!(res.text, "new");
    assert!(find(&logged(dir.path()), "session/load").contains(r#""sessionId":"s1""#));
    assert!(
        rec.take()
            .iter()
            .all(|e| !matches!(e, AgentEvent::Text { text, .. } if text.contains("old")))
    );

    let dir = tempfile::tempdir().unwrap();
    let body = r#"next; printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32002,"message":"no such session"}}\n' "$(id_of "$line")"
next; reply "$(id_of "$line")" '{"sessionId":"s2"}'
next; reply "$(id_of "$line")" '{"stopReason":"end_turn"}'
while next; do :; done
"#;
    let bin = fake(dir.path(), caps, body);
    let cx = TurnContext::new(CancellationToken::new(), Arc::new(EventRecorder::default()));
    let req = TurnRequest {
        system_prompt: "Role.".into(),
        prompt: "go".into(),
        resume_session: "opencode:gone".into(),
        worktree: dir.path().into(),
        ..Default::default()
    };
    let res = run(&bin, req, cx).await.unwrap();
    assert_eq!(res.session_id, "opencode:s2");
    assert!(
        find(&logged(dir.path()), "session/prompt").contains("<role-instructions>"),
        "a failed resume re-attaches the role"
    );
}

#[tokio::test]
async fn cancel_sends_session_cancel_and_detaches() {
    let dir = tempfile::tempdir().unwrap();
    let body = r#"next; reply "$(id_of "$line")" '{"sessionId":"s1"}'
while next; do :; done
"#;
    let bin = fake(dir.path(), CAPS, body);
    let rec = EventRecorder::default();
    let cancel = CancellationToken::new();
    let cx = TurnContext::new(cancel.clone(), Arc::new(rec.clone()));
    let req = TurnRequest {
        prompt: "x".into(),
        worktree: dir.path().into(),
        ..Default::default()
    };
    let b = bin.clone();
    let task = tokio::spawn(async move { run(&b, req, cx).await });
    while find(&logged(dir.path()), "session/prompt").is_empty() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    cancel.cancel();
    let err = task.await.unwrap().unwrap_err();
    assert!(err.is_canceled());
    assert_eq!(err.partial.session_id, "opencode:s1");
    assert!(rec.take().iter().all(|e| e.kind() != "result"));
    assert_eq!(
        find(&logged(dir.path()), "session/cancel"),
        r#"{"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"s1"}}"#
    );
}

#[tokio::test]
async fn auth_and_crash_errors() {
    let dir = tempfile::tempdir().unwrap();
    let body = r#"next; printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32000,"message":"Authentication required"}}\n' "$(id_of "$line")"
while next; do :; done
"#;
    let bin = fake(dir.path(), CAPS, body);
    let req = TurnRequest {
        prompt: "x".into(),
        worktree: dir.path().into(),
        ..Default::default()
    };
    let cx = TurnContext::new(CancellationToken::new(), Arc::new(EventRecorder::default()));
    let err = run(&bin, req.clone(), cx).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Auth);
    assert_eq!(
        err.message,
        "OpenCode is not signed in: Run `opencode auth login` in a terminal"
    );

    let body = r#"next; reply "$(id_of "$line")" '{"sessionId":"s1"}'
next; echo 'panic: boom' >&2; exit 1
"#;
    let bin = fake(dir.path(), CAPS, body);
    let rec = EventRecorder::default();
    let cx = TurnContext::new(CancellationToken::new(), Arc::new(rec.clone()));
    let err = run(&bin, req, cx).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert_eq!(
        err.message,
        "OpenCode exited before the turn completed\npanic: boom"
    );
    assert_eq!(err.partial.stderr, "panic: boom");
    assert!(rec.take().iter().all(|e| e.kind() != "result"));
}

/// Re-runs itself with `RECUR_OPENCODE` pointing at the fake, so the
/// provider resolves it without mutating this process's environment.
#[tokio::test]
async fn provider_probes_and_caches_models() {
    let Some(dir) = std::env::var_os("ACP_FAKE_DIR").map(PathBuf::from) else {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("opts"), OPTIONS).unwrap();
        let body = format!(
            "OPTS='{}'\n{}",
            dir.path().join("opts").display(),
            r#"next; reply "$(id_of "$line")" "$(cat "$OPTS")"
next; reply "$(id_of "$line")" 'null'
while next; do :; done
"#
        );
        let bin = fake(dir.path(), CAPS, &body);
        let ok = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "turn::provider_probes_and_caches_models",
                "--exact",
                "--nocapture",
            ])
            .env("ACP_FAKE_DIR", dir.path())
            .env("RECUR_OPENCODE", &bin)
            .status()
            .unwrap()
            .success();
        assert!(ok, "re-run failed");
        return;
    };
    let dir = dir.as_path();
    let bin = dir.join("agent");
    let p = AcpProvider::new(
        opencode(),
        AcpConfig {
            data_dir: dir.join("data"),
            env_scrub: Vec::new(),
        },
    );
    assert_eq!(p.locate().unwrap(), bin);
    let st = p.status(&Settings::default()).await;
    assert!(st.installed && st.authenticated, "{st:?}");
    assert_eq!(
        (st.version.as_str(), st.model_count, st.error.as_str()),
        ("9.9.9", 2, "")
    );
    assert!(dir.join("data/acp-probe/opencode").is_dir());
    let log = logged(dir);
    assert!(find(&log, "session/close").contains(r#""sessionId":"s1""#));

    let models = p.models(&Settings::default()).await.unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(
        (models[1].model_ref.as_str(), models[1].label.as_str()),
        ("opencode/fake/b", "Fake B")
    );
    assert_eq!(models[0].flags[0].id, "effort");
    let inits = |log: &[String]| {
        log.iter()
            .filter(|l| l.contains(r#""method":"initialize""#))
            .count()
    };
    assert_eq!(inits(&logged(dir)), 1, "models() reused the status probe");
    assert_eq!(p.model_label("fake/b"), "Fake B");
    assert_eq!(p.model_label("other"), "other");
    assert_eq!(p.cached_models(&Settings::default()).len(), 2);

    let off = Settings {
        disabled_providers: vec!["opencode".into()],
        ..Default::default()
    };
    assert!(p.models(&off).await.unwrap().is_empty());
    assert!(p.resume_handle_fits("opencode:ses_1") && p.resume_handle_fits(""));
    assert!(!p.resume_handle_fits("antigravity:x") && !p.resume_handle_fits("ses_1"));
}

#[tokio::test]
async fn probe_reports_login_hint() {
    let dir = tempfile::tempdir().unwrap();
    let body = r#"next; printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32000,"message":"Authentication required"}}\n' "$(id_of "$line")"
while next; do :; done
"#;
    let bin = fake(dir.path(), CAPS, body);
    let ag = acp_agent("antigravity").unwrap();
    let p = agent::probe(ag, &Launch::plain(ag, &bin), dir.path())
        .await
        .unwrap();
    assert_eq!(p.version, "9.9.9");
    assert_eq!(
        p.session.unwrap_err(),
        "Antigravity is not signed in: Run `agy` in a terminal and sign in with Google"
    );
}

/// Live smoke against a signed-in OpenCode with its free model:
///
/// ```text
/// cargo nextest run -p recur-provider-acp --run-ignored only opencode_live_smoke
/// ```
#[tokio::test]
#[ignore = "needs a signed-in opencode on PATH"]
async fn opencode_live_smoke() {
    let p = AcpProvider::new(
        opencode(),
        AcpConfig {
            data_dir: tempfile::tempdir().unwrap().keep(),
            env_scrub: Vec::new(),
        },
    );
    let st = p.status(&Settings::default()).await;
    assert!(st.authenticated, "{st:?}");
    assert!(
        p.cached_models(&Settings::default())
            .iter()
            .any(|m| m.model_ref == "opencode/opencode/big-pickle")
    );
    let wt = tempfile::tempdir().unwrap();
    let rec = EventRecorder::default();
    let cx = TurnContext::new(CancellationToken::new(), Arc::new(rec.clone()));
    let req = TurnRequest {
        model: "opencode/big-pickle".into(),
        prompt: "Reply with exactly the word: pong".into(),
        worktree: wt.path().into(),
        include_partials: true,
        ..Default::default()
    };
    let res = tokio::time::timeout(Duration::from_secs(240), p.run_turn(req.clone(), cx))
        .await
        .expect("live turn hung")
        .unwrap();
    assert!(!res.is_error, "{res:?}");
    assert!(
        res.session_id.starts_with("opencode:ses_"),
        "{}",
        res.session_id
    );
    assert!(res.text.to_lowercase().contains("pong"), "{}", res.text);
    assert_eq!(rec.take().last().map(AgentEvent::kind), Some("result"));

    let cx = TurnContext::new(CancellationToken::new(), Arc::new(rec.clone()));
    let again = TurnRequest {
        prompt: "What word did you just reply with? One word.".into(),
        resume_session: res.session_id.clone(),
        ..req
    };
    let res2 = tokio::time::timeout(Duration::from_secs(240), p.run_turn(again, cx))
        .await
        .expect("live resume hung")
        .unwrap();
    assert_eq!(res2.session_id, res.session_id);
    assert!(res2.text.to_lowercase().contains("pong"), "{}", res2.text);
}

/// Live smoke against a signed-in Antigravity CLI:
///
/// ```text
/// cargo nextest run -p recur-provider-acp --run-ignored only antigravity_live_smoke
/// ```
#[tokio::test]
#[ignore = "needs a signed-in agy on PATH"]
async fn antigravity_live_smoke() {
    let p = AcpProvider::new(
        acp_agent("antigravity").unwrap(),
        AcpConfig {
            data_dir: tempfile::tempdir().unwrap().keep(),
            env_scrub: Vec::new(),
        },
    );
    let st = p.status(&Settings::default()).await;
    assert!(st.authenticated, "{st:?}");
    let models = p.cached_models(&Settings::default());
    assert!(
        models
            .iter()
            .any(|m| m.model_ref == "antigravity/gemini-3.8-flash"),
        "{models:?}"
    );
    assert!(models[0].flags.iter().any(|f| f.id == "effort"));
    let wt = tempfile::tempdir().unwrap();
    let rec = EventRecorder::default();
    let cx = TurnContext::new(CancellationToken::new(), Arc::new(rec.clone()));
    let req = TurnRequest {
        model: "gemini-3.8-flash".into(),
        flags: [("effort".to_owned(), "low".to_owned())].into(),
        prompt: "Run the shell command `echo hi > hi.txt`, then reply with exactly the word: pong"
            .into(),
        permission_mode: "bypassPermissions".into(),
        worktree: wt.path().into(),
        include_partials: true,
        ..Default::default()
    };
    let res = tokio::time::timeout(Duration::from_secs(240), p.run_turn(req.clone(), cx))
        .await
        .expect("live turn hung")
        .unwrap();
    assert!(!res.is_error, "{res:?}");
    assert!(
        res.session_id.starts_with("antigravity:"),
        "{}",
        res.session_id
    );
    assert!(res.text.to_lowercase().contains("pong"), "{}", res.text);
    assert!(
        res.usage.input_tokens > 0 && res.usage.context_window > 0,
        "{:?}",
        res.usage
    );
    assert!(wt.path().join("hi.txt").exists());
    let evs = rec.take();
    let step = evs.iter().find_map(|e| match e {
        AgentEvent::Step(s) => Some(s.clone()),
        _ => None,
    });
    let step = step.expect("a tool step");
    assert_eq!(
        p.display_tool(&step.name, &step.input).name,
        "Bash",
        "{step:?}"
    );
    assert_eq!(evs.last().map(AgentEvent::kind), Some("result"));

    let cx = TurnContext::new(CancellationToken::new(), Arc::new(rec.clone()));
    let again = TurnRequest {
        prompt: "What word did you just reply with? One word.".into(),
        resume_session: res.session_id.clone(),
        ..req.clone()
    };
    let res2 = tokio::time::timeout(Duration::from_secs(240), p.run_turn(again, cx))
        .await
        .expect("live resume hung")
        .unwrap();
    assert_eq!(res2.session_id, res.session_id);
    assert!(res2.text.to_lowercase().contains("pong"), "{}", res2.text);

    rec.take();
    let cx = TurnContext::new(CancellationToken::new(), Arc::new(rec.clone()));
    let blocked = TurnRequest {
        prompt: "Call your schedule tool once to set a one-time 60 second timer with prompt \
                 'ping', then reply with what it returned."
            .into(),
        resume_session: String::new(),
        ..req
    };
    tokio::time::timeout(Duration::from_secs(240), p.run_turn(blocked, cx))
        .await
        .expect("live blocked turn hung")
        .unwrap();
    let denied = rec.take().into_iter().find_map(|e| match e {
        AgentEvent::ToolResult(r) if r.name == "schedule" => Some(r),
        _ => None,
    });
    let denied = denied.expect("a schedule call");
    assert!(denied.is_error, "{denied:?}");
}
