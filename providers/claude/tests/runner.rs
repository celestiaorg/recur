//! `ClaudeProvider::run_turn` against fake `claude` shell scripts: Go
//! `claude_inject_test.go` (stdin close on result, background-task hold,
//! steer nack, env scrub) plus steer delivery, process-group kill and the
//! no-result errors.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use recur_agent::testing::EventRecorder;
use recur_agent::{
    AgentEvent, ErrorKind, Provider, ProviderError, Steer, SteerInbox, TurnContext, TurnRequest,
    TurnResult, UserImage,
};
use recur_provider_claude::input::user_message_line;
use recur_provider_claude::{ClaudeConfig, ClaudeProvider};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const RESULT_DONE: &str = r#"echo '{"type":"result","subtype":"success","session_id":"s1","is_error":false,"num_turns":1,"duration_ms":1,"result":"done","total_cost_usd":0.01}'"#;

fn fake(dir: &Path, body: &str) -> PathBuf {
    let bin = dir.join("fake-claude");
    recur_exec::write_executable(&bin, format!("#!/bin/sh\n{body}"), 0o755).unwrap();
    bin
}

fn provider(bin: &Path) -> ClaudeProvider {
    ClaudeProvider::new(ClaudeConfig {
        bin: bin.to_string_lossy().into_owned(),
        ..Default::default()
    })
}

fn req(prompt: &str, dir: &Path) -> TurnRequest {
    TurnRequest {
        prompt: prompt.into(),
        worktree: dir.into(),
        ..Default::default()
    }
}

struct Turn {
    rec: EventRecorder,
    cancel: CancellationToken,
    steer: Option<mpsc::Sender<Steer>>,
}

impl Turn {
    fn new(steerable: bool) -> (Self, TurnContext) {
        let rec = EventRecorder::default();
        let cancel = CancellationToken::new();
        let mut cx = TurnContext::new(cancel.clone(), Arc::new(rec.clone()));
        let steer = steerable.then(|| {
            let (tx, rx) = mpsc::channel(8);
            cx.steer = Some(SteerInbox(rx));
            tx
        });
        (Self { rec, cancel, steer }, cx)
    }

    fn results(&self) -> Vec<String> {
        self.rec
            .take()
            .into_iter()
            .filter_map(|e| match e {
                AgentEvent::TurnEnd(r) => Some(r.text),
                _ => None,
            })
            .collect()
    }
}

async fn run(
    p: &ClaudeProvider,
    r: TurnRequest,
    cx: TurnContext,
    limit: Duration,
) -> Result<TurnResult, ProviderError> {
    tokio::time::timeout(limit, p.run_turn(r, cx))
        .await
        .expect("run_turn hung (stdin/stdout deadlock?)")
}

/// The D49 deadlock regression: under `--input-format stream-json` claude
/// stays alive after `result` until stdin EOF, so the runner must close
/// stdin on the result.
#[tokio::test]
async fn closes_stdin_on_result() {
    let dir = tempfile::tempdir().unwrap();
    let bin = fake(dir.path(), &format!("{RESULT_DONE}\ncat >/dev/null\n"));
    let (_turn, cx) = Turn::new(true);
    let res = run(
        &provider(&bin),
        req("do the thing", dir.path()),
        cx,
        Duration::from_secs(3),
    )
    .await
    .unwrap();
    assert_eq!(
        (res.subtype.as_str(), res.text.as_str()),
        ("success", "done")
    );
}

const BG_LIVE: &str = r#"echo '{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"b1","task_type":"local_bash"}]}'"#;
const BG_EMPTY: &str =
    r#"echo '{"type":"system","subtype":"background_tasks_changed","tasks":[]}'"#;
const BG_AMBIENT: &str = r#"echo '{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"w1","task_type":"monitor_ws","ambient":true}]}'"#;

fn bg_result(text: &str) -> String {
    format!(
        r#"echo '{{"type":"result","subtype":"success","session_id":"s1","is_error":false,"num_turns":1,"duration_ms":1,"result":"{text}"}}'"#
    )
}

