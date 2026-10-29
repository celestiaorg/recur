//! Worktree watcher tests, including the fingerprint covering the branch.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::*;
use crate::testing::{TestEngine, add_project, git, init_repo, wait_for};

fn resolved_tempdir() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let p = std::fs::canonicalize(d.path()).unwrap();
    (d, p)
}

/// Skips when this environment cannot deliver file events (CI sandboxes).
async fn fs_events_delivered() -> bool {
    let (_d, dir) = resolved_tempdir();
    let Ok(mut s) = backend::start_fs_watch(&dir, Duration::from_millis(20)) else {
        return false;
    };
    std::fs::write(dir.join("probe.txt"), "x").unwrap();
    tokio::time::timeout(Duration::from_secs(2), s.recv())
        .await
        .is_ok_and(|b| b.is_some())
}

#[tokio::test]
async fn watch_fires_on_disk_change() {
    let t = TestEngine::new(None);
    t.engine
        .set_fs_timing(Duration::from_millis(20), Duration::from_millis(150));
    let repo = init_repo();
    add_project(&t.store, &repo.path);
    let ws = t
        .engine
        .create_workspace("proj", "watched", "")
        .await
        .unwrap();
    t.engine.watch_workspace(&ws.id);
    assert_eq!(t.engine.watched_workspace(), ws.id);
    assert!(t.events.has("workspace.watched"));
    tokio::time::sleep(Duration::from_millis(450)).await;
    assert!(
        !t.events.has("workspace.fs_changed"),
        "arming/baselining must not fire"
    );
    std::fs::write(Path::new(&ws.worktree_path).join("external.txt"), "hi").unwrap();
    wait_for(
        Duration::from_secs(3),
        "fs_changed after an external write",
        || t.events.has("workspace.fs_changed"),
    )
    .await;
    t.engine.watch_workspace("");
    assert_eq!(t.engine.watched_workspace(), "");
    t.close().await;
}

#[tokio::test]
async fn watch_fires_on_non_git_working_copy() {
    let t = TestEngine::new(None);
    t.engine
        .set_fs_timing(Duration::from_millis(20), Duration::from_millis(150));
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("readme.txt"), "v1\n").unwrap();
    add_project(&t.store, &dir.path().to_string_lossy());
    let ws = t.engine.open_main_workspace("proj").await.unwrap();
    assert_eq!(ws.worktree_path, dir.path().to_string_lossy());
    t.engine.watch_workspace(&ws.id);
    // Survive several polls: the old watcher died on the first git failure.
    tokio::time::sleep(Duration::from_millis(450)).await;
    assert!(!t.events.has("workspace.fs_changed"));
    std::fs::write(dir.path().join("readme.txt"), "v2 agent\n").unwrap();
    wait_for(
        Duration::from_secs(3),
        "fs_changed after modify in a plain folder",
        || t.events.has("workspace.fs_changed"),
    )
    .await;
    t.engine.watch_workspace("");
    t.close().await;
}

#[tokio::test]
async fn stream_beats_backstop() {
    if !fs_events_delivered().await {
        eprintln!("fs watch not delivering in this environment");
        return;
    }
    let t = TestEngine::new(None);
    t.engine
        .set_fs_timing(Duration::from_millis(20), Duration::from_secs(30));
    let repo = init_repo();
    add_project(&t.store, &repo.path);
    let ws = t
        .engine
        .create_workspace("proj", "watched", "")
        .await
        .unwrap();
    t.engine.watch_workspace(&ws.id);
    tokio::time::sleep(Duration::from_millis(120)).await;
    let before = t.events.count("workspace.fs_changed");
    std::fs::write(Path::new(&ws.worktree_path).join("external.txt"), "hi").unwrap();
    wait_for(
        Duration::from_secs(2),
        "fs_changed via the stream (backstop is 30s)",
        || t.events.count("workspace.fs_changed") > before,
    )
    .await;
    // The event names the changed path.
    let paths: Vec<String> = t
        .events
        .by_name("workspace.fs_changed")
        .into_iter()
        .filter_map(|e| match e {
            crate::bus::EngineEvent::WorkspaceFsChanged(f) => Some(f.paths),
            _ => None,
        })
        .flatten()
        .collect();
    assert!(paths.iter().any(|p| p == "external.txt"), "{paths:?}");
    t.engine.watch_workspace("");
    t.close().await;
}

