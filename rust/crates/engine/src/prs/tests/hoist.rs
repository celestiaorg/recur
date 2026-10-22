//! PR chat hosts and hoisting (Go `github_prs_test.go` EnsurePRChatHost,
//! hoist, unregister and reservation cases; `artifacts_test.go`'s
//! `hoistPRChatHost` cases).
//!
//! Go hoisted on the unregistering goroutine; here the deferred hoist runs
//! as a task (`maybe_archive_linked_pr_chat_host`), so those cases wait for
//! it.

use std::time::Duration;

use recur_domain::{
    Agent, CHAT_MODE_REVIEW, ContentBlock, Finding, FindingStatus, Message, Plan, PlanStep, Pr,
    PROVIDER_CLAUDE_CODE, Session, UserQuestion, UserQuestionAnswer, WORKSPACE_SOLO, Workspace,
};
use tokio_util::sync::CancellationToken;

use super::support::*;
use crate::bus::EngineEvent;
use crate::codes::chat_turn_key;
use crate::testing::{TempRepo, init_repo, wait_for};
use crate::workspace::create::{CreatingAlloc, PHASE_CREATING};

/// A project on a fresh repo plus a real linked worktree.
async fn linked(px: &Px) -> (TempRepo, Workspace) {
    let repo = init_repo();
    px.add_project("acme", &repo.path);
    let w = px.eng().create_workspace("acme", "linked", "main").await.unwrap();
    (repo, w)
}

fn linked_pr(id: &str, ws_id: &str) -> Pr {
    Pr {
        id: id.into(),
        workspace_id: ws_id.into(),
        title: "Already linked".into(),
        group: "you".into(),
        state: String::new(),
        url: String::new(),
        branch: String::new(),
        base: String::new(),
        ..gh_pr(8, "")
    }
}

fn host_ws(id: &str, pr_id: &str, repo: &str) -> Workspace {
    Workspace {
        id: id.into(),
        project_id: "acme".into(),
        goal: "Already linked".into(),
        host_pr_id: pr_id.into(),
        worktree_path: repo.into(),
        r#type: WORKSPACE_SOLO.into(),
        ..Workspace::default()
    }
}

