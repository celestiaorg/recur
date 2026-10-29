//! `CodeService`: the Observatory's file tree, reads and writes, search, diffs,
//! commits and staging over the resolved workspace root.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use parking_lot::Mutex;
use recur_domain::path;
use recur_domain::{
    AgentSkill, Commit, FeedItem, FileContent, FileDiff, FileDiffResult, FileSearchResult,
    OutlineSymbol, ReplaceResult, SearchHit, SearchOpts, SearchResult, SymbolDef, SymbolRef,
    TreeEntry, WorktreeCommitResult, effective_skill_enabled, run_active,
};
use recur_engine::bus::{EngineEvent, WorkspaceEvent};
use recur_engine::text::html_escape;
use tokio_util::sync::CancellationToken;

use super::rt::{blocking, rt, rt_ctx};
use super::{AppServices, err};
use crate::api::CodeService;
use crate::{Error, Result};

const NOT_WRITABLE: &str =
    "editing is only allowed in a loop worktree — open a loop or use an external editor";

/// The absolute paths the user opened that sit outside every project worktree.
/// Observatory reads and writes may touch them; agent tools still go through
/// `safe_join` and cannot.
#[derive(Default)]
pub struct LooseFiles(Mutex<HashSet<PathBuf>>);

impl LooseFiles {
    pub fn allow(&self, abs: &str) {
        if abs.is_empty() {
            return;
        }
        self.0.lock().insert(clean(abs));
    }

    pub fn ok(&self, abs: &str) -> bool {
        !abs.is_empty() && self.0.lock().contains(&clean(abs))
    }
}

/// Lexically cleans an absolute path.
fn clean(abs: &str) -> PathBuf {
    PathBuf::from(path::clean(abs))
}

fn is_abs(p: &str) -> bool {
    p.starts_with('/')
}

fn dir_base(p: &str) -> (PathBuf, String) {
    (PathBuf::from(path::dir(p)), path::base(p))
}

/// A recorded worktree whose directory is gone: live-refresh reads answer
/// empty instead of painting the project checkout's state.
fn worktree_missing(e: &recur_code::Error) -> bool {
    matches!(e, recur_code::Error::WorktreeMissing(_))
}

impl AppServices {
    /// The browse root, whether it is writable, and the base branch the
    /// workspace (or project) was spun off, in one pass. A recorded worktree
    /// that is gone is `WorktreeMissing`, never the project checkout.
    pub(crate) fn resolve_scope(
        &self,
        project_id: &str,
        workspace_id: &str,
    ) -> std::result::Result<(PathBuf, bool, String), recur_code::Error> {
        let (mut project_id, mut wt, mut base, mut main) =
            (project_id.to_owned(), String::new(), String::new(), false);
        if !workspace_id.is_empty()
            && let Some(w) = self.store().workspace(workspace_id)
        {
            wt = w.worktree_path;
            base = w.base_branch;
            main = w.is_main;
            if project_id.is_empty() {
                project_id = w.project_id;
            }
        }
        let mut repo = String::new();
        if !project_id.is_empty() {
            let Some(p) = self.store().project(&project_id) else {
                return Err(recur_code::Error::Msg(format!(
                    "project {:?} not found",
                    project_id
                )));
            };
            repo = p.repo_path;
            if base.is_empty() && !main {
                base = p.base_branch;
            }
        }
        let (root, writable) = recur_code::resolve_root_info(&repo, &wt)?;
        Ok((root, writable, base))
    }

    pub(crate) fn resolve_root(
        &self,
        project_id: &str,
        workspace_id: &str,
    ) -> std::result::Result<(PathBuf, bool), recur_code::Error> {
        self.resolve_scope(project_id, workspace_id)
            .map(|(root, writable, _)| (root, writable))
    }

    /// The sandbox root and relative path of a read. Absolute paths pass only
    /// when the user opened them (the loose allowlist, seeded from the
    /// workspace's saved editor tabs so restored tabs work after a restart).
    pub(crate) fn resolve_read_path(
        &self,
        project_id: &str,
        workspace_id: &str,
        path: &str,
    ) -> std::result::Result<(PathBuf, String), recur_code::Error> {
        if is_abs(path) {
            if !self.ensure_loose(workspace_id, path) {
                return Err(recur_code::Error::Msg(
                    "path is outside the workspace".into(),
                ));
            }
            return Ok(dir_base(path));
        }
        let (root, _) = self.resolve_root(project_id, workspace_id)?;
        Ok((root, path.to_owned()))
    }

