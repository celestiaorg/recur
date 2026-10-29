//! Chat recover tests and the resume case of ask_user: zombie Stop, Recover
//! idling live-but-dead chats and closing their streams, AutoResume continuing
//! PA*/MA tabs (resuming the tab's own handle and dock pick), the Maestro
//! rules, and quit leaving an interrupted chat live for the next boot.

use std::sync::Arc;
use std::time::Duration;

use recur_agent::TurnResult;
use recur_domain::{
    Agent, AgentRole, CHAT_MODE_AGENT, CHAT_MODE_PLAN, Checkpoint, Escalation, Flags, Message,
    PROVIDER_CLAUDE_CODE, PROVIDER_CURSOR, STREAM_STREAMING, Session, UserQuestion,
    UserQuestionAnswer, Workspace, WorkspaceState,
};
use tokio_util::sync::CancellationToken;

use super::test_support::*;
use crate::chat::recover::maestro_resume_needs_loop;
use crate::codes::MAESTRO_AGENT_CODE;
use crate::lifecycle::PHASE_INTERRUPTED;
use crate::loops::test_support::{AsyncProvider, ctx_runner};
use crate::testing::wait_for;
use crate::turn::finalize::{CHAT_INTERRUPTED_REPLY, CHAT_RESUME_DISPLAY, CHAT_STOPPED_REPLY};
use crate::turn::transcript::flatten_transcript;

pub(super) fn persist_working_chat(fx: &Fx, ws: &str, code: &str) -> Session {
    let eng = fx.eng();
    eng.ensure_pair_agent(ws, code, "sonnet");
    fx.te
        .store
        .update_agent(ws, code, |a| a.status = "working".into());
    let sess = eng
        .ensure_agent_session(
            ws,
            AgentRole::Coder.as_str(),
            code,
            "Agent",
            PROVIDER_CLAUDE_CODE,
        )
        .unwrap();
    persist_streaming_tool(fx, &sess.id, "tu-crash", "toolu_crash");
    fx.te
        .store
        .set_session_status(&sess.id, "streaming")
        .unwrap();
    sess
}

fn persist_streaming_tool(fx: &Fx, session_id: &str, id: &str, provider_id: &str) {
    let msg = fx
        .te
        .store
        .create_message(
            session_id,
            Message {
                role: "assistant".into(),
                stream_status: STREAM_STREAMING.into(),
                ..Message::default()
            },
        )
        .unwrap();
    fx.te
        .store
        .append_content_block(
            &msg,
            recur_domain::ContentBlock::tool_use(
                id,
                provider_id,
                "Bash",
                r#"{"command":"sleep 999"}"#,
            ),
        )
        .unwrap();
}

fn persist_directing_maestro(fx: &Fx, ws: &str) {
    fx.eng().ensure_maestro_agent(ws, "goal");
    fx.te
        .store
        .update_agent(ws, MAESTRO_AGENT_CODE, |a| a.status = "directing".into());
    fx.eng()
        .ensure_agent_session(
            ws,
            AgentRole::Maestro.as_str(),
            MAESTRO_AGENT_CODE,
            "Maestro",
            PROVIDER_CLAUDE_CODE,
        )
        .unwrap();
}

fn queued(fx: &Fx) -> Vec<String> {
    fx.eng()
        .interrupted_chats
        .lock()
        .iter()
        .map(|c| c.code.clone())
        .collect()
}

#[tokio::test]
async fn stop_heals_a_zombie_working_agent() {
    let fx = Fx::new(Some(capture("z", "OK.").provider()));
    let ws = fx.workspace("zombie-stop").await;
    persist_working_chat(&fx, &ws.id, "PA");
    assert!(fx.eng().cancel_chat(&ws.id, "PA").await, "the zombie heals");
    assert_eq!(fx.te.store.agent(&ws.id, "PA").unwrap().status, "idle");
    let items = fx.eng().workspace_chat_items(&ws.id, "PA").await;
    assert!(
        items
            .iter()
            .any(|it| it.kind == "text" && it.text == CHAT_STOPPED_REPLY)
    );
    assert!(
        !items
            .iter()
            .any(|it| it.kind == "tool" && it.result_pending),
        "{items:?}"
    );
    fx.te.close().await;
}

