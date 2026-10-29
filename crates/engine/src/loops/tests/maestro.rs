//! Maestro tests: the conductor's reply and session resume,
//! its tool scope, standing-prompt refresh, the conductor-not-specialist
//! prompt, the `[[loop]]` mission on follow-ups, failure display, live
//! deltas, running beside a live walk, and steering within the rails.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use recur_agent_tools::Host;
use recur_agent_tools::names::mcp;
use recur_domain::{AgentRole, AutonomyLevel, WorkspaceState};

use crate::bus::EngineEvent;
use crate::codes::MAESTRO_AGENT_CODE;
use crate::host::EngineHost;
use crate::loops::test_support::*;
use crate::roles::{MaestroContext, maestro_system_prompt};
use crate::testing::{TestEngine, add_project, init_repo, wait_for};
use crate::turn::test_support::{maestro_fixture, started_loop};
use crate::turn::transcript::content_items;

async fn maestro_engine(
    runner: &Arc<CaptureScript>,
) -> (TestEngine, String, crate::testing::TempRepo) {
    let t = TestEngine::new(Some(runner.provider()));
    let repo = init_repo();
    let proj = add_project(&t.store, &repo.path);
    let id = started_loop(&t.engine, &proj).await;
    (t, id, repo)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn maestro_chat_reply_and_resume() {
    let runner = CaptureScript::new(|call| match call {
        0 => maestro_fixture(
            "maestro-sess-1",
            "Code and review are done — converged to 0 findings.",
        ),
        _ => maestro_fixture(
            "maestro-sess-1",
            "Google is implemented and covered by passing tests.",
        ),
    });
    let (t, ws, _repo) = maestro_engine(&runner).await;
    let reply1 = t
        .engine
        .send_maestro_message(&ws, "where are we?", &[])
        .await;
    assert!(reply1.contains("converged to 0 findings"), "{reply1}");
    let s0 = runner.spec_at(0).unwrap();
    assert_eq!(s0.resume_session, "");
    assert_eq!(s0.role, AgentRole::Maestro);
    let has = |tool: &str| {
        s0.allowed_tools
            .as_ref()
            .is_some_and(|a| a.iter().any(|t| t == tool))
    };
    for tool in ["Edit", "Write", "Bash", "NotebookEdit"] {
        assert!(!has(tool), "the Maestro must not have {tool}");
    }
    for tool in ["Read", "Grep", "Bash(git diff:*)"] {
        assert!(has(tool), "missing read tool {tool}");
    }
    for tool in [
        mcp::EDIT_PLAN,
        mcp::DISPATCH_SPECIALIST,
        mcp::EXTEND_ROUNDS,
        mcp::REENTER_PHASE,
        mcp::RUN_PHASE,
        mcp::LIST_TERMINALS,
        mcp::READ_TERMINAL,
    ] {
        assert!(has(tool), "missing {tool}");
    }
    assert_eq!(t.engine.maestro_session(&ws), "maestro-sess-1");
    let chat1 = content_items(&t.engine.maestro_chat_items(&ws).await);
    assert_eq!(chat1.len(), 2, "{chat1:?}");
    assert_eq!(
        (chat1[0].role.as_str(), chat1[0].text.as_str()),
        ("user", "where are we?")
    );
    assert_eq!(chat1[1].role, "assistant");

    let reply2 = t
        .engine
        .send_maestro_message(&ws, "what about google?", &[])
        .await;
    assert!(reply2.contains("Google is implemented"), "{reply2}");
    assert_eq!(
        content_items(&t.engine.maestro_chat_items(&ws).await).len(),
        4
    );
    let footers = t
        .engine
        .maestro_chat_items(&ws)
        .await
        .iter()
        .filter(|it| it.kind == "turn")
        .count();
    assert_eq!(footers, 2);
    let s1 = runner.spec_at(1).expect("a second turn");
    assert_eq!(s1.resume_session, "maestro-sess-1");
    assert!(s0.include_partials);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn maestro_refresh_standing_when_context_changes() {
    let runner = CaptureScript::new(|call| maestro_fixture(&format!("maestro-ctx-{call}"), "ok"));
    let (t, ws, _repo) = maestro_engine(&runner).await;
    t.engine
        .send_maestro_message(&ws, "where are we?", &[])
        .await;
    t.engine
        .send_maestro_message(&ws, "still there?", &[])
        .await;
    assert!(
        !runner.spec_at(1).unwrap().refresh_standing,
        "unchanged context"
    );
    t.store.update_workspace(&ws, |lp| {
        lp.state = WorkspaceState::Reviewing;
        lp.iteration = 9;
    });
    t.engine.send_maestro_message(&ws, "and now?", &[]).await;
    let s2 = runner.spec_at(2).unwrap();
    assert!(
        s2.refresh_standing,
        "a changed context refreshes standing notes"
    );
    assert!(
        s2.system_prompt.contains("iteration 9"),
        "{}",
        s2.system_prompt
    );
    t.close().await;
}

#[test]
fn maestro_system_prompt_conductor_not_specialist() {
    let mission = "CHARTER: do not open a PR.";
    let prompt = maestro_system_prompt(&MaestroContext {
        goal: "Review & fix".into(),
        mission: mission.into(),
        state: "reviewing".into(),
        ..MaestroContext::default()
    });
    for want in [
        "do not do the loop's phase work yourself",
        "Read/Grep/Glob only to answer the human",
        "the engine walks the graph",
        "pick by whether the walk is live",
        "paused at a checkpoint",
        "escalated at the round cap",
        mission,
        "Task: Review & fix",
        mcp::MESSAGE_CHILD,
    ] {
        assert!(prompt.contains(want), "missing {want:?}");
    }
    assert!(!prompt.contains("specialists execute this"));
    assert!(!prompt.contains("idle (stopped/failed/paused/settled)"));
    let live_paused = maestro_system_prompt(&MaestroContext {
        state: "paused".into(),
        live: true,
        ..MaestroContext::default()
    });
    assert!(
        live_paused.contains("paused (live walk)")
            && live_paused.contains("Resume continues this walk")
    );
    let dup = maestro_system_prompt(&MaestroContext {
        goal: mission.into(),
        mission: mission.into(),
        state: "reviewing".into(),
        ..MaestroContext::default()
    });
    assert_eq!(dup.matches(mission).count(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn maestro_follow_up_includes_loop_prompt() {
    let runner = CaptureScript::new(|_| maestro_fixture("charter", "OK."));
    let t = TestEngine::new(Some(runner.provider()));
    t.install_test_pack(
        "CharterLoop",
        r#"
[bundle]
name = "Charter"
id = "test.charter"
version = "1.0.0"

[[loop]]
id = "test.charter.loop"
name = "Charter"
conductor = "recur.roles.maestro"
prompt = "CHARTER: do not open a PR."
entry = "converge"

[[loop.phase]]
id = "converge"
uses = "converge"
start = "review"
[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "recur.roles.reviewer"
"#,
    );
    let repo = init_repo();
    let proj = add_project(&t.store, &repo.path);
    let ws = t
        .engine
        .create_workspace(&proj, "scratch", "")
        .await
        .unwrap();
    // Listing chats (E4) creates the PA tab of a fresh workspace.
    t.engine.ensure_pair_agent(&ws.id, "PA", "");
    t.engine
        .remember_chat_loop_policy(&ws.id, "PA", "test.charter.loop", "")
        .await;
    let mut slots = 0;
    while t.engine.governor().try_acquire_loop() {
        slots += 1;
    }
    let w = t.store.workspace(&ws.id).unwrap();
    t.engine
        .send_loop_chat_message(w, "PA", "go", &[], "Review & fix")
        .await;
    let w = t.store.workspace(&ws.id).unwrap();
    t.engine
        .send_loop_chat_message(w, "PA", "where are we?", &[], "")
        .await;
    for _ in 0..slots {
        t.engine.governor().release_loop();
    }
    let specs = runner.with_role(AgentRole::Maestro);
    assert_eq!(specs.len(), 1, "one follow-up conductor turn");
    assert!(
        specs[0]
            .system_prompt
            .contains("CHARTER: do not open a PR.")
    );
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn maestro_failure_keeps_display_text_without_duplicate_user() {
    let runner = CaptureScript::new(|_| {
        vec![
            r#"{"type":"system","subtype":"init","session_id":"m-fail","model":"m","mcp_servers":[]}"#.into(),
            r#"{"type":"result","subtype":"error_during_execution","session_id":"m-fail","is_error":true,"num_turns":1,"duration_ms":12}"#.into(),
        ]
    });
    let (t, ws, _repo) = maestro_engine(&runner).await;
    t.engine
        .send_maestro_message_to(
            &ws,
            MAESTRO_AGENT_CODE,
            "full wire prompt about the findings list",
            &[],
            "Fix findings",
        )
        .await;
    let s0 = runner.with_role(AgentRole::Maestro).remove(0);
    assert_eq!(s0.display_text, "Fix findings");
    let items = content_items(&t.engine.maestro_chat_items(&ws).await);
    let users: Vec<_> = items
        .iter()
        .filter(|it| it.role == "user" && it.kind == "text")
        .collect();
    assert_eq!(users.len(), 1, "{items:?}");
    assert_eq!(users[0].text, "Fix findings");
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn maestro_chat_publishes_deltas() {
    let runner =
        CaptureScript::new(|_| maestro_fixture("maestro-sess-d", "Status: **merged** on `main`."));
    let (t, ws, _repo) = maestro_engine(&runner).await;
    let deltas: Arc<Mutex<Vec<String>>> = Arc::default();
    {
        let (d, w) = (Arc::clone(&deltas), ws.clone());
        t.engine.bus.subscribe_fn(move |ev| {
            if ev.name() != "session.delta" {
                return;
            }
            let v: serde_json::Value = serde_json::from_str(&ev.payload_json()).unwrap_or_default();
            if v["workspaceId"] == w.as_str()
                && let Some(text) = v["text"].as_str()
            {
                d.lock().push(text.to_owned());
            }
        });
    }
    let reply = t
        .engine
        .send_maestro_message(&ws, "where are we?", &[])
        .await;
    let joined = deltas.lock().join("");
    assert!(!joined.is_empty(), "session.delta during the turn");
    assert!(joined.contains("merged"), "{joined}");
    assert!(reply.contains("merged"), "{reply}");
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn maestro_chat_does_not_block_running_loop() {
    let repo = init_repo();
    let gate = tokio_util::sync::CancellationToken::new();
    let t = TestEngine::new(Some(role_router_runner(
        gate.clone(),
        maestro_fixture("sess-conc", "We're mid-coding — iteration 2, going well."),
    )));
    let proj = add_project(&t.store, &repo.path);
    let ws = t
        .engine
        .start_workspace(&proj, "long running task", "", AutonomyLevel::default())
        .await
        .unwrap();
    wait_for(Duration::from_secs(3), "loop agent running", || {
        t.engine.governor().agents_running() >= 1
    })
    .await;
    let reply = tokio::time::timeout(
        Duration::from_secs(5),
        t.engine.send_maestro_message(&ws, "status?", &[]),
    )
    .await
    .expect("the chat must not block on the walk");
    assert!(reply.contains("iteration 2"), "{reply}");
    gate.cancel();
    wait_for(Duration::from_secs(5), "loop to finish", || {
        t.store.workspace(&ws).is_some_and(|l| {
            matches!(
                l.state,
                WorkspaceState::PrOpen | WorkspaceState::Stopped | WorkspaceState::Failed
            )
        })
    })
    .await;
    t.close().await;
}

fn deviations(t: &TestEngine) -> Vec<recur_domain::FeedItem> {
    t.events
        .all()
        .into_iter()
        .filter_map(|e| match e {
            EngineEvent::WorkspaceEvent(w) => Some(w.event),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn maestro_steering_within_rails() {
    let runner =
        CaptureScript::new(|_| maestro_fixture("sess-steer", "Sounds good — re-planning."));
    let (t, ws, _repo) = maestro_engine(&runner).await;
    let host = EngineHost::new(Arc::downgrade(&t.engine));
    host.plan(
        &ws,
        MAESTRO_AGENT_CODE,
        "Split this into two PRs and prioritize the config change first.",
    );
    assert!(
        deviations(&t)
            .iter()
            .any(|it| it.is_deviation && it.text.contains("Re-planned from your chat")),
        "a plan-change deviation"
    );
    host.plan(
        &ws,
        MAESTRO_AGENT_CODE,
        "Skip the browser verify and open the PR now.",
    );
    let evs = deviations(&t);
    assert!(
        evs.iter()
            .any(|it| it.is_deviation && it.text.contains("Skip the browser verify"))
    );
    assert!(
        !evs.iter()
            .any(|it| it.text.contains("Can't do that from chat")),
        "no keyword gate"
    );
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn maestro_steer_reaches_next_agent_turn() {
    let repo = init_repo();
    let runner = Arc::new(SteerCapture::default());
    let t = TestEngine::new(Some(runner.provider()));
    let proj = add_project(&t.store, &repo.path);
    let ws = t
        .engine
        .start_workspace(
            &proj,
            "implement file watcher",
            "",
            AutonomyLevel::default(),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), runner.gated.notified())
        .await
        .expect("the coder turn blocks in the gate");
    let steer = "Build on the existing in-progress worktree changes; do not start from scratch.";
    EngineHost::new(Arc::downgrade(&t.engine)).plan(&ws, MAESTRO_AGENT_CODE, steer);
    runner.gate.cancel();
    wait_for(Duration::from_secs(10), "loop to finish", || {
        t.store.workspace(&ws).is_some_and(|l| {
            matches!(
                l.state,
                WorkspaceState::PrOpen | WorkspaceState::Stopped | WorkspaceState::Failed
            )
        })
    })
    .await;
    let specs = runner.specs.lock().clone();
    let specialists: Vec<_> = specs
        .iter()
        .filter(|s| s.role != AgentRole::Maestro)
        .collect();
    assert!(specialists.len() >= 2);
    assert!(
        !specialists[0].prompt.contains(steer),
        "the in-flight prompt is not mutated"
    );
    let injected = runner.injected.lock().clone();
    assert!(
        injected.first().is_some_and(|s| s.contains(steer)),
        "{injected:?}"
    );
    assert!(
        specialists[1..]
            .iter()
            .any(|s| s.prompt.contains("Maestro steer") && s.prompt.contains(steer)),
        "a newly spawned specialist carries the steer"
    );
    t.close().await;
}
