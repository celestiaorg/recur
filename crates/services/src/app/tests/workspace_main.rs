//! The main working copy.

use recur_domain::{
    Project, UPDATE_WORKSPACE_AUTOSTASH, UPDATE_WORKSPACE_NOOP, UPDATE_WORKSPACE_UPDATED,
    WORKSPACE_MAIN, Workspace,
};

use super::{Fx, git, git_out, read, write};

fn same(a: &str, b: &str) -> bool {
    std::fs::canonicalize(a).ok() == std::fs::canonicalize(b).ok()
}

#[tokio::test(flavor = "multi_thread")]
async fn open_main_workspace() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let pid = fx.add_project(&repo);
    let ws = fx.s.workspaces.open_main_workspace(&pid).await.unwrap();
    assert!(ws.is_main && ws.r#type == WORKSPACE_MAIN);
    assert!(!ws.has_run());
    assert!(same(&ws.worktree_path, &repo));
    assert!(ws.base_branch.is_empty());
    let ws2 = fx.s.workspaces.open_main_workspace(&pid).await.unwrap();
    assert_eq!(ws2.id, ws.id, "singleton per project");
    let mains =
        fx.s.workspaces
            .list_workspaces()
            .await
            .unwrap()
            .iter()
            .filter(|w| w.is_main && w.project_id == pid)
            .count();
    assert_eq!(mains, 1);
    fx.s.code
        .write_file(&pid, &ws.id, "main.txt", "hi\n")
        .await
        .unwrap();
    assert!(
        fx.s.code
            .commit(&pid, &ws.id, "main: add file")
            .await
            .unwrap()
            .committed
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn open_main_workspace_concurrent_singleton() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let pid = fx.add_project(&repo);
    let calls = (0..8).map(|_| fx.s.workspaces.open_main_workspace(&pid));
    let got = futures::future::join_all(calls).await;
    let ids: Vec<String> = got.into_iter().map(|r| r.unwrap().id).collect();
    assert!(ids.iter().all(|id| *id == ids[0]), "{ids:?}");
    let mains =
        fx.s.workspaces
            .list_workspaces()
            .await
            .unwrap()
            .iter()
            .filter(|w| w.is_main && w.project_id == pid)
            .count();
    assert_eq!(mains, 1);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn open_main_workspace_repairs_moved_repo() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let pid = fx.add_project(&repo);
    let ws = fx.s.workspaces.open_main_workspace(&pid).await.unwrap();
    let repo2 = fx.init_git_repo();
    let mut proj = fx.store().project(&pid).unwrap();
    proj.repo_path.clone_from(&repo2);
    fx.store().update_project_run_config(proj).unwrap();
    let ws2 = fx.s.workspaces.open_main_workspace(&pid).await.unwrap();
    assert_eq!(ws2.id, ws.id);
    assert!(same(&ws2.worktree_path, &repo2));
    assert!(same(
        &fx.store().workspace(&ws.id).unwrap().worktree_path,
        &repo2
    ));
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn main_workspace_refuses_loop_and_pr() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let pid = fx.add_project(&repo);
    let ws = fx.s.workspaces.open_main_workspace(&pid).await.unwrap();
    assert!(
        fx.svc
            .engine
            .start_run_on_workspace(&ws.id, "do a thing")
            .await
            .is_err()
    );
    assert!(
        fx.s.workspaces
            .open_workspace_pr(&ws.id, false)
            .await
            .is_err()
    );
    assert!(fx.s.workspaces.merge_workspace(&ws.id, "").await.is_err());
    assert!(
        fx.s.workspaces
            .update_workspace_with_base(&ws.id, "")
            .await
            .is_err()
    );
    assert!(fx.s.workspaces.remove_workspace(&ws.id).await.is_err());
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn staged_files_non_git_working_copy() {
    let fx = Fx::new().await;
    let dir = fx.temp();
    write(format!("{dir}/note.txt"), "hi\n");
    let p = fx
        .store()
        .add_project(Project {
            id: "plain-scm".into(),
            name: "plain-scm".into(),
            repo_path: dir,
            ..Default::default()
        })
        .unwrap();
    let ws = fx.s.workspaces.open_main_workspace(&p.id).await.unwrap();
    assert!(
        fx.s.code
            .staged_files(&p.id, &ws.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        fx.s.code
            .working_status(&p.id, &ws.id)
            .await
            .unwrap()
            .is_empty()
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn staged_files_non_writable_root() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    write(format!("{repo}/staged.txt"), "x\n");
    git(&repo, &["add", "staged.txt"]);
    let pid = fx.add_project(&repo);
    let staged_empty = |ws: &'static str| {
        let s = fx.s.clone();
        let pid = pid.clone();
        async move {
            let paths = s.code.staged_files(&pid, ws).await.unwrap();
            assert!(paths.is_empty(), "StagedFiles({ws:?}) = {paths:?}");
        }
    };
    let gs = fx.s.code.git_status(&pid, "").await.unwrap();
    assert!(gs.contains_key("staged.txt"), "setup: {gs:?}");
    staged_empty("").await;
    fx.store()
        .add_workspace(Workspace {
            id: "pending-wt".into(),
            project_id: pid.clone(),
            ..Default::default()
        })
        .unwrap();
    staged_empty("pending-wt").await;
    let gone = format!("{}/gone", fx.temp());
    fx.store()
        .add_workspace(Workspace {
            id: "stale-wt".into(),
            project_id: pid.clone(),
            worktree_path: gone,
            ..Default::default()
        })
        .unwrap();
    staged_empty("stale-wt").await;
    assert!(
        fx.s.code
            .working_status(&pid, "stale-wt")
            .await
            .unwrap()
            .is_empty()
    );
    let wd = fx.s.code.diff_working(&pid, "stale-wt").await.unwrap();
    assert!(wd.files.unwrap_or_default().is_empty());
    assert!(
        fx.s.code
            .git_status(&pid, "stale-wt")
            .await
            .unwrap()
            .is_empty()
    );
    let e = fx.s.code.list_tree(&pid, "stale-wt", "").await.unwrap_err();
    assert!(e.message.starts_with("worktree is not a directory"), "{e}");
    assert!(
        fx.s.code
            .stage(&pid, "pending-wt", &["README.md".into()])
            .await
            .is_err(),
        "mutations still refuse"
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn add_project_seeds_working_copy_from_checkout() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let p =
        fx.s.projects
            .add_project(&Project {
                name: "app".into(),
                repo_path: repo,
                base_branch: "origin/main".into(),
                ..Default::default()
            })
            .await
            .unwrap();
    let main = fx
        .store()
        .workspaces()
        .into_iter()
        .find(|w| w.is_main && w.project_id == p.id)
        .expect("a main working copy");
    assert_eq!(main.branch, "main");
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn open_main_workspace_non_git_clears_branch() {
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
    assert!(!fx.s.projects.get_project(&p.id).await.unwrap().is_git);
    let ws = fx.s.workspaces.open_main_workspace(&p.id).await.unwrap();
    assert!(ws.branch.is_empty());
    fx.store()
        .update_workspace(&ws.id, |m| m.branch = "main".into())
        .unwrap();
    let ws2 = fx.s.workspaces.open_main_workspace(&p.id).await.unwrap();
    assert!(ws2.branch.is_empty(), "stale branch cleared on reopen");
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_main_workspace_keeps_checkout() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let pid = fx.add_project(&repo);
    let ws = fx.s.workspaces.open_main_workspace(&pid).await.unwrap();
    fx.s.workspaces.delete_workspace(&ws.id).await.unwrap();
    assert_eq!(read(format!("{repo}/README.md")), "hi\n");
    fx.s.workspaces.open_main_workspace(&pid).await.unwrap();
    fx.close().await;
}

fn advance_origin(fx: &Fx, bare: &str, file: &str, body: &str, msg: &str) {
    let pusher = format!("{}/pusher", fx.temp());
    git(&fx.temp(), &["clone", bare, &pusher]);
    git(&pusher, &["config", "user.name", "test"]);
    git(&pusher, &["config", "user.email", "test@example.com"]);
    write(format!("{pusher}/{file}"), body);
    git(&pusher, &["add", file]);
    git(&pusher, &["commit", "-m", msg]);
    git(&pusher, &["push", "origin", "main"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn main_working_copy_land_and_remote() {
    let fx = Fx::new().await;
    let (clone, bare) = fx.init_git_repo_with_remote();
    let pid = fx.add_project(&clone);
    let w = &fx.s.workspaces;
    let ws = w.open_main_workspace(&pid).await.unwrap();
    let land = w.workspace_land_state(&ws.id).await.unwrap();
    assert!(!land.dirty && !land.unmerged && land.ahead_base == 0 && land.behind_base == 0);
    assert_eq!(
        (land.remote.as_str(), land.base.as_str()),
        ("origin", "origin/main")
    );

    write(format!("{clone}/dirty.txt"), "wip\n");
    assert!(w.workspace_land_state(&ws.id).await.unwrap().dirty);
    assert!(w.push_working_copy(&ws.id).await.is_err(), "nothing ahead");
    assert!(
        fx.s.code
            .commit(&pid, &ws.id, "main: dirty")
            .await
            .unwrap()
            .committed
    );
    write(format!("{clone}/leftover.txt"), "still here\n");
    let land = w.workspace_land_state(&ws.id).await.unwrap();
    assert!(land.dirty && land.ahead_base >= 1, "{land:?}");
    let pushed = w.push_working_copy(&ws.id).await.unwrap();
    assert!(pushed.is_main);
    let land = w.workspace_land_state(&ws.id).await.unwrap();
    assert_eq!(land.ahead_base, 0);
    assert!(land.dirty);
    assert_eq!(read(format!("{clone}/leftover.txt")), "still here\n");
    assert_eq!(
        git_out(&bare, &["show", "HEAD:dirty.txt"]).unwrap().trim(),
        "wip"
    );

    advance_origin(&fx, &bare, "from-origin.txt", "theirs\n", "origin advance");
    write(format!("{clone}/README.md"), "hello\nlocal edit\n");
    let land = w.fetch_working_copy_upstream(&ws.id).await.unwrap();
    assert!(land.behind_base >= 1 && land.dirty, "{land:?}");
    let res = w.update_working_copy_from_remote(&ws.id, "").await.unwrap();
    assert_eq!(res.status, UPDATE_WORKSPACE_UPDATED);
    assert_eq!(read(format!("{clone}/from-origin.txt")).trim(), "theirs");
    assert_eq!(
        read(format!("{clone}/README.md")).trim(),
        "hello\nlocal edit"
    );
    assert_eq!(read(format!("{clone}/leftover.txt")), "still here\n");
    let land = w.workspace_land_state(&ws.id).await.unwrap();
    assert_eq!(land.behind_base, 0);
    assert!(land.dirty);
    let noop = w.update_working_copy_from_remote(&ws.id, "").await.unwrap();
    assert_eq!(noop.status, UPDATE_WORKSPACE_NOOP);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn main_working_copy_update_conflict_aborts() {
    let fx = Fx::new().await;
    let (clone, bare) = fx.init_git_repo_with_remote();
    let pid = fx.add_project(&clone);
    let ws = fx.s.workspaces.open_main_workspace(&pid).await.unwrap();
    write(format!("{clone}/README.md"), "ours\n");
    assert!(
        fx.s.code
            .commit(&pid, &ws.id, "local readme")
            .await
            .unwrap()
            .committed
    );
    write(format!("{clone}/notes.txt"), "wip notes\n");
    advance_origin(&fx, &bare, "README.md", "theirs\n", "origin readme");
    let e =
        fx.s.workspaces
            .update_working_copy_from_remote(&ws.id, "")
            .await
            .unwrap_err();
    assert!(e.message.contains("left unchanged"), "{e}");
    assert_eq!(read(format!("{clone}/README.md")).trim(), "ours");
    assert_eq!(read(format!("{clone}/notes.txt")).trim(), "wip notes");
    assert!(
        !fx.s
            .workspaces
            .workspace_land_state(&ws.id)
            .await
            .unwrap()
            .unmerged
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn main_working_copy_update_refuses_in_progress() {
    let fx = Fx::new().await;
    let (clone, bare) = fx.init_git_repo_with_remote();
    let pid = fx.add_project(&clone);
    let ws = fx.s.workspaces.open_main_workspace(&pid).await.unwrap();
    write(format!("{clone}/README.md"), "ours\n");
    assert!(
        fx.s.code
            .commit(&pid, &ws.id, "local readme")
            .await
            .unwrap()
            .committed
    );
    advance_origin(&fx, &bare, "README.md", "theirs\n", "origin readme");
    git(&clone, &["fetch", "origin"]);
    let merged = std::process::Command::new("git")
        .args(["merge", "origin/main"])
        .current_dir(&clone)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .unwrap();
    assert!(!merged.status.success(), "expected a conflicting merge");
    write(format!("{clone}/README.md"), "partial resolution\n");
    let e =
        fx.s.workspaces
            .update_working_copy_from_remote(&ws.id, "")
            .await
            .unwrap_err();
    assert!(e.message.contains("already in progress"), "{e}");
    assert_eq!(
        read(format!("{clone}/README.md")).trim(),
        "partial resolution"
    );
    assert!(
        fx.s.workspaces
            .workspace_land_state(&ws.id)
            .await
            .unwrap()
            .unmerged
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn main_working_copy_autostash_left_in_stash() {
    let fx = Fx::new().await;
    let (clone, bare) = fx.init_git_repo_with_remote();
    let pid = fx.add_project(&clone);
    let ws = fx.s.workspaces.open_main_workspace(&pid).await.unwrap();
    write(format!("{clone}/README.md"), "hello\nlocal\n");
    advance_origin(&fx, &bare, "README.md", "hello\nremote\n", "origin readme");
    let res =
        fx.s.workspaces
            .update_working_copy_from_remote(&ws.id, "")
            .await
            .unwrap();
    assert_eq!(res.status, UPDATE_WORKSPACE_AUTOSTASH);
    assert_eq!(read(format!("{clone}/README.md")).trim(), "hello\nremote");
    let stash = git_out(&clone, &["stash", "list"]).unwrap();
    assert!(stash.to_lowercase().contains("autostash"), "{stash}");
    assert!(
        !fx.s
            .workspaces
            .workspace_land_state(&ws.id)
            .await
            .unwrap()
            .unmerged
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn main_working_copy_remote_refused_on_loop() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let pid = fx.add_project(&repo);
    let ws =
        fx.s.workspaces
            .create_workspace(&pid, "not main", "", "agent", "", "", None)
            .await
            .unwrap();
    fx.wait_ready(&ws.id).await;
    assert!(
        fx.s.workspaces
            .update_working_copy_from_remote(&ws.id, "")
            .await
            .is_err()
    );
    assert!(fx.s.workspaces.push_working_copy(&ws.id).await.is_err());
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn main_working_copy_land_no_remote() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let pid = fx.add_project(&repo);
    let ws = fx.s.workspaces.open_main_workspace(&pid).await.unwrap();
    let land = fx.s.workspaces.workspace_land_state(&ws.id).await.unwrap();
    assert!(land.remote.is_empty() && land.base.is_empty());
    assert!(!land.dirty);
    fx.close().await;
}
