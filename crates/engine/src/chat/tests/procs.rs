//! Background processes: run_background and stop_terminal through the host,
//! the permission gate, and the exit notice turn (coalescing, no notice after
//! a stop or with notify_on_exit off).

use std::sync::Arc;
use std::time::Duration;

use recur_agent::attach::DISPLAY_HIDDEN_SENTINEL;
use recur_agent_tools::{BackgroundRequest, Host};
use recur_domain::{PermissionDecision, Workspace};
use tokio_util::sync::CancellationToken;

use super::test_support::*;
use crate::chat::procs::EXIT_COALESCE;
use crate::host::EngineHost;
use crate::loops::test_support::CaptureScript;
use crate::testing::{TestOptions, wait_for};

struct ProcFx {
    fx: Fx,
    term: Arc<recur_term::Manager>,
    runner: Arc<CaptureScript>,
    ws: Workspace,
}

impl ProcFx {
    async fn new(bypass: bool) -> Self {
        let term = Arc::new(recur_term::Manager::new(|_| {}));
        let runner = capture_same("procs", "noted");
        let fx = Fx::with(
            Some(runner.provider()),
            TestOptions {
                terminals: Some(term.clone()),
                ..Default::default()
            },
        );
        let mut set = fx.te.store.settings();
        set.bypass_permissions = bypass;
        fx.te.store.set_settings(set);
        let ws = fx.workspace("procs").await;
        fx.eng().ensure_pair_agent(&ws.id, "PA", "");
        Self {
            fx,
            term,
            runner,
            ws,
        }
    }

    fn host(&self) -> EngineHost {
        EngineHost::new(Arc::downgrade(self.fx.eng()))
    }

    async fn run(&self, command: &str, notify: bool) -> Result<String, String> {
        self.run_raw(command, notify)
            .await
            .map(|v| v["id"].as_str().unwrap().to_owned())
    }

    async fn run_raw(&self, command: &str, notify: bool) -> Result<serde_json::Value, String> {
        let req = BackgroundRequest {
            command: command.into(),
            notify_on_exit: notify,
            ..Default::default()
        };
        self.host()
            .run_background(&CancellationToken::new(), &self.ws.id, "PA", req)
            .await
            .map(|r| serde_json::from_str(&r).unwrap())
            .map_err(|e| e.0)
    }

    fn turns(&self) -> Vec<recur_agent::TurnRequest> {
        self.runner.specs.lock().clone()
    }