fn pair_session(px: &Px, ws_id: &str, code: &str) -> Session {
    px.te
        .store
        .create_session(Session {
            workspace_id: ws_id.into(),
            kind: "pair".into(),
            provider: PROVIDER_CLAUDE_CODE.into(),
            config_json: format!(r#"{{"code":"{code}"}}"#),
            ..Session::default()
        })
        .unwrap()
}

fn say(px: &Px, session: &Session, text: &str) {
    px.te
        .store
        .append_message(
            &session.id,
            Message {
                role: "user".into(),
                content: Some(vec![ContentBlock::text(text)]),
                ..Message::default()
            },
        )
        .unwrap();
}

fn agent(px: &Px, ws_id: &str, a: Agent) {
    px.te.store.upsert_agent(ws_id, a);
}

fn pa(id: &str) -> Agent {
    Agent {
        id: id.into(),
        code: "PA".into(),
        name: "Agent".into(),
        ..Agent::default()
    }
}

/// A host with one PA session and agent; returns the session.
fn host_with_pa(px: &Px, host: Workspace) -> Session {
    let host = px.add_ws(host);
    let sess = pair_session(px, &host.id, "PA");
    agent(px, &host.id, pa("a-pa"));
    sess
}

fn session_on(px: &Px, ws_id: &str, code: &str) -> Option<String> {
    px.te
        .store
        .session_for_loop_agent(ws_id, code)
        .map(|s| s.id)
}

async fn wait_archived(px: &Px, id: &str) {
    wait_for(Duration::from_secs(2), "host archived", || {
        px.te.store.workspace(id).is_some_and(|w| w.archived)
    })
    .await;
}

fn live_turn(px: &Px, ws_id: &str, code: &str) -> std::sync::Arc<crate::chatlife::ChatTurn> {
    px.eng()
        .register_chat_turn(ws_id, code, CancellationToken::new())
        .expect("turn registered")
}

#[tokio::test]
async fn ensure_host_creates_hidden_workspace() {
    let px = Px::new();
    let repo = init_repo();
    px.add_project("acme", &repo.path);
    px.add_pr(Pr {
        title: "Imported PR".into(),
        ..gh_pr(9, "feat/import")
    });
    let host = px.eng().ensure_pr_chat_host("gh-acme-9").await.unwrap();
    assert!(host.is_pr_chat_host());
    assert_eq!(host.host_pr_id, "gh-acme-9");
    assert_eq!(host.worktree_path, repo.path);
    assert_eq!(px.pr("gh-acme-9").workspace_id, "", "linked only by Create Workspace");
    let again = px.eng().ensure_pr_chat_host("gh-acme-9").await.unwrap();
    assert_eq!(again.id, host.id);
    assert_eq!(px.te.store.workspaces().len(), 1);
    let tab = px.eng().create_workspace_chat(&host.id).await;
    assert_eq!(tab.mode, CHAT_MODE_REVIEW);
    assert_eq!(px.eng().workspace_chat_selection(&host.id, &tab.code).2, CHAT_MODE_REVIEW);
    let mut remaining = Vec::new();
    for c in px.eng().list_workspace_chats(&host.id) {
        remaining = px.eng().close_workspace_chat(&host.id, &c.code).await;
    }
    assert_eq!(remaining.len(), 1, "one replacement tab");
    assert_eq!(
        px.eng().workspace_chat_selection(&host.id, &remaining[0].code).2,
        CHAT_MODE_REVIEW
    );
}

#[tokio::test]
async fn ensure_host_returns_linked_workspace() {
    let px = Px::new();
    let (_repo, w) = linked(&px).await;
    px.add_pr(linked_pr("gh-acme-8", &w.id));
    let got = px.eng().ensure_pr_chat_host("gh-acme-8").await.unwrap();
    assert_eq!(got.id, w.id);
    assert!(!got.is_pr_chat_host());
}

#[tokio::test]
async fn ensure_host_hoists_leftover_host_when_linked() {
    let px = Px::new();
    let (repo, w) = linked(&px).await;
    px.add_pr(linked_pr("gh-acme-8", &w.id));
    let sess = host_with_pa(&px, host_ws("W-host", "gh-acme-8", &repo.path));
    let got = px.eng().ensure_pr_chat_host("gh-acme-8").await.unwrap();
    assert_eq!(got.id, w.id);
    assert_eq!(session_on(&px, &w.id, "PA"), Some(sess.id));
    assert!(px.ws("W-host").archived);
}

#[tokio::test]
async fn hoist_merges_placeholder_pa() {
    let px = Px::new();
    let (repo, w) = linked(&px).await;
    agent(
        &px,
        &w.id,
        Agent {
            id: format!("{}-PA", w.id),
            workspace_id: w.id.clone(),
            status: "idle".into(),
            ..pa("")
        },
    );
    px.add_pr(linked_pr("gh-acme-8b", &w.id));
    px.add_ws(host_ws("W-host-merge", "gh-acme-8b", &repo.path));
    pair_session(&px, "W-host-merge", "PA");
    agent(
        &px,
        "W-host-merge",
        Agent {
            workspace_id: "W-host-merge".into(),
            session_id: "prov-host".into(),
            unseen: true,
            model: "sonnet".into(),
            status: "idle".into(),
            ..pa("a-pa")
        },
    );
    px.eng().ensure_pr_chat_host("gh-acme-8b").await.unwrap();
    let got = px.te.store.agent(&w.id, "PA").expect("dest PA");
    assert_eq!(got.session_id, "prov-host");
    assert_eq!(got.id, format!("{}-PA", w.id));
    assert!(got.unseen);
}

#[tokio::test]
async fn hoist_defers_while_host_chat_live() {
    let px = Px::new();
    let (repo, w) = linked(&px).await;
    px.add_pr(linked_pr("gh-acme-8c", &w.id));
    let sess = host_with_pa(&px, host_ws("W-host-live", "gh-acme-8c", &repo.path));
    live_turn(&px, "W-host-live", "PA");
    let got = px.eng().ensure_pr_chat_host("gh-acme-8c").await.unwrap();
    assert_eq!(got.id, "W-host-live");
    assert!(!px.ws("W-host-live").archived);
    assert!(session_on(&px, "W-host-live", "PA").is_some());
    px.eng()
        .chats
        .lock()
        .remove(&chat_turn_key("W-host-live", "PA"));
    let got = px.eng().ensure_pr_chat_host("gh-acme-8c").await.unwrap();
    assert_eq!(got.id, w.id);
    assert_eq!(session_on(&px, &w.id, "PA"), Some(sess.id));
    assert!(px.ws("W-host-live").archived);
}

#[tokio::test]
async fn hoist_defers_while_dest_chat_live() {
    let px = Px::new();
    let (repo, w) = linked(&px).await;
    let _ = px.eng().list_workspace_chats(&w.id);
    px.add_pr(linked_pr("gh-acme-8d", &w.id));
    let sess = host_with_pa(&px, host_ws("W-host-dest-live", "gh-acme-8d", &repo.path));
    live_turn(&px, &w.id, "PA");
    let got = px.eng().ensure_pr_chat_host("gh-acme-8d").await.unwrap();
    assert_eq!(got.id, "W-host-dest-live");
    assert!(!px.ws("W-host-dest-live").archived);
    px.eng().chats.lock().remove(&chat_turn_key(&w.id, "PA"));
    let got = px.eng().ensure_pr_chat_host("gh-acme-8d").await.unwrap();
    assert_eq!(got.id, w.id);
    assert_eq!(session_on(&px, &w.id, "PA"), Some(sess.id));
    assert!(px.ws("W-host-dest-live").archived);
}

#[tokio::test]
async fn hoist_remaps_colliding_transcripts() {
    let px = Px::new();
    let (repo, w) = linked(&px).await;
    let live = pair_session(&px, &w.id, "PA");
    say(&px, &live, "live chat");
    agent(
        &px,
        &w.id,
        Agent {
            id: format!("{}-PA", w.id),
            workspace_id: w.id.clone(),
            session_id: "prov-live".into(),
            ..pa("")
        },
    );
    px.add_pr(linked_pr("gh-acme-8d", &w.id));
    px.add_ws(host_ws("W-host-collide", "gh-acme-8d", &repo.path));
    let host_sess = pair_session(&px, "W-host-collide", "PA");
    say(&px, &host_sess, "host chat");
    agent(
        &px,
        "W-host-collide",
        Agent {
            workspace_id: "W-host-collide".into(),
            session_id: "prov-host".into(),
            ..pa("a-pa")
        },
    );
    px.eng().ensure_pr_chat_host("gh-acme-8d").await.unwrap();
    assert_eq!(session_on(&px, &w.id, "PA"), Some(live.id));
    assert_eq!(session_on(&px, &w.id, "PA2"), Some(host_sess.id));
    assert_eq!(px.te.store.agent(&w.id, "PA").unwrap().session_id, "prov-live");
    let moved = px.te.store.agent(&w.id, "PA2").unwrap();
    assert_eq!(moved.session_id, "prov-host");
    assert_eq!(moved.id, format!("{}-PA2", w.id));
    assert_eq!(moved.workspace_id, w.id);
    assert_eq!(
        px.eng().resolve_hoisted_chat("W-host-collide", "PA"),
        (w.id.clone(), "PA2".to_owned())
    );
}

#[tokio::test]
async fn drop_continue_gen_retries_host_hoist() {
    let px = Px::new();
    let (repo, w) = linked(&px).await;
    let _ = px.eng().list_workspace_chats(&w.id);
    px.add_pr(linked_pr("gh-acme-8m", &w.id));
    let sess = host_with_pa(&px, host_ws("W-host-continue", "gh-acme-8m", &repo.path));
    let generation = px.eng().alloc_continue_gen("W-host-continue", "PA");
    let got = px.eng().ensure_pr_chat_host("gh-acme-8m").await.unwrap();
    assert_eq!(got.id, "W-host-continue", "a live remount gen defers the hoist");
    px.eng().drop_continue_gen(generation);
    wait_archived(&px, "W-host-continue").await;
    assert_eq!(session_on(&px, &w.id, "PA"), Some(sess.id));
}

#[tokio::test]
async fn drop_continue_gen_defers_while_chat_life_held() {
    let px = Px::new();
    let (repo, w) = linked(&px).await;
    let _ = px.eng().list_workspace_chats(&w.id);
    px.add_pr(linked_pr("gh-acme-8m2", &w.id));
    let sess = host_with_pa(&px, host_ws("W-host-continue-life", "gh-acme-8m2", &repo.path));
    let (life, held) = px.eng().lock_chat_life("W-host-continue-life", "PA").await;
    assert!(held);
    let generation = px.eng().alloc_continue_gen("W-host-continue-life", "PA");
    px.eng().drop_continue_gen(generation);
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(
        !px.ws("W-host-continue-life").archived,
        "archived while the life lock is held"
    );
    drop(life);
    wait_archived(&px, "W-host-continue-life").await;
    assert_eq!(session_on(&px, &w.id, "PA"), Some(sess.id));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mode_switch_setup_failure_retries_pending_hoist() {
    let forge = std::sync::Arc::new(recur_forge::testing::FakeForge::new());
    let te = crate::testing::TestEngine::with(crate::testing::TestOptions {
        forges: vec![forge.clone()],
        mcp_addr: Some("127.0.0.1:-1".into()),
        ..crate::testing::TestOptions::default()
    });
    let px = Px { te, forge };
    let (repo, w) = linked(&px).await;
    let _ = px.eng().list_workspace_chats(&w.id);
    px.add_pr(linked_pr("gh-acme-8m3", &w.id));
    let sess = host_with_pa(&px, host_ws("W-host-continue-setup", "gh-acme-8m3", &repo.path));
    let generation = px.eng().alloc_continue_gen("W-host-continue-setup", "PA");
    tokio::time::timeout(
        Duration::from_secs(2),
        px.eng()
            .resume_after_mode_switch("W-host-continue-setup", "PA", "agent", "ask", generation),
    )
    .await
    .expect("the continuation deadlocked on the hoist");
    wait_archived(&px, "W-host-continue-setup").await;
    assert_eq!(session_on(&px, &w.id, "PA"), Some(sess.id));
}

#[tokio::test]
async fn hoist_moves_plan_and_findings() {
    let px = Px::new();
    let (repo, w) = linked(&px).await;
    px.add_pr(linked_pr("gh-acme-8n", &w.id));
    px.add_ws(Workspace {
        definition_of_done: "Ship the toggle".into(),
        plan: Some(Plan {
            summary: "Dark mode".into(),
            body: "Touch Settings.svelte".into(),
            chat_code: "PA".into(),
            steps: vec![PlanStep {
                id: "s1".into(),
                title: "Add the toggle".into(),
                status: "pending".into(),
                ..PlanStep::default()
            }],
            ..Plan::default()
        }),
        ..host_ws("W-host-plan", "gh-acme-8n", &repo.path)
    });
    px.te.store.add_finding(Finding {
        id: "F-host-1".into(),
        workspace_id: "W-host-plan".into(),
        file: "Settings.svelte".into(),
        line: 12,
        severity: "medium".into(),
        message: "contrast".into(),
        status: FindingStatus::Open.as_str().into(),
        ..Finding::default()
    });
    agent(&px, "W-host-plan", pa("a-pa"));
    pair_session(&px, "W-host-plan", "PA");
    px.eng().ensure_pr_chat_host("gh-acme-8n").await.unwrap();
    let got = px.ws(&w.id);
    let plan = got.plan.expect("dest plan");
    assert_eq!((plan.summary.as_str(), plan.chat_code.as_str()), ("Dark mode", "PA"));
    assert_eq!(got.definition_of_done, "Ship the toggle");
    let findings = px.te.store.findings(&w.id);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].id, "F-host-1");
    assert_eq!(findings[0].workspace_id, w.id);
    assert_eq!(px.te.store.open_findings_count("W-host-plan"), 0);
    assert!(px.te.events.all().iter().any(|e| matches!(
        e,
        EngineEvent::FindingsUpdated(r) if r.workspace_id == w.id
    )));
    assert!(px.te.events.all().iter().any(|e| matches!(
        e,
        EngineEvent::WorkspaceHoisted(h)
            if h.workspace_id == w.id && h.chats_changed && h.from_workspace_id == "W-host-plan"
    )));
}

