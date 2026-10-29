//! `OpenPath` (Go `open_path.go`): open a file or folder in Recur. Find or
//! create its project, focus the working copy or the matching workspace, and
//! jump to the file. Homeless files become absolute-path tabs on the
//! singleton Files project instead of adding their folder as a project.

use recur_domain::path;
use recur_domain::{FILES_PROJECT_ID, OpenPathResult, Project, Workspace};
use recur_git::{align_under_root, canon_path, expand_home, path_has_prefix, same_path};

use super::{AppServices, err};
use crate::Result;
use crate::events::AppEvent;

/// `abs` relative to `root` with forward slashes, `""` for the root itself,
/// and `abs` unchanged when it is outside `root`.
fn rel_or_abs(root: &str, abs: &str) -> String {
    let abs = align_under_root(abs, root);
    if same_path(root, &abs) {
        return String::new();
    }
    let rel = recur_code::rel_slash(std::path::Path::new(root), std::path::Path::new(&abs));
    if rel == "." {
        return String::new();
    }
    if rel.starts_with("..") {
        return abs;
    }
    rel
}

impl AppServices {
    /// Go `OpenPath`: serialized, then published as `app.open`.
    pub(crate) async fn open_path_at(&self, path: &str) -> Result<OpenPathResult> {
        let _guard = self.open_mu.lock().await;
        let res = self.open_path_locked(path).await?;
        self.hub.publish(AppEvent::AppOpen(res.clone()));
        Ok(res)
    }

    async fn open_path_locked(&self, path: &str) -> Result<OpenPathResult> {
        let abs = canon_path(path).map_err(err)?;
        let st = std::fs::metadata(&abs).map_err(|e| err(format!("{abs}: {e}")))?;
        let is_file = !st.is_dir();
        let dir = if is_file {
            path::dir(&abs)
        } else {
            abs.clone()
        };
        let res = OpenPathResult {
            path: abs.clone(),
            ..Default::default()
        };

        // 1. The longest matching workspace worktree (managed or main).
        if let Some(ws) = self.best_workspace_for(&abs) {
            return Ok(self.finish_in_workspace(res, ws, &abs, is_file).await);
        }
        // 2. The longest matching project root.
        if let Some(p) = self.best_project_for(&dir) {
            return self.finish_in_project(res, p, &abs, is_file, false).await;
        }
        // 3. A git checkout Recur does not know yet: adopt its toplevel.
        if let Some(top) = recur_git::repo_root(&dir).await {
            if let Some(p) = self.best_project_for(&top) {
                return self.finish_in_project(res, p, &abs, is_file, false).await;
            }
            let (p, created) = self.ensure_project_at(&top).await?;
            return self.finish_in_project(res, p, &abs, is_file, created).await;
        }
        // 4. A folder becomes a (possibly non-git) project.
        if !is_file {
            let (p, created) = self.ensure_project_at(&abs).await?;
            return self.finish_in_project(res, p, &abs, false, created).await;
        }
        // 5. A homeless file: the Files project plus an absolute-path tab.
        self.finish_loose_file(res, &abs).await
    }

    async fn finish_in_workspace(
        &self,
        mut res: OpenPathResult,
        ws: Workspace,
        abs: &str,
        is_file: bool,
    ) -> OpenPathResult {
        let root = canon_path(&ws.worktree_path).unwrap_or_else(|_| path::clean(&ws.worktree_path));
        res.project_id.clone_from(&ws.project_id);
        res.workspace_id.clone_from(&ws.id);
        if let Some(p) = self.store().project(&ws.project_id) {
            res.project = Some(Box::new(self.with_is_git(p).await));
        }
        res.workspace = Some(Box::new(ws));
        if is_file {
            res.file = rel_or_abs(&root, abs);
            if res.file.starts_with('/') {
                self.loose.allow(abs);
                res.loose = true;
            }
        } else if !same_path(abs, &root) {
            res.reveal = rel_or_abs(&root, abs);
        }
        res
    }

