//! Workspace id, allocation, setup/teardown, diff sync, branch sync and
//! hand-workspace lifecycle tests. Cases that dispatch a loop (`dispatch`,
//! `StartRunOnWorkspace`, `BeginStartWorkspace*`) are in `loops::tests`; Open
//! PR after rename is in `prs::tests`.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use pretty_assertions::assert_eq;
use recur_domain::{
    Group, Pr, PrSource, Project, Session, WORKSPACE_SOLO, Workspace, WorkspaceState,
};
use regex::Regex;
use tokio_util::sync::CancellationToken;

use super::create::{CreatingAlloc, PHASE_CREATING, STATUS_CREATING};
use super::ids::*;
use crate::bus::EngineEvent;
use crate::loop_handle::{LoopHandle, LoopState};
use crate::loop_registry::RunningLoop;
use crate::testing::{TestEngine, add_project, add_run_project, git, git_out, init_repo, wait_for};

fn words() -> (Vec<String>, Vec<String>) {
    (
        KITCHEN_ADJECTIVES.iter().map(|s| (*s).to_owned()).collect(),
        KITCHEN_DISHES.iter().map(|s| (*s).to_owned()).collect(),
    )
}

fn run_project(t: &TestEngine, repo: &str, id: &str, f: impl FnOnce(&mut Project)) -> Project {
    let mut p = Project {
        id: id.into(),
        name: id.into(),
        repo_path: repo.into(),
        base_branch: "main".into(),
        ..Project::default()
    };
    f(&mut p);
    add_run_project(&t.store, p)
}

fn update_run_config(t: &TestEngine, p: &mut Project, f: impl FnOnce(&mut Project)) {
    f(p);
    p.write_project_file = true;
    *p = t
        .store
        .update_project_run_config(p.clone())
        .expect("update run config");
}

fn exists(p: &str) -> bool {
    Path::new(p).exists()
}

async fn branch_exists(repo: &str, branch: &str) -> bool {
    recur_git::Manager::new(repo, "")
        .commit_exists(branch)
        .await
}

// ---- workspace id

#[test]
fn alloc_kitchen_id_format() {
    let re = Regex::new(r"^[a-z]+-[a-z]+(-\d+)?$").unwrap();
    let (adj, dish) = words();
    let mut taken: HashSet<String> = HashSet::new();
    for _ in 0..200 {
        let id = alloc_kitchen_id(&adj, &dish, |s| taken.contains(s));
        assert!(re.is_match(&id), "{id} does not match adj-dish(-n)");
        let parts: Vec<&str> = id.split('-').collect();
        assert!(
            KITCHEN_ADJECTIVES.contains(&parts[0]),
            "unknown adjective in {id}"
        );
        assert!(KITCHEN_DISHES.contains(&parts[1]), "unknown dish in {id}");
        if parts.len() == 3 {
            assert!(parts[2].parse::<i64>().unwrap() >= 2);
        }
        taken.insert(id);
    }
}

#[test]
fn alloc_kitchen_id_collision_suffix() {
    let re = Regex::new(r"^[a-z]+-[a-z]+(-\d+)?$").unwrap();
    let (adj, dish) = words();
    let mut taken: HashSet<String> = HashSet::new();
    for a in KITCHEN_ADJECTIVES {
        for d in KITCHEN_DISHES {
            taken.insert(format!("{a}-{d}"));
        }
    }
    let id = alloc_kitchen_id(&adj, &dish, |s| taken.contains(s));
    assert!(
        id.ends_with("-2"),
        "expected -2 when all pairs are taken, got {id}"
    );
    assert!(re.is_match(&id));
    taken.insert(id);
    let id3 = alloc_kitchen_id(&adj, &dish, |s| taken.contains(s));
    let n: i64 = id3.rsplit('-').next().unwrap().parse().unwrap();
    assert!(n >= 2, "{id3}");
}

#[test]
fn compose_ids() {
    assert_eq!(
        compose_workspace_id("recur", "flaky-ramen"),
        "recur.flaky-ramen"
    );
    assert_eq!(compose_workspace_id("", "flaky-ramen"), "flaky-ramen");
    assert_eq!(compose_workspace_id("p", " "), "p");
}

#[test]
fn default_branch_prefix_constants_match() {
    assert_eq!(
        recur_git::DEFAULT_BRANCH_PREFIX,
        recur_domain::DEFAULT_BRANCH_NAME_PREFIX
    );
}

#[test]
fn repo_key_resolves_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let links = tempfile::tempdir().unwrap();
    let link = links.path().join("alias");
    std::os::unix::fs::symlink(dir.path(), &link).unwrap();
    assert_eq!(
        repo_key(&dir.path().to_string_lossy()),
        repo_key(&link.to_string_lossy())
    );
    assert_eq!(repo_key(""), ".");
}

#[tokio::test]
async fn new_workspace_id_uses_kitchen_names() {
    let t = TestEngine::new(None);
    let id = t.engine.new_workspace_id("demo", None).await;
    let re = Regex::new(r"^[a-z0-9-]+\.[a-z]+-[a-z]+(-\d+)?$").unwrap();
    assert!(re.is_match(&id), "{id}");
    assert!(id.starts_with("demo."));
    assert!(!id.to_uppercase().starts_with("LOOP-"));
}

#[tokio::test]
async fn new_workspace_id_avoids_existing_and_deleting() {
    let t = TestEngine::new(None);
    let mut seen = HashSet::new();
    for _ in 0..50 {
        let id = t.engine.new_workspace_id("demo", None).await;
        assert!(seen.insert(id.clone()), "duplicate workspace id {id}");
        t.store
            .add_workspace(Workspace {
                id,
                project_id: "p".into(),
                goal: "g".into(),
                r#type: WORKSPACE_SOLO.into(),
                ..Workspace::default()
            })
            .unwrap();
    }
    t.engine.set_kitchen_words(&["bubbly"], &["pretzel"]);
    assert!(t.engine.reserve_deleting("p.bubbly-pretzel"));
    assert_eq!(
        t.engine.new_workspace_id("p", None).await,
        "p.bubbly-pretzel-2"
    );
}

#[tokio::test]
async fn new_workspace_id_skips_existing_recur_branch() {
    let t = TestEngine::new(None);
    t.engine.set_kitchen_words(&["bubbly"], &["pretzel"]);
    let repo = init_repo();
    git(&repo.path, &["branch", "recur/bubbly-pretzel"]);
    let root = tempfile::tempdir().unwrap();
    let mgr = recur_git::Manager::new(&repo.path, &root.path().to_string_lossy());
    assert_eq!(
        t.engine.new_workspace_id("p", Some(&mgr)).await,
        "p.bubbly-pretzel-2"
    );
}

#[tokio::test]
async fn new_workspace_id_skips_existing_unprefixed_branch() {
    let t = TestEngine::new(None);
    t.engine.set_kitchen_words(&["bubbly"], &["pretzel"]);
    let repo = init_repo();
    git(&repo.path, &["branch", "bubbly-pretzel"]);
    t.store.set_settings(recur_domain::Settings {
        branch_name_prefix: Some(String::new()),
        ..recur_domain::Settings::default()
    });
    let root = tempfile::tempdir().unwrap();
    let mgr =
        recur_git::Manager::new(&repo.path, &root.path().to_string_lossy()).with_branch_prefix("");
    assert_eq!(
        t.engine.new_workspace_id("p", Some(&mgr)).await,
        "p.bubbly-pretzel-2"
    );
}

