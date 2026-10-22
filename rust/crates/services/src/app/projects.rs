//! `ProjectsService` (Go `services.go` projects half and `schedules.go`):
//! projects and sidebar groups, base-branch pickers, copy-file previews and
//! scheduled prompts.

use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt as _;
use recur_bundles::go::quote;
use recur_domain::{
    BranchChoice, CopyFilesPreview, FILES_PROJECT_ID, Project, ProjectGroup, ScheduledPrompt,
    next_scheduled_time,
};
use recur_git::LOCAL_BRANCH_GROUP;
use regex::Regex;
use tokio_util::sync::CancellationToken;

use super::rt::{blocking, rt};
use super::{AppServices, err};
use crate::api::{ProjectsService, SettingsService};
use crate::{Error, Result};

/// Parallel `is_repo` probes when listing projects, so boot does not spawn
/// one git process per project unbounded.
const IS_GIT_PROBE_CONCURRENCY: usize = 8;

/// Sidebar avatar tints, assigned round-robin to new projects.
const PROJECT_COLORS: [&str; 5] = [
    "var(--cyan)",
    "var(--amber)",
    "var(--indigo)",
    "var(--emerald)",
    "var(--coral)",
];

/// Branch listings share one 10 s budget (Go's `context.WithTimeout`).
const BRANCH_TIMEOUT: Duration = Duration::from_secs(10);

static SLUG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new("[^a-z0-9]+").expect("slug re"));

/// Lowercases `s`, keeps a path's last segment, and collapses
/// non-alphanumerics into single hyphens.
pub(crate) fn slugify(s: &str) -> String {
    let s = s.trim().to_lowercase();
    let s = match s.rfind('/') {
        Some(i) => &s[i + 1..],
        None => &s,
    };
    SLUG_RE.replace_all(s, "-").trim_matches('-').to_owned()
}

fn first_non_empty<'a>(vals: &[&'a str]) -> &'a str {
    vals.iter()
        .copied()
        .find(|v| !v.trim().is_empty())
        .unwrap_or("")
}

/// Go `strings.ToUpper(string(r))` for the first rune: the simple case
/// mapping, so a rune whose uppercase is several characters stays as is.
fn avatar_letter(name: &str) -> String {
    let Some(c) = name.chars().next() else {
        return String::new();
    };
    let mut up = c.to_uppercase();
    match (up.next(), up.next()) {
        (Some(u), None) => u.to_string(),
        _ => c.to_string(),
    }
}

/// The base branch to suggest for a checkout, bounded like Go's 10 s context.
pub(crate) async fn suggest_base_branch(repo_path: &str) -> String {
    tokio::time::timeout(
        BRANCH_TIMEOUT,
        recur_git::suggest_base_branch_for_repo(repo_path),
    )
    .await
    .unwrap_or_else(|_| "main".into())
}

