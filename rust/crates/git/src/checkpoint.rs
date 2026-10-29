//! Checkpoints: hidden snapshots of a worktree (tracked changes plus
//! untracked, non-ignored files) stored as commits under a private ref
//! namespace (`refs/recur/checkpoints/…`).
//!
//! Nothing on the working branch, the real index, the stash stack, the reflog
//! or the working tree changes: staging happens in a private index copy
//! (`GIT_INDEX_FILE`) in Recur's data dir, so untracked files stay untracked.
//! `git log` / `branch` / `tag` / `stash list` never show a checkpoint; only
//! `show-ref`, `for-each-ref` and `log --all` do. `git push --mirror` would
//! publish them, plain push/fetch do not.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;

use recur_domain::FileDiff;
use recur_domain::path;

use crate::command::command;
use crate::diff::{commit_ref_exists, parse_highlighted_patch};
use crate::error::{Error, Result};

/// The namespace every checkpoint ref lives under.
pub const CHECKPOINT_REF_PREFIX: &str = "refs/recur/checkpoints/";

/// Gives commit-tree an author even when none is configured: checkpoints are
/// Recur's, not the user's.
const CHECKPOINT_IDENTITY: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "Recur"),
    ("GIT_AUTHOR_EMAIL", "checkpoint@recur.local"),
    ("GIT_COMMITTER_NAME", "Recur"),
    ("GIT_COMMITTER_EMAIL", "checkpoint@recur.local"),
];

/// Snapshots `worktree` into `ref_name` and returns the commit oid.
/// `index_dir` holds the private index copy (created if needed) and must be
/// outside the worktree. `ref_name` must start with [`CHECKPOINT_REF_PREFIX`].
pub async fn checkpoint(worktree: &str, index_dir: &str, ref_name: &str) -> Result<String> {
    if !ref_name.starts_with(CHECKPOINT_REF_PREFIX) {
        return Err(Error::msg(format!(
            "checkpoint ref {:?} outside {CHECKPOINT_REF_PREFIX}",
            ref_name
        )));
    }
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o755)
        .create(index_dir)
        .map_err(|e| Error::io_path(index_dir, &e))?;
    let priv_index = path::join(&[index_dir, "index"]);
    // Seed from the real index so `add -A` only re-hashes files whose stat
    // changed (milliseconds on a large repo instead of a full rehash).
    let real_index = run(
        worktree,
        &[],
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
    )
    .await?;
    match std::fs::read(real_index.trim()) {
        Ok(data) => write_file(&priv_index, &data).map_err(|e| Error::io_path(&priv_index, &e))?,
        // Unborn repo: start from an empty index.
        Err(_) => {
            let _ = std::fs::remove_file(&priv_index);
        }
    }
    let index_env = [("GIT_INDEX_FILE", priv_index.as_str())];
    run(worktree, &index_env, &["add", "-A", "--", "."]).await?;
    let tree = run(worktree, &index_env, &["write-tree"]).await?;
    let mut args = vec!["commit-tree", tree.trim(), "-m", "recur checkpoint"];
    if commit_ref_exists(worktree, "HEAD").await {
        args.extend(["-p", "HEAD"]);
    }
    let oid = run(worktree, &CHECKPOINT_IDENTITY, &args)
        .await?
        .trim()
        .to_string();
    run(worktree, &[], &["update-ref", ref_name, oid.as_str()]).await?;
    Ok(oid)
}

/// ref → oid for every checkpoint under `prefix` (a full ref path such as
/// `CHECKPOINT_REF_PREFIX + workspace_id + "/"`).
pub async fn list_checkpoints(worktree: &str, prefix: &str) -> Option<HashMap<String, String>> {
    list_checkpoint_refs(worktree, prefix, "objectname").await
}

/// ref → tree oid in one git call. Comparing trees ignores commit metadata.
pub async fn list_checkpoint_trees(
    worktree: &str,
    prefix: &str,
) -> Option<HashMap<String, String>> {
    list_checkpoint_refs(worktree, prefix, "tree").await
}