#[tokio::test]
async fn create_workspace_branch_is_recur_workspace_id() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    run_project(&t, &repo.path, "p-branch", |_| {});
    let ws = t
        .engine
        .create_workspace("p-branch", "Auth refresh", "")
        .await
        .unwrap();
    assert_eq!(
        ws.branch,
        format!("recur/{}", recur_git::workspace_slug(&ws.id))
    );
    assert!(ws.id.starts_with("p-branch."));
    assert_eq!(
        ws.worktree_path,
        format!(
            "{}/p-branch/{}",
            t.engine.worktree_root(),
            recur_git::workspace_slug(&ws.id)
        )
    );
    assert_eq!(ws.phase, "editing");
    assert_eq!(ws.status_label, "Idle");
    assert!(ws.port > 0);
    t.close().await;
}

#[tokio::test]
async fn create_workspace_honors_branch_prefix_setting() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    run_project(&t, &repo.path, "p-pfx", |_| {});
    t.store.set_settings(recur_domain::Settings {
        branch_name_prefix: Some("josh".into()),
        ..recur_domain::Settings::default()
    });
    let ws = t
        .engine
        .create_workspace("p-pfx", "custom prefix", "")
        .await
        .unwrap();
    assert_eq!(
        ws.branch,
        format!("josh/{}", recur_git::workspace_slug(&ws.id))
    );
    t.store.set_settings(recur_domain::Settings {
        branch_name_prefix: Some(String::new()),
        ..recur_domain::Settings::default()
    });
    let none = t
        .engine
        .create_workspace("p-pfx", "no prefix", "")
        .await
        .unwrap();
    assert_eq!(none.branch, recur_git::workspace_slug(&none.id));
    t.close().await;
}

#[tokio::test]
async fn create_workspace_does_not_reuse_leftover_branch() {
    let t = TestEngine::new(None);
    t.engine.set_kitchen_words(&["bubbly"], &["pretzel"]);
    let repo = init_repo();
    run_project(&t, &repo.path, "p-leftover", |_| {});
    let first = t
        .engine
        .create_workspace("p-leftover", "old work", "")
        .await
        .unwrap();
    assert_eq!(first.id, "p-leftover.bubbly-pretzel");
    std::fs::write(
        Path::new(&first.worktree_path).join("stale.txt"),
        "old work\n",
    )
    .unwrap();
    git(&first.worktree_path, &["add", "stale.txt"]);
    git(&first.worktree_path, &["commit", "-m", "stale leftover"]);
    t.engine.delete_workspace(&first.id).await.unwrap();
    // The id stays reserved until the background teardown is done.
    wait_for(Duration::from_secs(5), "delete teardown", || {
        !t.engine.is_deleting(&first.id)
    })
    .await;

    let second = t
        .engine
        .create_workspace("p-leftover", "fresh work", "")
        .await
        .unwrap();
    assert_eq!(
        second.id, "p-leftover.bubbly-pretzel-2",
        "a leftover branch must not recycle the id"
    );
    assert_eq!(second.branch, "recur/bubbly-pretzel-2");
    assert!(!Path::new(&second.worktree_path).join("stale.txt").exists());
    t.close().await;
}

#[tokio::test]
async fn create_workspace_nests_under_project() {
    let t = TestEngine::new(None);
    t.engine.set_kitchen_words(&["bubbly"], &["pretzel"]);
    let repo = init_repo();
    run_project(&t, &repo.path, "kitchen", |_| {});
    let ws = t
        .engine
        .create_workspace("kitchen", "nested", "")
        .await
        .unwrap();
    assert_eq!(ws.id, "kitchen.bubbly-pretzel");
    let want = format!("{}/kitchen/bubbly-pretzel", t.engine.worktree_root());
    assert_eq!(ws.worktree_path, want);
    assert!(exists(&want));
    t.close().await;
}

#[tokio::test]
async fn two_projects_share_kitchen_slug() {
    let t = TestEngine::new(None);
    t.engine.set_kitchen_words(&["bubbly"], &["pretzel"]);
    let (r1, r2) = (init_repo(), init_repo());
    run_project(&t, &r1.path, "alpha", |_| {});
    run_project(&t, &r2.path, "beta", |_| {});
    let a = t.engine.create_workspace("alpha", "one", "").await.unwrap();
    let b = t.engine.create_workspace("beta", "two", "").await.unwrap();
    assert_eq!(
        (a.id.as_str(), b.id.as_str()),
        ("alpha.bubbly-pretzel", "beta.bubbly-pretzel")
    );
    assert_eq!(a.branch, "recur/bubbly-pretzel");
    assert_eq!(b.branch, "recur/bubbly-pretzel");
    t.close().await;
}

#[tokio::test]
async fn same_repo_projects_do_not_reuse_kitchen_slug() {
    let t = TestEngine::new(None);
    t.engine.set_kitchen_words(&["bubbly"], &["pretzel"]);
    let repo = init_repo();
    run_project(&t, &repo.path, "alpha", |_| {});
    run_project(&t, &repo.path, "beta", |_| {});
    // Persist before allocate: the first tree is not on disk yet, so the
    // store + same-repo scan must refuse the slug.
    let a = t
        .engine
        .begin_create_workspace("alpha", "one", "")
        .await
        .unwrap();
    let b = t
        .engine
        .begin_create_workspace("beta", "two", "")
        .await
        .unwrap();
    assert_eq!(a.id, "alpha.bubbly-pretzel");
    assert_eq!(b.id, "beta.bubbly-pretzel-2");
    assert_eq!(
        (a.branch.as_str(), b.branch.as_str()),
        ("recur/bubbly-pretzel", "recur/bubbly-pretzel-2")
    );
    wait_for(Duration::from_secs(5), "both allocate", || {
        !t.engine.creating_in_flight(&a.id) && !t.engine.creating_in_flight(&b.id)
    })
    .await;
    t.close().await;
}

#[tokio::test]
async fn same_repo_symlink_paths_do_not_reuse_kitchen_slug() {
    let t = TestEngine::new(None);
    t.engine.set_kitchen_words(&["bubbly"], &["pretzel"]);
    let repo = init_repo();
    let links = tempfile::tempdir().unwrap();
    let link = links.path().join("alias");
    std::os::unix::fs::symlink(&repo.path, &link).unwrap();
    run_project(&t, &repo.path, "alpha", |_| {});
    run_project(&t, &link.to_string_lossy(), "beta", |_| {});
    let a = t
        .engine
        .begin_create_workspace("alpha", "one", "")
        .await
        .unwrap();
    let b = t
        .engine
        .begin_create_workspace("beta", "two", "")
        .await
        .unwrap();
    assert_eq!(a.id, "alpha.bubbly-pretzel");
    assert_eq!(
        b.id, "beta.bubbly-pretzel-2",
        "a symlink alias is the same repo"
    );
    wait_for(Duration::from_secs(5), "both allocate", || {
        !t.engine.creating_in_flight(&a.id) && !t.engine.creating_in_flight(&b.id)
    })
    .await;
    t.close().await;
}