#[tokio::test]
async fn recover_idles_an_interrupted_chat_and_closes_its_stream() {
    let fx = Fx::new(Some(capture("r", "OK.").provider()));
    let ws = fx.workspace("crash-chat").await;
    let sess = persist_working_chat(&fx, &ws.id, "PA");
    fx.eng().recover();
    assert_eq!(fx.te.store.agent(&ws.id, "PA").unwrap().status, "idle");
    assert_eq!(fx.te.store.get_session(&sess.id).unwrap().status, "idle");
    let items = fx.eng().workspace_chat_items(&ws.id, "PA").await;
    assert!(
        items
            .iter()
            .any(|it| it.kind == "text" && it.text == CHAT_INTERRUPTED_REPLY)
    );
    assert!(
        !items
            .iter()
            .any(|it| it.kind == "tool" && it.result_pending)
    );
    assert_eq!(queued(&fx), ["PA"]);
    fx.te.close().await;
}

const CURSOR_RESUME: &str = "agent-6eebccee-4bef-4e7f-a2f7-27e94087de8b";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auto_resume_continues_an_interrupted_chat() {
    let c = capture_same("sess-resume", "picked up.");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("resume-chat").await;
    persist_working_chat(&fx, &ws.id, "PA");
    fx.te
        .store
        .update_agent(&ws.id, "PA", |a| a.session_id = CURSOR_RESUME.into());
    let f: Flags = [("effort".to_owned(), "high".to_owned())]
        .into_iter()
        .collect();
    fx.eng()
        .remember_workspace_chat_selection(
            &ws.id,
            "PA",
            "cursor/composer-2.5",
            Some(&f),
            "agent",
            None,
        )
        .await;
    fx.eng().recover();
    fx.eng().auto_resume().await;
    let cc = Arc::clone(&c);
    wait_for(Duration::from_secs(5), "auto-resume turn", || {
        cc.with_role(AgentRole::Coder)
            .first()
            .is_some_and(|s| s.prompt.contains("Recur restarted"))
    })
    .await;
    let spec = c.with_role(AgentRole::Coder).remove(0);
    assert_eq!(
        spec.resume_session, CURSOR_RESUME,
        "a restart resumes, never injects"
    );
    assert!(!spec.prompt.contains("<transcript>"));
    assert_eq!(spec.display_text, CHAT_RESUME_DISPLAY);
    assert!(!spec.skip_record_user, "no open turn to reuse");
    assert!(spec.model.contains("composer"), "{}", spec.model);
    let (st, id) = (Arc::clone(&fx.te.store), ws.id.clone());
    wait_for(Duration::from_secs(3), "resumed chat idle", || {
        st.agent(&id, "PA").is_some_and(|a| a.status == "idle")
    })
    .await;
    fx.te.close().await;
}

