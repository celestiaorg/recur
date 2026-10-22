//! `workspace_open_test.go`.

use recur_domain::Workspace;

use super::Fx;

#[tokio::test(flavor = "multi_thread")]
async fn open_workspace_in() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let pid = fx.add_project(&repo);
    let ws = fx
        .s
        .workspaces
        .create_workspace(&pid, "open-in", "", "agent", "", "", None)
        .await
        .unwrap();
    let ws = fx.wait_ready(&ws.id).await;
    assert!(!ws.worktree_path.is_empty());
    let targets = fx.s.workspaces.list_open_in_targets().await.unwrap();
    if cfg!(target_os = "macos") {
        assert_eq!(targets.first().map(|t| t.id.as_str()), Some("finder"));
    }
    assert!(fx.s.workspaces.open_workspace_in("nope", "finder").await.is_err());
    fx.store()
        .add_workspace(Workspace {
            id: "empty-wt".into(),
            project_id: pid.clone(),
            ..Default::default()
        })
        .unwrap();
    let e = fx
        .s
        .workspaces
        .open_workspace_in("empty-wt", "finder")
        .await
        .unwrap_err();
    assert_eq!(e.message, "workspace \"empty-wt\" has no folder on disk");
    assert!(fx.s.workspaces.open_workspace_in(&ws.id, "nope").await.is_err());
    fx.close().await;
}