    fn ensure_loose(&self, workspace_id: &str, abs: &str) -> bool {
        if self.loose.ok(abs) {
            return true;
        }
        if !workspace_id.is_empty()
            && let Ok(Some(sess)) = self.editor_session(workspace_id)
        {
            for f in sess.open_files.unwrap_or_default() {
                if is_abs(&f) {
                    self.loose.allow(&f);
                }
            }
        }
        self.loose.ok(abs)
    }

    /// Refuses worktree mutations while the workspace's run owns the tree
    /// (decision #3: refuse with the reason named). Paused, escalated and
    /// settled runs unlock.
    fn run_lock(&self, workspace_id: &str) -> Result<()> {
        if workspace_id.is_empty() {
            return Ok(());
        }
        if let Some(ws) = self.store().workspace(workspace_id)
            && ws.has_run()
            && run_active(&ws.state)
        {
            return Err(Error::new(
                "a loop is running in this workspace — the run owns the worktree; pause or stop it before editing",
            ));
        }
        Ok(())
    }

    /// A human edit on the workspace activity feed.
    pub(crate) fn record_human_edit(&self, workspace_id: &str, text: String) {
        if workspace_id.is_empty() {
            return;
        }
        let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
        let item = FeedItem {
            id: format!("human-edit-{nanos}"),
            agent_code: "AR".into(),
            name: "You".into(),
            text,
            time: "now".into(),
            ..FeedItem::default()
        };
        self.store().prepend_feed(workspace_id, item.clone());
        self.engine
            .bus
            .publish(EngineEvent::WorkspaceEvent(WorkspaceEvent {
                event: item,
                workspace_id: workspace_id.to_owned(),
            }));
    }

    fn writable_root(
        &self,
        project_id: &str,
        workspace_id: &str,
        refusal: &str,
    ) -> Result<PathBuf> {
        let (root, writable) = self.resolve_root(project_id, workspace_id).map_err(err)?;
        if !writable {
            return Err(Error::new(refusal));
        }
        Ok(root)
    }

    fn index_key(project_id: &str, workspace_id: &str) -> String {
        if workspace_id.is_empty() {
            project_id.to_owned()
        } else {
            workspace_id.to_owned()
        }
    }

    async fn read_file_at(
        &self,
        project_id: &str,
        workspace_id: &str,
        path: &str,
    ) -> Result<FileContent> {
        if is_abs(path) {
            let (root, rel) = self
                .resolve_read_path(project_id, workspace_id, path)
                .map_err(err)?;
            let mut fc =
                blocking(move || recur_code::read_file(&root, &rel, true).map_err(err)).await?;
            fc.path = path.to_owned();
            return Ok(fc);
        }
        let (root, writable) = self.resolve_root(project_id, workspace_id).map_err(err)?;
        let path = path.to_owned();
        blocking(move || recur_code::read_file(&root, &path, writable).map_err(err)).await
    }

    async fn write_file_at(
        &self,
        project_id: &str,
        workspace_id: &str,
        path: &str,
        content: String,
    ) -> Result<()> {
        if is_abs(path) {
            let (root, rel) = self
                .resolve_read_path(project_id, workspace_id, path)
                .map_err(err)?;
            self.run_lock(workspace_id)?;
            blocking(move || recur_code::write_file(&root, &rel, &content).map_err(err)).await?;
            self.record_human_edit(
                workspace_id,
                format!(
                    "Patched <code>{}</code> (not committed).",
                    html_escape(path)
                ),
            );
            return Ok(());
        }
        let root = self.writable_root(project_id, workspace_id, NOT_WRITABLE)?;
        self.run_lock(workspace_id)?;
        let rel = path.to_owned();
        blocking(move || recur_code::write_file(&root, &rel, &content).map_err(err)).await?;
        self.record_human_edit(
            workspace_id,
            format!(
                "Patched <code>{}</code> in the worktree (not committed).",
                html_escape(path)
            ),
        );
        Ok(())
    }

    async fn list_changed(&self, workspace_id: &str) -> Vec<String> {
        let mut out = BTreeSet::new();
        if let Some(w) = self.store().workspace(workspace_id)
            && let Ok((root, _, base)) = self.resolve_scope(&w.project_id, workspace_id)
        {
            out.extend(recur_code::git_status(&root, &base).await.into_keys());
        }
        if let Some(pr) = self.store().pr_by_workspace(workspace_id) {
            out.extend(pr.files.into_iter().flatten().map(|f| f.path));
        }
        out.remove("");
        out.into_iter().collect()
    }

