//! Tests that change the process environment, as the Go tests did with
//! `t.Setenv`. They live in their own test binary and hold one lock while the
//! environment is changed, so no other thread reads it concurrently.
#![allow(unsafe_code)]

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, MutexGuard, PoisonError};

use recur_git::Manager;
use tempfile::TempDir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Restores every variable it changed when dropped.
struct Env {
    saved: Vec<(String, Option<OsString>)>,
    _lock: MutexGuard<'static, ()>,
}

impl Env {
    fn new() -> Env {
        Env {
            saved: Vec::new(),
            _lock: ENV_LOCK.lock().unwrap_or_else(PoisonError::into_inner),
        }
    }

    fn remember(&mut self, key: &str) {
        if !self.saved.iter().any(|(k, _)| k == key) {
            self.saved.push((key.to_string(), std::env::var_os(key)));
        }
    }

    fn set(&mut self, key: &str, value: impl AsRef<std::ffi::OsStr>) {
        self.remember(key);
        // SAFETY: ENV_LOCK serializes every test in this binary, and no other
        // thread reads the environment while it is held.
        unsafe { std::env::set_var(key, value) };
    }

    fn unset(&mut self, key: &str) {
        self.remember(key);
        // SAFETY: as in `set`.
        unsafe { std::env::remove_var(key) };
    }

    /// Go `isolateGitConfig`: hide global/system git config and author env so
    /// only the repository under test is consulted.
    fn isolate_git_config(&mut self, home: &Path) {
        let empty = home.join("empty.gitconfig");
        std::fs::write(&empty, b"").expect("write empty gitconfig");
        self.set("HOME", home);
        self.set("XDG_CONFIG_HOME", home);
        self.set("GIT_CONFIG_GLOBAL", &empty);
        self.set("GIT_CONFIG_SYSTEM", &empty);
        self.set("GIT_CONFIG_NOSYSTEM", "1");
        for k in [
            "GIT_AUTHOR_NAME",
            "GIT_AUTHOR_EMAIL",
            "GIT_COMMITTER_NAME",
            "GIT_COMMITTER_EMAIL",
            "EMAIL",
        ] {
            self.unset(k);
        }
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        for (k, v) in self.saved.drain(..) {
            // SAFETY: still holding ENV_LOCK.
            unsafe {
                match v {
                    Some(v) => std::env::set_var(&k, v),
                    None => std::env::remove_var(&k),
                }
            }
        }
    }
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
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
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn init_repo() -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    git(dir.path(), &["init", "-b", "main"]);
    git(dir.path(), &["config", "user.name", "test"]);
    git(dir.path(), &["config", "user.email", "test@example.com"]);
    std::fs::write(dir.path().join("README.md"), "hello\n").expect("write");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "initial"]);
    dir
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

fn path(d: &TempDir) -> String {
    d.path().to_string_lossy().into_owned()
}

#[test]
fn identity_comes_from_git_and_errors_without_one() {
    let repo = init_repo();
    let home = tempfile::tempdir().expect("tempdir");
    let mut env = Env::new();
    env.isolate_git_config(home.path());
    runtime().block_on(async {
        let (name, email) = recur_git::identity(&path(&repo)).await.expect("identity");
        assert_eq!(
            (name.as_str(), email.as_str()),
            ("test", "test@example.com")
        );
        let r = repo.path();
        for args in [
            ["config", "user.useConfigOnly", "true"].as_slice(),
            &["config", "--unset", "user.name"],
            &["config", "--unset", "user.email"],
        ] {
            let out = Command::new("git")
                .args(args)
                .current_dir(r)
                .output()
                .expect("git");
            assert!(out.status.success(), "git {args:?}");
        }
        let err = recur_git::identity(&path(&repo))
            .await
            .expect_err("identity without config must fail");
        assert!(err.message().starts_with("git identity: "), "{err}");
    });
    drop(env);
}