/// Two PA transcripts: the host's moves to PA2 on dest.
async fn colliding(px: &Px, pr_id: &str, host_id: &str) -> (TempRepo, Workspace, Session, Session) {
    let (repo, w) = linked(px).await;
    let live = pair_session(px, &w.id, "PA");
    say(px, &live, "live chat");
    agent(
        px,
        &w.id,
        Agent {
            id: format!("{}-PA", w.id),
            workspace_id: w.id.clone(),
            session_id: "prov-live".into(),
            ..pa("")
        },
    );
    px.add_pr(linked_pr(pr_id, &w.id));
    px.add_ws(host_ws(host_id, pr_id, &repo.path));
    let host = pair_session(px, host_id, "PA");
    say(px, &host, "host chat");
    agent(
        px,
        host_id,
        Agent {
            workspace_id: host_id.into(),
            session_id: "prov-host".into(),
            ..pa("a-pa")
        },
    );
    (repo, w, live, host)
}

#[tokio::test]
async fn hoist_transfers_host_focus() {
    let px = Px::new();
    let (_repo, w, _, _) = colliding(&px, "gh-acme-8o", "W-host-focus").await;
    px.eng().set_workspace_chat_focus("W-host-focus", "PA");
    px.eng().ensure_pr_chat_host("gh-acme-8o").await.unwrap();
    assert!(px.eng().is_chat_focused(&w.id, "PA2"));
    assert!(px.te.events.all().iter().any(|e| matches!(
        e,
        EngineEvent::WorkspaceHoisted(h) if h.workspace_id == w.id && h.focus_chat_code == "PA2"
    )));
}

