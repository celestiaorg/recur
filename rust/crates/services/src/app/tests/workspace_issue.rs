//! `workspace_issue_test.go` (the service half: link moves, clear, claim).

use recur_domain::{IssueRef, Workspace};

use super::Fx;

#[tokio::test(flavor = "multi_thread")]
async fn set_workspace_issue_moves_link_and_clears() {
    let fx = Fx::new().await;
    for id in ["A", "B"] {
        fx.store()
            .add_workspace(Workspace {
                id: id.into(),
                project_id: "p".into(),
                ..Default::default()
            })
            .unwrap();
    }
    let issue = IssueRef {
        source: "linear".into(),
        id: "uuid-157".into(),
        key: "BOTFUN-157".into(),
    };
    fx.s.workspaces.set_workspace_issue("A", &issue).await.unwrap();
    fx.s.workspaces.set_workspace_issue("B", &issue).await.unwrap();
    assert!(fx.store().workspace("A").unwrap().issue.is_none());
    assert_eq!(fx.store().workspace("B").unwrap().issue.unwrap().id, issue.id);
    // The claim ran before returning, once per link.
    assert_eq!(fx.trackers[1].claimed().len(), 2);
    fx.s.workspaces.clear_workspace_issue("B").await.unwrap();
    assert!(fx.store().workspace("B").unwrap().issue.is_none());
    assert!(fx.s.workspaces.clear_workspace_issue("missing").await.is_err());
    let e = fx
        .s
        .workspaces
        .set_workspace_issue("A", &IssueRef::default())
        .await
        .unwrap_err();
    assert_eq!(e.message, "issue source and id are required");
    fx.close().await;
}
