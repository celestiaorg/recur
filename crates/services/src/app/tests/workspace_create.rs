//! Workspace creation services.

use recur_domain::{Project, WORKSPACE_SOLO};
use recur_git::LOCAL_BRANCH_GROUP;

use super::{Fx, git, write};

#[tokio::test(flavor = "multi_thread")]
async fn create_solo_workspace() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let pid = fx.add_project(&repo);
    let ws =
        fx.s.workspaces
            .create_workspace(&pid, "tidy the header", "", "agent", "", "", None)
            .await
            .unwrap();
    assert_eq!(ws.r#type, WORKSPACE_SOLO);
    assert!(!ws.branch.is_empty());
    assert!(
        ws.worktree_path.is_empty(),
        "returns before the worktree exists"
    );
    let ws = fx.wait_ready(&ws.id).await;
    assert!(!ws.worktree_path.is_empty());
    assert!(
        fx.s.workspaces
            .list_workspaces()
            .await
            .unwrap()
            .iter()
            .any(|w| w.id == ws.id)
    );
    let newer =
        fx.s.workspaces
            .create_workspace(&pid, "after", "", "agent", "", "", None)
            .await
            .unwrap();
    let list = fx.s.workspaces.list_workspaces().await.unwrap();
    assert_eq!(list[0].id, newer.id, "newest first");
    fx.wait_ready(&newer.id).await;
    assert!(
        fx.s.code
            .write_file(&pid, "", "solo.txt", "hi\n")
            .await
            .is_err(),
        "the bare project checkout is not writable"
    );
    fx.s.code
        .write_file(&pid, &ws.id, "solo.txt", "hi\n")
        .await
        .unwrap();
    let res =
        fx.s.code
            .commit(&pid, &ws.id, "solo: add file")
            .await
            .unwrap();
    assert!(res.committed);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn generate_commit_message_requires_staged() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let pid = fx.add_project(&repo);
    let ws =
        fx.s.workspaces
            .create_workspace(&pid, "scm ai", "", "agent", "", "", None)
            .await
            .unwrap();
    let ws = fx.wait_ready(&ws.id).await;
    let e =
        fx.s.code
            .generate_commit_message(&pid, &ws.id)
            .await
            .unwrap_err();
    assert_eq!(e.message, "stage changes to generate a commit message");
    fx.s.code
        .write_file(&pid, &ws.id, "msg.txt", "x\n")
        .await
        .unwrap();
    assert!(
        fx.s.code
            .generate_commit_message(&pid, &ws.id)
            .await
            .is_err(),
        "unstaged only is still refused"
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn create_workspace_refuses_non_git() {
    let fx = Fx::new().await;
    let dir = fx.temp();
    let p = fx
        .store()
        .add_project(Project {
            id: "plain".into(),
            name: "plain".into(),
            repo_path: dir,
            base_branch: "main".into(),
            ..Default::default()
        })
        .unwrap();
    assert!(
        fx.s.workspaces
            .create_workspace(&p.id, "nope", "main", "agent", "", "", None)
            .await
            .is_err()
    );
    assert!(
        fx.s.workspaces
            .create_workspace(
                &p.id,
                "nope",
                "main",
                "loop",
                "do a thing",
                "balanced",
                None
            )
            .await
            .is_err()
    );
    assert!(fx.s.projects.list_branches(&p.id).await.unwrap().is_empty());
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn list_branches_for_path_no_phantom_preferred() {
    let fx = Fx::new().await;
    let dir = fx.temp();
    git(&dir, &["init", "-b", "master"]);
    write(format!("{dir}/README.md"), "hi\n");
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "initial"]);
    for preferred in ["", "main", "origin/main"] {
        let choices =
            fx.s.projects
                .list_branches_for_path(&dir, preferred)
                .await
                .unwrap();
        assert!(
            choices
                .iter()
                .all(|c| c.name != "main" && c.name != "origin/main"),
            "{preferred:?} injected a phantom into {choices:?}"
        );
        assert_eq!(choices[0].name, "master", "{choices:?}");
    }
    fx.close().await;
}

fn seed_upstream(fx: &Fx, branches: &[&str]) -> String {
    let bare = format!("{}/upstream.git", fx.temp());
    let seed = fx.temp();
    git(&seed, &["init", "-b", "main"]);
    write(format!("{seed}/README.md"), "hi\n");
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-m", "initial"]);
    for b in branches {
        git(&seed, &["branch", b]);
    }
    git(&seed, &["clone", "--bare", &seed, &bare]);
    let clone = format!("{}/work", fx.temp());
    git(&fx.temp(), &["clone", &bare, &clone]);
    clone
}

#[tokio::test(flavor = "multi_thread")]
async fn list_branches_for_path_works_without_project_id() {
    let fx = Fx::new().await;
    let clone = seed_upstream(&fx, &["develop"]);
    let choices =
        fx.s.projects
            .list_branches_for_path(&clone, "origin/main")
            .await
            .unwrap();
    let mut last = "";
    for c in &choices {
        assert!(
            !(last == LOCAL_BRANCH_GROUP && c.group != last),
            "Local must be last: {choices:?}"
        );
        last = &c.group;
    }
    assert!(
        choices
            .iter()
            .any(|c| c.name == "origin/develop" || c.name == "develop")
    );
    assert!(choices.iter().any(|c| c.name == "origin/main"));
    assert!(
        choices
            .iter()
            .any(|c| c.name == "main" && c.group == LOCAL_BRANCH_GROUP)
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn list_branches_missing_default_stays_in_group() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let p = fx
        .store()
        .add_project(Project {
            id: "missing-def".into(),
            name: "missing-def".into(),
            repo_path: repo,
            base_branch: "does-not-exist".into(),
            ..Default::default()
        })
        .unwrap();
    let choices = fx.s.projects.list_branches(&p.id).await.unwrap();
    let mut runs = 0;
    let mut prev = String::new();
    for c in &choices {
        if c.group == LOCAL_BRANCH_GROUP && prev != LOCAL_BRANCH_GROUP {
            runs += 1;
        }
        prev.clone_from(&c.group);
    }
    assert_eq!(runs, 1, "{choices:?}");
    assert!(
        choices
            .iter()
            .any(|c| c.name == "does-not-exist" && c.group == LOCAL_BRANCH_GROUP)
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn add_project_suggests_origin_main() {
    let fx = Fx::new().await;
    let clone = seed_upstream(&fx, &[]);
    let p =
        fx.s.projects
            .add_project(&Project {
                name: "suggest-base".into(),
                repo_path: clone,
                ..Default::default()
            })
            .await
            .unwrap();
    assert_eq!(p.base_branch, "origin/main");
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn list_branches_groups_remotes_first() {
    let fx = Fx::new().await;
    let clone = seed_upstream(&fx, &["feature/x"]);
    let p = fx
        .store()
        .add_project(Project {
            id: "remote-base".into(),
            name: "remote-base".into(),
            repo_path: clone,
            base_branch: "origin/main".into(),
            ..Default::default()
        })
        .unwrap();
    let choices = fx.s.projects.list_branches(&p.id).await.unwrap();
    assert!(choices.len() >= 2, "{choices:?}");
    assert_eq!(
        (choices[0].name.as_str(), choices[0].group.as_str()),
        ("origin/main", "origin")
    );
    let first_local = choices
        .iter()
        .position(|c| c.group == LOCAL_BRANCH_GROUP)
        .expect("a Local group");
    assert!(first_local > 0);
    assert!(
        choices[first_local..]
            .iter()
            .all(|c| c.group == LOCAL_BRANCH_GROUP)
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn create_solo_workspace_default_title() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let pid = fx.add_project(&repo);
    let ws =
        fx.s.workspaces
            .create_workspace(&pid, "", "", "agent", "", "", None)
            .await
            .unwrap();
    assert_eq!(
        (ws.goal.as_str(), ws.short.as_str()),
        ("New Workspace", "New Workspace")
    );
    assert!(!ws.has_run());
    let ws = fx.wait_ready(&ws.id).await;
    assert_eq!(ws.status_label, "Idle");
    fx.close().await;
}