/// The grouped base-branch picker for a checkout (Go `listBranchChoices`).
/// `def` is promoted within its group, never prepended as its own heading.
/// With `fold_missing` (a saved project) a configured default absent from
/// the repo is still listed; otherwise only refs that resolve are folded in.
pub(crate) async fn list_branch_choices(
    repo_path: &str,
    def: &str,
    fold_missing: bool,
) -> Vec<BranchChoice> {
    let def = def.trim();
    // Non-git folders host no workspaces: no invented "main".
    if repo_path.is_empty() || !recur_git::is_repo(repo_path).await {
        return Vec::new();
    }
    let fallback = || {
        if fold_missing && !def.is_empty() {
            vec![BranchChoice {
                name: def.to_owned(),
                group: LOCAL_BRANCH_GROUP.into(),
            }]
        } else {
            Vec::new()
        }
    };
    let mgr = recur_git::Manager::new(repo_path, "");
    let work = async {
        let mut remotes = mgr.remotes().await.unwrap_or_default();
        remotes.sort();
        let remote_branches = mgr.list_remote_branches().await.unwrap_or_default();
        let Ok(mut local) = mgr.list_local_branches().await else {
            return None;
        };
        let mut by_remote: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for b in remote_branches {
            if let Some((remote, _)) = recur_git::parse_remote_tracking_ref(&remotes, &b) {
                by_remote.entry(remote).or_default().push(b);
            }
        }
        let inject = !def.is_empty() && (fold_missing || mgr.commit_exists(def).await);
        if inject {
            if let Some((remote, _)) = recur_git::parse_remote_tracking_ref(&remotes, def) {
                let list = by_remote.entry(remote.clone()).or_default();
                if !list.iter().any(|b| b == def) {
                    list.insert(0, def.to_owned());
                }
                if !remotes.contains(&remote) {
                    remotes.push(remote);
                    remotes.sort();
                }
            } else if !local.iter().any(|b| b == def) {
                local.insert(0, def.to_owned());
            }
        }
        Some((remotes, by_remote, local))
    };
    let Ok(Some((remotes, by_remote, local))) = tokio::time::timeout(BRANCH_TIMEOUT, work).await
    else {
        return fallback();
    };

    let mut out: Vec<BranchChoice> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut append_group = |group: &str, names: &[String]| {
        let ordered = names
            .iter()
            .filter(|n| *n == def)
            .take(1)
            .chain(names.iter().filter(|n| *n != def));
        for n in ordered {
            if seen.insert(n.clone()) {
                out.push(BranchChoice {
                    name: n.clone(),
                    group: group.to_owned(),
                });
            }
        }
    };
    for remote in &remotes {
        if let Some(names) = by_remote.get(remote) {
            append_group(remote, names);
        }
    }
    // BTreeMap iteration is the sorted order Go applied to extra remotes.
    for (remote, names) in &by_remote {
        if !remotes.contains(remote) {
            append_group(remote, names);
        }
    }
    append_group(LOCAL_BRANCH_GROUP, &local);
    if out.is_empty() {
        return fallback();
    }
    out
}

impl AppServices {
    /// `is_git` is computed (not persisted) for UI gating of worktree
    /// creation.
    pub(crate) async fn with_is_git(&self, mut p: Project) -> Project {
        p.is_git = recur_git::is_repo(&p.repo_path).await;
        p
    }

    pub(crate) async fn list_projects_sync(&self) -> Vec<Project> {
        let projects = self.store().projects().unwrap_or_default();
        futures::stream::iter(projects)
            .map(|mut p| async move {
                p.is_git = recur_git::is_repo(&p.repo_path).await;
                p
            })
            .buffered(IS_GIT_PROBE_CONCURRENCY)
            .collect()
            .await
    }

    pub(crate) async fn get_project_at(&self, id: &str) -> Result<Project> {
        let p = self
            .store()
            .project(id)
            .ok_or_else(|| Error::new(format!("project {} not found", quote(id))))?;
        Ok(self.with_is_git(p).await)
    }

    fn taken_project_id(&self, id: &str) -> bool {
        id == FILES_PROJECT_ID || self.store().has_project(id)
    }

    fn unique_project_id(&self, base: &str) -> String {
        let base = if base.is_empty() { "project" } else { base };
        let mut id = base.to_owned();
        let mut i = 2;
        while self.taken_project_id(&id) {
            id = format!("{base}-{i}");
            i += 1;
        }
        id
    }

    fn check_group(&self, group_id: &str) -> Result<()> {
        if !group_id.is_empty() && !self.store().has_project_group(group_id) {
            return Err(Error::new(format!(
                "project group {} not found",
                quote(group_id)
            )));
        }
        Ok(())
    }

