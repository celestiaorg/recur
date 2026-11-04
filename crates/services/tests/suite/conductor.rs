//! Import from Conductor over a Conductor-shaped database: a live workspace
//! is adopted in place with its chat, an archived one is skipped, and a rerun
//! imports nothing.

use std::path::Path;
use std::sync::Arc;

use pretty_assertions::assert_eq;
use recur_engine::GitSetup;
use recur_engine::chat::fork::fork_history_preamble;
use recur_engine::session_config::AgentConfig;
use recur_services::{AppConfig, AppOptions, AppServices};
use recur_store::Store;

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn conductor_db(path: &Path, repo: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let c = rusqlite::Connection::open(path).unwrap();
    c.execute_batch(
        "CREATE TABLE repos (id TEXT PRIMARY KEY, name TEXT, root_path TEXT, default_branch TEXT);
         CREATE TABLE workspaces (local_id TEXT PRIMARY KEY, repository_id TEXT, directory_name TEXT,
             branch TEXT, state TEXT, workspace_path TEXT, workspace_name TEXT, pr_title TEXT,
             intended_target_branch TEXT, initialization_parent_branch TEXT, created_at TEXT);
         CREATE TABLE sessions (id TEXT PRIMARY KEY, workspace_id TEXT, agent_type, model TEXT,
             title TEXT, is_hidden INTEGER, created_at TEXT);
         CREATE TABLE session_messages (id TEXT PRIMARY KEY, session_id TEXT, role TEXT, content TEXT,
             created_at TEXT, sent_at TEXT, cancelled_at TEXT);",
    )
    .unwrap();
    c.execute(
        "INSERT INTO repos VALUES ('r1', 'app', ?1, 'main')",
        [repo.to_string_lossy()],
    )
    .unwrap();
    c.execute_batch(
        "INSERT INTO workspaces VALUES
           ('w1', 'r1', 'asuncion', 'feat/x', 'ready', NULL, 'asuncion', 'Fix the thing', NULL, 'main', '2026-01-01'),
           ('w2', 'r1', 'tacoma', 'feat/old', 'archived', NULL, NULL, NULL, NULL, NULL, '2026-01-02');
         INSERT INTO sessions VALUES
           ('s1', 'w1', 'claude', 'opus-4-8-1m', 'Conductor chat', 0, '2026-01-01'),
           ('s2', 'w1', 'claude', 'opus', 'Hidden', 1, '2026-01-01'),
           ('s3', 'w2', 'claude', 'opus', 'Archived', 0, '2026-01-02');",
    )
    .unwrap();
    let rows = [
        ("user", "hello there", None),
        ("user", "never mind", Some("2026-01-01T00:00:01Z")),
        (
            "assistant",
            r#"{"type":"system","subtype":"init","session_id":"old-claude","model":"opus","mcp_servers":[]}"#,
            None,
        ),
        (
            "assistant",
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Reading it"},{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"a.txt"}}]}}"#,
            None,
        ),
        (
            "assistant",
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"file body"}]}}"#,
            None,
        ),
        (
            "assistant",
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"All done"}]}}"#,
            None,
        ),
        (
            "assistant",
            r#"{"type":"result","subtype":"success","session_id":"old-claude","duration_ms":5,"is_error":false,"result":"All done"}"#,
            None,
        ),
    ];
    for (i, (role, content, cancelled)) in rows.iter().enumerate() {
        c.execute(
            "INSERT INTO session_messages VALUES (?1, 's1', ?2, ?3, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', ?4)",
            rusqlite::params![format!("m{i}"), role, content, cancelled],
        )
        .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn imports_live_conductor_workspaces_once() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let config = AppConfig::under(&root);
    std::fs::create_dir_all(&config.worktrees_dir).unwrap();
    let home = config.home.clone().unwrap();

    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "hi\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "init"]);
    let wt = home.join("conductor/workspaces/app/asuncion");
    git(
        &repo,
        &["worktree", "add", "-b", "feat/x", &wt.to_string_lossy()],
    );
    // Conductor's root_path is stale once a repo moves; the worktree still
    // points at the real checkout.
    conductor_db(
        &home.join("Library/Application Support/com.conductor.app/conductor.db"),
        &root.join("moved-away"),
    );

    let store = Arc::new(Store::open(&root.join("recur.db")).unwrap());
    let svc = AppServices::new(
        store,
        config.clone(),
        AppOptions {
            pin: Some(None),
            forges: Some(Vec::new()),
            trackers: Some(Vec::new()),
            git: Some(GitSetup {
                worktree_root: config.worktrees_dir.to_string_lossy().into_owned(),
                ..GitSetup::default()
            }),
        },
    )
    .await;
    let s = svc.services();
    assert!(svc.conductor_db().is_some());

    let report = s.projects.import_conductor().await.unwrap();
    assert_eq!(
        (
            report.projects_added,
            report.workspaces_imported,
            report.chats_imported
        ),
        (1, 1, 1),
        "{report:?}"
    );
    assert!(report.skipped.is_empty(), "{report:?}");

    let projects = s.projects.list_projects().await.unwrap();
    let project = projects
        .iter()
        .find(|p| p.repo_path == repo.to_string_lossy())
        .expect("project for the repo");
    let imported: Vec<_> = s
        .workspaces
        .list_workspaces()
        .await
        .unwrap()
        .into_iter()
        .filter(|w| !w.is_main)
        .collect();
    assert_eq!(imported.len(), 1);
    let ws = &imported[0];
    assert_eq!(ws.project_id, project.id);
    assert_eq!(ws.worktree_path, wt.to_string_lossy());
    assert_eq!(ws.branch, "feat/x");
    assert_eq!(ws.base_branch, "main");
    assert_eq!(ws.goal, "Fix the thing");
    assert!(ws.port > 0);
    assert!(wt.join("a.txt").is_file());

    let tabs = s.workspaces.list_workspace_chats(&ws.id).await.unwrap();
    assert_eq!(
        tabs.iter()
            .map(|t| (t.code.as_str(), t.title.as_str()))
            .collect::<Vec<_>>(),
        [("PA", "Conductor chat")]
    );
    let store = &svc.engine().store;
    let sess = store.session_for_loop_agent(&ws.id, "PA").unwrap();
    assert_eq!(sess.provider, "claude-code");
    assert_eq!(sess.provider_session_ref, "");
    let cfg = AgentConfig::parse(&sess.config_json);
    assert!(cfg.fork_inject);
    assert_eq!(cfg.model, "claude-code/claude-opus-4-8");

    let msgs = store.session_transcript(&sess.id).unwrap().unwrap();
    assert_eq!(
        msgs.iter().map(|m| m.role.as_str()).collect::<Vec<_>>(),
        ["user", "assistant", "user", "assistant"]
    );
    let preamble = fork_history_preamble(&msgs, None, "");
    for want in ["hello there", "Reading it", "file body", "All done"] {
        assert!(preamble.contains(want), "{want} missing from {preamble}");
    }
    assert!(!preamble.contains("never mind"));

    let again = s.projects.import_conductor().await.unwrap();
    assert_eq!((again.projects_added, again.workspaces_imported), (0, 0));
    assert_eq!(again.skipped, ["Fix the thing: already in Recur"]);
}
