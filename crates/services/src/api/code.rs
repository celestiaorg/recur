use std::collections::BTreeMap;

use async_trait::async_trait;
use recur_domain::{
    AgentSkill, Commit, FileContent, FileDiff, FileDiffResult, FileSearchResult, OutlineSymbol,
    ReplaceResult, SearchHit, SearchOpts, SearchResult, SymbolDef, SymbolRef, TreeEntry,
    WorktreeCommitResult,
};

use crate::Result;

/// CodeService is the observatory code browser / edit surface (ADE phases 1–2, 4).
///
/// Methods that walk the tree, read files, or shell out to git are cancelled
/// when the caller drops the call, so navigating away from a search, or
/// firing a pathological regex at a monorepo, actually stops the work instead
/// of leaving it to run to completion.
///
/// Rust port of Go `app.CodeService` (`frontend/bindings/recur/internal/app/codeservice.ts`).
#[async_trait]
pub trait CodeService: Send + Sync {
    /// AbsPath returns the absolute filesystem path for a repo-relative path.
    async fn abs_path(&self, project_id: &str, workspace_id: &str, path: &str) -> Result<String>;

    /// BridgeBenchDone persists frontend-measured stats for the driving script to read.
    async fn bridge_bench_done(&self, stats_json: &str) -> Result<()>;

    /// BridgeBenchEnabled reports whether the frontend should run the RTT bench at boot.
    async fn bridge_bench_enabled(&self) -> Result<bool>;

    /// BridgeEcho returns len(payload). Times serialization + IPC for larger bodies.
    /// Uses string (not []byte) so the JS side can pass a plain string without
    /// TypedArray → Go []uint8 decode failures.
    async fn bridge_echo(&self, payload: &str) -> Result<i64>;

    /// BridgePing is a no-op echo used to time a minimal binding round-trip.
    async fn bridge_ping(&self, n: i64) -> Result<i64>;

    /// Commit stages all worktree changes and commits with the user's git identity.
    async fn commit(
        &self,
        project_id: &str,
        workspace_id: &str,
        message: &str,
    ) -> Result<WorktreeCommitResult>;

    /// CommitDiff returns one commit's unified diff (`git show --first-parent`)
    /// for the Commits-tab drill-in. Hash may be the short or full SHA.
    async fn commit_diff(
        &self,
        project_id: &str,
        workspace_id: &str,
        hash: &str,
    ) -> Result<Vec<FileDiff>>;

    /// CommitStaged commits only staged changes with the user's git identity.
    async fn commit_staged(
        &self,
        project_id: &str,
        workspace_id: &str,
        message: &str,
    ) -> Result<WorktreeCommitResult>;

    /// Copy duplicates a file or directory within a loop worktree.
    async fn copy(&self, project_id: &str, workspace_id: &str, from: &str, to: &str) -> Result<()>;

    /// CreateFile creates an empty file in a loop worktree.
    async fn create_file(&self, project_id: &str, workspace_id: &str, path: &str) -> Result<()>;

    /// Delete removes a file or directory tree from a loop worktree.
    async fn delete(&self, project_id: &str, workspace_id: &str, path: &str) -> Result<()>;

    /// DiffVsBase returns the Observatory files-changed diff: working tree versus
    /// the workspace's base branch (merge-base with HEAD, uncommitted included).
    /// An empty base (main working copy) diffs versus HEAD. Non-git roots return
    /// an empty list, not an error. Truncated is set when untracked paths were capped.
    async fn diff_vs_base(&self, project_id: &str, workspace_id: &str) -> Result<FileDiffResult>;

    /// DiffWorking returns the Observatory uncommitted stacked diff: working tree
    /// (staged + unstaged tracked, plus untracked text) versus HEAD. Independent of
    /// the workspace base branch. Non-git / missing worktree → empty list, not error.
    /// Truncated is set when untracked paths were capped.
    async fn diff_working(&self, project_id: &str, workspace_id: &str) -> Result<FileDiffResult>;

    /// DiscardAll resets the worktree to HEAD and removes untracked files.
    async fn discard_all(&self, project_id: &str, workspace_id: &str) -> Result<()>;

    /// DiscardFile restores path to HEAD or deletes it when untracked (worktree only).
    async fn discard_file(&self, project_id: &str, workspace_id: &str, path: &str) -> Result<()>;

    /// FileAtBase returns the contents of path at the loop/project base merge-base.
    /// Empty string when the file did not exist then (added/untracked). Used by the
    /// observatory CodeMirror unified merge view.
    async fn file_at_base(
        &self,
        project_id: &str,
        workspace_id: &str,
        path: &str,
    ) -> Result<String>;

    /// FileDiff returns the diff of path versus the loop/project base branch
    /// (merge-base with HEAD, working tree included). Untracked files are a full add.
    async fn file_diff(&self, project_id: &str, workspace_id: &str, path: &str)
    -> Result<FileDiff>;