    /// Go `AddProject`.
    pub(crate) async fn add_project_at(&self, mut p: Project) -> Result<Project> {
        if p.id.is_empty() {
            p.id = self.unique_project_id(&slugify(first_non_empty(&[&p.name, &p.repo_path])));
        }
        if p.name.is_empty() {
            p.name.clone_from(&p.id);
        }
        if p.pic.is_empty() && !p.name.is_empty() {
            p.pic = avatar_letter(&p.name);
        }
        if p.color.is_empty() {
            let n = self.store().projects().map_or(0, |v| v.len());
            p.color = PROJECT_COLORS[n % PROJECT_COLORS.len()].into();
        }
        self.check_group(&p.group_id)?;
        // Sidebar default on create only: an upsert keeps the caller's Open.
        if !self.store().has_project(&p.id) {
            p.open = true;
        }
        if p.base_branch.trim().is_empty() {
            p.base_branch = suggest_base_branch(&p.repo_path).await;
        }
        let saved = self.store().add_project(p).map_err(err)?;
        let saved = self.with_is_git(saved).await;
        // Seed the working-copy row from the real checkout so the sidebar does
        // not fall back to Project.BaseBranch (often origin/…, not HEAD).
        let _ = self.engine.open_main_workspace(&saved.id).await;
        Ok(saved)
    }

    async fn preview_copy_files_at(&self, project_id: &str, patterns: String) -> CopyFilesPreview {
        let p = match self.get_project_at(project_id).await {
            Ok(p) => p,
            Err(e) => {
                return CopyFilesPreview {
                    error: e.message,
                    ..Default::default()
                };
            }
        };
        blocking(move || {
            Ok(recur_code::copyfiles::preview(
                &CancellationToken::new(),
                None,
                &p.repo_path,
                &patterns,
            ))
        })
        .await
        .unwrap_or_default()
    }

    async fn save_scheduled_prompt_at(&self, p: ScheduledPrompt) -> Result<ScheduledPrompt> {
        let catalog = self.list_model_catalog().await?;
        if catalog
            .iter()
            .any(|g| g.models.iter().any(|m| m.model_ref == p.model))
        {
            return self.engine.save_scheduled_prompt(p).await.map_err(err);
        }
        Err(Error::new("choose an available model"))
    }
}

