//! Skills settings, read_terminal and MCP argument backfill tests.

use std::time::{Duration, Instant};

use recur_agent_tools::Host;

use crate::host::EngineHost;
use crate::testing::{TestEngine, add_project, init_repo};

fn skill(worktree: &str, name: &str) {
    let dir = std::path::Path::new(worktree)
        .join(".agents/skills")
        .join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: Noisy helper.\n---\n# Noise\n"),
    )
    .unwrap();
}

async fn workspace(te: &TestEngine, repo: &str) -> recur_domain::Workspace {
    let proj = add_project(&te.store, repo);
    te.engine
        .create_workspace(&proj, "skills", "")
        .await
        .unwrap()
}

fn host(te: &TestEngine) -> EngineHost {
    EngineHost::new(std::sync::Arc::downgrade(&te.engine))
}

fn listed(list: &[recur_domain::AgentSkill], name: &str) -> bool {
    list.iter().any(|s| s.name == name)
}

#[tokio::test]
async fn list_and_activate_respect_disabled_settings() {
    let te = TestEngine::new(None);
    let repo = init_repo();
    let ws = workspace(&te, &repo.path).await;
    skill(&ws.worktree_path, "recur-t-noise");
    let h = host(&te);
    assert!(listed(
        &h.list_skills(&ws.id, "PA").await.unwrap(),
        "recur-t-noise"
    ));
    let mut set = te.store.settings();
    set.disabled_skills = vec!["recur-t-noise".into()];
    te.store.set_settings(set);
    assert!(!listed(
        &h.list_skills(&ws.id, "PA").await.unwrap(),
        "recur-t-noise"
    ));
    let e = h
        .activate_skill(&ws.id, "PA", "recur-t-noise")
        .await
        .unwrap_err();
    assert_eq!(e.0, "skill \"recur-t-noise\" is disabled");
    let mut set = te.store.settings();
    set.disabled_skills = Vec::new();
    te.store.set_settings(set);
    let out = h
        .activate_skill(&ws.id, "PA", "recur-t-noise")
        .await
        .unwrap();
    assert!(out.contains("<skill_content name=\"recur-t-noise\">"));
    let e = h
        .activate_skill(&ws.id, "PA", "recur-t-missing")
        .await
        .unwrap_err();
    assert_eq!(
        e.0,
        "unknown skill \"recur-t-missing\" — call list_skills for names"
    );
    te.close().await;
}

#[tokio::test]
async fn project_disable_and_enable_overrides() {
    let te = TestEngine::new(None);
    let repo = init_repo();
    let ws = workspace(&te, &repo.path).await;
    skill(&ws.worktree_path, "recur-t-repo-only");
    skill(&ws.worktree_path, "recur-t-special");
    let mut proj = te.store.project(&ws.project_id).unwrap();
    proj.disabled_skills = vec!["recur-t-repo-only".into()];
    te.store.update_project_run_config(proj).unwrap();
    let h = host(&te);
    assert!(!listed(
        &h.list_skills(&ws.id, "PA").await.unwrap(),
        "recur-t-repo-only"
    ));
    assert!(
        h.activate_skill(&ws.id, "PA", "recur-t-repo-only")
            .await
            .unwrap_err()
            .0
            .contains("disabled")
    );
    let mut set = te.store.settings();
    set.disabled_skills = vec!["recur-t-special".into()];
    te.store.set_settings(set);
    assert!(!listed(
        &h.list_skills(&ws.id, "PA").await.unwrap(),
        "recur-t-special"
    ));
    let mut proj = te.store.project(&ws.project_id).unwrap();
    proj.enabled_skills = vec!["recur-t-special".into()];
    te.store.update_project_run_config(proj).unwrap();
    assert!(listed(
        &h.list_skills(&ws.id, "PA").await.unwrap(),
        "recur-t-special"
    ));
    h.activate_skill(&ws.id, "PA", "recur-t-special")
        .await
        .unwrap();
    te.close().await;
}