#[tokio::test]
async fn create_refuses_non_git_and_unknown_projects() {
    let t = TestEngine::new(None);
    let err = t
        .engine
        .create_workspace("nope", "x", "")
        .await
        .unwrap_err();
    assert_eq!(err.0, "project \"nope\" not found");
    let dir = tempfile::tempdir().unwrap();
    run_project(&t, &dir.path().to_string_lossy(), "plain", |_| {});
    let err = t
        .engine
        .create_workspace("plain", "x", "")
        .await
        .unwrap_err();
    assert_eq!(
        err.0,
        "project \"plain\" is not a git repository: workspaces need a git checkout"
    );
    t.store
        .add_project(Project {
            id: "empty".into(),
            ..Project::default()
        })
        .unwrap();
    let err = t
        .engine
        .create_workspace("empty", "x", "")
        .await
        .unwrap_err();
    assert_eq!(err.0, "project \"empty\" has no repo path");
}

// ---- workspace create (allocate)

#[tokio::test]
async fn begin_create_returns_before_worktree() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    let marker = t.data_dir.join("setup.slow");
    run_project(&t, &repo.path, "p1", |p| {
        p.setup_cmd = format!("sleep 0.4 && echo ok > '{}'", marker.display());
    });
    let ws = t
        .engine
        .begin_create_workspace("p1", "eager", "main")
        .await
        .unwrap();
    assert_eq!(ws.worktree_path, "", "returns before the worktree exists");
    assert_eq!(ws.phase, PHASE_CREATING);
    assert_eq!(ws.status_label, STATUS_CREATING);
    assert!(
        !marker.exists(),
        "setup finished before BeginCreate returned"
    );
    wait_for(Duration::from_secs(5), "worktree allocate", || {
        t.store
            .workspace(&ws.id)
            .is_some_and(|w| !w.worktree_path.is_empty() && w.phase == "editing")
    })
    .await;
    assert!(marker.exists(), "setup did not run");
    assert!(t.store.workspace(&ws.id).unwrap().port > 0);
    t.close().await;
}

#[tokio::test]
async fn allocate_publishes_worktree_before_setup() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    let start = t.data_dir.join("setup.start");
    let done = t.data_dir.join("setup.done");
    run_project(&t, &repo.path, "p1", |p| {
        p.setup_cmd = format!(
            "while [ ! -f '{}' ]; do sleep 0.05; done; echo ok > '{}'",
            start.display(),
            done.display()
        );
    });
    let ws = t
        .engine
        .begin_create_workspace("p1", "eager-paint", "main")
        .await
        .unwrap();
    let id = ws.id.clone();
    wait_for(Duration::from_secs(5), "worktree path published before setup", || {
        let Some(got) = t.store.workspace(&id) else {
            return false;
        };
        if got.worktree_path.is_empty() || got.phase != PHASE_CREATING {
            return false;
        }
        t.events.by_name("workspace.updated").iter().any(|e| {
            matches!(e, EngineEvent::WorkspaceUpdated(u) if u.workspace_id == id && !u.worktree_path.is_empty())
        })
    })
    .await;
    assert!(
        !done.exists(),
        "setup finished before worktreePath was published"
    );
    std::fs::write(&start, "go").unwrap();
    wait_for(Duration::from_secs(5), "setup complete", || {
        t.store.workspace(&id).is_some_and(|w| w.phase == "editing")
    })
    .await;
    assert!(done.exists());
    t.close().await;
}

#[tokio::test]
async fn failed_setup_keeps_worktree_after_eager_publish() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    let start = t.data_dir.join("setup.start");
    run_project(&t, &repo.path, "p1", |p| {
        p.setup_cmd = format!(
            "while [ ! -f '{}' ]; do sleep 0.05; done; exit 1",
            start.display()
        );
    });
    let ws = t
        .engine
        .begin_create_workspace("p1", "keep-on-fail", "main")
        .await
        .unwrap();
    wait_for(Duration::from_secs(5), "worktree path before setup", || {
        t.store
            .workspace(&ws.id)
            .is_some_and(|w| !w.worktree_path.is_empty() && w.phase == PHASE_CREATING)
    })
    .await;
    let got = t.store.workspace(&ws.id).unwrap();
    let user_file = Path::new(&got.worktree_path).join("user-edit.txt");
    std::fs::write(&user_file, "keep me").unwrap();
    std::fs::write(&start, "go").unwrap();
    wait_for(Duration::from_secs(5), "create failure", || {
        t.store
            .workspace(&ws.id)
            .is_some_and(|w| w.phase == "failed")
    })
    .await;
    let got = t.store.workspace(&ws.id).unwrap();
    assert!(
        !got.worktree_path.is_empty(),
        "failed setup cleared WorktreePath"
    );
    assert!(!got.last_error.is_empty());
    assert_eq!(got.status_label, "Couldn't create");
    assert_eq!(std::fs::read_to_string(&user_file).unwrap(), "keep me");
    t.close().await;
}

#[tokio::test]
async fn delete_workspace_cancels_in_flight_create() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    let marker = t.data_dir.join("setup.should-not");
    run_project(&t, &repo.path, "p1", |p| {
        p.setup_cmd = format!("sleep 2 && echo no > '{}'", marker.display());
    });
    let ws = t
        .engine
        .begin_create_workspace("p1", "discard-me", "main")
        .await
        .unwrap();
    t.engine.delete_workspace(&ws.id).await.unwrap();
    assert!(t.store.workspace(&ws.id).is_none());
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!marker.exists(), "setup continued after delete");
    t.close().await;
}

#[tokio::test]
async fn delete_during_create_does_not_teardown_main_repo() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    let torn = Path::new(&repo.path).join("teardown.marker");
    run_project(&t, &repo.path, "p1", |p| {
        p.setup_cmd = "sleep 2".into();
        p.teardown_cmd = "echo torn > teardown.marker".into();
    });
    let ws = t
        .engine
        .begin_create_workspace("p1", "cancel-me", "main")
        .await
        .unwrap();
    t.engine.delete_workspace(&ws.id).await.unwrap();
    t.engine.shutdown().await;
    assert!(
        !torn.exists(),
        "teardown ran in the project's real checkout"
    );
}

#[tokio::test]
async fn remove_cancels_in_flight_create() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    let marker = t.data_dir.join("setup.after-remove");
    run_project(&t, &repo.path, "p1", |p| {
        p.setup_cmd = format!("sleep 2 && echo no > '{}'", marker.display());
    });
    let ws = t
        .engine
        .begin_create_workspace("p1", "remove-me", "main")
        .await
        .unwrap();
    let removed = t.engine.remove_workspace(&ws.id).await.unwrap();
    assert!(removed.archived);
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(!marker.exists(), "setup continued after remove");
    let got = t.store.workspace(&ws.id).unwrap();
    assert!(
        got.phase != "editing" || got.worktree_path.is_empty(),
        "allocate finished after remove and restored the worktree"
    );
    t.close().await;
}