/// The next three run times (Go `PreviewScheduledPrompt`).
pub(crate) fn preview_scheduled_prompt(
    p: &ScheduledPrompt,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<i64>> {
    let mut out = Vec::new();
    let mut after = now;
    for _ in 0..3 {
        match next_scheduled_time(p, after).map_err(err)? {
            Some(next) => {
                out.push(next.timestamp_millis());
                after = next.with_timezone(&chrono::Utc);
            }
            None => {
                if out.is_empty() && !p.on_startup() {
                    return Err(Error::new("choose a future date and time"));
                }
                break;
            }
        }
    }
    Ok(out)
}

#[async_trait]
impl ProjectsService for AppServices {
    async fn add_project(&self, p: &Project) -> Result<Project> {
        rt!(self, p => |s| s.add_project_at(p).await)
    }

    async fn add_project_group(&self, g: &ProjectGroup) -> Result<ProjectGroup> {
        rt!(self, g => |s| {
            let mut g = g;
            if g.name.is_empty() {
                g.name = "New group".into();
            }
            if g.id.is_empty() {
                g.id = uuid::Uuid::now_v7().to_string();
            }
            if !s.store().has_project_group(&g.id) {
                g.open = true;
            }
            s.store().add_project_group(g).map_err(err)
        })
    }

    async fn delete_project(&self, id: &str) -> Result<()> {
        rt!(self, id => |s| s.engine.delete_project(&id).await.map_err(err))
    }

    async fn delete_project_group(&self, id: &str) -> Result<()> {
        rt!(self, id => |s| s.store().delete_project_group(&id).map_err(err))
    }

    async fn delete_scheduled_prompt(&self, id: &str) -> Result<()> {
        rt!(self, id => |s| s.engine.delete_scheduled_prompt(&id).await.map_err(err))
    }

    async fn get_project(&self, id: &str) -> Result<Project> {
        rt!(self, id => |s| s.get_project_at(&id).await)
    }

    async fn list_branches(&self, project_id: &str) -> Result<Vec<BranchChoice>> {
        rt!(self, project_id => |s| {
            let p = s.store().project(&project_id).ok_or_else(|| {
                Error::new(format!("project {} not found", quote(&project_id)))
            })?;
            let def = if p.base_branch.is_empty() { "main".to_owned() } else { p.base_branch };
            // A saved default may name a deleted branch: still show it.
            Ok(list_branch_choices(&p.repo_path, &def, true).await)
        })
    }

    async fn list_branches_for_path(
        &self,
        repo_path: &str,
        preferred: &str,
    ) -> Result<Vec<BranchChoice>> {
        rt!(self, repo_path, preferred => |_s| {
            Ok(list_branch_choices(&repo_path, preferred.trim(), false).await)
        })
    }

    async fn list_project_groups(&self) -> Result<Vec<ProjectGroup>> {
        rt!(self, => |s| Ok(s.store().project_groups().unwrap_or_default()))
    }

    async fn list_projects(&self) -> Result<Vec<Project>> {
        rt!(self, => |s| Ok(s.list_projects_sync().await))
    }

    async fn list_scheduled_prompts(&self, project_id: &str) -> Result<Vec<ScheduledPrompt>> {
        rt!(self, project_id => |s| {
            let all = s.store().scheduled_prompts().map_err(err)?;
            Ok(all
                .into_iter()
                .filter(|p| project_id.is_empty() || p.project_id == project_id)
                .collect())
        })
    }

    async fn pause_scheduled_prompt(&self, id: &str, paused: bool) -> Result<()> {
        rt!(self, id, paused => |s| s.engine.pause_scheduled_prompt(&id, paused).await.map_err(err))
    }

    async fn preview_copy_files(
        &self,
        project_id: &str,
        patterns: &str,
    ) -> Result<CopyFilesPreview> {
        rt!(self, project_id, patterns => |s| Ok(s.preview_copy_files_at(&project_id, patterns).await))
    }

    async fn preview_scheduled_prompt(&self, p: &ScheduledPrompt) -> Result<Vec<i64>> {
        preview_scheduled_prompt(p, chrono::Utc::now())
    }

    async fn reorder_project_groups(&self, ids: &[String]) -> Result<()> {
        rt!(self, ids => |s| s.store().reorder_project_groups(&ids).map_err(err))
    }

    async fn reorder_projects(&self, ids: &[String]) -> Result<()> {
        rt!(self, ids => |s| s.store().reorder_projects(&ids).map_err(err))
    }

    async fn run_scheduled_prompt_now(&self, id: &str) -> Result<String> {
        rt!(self, id => |s| s.engine.run_scheduled_prompt_now(&id).await.map_err(err))
    }

    async fn save_scheduled_prompt(&self, p: &ScheduledPrompt) -> Result<ScheduledPrompt> {
        rt!(self, p => |s| s.save_scheduled_prompt_at(p).await)
    }

    async fn set_project_group(&self, project_id: &str, group_id: &str) -> Result<Project> {
        rt!(self, project_id, group_id => |s| {
            let mut p = s.store().project(&project_id).ok_or_else(|| {
                Error::new(format!("project {} not found", quote(&project_id)))
            })?;
            s.check_group(&group_id)?;
            p.group_id = group_id;
            let saved = s.store().update_project_run_config(p).map_err(err)?;
            Ok(s.with_is_git(saved).await)
        })
    }

    async fn suggest_base_branch(&self, repo_path: &str) -> Result<String> {
        rt!(self, repo_path => |_s| Ok(suggest_base_branch(&repo_path).await))
    }

    async fn update_project_group(&self, g: &ProjectGroup) -> Result<ProjectGroup> {
        rt!(self, g => |s| {
            if g.name.trim().is_empty() {
                return Err(Error::new("group name is required"));
            }
            s.store().update_project_group(g).map_err(err)
        })
    }

    async fn update_project_run_config(&self, p: &Project) -> Result<Project> {
        rt!(self, p => |s| {
            s.check_group(&p.group_id)?;
            let saved = s.store().update_project_run_config(p).map_err(err)?;
            Ok(s.with_is_git(saved).await)
        })
    }
}