    async fn close(self) {
        self.term.close_all();
        self.fx.te.close().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn exit_sends_one_hidden_notice_to_the_starting_chat() {
    let p = ProcFx::new(true).await;
    let id = p
        .run("sleep 3.2; echo bye-$((1+1)); exit 3", true)
        .await
        .unwrap();
    let listed = p.host().list_terminals(&p.ws.id, "PA").await.unwrap();
    let t = listed.iter().find(|t| t.id == id).expect("listed");
    assert!(t.background);
    assert_eq!(t.started_by, "PA");
    wait_for(Duration::from_secs(5), "exit notice turn", || {
        !p.turns().is_empty()
    })
    .await;
    let turn = &p.turns()[0];
    assert!(turn.prompt.contains(&format!("{id} (")), "{}", turn.prompt);
    assert!(
        turn.prompt.contains("exited with code 3"),
        "{}",
        turn.prompt
    );
    assert!(turn.prompt.contains("bye-2"), "{}", turn.prompt);
    let label = format!(
        "{DISPLAY_HIDDEN_SENTINEL}Background process sleep 3.2; echo bye-$((1+1)); exit 3 exited (3)"
    );
    let items = p.fx.items(&p.ws.id, "PA").await;
    assert!(
        items.iter().any(|i| i.role == "user" && i.text == label),
        "{items:?}"
    );
    let log =
        p.fx.te
            .data_dir
            .join("workspaces")
            .join(&p.ws.id)
            .join("procs")
            .join(format!("{id}.log"));
    assert!(std::fs::read_to_string(&log).unwrap().contains("bye-2"));
    assert!(
        !log.with_extension("json").exists(),
        "an exit while Recur runs drops the record"
    );
    p.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_next_turn_hears_about_processes_recur_quit_stopped() {
    let p = ProcFx::new(true).await;
    let dir =
        p.fx.te
            .data_dir
            .join("workspaces")
            .join(&p.ws.id)
            .join("procs");
    std::fs::create_dir_all(&dir).unwrap();
    let rec = |id: &str, chat: &str, stopped_at: i64| {
        let r = crate::chat::procs::ProcRecord {
            id: id.into(),
            title: format!("{id} server"),
            command: "pnpm dev".into(),
            started_by: chat.into(),
            stopped_at,
        };
        std::fs::write(
            dir.join(format!("{id}.json")),
            serde_json::to_vec(&r).unwrap(),
        )
        .unwrap();
    };
    rec("t1", "PA", 1_795_208_000_000);
    rec("t2", "PA2", 1_795_208_000_000);
    let got = p.fx.eng().stopped_procs_prompt(&p.ws.id, "PA");
    assert!(
        got.contains("t1 server (`pnpm dev`, terminal t1): stopped when Recur quit at"),
        "{got}"
    );
    assert!(
        got.contains("closing a terminal tab never stops them"),
        "{got}"
    );
    assert!(!got.contains("t2"), "{got}");
    assert!(!dir.join("t1.json").exists(), "reported once");
    assert!(dir.join("t2.json").exists(), "another chat's record stays");
    assert_eq!(p.fx.eng().stopped_procs_prompt(&p.ws.id, "PA"), "");
    p.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn exits_coalesce_and_stops_or_quiet_processes_send_nothing() {
    let p = ProcFx::new(true).await;
    let (a, b, quiet, long) = tokio::join!(
        p.run("sleep 3.2; exit 1", true),
        p.run("sleep 3.6; exit 2", true),
        p.run("sleep 3.2; exit 5", false),
        p.run("sleep 30", true),
    );
    let (a, b, quiet, long) = (a.unwrap(), b.unwrap(), quiet.unwrap(), long.unwrap());
    assert!(
        p.fx.eng()
            .stop_terminal_from_chat(&p.ws.id, &long)
            .unwrap()
            .starts_with("stopped background process")
    );
    wait_for(Duration::from_secs(5), "exit notice turn", || {
        !p.turns().is_empty()
    })
    .await;
    tokio::time::sleep(EXIT_COALESCE + Duration::from_millis(500)).await;
    let turns = p.turns();
    assert_eq!(turns.len(), 1, "one notice for both exits");
    let prompt = &turns[0].prompt;
    assert!(prompt.contains(&a) && prompt.contains(&b), "{prompt}");
    assert!(
        !prompt.contains(&quiet) && !prompt.contains(&long),
        "{prompt}"
    );
    p.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_exit_after_its_chat_closed_does_not_reopen_it() {
    let p = ProcFx::new(true).await;
    p.fx.eng().ensure_pair_agent(&p.ws.id, "PA2", "");
    p.run("sleep 3.2; exit 1", true).await.unwrap();
    p.fx.eng().close_workspace_chat(&p.ws.id, "PA").await;
    tokio::time::sleep(Duration::from_millis(3200) + EXIT_COALESCE + Duration::from_secs(1)).await;
    assert!(p.turns().is_empty(), "no notice turn on a closed chat");
    assert!(
        p.fx.te.store.agent(&p.ws.id, "PA").is_none(),
        "stays closed"
    );
    p.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_fast_exit_is_reported_in_the_result_not_a_notice() {
    let p = ProcFx::new(true).await;
    let v = p.run_raw("echo hi-$((1+1)); exit 3", true).await.unwrap();
    assert!(v["output"].as_str().unwrap().contains("hi-2"), "{v}");
    assert_eq!(v["exited"], true, "{v}");
    assert_eq!(v["exitCode"], 3, "{v}");
    tokio::time::sleep(EXIT_COALESCE + Duration::from_millis(500)).await;
    assert!(
        p.turns().is_empty(),
        "no notice for an exit already reported"
    );
    p.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_terminal_refuses_tabs_the_user_opened() {
    let p = ProcFx::new(true).await;
    let tab = p
        .term
        .open(&p.ws.id, &p.ws.worktree_path, 80, 24, &[])
        .unwrap();
    let err = p
        .host()
        .stop_terminal(&p.ws.id, "PA", &tab.id)
        .await
        .unwrap_err();
    assert!(err.0.contains("a tab the user opened"), "{err}");
    assert!(p.term.list(&p.ws.id).iter().any(|t| t.id == tab.id));
    p.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn ask_mode_routes_run_background_through_approval() {
    let p = ProcFx::new(false).await;
    let task = {
        let (h, ws) = (p.host(), p.ws.id.clone());
        tokio::spawn(async move {
            let req = BackgroundRequest {
                command: "sleep 30".into(),
                ..Default::default()
            };
            h.run_background(&CancellationToken::new(), &ws, "PA", req)
                .await
        })
    };
    let eng = p.fx.eng().clone();
    wait_for(Duration::from_secs(5), "permission request", || {
        !eng.pending_permissions().is_empty()
    })
    .await;
    let req = eng.pending_permissions().remove(0);
    assert_eq!(req.tool, "mcp__recur__run_background");
    assert!(eng.answer_permission(
        &req.id,
        PermissionDecision {
            allow: false,
            reason: "not now".into(),
            ..Default::default()
        }
    ));
    let err = task.await.unwrap().unwrap_err();
    assert_eq!(err.0, "permission denied: not now");
    assert!(p.term.list(&p.ws.id).is_empty(), "nothing may start");
    p.close().await;
}