    /// FindDefs returns definition sites for a symbol name.
    async fn find_defs(
        &self,
        project_id: &str,
        workspace_id: &str,
        name: &str,
    ) -> Result<Vec<SymbolDef>>;

    /// FindRefs returns references (and defs) for a symbol name.
    async fn find_refs(
        &self,
        project_id: &str,
        workspace_id: &str,
        name: &str,
    ) -> Result<Vec<SymbolRef>>;

    /// GenerateCommitMessage asks the workspace title model for a commit message
    /// based on the currently staged diff. Requires a writable worktree with staged changes.
    async fn generate_commit_message(&self, project_id: &str, workspace_id: &str)
    -> Result<String>;

    /// GitStatus returns path → m/a/d/u for the resolved root. For a loop worktree
    /// this is versus the merge-base with the base branch (spin-off point), including
    /// committed loop work and the working tree. For a bare project checkout with no
    /// base, porcelain versus HEAD.
    async fn git_status(
        &self,
        project_id: &str,
        workspace_id: &str,
    ) -> Result<BTreeMap<String, String>>;

    /// ListAllSkills returns every discovered Agent Skill (including model-only /
    /// disable-model-invocation) for Settings → Skills toggles. Disabled skills
    /// remain in the list so toggles stay visible. Empty project/workspace still
    /// returns user + shipped skills from home roots. A non-empty project/workspace
    /// that fails to resolve surfaces the error (no silent home-only fallback).
    async fn list_all_skills(
        &self,
        project_id: &str,
        workspace_id: &str,
    ) -> Result<Vec<AgentSkill>>;

    /// ListChangedFiles returns paths changed in the loop RIGHT NOW — computed live
    /// off disk (including a coding agent's not-yet-committed edits and brand-new
    /// untracked files) versus the loop's base branch (merge-base with HEAD), so
    /// the Observatory's "Changed in loop" list tracks the worktree in
    /// near-real-time — a PR (pr.Files) doesn't exist until the loop converges
    /// (openPR), which is exactly the window (agent actively editing) this is
    /// meant to cover.
    ///
    /// Live membership matches git.ChangeStatus (and therefore DiffVsBase /
    /// DiffWorkingVsBase) for an intact worktree. The last-known PR snapshot is
    /// unioned on top so files stay listed once a loop converges and its worktree
    /// is torn down — DiffVsBase errors in that degenerate state, so the extra
    /// paths are intentional and do not apply to a live checkout.
    async fn list_changed_files(&self, workspace_id: &str) -> Result<Vec<String>>;

    /// ListCommits returns commits for the Observatory Commits tab, newest first.
    /// With a base branch this is unique-to-this-workspace (merge-base..HEAD).
    /// The main working copy has no merge-base, so this is recent HEAD history.
    /// Empty when the root is not a git repo. Per-commit file diffs stay empty
    /// until CommitDiff.
    async fn list_commits(&self, project_id: &str, workspace_id: &str) -> Result<Vec<Commit>>;

    /// ListFiles returns a flat list of repo-relative file paths. Prefer SearchFiles
    /// for @-mentions / Quick Open — ranking stays in Go and avoids shipping the
    /// full corpus. May return a partial list while the first build is streaming.
    async fn list_files(&self, project_id: &str, workspace_id: &str) -> Result<Vec<String>>;

    /// ListSkills returns Agent Skills (SKILL.md) for the composer `/` menu
    /// (user-invocable only). Discovery is provider-agnostic. Skills the user
    /// disabled in Settings → Skills are omitted.
    async fn list_skills(&self, project_id: &str, workspace_id: &str) -> Result<Vec<AgentSkill>>;

    /// ListTree returns one level of the file tree under path (repo-relative).
    /// Depth is always 1 so large repos stay usable — the UI lazy-loads children
    /// when a directory is expanded (pass that dir's path on the next call).
    /// Entries carry change status versus the loop/project base branch when set;
    /// deleted paths that are missing on disk are still listed.
    ///
    /// Refreshing several directories at once should use ListTrees: this computes
    /// git status for the one listing it returns.
    async fn list_tree(
        &self,
        project_id: &str,
        workspace_id: &str,
        path: &str,
    ) -> Result<Vec<TreeEntry>>;

    /// ListTrees lists several directories in one call, sharing a single git status
    /// pass across all of them. This is the live-refresh path: re-listing N expanded
    /// directories one ListTree at a time forked three git processes per directory
    /// per tick.
    ///
    /// The result maps each requested directory (normalized to a forward-slash
    /// repo-relative path, "" for the repo root) to its children. A directory that
    /// could not be listed — typically because the agent just deleted it — is absent
    /// from the map rather than failing the whole batch.
    async fn list_trees(
        &self,
        project_id: &str,
        workspace_id: &str,
        dirs: &[String],
    ) -> Result<BTreeMap<String, Vec<TreeEntry>>>;

