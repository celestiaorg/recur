//! The project's "main working copy" workspace (Go `OpenMainWorkspace`): no
//! worktree, no branch of its own; the path is the real checkout, so the
//! Observatory edits and commits land in the user's repo. Never runs setup,
//! never engages the autopilot, never has its checkout removed.

use std::time::Duration;

use recur_domain::{Group, WORKSPACE_MAIN, Workspace};

use crate::Engine;
use crate::core::{EngineError, Result};

impl Engine {
    /// Returns the project's main working copy, creating it on first open
    /// and repairing a stored one against the project's current checkout
    /// (RepoPath is editable) and live branch.
    pub async fn open_main_workspace(&self, project_id: &str) -> Result<Workspace> {
        let proj = self
            .store
            .project(project_id)
            .ok_or_else(|| EngineError(format!("project {project_id:?} not found")))?;
        if proj.repo_path.trim().is_empty() {
            return Err(EngineError(format!(
                "project {project_id:?} has no repo path"
            )));
        }
        let root = abs_path(&recur_git::expand_home(&proj.repo_path))
            .map_err(|e| EngineError(format!("resolve repo path: {e}")))?;
        if !std::fs::metadata(&root).is_ok_and(|m| m.is_dir()) {
            return Err(EngineError(format!(
                "repository path {:?} is not a directory",
                proj.repo_path
            )));
        }

        // Serialize find-or-create: two concurrent opens must not both add a
        // main row (the store has no uniqueness on (project, main)).
        let _open = self.open_main_mu.lock().await;

        // The live branch of a git root. A probe failure on a real repo keeps
        // the stored branch; non-git folders clear it.
        let is_git = recur_git::is_repo(&root).await;
        let mut branch = String::new();
        let mut branch_ok = false;
        if is_git
            && let Ok(Ok(b)) =
                tokio::time::timeout(Duration::from_secs(10), recur_git::current_branch(&root))
                    .await
        {
            branch = b;
            branch_ok = true;
        }

        for w in self.store.workspaces() {
            if !w.is_main || w.project_id != project_id {
                continue;
            }
            let mut want_branch = w.branch.clone();
            if !is_git {
                want_branch.clear();
            } else if branch_ok {
                want_branch = branch.clone();
            }
            if w.worktree_path == root && w.branch == want_branch {
                return Ok(w);
            }
            let Some(updated) = self.store.update_workspace(&w.id, |m| {
                m.worktree_path = root.clone();
                if !is_git {
                    m.branch.clear();
                } else if branch_ok {
                    m.branch = branch.clone();
                }
            }) else {
                return Ok(w);
            };
            self.publish_loop_updated(
                &updated.id,
                updated.state.clone(),
                &updated.phase,
                updated.iteration,
            );
            return Ok(updated);
        }

        // A free port so Run dev works against the checkout.
        let port = recur_project::runconfig::free_port()
            .map_err(|e| EngineError(format!("allocate port: {e}")))?;
        let main_branch = if is_git && branch_ok {
            branch
        } else {
            String::new()
        };
        let id = self.new_workspace_id(project_id, None).await;
        let rec = Workspace {
            id,
            project_id: project_id.to_owned(),
            r#type: WORKSPACE_MAIN.into(),
            is_main: true,
            group: Group::Active,
            goal: "Working copy".into(),
            short: "Working copy".into(),
            status_label: "Idle".into(),
            phase: "editing".into(),
            branch: main_branch,
            // Uncommitted-vs-HEAD baseline.
            base_branch: String::new(),
            worktree_path: root,
            port: i64::from(port),
            last: "opened".into(),
            elapsed_label: "just now".into(),
            ..Workspace::default()
        };
        let saved = self
            .store
            .add_workspace(rec)
            .map_err(|e| EngineError(e.to_string()))?;
        self.publish_loop_updated(
            &saved.id,
            saved.state.clone(),
            &saved.phase,
            saved.iteration,
        );
        Ok(saved)
    }
}

/// Go `filepath.Abs`: joins a relative path onto the cwd and cleans.
fn abs_path(p: &str) -> std::io::Result<String> {
    let joined = if p.starts_with('/') {
        p.to_owned()
    } else {
        format!("{}/{p}", std::env::current_dir()?.to_string_lossy())
    };
    let rooted = joined.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    let s = out.join("/");
    Ok(if rooted { format!("/{s}") } else { s })
}