#[tokio::test]
async fn a_restart_resume_never_displaces_a_live_turn() {
    let fx = Fx::new(Some(capture_same("sess-live", "ok").provider()));
    let ws = fx.workspace("resume-idle").await;
    let cancel = CancellationToken::new();
    let turn = fx
        .eng()
        .register_chat_turn(&ws.id, "PA", cancel.clone())
        .unwrap();
    let reply = fx
        .eng()
        .send_workspace_message(
            &ws.id,
            "PA",
            crate::turn::finalize::CHAT_RESUME_PROMPT,
            "sonnet",
            None,
            &[],
            "",
            CHAT_RESUME_DISPLAY,
        )
        .await;
    assert_eq!(reply, "");
    assert!(
        fx.eng()
            .live_chat_turn(&ws.id, "PA")
            .is_some_and(|t| Arc::ptr_eq(&t, &turn))
    );
    assert!(!cancel.is_cancelled());
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auto_resume_drops_a_foreign_cursor_handle_on_claude() {
    let c = capture_same("sess-resume", "picked up.");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("resume-mismatch").await;
    persist_working_chat(&fx, &ws.id, "PA");
    fx.te
        .store
        .update_agent(&ws.id, "PA", |a| a.session_id = CURSOR_RESUME.into());
    fx.eng()
        .remember_workspace_chat_selection(&ws.id, "PA", "sonnet", None, "agent", None)
        .await;
    fx.eng().recover();
    fx.eng().auto_resume().await;
    let cc = Arc::clone(&c);
    wait_for(Duration::from_secs(5), "auto-resume turn", || {
        cc.with_role(AgentRole::Coder)
            .first()
            .is_some_and(|s| s.prompt.contains("Recur restarted"))
    })
    .await;
    let spec = c.with_role(AgentRole::Coder).remove(0);
    assert!(spec.resume_session.is_empty());
    assert!(
        spec.prompt.contains("<transcript>"),
        "a foreign handle injects history"
    );
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auto_resume_continues_the_maestro_on_a_settled_loop() {
    let c = capture_same("sess-ma-settled", "continuing.");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("settled-ma").await;
    persist_directing_maestro(&fx, &ws.id);
    fx.te.store.update_workspace(&ws.id, |lp| {
        lp.run_started = true;
        lp.state = WorkspaceState::PrOpen;
        lp.phase = "ready".into();
    });
    fx.eng().recover();
    assert_eq!(queued(&fx), [MAESTRO_AGENT_CODE]);
    fx.eng().auto_resume().await;
    let cc = Arc::clone(&c);
    wait_for(Duration::from_secs(5), "maestro auto-resume", || {
        cc.with_role(AgentRole::Maestro)
            .first()
            .is_some_and(|s| s.prompt.contains("Recur restarted"))
    })
    .await;
    assert_eq!(
        c.with_role(AgentRole::Maestro)[0].display_text,
        CHAT_RESUME_DISPLAY
    );
    fx.te.close().await;
}

#[test]
fn the_maestro_resume_needs_a_loop_rule() {
    let w = |state: WorkspaceState, phase: &str, retryable: bool| Workspace {
        state,
        phase: phase.into(),
        retryable,
        ..Workspace::default()
    };
    for (name, ws, want) in [
        ("queued", w(WorkspaceState::Queued, "", false), true),
        ("coding", w(WorkspaceState::Coding, "", false), true),
        (
            "paused interrupted",
            w(WorkspaceState::Paused, PHASE_INTERRUPTED, false),
            true,
        ),
        (
            "paused by you",
            w(WorkspaceState::Paused, "paused by you", false),
            true,
        ),
        (
            "answered waiting",
            w(
                WorkspaceState::Paused,
                "answered — waiting to resume",
                false,
            ),
            true,
        ),
        ("escalated", w(WorkspaceState::Escalated, "", false), true),
        (
            "retryable failed",
            w(WorkspaceState::Failed, "", true),
            true,
        ),
        ("pr_open", w(WorkspaceState::PrOpen, "", false), false),
        ("merged", w(WorkspaceState::Merged, "", false), false),
        ("stopped", w(WorkspaceState::Stopped, "", false), false),
        ("failed", w(WorkspaceState::Failed, "", false), false),
    ] {
        assert_eq!(maestro_resume_needs_loop(&ws), want, "{name}");
    }
}

#[tokio::test]
async fn an_interrupted_maestro_skips_a_parked_loop() {
    for case in [
        "queued",
        "escalated unanswered",
        "answered waiting to resume",
    ] {
        let c = capture_same("sess-ma-parked", "should not run");
        let fx = Fx::new(Some(c.provider()));
        let ws = fx.workspace("parked-ma").await;
        persist_directing_maestro(&fx, &ws.id);
        let st = &fx.te.store;
        match case {
            "queued" => {
                st.update_workspace(&ws.id, |lp| {
                    lp.run_started = true;
                    lp.state = WorkspaceState::Queued;
                    lp.phase = "waiting for a slot".into();
                });
            }
            _ => {
                st.update_workspace(&ws.id, |lp| {
                    lp.run_started = true;
                    lp.state = WorkspaceState::Escalated;
                    lp.phase = "asking a question".into();
                });
                let answered = case.starts_with("answered");
                st.add_escalation(Escalation {
                    workspace_id: ws.id.clone(),
                    question: "A or B?".into(),
                    status: if answered { "answered" } else { "open" }.into(),
                    answer: if answered { "A" } else { "" }.into(),
                    ..Escalation::default()
                });
            }
        }
        fx.eng().recover();
        fx.eng()
            .resume_interrupted_chat(&ws.id, MAESTRO_AGENT_CODE)
            .await;
        assert!(c.with_role(AgentRole::Maestro).is_empty(), "{case}");
        fx.te.close().await;
    }
}

#[tokio::test]
async fn recover_does_not_queue_a_specialist_as_a_chat() {
    let fx = Fx::new(Some(capture("sp", "OK.").provider()));
    let ws = fx.workspace("specialist").await;
    fx.te.store.upsert_agent(
        &ws.id,
        Agent {
            id: format!("{}-CO", ws.id),
            workspace_id: ws.id.clone(),
            code: "CO".into(),
            name: "Coder".into(),
            status: "working".into(),
            ..Agent::default()
        },
    );
    fx.eng().recover();
    assert!(queued(&fx).is_empty());
    assert_eq!(fx.te.store.agent(&ws.id, "CO").unwrap().status, "idle");
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quit_leaves_a_chat_working_for_recover() {
    let fx = Fx::new(Some(ctx_runner()));
    let ws = fx.workspace("quit-chat").await;
    let (eng, id) = (Arc::clone(fx.eng()), ws.id.clone());
    let done = tokio::spawn(async move {
        eng.send_workspace_message(&id, "PA", "long task", "sonnet", None, &[], "", "")
            .await
    });
    let (st, id) = (Arc::clone(&fx.te.store), ws.id.clone());
    wait_for(Duration::from_secs(3), "chat agent working", || {
        st.agent(&id, "PA").is_some_and(|a| a.status == "working")
    })
    .await;
    fx.eng().shutdown().await;
    tokio::time::timeout(Duration::from_secs(5), done)
        .await
        .expect("send returns after shutdown")
        .unwrap();
    assert_eq!(fx.te.store.agent(&ws.id, "PA").unwrap().status, "working");
    let eng2 = fx
        .te
        .reopen(Some(capture_same("sess-after", "ok").provider()));
    eng2.recover();
    assert_eq!(fx.te.store.agent(&ws.id, "PA").unwrap().status, "idle");
    assert_eq!(eng2.interrupted_chats.lock().len(), 1);
    eng2.shutdown().await;
}

#[tokio::test]
async fn recover_skips_a_missing_worktree() {
    let fx = Fx::new(Some(capture("gone", "OK.").provider()));
    let ws = fx.workspace("gone-tree").await;
    persist_working_chat(&fx, &ws.id, "PA");
    std::fs::remove_dir_all(&ws.worktree_path).unwrap();
    fx.eng().recover();
    assert!(queued(&fx).is_empty());
    assert_eq!(fx.te.store.agent(&ws.id, "PA").unwrap().status, "idle");
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_resume_skips_a_live_user_turn() {
    let fx = Fx::new(Some(ctx_runner()));
    let ws = fx.workspace("live-skip").await;
    persist_working_chat(&fx, &ws.id, "PA");
    fx.eng().recover();
    let (eng, id) = (Arc::clone(fx.eng()), ws.id.clone());
    let done = tokio::spawn(async move {
        eng.send_workspace_message(&id, "PA", "user follow-up", "sonnet", None, &[], "", "")
            .await
    });
    let (st, id) = (Arc::clone(&fx.te.store), ws.id.clone());
    wait_for(Duration::from_secs(3), "user turn working", || {
        st.agent(&id, "PA").is_some_and(|a| a.status == "working")
    })
    .await;
    fx.eng().resume_interrupted_chat(&ws.id, "PA").await;
    assert!(
        fx.eng().cancel_chat(&ws.id, "PA").await,
        "the user turn is still registered"
    );
    assert_eq!(join_hang(done, "user turn").await, CHAT_STOPPED_REPLY);
    fx.te.close().await;
}

#[tokio::test]
async fn a_resume_skips_a_closed_tab() {
    let c = capture_same("sess-x", "nope");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("closed-tab").await;
    persist_working_chat(&fx, &ws.id, "PA");
    fx.eng().recover();
    fx.te.store.delete_agent(&ws.id, "PA");
    fx.eng().auto_resume().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(c.with_role(AgentRole::Coder).is_empty());
    fx.te.close().await;
}

#[tokio::test]
async fn recover_closes_a_specialists_pending_tools() {
    let fx = Fx::new(Some(capture("spt", "OK.").provider()));
    let ws = fx.workspace("specialist-tools").await;
    fx.te.store.upsert_agent(
        &ws.id,
        Agent {
            id: format!("{}-CO", ws.id),
            workspace_id: ws.id.clone(),
            code: "CO".into(),
            name: "Coder".into(),
            status: "working".into(),
            ..Agent::default()
        },
    );
    let sess = fx
        .eng()
        .ensure_agent_session(
            &ws.id,
            AgentRole::Coder.as_str(),
            "CO",
            "Coder",
            PROVIDER_CLAUDE_CODE,
        )
        .unwrap();
    persist_streaming_tool(&fx, &sess.id, "tu-co", "toolu_co");
    fx.eng().recover();
    assert!(queued(&fx).is_empty());
    let msgs = fx
        .te
        .store
        .session_transcript(&sess.id)
        .unwrap()
        .unwrap_or_default();
    let items = flatten_transcript(fx.eng().providers(), &msgs, "", &Flags::new());
    assert!(
        !items
            .iter()
            .any(|it| it.kind == "tool" && it.result_pending),
        "{items:?}"
    );
    assert!(
        items
            .iter()
            .any(|it| it.kind == "tool" && !it.result.is_empty())
    );
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_that_completed_during_quit_goes_idle() {
    let (started, release) = (CancellationToken::new(), CancellationToken::new());
    let (s, r) = (started.clone(), release.clone());
    let provider = AsyncProvider::wrap(move |_req, _cx| {
        let (s, r) = (s.clone(), r.clone());
        async move {
            s.cancel();
            r.cancelled().await;
            Ok(TurnResult {
                text: "done".into(),
                ..TurnResult::default()
            })
        }
    });
    let fx = Fx::new(Some(provider));
    let ws = fx.workspace("finished-quit").await;
    let (eng, id) = (Arc::clone(fx.eng()), ws.id.clone());
    let done = tokio::spawn(async move {
        eng.send_workspace_message(&id, "PA", "almost done", "sonnet", None, &[], "", "")
            .await
    });
    within_hang(&started, "turn starts").await;
    let eng = Arc::clone(fx.eng());
    let quit = tokio::spawn(async move { eng.shutdown().await });
    tokio::time::sleep(Duration::from_millis(20)).await;
    release.cancel();
    tokio::time::timeout(Duration::from_secs(5), done)
        .await
        .expect("send returns")
        .unwrap();
    quit.await.unwrap();
    assert_eq!(fx.te.store.agent(&ws.id, "PA").unwrap().status, "idle");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_chat_resumes_alongside_its_loop() {
    let c = capture_same("sess-loop", "ok");
    let fx = Fx::new(Some(c.provider()));
    fx.te.install_shipped_loop_pack();
    let ws = fx.workspace("loop-and-chat").await;
    persist_working_chat(&fx, &ws.id, "PA");
    fx.eng()
        .remember_workspace_chat_selection(&ws.id, "PA", "sonnet", None, CHAT_MODE_AGENT, None)
        .await;
    fx.te.store.update_workspace(&ws.id, |lp| {
        lp.run_started = true;
        lp.state = WorkspaceState::Coding;
        lp.phase = "iteration 1".into();
        lp.iteration = 1;
        lp.checkpoint = Some(Checkpoint {
            state: WorkspaceState::Coding,
            iteration: 1,
            ..Checkpoint::default()
        });
    });
    fx.eng().recover();
    assert_eq!(queued(&fx).len(), 1);
    fx.eng().auto_resume().await;
    let (st, id) = (Arc::clone(&fx.te.store), ws.id.clone());
    wait_for(Duration::from_secs(8), "loop leaves interrupted", || {
        st.workspace(&id)
            .is_some_and(|w| !(w.state == WorkspaceState::Paused && w.phase == PHASE_INTERRUPTED))
    })
    .await;
    let cc = Arc::clone(&c);
    wait_for(Duration::from_secs(8), "the PA tab resumes", || {
        cc.with_role(AgentRole::Coder)
            .iter()
            .any(|s| s.display_text == CHAT_RESUME_DISPLAY)
    })
    .await;
    fx.te.close().await;
}

#[tokio::test]
async fn an_ask_user_resume_keeps_the_dock_provider() {
    const CURSOR_AGENT: &str = "agent-192c0959-3abe-403c-a3e6-aeb25a687b0c";
    let c = capture_same(CURSOR_AGENT, "OK.");
    let fx = Fx::new(Some(c.provider()));
    let mut set = fx.te.store.settings();
    set.model_plan = "claude-code/opus".into();
    set.model_ask = "claude-code/sonnet".into();
    set.model_agent = "claude-code/sonnet".into();
    fx.te.store.set_settings(set);
    let ws = fx.workspace("ask-resume").await;
    let f: Flags = [("fast".to_owned(), "true".to_owned())]
        .into_iter()
        .collect();
    fx.eng()
        .send_workspace_message(
            &ws.id,
            "PA",
            "plan the work",
            "cursor/composer-2.5",
            Some(&f),
            &[],
            CHAT_MODE_PLAN,
            "",
        )
        .await;
    fx.eng().set_agent_session(&ws.id, "PA", CURSOR_AGENT);
    let before = c.with_role(AgentRole::Coder).len();
    fx.eng()
        .resume_after_ask_user(
            UserQuestion {
                workspace_id: ws.id.clone(),
                agent_code: "PA".into(),
                question: "Which depth?".into(),
                ..UserQuestion::default()
            },
            UserQuestionAnswer {
                choice: "Write the full comparison plan".into(),
                ..UserQuestionAnswer::default()
            },
            Vec::new(),
        )
        .await;
    let specs = c.with_role(AgentRole::Coder);
    assert!(specs.len() > before);
    let got = specs.last().unwrap();
    assert_eq!(
        fx.te
            .store
            .session_for_loop_agent(&ws.id, "PA")
            .unwrap()
            .provider,
        PROVIDER_CURSOR,
        "the dock's Cursor, not the Settings Claude"
    );
    assert_eq!(got.model, "composer-2.5");
    assert_eq!(got.resume_session, CURSOR_AGENT);
    assert!(
        got.prompt.contains("Which depth?")
            && got.prompt.contains("Write the full comparison plan")
    );
    assert!(got.append_system_prompt.contains("PLAN mode"));
    fx.te.close().await;
}
