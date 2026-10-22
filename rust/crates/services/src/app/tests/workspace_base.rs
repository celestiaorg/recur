//! `workspace_base_test.go`.

use super::{Fx, git, git_out};

async fn develop_fixture() -> (Fx, String, String) {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    git(&repo, &["branch", "develop"]);
    let pid = fx.add_project(&repo);
    (fx, repo, pid)
}

#[tokio::test(flavor = "multi_thread")]
async fn set_workspace_base_branch() {
    let (fx, _, pid) = develop_fixture().await;
    let ws = fx.solo(&pid, "retarget me", "main").await;
    assert_eq!(ws.base_branch, "main");
    let w = &fx.s.workspaces;
    let got = w.set_workspace_base_branch(&ws.id, "develop").await.unwrap();
    assert_eq!(got.base_branch, "develop");
    assert_eq!(fx.store().workspace(&ws.id).unwrap().base_branch, "develop");
    let again = w.set_workspace_base_branch(&ws.id, "develop").await.unwrap();
    assert_eq!(again.base_branch, "develop", "same branch is a no-op");
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn set_workspace_base_branch_refuses() {
    let (fx, repo, pid) = develop_fixture().await;
    let ws = fx.solo(&pid, "refusals", "main").await;
    let w = &fx.s.workspaces;
    for (name, id, base) in [
        ("empty", ws.id.as_str(), ""),
        ("whitespace", &ws.id, "  "),
        ("missing workspace", "no-such", "develop"),
        ("unknown branch", &ws.id, "does-not-exist"),
        ("own branch", &ws.id, &ws.branch),
        ("HEAD", &ws.id, "HEAD"),
    ] {
        assert!(w.set_workspace_base_branch(id, base).await.is_err(), "{name}");
    }
    let main = w.open_main_workspace(&pid).await.unwrap();
    assert!(w.set_workspace_base_branch(&main.id, "develop").await.is_err());
    git(&repo, &["tag", "v1.0"]);
    assert!(w.set_workspace_base_branch(&ws.id, "v1.0").await.is_err(), "tag");
    let sha = git_out(&repo, &["rev-parse", "HEAD"]).unwrap();
    assert!(w.set_workspace_base_branch(&ws.id, sha.trim()).await.is_err(), "sha");
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn set_workspace_base_branch_retargets_open_pr() {
    let (fx, _, pid) = develop_fixture().await;
    let ws = fx.solo(&pid, "open a pr", "main").await;
    fx.commit_file(&pid, &ws.id, "feature.txt", "the feature\n", "solo: add feature")
        .await;
    let pr_id = fx.s.workspaces.open_workspace_pr(&ws.id, false).await.unwrap();
    assert_eq!(fx.s.prs.get_pr(&pr_id).await.unwrap().base, "main");
    fx.s.workspaces
        .set_workspace_base_branch(&ws.id, "develop")
        .await
        .unwrap();
    assert_eq!(fx.s.prs.get_pr(&pr_id).await.unwrap().base, "develop");
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn set_workspace_base_branch_leaves_merged_pr() {
    let (fx, _, pid) = develop_fixture().await;
    let ws = fx.solo(&pid, "merged pr", "main").await;
    fx.commit_file(&pid, &ws.id, "feature.txt", "the feature\n", "solo: add feature")
        .await;
    let pr_id = fx.s.workspaces.open_workspace_pr(&ws.id, false).await.unwrap();
    fx.s.prs.merge_pr(&pr_id, "").await.unwrap();
    fx.s.workspaces
        .set_workspace_base_branch(&ws.id, "develop")
        .await
        .unwrap();
    assert_eq!(fx.s.prs.get_pr(&pr_id).await.unwrap().base, "main", "historical");
    assert_eq!(fx.store().workspace(&ws.id).unwrap().base_branch, "develop");
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn set_workspace_base_branch_trims() {
    let (fx, _, pid) = develop_fixture().await;
    let ws = fx.solo(&pid, "trim", "main").await;
    let got = fx
        .s
        .workspaces
        .set_workspace_base_branch(&ws.id, "  develop  ")
        .await
        .unwrap();
    assert_eq!(got.base_branch, "develop");
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn set_workspace_base_branch_accepts_remote_tracking() {
    let fx = Fx::new().await;
    let (clone, _) = fx.init_git_repo_with_remote();
    let pid = fx.add_project(&clone);
    let ws = fx.solo(&pid, "remote base", "main").await;
    let got = fx
        .s
        .workspaces
        .set_workspace_base_branch(&ws.id, "origin/main")
        .await
        .unwrap();
    assert_eq!(got.base_branch, "origin/main");
    fx.close().await;
}