#[tokio::test]
async fn retry_workspace_setup_after_failed_create() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    let marker = t.data_dir.join("setup.retry");
    let mut proj = run_project(&t, &repo.path, "p1", |p| p.setup_cmd = "exit 1".into());
    let ws = t
        .engine
        .begin_create_workspace("p1", "retry-me", "main")
        .await
        .unwrap();
    wait_for(Duration::from_secs(5), "create failure", || {
        t.store
            .workspace(&ws.id)
            .is_some_and(|w| w.phase == "failed" && !w.worktree_path.is_empty())
    })
    .await;
    let kept = t.store.workspace(&ws.id).unwrap().worktree_path;
    update_run_config(&t, &mut proj, |p| {
        p.setup_cmd = format!("echo ok > '{}'", marker.display());
    });
    t.engine.retry_workspace_setup(&ws.id).await.unwrap();
    wait_for(Duration::from_secs(5), "retry setup complete", || {
        t.store
            .workspace(&ws.id)
            .is_some_and(|w| w.phase == "editing")
    })
    .await;
    let got = t.store.workspace(&ws.id).unwrap();
    assert_eq!(got.last_error, "");
    assert_eq!(got.worktree_path, kept);
    assert!(marker.exists(), "retry setup did not run");
    t.close().await;
}

#[tokio::test]
async fn retry_workspace_setup_rejections() {
    let t = TestEngine::new(None);
    let failed = |id: &str, last_error: &str, tree: &str| Workspace {
        id: id.into(),
        project_id: "p1".into(),
        r#type: WORKSPACE_SOLO.into(),
        phase: "failed".into(),
        status_label: "Couldn't create".into(),
        last_error: last_error.into(),
        worktree_path: tree.into(),
        ..Workspace::default()
    };
    t.store
        .add_workspace(failed(
            "ws-no-tree",
            "setup command failed: exit status 23",
            "",
        ))
        .unwrap();
    let err = t
        .engine
        .retry_workspace_setup("ws-no-tree")
        .await
        .unwrap_err();
    assert!(err.0.contains("the worktree was removed"), "{err}");
    let got = t.store.workspace("ws-no-tree").unwrap();
    assert_eq!(
        (got.phase.as_str(), got.last_error.as_str()),
        ("failed", "setup command failed: exit status 23")
    );

    t.store
        .add_project(Project {
            id: "p1".into(),
            name: "p1".into(),
            ..Project::default()
        })
        .unwrap();
    let tree = tempfile::tempdir().unwrap();
    let tree_s = tree.path().to_string_lossy().into_owned();
    t.store
        .add_workspace(failed(
            "ws-inflight",
            "setup command failed: exit status 1",
            &tree_s,
        ))
        .unwrap();
    let done = CancellationToken::new();
    done.cancel();
    t.engine.creating.lock().insert(
        "ws-inflight".into(),
        Arc::new(CreatingAlloc {
            cancel: CancellationToken::new(),
            done,
        }),
    );
    let err = t
        .engine
        .retry_workspace_setup("ws-inflight")
        .await
        .unwrap_err();
    assert!(err.0.contains("already running"), "{err}");
    let got = t.store.workspace("ws-inflight").unwrap();
    assert_eq!(
        got.last_error, "setup command failed: exit status 1",
        "row mutated"
    );

    t.store
        .add_workspace(failed(
            "ws-copy",
            "copy files: glob matched a directory",
            &tree_s,
        ))
        .unwrap();
    let err = t.engine.retry_workspace_setup("ws-copy").await.unwrap_err();
    assert!(err.0.contains("did not fail setup"), "{err}");

    t.store
        .add_workspace(Workspace {
            id: "ws-main".into(),
            is_main: true,
            ..Workspace::default()
        })
        .unwrap();
    assert_eq!(
        t.engine
            .retry_workspace_setup("ws-main")
            .await
            .unwrap_err()
            .0,
        "retry setup only applies to a hand workspace"
    );
    t.store
        .add_workspace(Workspace {
            id: "ws-run".into(),
            run_started: true,
            ..Workspace::default()
        })
        .unwrap();
    assert_eq!(
        t.engine
            .retry_workspace_setup("ws-run")
            .await
            .unwrap_err()
            .0,
        "retry setup only applies to a workspace that never started a workflow"
    );
    assert_eq!(
        t.engine
            .retry_workspace_setup("missing")
            .await
            .unwrap_err()
            .0,
        "workspace \"missing\" not found"
    );
}

#[tokio::test]
async fn retry_workspace_setup_finishes_pending_fork() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    let marker = t.data_dir.join("setup.retry-fork");
    let mut proj = run_project(&t, &repo.path, "p1", |_| {});
    let src = t
        .engine
        .create_workspace("p1", "src", "main")
        .await
        .unwrap();
    std::fs::write(
        Path::new(&src.worktree_path).join("dirty.txt"),
        "uncommitted\n",
    )
    .unwrap();
    update_run_config(&t, &mut proj, |p| p.setup_cmd = "exit 1".into());
    let dest = t
        .engine
        .begin_create_workspace("p1", "fork-dest", &src.branch)
        .await
        .unwrap();
    wait_for(Duration::from_secs(5), "create failure", || {
        t.store
            .workspace(&dest.id)
            .is_some_and(|w| w.phase == "failed" && !w.worktree_path.is_empty())
    })
    .await;
    assert_eq!(t.store.workspace(&dest.id).unwrap().base_branch, src.branch);
    t.engine
        .fork_overlay
        .lock()
        .insert(dest.id.clone(), src.clone());
    update_run_config(&t, &mut proj, |p| {
        p.setup_cmd = format!("echo ok > '{}'", marker.display());
    });
    t.engine.retry_workspace_setup(&dest.id).await.unwrap();
    wait_for(Duration::from_secs(5), "retry setup complete", || {
        t.store
            .workspace(&dest.id)
            .is_some_and(|w| w.phase == "editing")
    })
    .await;
    let got = t.store.workspace(&dest.id).unwrap();
    assert_eq!(
        got.base_branch, src.base_branch,
        "the source merge-base wins"
    );
    assert_eq!(
        std::fs::read_to_string(Path::new(&got.worktree_path).join("dirty.txt")).unwrap(),
        "uncommitted\n"
    );
    assert!(marker.exists());
    t.close().await;
}

#[tokio::test]
async fn fail_creating_keeps_worktree_path_and_fails_run() {
    let t = TestEngine::new(None);
    t.store
        .add_workspace(Workspace {
            id: "ws-fail".into(),
            project_id: "p1".into(),
            run_started: true,
            state: WorkspaceState::Queued,
            worktree_path: "/tmp/recur-kept-worktree".into(),
            phase: "creating".into(),
            ..Workspace::default()
        })
        .unwrap();
    t.engine.fail_creating("ws-fail", "dispatch failed");
    let got = t.store.workspace("ws-fail").unwrap();
    assert_eq!(got.worktree_path, "/tmp/recur-kept-worktree");
    assert_eq!(got.state, WorkspaceState::Failed);
    assert_eq!(got.last_error, "dispatch failed");
    t.engine
        .check_loop_settled("ws-fail", &got)
        .expect("a failed create is deletable");
}

