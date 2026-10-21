//! Port of Go `loop_engage_test.go` through `send_loop_chat_message`: one
//! chat = one loop, engage without a conductor turn, the planner's rename
//! nudge, attachments, refused launches, owner stamping, and
//! `start_chat_loop_walk` against closed tabs. The run-lock, retry
//! sentinel and tab list/close cases go through E4's `SendWorkspaceMessage`
//! / `CloseWorkspaceChat` and are E4's.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use recur_agent::attach::marker;
use recur_agent_tools::names::mcp;
use recur_domain::{AgentRole, ChatAttachment, Checkpoint, WorkspaceState};

use crate::codes::PAIR_AGENT_CODE;
use crate::loops::Walk;
use crate::loops::test_support::*;
use crate::testing::{TempRepo, TestEngine, add_project, init_repo, wait_for};
use crate::turn::mirror::SessionMirror;
use crate::turn::test_support::maestro_fixture;
use crate::turn::transcript::content_items;

async fn scratch(t: &TestEngine, name: &str) -> (String, TempRepo) {
    let repo = init_repo();
    let proj = add_project(&t.store, &repo.path);
    let ws = t.engine.create_workspace(&proj, name, "").await.unwrap();
    (ws.id, repo)
}

async fn send(t: &TestEngine, ws: &str, code: &str, text: &str, display: &str) -> String {
    send_with(t, ws, code, text, &[], display).await
}

async fn send_with(
    t: &TestEngine,
    ws: &str,
    code: &str,
    text: &str,
    atts: &[ChatAttachment],
    display: &str,
) -> String {
    let rec = t.store.workspace(ws).unwrap();
    t.engine
        .send_loop_chat_message(rec, code, text, atts, display)
        .await
}

fn png(id: &str, name: &str) -> ChatAttachment {
    ChatAttachment {
        id: id.into(),
        name: name.into(),
        mime: "image/png".into(),
        data: base64::engine::general_purpose::STANDARD.encode([
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x00,
        ]),
    }
}

fn has_tool(spec: &recur_agent::TurnRequest, tool: &str) -> bool {
    spec.allowed_tools
        .as_ref()
        .is_some_and(|a| a.iter().any(|t| t == tool))
}

/// E4's `CloseWorkspaceChat` as far as the walk sees it: the tab's agent
/// row goes and it stops owning the workspace's loop.
fn close_tab(store: &recur_store::Store, ws: &str, code: &str) {
    store.update_workspace(ws, |w| {
        if w.loop_chat_code == code {
            w.loop_chat_code.clear();
        }
    });
    store.delete_agent(ws, code);
}

/// A settled loop workspace whose PA tab owns it (Go
/// `startChatLoopWalkCloseFixture`).
async fn settled_owner(t: &TestEngine) -> (String, TempRepo) {
    let (ws, repo) = scratch(t, "scratch").await;
    t.store.update_workspace(&ws, |w| {
        w.run_started = true;
        w.loop_chat_code = PAIR_AGENT_CODE.into();
        w.state = WorkspaceState::Paused;
        w.phase = "idle".into();
    });
    t.engine.ensure_pair_agent(&ws, PAIR_AGENT_CODE, "");
    (ws, repo)
}

