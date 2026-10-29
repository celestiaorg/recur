//! Chat and fork rename tests (tab titles read through `chat_tab_title`; the
//! strip itself is E4's).

use recur_agent_tools::Host;
use recur_domain::WorkspaceState;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::bus::EngineEvent;
use crate::host::EngineHost;
use crate::loop_handle::{LoopHandle, LoopState};
use crate::loop_registry::RunningLoop;
use crate::testing::{TestEngine, add_project, init_repo};

async fn fixture(
    name: &str,
) -> (
    TestEngine,
    recur_domain::Workspace,
    crate::testing::TempRepo,
) {
    let te = TestEngine::new(None);
    let repo = init_repo();
    let proj = add_project(&te.store, &repo.path);
    let ws = te.engine.create_workspace(&proj, name, "").await.unwrap();
    te.engine.ensure_pair_agent(&ws.id, "PA", "");
    (te, ws, repo)
}

fn walk(te: &TestEngine, ws: &str, chat: &str, key: &str) -> std::sync::Arc<LoopHandle> {
    let h = LoopHandle::new(
        ws,
        "proj",
        key,
        LoopState {
            chat_code: chat.into(),
            ..LoopState::default()
        },
    );
    assert!(te.engine.register_loop(RunningLoop {
        handle: h.clone(),
        cancel: CancellationToken::new(),
    }));
    h
}

#[tokio::test]
async fn rename_workspace_branch_and_chat() {
    let (te, ws, _repo) = fixture("").await;
    assert_eq!(
        te.engine.rename_workspace(&ws.id, "Dark mode toggle"),
        "Renamed workspace to Dark mode toggle."
    );
    let named = te.store.workspace(&ws.id).unwrap();
    assert_eq!(
        (named.goal.as_str(), named.short.as_str()),
        ("Dark mode toggle", "Dark mode toggle")
    );
    assert!(te.events.by_name("workspace.updated").iter().any(
        |e| matches!(e, EngineEvent::WorkspaceUpdated(u) if u.workspace_id == ws.id && u.goal == "Dark mode toggle")
    ));
    assert_eq!(
        te.engine
            .rename_workspace_branch(&ws.id, "add-dark-mode")
            .await,
        "Renamed branch to recur/add-dark-mode."
    );
    let renamed = te.store.workspace(&ws.id).unwrap();
    assert_eq!(renamed.branch, "recur/add-dark-mode");
    assert_eq!(
        recur_git::current_branch(&renamed.worktree_path)
            .await
            .unwrap(),
        "recur/add-dark-mode"
    );
    assert_eq!(
        te.engine.rename_chat_session(&ws.id, "PA", "Auth refresh"),
        "Renamed chat to Auth refresh."
    );
    assert_eq!(te.engine.chat_tab_title(&ws.id, "PA"), "Auth refresh");
    let tab = te.events.by_name("workspace.chat_tab");
    assert_eq!(
        tab.last().unwrap().payload_json(),
        format!(
            r#"{{"code":"PA","title":"Auth refresh","workspaceId":"{}"}}"#,
            ws.id
        )
    );
    assert_eq!(
        te.events
            .by_name("chat.renamed")
            .last()
            .unwrap()
            .payload_json(),
        format!(
            r#"{{"agentCode":"PA","title":"Auth refresh","workspaceId":"{}"}}"#,
            ws.id
        )
    );
    te.close().await;
}

#[tokio::test]
async fn rename_workspace_rejects_invalid_titles() {
    let (te, ws, _repo) = fixture("scratch").await;
    for title in ["...", DEFAULT_WORKSPACE_TITLE, &".".repeat(80)] {
        assert_eq!(
            te.engine.rename_workspace(&ws.id, title),
            "rename_workspace: invalid title"
        );
        assert_eq!(te.store.workspace(&ws.id).unwrap().goal, "scratch");
    }
    assert_eq!(
        te.engine.rename_workspace(&ws.id, " "),
        "rename_workspace: title is required"
    );
    assert_eq!(
        te.engine.rename_workspace("nope", "x"),
        "rename_workspace: unknown workspace"
    );
    te.store.update_workspace(&ws.id, |l| {
        l.run_started = true;
        l.goal = "ship the original long task".into();
    });
    assert!(
        te.engine
            .rename_workspace(&ws.id, "Short title")
            .starts_with("Renamed workspace")
    );
    assert_eq!(te.store.workspace(&ws.id).unwrap().goal, "Short title");
    te.close().await;
}