#[tokio::test]
async fn allocate_copies_configured_files_before_setup() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    std::fs::create_dir_all(Path::new(&repo.path).join("apps/web")).unwrap();
    std::fs::write(Path::new(&repo.path).join(".env"), "SECRET=1\n").unwrap();
    std::fs::write(Path::new(&repo.path).join("apps/web/.env.local"), "X=2\n").unwrap();
    let marker = t.data_dir.join("setup.saw.env");
    run_project(&t, &repo.path, "p1", |p| {
        p.copy_files = ".env*\n".into();
        p.setup_cmd = format!(
            "test -f .env && test -f apps/web/.env.local && echo ok > '{}'",
            marker.display()
        );
    });
    let ws = t
        .engine
        .create_workspace("p1", "copy-files", "main")
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(Path::new(&ws.worktree_path).join(".env")).unwrap(),
        "SECRET=1\n"
    );
    assert!(
        Path::new(&ws.worktree_path)
            .join("apps/web/.env.local")
            .exists()
    );
    assert!(marker.exists(), "setup did not see the copied files");
    t.close().await;
}

#[tokio::test]
async fn remove_workspace_refuses_in_flight_chat() {
    let t = TestEngine::new(None);
    t.store
        .add_workspace(Workspace {
            id: "ws-chat".into(),
            project_id: "p1".into(),
            r#type: WORKSPACE_SOLO.into(),
            short: "hand".into(),
            ..Workspace::default()
        })
        .unwrap();
    t.engine
        .register_chat_turn("ws-chat", "PA", CancellationToken::new())
        .unwrap();
    let err = t.engine.remove_workspace("ws-chat").await.unwrap_err();
    assert!(err.0.contains("running"), "{err}");
    assert!(!t.store.workspace("ws-chat").unwrap().archived);
    assert!(
        !t.engine.is_deleting("ws-chat"),
        "a refusal releases the reservation"
    );
}

// ---- setup teardown

#[tokio::test]
async fn create_workspace_runs_setup_once() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    let marker = t.data_dir.join("setup.ok");
    let mut proj = run_project(&t, &repo.path, "p1", |p| {
        p.setup_cmd = format!("echo hi > '{}'", marker.display());
    });
    let ws = t
        .engine
        .create_workspace("p1", "feature", "main")
        .await
        .unwrap();
    assert!(ws.port > 0);
    assert!(!ws.worktree_path.is_empty());
    assert!(marker.exists());
    let feed = t.store.feed(&ws.id).unwrap_or_default();
    assert!(feed.iter().any(|f| f.text == "Setup command finished."));

    let marker2 = t.data_dir.join("setup2.ok");
    update_run_config(&t, &mut proj, |p| {
        p.setup_cmd = format!("echo hi > '{}'", marker2.display());
    });
    let ws2 = t
        .engine
        .create_workspace("p1", "other", "main")
        .await
        .unwrap();
    assert_ne!(ws2.port, ws.port, "distinct ports");
    assert!(marker2.exists());
    t.close().await;
}

#[tokio::test]
async fn remove_runs_teardown_after_returning() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    let gate = t.data_dir.join("teardown.gate");
    let torn = t.data_dir.join("torn.down");
    run_project(&t, &repo.path, "p1", |p| {
        p.teardown_cmd = format!(
            "while [ ! -f '{}' ]; do sleep 0.01; done && echo bye > '{}'",
            gate.display(),
            torn.display()
        );
    });
    let ws = t
        .engine
        .create_workspace("p1", "feature", "main")
        .await
        .unwrap();
    let removed = t.engine.remove_workspace(&ws.id).await.unwrap();
    assert!(removed.archived && removed.worktree_path.is_empty() && removed.port == 0);
    assert!(
        exists(&ws.worktree_path),
        "worktree stays until teardown is released"
    );
    assert!(!torn.exists());
    assert!(t.events.has("workspace.removed"));
    std::fs::write(&gate, "go\n").unwrap();
    wait_for(Duration::from_secs(5), "teardown command", || torn.exists()).await;
    wait_for(Duration::from_secs(5), "worktree removed", || {
        !exists(&ws.worktree_path)
    })
    .await;
    let list = git_out(&repo.path, &["worktree", "list", "--porcelain"]).unwrap();
    assert!(
        !list.contains(&ws.worktree_path),
        "git still lists the worktree:\n{list}"
    );
    t.close().await;
}

#[tokio::test]
async fn delete_runs_teardown_after_returning() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    let gate = t.data_dir.join("teardown.gate");
    let torn = t.data_dir.join("torn.down");
    run_project(&t, &repo.path, "p1", |p| {
        p.teardown_cmd = format!(
            "while [ ! -f '{}' ]; do sleep 0.01; done && echo bye > '{}'",
            gate.display(),
            torn.display()
        );
    });
    let ws = t
        .engine
        .create_workspace("p1", "feature", "main")
        .await
        .unwrap();
    std::fs::create_dir_all(t.data_dir.join("workspaces").join(&ws.id)).unwrap();
    t.engine.delete_workspace(&ws.id).await.unwrap();
    assert!(
        t.store.workspace(&ws.id).is_none(),
        "row gone before disk teardown"
    );
    assert!(t.events.has("workspace.deleted"));
    assert!(exists(&ws.worktree_path));
    assert!(!torn.exists());
    std::fs::write(&gate, "go\n").unwrap();
    wait_for(Duration::from_secs(5), "teardown command", || torn.exists()).await;
    wait_for(Duration::from_secs(5), "worktree removed", || {
        !exists(&ws.worktree_path)
    })
    .await;
    wait_for(Duration::from_secs(5), "data dir removed", || {
        !t.data_dir.join("workspaces").join(&ws.id).exists()
    })
    .await;
    t.close().await;
}

#[tokio::test]
async fn create_failed_setup_deletes_empty_branch() {
    let t = TestEngine::new(None);
    t.engine.set_kitchen_words(&["bubbly"], &["pretzel"]);
    let repo = init_repo();
    let mut proj = run_project(&t, &repo.path, "p-setup-fail", |p| {
        p.setup_cmd = "exit 1".into()
    });
    assert!(
        t.engine
            .create_workspace("p-setup-fail", "fail", "main")
            .await
            .is_err()
    );
    assert!(
        t.store.workspace("p-setup-fail.bubbly-pretzel").is_none(),
        "rolled back"
    );
    assert!(
        !branch_exists(&repo.path, "recur/bubbly-pretzel").await,
        "empty branch deleted"
    );
    update_run_config(&t, &mut proj, |p| p.setup_cmd.clear());
    let ws = t
        .engine
        .create_workspace("p-setup-fail", "retry", "main")
        .await
        .unwrap();
    assert_eq!(ws.id, "p-setup-fail.bubbly-pretzel");
    assert_eq!(ws.branch, "recur/bubbly-pretzel");
    t.close().await;
}

#[tokio::test]
async fn failed_setup_last_error_includes_stderr() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    run_project(&t, &repo.path, "p-out", |p| {
        p.setup_cmd = "echo 'missing tool: foo' >&2; exit 23".into();
    });
    let ws = t
        .engine
        .begin_create_workspace("p-out", "fail", "main")
        .await
        .unwrap();
    wait_for(Duration::from_secs(5), "create failure", || {
        t.store
            .workspace(&ws.id)
            .is_some_and(|w| w.phase == "failed")
    })
    .await;
    let got = t.store.workspace(&ws.id).unwrap();
    assert!(
        got.last_error.contains("exit status 23"),
        "{}",
        got.last_error
    );
    assert!(
        got.last_error.contains("missing tool: foo"),
        "{}",
        got.last_error
    );
    assert!(got.last_error.starts_with("setup command failed: "));
    let feed = t.store.feed(&ws.id).unwrap_or_default();
    assert!(feed.iter().any(|f| f.text.contains("Setup command failed")));
    t.close().await;
}