#[test]
fn add_empty_repo_requires_git_identity() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(dir.path())
        .output()
        .expect("git");
    assert!(out.status.success());
    let out = Command::new("git")
        .args(["config", "user.useConfigOnly", "true"])
        .current_dir(dir.path())
        .output()
        .expect("git");
    assert!(out.status.success());
    let home = tempfile::tempdir().expect("tempdir");
    let mut env = Env::new();
    env.isolate_git_config(home.path());
    let root = tempfile::tempdir().expect("tempdir");
    runtime().block_on(async {
        let m = Manager::new(&path(&dir), &path(&root));
        let err = m
            .add("LOOP-EMPTY", "first-task", "main")
            .await
            .expect_err("add without identity");
        assert!(err.message().contains("git identity"), "{err}");
        assert!(
            !m.commit_exists("main").await,
            "no initial commit without a git identity"
        );
    });
    drop(env);
}

#[test]
fn merge_creates_local_from_selected_remote() {
    // Two remotes both have main and there is no local main: merging onto
    // upstream/main must create main from upstream, not DWIM to origin.
    let tmp = |name: &str| {
        let d = tempfile::tempdir().expect("tempdir");
        let p = d.path().join(name);
        (d, p)
    };
    let (_o, origin_bare) = tmp("origin.git");
    let (_u, upstream_bare) = tmp("upstream.git");
    let seed = tempfile::tempdir().expect("tempdir");
    git(seed.path(), &["init", "-b", "main"]);
    std::fs::write(seed.path().join("README.md"), "origin tip\n").expect("write");
    git(seed.path(), &["add", "."]);
    git(seed.path(), &["commit", "-m", "origin initial"]);
    git(
        seed.path(),
        &[
            "clone",
            "--bare",
            &path(&seed),
            &origin_bare.to_string_lossy(),
        ],
    );
    let up_seed = tempfile::tempdir().expect("tempdir");
    git(up_seed.path(), &["init", "-b", "main"]);
    std::fs::write(up_seed.path().join("README.md"), "upstream tip\n").expect("write");
    git(up_seed.path(), &["add", "."]);
    git(up_seed.path(), &["commit", "-m", "upstream initial"]);
    git(
        up_seed.path(),
        &[
            "clone",
            "--bare",
            &path(&up_seed),
            &upstream_bare.to_string_lossy(),
        ],
    );
    let (_c, clone) = tmp("work");
    let cwd = tempfile::tempdir().expect("tempdir");
    git(
        cwd.path(),
        &[
            "clone",
            &origin_bare.to_string_lossy(),
            &clone.to_string_lossy(),
        ],
    );
    git(
        &clone,
        &[
            "remote",
            "add",
            "upstream",
            &upstream_bare.to_string_lossy(),
        ],
    );
    git(&clone, &["fetch", "upstream"]);
    git(&clone, &["checkout", "--detach"]);
    git(&clone, &["branch", "-D", "main"]);

    let root = tempfile::tempdir().expect("tempdir");
    let clone_s = clone.to_string_lossy().into_owned();
    let mut env = Env::new();
    runtime().block_on(async {
        let m = Manager::new(&clone_s, &path(&root));
        let (wt, _) = m.add("ws-up", "ws-up", "upstream/main").await.expect("add");
        std::fs::write(Path::new(&wt.path).join("feat.txt"), "from loop\n").expect("write");
        m.commit(&wt.path, "feat", "Coder", "coder@recur.local")
            .await
            .expect("commit");
        // Merges commit as the user's own identity; a fresh runner has none.
        env.set("GIT_AUTHOR_NAME", "test");
        env.set("GIT_AUTHOR_EMAIL", "test@example.com");
        env.set("GIT_COMMITTER_NAME", "test");
        env.set("GIT_COMMITTER_EMAIL", "test@example.com");
        m.merge(&wt.branch, "upstream/main", "merge up", "merge")
            .await
            .expect("merge onto upstream/main");
    });
    drop(env);
    let body = std::fs::read_to_string(clone.join("README.md")).expect("read");
    assert_eq!(body, "upstream tip\n", "main should come from upstream");
}