#[tokio::test]
async fn model_disabled_skills_stay_off_the_catalog() {
    let te = TestEngine::new(None);
    let repo = init_repo();
    let ws = workspace(&te, &repo.path).await;
    let dir = std::path::Path::new(&ws.worktree_path).join(".agents/skills/recur-t-user-only");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: recur-t-user-only\ndescription: x\ndisable-model-invocation: true\n---\nbody\n",
    )
    .unwrap();
    let h = host(&te);
    assert!(!listed(
        &h.list_skills(&ws.id, "PA").await.unwrap(),
        "recur-t-user-only"
    ));
    assert_eq!(
        h.activate_skill(&ws.id, "PA", "recur-t-user-only")
            .await
            .unwrap_err()
            .0,
        "skill \"recur-t-user-only\" is user-invocable only (disable-model-invocation)"
    );
    assert_eq!(
        h.list_skills("nope", "PA").await.unwrap_err().0,
        "workspace not found"
    );
    te.close().await;
}

fn term_engine() -> (TestEngine, std::sync::Arc<recur_term::Manager>) {
    let mgr = std::sync::Arc::new(recur_term::Manager::new(|_| {}));
    let te = TestEngine::with(crate::testing::TestOptions {
        terminals: Some(mgr.clone()),
        ..Default::default()
    });
    (te, mgr)
}

#[tokio::test]
async fn read_terminal_marks_exited_and_lists() {
    let (te, mgr) = term_engine();
    let cwd = tempfile::tempdir().unwrap();
    let s = mgr
        .open("ws", &cwd.path().to_string_lossy(), 80, 24, &[])
        .unwrap();
    let h = host(&te);
    let list = h.list_terminals("ws", "PA").await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, s.id);
    mgr.write(&s.id, "exit 1\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && !mgr.list("ws").iter().any(|i| i.exited) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let got = h.read_terminal("ws", "PA", &s.id, 20).await.unwrap();
    assert!(got.contains(", exited (1)"), "{got}");
    assert_eq!(
        h.read_terminal("other", "PA", "", 20).await.unwrap_err().0,
        "no live terminals in this workspace"
    );
    assert_eq!(
        h.read_terminal("ws", "PA", "zzz", 20).await.unwrap_err().0,
        "terminal \"zzz\" not found — call list_terminals"
    );
    mgr.close_all();
    te.close().await;
}

#[tokio::test]
async fn terminals_without_a_source() {
    let te = TestEngine::new(None);
    let h = host(&te);
    assert!(h.list_terminals("ws", "PA").await.is_none());
    assert_eq!(
        h.read_terminal("ws", "PA", "", 0).await.unwrap_err().0,
        "no Observatory terminals in this session"
    );
    te.close().await;
}

#[tokio::test]
async fn note_tool_args_backfills_empty_tool_use() {
    let te = TestEngine::new(None);
    crate::turn::test_support::add_solo_workspace(&te.engine, "W1");
    te.engine.ensure_pair_agent("W1", "PA", "codex/gpt-5");
    let sess = te
        .engine
        .ensure_agent_session("W1", "coder", "PA", "Agent", "codex")
        .unwrap();
    te.engine.set_agent_session("W1", "PA", &sess.id);
    let msg = te
        .store
        .create_message(
            &sess.id,
            recur_domain::Message {
                role: "assistant".into(),
                ..Default::default()
            },
        )
        .unwrap();
    te.store
        .append_content_block(
            &msg,
            recur_domain::ContentBlock::tool_use("int-1", "call_1", "mcp__recur__ask_user", "{}"),
        )
        .unwrap();
    let h = host(&te);
    h.note_tool_args("W1", "PA", "ask_user", r#"{"question":"Which?"}"#);
    let tu = te
        .store
        .latest_tool_use_by_name_suffix(&sess.id, "__ask_user")
        .unwrap();
    assert_eq!(tu.input, r#"{"question":"Which?"}"#);
    let ev = te.events.by_name("session.tool_input");
    assert_eq!(ev.len(), 1);
    assert_eq!(
        ev[0].payload_json(),
        r#"{"code":"PA","id":"call_1","input":"{\"question\":\"Which?\"}","toolInput":"Which?","workspaceId":"W1"}"#
    );
    // Already filled, or nothing worth writing: untouched.
    h.note_tool_args("W1", "PA", "ask_user", r#"{"question":"Other"}"#);
    h.note_tool_args("W1", "PA", "ask_user", "{}");
    h.note_tool_args("W1", "PA", "ask_user", "null");
    h.note_tool_args("W1", "PA", "ask_user", "");
    assert_eq!(
        te.store
            .latest_tool_use_by_name_suffix(&sess.id, "__ask_user")
            .unwrap()
            .input,
        r#"{"question":"Which?"}"#
    );
    te.close().await;
}