    async fn file_diff_at(
        &self,
        project_id: &str,
        workspace_id: &str,
        path: &str,
    ) -> Result<FileDiff> {
        if is_abs(path) {
            let (root, rel) = self
                .resolve_read_path(project_id, workspace_id, path)
                .map_err(err)?;
            return recur_code::file_diff(&root, "", &rel).await.map_err(err);
        }
        let (root, _, base) = self.resolve_scope(project_id, workspace_id).map_err(err)?;
        recur_code::file_diff(&root, &base, path).await.map_err(err)
    }

    async fn diff_vs_base_at(
        &self,
        project_id: &str,
        workspace_id: &str,
    ) -> Result<FileDiffResult> {
        let (root, _, base) = self.resolve_scope(project_id, workspace_id).map_err(err)?;
        diff_working(&root, &base).await
    }

    async fn diff_working_at(
        &self,
        project_id: &str,
        workspace_id: &str,
    ) -> Result<FileDiffResult> {
        let root = match self.resolve_root(project_id, workspace_id) {
            Err(e) if worktree_missing(&e) => return Ok(empty_diff()),
            r => r.map_err(err)?.0,
        };
        diff_working(&root, "").await
    }

    async fn list_commits_at(&self, project_id: &str, workspace_id: &str) -> Result<Vec<Commit>> {
        let (root, _, base) = self.resolve_scope(project_id, workspace_id).map_err(err)?;
        let root = root.to_string_lossy().into_owned();
        if !recur_git::is_repo(&root).await {
            return Ok(Vec::new());
        }
        recur_git::Manager::new("", "")
            .log_commits(&root, &base)
            .await
            .map_err(err)
    }

    async fn commit_diff_at(
        &self,
        project_id: &str,
        workspace_id: &str,
        hash: &str,
    ) -> Result<Vec<FileDiff>> {
        let hash = hash.trim();
        if hash.is_empty() {
            return Err(Error::new("commit hash is required"));
        }
        let (root, _) = self.resolve_root(project_id, workspace_id).map_err(err)?;
        let root = root.to_string_lossy().into_owned();
        if !recur_git::is_repo(&root).await {
            return Err(Error::new("not a git repository"));
        }
        recur_git::Manager::new("", "")
            .commit_diff_first_parent(&root, hash)
            .await
            .map_err(err)
    }

    async fn replace_at(
        &self,
        cancel: CancellationToken,
        project_id: &str,
        workspace_id: &str,
        query: String,
        replacement: String,
        opts: SearchOpts,
    ) -> Result<ReplaceResult> {
        let root = self.writable_root(project_id, workspace_id, NOT_WRITABLE)?;
        self.run_lock(workspace_id)?;
        let res = blocking(move || {
            recur_code::replace_in_files(&cancel, &root, &query, &replacement, &opts).map_err(err)
        })
        .await?;
        if res.replacements > 0 {
            self.record_human_edit(
                workspace_id,
                format!(
                    "Replaced {} occurrence(s) in {} file(s) in the worktree (not committed).",
                    res.replacements, res.files_changed
                ),
            );
        }
        Ok(res)
    }

    fn list_files_at(&self, project_id: &str, workspace_id: &str) -> Result<Vec<String>> {
        let (root, _) = self.resolve_root(project_id, workspace_id).map_err(err)?;
        Ok(self.index.paths(
            &Self::index_key(project_id, workspace_id),
            &root.to_string_lossy(),
        ))
    }

    fn search_files_at(
        &self,
        cancel: &CancellationToken,
        project_id: &str,
        workspace_id: &str,
        query: &str,
        limit: i64,
        files_only: bool,
    ) -> Result<FileSearchResult> {
        let (root, _) = self.resolve_root(project_id, workspace_id).map_err(err)?;
        let limit = if limit <= 0 { 50 } else { limit as usize };
        Ok(self.index.search(
            cancel,
            &Self::index_key(project_id, workspace_id),
            &root.to_string_lossy(),
            query,
            limit,
            files_only,
        ))
    }

    fn refresh_index_at(&self, project_id: &str, workspace_id: &str) -> Result<Vec<String>> {
        let (root, _) = self.resolve_root(project_id, workspace_id).map_err(err)?;
        Ok(self
            .index
            .refresh(
                &Self::index_key(project_id, workspace_id),
                &root.to_string_lossy(),
            )
            .paths)
    }

    async fn list_skills_at(
        &self,
        cancel: CancellationToken,
        project_id: &str,
        workspace_id: &str,
    ) -> Result<Vec<AgentSkill>> {
        let (root, _) = self.resolve_root(project_id, workspace_id).map_err(err)?;
        let root = root.to_string_lossy().into_owned();
        let all =
            blocking(move || recur_code::list_skills_for_menu(&cancel, &root).map_err(err)).await?;
        let global = self.store().settings().disabled_skills;
        let (mut proj_disabled, mut proj_enabled) = (Vec::new(), Vec::new());
        if !project_id.is_empty()
            && let Some(p) = self.store().project(project_id)
        {
            proj_disabled = p.disabled_skills;
            proj_enabled = p.enabled_skills;
        }
        Ok(all
            .into_iter()
            .filter(|sk| effective_skill_enabled(&sk.name, &global, &proj_disabled, &proj_enabled))
            .collect())
    }