fn assert_closed_tab_did_not_start_walk(t: &TestEngine, ws: &str) {
    let rec = t.store.workspace(ws).unwrap();
    assert_eq!(rec.loop_chat_code, "");
    assert_ne!(rec.state, WorkspaceState::Failed, "{}", rec.phase);
    assert!(
        !matches!(rec.state, WorkspaceState::Planning | WorkspaceState::Coding),
        "closed tab walk started: {:?}",
        rec.state
    );
    assert_ne!(rec.state, WorkspaceState::Queued, "must not stay queued");
    assert!(t.engine.running_loop_by_id(ws).is_none());
    assert!(t.engine.running_loop_by_chat(ws, PAIR_AGENT_CODE).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loop_mode_engages_run_on_workspace() {
    let runner = CaptureScript::new(|_| maestro_fixture("engage-sess", "OK."));
    let t = TestEngine::new(Some(runner.provider()));
    let (ws, _repo) = scratch(&t, "scratch").await;
    assert!(!t.store.workspace(&ws).unwrap().has_run());
    let slots = take_slots(&t);
    t.engine.ensure_pair_agent(&ws, "PA", "");

    let reply = send(&t, &ws, "PA", "ship dark mode", "").await;
    for bad in ["Follow along", "already has a loop", "I couldn't engage"] {
        assert!(!reply.contains(bad), "{reply}");
    }
    let got = t.store.workspace(&ws).unwrap();
    assert!(got.has_run() && got.run_started, "{got:?}");
    assert_eq!(got.state, WorkspaceState::Queued);
    assert_eq!(got.goal, "scratch", "the existing workspace name");
    assert_eq!(got.loop_chat_code, "PA", "no singleton MA injection");

    t.engine.ensure_pair_agent(&ws, "PA2", "");
    let again = send(&t, &ws, "PA2", "converge the reviews", "").await;
    for bad in ["already running", "another workspace", "I couldn't engage"] {
        assert!(!again.contains(bad), "{again}");
    }
    assert_eq!(t.store.workspaces().len(), 1);
    assert!(t.engine.chat_loop_started(&ws, "PA2"));
    release_slots(&t, slots);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn second_loop_chat_starts_fresh_walk_from_entry() {
    let runner = ConvergeRunner::new(&[0]);
    let t = TestEngine::new(Some(runner.provider()));
    t.install_fixture_pack();
    let (ws, _repo) = scratch(&t, "scratch").await;
    t.store.update_workspace(&ws, |w| {
        w.run_started = true;
        w.loop_id = "test.loops.verify".into();
        w.loop_chat_code = "PA".into();
        w.state = WorkspaceState::Paused;
        w.phase = "idle".into();
        w.checkpoint = Some(Checkpoint {
            phase_id: "pr".into(),
            ..Checkpoint::default()
        });
    });
    t.engine.ensure_pair_agent(&ws, "PA", "");
    t.engine.ensure_pair_agent(&ws, "PA2", "");
    t.engine
        .remember_chat_loop_policy(&ws, "PA2", "test.loops.converge", "test.policies.fail")
        .await;
    let reply = send(&t, &ws, "PA2", "fix the open findings", "").await;
    assert!(
        !reply.contains("already has a loop") && !reply.contains("I couldn't engage"),
        "{reply}"
    );
    wait_for(
        Duration::from_secs(15),
        "fresh converge walk to review or idle",
        || {
            let got = t.store.workspace(&ws).unwrap();
            got.phase == "phase missing"
                || runner.counts().1 > 0
                || got.state == WorkspaceState::Reviewing
        },
    )
    .await;
    let got = t.store.workspace(&ws).unwrap();
    assert_ne!(got.phase, "phase missing", "reused the sibling checkpoint");
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loop_chat_rename_nudge_first_session() {
    let runner = CaptureScript::new(|_| maestro_fixture("loop-nudge", "OK."));
    let t = TestEngine::new(Some(runner.provider()));
    let (ws, _repo) = scratch(&t, "").await;
    let slots = take_slots(&t);
    send(&t, &ws, "PA", "add a dark-mode toggle", "").await;
    assert!(
        runner.with_role(AgentRole::Maestro).is_empty(),
        "first engage ran a conductor turn"
    );
    assert_eq!(
        t.store.workspace(&ws).unwrap().goal,
        "add a dark-mode toggle"
    );
    assert_eq!(t.engine.chat_tab_title(&ws, "PA"), "add a dark-mode toggle");
    release_slots(&t, slots);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loop_mode_planner_rename_nudge() {
    struct Case {
        ws_name: &'static str,
        want_nudge: Vec<&'static str>,
        want_tools: Vec<&'static str>,
        no_nudge: Vec<&'static str>,
        no_tools: Vec<&'static str>,
    }
    let cases = [
        Case {
            ws_name: "",
            want_nudge: vec![
                "first session in this workspace",
                mcp::RENAME_WORKSPACE,
                mcp::RENAME_BRANCH,
                mcp::RENAME_SESSION,
            ],
            want_tools: vec![
                mcp::RENAME_WORKSPACE,
                mcp::RENAME_BRANCH,
                mcp::RENAME_SESSION,
            ],
            no_nudge: vec![],
            no_tools: vec![],
        },
        Case {
            ws_name: "scratch",
            want_nudge: vec!["first message in this chat"],
            want_tools: vec![mcp::RENAME_SESSION],
            no_nudge: vec![
                "first session in this workspace",
                mcp::RENAME_WORKSPACE,
                mcp::RENAME_BRANCH,
            ],
            no_tools: vec![mcp::RENAME_WORKSPACE, mcp::RENAME_BRANCH],
        },
    ];
    for c in cases {
        let runner = CaptureScript::new(|_| maestro_fixture("loop-planner-nudge", "OK."));
        let t = TestEngine::new(Some(runner.provider()));
        let (ws, _repo) = scratch(&t, c.ws_name).await;
        let reply = send(&t, &ws, "PA", "add a dark-mode toggle", "").await;
        assert!(!reply.contains("I couldn't engage"), "{reply}");
        let mut spec = None;
        wait_for(
            Duration::from_secs(10),
            "planner spec with rename nudge",
            || {
                spec = runner
                    .with_role(AgentRole::Maestro)
                    .into_iter()
                    .find(|s| !s.append_system_prompt.is_empty());
                spec.is_some()
            },
        )
        .await;
        let spec = spec.unwrap();
        for want in &c.want_nudge {
            assert!(
                spec.append_system_prompt.contains(want),
                "{}: missing {want}: {}",
                c.ws_name,
                spec.append_system_prompt
            );
        }
        for refuse in &c.no_nudge {
            assert!(
                !spec.append_system_prompt.contains(refuse),
                "{}: has {refuse}",
                c.ws_name
            );
        }
        for tool in &c.want_tools {
            assert!(has_tool(&spec, tool), "{}: missing {tool}", c.ws_name);
        }
        for tool in &c.no_tools {
            assert!(!has_tool(&spec, tool), "{}: has {tool}", c.ws_name);
        }
        t.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loop_engage_skips_conductor_turn() {
    let runner = CaptureScript::new(|_| maestro_fixture("loop-engage", "OK."));
    let t = TestEngine::new(Some(runner.provider()));
    let (ws, _repo) = scratch(&t, "scratch").await;
    let slots = take_slots(&t);
    let mission =
        "Review the current worktree, then fix and re-review until the policy bar passes.";
    let reply = send(&t, &ws, "PA", mission, "Review & fix").await;
    assert_eq!(reply, "", "no conductor turn");
    assert!(runner.with_role(AgentRole::Maestro).is_empty());
    assert!(t.store.workspace(&ws).unwrap().has_run());
    assert_eq!(t.engine.chat_loop_goal(&ws, "PA").0, mission);
    assert_eq!(t.engine.chat_tab_title(&ws, "PA"), "Review & fix");
    let items = content_items(&t.engine.workspace_chat_items(&ws, "PA").await);
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].role, "user");
    assert_eq!(items[0].text, "Review & fix");

    send(&t, &ws, "PA", "where are we?", "").await;
    let specs = runner.with_role(AgentRole::Maestro);
    assert_eq!(specs.len(), 1);
    assert!(specs[0].prompt.contains("where are we?"));
    assert!(
        specs[0]
            .system_prompt
            .contains("do not do the loop's phase work yourself"),
        "{}",
        specs[0].system_prompt
    );
    release_slots(&t, slots);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loop_engage_persists_image_attachments() {
    let runner = CaptureScript::new(|_| maestro_fixture("loop-img", "OK."));
    let t = TestEngine::new(Some(runner.provider()));
    let (ws, _repo) = scratch(&t, "scratch").await;
    let slots = take_slots(&t);
    let m = marker("img1");
    let reply = send_with(
        &t,
        &ws,
        "PA",
        &format!("look at this {m}"),
        &[png("img1", "shot.png")],
        "",
    )
    .await;
    assert_eq!(reply, "");
    let items = content_items(&t.engine.workspace_chat_items(&ws, "PA").await);
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].role, "user");
    assert!(items[0].text.contains(&m), "{}", items[0].text);
    assert_eq!(items[0].attachments.len(), 1);
    assert_eq!(items[0].attachments[0].id, "img1");
    assert!(!items[0].attachments[0].url.is_empty());
    let named = t.store.workspace(&ws).unwrap();
    assert!(!named.goal.contains("⟦attachment:"), "{}", named.goal);
    assert_eq!(t.engine.chat_tab_title(&ws, "PA"), "look at this");
    release_slots(&t, slots);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_engage_does_not_mark_tab_started() {
    let runner = CaptureScript::new(|_| Vec::new());
    let t = TestEngine::new(Some(runner.provider()));
    t.install_fixture_pack();
    let (ws, _repo) = scratch(&t, "scratch").await;
    t.engine
        .remember_chat_loop_policy(&ws, "PA", "no.such.loop", "")
        .await;
    let reply = send(&t, &ws, "PA", "review and fix", "Review & fix").await;
    assert!(reply.contains("I couldn't engage"), "{reply}");
    assert!(!t.engine.chat_loop_started(&ws, "PA"));
    assert_eq!(t.engine.chat_loop_goal(&ws, "PA").0, "review and fix");
    assert!(!t.store.workspace(&ws).unwrap().has_run());

    let slots = take_slots(&t);
    t.engine
        .remember_chat_loop_policy(&ws, "PA", "test.loops.converge", "test.policies.fail")
        .await;
    let reply = send(&t, &ws, "PA", "review and fix", "Review & fix").await;
    assert!(!reply.contains("I couldn't engage"), "{reply}");
    assert!(t.engine.chat_loop_started(&ws, "PA"));
    assert!(t.store.workspace(&ws).unwrap().has_run());
    release_slots(&t, slots);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn first_engage_stamps_owner_before_dispatch() {
    let runner = ConvergeRunner::new(&[0]);
    let t = TestEngine::new(Some(runner.provider()));
    t.install_fixture_pack();
    let (ws, _repo) = scratch(&t, "scratch").await;
    t.engine.ensure_pair_agent(&ws, "PA2", "");
    t.engine
        .remember_chat_loop_policy(&ws, "PA2", "test.loops.converge", "test.policies.fail")
        .await;
    let reply = send(&t, &ws, "PA2", "fix the open findings", "Review & fix").await;
    assert!(!reply.contains("I couldn't engage"), "{reply}");
    wait_for(Duration::from_secs(15), "reviewer ran", || {
        runner.counts().1 > 0
    })
    .await;
    for spec in runner.state.lock().specs.iter() {
        assert_eq!(spec.chat_code, "PA2", "role {:?}", spec.role);
    }
    t.close().await;
}

#[tokio::test]
async fn specialist_sessions_keyed_by_owner() {
    let t = TestEngine::new(None);
    let (ws, _repo) = scratch(&t, "scratch").await;
    let a = t
        .engine
        .ensure_owned_agent_session(&ws, "coder", "CO", "Coder", "", "PA")
        .expect("PA CO");
    let b = t
        .engine
        .ensure_owned_agent_session(&ws, "coder", "CO", "Coder", "", "PA2")
        .expect("PA2 CO");
    assert_ne!(a.id, b.id, "two conductor tabs must not share a CO session");
    assert_eq!(
        t.store.session_for_chat_agent(&ws, "PA", "CO").unwrap().id,
        a.id
    );
    assert_eq!(
        t.store.session_for_chat_agent(&ws, "PA2", "CO").unwrap().id,
        b.id
    );
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn start_chat_loop_walk_does_not_restore_closed_tab() {
    let t = TestEngine::new(Some(CaptureScript::new(|_| Vec::new()).provider()));
    let (ws, _repo) = settled_owner(&t).await;
    t.engine.ensure_pair_agent(&ws, "PA2", "");
    close_tab(&t.store, &ws, "PA2");
    assert!(t.store.agent(&ws, "PA2").is_none());
    t.engine
        .start_chat_loop_walk(&ws, "PA2", "", "", "fix it")
        .await;
    assert_ne!(t.store.workspace(&ws).unwrap().loop_chat_code, "PA2");
    assert!(t.engine.running_loop_by_chat(&ws, "PA2").is_none());
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn start_chat_loop_walk_close_after_unlock_immediate_dispatch() {
    let t = TestEngine::new(Some(CaptureScript::new(|_| Vec::new()).provider()));
    let (ws, _repo) = settled_owner(&t).await;
    let (store, id) = (Arc::clone(&t.store), ws.clone());
    t.engine
        .set_start_walk_hook(move || close_tab(&store, &id, PAIR_AGENT_CODE));
    t.engine
        .start_chat_loop_walk(&ws, PAIR_AGENT_CODE, "", "", "fix it")
        .await;
    assert_closed_tab_did_not_start_walk(&t, &ws);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn start_chat_loop_walk_close_after_unlock_queued_then_drain() {
    let t = TestEngine::new(Some(CaptureScript::new(|_| Vec::new()).provider()));
    let (ws, _repo) = settled_owner(&t).await;
    let mut slots = take_slots(&t);
    let (store, id) = (Arc::clone(&t.store), ws.clone());
    t.engine
        .set_start_walk_hook(move || close_tab(&store, &id, PAIR_AGENT_CODE));
    t.engine
        .start_chat_loop_walk(&ws, PAIR_AGENT_CODE, "", "", "fix it")
        .await;
    assert_ne!(
        t.store.workspace(&ws).unwrap().state,
        WorkspaceState::Queued,
        "closed tab must not be requeued"
    );
    if slots > 0 {
        t.engine.governor().release_loop();
        slots -= 1;
    }
    t.engine.drain_queued().await;
    assert_closed_tab_did_not_start_walk(&t, &ws);
    release_slots(&t, slots);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn start_chat_loop_walk_clears_stale_goal_display() {
    let t = TestEngine::new(Some(CaptureScript::new(|_| Vec::new()).provider()));
    let (ws, _repo) = settled_owner(&t).await;
    t.store.update_workspace(&ws, |w| {
        w.goal = "Linear ENG-123\n\nFull issue body".into();
        w.goal_display = "Linear ENG-123 · title".into();
    });
    let slots = take_slots(&t);
    t.engine
        .start_chat_loop_walk(&ws, PAIR_AGENT_CODE, "", "", "fix it")
        .await;
    let got = t.store.workspace(&ws).unwrap();
    assert_eq!(got.goal, "fix it");
    assert_eq!(got.goal_display, "", "a different goal drops the display");
    release_slots(&t, slots);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loop_goal_images_use_engage_turn_not_first_chat() {
    let runner = CaptureScript::new(|_| maestro_fixture("engage-img", "OK."));
    let t = TestEngine::new(Some(runner.provider()));
    let (ws, _repo) = scratch(&t, "scratch").await;
    // E4's Agent-mode turn records imgA on PA's first user message.
    let (text, imgs) = crate::attachments::prepare_chat_attachments(
        &t.data_dir,
        &ws,
        &format!("see {}", marker("imgA")),
        &[png("imgA", "a.png")],
    )
    .unwrap();
    t.engine.ensure_pair_agent(&ws, "PA", "");
    let sess = t
        .engine
        .ensure_agent_session(&ws, "coder", "PA", "Agent", "")
        .unwrap();
    SessionMirror::new(Arc::clone(&t.store), &sess.id, "", &sess.provider)
        .record_user("", &text, "", &imgs);

    let slots = take_slots(&t);
    let reply = send_with(
        &t,
        &ws,
        "PA",
        &format!("now {}", marker("imgB")),
        &[png("imgB", "b.png")],
        "",
    )
    .await;
    assert!(!reply.contains("I couldn't engage"), "{reply}");
    let imgs = t.engine.loop_goal_images(&ws, "PA");
    assert_eq!(imgs.len(), 1, "{imgs:?}");
    assert_eq!(imgs[0].id, "imgB", "the engage turn's set");
    release_slots(&t, slots);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reengage_keeps_prompt_action_goal_display() {
    let t = TestEngine::new(Some(CaptureScript::new(|_| Vec::new()).provider()));
    t.install_fixture_pack();
    let (ws, _repo) = settled_owner(&t).await;
    let slots = take_slots(&t);
    t.engine.ensure_pair_agent(&ws, "PA2", "");
    let wire =
        "The human already wrote this change. Review the current worktree, then fix and re-review.";
    send(&t, &ws, "PA2", wire, "Review & fix").await;
    let rec = t.store.workspace(&ws).unwrap();
    assert_eq!(rec.goal, wire);
    assert_eq!(rec.goal_display, "Review & fix");
    let scoped = t.engine.chat_scoped_workspace(rec, "PA2");
    assert_eq!(Walk::new(&t.engine, &scoped).goal_label(), "Review & fix");
    release_slots(&t, slots);
    t.close().await;
}
