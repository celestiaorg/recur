//! Opening paths.

use recur_domain::{FILES_PROJECT_ID, Project};

use super::{Fx, read, write};
use crate::api::{EditorSessionMeta, EditorSessionPatch};

#[tokio::test(flavor = "multi_thread")]
async fn open_path_new_git_folder() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let w = &fx.s.workspaces;
    let res = w.open_path(&repo).await.unwrap();
    assert!(
        res.created && !res.workspace_id.is_empty() && !res.project_id.is_empty(),
        "{res:?}"
    );
    assert!(!res.loose && res.file.is_empty());
    assert!(res.workspace.as_ref().is_some_and(|w| w.is_main));
    let again = w.open_path(&repo).await.unwrap();
    assert!(!again.created);
    assert_eq!(
        (again.project_id, again.workspace_id),
        (res.project_id, res.workspace_id)
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn open_path_file_inside_project() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let pid = fx.add_project(&repo);
    let res =
        fx.s.workspaces
            .open_path(&format!("{repo}/README.md"))
            .await
            .unwrap();
    assert!(!res.created && res.project_id == pid, "{res:?}");
    assert_eq!(res.file, "README.md");
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn open_path_nested_folder_reuses_project() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    std::fs::create_dir(format!("{repo}/docs")).unwrap();
    let pid = fx.add_project(&repo);
    let res =
        fx.s.workspaces
            .open_path(&format!("{repo}/docs"))
            .await
            .unwrap();
    assert!(!res.created && res.project_id == pid, "{res:?}");
    assert_eq!(res.reveal, "docs");
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn open_path_adopts_git_toplevel() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    write(format!("{repo}/pkg/x.go"), "package pkg\n");
    let res =
        fx.s.workspaces
            .open_path(&format!("{repo}/pkg/x.go"))
            .await
            .unwrap();
    assert!(res.created, "an unknown git repo gets a project");
    let got = std::fs::canonicalize(&res.project.as_ref().unwrap().repo_path).unwrap();
    assert_eq!(got, std::fs::canonicalize(&repo).unwrap());
    assert_eq!(res.file, "pkg/x.go");
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn open_path_homeless_file() {
    let fx = Fx::new().await;
    let dir = fx.temp();
    let file = format!("{dir}/note.txt");
    write(&file, "hi\n");
    let res = fx.s.workspaces.open_path(&file).await.unwrap();
    assert!(res.loose && res.project_id == FILES_PROJECT_ID, "{res:?}");
    assert!(res.file.starts_with('/'), "{}", res.file);
    let code = &fx.s.code;
    let fc = code
        .read_file(&res.project_id, &res.workspace_id, &res.file)
        .await
        .unwrap();
    assert_eq!(fc.content, "hi\n");
    code.write_file(&res.project_id, &res.workspace_id, &res.file, "bye\n")
        .await
        .unwrap();
    assert_eq!(read(&file), "bye\n");
    assert!(
        code.read_file(
            &res.project_id,
            &res.workspace_id,
            &format!("{dir}/other.txt")
        )
        .await
        .is_err(),
        "a non-allowlisted abs path"
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn open_path_plain_folder() {
    let fx = Fx::new().await;
    let res = fx.s.workspaces.open_path(&fx.temp()).await.unwrap();
    assert!(
        res.created && !res.loose && res.project_id != FILES_PROJECT_ID,
        "{res:?}"
    );
    assert!(!res.project.is_some_and(|p| p.is_git));
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn open_path_loose_file_survives_restart() {
    let fx = Fx::new().await;
    let file = format!("{}/note.txt", fx.temp());
    write(&file, "hi\n");
    let res = fx.s.workspaces.open_path(&file).await.unwrap();
    fx.s.editor_session
        .update_editor_session(
            &res.workspace_id,
            &EditorSessionPatch {
                meta: Some(EditorSessionMeta {
                    open_files: Some(vec![res.file.clone()]),
                    selected_path: res.file.clone(),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let fresh = fx.reopen().await;
    let fc = fresh
        .services()
        .code
        .read_file(&res.project_id, &res.workspace_id, &res.file)
        .await
        .unwrap();
    assert_eq!(fc.content, "hi\n");
    fresh.shutdown().await;
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn add_project_reserves_files_id() {
    let fx = Fx::new().await;
    let p =
        fx.s.projects
            .add_project(&Project {
                name: "Files".into(),
                repo_path: fx.temp(),
                ..Default::default()
            })
            .await
            .unwrap();
    assert_ne!(p.id, FILES_PROJECT_ID);
    let file = format!("{}/note.txt", fx.temp());
    write(&file, "x\n");
    let res = fx.s.workspaces.open_path(&file).await.unwrap();
    assert_eq!(res.project_id, FILES_PROJECT_ID);
    fx.close().await;
}

fn flip_path_case(p: &str) -> String {
    let mut b = p.as_bytes().to_vec();
    if let Some(c) = b.iter_mut().rev().find(|c| c.is_ascii_alphabetic()) {
        *c ^= 0x20;
    }
    String::from_utf8(b).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn open_path_case_insensitive_reuse() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let w = &fx.s.workspaces;
    let res = w.open_path(&repo).await.unwrap();
    let alt = flip_path_case(&repo);
    if alt == repo || std::fs::metadata(&alt).is_err() {
        return fx.close().await;
    }
    let again = w.open_path(&alt).await.unwrap();
    assert!(
        !again.created && again.project_id == res.project_id,
        "{again:?}"
    );
    let file = format!("{alt}/README.md");
    if std::fs::metadata(&file).is_err() {
        return fx.close().await;
    }
    let got = w.open_path(&file).await.unwrap();
    assert!(!got.loose && got.file == "README.md", "{got:?}");
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn open_path_missing() {
    let fx = Fx::new().await;
    let missing = format!("{}/nope", fx.temp());
    assert!(fx.s.workspaces.open_path(&missing).await.is_err());
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn open_path_managed_worktree() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let pid = fx.add_project(&repo);
    let ws = fx.solo(&pid, "task", "main").await;
    write(format!("{}/extra.txt", ws.worktree_path), "x\n");
    let res =
        fx.s.workspaces
            .open_path(&format!("{}/extra.txt", ws.worktree_path))
            .await
            .unwrap();
    assert_eq!(
        (res.workspace_id.as_str(), res.file.as_str()),
        (ws.id.as_str(), "extra.txt")
    );
    fx.close().await;
}
