//! Ports of `internal/app/*_test.go` (the SV acceptance gate) over the real
//! services: a seeded in-memory store, a temp data dir, a scripted provider
//! pinned for every turn (Go `SetRunner(&ScriptedRunner{})`), the GitHub
//! forge with no `gh` (Go CI without gh), and fake issue trackers.

mod services;
mod workspace_create;
mod workspace_issue;
mod workspace_main;
mod workspace_open;
mod workspace_pr;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use recur_agent::Provider;
use recur_agent::testing::ScriptedProvider;
use recur_domain::{Project, Workspace};
use recur_engine::GitSetup;
use recur_engine::testing::BusRecorder;
use recur_forge::Forge;
use recur_issues::IssueTracker;
use recur_issues::testing::FakeTracker;
use recur_store::Store;
use tempfile::TempDir;

use super::{AppConfig, AppOptions, AppServices};
use crate::Services;

pub(super) struct Fx {
    pub svc: Arc<AppServices>,
    pub s: Services,
    pub events: BusRecorder,
    pub trackers: Vec<Arc<FakeTracker>>,
    pub root: PathBuf,
    temps: Mutex<Vec<TempDir>>,
    _dir: TempDir,
}

pub(super) struct FxOpts {
    /// A SQLite store under the fixture root (Go `store.New()`); else the
    /// demo-seeded in-memory store.
    pub file_store: bool,
    pub provider: Option<Arc<dyn Provider>>,
    pub forges: Option<Vec<Arc<dyn Forge>>>,
}

impl Default for FxOpts {
    fn default() -> Self {
        Self {
            file_store: false,
            provider: Some(Arc::new(ScriptedProvider::new(vec![]))),
            forges: None,
        }
    }
}

impl Fx {
    /// Go `newTestServices`: the demo-seeded in-memory store.
    pub async fn new() -> Self {
        Self::with(FxOpts::default()).await
    }