#[tokio::test]
async fn delete_failed_begin_create_prunes_empty_branch() {
    let t = TestEngine::new(None);
    t.engine.set_kitchen_words(&["bubbly"], &["pretzel"]);
    let repo = init_repo();
    run_project(&t, &repo.path, "p-ui", |p| p.setup_cmd = "exit 1".into());
    let ws = t
        .engine
        .begin_create_workspace("p-ui", "fail", "main")
        .await
        .unwrap();
    wait_for(Duration::from_secs(5), "create failure", || {
        t.store
            .workspace(&ws.id)
            .is_some_and(|w| w.phase == "failed")
    })
    .await;
    let branch = t.store.workspace(&ws.id).unwrap().branch;
    assert!(!branch.is_empty());
    t.engine.delete_workspace(&ws.id).await.unwrap();
    let mut gone = false;
    for _ in 0..300 {
        if !branch_exists(&repo.path, &branch).await {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(gone, "empty branch {branch} not pruned");
    t.close().await;
}

// ---- lifecycle (hand workspaces)

#[tokio::test]
async fn remove_keeps_history_and_branch_by_default() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    add_project(&t.store, &repo.path);
    let ws = t
        .engine
        .create_workspace("proj", "unique commit", "")
        .await
        .unwrap();
    std::fs::write(Path::new(&ws.worktree_path).join("f.txt"), "x\n").unwrap();
    git(&ws.worktree_path, &["add", "f.txt"]);
    git(&ws.worktree_path, &["commit", "-m", "f"]);
    t.engine
        .emit_workspace_event(&ws.id, "MA", "Maestro", "history");
    t.store
        .add_pr(Pr {
            id: "1".into(),
            workspace_id: ws.id.clone(),
            number: 1,
            source: PrSource::Local,
            branch: ws.branch.clone(),
            state: "In review".into(),
            group: "you".into(),
            ..Pr::default()
        })
        .unwrap();
    t.engine.remove_workspace(&ws.id).await.unwrap();
    wait_for(Duration::from_secs(5), "teardown done", || {
        !t.engine.is_deleting(&ws.id)
    })
    .await;
    assert!(!exists(&ws.worktree_path));
    assert!(
        branch_exists(&repo.path, &ws.branch).await,
        "default Remove keeps the branch"
    );
    assert!(
        !t.store.feed(&ws.id).unwrap_or_default().is_empty(),
        "history survives"
    );
    assert!(t.store.pr_by_workspace(&ws.id).is_some(), "the PR survives");
    assert!(t.store.workspace(&ws.id).unwrap().archived);
    t.close().await;
}

#[tokio::test]
async fn remove_deletes_branch_when_configured() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    let mut proj = run_project(&t, &repo.path, "proj", |_| {});
    proj.delete_branch_on_remove = true;
    t.store.update_project_run_config(proj).unwrap();
    let commit = |ws: &Workspace| {
        std::fs::write(Path::new(&ws.worktree_path).join("f.txt"), &ws.id).unwrap();
        git(&ws.worktree_path, &["add", "f.txt"]);
        git(&ws.worktree_path, &["commit", "-m", "f"]);
    };

    // Open PR: the branch stays for merge/rework.
    let open = t
        .engine
        .create_workspace("proj", "open pr", "")
        .await
        .unwrap();
    commit(&open);
    t.store
        .add_pr(Pr {
            id: "pr-open".into(),
            workspace_id: open.id.clone(),
            number: 1,
            source: PrSource::Local,
            state: "In review".into(),
            group: "you".into(),
            ..Pr::default()
        })
        .unwrap();
    assert_eq!(t.engine.workspace_discard_risk(&open.id).await, "");
    t.engine.remove_workspace(&open.id).await.unwrap();
    wait_for(Duration::from_secs(5), "teardown", || {
        !t.engine.is_deleting(&open.id)
    })
    .await;
    assert!(
        branch_exists(&repo.path, &open.branch).await,
        "open PR keeps its branch"
    );

    // Merged PR: force-delete proceeds, and the discard copy says so.
    let merged = t
        .engine
        .create_workspace("proj", "merged", "")
        .await
        .unwrap();
    commit(&merged);
    t.store
        .add_pr(Pr {
            id: "pr-merged".into(),
            workspace_id: merged.id.clone(),
            number: 2,
            source: PrSource::Local,
            state: "Merged".into(),
            group: "merged".into(),
            ..Pr::default()
        })
        .unwrap();
    assert_eq!(
        t.engine.workspace_discard_risk(&merged.id).await,
        "branch will be deleted"
    );
    std::fs::write(Path::new(&merged.worktree_path).join("dirty.txt"), "x").unwrap();
    assert_eq!(
        t.engine.workspace_discard_risk(&merged.id).await,
        "1 uncommitted change; branch will be deleted"
    );
    t.engine.remove_workspace(&merged.id).await.unwrap();
    let mut gone = false;
    for _ in 0..250 {
        if !branch_exists(&repo.path, &merged.branch).await {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(gone, "merged PR branch not deleted on remove");
    t.close().await;
}

#[tokio::test]
async fn delete_workspace_removes_everything() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    add_project(&t.store, &repo.path);
    let ws = t
        .engine
        .create_workspace("proj", "delete me", "")
        .await
        .unwrap();
    t.store
        .add_pr(Pr {
            id: "pr-del".into(),
            workspace_id: ws.id.clone(),
            number: 1,
            source: PrSource::Local,
            ..Pr::default()
        })
        .unwrap();
    t.store
        .create_session(Session {
            workspace_id: ws.id.clone(),
            kind: "provider".into(),
            config_json: r#"{"code":"PA","name":"Agent"}"#.into(),
            ..Session::default()
        })
        .unwrap();
    assert!(
        !t.store
            .sessions_for_loop(&ws.id)
            .unwrap_or_default()
            .is_empty()
    );
    t.engine.delete_workspace(&ws.id).await.unwrap();
    assert!(t.store.workspace(&ws.id).is_none());
    assert!(t.store.pr("pr-del").is_none(), "PR cascades");
    assert!(
        t.store
            .sessions_for_loop(&ws.id)
            .unwrap_or_default()
            .is_empty(),
        "sessions cascade"
    );
    assert!(t.events.has("workspace.deleted"));
    wait_for(Duration::from_secs(5), "worktree removed", || {
        !exists(&ws.worktree_path)
    })
    .await;
    t.close().await;
}

#[tokio::test]
async fn remove_and_delete_refuse_running_workspaces() {
    let t = TestEngine::new(None);
    t.store
        .add_workspace(Workspace {
            id: "w".into(),
            project_id: "p".into(),
            run_started: true,
            state: WorkspaceState::Coding,
            ..Workspace::default()
        })
        .unwrap();
    let running = "workspace \"w\" is running: stop it first";
    assert_eq!(t.engine.delete_workspace("w").await.unwrap_err().0, running);
    let h = LoopHandle::new("w", "p", "w", LoopState::default());
    assert!(t.engine.register_loop(RunningLoop {
        handle: Arc::clone(&h),
        cancel: CancellationToken::new(),
    }));
    t.store
        .update_workspace("w", |w| w.state = WorkspaceState::Stopped);
    assert_eq!(t.engine.remove_workspace("w").await.unwrap_err().0, running);
    t.engine.remove_loop(&h);
    t.store.update_workspace("w", |w| {
        w.state = WorkspaceState::Paused;
        w.phase = "paused".into();
    });
    assert_eq!(
        t.engine.delete_workspace("w").await.unwrap_err().0,
        running,
        "a user pause is resumable"
    );
    t.store.update_workspace("w", |w| w.phase = "idle".into());
    assert!(
        t.engine
            .check_loop_settled("w", &t.store.workspace("w").unwrap())
            .is_ok()
    );
    t.store
        .add_workspace(Workspace {
            id: "m".into(),
            is_main: true,
            ..Workspace::default()
        })
        .unwrap();
    assert_eq!(
        t.engine.remove_workspace("m").await.unwrap_err().0,
        "the main working copy can't be removed: it tracks the project's real checkout"
    );
    assert_eq!(
        t.engine.remove_workspace("zz").await.unwrap_err().0,
        "loop \"zz\" not found"
    );
    assert_eq!(
        t.engine.delete_workspace("zz").await.unwrap_err().0,
        "loop \"zz\" not found"
    );
    assert!(t.engine.reserve_deleting("w"));
    assert_eq!(
        t.engine.delete_workspace("w").await.unwrap_err().0,
        "workspace \"w\" is being removed"
    );
}

#[tokio::test]
async fn remove_parent_archives_children() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    add_project(&t.store, &repo.path);
    let parent = t
        .engine
        .create_workspace("proj", "parent", "")
        .await
        .unwrap();
    let child = t
        .engine
        .create_workspace("proj", "child", "")
        .await
        .unwrap();
    t.store
        .update_workspace(&child.id, |w| w.parent_id = parent.id.clone());
    t.engine.remove_workspace(&parent.id).await.unwrap();
    wait_for(Duration::from_secs(10), "child archived", || {
        t.store.workspace(&child.id).is_some_and(|w| w.archived)
    })
    .await;
    wait_for(Duration::from_secs(5), "child teardown", || {
        !t.engine.is_deleting(&child.id)
    })
    .await;
    assert!(
        !branch_exists(&repo.path, &child.branch).await,
        "a child branch is always deleted"
    );
    t.close().await;
}