/// Runs `body` in a fake claude where `$EOF` exists once the runner closed
/// stdin; returns the final result and every emitted result's text.
async fn run_bg(body: &str, ceiling: Duration) -> (TurnResult, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    let eof = dir.path().join("eof");
    let script = format!(
        "EOF='{}'\nexec 3<&0\n(cat <&3 >/dev/null; touch \"$EOF\") &\n{body}wait\n",
        eof.display()
    );
    let bin = fake(dir.path(), &script);
    let p = ClaudeProvider::new(ClaudeConfig {
        bin: bin.to_string_lossy().into_owned(),
        background_hold_ceiling: ceiling,
    });
    let (turn, cx) = Turn::new(true);
    let res = run(&p, req("bench", dir.path()), cx, Duration::from_secs(5))
        .await
        .unwrap();
    (res, turn.results())
}

const LONG: Duration = Duration::from_secs(600);

#[tokio::test]
async fn keeps_stdin_open_for_background_tasks() {
    let body = format!(
        "{BG_LIVE}\n{}\nsleep 0.3\n[ -e \"$EOF\" ] && exit 0\n{BG_EMPTY}\n{}\n",
        bg_result("waiting"),
        bg_result("got it")
    );
    let (res, results) = run_bg(&body, LONG).await;
    assert_eq!(res.text, "got it");
    assert_eq!(results, ["got it"], "only the post-task result is emitted");
}

/// A resumed session first replays tasks the previous process killed as a
/// model-less result; it must not close stdin before the real turn's tasks.
#[tokio::test]
async fn ignores_resume_task_replay() {
    let replay = r#"echo '{"type":"result","subtype":"success","session_id":"s1","is_error":false,"num_turns":0,"duration_ms":31,"result":"","origin":{"kind":"task-notification"}}'"#;
    let body = format!(
        "{replay}\nsleep 0.1\n{BG_LIVE}\n{}\nsleep 0.3\n[ -e \"$EOF\" ] && exit 0\n{BG_EMPTY}\n{}\n",
        bg_result("waiting"),
        bg_result("got it")
    );
    let (res, results) = run_bg(&body, LONG).await;
    assert_eq!(res.text, "got it");
    assert_eq!(results, ["got it"]);
}

#[tokio::test]
async fn ignores_ambient_background_tasks() {
    let body = format!(
        "{BG_AMBIENT}\n{}\ni=0\nwhile [ ! -e \"$EOF\" ] && [ $i -lt 50 ]; do sleep 0.1; i=$((i+1)); done\n[ -e \"$EOF\" ] || {}\n",
        bg_result("done"),
        bg_result("held")
    );
    let (res, results) = run_bg(&body, LONG).await;
    assert_eq!(res.text, "done");
    assert_eq!(results, ["done"]);
}

/// Tasks ending without a follow-up turn close stdin, and the held result is
/// still emitted.
#[tokio::test]
async fn closes_stdin_when_background_tasks_end() {
    let body = format!(
        "{BG_LIVE}\n{}\nsleep 0.1\n{BG_EMPTY}\n",
        bg_result("waiting")
    );
    let (res, results) = run_bg(&body, LONG).await;
    assert_eq!(res.text, "waiting");
    assert_eq!(results, ["waiting"]);
}

/// A task that never ends (dev server) holds stdin only until the ceiling.
#[tokio::test]
async fn background_hold_ceiling() {
    let body = format!("{BG_LIVE}\n{}\n", bg_result("server up"));
    let (res, results) = run_bg(&body, Duration::from_millis(100)).await;
    assert_eq!(res.text, "server up");
    assert_eq!(results, ["server up"]);
}

/// A plain steer is acked only after a stdin write; one still pending when
/// the turn unwinds nacks so the engine falls back to a new turn.
#[tokio::test]
async fn nacks_unflushed_plain_steer() {
    let dir = tempfile::tempdir().unwrap();
    let bin = fake(dir.path(), "cat >/dev/null\n");
    let (turn, cx) = Turn::new(true);
    let p = provider(&bin);
    let r = req("do the thing", dir.path());
    let task = tokio::spawn(async move { p.run_turn(r, cx).await });
    let (s, mut ack) = Steer::new("follow-up", Some("follow-up".into()));
    turn.steer.as_ref().unwrap().send(s).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(80), &mut ack)
            .await
            .is_err(),
        "acked before the stdin write"
    );
    turn.cancel.cancel();
    let delivered = tokio::time::timeout(Duration::from_secs(3), ack)
        .await
        .expect("pending plain steer was not nacked on unwind")
        .unwrap_or(false);
    assert!(!delivered);
    let err = task.await.unwrap().unwrap_err();
    assert!(err.is_canceled());
}