    async fn list_all_skills_at(
        &self,
        cancel: CancellationToken,
        project_id: &str,
        workspace_id: &str,
    ) -> Result<Vec<AgentSkill>> {
        let root = if !project_id.is_empty() || !workspace_id.is_empty() {
            let (r, _) = self.resolve_root(project_id, workspace_id).map_err(err)?;
            r.to_string_lossy().into_owned()
        } else {
            String::new()
        };
        blocking(move || recur_code::list_skills(&cancel, &root).map_err(err)).await
    }

    async fn working_status_at(
        &self,
        project_id: &str,
        workspace_id: &str,
    ) -> Result<BTreeMap<String, String>> {
        let root = match self.resolve_root(project_id, workspace_id) {
            Err(e) if worktree_missing(&e) => return Ok(BTreeMap::new()),
            r => r.map_err(err)?.0,
        };
        Ok(recur_git::working_status(&root.to_string_lossy())
            .await
            .map(|m| m.into_iter().collect())
            .unwrap_or_default())
    }

    async fn git_status_at(
        &self,
        project_id: &str,
        workspace_id: &str,
    ) -> Result<BTreeMap<String, String>> {
        let (root, _, base) = match self.resolve_scope(project_id, workspace_id) {
            Err(e) if worktree_missing(&e) => return Ok(BTreeMap::new()),
            r => r.map_err(err)?,
        };
        Ok(recur_code::git_status(&root, &base)
            .await
            .into_iter()
            .collect())
    }

    async fn commit_at(
        &self,
        project_id: &str,
        workspace_id: &str,
        message: &str,
        staged_only: bool,
    ) -> Result<WorktreeCommitResult> {
        let root = self.writable_root(
            project_id,
            workspace_id,
            "commits are only allowed in a loop worktree",
        )?;
        let root = root.to_string_lossy().into_owned();
        if !recur_git::is_repo(&root).await {
            return Err(Error::new("not a git repository"));
        }
        self.run_lock(workspace_id)?;
        let message = message.trim();
        if message.is_empty() {
            return Err(Error::new("commit message is required"));
        }
        let (name, email) = recur_git::identity(&root).await.map_err(err)?;
        let m = recur_git::Manager::new("", "");
        let res = if staged_only {
            m.commit_staged(&root, message, &name, &email).await
        } else {
            m.commit(&root, message, &name, &email).await
        }
        .map_err(err)?;
        let out = WorktreeCommitResult {
            hash: res.hash,
            committed: res.committed,
            additions: res.additions,
            deletions: res.deletions,
        };
        if out.committed {
            let what = if staged_only {
                "Committed staged"
            } else {
                "Committed"
            };
            self.record_human_edit(
                workspace_id,
                format!(
                    "{what} <code>{}</code>: {}",
                    html_escape(&out.hash),
                    html_escape(message)
                ),
            );
        }
        Ok(out)
    }

    async fn generate_commit_message_at(
        &self,
        cancel: &CancellationToken,
        project_id: &str,
        workspace_id: &str,
    ) -> Result<String> {
        let root = self.writable_root(
            project_id,
            workspace_id,
            "commit messages can only be generated in a loop worktree",
        )?;
        let root = root.to_string_lossy().into_owned();
        if !recur_git::is_repo(&root).await {
            return Err(Error::new("not a git repository"));
        }
        let diff = recur_git::staged_diff(&root).await.map_err(err)?;
        if diff.trim().is_empty() {
            return Err(Error::new("stage changes to generate a commit message"));
        }
        let subjects = recur_git::recent_commit_subjects(&root, 8).await;
        self.engine
            .generate_commit_message(cancel, workspace_id, &root, &diff, &subjects)
            .await
            .map_err(err)
    }

    async fn discard_at(
        &self,
        project_id: &str,
        workspace_id: &str,
        path: Option<&str>,
    ) -> Result<()> {
        let root = self.writable_root(
            project_id,
            workspace_id,
            "discard is only allowed in a loop worktree",
        )?;
        self.run_lock(workspace_id)?;
        let root = root.to_string_lossy().into_owned();
        match path {
            Some(path) => {
                recur_git::discard_file(&root, path).await.map_err(err)?;
                self.record_human_edit(
                    workspace_id,
                    format!("Discarded changes to <code>{}</code>", html_escape(path)),
                );
            }
            None => {
                recur_git::discard_all(&root).await.map_err(err)?;
                self.record_human_edit(
                    workspace_id,
                    "Discarded all uncommitted worktree changes".into(),
                );
            }
        }
        Ok(())
    }

