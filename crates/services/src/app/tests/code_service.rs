//! `code_service_test.go`.

use std::collections::BTreeSet;

use recur_domain::{FileDiff, Workspace};

use super::{Fx, git, write};

/// A worktree on `branch` off main, stored as workspace `id` of project `p`.
async fn worktree_workspace(fx: &Fx, repo: &str, id: &str, branch: &str) -> (String, String) {
    let pid = fx.add_project(repo);
    let mgr = recur_git::Manager::new(repo, &fx.temp());
    let (wt, _) = mgr.add(id, branch, "main").await.unwrap();
    fx.store()
        .add_workspace(Workspace {
            id: id.into(),
            project_id: pid.clone(),
            worktree_path: wt.path.clone(),
            base_branch: "main".into(),
            ..Default::default()
        })
        .unwrap();
    (pid, wt.path)
}

fn commit_all(dir: &str, msg: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-m", msg]);
}

fn paths(files: &[FileDiff]) -> BTreeSet<&str> {
    files.iter().map(|f| f.path.as_str()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn list_changed_files_live_worktree() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let (_, wt) = worktree_workspace(&fx, &repo, "loop-live", "live-refresh").await;
    let code = &fx.s.code;
    assert!(
        code.list_changed_files("loop-live")
            .await
            .unwrap()
            .is_empty()
    );
    write(format!("{wt}/README.md"), "hi\nedited\n");
    write(format!("{wt}/new.go"), "package main\n");
    let mut got = code.list_changed_files("loop-live").await.unwrap();
    got.sort();
    assert_eq!(got, ["README.md", "new.go"]);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn read_file_and_list_tree_report_not_found() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let pid = fx.add_project(&repo);
    let e =
        fx.s.code
            .read_file(&pid, "", "does-not-exist.txt")
            .await
            .unwrap_err();
    assert_eq!(e.message, "not found: does-not-exist.txt");
    let e =
        fx.s.code
            .list_tree(&pid, "", "no/such/dir")
            .await
            .unwrap_err();
    assert_eq!(e.message, "not found: no/such/dir");
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn diff_vs_base_and_commits() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let (pid, wt) = worktree_workspace(&fx, &repo, "ws-review", "review").await;
    write(format!("{wt}/feat.txt"), "one\n");
    commit_all(&wt, "add feat");
    write(format!("{wt}/clean.txt"), "ok\n");
    commit_all(&wt, "add clean");
    write(format!("{wt}/feat.txt"), "one\ntwo\n");
    write(format!("{wt}/new.txt"), "untracked\n");

    let code = &fx.s.code;
    let base = code.diff_vs_base(&pid, "ws-review").await.unwrap();
    let base = base.files.unwrap_or_default();
    assert!(
        paths(&base).is_superset(&BTreeSet::from(["feat.txt", "new.txt", "clean.txt"])),
        "{:?}",
        paths(&base)
    );
    let working = code.diff_working(&pid, "ws-review").await.unwrap();
    let working = working.files.unwrap_or_default();
    let working = paths(&working);
    assert!(
        working.contains("feat.txt") && working.contains("new.txt"),
        "{working:?}"
    );
    assert!(
        !working.contains("clean.txt"),
        "committed and clean vs HEAD"
    );

    let commits = code.list_commits(&pid, "ws-review").await.unwrap();
    assert_eq!(commits.len(), 2);
    let feat = commits
        .iter()
        .find(|c| c.message == "add feat")
        .expect("add feat");
    let diff = code
        .commit_diff(&pid, "ws-review", &feat.hash)
        .await
        .unwrap();
    assert!(paths(&diff).contains("feat.txt"), "{diff:?}");
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn list_commits_main_working_copy() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let pid = fx.add_project(&repo);
    fx.store()
        .add_workspace(Workspace {
            id: "main-wc".into(),
            project_id: pid.clone(),
            worktree_path: repo,
            is_main: true,
            ..Default::default()
        })
        .unwrap();
    let commits = fx.s.code.list_commits(&pid, "main-wc").await.unwrap();
    assert_eq!(
        commits
            .iter()
            .map(|c| c.message.as_str())
            .collect::<Vec<_>>(),
        ["initial"]
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn diff_working_unborn_head() {
    let fx = Fx::new().await;
    let dir = fx.temp();
    git(&dir, &["init", "-b", "main"]);
    write(format!("{dir}/staged.txt"), "in index\n");
    git(&dir, &["add", "staged.txt"]);
    write(format!("{dir}/loose.txt"), "untracked\n");
    let pid = fx.add_project(&dir);
    fx.store()
        .add_workspace(Workspace {
            id: "unborn".into(),
            project_id: pid.clone(),
            worktree_path: dir,
            is_main: true,
            ..Default::default()
        })
        .unwrap();
    let got = fx.s.code.diff_working(&pid, "unborn").await.unwrap();
    let got = got.files.unwrap_or_default();
    let got = paths(&got);
    assert!(
        got.contains("staged.txt") && got.contains("loose.txt"),
        "{got:?}"
    );
    fx.close().await;
}