    async fn finish_in_project(
        &self,
        mut res: OpenPathResult,
        p: Project,
        abs: &str,
        is_file: bool,
        created: bool,
    ) -> Result<OpenPathResult> {
        let ws = self.engine.open_main_workspace(&p.id).await.map_err(err)?;
        let p = self.with_is_git(p).await;
        res.created = created;
        res.project_id.clone_from(&p.id);
        res.workspace_id.clone_from(&ws.id);
        let root =
            canon_path(&p.repo_path).unwrap_or_else(|_| path::clean(&expand_home(&p.repo_path)));
        res.project = Some(Box::new(p));
        res.workspace = Some(Box::new(ws));
        if is_file {
            res.file = rel_or_abs(&root, abs);
        } else if !same_path(abs, &root) {
            res.reveal = rel_or_abs(&root, abs);
        }
        Ok(res)
    }

    async fn finish_loose_file(
        &self,
        mut res: OpenPathResult,
        abs: &str,
    ) -> Result<OpenPathResult> {
        let (p, created) = self.ensure_files_project().await?;
        let ws = self.engine.open_main_workspace(&p.id).await.map_err(err)?;
        self.loose.allow(abs);
        let p = self.with_is_git(p).await;
        res.created = created;
        res.loose = true;
        res.project_id.clone_from(&p.id);
        res.workspace_id.clone_from(&ws.id);
        res.file = abs.to_owned();
        res.project = Some(Box::new(p));
        res.workspace = Some(Box::new(ws));
        Ok(res)
    }

    fn best_workspace_for(&self, abs: &str) -> Option<Workspace> {
        let mut best: Option<(usize, Workspace)> = None;
        for w in self.store().workspaces() {
            if w.archived || w.is_pr_chat_host() || w.worktree_path.trim().is_empty() {
                continue;
            }
            let Ok(root) = canon_path(&w.worktree_path) else {
                continue;
            };
            if root.is_empty() || !path_has_prefix(abs, &root) {
                continue;
            }
            if best.as_ref().is_none_or(|(n, _)| root.len() > *n) {
                best = Some((root.len(), w));
            }
        }
        best.map(|(_, w)| w)
    }

    fn best_project_for(&self, abs: &str) -> Option<Project> {
        let mut best: Option<(usize, Project)> = None;
        for p in self.store().projects().unwrap_or_default() {
            if p.repo_path.trim().is_empty() {
                continue;
            }
            let Ok(root) = canon_path(&p.repo_path) else {
                continue;
            };
            if root.is_empty() || !path_has_prefix(abs, &root) {
                continue;
            }
            if best.as_ref().is_none_or(|(n, _)| root.len() > *n) {
                best = Some((root.len(), p));
            }
        }
        best.map(|(_, p)| p)
    }

    async fn ensure_project_at(&self, abs: &str) -> Result<(Project, bool)> {
        if let Some(p) = self.best_project_for(abs)
            && canon_path(&p.repo_path).is_ok_and(|root| same_path(&root, abs))
        {
            return Ok((p, false));
        }
        let mut name = path::base(abs);
        if name.is_empty() || name == "/" || name == "." {
            name = "folder".into();
        }
        let saved = self
            .add_project_at(Project {
                name,
                repo_path: abs.to_owned(),
                ..Default::default()
            })
            .await?;
        Ok((saved, true))
    }

    async fn ensure_files_project(&self) -> Result<(Project, bool)> {
        if let Some(p) = self.store().project(FILES_PROJECT_ID) {
            return Ok((p, false));
        }
        let dir = self.data_dir().join("open-files");
        std::fs::create_dir_all(&dir).map_err(err)?;
        let saved = self
            .add_project_at(Project {
                id: FILES_PROJECT_ID.into(),
                name: "Files".into(),
                pic: "F".into(),
                color: "var(--indigo)".into(),
                repo_path: dir.to_string_lossy().into_owned(),
                ..Default::default()
            })
            .await?;
        Ok((saved, true))
    }
}
