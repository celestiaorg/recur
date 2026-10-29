//! Test fixtures: real repositories in temp dirs, like the Go tests.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

/// A temp dir that lives for the test, with its path as a string.
pub(crate) struct Dir {
    _tmp: TempDir,
    pub path: String,
}

pub(crate) fn temp_dir() -> Dir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().to_string_lossy().into_owned();
    Dir { _tmp: tmp, path }
}

const IDENTITY: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "test"),
    ("GIT_AUTHOR_EMAIL", "test@example.com"),
    ("GIT_COMMITTER_NAME", "test"),
    ("GIT_COMMITTER_EMAIL", "test@example.com"),
];

/// Runs git in `dir` with the test identity, failing the test on error.
pub(crate) fn git(dir: &str, args: &[&str]) -> String {
    git_as(dir, args, &IDENTITY)
}

pub(crate) fn git_as(dir: &str, args: &[&str], env: &[(&str, &str)]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .envs(env.iter().copied())
        .output()
        .expect("spawn git");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "git {args:?} (in {dir}): {}\n{combined}",
        out.status
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// git without a forced identity (Go `gitOut`): combined output.
pub(crate) fn git_out(dir: &str, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("spawn git");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "git {args:?}: {}\n{combined}",
        out.status
    );
    combined
}

pub(crate) fn rev_parse(dir: &str, r: &str) -> String {
    let out = Command::new("git")
        .args(["-C", dir, "rev-parse", r])
        .output()
        .expect("spawn git");
    assert!(out.status.success(), "git rev-parse {r}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

pub(crate) fn write(dir: &str, rel: &str, content: &str) {
    write_bytes(dir, rel, content.as_bytes());
}

pub(crate) fn write_bytes(dir: &str, rel: &str, content: &[u8]) {
    let p = Path::new(dir).join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(&p, content).expect("write");
}

pub(crate) fn exists(path: &str) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// Go `initRepo`: one commit ("initial", README.md "hello\n") on `main`.
pub(crate) fn init_repo() -> Dir {
    let d = temp_dir();
    git(&d.path, &["init", "-b", "main"]);
    git(&d.path, &["config", "user.name", "test"]);
    git(&d.path, &["config", "user.email", "test@example.com"]);
    write(&d.path, "README.md", "hello\n");
    git(&d.path, &["add", "."]);
    git(&d.path, &["commit", "-m", "initial"]);
    d
}

/// Go `initDirtyRepo`: `a.go` committed then modified, plus `untracked`
/// untracked files.
pub(crate) fn init_dirty_repo(untracked: usize) -> Dir {
    let d = temp_dir();
    git(&d.path, &["init", "-b", "main"]);
    git(&d.path, &["config", "core.untrackedCache", "true"]);
    write(&d.path, "a.go", "package a\n");
    git(&d.path, &["add", "."]);
    git(&d.path, &["commit", "-m", "init"]);
    write(&d.path, "a.go", "package a\nfunc F() {}\n");
    for i in 0..untracked {
        write(&d.path, &format!("u{i}.txt"), "x\n");
    }
    d
}

/// Go `initRepoWithRemote`: a bare upstream and a clone of it (with
/// `origin`) holding one commit on `main`.
pub(crate) struct WithRemote {
    _dirs: Vec<Dir>,
    pub clone: String,
    pub bare: String,
}

pub(crate) fn init_repo_with_remote() -> WithRemote {
    let bare_dir = temp_dir();
    let bare = format!("{}/upstream.git", bare_dir.path);
    let seed = temp_dir();
    git(&seed.path, &["init", "-b", "main"]);
    write(&seed.path, "README.md", "hello\n");
    git(&seed.path, &["add", "."]);
    git(&seed.path, &["commit", "-m", "initial"]);
    git(&seed.path, &["clone", "--bare", &seed.path, &bare]);
    let clone_dir = temp_dir();
    let clone = format!("{}/work", clone_dir.path);
    let cwd = temp_dir();
    git(&cwd.path, &["clone", &bare, &clone]);
    WithRemote {
        _dirs: vec![bare_dir, seed, clone_dir, cwd],
        clone,
        bare,
    }
}

/// Go `gitConfigIdentity`.
pub(crate) fn config_identity(dir: &str) {
    git_out(dir, &["config", "user.name", "test"]);
    git_out(dir, &["config", "user.email", "test@example.com"]);
}

/// Go `commit` in diff_test.go: stage everything as Coder.
pub(crate) fn commit_as_coder(dir: &str, msg: &str) {
    let env = [
        ("GIT_AUTHOR_NAME", "Coder"),
        ("GIT_AUTHOR_EMAIL", "coder@recur.local"),
        ("GIT_COMMITTER_NAME", "Coder"),
        ("GIT_COMMITTER_EMAIL", "coder@recur.local"),
    ];
    git_as(dir, &["add", "-A"], &env);
    git_as(dir, &["commit", "-m", msg], &env);
}

/// Go `cleanPath` for comparing worktree paths.
pub(crate) fn clean(p: &str) -> String {
    crate::worktree::clean_path(p)
}