#[tokio::test]
async fn idle_create_is_near_instant_at_production_latency() {
    if !fs_events_delivered().await {
        return;
    }
    let t = TestEngine::new(None);
    t.engine
        .set_fs_timing(Duration::from_millis(400), Duration::from_secs(30));
    let repo = init_repo();
    add_project(&t.store, &repo.path);
    let ws = t
        .engine
        .create_workspace("proj", "watched", "")
        .await
        .unwrap();
    t.engine.watch_workspace(&ws.id);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let before = t.events.count("workspace.fs_changed");
    std::fs::write(Path::new(&ws.worktree_path).join("external.txt"), "hi").unwrap();
    wait_for(Duration::from_secs(2), "fs_changed via the stream", || {
        t.events.count("workspace.fs_changed") > before
    })
    .await;
    t.engine.watch_workspace("");
    t.close().await;
}

#[tokio::test]
async fn sees_file_in_new_subdir() {
    if !fs_events_delivered().await {
        return;
    }
    let (_d, dir) = resolved_tempdir();
    let mut s = backend::start_fs_watch(&dir, Duration::from_millis(20)).unwrap();
    let sub = dir.join("a").join("b");
    std::fs::create_dir_all(&sub).unwrap();
    let want = sub.join("deep.txt");
    std::fs::write(&want, "x").unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let batch = tokio::time::timeout_at(deadline, s.recv())
            .await
            .unwrap_or_else(|_| panic!("no event for {}", want.display()))
            .expect("stream ended");
        if batch.iter().any(|e| e.path == want) {
            return;
        }
    }
}