#[tokio::test]
async fn rename_branch_refuses_active_run() {
    let (te, ws, _repo) = fixture("scratch").await;
    let old = ws.branch.clone();
    te.store.update_workspace(&ws.id, |l| {
        l.run_started = true;
        l.state = WorkspaceState::Coding;
    });
    assert_eq!(
        te.engine.rename_workspace_branch(&ws.id, "nope").await,
        "rename_branch: a loop is running — wait until it pauses"
    );
    assert_eq!(te.store.workspace(&ws.id).unwrap().branch, old);
    for (state, name) in [
        (WorkspaceState::Planning, "during-planning"),
        (WorkspaceState::Paused, "after-pause"),
    ] {
        te.store.update_workspace(&ws.id, |l| l.state = state);
        assert_eq!(
            te.engine.rename_workspace_branch(&ws.id, name).await,
            format!("Renamed branch to recur/{name}.")
        );
    }
    te.close().await;
}

#[tokio::test]
async fn planner_rename_retitles_the_loop_tab() {
    let (te, ws, _repo) = fixture("scratch").await;
    walk(&te, &ws.id, "PA", &ws.id);
    assert_eq!(
        te.engine.rename_chat_session(&ws.id, "PL", "Dark mode"),
        "Renamed chat to Dark mode."
    );
    assert_eq!(te.engine.chat_tab_title(&ws.id, "PA"), "Dark mode");
    te.close().await;
}

#[tokio::test]
async fn planner_rename_follows_the_calling_turns_conductor() {
    let (te, ws, _repo) = fixture("scratch").await;
    te.engine.ensure_pair_agent(&ws.id, "PA2", "");
    te.store.update_workspace(&ws.id, |l| {
        l.run_started = true;
        l.loop_chat_code = "PA".into();
        l.state = WorkspaceState::Planning;
    });
    let a = walk(&te, &ws.id, "PA", &ws.id);
    let b = walk(&te, &ws.id, "PA2", &format!("{}\x1ePA2", ws.id));
    let (_ia, _ba) = a.bind_inject("PL");
    let (_ib, _bb) = b.bind_inject("PL");
    let h = EngineHost::new(std::sync::Arc::downgrade(&te.engine));
    assert_eq!(
        h.rename_session(&ws.id, "PL", "PA2", "Dark mode").await,
        "Renamed chat to Dark mode."
    );
    assert_eq!(te.engine.chat_tab_title(&ws.id, "PA2"), "Dark mode");
    assert_ne!(te.engine.chat_tab_title(&ws.id, "PA"), "Dark mode");
    te.close().await;
}

#[tokio::test]
async fn rename_keeps_the_tabs_own_provider() {
    let (te, ws, _repo) = fixture("rename-provider").await;
    let mut set = te.store.settings();
    set.model_agent = "cursor/composer-2.5".into();
    te.store.set_settings(set);
    te.store
        .update_agent(&ws.id, "PA", |a| a.model = "claude-code/sonnet".into());
    te.engine
        .ensure_agent_session(&ws.id, "coder", "PA", "Agent", "claude-code")
        .unwrap();
    te.engine.rename_chat_session(&ws.id, "PA", "Titled");
    assert_eq!(
        te.store
            .session_for_loop_agent(&ws.id, "PA")
            .unwrap()
            .provider,
        "claude-code"
    );
    te.close().await;
}

#[tokio::test]
async fn rename_session_refusals() {
    let (te, ws, _repo) = fixture("r").await;
    assert_eq!(
        te.engine.rename_chat_session(&ws.id, "PA", " "),
        "rename_session: title is required"
    );
    assert_eq!(
        te.engine.rename_chat_session(&ws.id, " ", "x"),
        "rename_session: missing agent"
    );
    assert_eq!(
        te.engine.rename_chat_session(&ws.id, "PA9", "x"),
        "rename_session: unknown chat tab"
    );
    let long = "é".repeat(250);
    let got = te.engine.rename_chat_session(&ws.id, "PA", &long);
    assert_eq!(got, format!("Renamed chat to {}.", "é".repeat(200)));
    assert_eq!(
        te.engine.rename_workspace_branch("nope", "x").await,
        "rename_branch: unknown workspace"
    );
    te.close().await;
}

#[test]
fn sanitize_titles() {
    assert_eq!(sanitize_title("\"Fix the bug.\"", 60), "Fix the bug");
    assert_eq!(sanitize_title("First line\nsecond", 60), "First line");
    assert_eq!(sanitize_title("New Workspace", 60), "");
    assert_eq!(sanitize_title("  ", 60), "");
    assert_eq!(sanitize_title(&"a".repeat(70), 60).chars().count(), 60);
    assert_eq!(sanitize_workspace_title("Ship it!"), "Ship it");
}