// ---- main working copy and its remote

#[tokio::test]
async fn open_main_workspace_reuses_and_repairs() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    add_project(&t.store, &repo.path);
    let ws = t.engine.open_main_workspace("proj").await.unwrap();
    assert!(ws.is_main);
    assert_eq!(ws.branch, "main");
    assert_eq!(ws.base_branch, "");
    assert_eq!(
        (ws.goal.as_str(), ws.phase.as_str()),
        ("Working copy", "editing")
    );
    assert!(ws.port > 0);
    assert_eq!(ws.worktree_path, repo.path);
    let again = t.engine.open_main_workspace("proj").await.unwrap();
    assert_eq!(again.id, ws.id, "one main row per project");

    git(&repo.path, &["checkout", "-b", "feature"]);
    let repaired = t.engine.open_main_workspace("proj").await.unwrap();
    assert_eq!(repaired.id, ws.id);
    assert_eq!(
        repaired.branch, "feature",
        "the cached branch follows the checkout"
    );
    let mains = t
        .store
        .workspaces()
        .into_iter()
        .filter(|w| w.is_main)
        .count();
    assert_eq!(mains, 1);

    assert_eq!(
        t.engine.open_main_workspace("nope").await.unwrap_err().0,
        "project \"nope\" not found"
    );
    t.store
        .add_project(Project {
            id: "gone".into(),
            repo_path: "/definitely/not/here".into(),
            ..Project::default()
        })
        .unwrap();
    assert!(
        t.engine
            .open_main_workspace("gone")
            .await
            .unwrap_err()
            .0
            .contains("is not a directory")
    );
}

#[tokio::test]
async fn open_main_workspace_on_plain_folder() {
    let t = TestEngine::new(None);
    let dir = tempfile::tempdir().unwrap();
    add_project(&t.store, &dir.path().to_string_lossy());
    let ws = t.engine.open_main_workspace("proj").await.unwrap();
    assert_eq!(ws.branch, "");
    assert_eq!(ws.worktree_path, dir.path().to_string_lossy());
}

/// A clone of `origin` registered as the main working copy.
async fn main_clone(t: &TestEngine) -> (crate::testing::TempRepo, tempfile::TempDir, Workspace) {
    let origin = init_repo();
    let dir = tempfile::tempdir().unwrap();
    let clone = dir.path().to_string_lossy().into_owned();
    git(&clone, &["clone", &origin.path, "."]);
    git(&clone, &["config", "user.name", "test"]);
    git(&clone, &["config", "user.email", "test@example.com"]);
    add_project(&t.store, &clone);
    let ws = t.engine.open_main_workspace("proj").await.unwrap();
    (origin, dir, ws)
}

#[tokio::test]
async fn update_working_copy_from_remote() {
    let t = TestEngine::new(None);
    let (origin, _dir, ws) = main_clone(&t).await;
    let res = t
        .engine
        .update_working_copy_from_remote(&ws.id, recur_domain::UpdateBaseMethod::default())
        .await
        .unwrap();
    assert_eq!(res.status, "noop");
    assert_eq!(res.base, "origin/main");

    std::fs::write(Path::new(&origin.path).join("up.txt"), "up\n").unwrap();
    git(&origin.path, &["add", "up.txt"]);
    git(&origin.path, &["commit", "-m", "upstream"]);
    std::fs::write(Path::new(&ws.worktree_path).join("local.txt"), "dirty\n").unwrap();
    let res = t
        .engine
        .update_working_copy_from_remote(&ws.id, recur_domain::UpdateBaseMethod::default())
        .await
        .unwrap();
    assert_eq!(res.status, "updated");
    assert!(Path::new(&ws.worktree_path).join("up.txt").exists());
    assert!(
        Path::new(&ws.worktree_path).join("local.txt").exists(),
        "autostash kept edits"
    );
    let feed = t.store.feed(&ws.id).unwrap_or_default();
    assert!(
        feed.iter()
            .any(|f| f.text == "Updated with latest <code>origin/main</code> (merge)."),
        "{feed:?}"
    );
    t.store
        .add_workspace(Workspace {
            id: "hand".into(),
            ..Workspace::default()
        })
        .unwrap();
    assert_eq!(
        t.engine
            .update_working_copy_from_remote("hand", Default::default())
            .await
            .unwrap_err()
            .0,
        "update from remote is only for the main working copy"
    );
}