    async fn stage_at(
        &self,
        project_id: &str,
        workspace_id: &str,
        paths: &[String],
        stage: bool,
    ) -> Result<()> {
        let refusal = if stage {
            "staging is only allowed in a loop worktree"
        } else {
            "unstaging is only allowed in a loop worktree"
        };
        let root = self.writable_root(project_id, workspace_id, refusal)?;
        self.run_lock(workspace_id)?;
        let root = root.to_string_lossy().into_owned();
        if stage {
            recur_git::stage(&root, paths).await
        } else {
            recur_git::unstage(&root, paths).await
        }
        .map_err(err)
    }

    async fn staged_files_at(&self, project_id: &str, workspace_id: &str) -> Result<Vec<String>> {
        let (root, writable) = match self.resolve_root(project_id, workspace_id) {
            Err(e) if worktree_missing(&e) => return Ok(Vec::new()),
            r => r.map_err(err)?,
        };
        if !writable {
            return Ok(Vec::new());
        }
        recur_git::staged_paths(&root.to_string_lossy())
            .await
            .map_err(err)
    }

    async fn mutate_worktree(
        &self,
        project_id: &str,
        workspace_id: &str,
        path: &str,
        verb: &str,
        f: impl FnOnce(&Path) -> recur_code::Result<()> + Send + 'static,
    ) -> Result<()> {
        let root = self.writable_root(project_id, workspace_id, NOT_WRITABLE)?;
        self.run_lock(workspace_id)?;
        blocking(move || f(&root).map_err(err)).await?;
        self.record_human_edit(
            workspace_id,
            format!(
                "{verb} <code>{}</code> in the worktree (not committed).",
                html_escape(path)
            ),
        );
        Ok(())
    }

    async fn move_at(
        &self,
        project_id: &str,
        workspace_id: &str,
        from: &str,
        to: &str,
        copy: bool,
    ) -> Result<()> {
        let root = self.writable_root(project_id, workspace_id, NOT_WRITABLE)?;
        self.run_lock(workspace_id)?;
        let (f, t) = (from.to_owned(), to.to_owned());
        blocking(move || {
            if copy {
                recur_code::copy(&root, &f, &t)
            } else {
                recur_code::rename(&root, &f, &t)
            }
            .map_err(err)
        })
        .await?;
        let verb = if copy { "Copied" } else { "Renamed" };
        self.record_human_edit(
            workspace_id,
            format!(
                "{verb} <code>{}</code> → <code>{}</code>",
                html_escape(from),
                html_escape(to)
            ),
        );
        Ok(())
    }

    fn joined(&self, project_id: &str, workspace_id: &str, path: &str) -> Result<String> {
        let (root, _) = self.resolve_root(project_id, workspace_id).map_err(err)?;
        let full = recur_code::safe_join(&root, path).map_err(err)?;
        Ok(full.to_string_lossy().into_owned())
    }
}

fn empty_diff() -> FileDiffResult {
    FileDiffResult {
        files: Some(Vec::new()),
        truncated: false,
    }
}

async fn diff_working(root: &Path, base: &str) -> Result<FileDiffResult> {
    let root = root.to_string_lossy().into_owned();
    if !recur_git::is_repo(&root).await {
        return Ok(empty_diff());
    }
    let pr = recur_git::Manager::new("", "")
        .diff_working_vs_base(&root, base)
        .await
        .map_err(err)?;
    Ok(FileDiffResult {
        files: Some(pr.files),
        truncated: pr.truncated,
    })
}

fn bench_out_path() -> PathBuf {
    std::env::temp_dir().join("recur-bridge-bench.json")
}

#[async_trait]
impl CodeService for AppServices {
    async fn abs_path(&self, project_id: &str, workspace_id: &str, path: &str) -> Result<String> {
        rt!(self, project_id, workspace_id, path => |s| {
            let (root, _) = s.resolve_root(&project_id, &workspace_id).map_err(err)?;
            recur_code::abs_path(&root, &path)
                .map(|p| p.to_string_lossy().into_owned())
                .map_err(err)
        })
    }