async fn list_checkpoint_refs(
    worktree: &str,
    prefix: &str,
    field: &str,
) -> Option<HashMap<String, String>> {
    let format = format!("--format=%(refname) %({field})");
    let out = run(worktree, &[], &["for-each-ref", format.as_str(), prefix])
        .await
        .ok()?;
    Some(
        out.trim()
            .split('\n')
            .filter_map(|line| line.split_once(' '))
            .map(|(name, oid)| (name.to_string(), oid.to_string()))
            .collect(),
    )
}

/// Removes every checkpoint ref under `prefix`. The objects become
/// unreachable and the user's normal `git gc` reclaims them.
pub async fn delete_checkpoints(repo: &str, prefix: &str) -> Result<()> {
    for r in list_checkpoints(repo, prefix)
        .await
        .unwrap_or_default()
        .keys()
    {
        run(repo, &[], &["update-ref", "-d", r.as_str()]).await?;
    }
    Ok(())
}

/// The unified diff `from` → `to` (checkpoint refs or any commit-ish),
/// parsed and highlighted like the Commits tab.
pub async fn diff_checkpoints(worktree: &str, from: &str, to: &str) -> Result<Vec<FileDiff>> {
    let out = run(worktree, &[], &["diff", "--no-color", from, to]).await?;
    Ok(parse_highlighted_patch(&out))
}

/// Makes the working tree match `ref_name`: tracked files are restored or
/// removed (no-overlay), then untracked files that did not exist at the
/// checkpoint are deleted. The index is untouched, so files untracked at
/// capture come back untracked.
///
/// simplify: index state (what was staged) is not snapshotted, so a file
/// staged at capture comes back as an unstaged modification; commits made
/// after the checkpoint stay on the branch. Upgrade path: pin a second tree of
/// the real index per checkpoint and read-tree it here.
pub async fn restore_checkpoint(worktree: &str, ref_name: &str) -> Result<()> {
    let source = format!("--source={ref_name}");
    run(
        worktree,
        &[],
        &[
            "restore",
            source.as_str(),
            "--worktree",
            "--no-overlay",
            "--",
            ".",
        ],
    )
    .await?;
    let in_ckpt = run_raw(
        worktree,
        &[],
        &["ls-tree", "-r", "-z", "--name-only", ref_name],
    )
    .await?;
    let keep: HashSet<&[u8]> = in_ckpt
        .split(|&b| b == 0)
        .filter(|p| !p.is_empty())
        .collect();
    let others = run_raw(
        worktree,
        &[],
        &["ls-files", "-z", "--others", "--exclude-standard"],
    )
    .await?;
    for p in others.split(|&b| b == 0) {
        if p.is_empty() || keep.contains(p) {
            continue;
        }
        let full = Path::new(worktree).join(OsStr::from_bytes(p));
        if let Err(e) = std::fs::remove_file(&full)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(Error::io_path(&full.to_string_lossy(), &e));
        }
    }
    Ok(())
}

/// Go `os.WriteFile(path, data, 0o644)`.
fn write_file(path: &str, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o644)
        .open(path)?
        .write_all(data)
}