#[tokio::test]
async fn push_working_copy() {
    let t = TestEngine::new(None);
    let (origin, _dir, ws) = main_clone(&t).await;
    assert_eq!(
        t.engine.push_working_copy(&ws.id).await.unwrap_err().0,
        "already up to date with origin/main"
    );
    // A non-bare origin refuses a push to its checked-out branch.
    git(&origin.path, &["checkout", "-b", "side"]);
    std::fs::write(Path::new(&ws.worktree_path).join("mine.txt"), "x\n").unwrap();
    git(&ws.worktree_path, &["add", "mine.txt"]);
    git(&ws.worktree_path, &["commit", "-m", "mine"]);
    std::fs::write(Path::new(&ws.worktree_path).join("untracked.txt"), "x\n").unwrap();
    let pushed = t.engine.push_working_copy(&ws.id).await.unwrap();
    assert_eq!(pushed.id, ws.id);
    let log = git_out(&origin.path, &["log", "--oneline", "main"]).unwrap();
    assert!(log.contains("mine"), "{log}");
    assert!(
        t.store
            .feed(&ws.id)
            .unwrap_or_default()
            .iter()
            .any(|f| f.text == "Pushed to <code>origin</code>.")
    );
    t.engine.fetch_working_copy_upstream(&ws.id).await;
}

// ---- diff sync

#[tokio::test]
async fn schedule_refresh_all_single_flight() {
    let t = TestEngine::new(None);
    t.engine.schedule_refresh_all_workspace_diffs();
    t.engine.schedule_refresh_all_workspace_diffs();
    t.engine.schedule_refresh_all_workspace_diffs();
    wait_for(Duration::from_secs(2), "refresh-all to settle", || {
        t.engine.refresh_all_workspace_diffs_busy() == (false, false)
    })
    .await;
}

#[tokio::test]
async fn sync_workspace_diff_from_git() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    t.store
        .add_project(Project {
            id: "p-diff".into(),
            name: "p-diff".into(),
            repo_path: repo.path.clone(),
            base_branch: "main".into(),
            ..Project::default()
        })
        .unwrap();
    git(&repo.path, &["checkout", "-b", "recur/diff-sync"]);
    std::fs::write(Path::new(&repo.path).join("side.txt"), "a\nb\n").unwrap();
    git(&repo.path, &["add", "side.txt"]);
    git(&repo.path, &["commit", "-m", "add side"]);
    std::fs::write(Path::new(&repo.path).join("side.txt"), "a\nb\nc\n").unwrap();
    let ws = t
        .store
        .add_workspace(Workspace {
            id: "LOOP-DIFF".into(),
            project_id: "p-diff".into(),
            goal: "diff sync".into(),
            short: "diff sync".into(),
            branch: "recur/diff-sync".into(),
            base_branch: "main".into(),
            worktree_path: repo.path.clone(),
            state: WorkspaceState::Queued,
            group: Group::Active,
            ..Workspace::default()
        })
        .unwrap();
    assert_eq!(ws.diff.add, 0);
    assert!(t.engine.sync_workspace_diff(&ws.id, false).await);
    let got = t.store.workspace(&ws.id).unwrap();
    assert!(
        got.diff.add >= 3,
        "expected ≥ +3 (2 committed + 1 dirty), got {:?}",
        got.diff
    );
    assert!(!t.events.has("workspace.updated"), "publish=false");
    assert!(!t.engine.sync_workspace_diff("missing", true).await);

    // Debounced: one publish after the delay.
    std::fs::write(Path::new(&repo.path).join("side.txt"), "a\nb\nc\nd\n").unwrap();
    t.engine.schedule_workspace_diff_sync(&ws.id);
    t.engine.schedule_workspace_diff_sync(&ws.id);
    wait_for(Duration::from_secs(3), "debounced diff publish", || {
        t.events.has("workspace.updated")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(
        t.events.count("workspace.updated"),
        1,
        "two schedules coalesce into one sync"
    );
}

// ---- branch sync

#[tokio::test]
async fn sync_workspace_branch_after_rename() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    add_project(&t.store, &repo.path);
    let ws = t
        .engine
        .create_workspace("proj", "rename-me", "")
        .await
        .unwrap();
    let old = ws.branch.clone();
    t.store
        .add_pr(Pr {
            id: "1".into(),
            workspace_id: ws.id.clone(),
            number: 1,
            source: PrSource::Local,
            branch: old.clone(),
            base: "main".into(),
            title: "local".into(),
            state: "In review".into(),
            group: "you".into(),
            ..Pr::default()
        })
        .unwrap();
    let gh_ws = t
        .engine
        .create_workspace("proj", "gh-frozen", "")
        .await
        .unwrap();
    t.store
        .add_pr(Pr {
            id: "gh-9".into(),
            workspace_id: gh_ws.id.clone(),
            number: 9,
            source: PrSource::GitHub,
            branch: "recur/original-gh-head".into(),
            base: "main".into(),
            title: "gh".into(),
            state: "In review".into(),
            group: "you".into(),
            ..Pr::default()
        })
        .unwrap();
    let h = LoopHandle::new(
        &*ws.id,
        "proj",
        &*ws.id,
        LoopState {
            branch: old.clone(),
            ..LoopState::default()
        },
    );
    assert!(t.engine.register_loop(RunningLoop {
        handle: Arc::clone(&h),
        cancel: CancellationToken::new(),
    }));

    git(&ws.worktree_path, &["branch", "-m", "human/renamed"]);
    assert_eq!(
        t.engine.sync_workspace_branch(&ws.id).await,
        "human/renamed"
    );
    assert_eq!(t.store.workspace(&ws.id).unwrap().branch, "human/renamed");
    assert_eq!(t.store.pr("1").unwrap().branch, "human/renamed");
    assert_eq!(h.branch(), "human/renamed", "the live walk follows");
    assert!(t.events.has("pr.updated"));
    assert_eq!(t.store.pr("gh-9").unwrap().branch, "recur/original-gh-head");

    git(
        &gh_ws.worktree_path,
        &["branch", "-m", "agent/also-renamed"],
    );
    assert_eq!(
        t.engine.sync_workspace_branch(&gh_ws.id).await,
        "agent/also-renamed"
    );
    assert_eq!(
        t.store.pr("gh-9").unwrap().branch,
        "recur/original-gh-head",
        "forge PR head stays frozen"
    );
    assert_eq!(
        t.store.workspace(&gh_ws.id).unwrap().branch,
        "agent/also-renamed"
    );

    let carried = t.events.by_name("workspace.updated").iter().any(|e| {
        matches!(e, EngineEvent::WorkspaceUpdated(u) if u.workspace_id == ws.id && u.branch == "human/renamed")
    });
    assert!(
        carried,
        "workspace.updated did not carry the renamed branch"
    );

    // Unchanged: no churn.
    let before = t.events.count("workspace.updated");
    assert_eq!(
        t.engine.sync_workspace_branch(&ws.id).await,
        "human/renamed"
    );
    assert_eq!(t.events.count("workspace.updated"), before);
    t.close().await;
}

#[tokio::test]
async fn sync_workspace_branch_on_main_checkout() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    add_project(&t.store, &repo.path);
    let ws = t.engine.open_main_workspace("proj").await.unwrap();
    assert_eq!(ws.branch, "main");
    git(&repo.path, &["checkout", "-b", "feature"]);
    assert_eq!(t.engine.sync_workspace_branch(&ws.id).await, "feature");
    assert_eq!(t.store.workspace(&ws.id).unwrap().branch, "feature");
    assert_eq!(t.engine.sync_workspace_branch("missing").await, "");
}