    async fn bridge_bench_done(&self, stats_json: &str) -> Result<()> {
        let path = bench_out_path();
        if let Err(e) = std::fs::write(&path, stats_json) {
            tracing::warn!("bridge-bench: write {}: {e}", path.display());
            return Ok(());
        }
        match serde_json::from_str::<serde_json::Value>(stats_json) {
            Ok(v) => tracing::info!(
                "bridge-bench results ({}):\n{}",
                path.display(),
                serde_json::to_string_pretty(&v).unwrap_or_default()
            ),
            Err(_) => tracing::info!("bridge-bench results written to {}", path.display()),
        }
        Ok(())
    }

    async fn bridge_bench_enabled(&self) -> Result<bool> {
        Ok(std::env::var("RECUR_BENCH_BRIDGE").as_deref() == Ok("1"))
    }

    async fn bridge_echo(&self, payload: &str) -> Result<i64> {
        Ok(payload.len() as i64)
    }

    async fn bridge_ping(&self, n: i64) -> Result<i64> {
        Ok(n)
    }

    async fn commit(
        &self,
        project_id: &str,
        workspace_id: &str,
        message: &str,
    ) -> Result<WorktreeCommitResult> {
        rt_ctx!(self, project_id, workspace_id, message => |s, _c| {
            s.commit_at(&project_id, &workspace_id, &message, false).await
        })
    }

    async fn commit_diff(
        &self,
        project_id: &str,
        workspace_id: &str,
        hash: &str,
    ) -> Result<Vec<FileDiff>> {
        rt_ctx!(self, project_id, workspace_id, hash => |s, _c| {
            s.commit_diff_at(&project_id, &workspace_id, &hash).await
        })
    }

    async fn commit_staged(
        &self,
        project_id: &str,
        workspace_id: &str,
        message: &str,
    ) -> Result<WorktreeCommitResult> {
        rt_ctx!(self, project_id, workspace_id, message => |s, _c| {
            s.commit_at(&project_id, &workspace_id, &message, true).await
        })
    }

    async fn copy(&self, project_id: &str, workspace_id: &str, from: &str, to: &str) -> Result<()> {
        rt!(self, project_id, workspace_id, from, to => |s| {
            s.move_at(&project_id, &workspace_id, &from, &to, true).await
        })
    }

    async fn create_file(&self, project_id: &str, workspace_id: &str, path: &str) -> Result<()> {
        rt!(self, project_id, workspace_id, path => |s| {
            let rel = path.clone();
            s.mutate_worktree(&project_id, &workspace_id, &path, "Created", move |root| {
                recur_code::create_file(root, &rel)
            })
            .await
        })
    }

    async fn delete(&self, project_id: &str, workspace_id: &str, path: &str) -> Result<()> {
        rt!(self, project_id, workspace_id, path => |s| {
            let rel = path.clone();
            s.mutate_worktree(&project_id, &workspace_id, &path, "Deleted", move |root| {
                recur_code::remove(root, &rel)
            })
            .await
        })
    }

    async fn diff_vs_base(&self, project_id: &str, workspace_id: &str) -> Result<FileDiffResult> {
        rt_ctx!(self, project_id, workspace_id => |s, _c| {
            s.diff_vs_base_at(&project_id, &workspace_id).await
        })
    }

    async fn diff_working(&self, project_id: &str, workspace_id: &str) -> Result<FileDiffResult> {
        rt_ctx!(self, project_id, workspace_id => |s, _c| {
            s.diff_working_at(&project_id, &workspace_id).await
        })
    }

    async fn discard_all(&self, project_id: &str, workspace_id: &str) -> Result<()> {
        rt_ctx!(self, project_id, workspace_id => |s, _c| {
            s.discard_at(&project_id, &workspace_id, None).await
        })
    }

    async fn discard_file(&self, project_id: &str, workspace_id: &str, path: &str) -> Result<()> {
        rt_ctx!(self, project_id, workspace_id, path => |s, _c| {
            s.discard_at(&project_id, &workspace_id, Some(&path)).await
        })
    }

    async fn file_at_base(
        &self,
        project_id: &str,
        workspace_id: &str,
        path: &str,
    ) -> Result<String> {
        rt_ctx!(self, project_id, workspace_id, path => |s, _c| {
            let (root, _, base) = s.resolve_scope(&project_id, &workspace_id).map_err(err)?;
            recur_code::file_at_base(&root, &base, &path).await.map_err(err)
        })
    }

    async fn file_diff(
        &self,
        project_id: &str,
        workspace_id: &str,
        path: &str,
    ) -> Result<FileDiff> {
        rt_ctx!(self, project_id, workspace_id, path => |s, _c| {
            s.file_diff_at(&project_id, &workspace_id, &path).await
        })
    }