    pub async fn with(opts: FxOpts) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().canonicalize().expect("canonical tempdir");
        let config = AppConfig::under(&root);
        std::fs::create_dir_all(&config.worktrees_dir).expect("worktrees dir");
        std::fs::create_dir_all(&config.bundles.user_dir).expect("user bundles");
        std::fs::create_dir_all(&config.bundles.default_dir).expect("bundles");
        let store = Arc::new(if opts.file_store {
            Store::open(&root.join("recur.db")).expect("store open")
        } else {
            Store::new_in_memory()
        });
        let trackers = vec![
            Arc::new(FakeTracker::new("github")),
            Arc::new(FakeTracker::new("linear")),
        ];
        let forges = opts.forges.unwrap_or_else(|| {
            vec![Arc::new(recur_forge::github::GitHub::with_gh(recur_forge::Gh::new(
                "/nonexistent/recur-test-gh",
            ))) as Arc<dyn Forge>]
        });
        let svc = AppServices::new(
            store,
            config.clone(),
            AppOptions {
                pin: Some(opts.provider),
                forges: Some(forges),
                trackers: Some(
                    trackers
                        .iter()
                        .map(|t| t.clone() as Arc<dyn IssueTracker>)
                        .collect(),
                ),
                git: Some(GitSetup {
                    worktree_root: config.worktrees_dir.to_string_lossy().into_owned(),
                    ..GitSetup::default()
                }),
            },
        )
        .await;
        let events = BusRecorder::attach(&svc.engine.bus);
        let s = svc.services();
        Self {
            svc,
            s,
            events,
            trackers,
            root,
            temps: Mutex::new(Vec::new()),
            _dir: dir,
        }
    }

    pub fn store(&self) -> &Store {
        &self.svc.engine.store
    }

    /// A temp dir that lives as long as the fixture (Go `t.TempDir()`).
    pub fn temp(&self) -> String {
        let d = tempfile::tempdir().expect("tempdir");
        let p = d.path().canonicalize().expect("canonical");
        self.temps.lock().push(d);
        p.to_string_lossy().into_owned()
    }

    /// Go `initGitRepo`: one commit on main.
    pub fn init_git_repo(&self) -> String {
        let dir = self.temp();
        git(&dir, &["init", "-b", "main"]);
        git(&dir, &["config", "user.name", "test"]);
        git(&dir, &["config", "user.email", "test@example.com"]);
        std::fs::write(Path::new(&dir).join("README.md"), "hi\n").expect("write");
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-m", "initial"]);
        dir
    }

    /// Go `initGitRepoWithRemote`: a clone of a bare upstream.
    pub fn init_git_repo_with_remote(&self) -> (String, String) {
        let bare = format!("{}/upstream.git", self.temp());
        let seed = self.temp();
        git(&seed, &["init", "-b", "main"]);
        git(&seed, &["config", "user.name", "test"]);
        git(&seed, &["config", "user.email", "test@example.com"]);
        std::fs::write(Path::new(&seed).join("README.md"), "hello\n").expect("write");
        git(&seed, &["add", "."]);
        git(&seed, &["commit", "-m", "initial"]);
        git(&seed, &["clone", "--bare", &seed, &bare]);
        let clone = format!("{}/work", self.temp());
        git(&self.temp(), &["clone", &bare, &clone]);
        git(&clone, &["config", "user.name", "test"]);
        git(&clone, &["config", "user.email", "test@example.com"]);
        (clone, bare)
    }

    /// Go `svcAddProject`: project `p` on `repo` with base main.
    pub fn add_project(&self, repo: &str) -> String {
        self.store()
            .add_project(Project {
                id: "p".into(),
                name: "p".into(),
                repo_path: repo.into(),
                base_branch: "main".into(),
                ..Project::default()
            })
            .expect("add project")
            .id
    }

    /// Go `waitWorkspaceReady`: the background `git worktree add` landed.
    pub async fn wait_ready(&self, id: &str) -> Workspace {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        while tokio::time::Instant::now() < deadline {
            if let Some(ws) = self.store().workspace(id) {
                if !ws.last_error.is_empty() && ws.worktree_path.is_empty() {
                    panic!("workspace create failed: {}", ws.last_error);
                }
                if !ws.worktree_path.is_empty() && ws.phase != "creating" {
                    return ws;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for workspace worktree");
    }

    /// A second clone of `bare` with the test identity (Go's `other`).
    pub fn clone_other(&self, bare: &str) -> String {
        let other = format!("{}/other", self.temp());
        git(&self.temp(), &["clone", bare, &other]);
        git(&other, &["config", "user.name", "test"]);
        git(&other, &["config", "user.email", "test@example.com"]);
        other
    }

    /// WriteFile + Commit through the services; panics unless committed.
    pub async fn commit_file(&self, pid: &str, ws: &str, file: &str, body: &str, msg: &str) {
        self.s.code.write_file(pid, ws, file, body).await.unwrap();
        let res = self.s.code.commit(pid, ws, msg).await.unwrap();
        assert!(res.committed, "commit {msg:?}: {res:?}");
    }

    /// CreateWorkspace (agent) and wait for its worktree.
    pub async fn solo(&self, pid: &str, goal: &str, base: &str) -> Workspace {
        let ws = self
            .s
            .workspaces
            .create_workspace(pid, goal, base, "agent", "", "", None)
            .await
            .unwrap();
        self.wait_ready(&ws.id).await
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.events.all().iter().map(|e| e.name()).collect()
    }

    pub async fn close(self) {
        self.svc.shutdown().await;
    }
}

/// Runs git with a fixed test identity; panics on failure.
pub(super) fn git(dir: &str, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?} (in {dir}): {}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `git -C dir args` output, `Err` with the combined output on failure.
pub(super) fn git_out(dir: &str, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        return Ok(s);
    }
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    Err(s)
}

pub(super) fn write(path: impl AsRef<Path>, body: &str) {
    if let Some(dir) = path.as_ref().parent() {
        std::fs::create_dir_all(dir).expect("mkdir");
    }
    std::fs::write(path, body).expect("write");
}

pub(super) fn read(path: impl AsRef<Path>) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}