#[tokio::test]
async fn follows_renamed_dir() {
    if !fs_events_delivered().await {
        return;
    }
    let (_d, dir) = resolved_tempdir();
    std::fs::create_dir_all(dir.join("src/old/deep")).unwrap();
    let mut s = backend::start_fs_watch(&dir, Duration::from_millis(20)).unwrap();
    let new_dir = dir.join("src/new");
    std::fs::rename(dir.join("src/old"), &new_dir).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut want = vec![new_dir.join("a.txt"), new_dir.join("deep").join("b.txt")];
    for p in &want {
        std::fs::write(p, "x").unwrap();
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while !want.is_empty() {
        let batch = tokio::time::timeout_at(deadline, s.recv())
            .await
            .unwrap_or_else(|_| panic!("missing events: {want:?}"))
            .expect("stream ended");
        want.retain(|p| !batch.iter().any(|e| &e.path == p));
    }
}

#[test]
fn head_change_matcher() {
    let root = Path::new("/repo");
    for (path, want) in [
        ("/repo/.git/HEAD", true),
        ("/repo/.git/index", false),
        ("/repo/.git/refs/heads/main", false),
        ("/repo/.git/HEAD.lock", false),
        ("/repo/HEAD", false),
        ("/repo/src/main.go", false),
        ("/repo/sub/.git/HEAD", false),
    ] {
        assert_eq!(fs_is_head_change(root, Path::new(path)), want, "{path}");
    }
}

#[test]
fn ignored_paths() {
    let root = Path::new("/repo");
    for (path, want) in [
        ("/repo/.git", true),
        ("/repo/.git/index", true),
        ("/repo/.DS_Store", true),
        ("/repo/a/.DS_Store", true),
        ("/repo/node_modules/x/y.js", true),
        ("/repo/web/node_modules", true),
        ("/repo/.github/workflows/ci.yml", false),
        ("/repo/src/a.go", false),
        ("/elsewhere/a.go", false),
    ] {
        assert_eq!(fs_ignored(root, Path::new(path)), want, "{path}");
    }
}

#[tokio::test]
async fn fingerprint_includes_branch() {
    let repo = init_repo();
    let root = tempfile::tempdir().unwrap();
    let m = recur_git::Manager::new(&repo.path, &root.path().to_string_lossy());
    let (wt, _) = m.add("LOOP-FP", "fp", "main").await.unwrap();
    let a = worktree_fingerprint(&wt.path, "")
        .await
        .expect("fingerprint");
    git(&wt.path, &["branch", "-m", "renamed-fp"]);
    let b = worktree_fingerprint(&wt.path, "")
        .await
        .expect("fingerprint after rename");
    assert_ne!(a, b, "fingerprint unchanged after branch rename");
}

async fn clone_of(origin: &str) -> (tempfile::TempDir, String) {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().to_string_lossy().into_owned();
    git(&p, &["clone", origin, "."]);
    (d, p)
}

#[tokio::test]
async fn fingerprint_sees_upstream_advance() {
    let origin = init_repo();
    let (_d, dir) = clone_of(&origin.path).await;
    let before = worktree_fingerprint(&dir, "").await.unwrap();
    git(&origin.path, &["commit", "--allow-empty", "-m", "ahead"]);
    git(&dir, &["fetch"]);
    assert_ne!(before, worktree_fingerprint(&dir, "").await.unwrap());
}

#[tokio::test]
async fn fingerprint_sees_base_advance() {
    let origin = init_repo();
    let (_d, dir) = clone_of(&origin.path).await;
    git(&dir, &["checkout", "-b", "feature"]);
    let before = worktree_fingerprint(&dir, "main").await.unwrap();
    git(&origin.path, &["commit", "--allow-empty", "-m", "ahead"]);
    git(&dir, &["fetch"]);
    assert_ne!(before, worktree_fingerprint(&dir, "main").await.unwrap());
}

#[tokio::test]
async fn fingerprint_sees_slashy_base_advance() {
    let origin = init_repo();
    git(&origin.path, &["checkout", "-b", "release/v1"]);
    let (_d, dir) = clone_of(&origin.path).await;
    git(&dir, &["checkout", "-b", "feature"]);
    let before = worktree_fingerprint(&dir, "release/v1").await.unwrap();
    git(&origin.path, &["commit", "--allow-empty", "-m", "ahead"]);
    git(&dir, &["fetch"]);
    assert_ne!(
        before,
        worktree_fingerprint(&dir, "release/v1").await.unwrap()
    );
}

#[tokio::test]
async fn fingerprint_sees_non_origin_base_advance() {
    let origin = init_repo();
    let (_d, dir) = clone_of(&origin.path).await;
    git(&dir, &["remote", "rename", "origin", "upstream"]);
    git(&dir, &["checkout", "-b", "feature"]);
    let before = worktree_fingerprint(&dir, "main").await.unwrap();
    git(&origin.path, &["commit", "--allow-empty", "-m", "ahead"]);
    git(&dir, &["fetch", "upstream"]);
    assert_ne!(before, worktree_fingerprint(&dir, "main").await.unwrap());
}

#[tokio::test]
async fn fingerprint_sees_first_commit() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().to_string_lossy().into_owned();
    git(&dir, &["init", "-b", "main"]);
    let before = worktree_fingerprint(&dir, "")
        .await
        .expect("an unborn HEAD still fingerprints");
    git(&dir, &["commit", "--allow-empty", "-m", "init"]);
    assert_ne!(before, worktree_fingerprint(&dir, "").await.unwrap());
}

#[tokio::test]
async fn fingerprint_ok_without_upstream_and_soft_fails_outside_git() {
    let repo = init_repo();
    git(&repo.path, &["checkout", "-b", "feature"]);
    assert!(worktree_fingerprint(&repo.path, "main").await.is_some());
    let d = tempfile::tempdir().unwrap();
    assert!(
        worktree_fingerprint(&d.path().to_string_lossy(), "")
            .await
            .is_none()
    );
}