/// A tool-bearing turn: the steer waits for the first tool_result slot, is
/// written as a stream-json user message, shows as a live user row, and
/// acks true. A conductor steer acks on receipt and carries the Maestro
/// prefix.
#[tokio::test]
async fn flushes_steers_on_tool_result() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("stdin.ndjson");
    let body = format!(
        r#"read -r seed
printf '%s\n' "$seed" > '{out}'
echo '{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"t1","name":"Bash","input":{{"command":"ls"}}}}]}}}}'
sleep 0.2
echo '{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"t1","content":"ok"}}]}}}}'
read -r plain
printf '%s\n' "$plain" >> '{out}'
read -r conductor
printf '%s\n' "$conductor" >> '{out}'
{RESULT_DONE}
cat >/dev/null
"#,
        out = out.display()
    );
    let bin = fake(dir.path(), &body);
    let (turn, cx) = Turn::new(true);
    let p = provider(&bin);
    let r = req("go", dir.path());
    let task = tokio::spawn(async move { p.run_turn(r, cx).await });
    let tx = turn.steer.clone().unwrap();
    let (plain, plain_ack) = Steer::new("check the tests", Some("Check the tests".into()));
    tx.send(plain).await.unwrap();
    let (conductor, conductor_ack) = Steer::new("tighten scope", None);
    tokio::time::sleep(Duration::from_millis(50)).await;
    tx.send(conductor).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(1), conductor_ack)
            .await
            .unwrap()
            .unwrap(),
        "conductor steers ack on receipt"
    );
    let res = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(res.text, "done");
    assert!(plain_ack.await.unwrap());

    let lines = std::fs::read_to_string(&out).unwrap();
    let want = [
        user_message_line("go", &[]),
        user_message_line("check the tests", &[]),
        user_message_line(
            "Maestro steer (follow this; it supersedes earlier plan guidance):\ntighten scope",
            &[],
        ),
    ]
    .concat();
    assert_eq!(lines, want);

    // Go flushes at the tool_result before emitting it, so the user row
    // precedes the result it was delivered after.
    let kinds: Vec<_> = turn.rec.take().iter().map(|e| e.kind()).collect();
    assert_eq!(kinds, ["step", "user", "tool_result", "result"]);
}

#[tokio::test]
async fn user_row_for_plain_steer() {
    let dir = tempfile::tempdir().unwrap();
    let body = format!(
        r#"read -r seed
echo '{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"t1","content":"ok"}}]}}}}'
read -r plain
{RESULT_DONE}
cat >/dev/null
"#
    );
    let bin = fake(dir.path(), &body);
    let (turn, cx) = Turn::new(true);
    let p = provider(&bin);
    let r = req("go", dir.path());
    let (plain, _ack) = Steer::new(" wire text ", Some("Shown".into()));
    turn.steer.as_ref().unwrap().send(plain).await.unwrap();
    run(&p, r, cx, Duration::from_secs(5)).await.unwrap();
    let user = turn
        .rec
        .take()
        .into_iter()
        .find(|e| e.kind() == "user")
        .unwrap();
    assert_eq!(
        user,
        AgentEvent::User {
            text: "wire text".into(),
            display: "Shown".into(),
            live_steer: true
        }
    );
}

/// Go `TestClaudeRunnerStripsLinearAPIKey`, through `env_scrub` (tracker
/// secrets come from `IssueTracker::secret_env`). HOME stands in for the
/// secret because tests cannot set process env safely.
#[tokio::test]
async fn scrubs_env() {
    if std::env::var_os("HOME").is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("env.txt");
    let bin = fake(
        dir.path(),
        &format!("env > '{}'\n{RESULT_DONE}\n", dump.display()),
    );
    let (_turn, cx) = Turn::new(false);
    let mut r = req("x", dir.path());
    r.env_scrub = vec!["HOME".into()];
    run(&provider(&bin), r, cx, Duration::from_secs(3))
        .await
        .unwrap();
    let env = std::fs::read_to_string(&dump).unwrap();
    assert!(!env.lines().any(|l| l.starts_with("HOME=")), "{env}");
    assert!(std::env::var_os("HOME").is_some(), "host env untouched");
}