#[tokio::test]
async fn hoist_reserves_source_codes() {
    let px = Px::new();
    let (repo, w) = linked(&px).await;
    let live = pair_session(&px, &w.id, "PA");
    say(&px, &live, "live chat");
    agent(
        &px,
        &w.id,
        Agent {
            id: format!("{}-PA", w.id),
            workspace_id: w.id.clone(),
            session_id: "prov-live".into(),
            ..pa("")
        },
    );
    px.add_pr(linked_pr("gh-acme-8e", &w.id));
    let host = "W-host-src-codes";
    px.add_ws(host_ws(host, "gh-acme-8e", &repo.path));
    let host_pa = pair_session(&px, host, "PA");
    say(&px, &host_pa, "host PA");
    let host_pa2 = pair_session(&px, host, "PA2");
    say(&px, &host_pa2, "host PA2");
    agent(
        &px,
        host,
        Agent {
            workspace_id: host.into(),
            session_id: "prov-host-pa".into(),
            ..pa("a-pa")
        },
    );
    agent(
        &px,
        host,
        Agent {
            id: "a-pa2".into(),
            code: "PA2".into(),
            workspace_id: host.into(),
            session_id: "prov-host-pa2".into(),
            ..Agent::default()
        },
    );
    px.eng().ensure_pr_chat_host("gh-acme-8e").await.unwrap();
    assert_eq!(session_on(&px, &w.id, "PA"), Some(live.id));
    assert_eq!(session_on(&px, &w.id, "PA2"), Some(host_pa2.id));
    assert_eq!(session_on(&px, &w.id, "PA3"), Some(host_pa.id));
    assert_eq!(px.te.store.agent(&w.id, "PA2").unwrap().session_id, "prov-host-pa2");
    assert_eq!(px.te.store.agent(&w.id, "PA3").unwrap().session_id, "prov-host-pa");
    let hoisted = px
        .te
        .events
        .all()
        .into_iter()
        .find_map(|e| match e {
            EngineEvent::WorkspaceHoisted(h)
                if h.workspace_id == w.id && h.chats_changed && h.from_workspace_id == host =>
            {
                Some(h)
            }
            _ => None,
        })
        .expect("hoist event");
    assert_eq!(hoisted.code_remap.get("PA").map(String::as_str), Some("PA3"));
    let moved = hoisted.moved_codes.unwrap_or_default();
    assert!(moved.contains(&"PA2".to_owned()) && moved.contains(&"PA3".to_owned()), "{moved:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hoist_waits_for_chat_life() {
    let px = Px::new();
    let (repo, w) = linked(&px).await;
    px.add_pr(linked_pr("gh-acme-8f", &w.id));
    let sess = host_with_pa(&px, host_ws("W-host-wait", "gh-acme-8f", &repo.path));
    let _ = px.eng().list_workspace_chats(&w.id);
    let (life, held) = px.eng().lock_chat_life("W-host-wait", "PA").await;
    assert!(held);
    let e = std::sync::Arc::clone(px.eng());
    let ensure = tokio::spawn(async move { e.ensure_pr_chat_host("gh-acme-8f").await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!ensure.is_finished(), "returned while the life lock was held");
    assert!(!px.ws("W-host-wait").archived);
    let eng = px.eng();
    assert_eq!(eng.create_workspace_chat(&w.id).await.code, "", "no new tab during a hoist");
    assert_eq!(
        eng.fork_workspace_chat(&w.id, "PA", "fork-during-hoist").await.code,
        "",
        "no fork during a hoist"
    );
    let tabs = eng.close_workspace_chat(&w.id, "PA").await;
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[0].code, "PA");
    eng.remember_workspace_chat_selection(&w.id, "PA", "sonnet", None, "agent", None)
        .await;
    assert!(session_on(&px, &w.id, "PA").is_none(), "selection created a dest session");
    eng.remember_workspace_chat_prompt_group(&w.id, "PA", "ops").await;
    assert!(session_on(&px, &w.id, "PA").is_none(), "prompt group created a dest session");
    eng.remember_chat_loop_policy(&w.id, "PA", "test.loops.converge", "")
        .await;
    assert!(session_on(&px, &w.id, "PA").is_none(), "loop policy created a dest session");
    drop(life);
    tokio::time::timeout(Duration::from_secs(2), ensure)
        .await
        .expect("still blocked after unlock")
        .unwrap()
        .unwrap();
    assert_eq!(session_on(&px, &w.id, "PA"), Some(sess.id));
    assert!(px.ws("W-host-wait").archived);
}

#[tokio::test]
async fn unregister_archives_linked_host() {
    let px = Px::new();
    let (repo, w) = linked(&px).await;
    px.add_pr(linked_pr("gh-acme-8g", &w.id));
    let sess = host_with_pa(&px, host_ws("W-host-unreg", "gh-acme-8g", &repo.path));
    let turn = live_turn(&px, "W-host-unreg", "PA");
    let got = px.eng().ensure_pr_chat_host("gh-acme-8g").await.unwrap();
    assert_eq!(got.id, "W-host-unreg");
    px.eng().unregister_chat_turn("W-host-unreg", "PA", &turn);
    wait_archived(&px, "W-host-unreg").await;
    assert_eq!(session_on(&px, &w.id, "PA"), Some(sess.id));
}

#[tokio::test]
async fn unregister_archives_linked_host_from_dest() {
    let px = Px::new();
    let (repo, w) = linked(&px).await;
    let _ = px.eng().list_workspace_chats(&w.id);
    px.add_pr(linked_pr("gh-acme-8j", &w.id));
    let sess = host_with_pa(&px, host_ws("W-host-dest-unreg", "gh-acme-8j", &repo.path));
    let turn = live_turn(&px, &w.id, "PA");
    let got = px.eng().ensure_pr_chat_host("gh-acme-8j").await.unwrap();
    assert_eq!(got.id, "W-host-dest-unreg");
    px.eng().unregister_chat_turn(&w.id, "PA", &turn);
    wait_archived(&px, "W-host-dest-unreg").await;
    assert_eq!(session_on(&px, &w.id, "PA"), Some(sess.id));
}

#[tokio::test]
async fn close_chat_during_hoist_does_not_reseed_source() {
    let px = Px::new();
    let (repo, w) = linked(&px).await;
    px.add_pr(linked_pr("gh-acme-8k", &w.id));
    px.add_ws(host_ws("W-host-reseed", "gh-acme-8k", &repo.path));
    agent(&px, "W-host-reseed", pa("a-pa"));
    let guard = px
        .eng()
        .hold_pr_chat_hoist(&["W-host-reseed", &w.id])
        .await
        .expect("idle ids");
    px.te
        .store
        .move_workspace_agents("W-host-reseed", &w.id, &Default::default());
    assert!(px.te.store.agent("W-host-reseed", "PA").is_none());
    px.eng().close_workspace_chat("W-host-reseed", "PA").await;
    assert!(
        px.te.store.agent("W-host-reseed", "PA").is_none(),
        "close reseeded PA on a hoisting source"
    );
    drop(guard);
}

#[tokio::test]
async fn unregister_archives_settled_host() {
    let px = Px::new();
    let repo = init_repo();
    px.add_project("acme", &repo.path);
    px.add_pr(Pr {
        id: "gh-acme-8h".into(),
        title: "Done".into(),
        ..gh_pr(8, "feat")
    });
    let sess = host_with_pa(&px, host_ws("W-host-settled", "gh-acme-8h", &repo.path));
    let turn = live_turn(&px, "W-host-settled", "PA");
    px.te.store.update_pr("gh-acme-8h", |p| {
        p.state = "Merged".into();
        p.group = "merged".into();
    });
    px.eng().archive_pr_chat_hosts("gh-acme-8h", "").await;
    assert!(!px.ws("W-host-settled").archived, "archived while chat is live");
    px.eng().unregister_chat_turn("W-host-settled", "PA", &turn);
    wait_archived(&px, "W-host-settled").await;
    assert_eq!(
        session_on(&px, "W-host-settled", "PA"),
        Some(sess.id),
        "a settled archive keeps the session on the host"
    );
}

#[tokio::test]
async fn unregister_keeps_open_unlinked_host() {
    let px = Px::new();
    let repo = init_repo();
    px.add_project("acme", &repo.path);
    px.add_pr(Pr {
        id: "gh-acme-8i".into(),
        title: "Open".into(),
        ..gh_pr(8, "feat")
    });
    let host = px.eng().ensure_pr_chat_host("gh-acme-8i").await.unwrap();
    let turn = live_turn(&px, &host.id, "PA");
    px.eng().unregister_chat_turn(&host.id, "PA", &turn);
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!px.ws(&host.id).archived);
    assert_eq!(
        px.eng().ensure_pr_chat_host("gh-acme-8i").await.unwrap().id,
        host.id
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hold_is_exclusive() {
    let px = Px::new();
    let first = px
        .eng()
        .hold_pr_chat_hoist(&["host-a", "keep"])
        .await
        .expect("idle ids");
    let e = std::sync::Arc::clone(px.eng());
    let second = tokio::spawn(async move {
        let g = e.hold_pr_chat_hoist(&["host-b", "keep"]).await;
        g.is_some()
    });
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(!second.is_finished(), "second hold acquired keep during the first");
    assert!(px.eng().hoisting.lock().contains("keep"));
    assert!(px.eng().hoisting.lock().contains("host-a"));
    drop(first);
    let held = tokio::time::timeout(Duration::from_secs(2), second)
        .await
        .expect("second hold waits for release")
        .unwrap();
    assert!(held);
    assert!(!px.eng().hoisting.lock().contains("keep"), "tombstone leaked");
}

#[tokio::test]
async fn hold_refuses_deleting() {
    let px = Px::new();
    px.eng().deleting.lock().insert("keep".into());
    assert!(px.eng().hold_pr_chat_hoist(&["host-a", "keep"]).await.is_none());
    assert!(!px.eng().hoisting.lock().contains("host-a"));
}

#[tokio::test]
async fn reserve_deleting_refuses_hoist() {
    let px = Px::new();
    let guard = px
        .eng()
        .hold_pr_chat_hoist(&["host-a", "keep"])
        .await
        .expect("idle ids");
    assert!(!px.eng().reserve_deleting("keep"));
    assert!(!px.eng().is_deleting("keep"));
    drop(guard);
    assert!(px.eng().reserve_deleting("keep"));
    assert!(px.eng().is_deleting("keep"));
}

#[tokio::test]
async fn reserve_deleting_refuses_existing() {
    let px = Px::new();
    assert!(px.eng().reserve_deleting("keep"));
    assert!(!px.eng().reserve_deleting("keep"));
    assert!(px.eng().is_deleting("keep"));
    px.eng().release_deleting("keep");
    assert!(px.eng().reserve_deleting("keep"));
}

fn solo(px: &Px, id: &str) {
    px.add_ws(Workspace {
        id: id.into(),
        project_id: "acme".into(),
        r#type: WORKSPACE_SOLO.into(),
        ..Workspace::default()
    });
}

#[tokio::test]
async fn delete_refuses_existing_deleting() {
    let px = Px::new();
    solo(&px, "ws-busy");
    px.eng().deleting.lock().insert("ws-busy".into());
    let err = px.eng().delete_workspace("ws-busy").await.unwrap_err();
    assert!(err.0.contains("being removed"), "{err}");
    assert!(px.te.store.workspace("ws-busy").is_some());
}

#[tokio::test]
async fn delete_refuses_hoisting_destination() {
    let px = Px::new();
    solo(&px, "keep");
    let _guard = px.eng().hold_pr_chat_hoist(&["host-a", "keep"]).await.unwrap();
    let err = px.eng().delete_workspace("keep").await.unwrap_err();
    assert!(err.0.contains("being removed"), "{err}");
    assert!(px.te.store.workspace("keep").is_some());
}

#[tokio::test]
async fn remove_refuses_hoisting_destination() {
    let px = Px::new();
    solo(&px, "keep");
    let _guard = px.eng().hold_pr_chat_hoist(&["host-a", "keep"]).await.unwrap();
    let err = px.eng().remove_workspace("keep").await.unwrap_err();
    assert!(err.0.contains("being removed"), "{err}");
    assert!(!px.ws("keep").archived);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remove_reserves_before_waiting_create() {
    let px = Px::new();
    solo(&px, "keep");
    solo(&px, "host");
    let sess = pair_session(&px, "host", "PA");
    let alloc = std::sync::Arc::new(CreatingAlloc {
        cancel: CancellationToken::new(),
        done: CancellationToken::new(),
    });
    px.eng()
        .creating
        .lock()
        .insert("keep".into(), std::sync::Arc::clone(&alloc));
    let e = std::sync::Arc::clone(px.eng());
    let remove = tokio::spawn(async move { e.remove_workspace("keep").await });
    wait_for(Duration::from_secs(2), "deleting reservation", || {
        px.eng().is_deleting("keep")
    })
    .await;
    assert!(!remove.is_finished(), "returned before the create wait");
    px.eng().archive_one_pr_chat_host("host", "keep").await;
    assert!(!px.ws("host").archived, "archived onto a workspace being removed");
    assert_eq!(session_on(&px, "host", "PA"), Some(sess.id));
    alloc.done.cancel();
    tokio::time::timeout(Duration::from_secs(2), remove)
        .await
        .expect("remove finishes after the create wait")
        .unwrap()
        .unwrap();
    assert!(px.ws("keep").archived);
}

#[tokio::test]
async fn create_from_pr_promotes_chat_host() {
    let px = Px::new();
    let (_keep, clone) = clone_with_pull_head(42, "feat/oauth");
    px.add_project("acme", &clone);
    px.add_pr(gh_pr(42, "feat/oauth"));
    let host = px.eng().ensure_pr_chat_host("gh-acme-42").await.unwrap();
    let got = px.eng().create_workspace_from_pr("gh-acme-42").await.unwrap();
    assert_eq!(got.id, host.id, "promoted in place");
    assert_eq!(host.base_branch, "origin/main");
    assert_eq!(px.pr("gh-acme-42").workspace_id, "", "linked only after allocate");
    wait_for(Duration::from_secs(8), "promoted worktree", || {
        let w = px.ws(&host.id);
        !w.is_pr_chat_host()
            && !w.worktree_path.is_empty()
            && w.worktree_path != clone
            && px.pr("gh-acme-42").workspace_id == host.id
    })
    .await;
    let w = px.ws(&host.id);
    assert_eq!(w.base_branch, "origin/main");
    assert_eq!(px.te.store.workspaces().len(), 1);
}

#[tokio::test]
async fn send_rejects_agent_on_pr_chat_host() {
    let px = Px::new();
    let repo = init_repo();
    px.add_project("acme", &repo.path);
    px.add_pr(Pr {
        title: "Docs".into(),
        ..gh_pr(3, "docs")
    });
    let host = px.eng().ensure_pr_chat_host("gh-acme-3").await.unwrap();
    for mode in ["agent", "loop"] {
        let reply = px
            .eng()
            .send_workspace_message(&host.id, "PA", "edit the files", "sonnet", None, &[], mode, "")
            .await;
        assert!(reply.contains("Create a workspace"), "{mode}: {reply}");
    }
}

#[tokio::test]
async fn ensure_host_rejects_settled_pr() {
    let px = Px::new();
    let repo = init_repo();
    px.add_project("acme", &repo.path);
    px.add_pr(Pr {
        title: "Done".into(),
        group: "merged".into(),
        state: "Merged".into(),
        ..gh_pr(9, "feat")
    });
    assert!(px.eng().ensure_pr_chat_host("gh-acme-9").await.is_err());
    assert!(px.te.store.workspaces().is_empty());
}

#[tokio::test]
async fn create_from_pr_promotes_when_workspace_id_points_at_host() {
    let px = Px::new();
    let (_keep, clone) = clone_with_pull_head(42, "feat/oauth");
    px.add_project("acme", &clone);
    px.add_pr(gh_pr(42, "feat/oauth"));
    let host = px.eng().ensure_pr_chat_host("gh-acme-42").await.unwrap();
    px.te
        .store
        .update_pr("gh-acme-42", |p| p.workspace_id = host.id.clone());
    let got = px.eng().create_workspace_from_pr("gh-acme-42").await.unwrap();
    assert_eq!(got.id, host.id);
    assert_eq!(got.phase, PHASE_CREATING);
    wait_for(Duration::from_secs(8), "allocate", || {
        !px.eng().creating_in_flight(&host.id)
    })
    .await;
}

#[tokio::test]
async fn ask_user_resume_holds_host_busy() {
    let px = Px::new();
    let (repo, w) = linked(&px).await;
    px.add_pr(linked_pr("gh-acme-8ask", &w.id));
    let sess = host_with_pa(&px, host_ws("W-host-ask", "gh-acme-8ask", &repo.path));
    let q = px.eng().ask_users.park(
        UserQuestion {
            workspace_id: "W-host-ask".into(),
            agent_code: "PA".into(),
            question: "color?".into(),
            ..UserQuestion::default()
        },
        None,
    );
    assert!(px.eng().pr_chat_host_busy("W-host-ask"));
    let (got, _) = px
        .eng()
        .ask_users
        .answer(
            &q.id,
            &UserQuestionAnswer {
                choice: "Blue".into(),
                ..UserQuestionAnswer::default()
            },
        )
        .expect("answered");
    assert!(px.eng().pending_ask_users().is_empty());
    assert!(px.eng().pr_chat_host_busy("W-host-ask"), "the resume gap keeps it busy");
    px.eng().maybe_archive_linked_pr_chat_host("W-host-ask");
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!px.ws("W-host-ask").archived);
    assert!(session_on(&px, "W-host-ask", "PA").is_some());
    px.eng().ask_users.drop_resume(&got.id);
    assert!(!px.eng().pr_chat_host_busy("W-host-ask"));
    px.eng().maybe_archive_linked_pr_chat_host("W-host-ask");
    wait_archived(&px, "W-host-ask").await;
    assert_eq!(session_on(&px, &w.id, "PA"), Some(sess.id));
}

#[tokio::test]
async fn archive_one_revalidates_link() {
    let px = Px::new();
    let repo = init_repo();
    px.add_project("acme", &repo.path);
    let w1 = px.eng().create_workspace("acme", "keep-stale", "main").await.unwrap();
    let w2 = px.eng().create_workspace("acme", "keep-live", "main").await.unwrap();
    px.add_pr(Pr {
        title: "Relinked".into(),
        ..linked_pr("gh-acme-8relink", &w2.id)
    });
    let sess = host_with_pa(&px, host_ws("W-host-relink", "gh-acme-8relink", &repo.path));
    px.eng().archive_one_pr_chat_host("W-host-relink", &w1.id).await;
    assert!(!px.ws("W-host-relink").archived, "archived onto a stale keep");
    assert!(session_on(&px, &w1.id, "PA").is_none());
    assert_eq!(session_on(&px, "W-host-relink", "PA"), Some(sess.id.clone()));
    px.eng().archive_one_pr_chat_host("W-host-relink", &w2.id).await;
    assert!(px.ws("W-host-relink").archived);
    assert_eq!(session_on(&px, &w2.id, "PA"), Some(sess.id));
}

#[tokio::test]
async fn hoist_rewrites_plan_artifact_paths() {
    let mut dest = Workspace {
        id: "W-live".into(),
        ..Workspace::default()
    };
    let from = Workspace {
        id: "W-host".into(),
        definition_of_done: "done".into(),
        plan: Some(Plan {
            chat_code: "PA".into(),
            attachments: vec![recur_domain::PlanAttachment {
                id: "a1".into(),
                kind: "md".into(),
                path: "workspaces/W-host/artifacts/a1.md".into(),
                ..Default::default()
            }],
            ..Plan::default()
        }),
        ..Workspace::default()
    };
    let remap = std::collections::BTreeMap::from([("PA".to_owned(), "PA2".to_owned())]);
    crate::hoist::hoist_pr_chat_host_plan(&mut dest, &from, &remap);
    let plan = dest.plan.unwrap();
    assert_eq!(plan.attachments[0].path, "workspaces/W-live/artifacts/a1.md");
    assert_eq!(plan.chat_code, "PA2");
    assert_eq!(dest.definition_of_done, "done");
    assert_eq!(
        crate::hoist::hoist_remap_codes(
            &["PA".into(), "PA2".into(), String::new(), "MA".into()],
            &remap
        ),
        ["PA2", "MA"]
    );
}