    async fn find_defs(
        &self,
        project_id: &str,
        workspace_id: &str,
        name: &str,
    ) -> Result<Vec<SymbolDef>> {
        rt_ctx!(self, project_id, workspace_id, name => |s, c| {
            let (root, _) = s.resolve_root(&project_id, &workspace_id).map_err(err)?;
            blocking(move || recur_code::find_defs(&c, &root, &name).map_err(err)).await
        })
    }

    async fn find_refs(
        &self,
        project_id: &str,
        workspace_id: &str,
        name: &str,
    ) -> Result<Vec<SymbolRef>> {
        rt_ctx!(self, project_id, workspace_id, name => |s, c| {
            let (root, _) = s.resolve_root(&project_id, &workspace_id).map_err(err)?;
            blocking(move || recur_code::find_refs(&c, &root, &name).map_err(err)).await
        })
    }

    async fn generate_commit_message(
        &self,
        project_id: &str,
        workspace_id: &str,
    ) -> Result<String> {
        rt_ctx!(self, project_id, workspace_id => |s, c| {
            s.generate_commit_message_at(&c, &project_id, &workspace_id).await
        })
    }

    async fn git_status(
        &self,
        project_id: &str,
        workspace_id: &str,
    ) -> Result<BTreeMap<String, String>> {
        rt_ctx!(self, project_id, workspace_id => |s, _c| {
            s.git_status_at(&project_id, &workspace_id).await
        })
    }

    async fn list_all_skills(
        &self,
        project_id: &str,
        workspace_id: &str,
    ) -> Result<Vec<AgentSkill>> {
        rt_ctx!(self, project_id, workspace_id => |s, c| {
            s.list_all_skills_at(c, &project_id, &workspace_id).await
        })
    }

    async fn list_changed_files(&self, workspace_id: &str) -> Result<Vec<String>> {
        rt_ctx!(self, workspace_id => |s, _c| Ok(s.list_changed(&workspace_id).await))
    }

    async fn list_commits(&self, project_id: &str, workspace_id: &str) -> Result<Vec<Commit>> {
        rt_ctx!(self, project_id, workspace_id => |s, _c| {
            s.list_commits_at(&project_id, &workspace_id).await
        })
    }

    async fn list_files(&self, project_id: &str, workspace_id: &str) -> Result<Vec<String>> {
        rt_ctx!(self, project_id, workspace_id => |s, _c| s.list_files_at(&project_id, &workspace_id))
    }

    async fn list_skills(&self, project_id: &str, workspace_id: &str) -> Result<Vec<AgentSkill>> {
        rt_ctx!(self, project_id, workspace_id => |s, c| {
            s.list_skills_at(c, &project_id, &workspace_id).await
        })
    }

    async fn list_tree(
        &self,
        project_id: &str,
        workspace_id: &str,
        path: &str,
    ) -> Result<Vec<TreeEntry>> {
        rt_ctx!(self, project_id, workspace_id, path => |s, _c| {
            let (root, _, base) = s.resolve_scope(&project_id, &workspace_id).map_err(err)?;
            recur_code::list_tree(&root, &path, 1, &base).await.map_err(err)
        })
    }

    async fn list_trees(
        &self,
        project_id: &str,
        workspace_id: &str,
        dirs: &[String],
    ) -> Result<BTreeMap<String, Vec<TreeEntry>>> {
        rt_ctx!(self, project_id, workspace_id, dirs => |s, _c| {
            let (root, _, base) = s.resolve_scope(&project_id, &workspace_id).map_err(err)?;
            recur_code::list_trees(&root, &dirs, 1, &base).await.map_err(err)
        })
    }

    async fn mkdir(&self, project_id: &str, workspace_id: &str, path: &str) -> Result<()> {
        rt!(self, project_id, workspace_id, path => |s| {
            let rel = path.clone();
            s.mutate_worktree(&project_id, &workspace_id, &path, "Created folder", move |root| {
                recur_code::mkdir(root, &rel)
            })
            .await
        })
    }

    async fn open_in_editor(
        &self,
        project_id: &str,
        workspace_id: &str,
        path: &str,
        line: i64,
    ) -> Result<()> {
        rt!(self, project_id, workspace_id, path, line => |s| {
            let full = s.joined(&project_id, &workspace_id, &path)?;
            recur_code::open_in_editor(&full, line).map_err(err)
        })
    }

    async fn outline(
        &self,
        project_id: &str,
        workspace_id: &str,
        path: &str,
    ) -> Result<Vec<OutlineSymbol>> {
        rt!(self, project_id, workspace_id, path => |s| {
            let (root, rel) = s
                .resolve_read_path(&project_id, &workspace_id, &path)
                .map_err(err)?;
            blocking(move || recur_code::outline(&root, &rel).map_err(err)).await
        })
    }