async fn run(dir: &str, env: &[(&str, &str)], args: &[&str]) -> Result<String> {
    run_raw(dir, env, args)
        .await
        .map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// stdout, or `git <verb>: <stderr>: <err>` (Go `run` in checkpoint.go).
async fn run_raw(dir: &str, env: &[(&str, &str)], args: &[&str]) -> Result<Vec<u8>> {
    let mut cmd = command(["-C", dir]).args(args);
    for (k, v) in env {
        cmd = cmd.env(k, v);
    }
    let ran = cmd.capture().await;
    let verb = args.first().copied().unwrap_or_default();
    match ran.result {
        Ok(()) => Ok(ran.stdout),
        Err(e) => {
            let msg = String::from_utf8_lossy(&ran.stderr).into_owned();
            let msg = msg.trim();
            Err(if msg.is_empty() {
                e.wrap(format!("git {verb}"))
            } else {
                e.wrap(format!("git {verb}: {msg}"))
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{git_out, init_repo, temp_dir, write};

    /// A checkpoint captures tracked edits and untracked files without
    /// touching the branch, the real index, the stash stack, or the tree.
    #[tokio::test]
    async fn checkpoint_is_invisible_and_complete() {
        let repo = init_repo();
        let dir = repo.path.as_str();
        let idx_root = temp_dir();
        let idx = format!("{}/ckpt", idx_root.path);
        write(dir, "README.md", "edited\n");
        write(dir, "new.txt", "untracked\n");
        git_out(
            dir,
            &["stash", "push", "-m", "user-stash", "--", "README.md"],
        );
        write(dir, "README.md", "edited again\n");
        let head_before = git_out(dir, &["rev-parse", "HEAD"]);
        let status_before = git_out(dir, &["status", "--porcelain"]);
        let stash_before = git_out(dir, &["stash", "list"]);

        let r = format!("{CHECKPOINT_REF_PREFIX}ws1/msg1/start");
        let oid = checkpoint(dir, &idx, &r).await.expect("checkpoint");

        assert_eq!(
            git_out(dir, &["rev-parse", "HEAD"]),
            head_before,
            "HEAD moved"
        );
        assert_eq!(
            git_out(dir, &["status", "--porcelain"]),
            status_before,
            "status changed"
        );
        assert!(
            status_before.contains("?? new.txt"),
            "fixture: new.txt should be untracked: {status_before}"
        );
        assert_eq!(
            git_out(dir, &["stash", "list"]),
            stash_before,
            "stash list changed"
        );
        let log = git_out(dir, &["log", "--oneline", "--branches", "--tags"]);
        assert!(
            !log.contains("recur checkpoint"),
            "checkpoint visible in branch/tag log:\n{log}"
        );
        let tree = git_out(dir, &["ls-tree", "-r", "--name-only", &oid]);
        assert!(
            tree.contains("new.txt"),
            "untracked file missing from checkpoint tree:\n{tree}"
        );
        assert_eq!(
            git_out(dir, &["show", &format!("{oid}:README.md")]),
            "edited again\n"
        );
        let prefix = format!("{CHECKPOINT_REF_PREFIX}ws1/");
        let refs = list_checkpoints(dir, &prefix).await.unwrap_or_default();
        assert_eq!(refs.get(&r), Some(&oid), "list_checkpoints = {refs:?}");
        assert!(
            std::fs::metadata(format!("{idx}/index")).is_ok(),
            "private index not in the data dir"
        );

        let same = format!("{CHECKPOINT_REF_PREFIX}ws1/msg1/same");
        checkpoint(dir, &idx, &same).await.expect("checkpoint same");
        let trees = list_checkpoint_trees(dir, &prefix)
            .await
            .unwrap_or_default();
        let want = git_out(dir, &["rev-parse", &format!("{oid}^{{tree}}")])
            .trim()
            .to_string();
        assert_eq!(trees.get(&r), Some(&want));
        assert!(
            !trees[&r].is_empty() && trees[&r] == trees[&same],
            "identical snapshots should not differ"
        );

        // Later edits diff against the checkpoint, and restore rewinds them:
        // new.txt comes back untracked, the agent's extra file is deleted.
        write(dir, "README.md", "agent\n");
        write(dir, "agent.txt", "x\n");
        let end = format!("{CHECKPOINT_REF_PREFIX}ws1/msg1/end");
        checkpoint(dir, &idx, &end).await.expect("checkpoint end");
        let trees = list_checkpoint_trees(dir, &prefix)
            .await
            .unwrap_or_default();
        assert!(
            !trees[&end].is_empty() && trees[&r] != trees[&end],
            "edited snapshot should differ"
        );
        let files = diff_checkpoints(dir, &r, &end)
            .await
            .expect("diff_checkpoints");
        let names: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert!(
            files.len() == 2 && names.contains(&"agent.txt"),
            "turn diff files = {names:?}"
        );
        restore_checkpoint(dir, &r).await.expect("restore");
        assert_eq!(
            git_out(dir, &["status", "--porcelain"]),
            status_before,
            "status after restore"
        );
        assert!(
            std::fs::metadata(format!("{dir}/agent.txt")).is_err(),
            "agent.txt should be removed"
        );

        delete_checkpoints(dir, &prefix).await.expect("delete");
        let left = list_checkpoints(dir, CHECKPOINT_REF_PREFIX)
            .await
            .unwrap_or_default();
        assert!(left.is_empty(), "refs left after delete: {left:?}");
    }
}
