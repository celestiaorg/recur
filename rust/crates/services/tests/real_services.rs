//! Boots the real services on a temp data dir with a scripted Claude
//! transcript and a fake GitHub forge, then drives a chat turn, a loop to an
//! open PR, and a PR sync, checking the AppEvents a view would receive.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use recur_domain::{Project, WorkspaceState};
use recur_engine::GitSetup;
use recur_forge::testing::FakeForge;
use recur_forge::{Forge, PrListRow, PrListState};
use recur_provider_claude::testing::NdjsonScript;
use recur_services::{AppConfig, AppEvent, AppOptions, AppServices};
use recur_store::Store;
use tokio::sync::mpsc::UnboundedReceiver;

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// Collects events until `done` holds for one of them (or 20s pass).
async fn collect_until(
    rx: &mut UnboundedReceiver<AppEvent>,
    seen: &mut Vec<AppEvent>,
    done: impl Fn(&AppEvent) -> bool,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(ev)) => {
                let hit = done(&ev);
                seen.push(ev);
                if hit {
                    return;
                }
            }
            _ => panic!(
                "timed out; saw {:?}",
                seen.iter().map(AppEvent::name).collect::<Vec<_>>()
            ),
        }
    }
}

/// Collects until no event arrives for 300ms.
async fn drain(rx: &mut UnboundedReceiver<AppEvent>, seen: &mut Vec<AppEvent>) {
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(300), rx.recv()).await {
        seen.push(ev);
    }
}

fn names(seen: &[AppEvent]) -> Vec<&'static str> {
    seen.iter().map(AppEvent::name).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn chat_loop_and_pr_sync_emit_app_events() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let config = AppConfig::under(&root);
    std::fs::create_dir_all(&config.worktrees_dir).unwrap();
    std::fs::create_dir_all(&config.bundles.default_dir).unwrap();
    std::fs::create_dir_all(&config.bundles.user_dir).unwrap();
    let store = Arc::new(Store::open(&root.join("recur.db")).unwrap());

    let provider = NdjsonScript::new([
        r#"{"type":"system","subtype":"init","session_id":"s1","model":"sonnet","mcp_servers":[]}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"working"},{"type":"tool_use","id":"t1","name":"Write","input":{"file_path":"svc.txt"}}]}}"#,
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":false}]}}"#,
        r#"{"type":"result","subtype":"success","session_id":"s1","duration_ms":10,"is_error":false,"num_turns":1,"result":"scripted reply","total_cost_usd":0}"#,
    ])
    .with_edits([("svc.txt".to_owned(), "from the agent\n".to_owned())]);

    let forge = Arc::new(FakeForge::new());
    forge.on_login(|()| async { "avery".to_owned() });
    forge.on_list_prs(|(_, state, _)| async move {
        if state != PrListState::Open {
            return Ok(Vec::new());
        }
        Ok(vec![PrListRow {
            number: 12,
            title: "Please review".into(),
            url: "https://github.com/acme/app/pull/12".into(),
            head_ref: "feat/oauth".into(),
            base_ref: "main".into(),
            author_login: "sam".into(),
            state: "OPEN".into(),
            checks_ok: true,
            updated_at: chrono::Utc::now().to_rfc3339(),
            ..PrListRow::default()
        }])
    });

    let svc = AppServices::new(
        store,
        config.clone(),
        AppOptions {
            pin: Some(Some(Arc::new(provider))),
            forges: Some(vec![forge as Arc<dyn Forge>]),
            trackers: Some(Vec::new()),
            git: Some(GitSetup {
                worktree_root: config.worktrees_dir.to_string_lossy().into_owned(),
                ..GitSetup::default()
            }),
        },
    )
    .await;
    svc.recover();
    let s = svc.services();
    let mut rx = svc.events().subscribe();
    let mut seen = Vec::new();

    // Rail #1 (verify_browser) gates pr_open behind a browser verify this
    // repo has no run config for.
    let mut policy = svc.engine().store.policy();
    for r in policy.rails.iter_mut().flatten() {
        if r.key == "verify_browser" {
            r.on = false;
        }
    }
    svc.engine().store.set_policy(policy);

    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    std::fs::write(repo.join("README.md"), "hi\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "initial"]);
    let pid = s
        .projects
        .add_project(&Project {
            name: "acme".into(),
            repo_path: repo.to_string_lossy().into_owned(),
            base_branch: "main".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    assert!(
        s.projects.list_projects().await.unwrap().iter().any(|p| p.id == pid),
        "list projects"
    );

    // A chat turn in a hand-driven workspace.
    let ws = s
        .workspaces
        .create_workspace(&pid, "chat about it", "", "agent", "", "", None)
        .await
        .unwrap();
    collect_until(&mut rx, &mut seen, |e| {
        matches!(e, AppEvent::WorkspaceUpdated(u) if u.workspace_id == ws.id && u.worktree_path.as_deref().is_some_and(|p| !p.is_empty()))
    })
    .await;
    let reply = s
        .workspaces
        .send_workspace_message(&ws.id, "", "hello", "", &BTreeMap::new(), &[], "agent", "", false)
        .await
        .unwrap();
    assert_eq!(reply, "working");
    drain(&mut rx, &mut seen).await;
    let chat = names(&seen);
    for want in [
        "workspace.chat_tab",
        "agent.updated",
        "session.user",
        "session.delta",
        "session.step",
        "session.tool_result",
        "session.turn_end",
    ] {
        assert!(chat.contains(&want), "chat turn missing {want}: {chat:?}");
    }

    // A loop from goal to an open PR.
    seen.clear();
    let id = s
        .workspaces
        .start_workspace(&pid, "Add a service file", "", "")
        .await
        .unwrap();
    collect_until(&mut rx, &mut seen, |e| {
        matches!(e, AppEvent::WorkspaceUpdated(u) if u.workspace_id == id && u.state == Some(WorkspaceState::PrOpen))
    })
    .await;
    let looped = names(&seen);
    for want in ["workspace.updated", "agent.updated", "workspace.event", "pr.updated"] {
        assert!(looped.contains(&want), "loop missing {want}: {looped:?}");
    }
    let pr = s
        .prs
        .list_prs()
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.workspace_id == id)
        .expect("the loop's PR");
    assert!(pr.file_count > 0, "{pr:?}");
    s.workspaces.stop_loop(&id).await.unwrap();

    // A PR sync importing the forge's open PR.
    seen.clear();
    let synced = s.prs.sync_github_prs().await.unwrap();
    assert!(synced.iter().any(|p| p.number == 12 && p.project_id == pid), "{synced:?}");
    collect_until(&mut rx, &mut seen, |e| matches!(e, AppEvent::PrsSynced(_))).await;
    assert!(
        s.prs.list_prs().await.unwrap().iter().any(|p| p.number == 12),
        "the synced PR is listed"
    );

    svc.shutdown().await;
}