    async fn read_file(
        &self,
        project_id: &str,
        workspace_id: &str,
        path: &str,
    ) -> Result<FileContent> {
        rt!(self, project_id, workspace_id, path => |s| {
            s.read_file_at(&project_id, &workspace_id, &path).await
        })
    }

    async fn refresh_file_index(
        &self,
        project_id: &str,
        workspace_id: &str,
    ) -> Result<Vec<String>> {
        rt_ctx!(self, project_id, workspace_id => |s, _c| s.refresh_index_at(&project_id, &workspace_id))
    }

    async fn rename(
        &self,
        project_id: &str,
        workspace_id: &str,
        from: &str,
        to: &str,
    ) -> Result<()> {
        rt!(self, project_id, workspace_id, from, to => |s| {
            s.move_at(&project_id, &workspace_id, &from, &to, false).await
        })
    }

    async fn replace_in_files(
        &self,
        project_id: &str,
        workspace_id: &str,
        query: &str,
        replacement: &str,
        opts: &SearchOpts,
    ) -> Result<ReplaceResult> {
        rt_ctx!(self, project_id, workspace_id, query, replacement, opts => |s, c| {
            s.replace_at(c, &project_id, &workspace_id, query, replacement, opts).await
        })
    }

    async fn reveal_in_finder(
        &self,
        project_id: &str,
        workspace_id: &str,
        path: &str,
    ) -> Result<()> {
        rt!(self, project_id, workspace_id, path => |s| {
            let full = s.joined(&project_id, &workspace_id, &path)?;
            recur_code::reveal_in_finder(&full).map_err(err)
        })
    }

    async fn search(
        &self,
        project_id: &str,
        workspace_id: &str,
        query: &str,
    ) -> Result<Vec<SearchHit>> {
        rt_ctx!(self, project_id, workspace_id, query => |s, c| {
            let (root, _) = s.resolve_root(&project_id, &workspace_id).map_err(err)?;
            blocking(move || {
                recur_code::search(&c, &root, &query)
                    .map(Option::unwrap_or_default)
                    .map_err(err)
            })
            .await
        })
    }

    async fn search_advanced(
        &self,
        project_id: &str,
        workspace_id: &str,
        query: &str,
        opts: &SearchOpts,
    ) -> Result<SearchResult> {
        rt_ctx!(self, project_id, workspace_id, query, opts => |s, c| {
            let (root, _) = s.resolve_root(&project_id, &workspace_id).map_err(err)?;
            blocking(move || recur_code::search_opts(&c, &root, &query, &opts).map_err(err)).await
        })
    }

    async fn search_files(
        &self,
        project_id: &str,
        workspace_id: &str,
        query: &str,
        limit: i64,
        files_only: bool,
    ) -> Result<FileSearchResult> {
        rt_ctx!(self, project_id, workspace_id, query, limit, files_only => |s, c| {
            s.search_files_at(&c, &project_id, &workspace_id, &query, limit, files_only)
        })
    }

    async fn set_file_index_exclude(&self, patterns: &[String]) -> Result<()> {
        rt_ctx!(self, patterns => |s, _c| {
            s.index.set_exclude(&patterns);
            Ok(())
        })
    }

    async fn stage(&self, project_id: &str, workspace_id: &str, paths: &[String]) -> Result<()> {
        rt_ctx!(self, project_id, workspace_id, paths => |s, _c| {
            s.stage_at(&project_id, &workspace_id, &paths, true).await
        })
    }

    async fn staged_files(&self, project_id: &str, workspace_id: &str) -> Result<Vec<String>> {
        rt_ctx!(self, project_id, workspace_id => |s, _c| {
            s.staged_files_at(&project_id, &workspace_id).await
        })
    }

    async fn unstage(&self, project_id: &str, workspace_id: &str, paths: &[String]) -> Result<()> {
        rt_ctx!(self, project_id, workspace_id, paths => |s, _c| {
            s.stage_at(&project_id, &workspace_id, &paths, false).await
        })
    }

    async fn working_status(
        &self,
        project_id: &str,
        workspace_id: &str,
    ) -> Result<BTreeMap<String, String>> {
        rt_ctx!(self, project_id, workspace_id => |s, _c| {
            s.working_status_at(&project_id, &workspace_id).await
        })
    }

    async fn write_file(
        &self,
        project_id: &str,
        workspace_id: &str,
        path: &str,
        content: &str,
    ) -> Result<()> {
        rt!(self, project_id, workspace_id, path, content => |s| {
            s.write_file_at(&project_id, &workspace_id, &path, content).await
        })
    }
}