/// The classic path: prompt on argv, stdin closed, events parsed in order.
#[tokio::test]
async fn stdout_only_turn() {
    let dir = tempfile::tempdir().unwrap();
    let argv = dir.path().join("argv.txt");
    let body = format!(
        r#"for a in "$@"; do printf '%s\n' "$a"; done > '{argv}'
cat >> '{argv}'
echo '{{"type":"system","subtype":"init","session_id":"s1","model":"claude-sonnet-5","mcp_servers":[]}}'
echo 'not json'
{RESULT_DONE}
"#,
        argv = argv.display()
    );
    let bin = fake(dir.path(), &body);
    let (turn, cx) = Turn::new(false);
    let res = run(
        &provider(&bin),
        req("hello", dir.path()),
        cx,
        Duration::from_secs(3),
    )
    .await
    .unwrap();
    assert_eq!(res.session_id, "s1");
    let args = std::fs::read_to_string(&argv).unwrap();
    assert!(
        args.starts_with("-p\nhello\n--output-format\nstream-json\n--verbose\n"),
        "{args}"
    );
    let kinds: Vec<_> = turn.rec.take().iter().map(|e| e.kind()).collect();
    assert_eq!(kinds, ["init", "result"]);
}

/// Images force stream-json input even without steering.
#[tokio::test]
async fn images_go_on_stdin() {
    let dir = tempfile::tempdir().unwrap();
    let seed = dir.path().join("seed.json");
    let bin = fake(
        dir.path(),
        &format!(
            "read -r seed\nprintf '%s\\n' \"$seed\" > '{}'\n{RESULT_DONE}\ncat >/dev/null\n",
            seed.display()
        ),
    );
    let (_turn, cx) = Turn::new(false);
    let mut r = req("look ⟦attachment:img1⟧", dir.path());
    r.images = vec![UserImage {
        id: "img1".into(),
        name: "a.png".into(),
        mime: "image/png".into(),
        data: "AAAA".into(),
        path: "workspaces/W/attachments/img1.png".into(),
    }];
    let want = user_message_line(&r.prompt, &r.images);
    run(&provider(&bin), r, cx, Duration::from_secs(3))
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(&seed).unwrap(), want);
    assert!(want.contains(r#""type":"image""#));
}

/// Cancel kills the whole process group (SIGTERM, then SIGKILL after the
/// grace), including children, and returns `canceled` with the partial
/// result.
#[tokio::test]
async fn cancel_kills_process_group() {
    let dir = tempfile::tempdir().unwrap();
    let pidfile = dir.path().join("child.pid");
    let body = format!(
        "echo '{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s9\",\"model\":\"m\",\"mcp_servers\":[]}}'\nsleep 30 &\necho $! > '{}'\ntrap '' TERM\nwait\n",
        pidfile.display()
    );
    let bin = fake(dir.path(), &body);
    let (turn, cx) = Turn::new(false);
    let p = provider(&bin);
    let r = req("x", dir.path());
    let task = tokio::spawn(async move { p.run_turn(r, cx).await });
    while !pidfile.exists() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    let child: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    turn.cancel.cancel();
    let err = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("cancel did not end the turn")
        .unwrap()
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Canceled);
    let alive = rustix::process::test_kill_process(rustix::process::Pid::from_raw(child).unwrap());
    assert!(alive.is_err(), "group child survived the kill");
}

#[tokio::test]
async fn no_result_errors() {
    let dir = tempfile::tempdir().unwrap();
    let bin = fake(
        dir.path(),
        "echo 'You have hit your usage limit' >&2\nexit 3\n",
    );
    let (_turn, cx) = Turn::new(false);
    let err = run(
        &provider(&bin),
        req("x", dir.path()),
        cx,
        Duration::from_secs(3),
    )
    .await
    .unwrap_err();
    assert_eq!(
        err.message,
        "claude exited without a result: exit status 3: You have hit your usage limit"
    );
    assert_eq!(err.partial.stderr, "You have hit your usage limit\n");
    assert!(err.transient());

    let bin = fake(dir.path(), "exit 0\n");
    let (_turn, cx) = Turn::new(false);
    let err = run(
        &provider(&bin),
        req("x", dir.path()),
        cx,
        Duration::from_secs(3),
    )
    .await
    .unwrap_err();
    assert_eq!(err.message, "claude produced no result line");

    let missing = ClaudeProvider::new(ClaudeConfig {
        bin: "recur-no-such-claude".into(),
        ..Default::default()
    });
    let (_turn, cx) = Turn::new(false);
    let err = missing
        .run_turn(req("x", dir.path()), cx)
        .await
        .unwrap_err();
    assert_eq!(
        err.message,
        r#"start claude: exec: "recur-no-such-claude": executable file not found in $PATH"#
    );
}