    /// Mkdir creates a directory in a loop worktree.
    async fn mkdir(&self, project_id: &str, workspace_id: &str, path: &str) -> Result<()>;

    /// OpenInEditor opens path:line in an external editor.
    async fn open_in_editor(
        &self,
        project_id: &str,
        workspace_id: &str,
        path: &str,
        line: i64,
    ) -> Result<()>;

    /// Outline returns top-level symbols for a file.
    async fn outline(
        &self,
        project_id: &str,
        workspace_id: &str,
        path: &str,
    ) -> Result<Vec<OutlineSymbol>>;

    /// ReadFile returns file metadata (and inlined text for small files) for preview/edit.
    /// Images and large text leave Content empty; the webview streams those from /workspace-file.
    async fn read_file(
        &self,
        project_id: &str,
        workspace_id: &str,
        path: &str,
    ) -> Result<FileContent>;

    /// RefreshFileIndex forces a full reindex of the workspace (window-focus
    /// backstop / manual). Returns the current snapshot (possibly still building).
    async fn refresh_file_index(&self, project_id: &str, workspace_id: &str)
    -> Result<Vec<String>>;

    /// Rename moves a file or directory within a loop worktree.
    async fn rename(
        &self,
        project_id: &str,
        workspace_id: &str,
        from: &str,
        to: &str,
    ) -> Result<()>;

    /// ReplaceInFiles replaces matches across the loop worktree (refused on bare project).
    async fn replace_in_files(
        &self,
        project_id: &str,
        workspace_id: &str,
        query: &str,
        replacement: &str,
        opts: &SearchOpts,
    ) -> Result<ReplaceResult>;

    /// RevealInFinder shows the path in the OS file manager.
    async fn reveal_in_finder(
        &self,
        project_id: &str,
        workspace_id: &str,
        path: &str,
    ) -> Result<()>;

    /// Search finds substring matches under the project/worktree.
    async fn search(
        &self,
        project_id: &str,
        workspace_id: &str,
        query: &str,
    ) -> Result<Vec<SearchHit>>;

    /// SearchAdvanced finds matches with regex/glob/pagination options. The result
    /// also reports how many files were skipped for exceeding the per-file size cap,
    /// so the UI can say so rather than implying there were no matches.
    async fn search_advanced(
        &self,
        project_id: &str,
        workspace_id: &str,
        query: &str,
        opts: &SearchOpts,
    ) -> Result<SearchResult>;

    /// SearchFiles ranks workspace paths for query (basename / camelCase / fuzzy).
    /// The index already applies files.exclude (SetFileIndexExclude) plus always-on
    /// .git / node_modules — @-mentions and Quick Open share that corpus.
    /// limit <= 0 defaults to 50. filesOnly skips directory hits (Quick Open).
    /// While the index is still building, Hits may be partial — Building/Ready tell
    /// the UI whether to keep refreshing.
    async fn search_files(
        &self,
        project_id: &str,
        workspace_id: &str,
        query: &str,
        limit: i64,
        files_only: bool,
    ) -> Result<FileSearchResult>;

    /// SetFileIndexExclude installs VS Code–style files.exclude patterns on the
    /// path index (same list as the Observatory explorer). Rebuilds warm indexes
    /// when the set changes so SearchFiles matches explorer-visible files.
    async fn set_file_index_exclude(&self, patterns: &[String]) -> Result<()>;

    /// Stage adds paths to the git index (worktree only).
    async fn stage(&self, project_id: &str, workspace_id: &str, paths: &[String]) -> Result<()>;

    /// StagedFiles returns paths currently staged in the worktree index.
    /// Best-effort read: a non-writable root (project checkout, or a workspace
    /// whose worktree is not on disk yet — eager create / mid-switch), a
    /// recorded worktree whose directory is gone, and a plain-folder working
    /// copy all return an empty list. Observatory live-refresh calls this
    /// after every save and on workspace switch; a 422 here used to toast
    /// "staging is only allowed in a loop worktree".
    async fn staged_files(&self, project_id: &str, workspace_id: &str) -> Result<Vec<String>>;

    /// Unstage removes paths from the git index (worktree only).
    async fn unstage(&self, project_id: &str, workspace_id: &str, paths: &[String]) -> Result<()>;

    /// WorkingStatus returns uncommitted porcelain status (path → m/a/d/u) versus HEAD.
    async fn working_status(
        &self,
        project_id: &str,
        workspace_id: &str,
    ) -> Result<BTreeMap<String, String>>;

    /// WriteFile saves edits into a loop worktree (refused on the bare project checkout).
    /// Successful human edits are recorded on the loop activity feed.
    async fn write_file(
        &self,
        project_id: &str,
        workspace_id: &str,
        path: &str,
        content: &str,
    ) -> Result<()>;
}
