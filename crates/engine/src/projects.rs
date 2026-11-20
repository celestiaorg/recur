//! Adding a project: Settings' Add project, opening a folder, the Conductor import and Ask
//! Recur's `create_project` all land here.

use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

use recur_agent_tools::CreateProjectRequest;
use recur_domain::{FILES_PROJECT_ID, Project};
use regex::Regex;

use crate::Engine;
use crate::core::{EngineError, Result};

/// Sidebar avatar tints, assigned round-robin to new projects.
const PROJECT_COLORS: [&str; 5] = [
    "var(--cyan)",
    "var(--amber)",
    "var(--indigo)",
    "var(--emerald)",
    "var(--coral)",
];

/// Branch probes share one 10 s budget.
pub const BRANCH_TIMEOUT: Duration = Duration::from_secs(10);

/// A clone may be large, but a stuck one (no credentials, no network) must end.
const CLONE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

static SLUG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new("[^a-z0-9]+").expect("slug re"));

/// Lowercases `s`, keeps a path's last segment, and collapses
/// non-alphanumerics into single hyphens.
pub fn slugify(s: &str) -> String {
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

/// The first rune uppercased with the simple case mapping, so a rune whose
/// uppercase is several characters stays as is.
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

/// The base branch to suggest for a checkout, bounded at 10 s.
pub async fn suggest_base_branch(repo_path: &str) -> String {
    tokio::time::timeout(
        BRANCH_TIMEOUT,
        recur_git::suggest_base_branch_for_repo(repo_path),
    )
    .await
    .unwrap_or_else(|_| "main".into())
}

/// A clone URL's repo name: `git@host:org/repo.git` → `repo`.
fn repo_name(url: &str) -> &str {
    let tail = url
        .trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()
        .unwrap_or("");
    tail.strip_suffix(".git").unwrap_or(tail)
}

/// `git init -b main` and an empty first commit (worktrees branch off a commit). Without a
/// git identity configured, the commit is authored as Recur.
async fn initial_commit(path: &str) -> std::result::Result<(), String> {
    let git = |args: Vec<&str>| recur_git::command(args).current_dir(path);
    git(vec!["init", "-b", "main"])
        .run()
        .await
        .map_err(|e| format!("git init failed: {e}"))?;
    let email = git(vec!["config", "user.email"])
        .output()
        .await
        .unwrap_or_default();
    let mut args = Vec::new();
    if String::from_utf8_lossy(&email).trim().is_empty() {
        args.extend(["-c", "user.name=Recur", "-c", "user.email=recur@localhost"]);
    }
    args.extend(["commit", "--allow-empty", "-m", "Initial commit"]);
    git(args)
        .run()
        .await
        .map_err(|e| format!("git commit failed: {e}"))
}

impl Engine {
    fn unique_project_id(&self, base: &str) -> String {
        let base = if base.is_empty() { "project" } else { base };
        let taken = |id: &str| id == FILES_PROJECT_ID || self.store.has_project(id);
        let mut id = base.to_owned();
        let mut i = 2;
        while taken(&id) {
            id = format!("{base}-{i}");
            i += 1;
        }
        id
    }

    /// Persists a new project and returns the saved copy: a stable slug id from the name
    /// (or repo path), an avatar letter and color when empty, open in the sidebar, the base
    /// branch suggested from the checkout, and its working-copy row seeded.
    pub async fn add_project(&self, mut p: Project) -> Result<Project> {
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
            let n = self.store.projects().map_or(0, |v| v.len());
            p.color = PROJECT_COLORS[n % PROJECT_COLORS.len()].into();
        }
        // Sidebar default on create only: an upsert keeps the caller's Open.
        if !self.store.has_project(&p.id) {
            p.open = true;
        }
        if p.base_branch.trim().is_empty() {
            p.base_branch = suggest_base_branch(&p.repo_path).await;
        }
        let mut saved = self
            .store
            .add_project(p)
            .map_err(|e| EngineError::new(e.to_string()))?;
        saved.is_git = recur_git::is_repo(&saved.repo_path).await;
        // Seed the working-copy row from the real checkout so the sidebar does
        // not fall back to Project.BaseBranch (often origin/…, not HEAD).
        let _ = self.open_main_workspace(&saved.id).await;
        Ok(saved)
    }

    /// Where a project without a path goes: Settings' new projects folder, else beside the
    /// most recently added project, else `~/Development`.
    fn default_project_parent(&self) -> String {
        let set = self.settings();
        if !set.new_projects_dir.trim().is_empty() {
            return recur_git::expand_home(set.new_projects_dir.trim());
        }
        self.store
            .projects()
            .unwrap_or_default()
            .iter()
            .rev()
            .filter(|p| p.id != FILES_PROJECT_ID)
            .find_map(|p| {
                Path::new(p.repo_path.trim())
                    .parent()
                    .map(|d| d.to_string_lossy().into_owned())
            })
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| recur_git::expand_home("~/Development"))
    }

    /// Host `create_project` (Ask Recur): registers an existing folder as is, or creates the
    /// folder as a fresh repo (or a clone) and registers that.
    pub async fn create_project_from_app(&self, req: CreateProjectRequest) -> String {
        let name = match req.name.trim() {
            "" => repo_name(&req.clone_url).to_owned(),
            n => n.to_owned(),
        };
        if name.is_empty() {
            return "create_project: name is required".into();
        }
        let path = match req.path.trim() {
            "" => {
                let slug = slugify(&name);
                let dir = if slug.is_empty() { "project" } else { &slug };
                Path::new(&self.default_project_parent())
                    .join(dir)
                    .to_string_lossy()
                    .into_owned()
            }
            p => recur_git::expand_home(p),
        };
        if !Path::new(&path).is_absolute() {
            return format!("create_project: path {path:?} must be absolute or start with ~");
        }
        let projects = self.store.projects().unwrap_or_default();
        if let Some(p) = projects.iter().find(|p| {
            Path::new(&p.repo_path) == Path::new(&path) || p.name.eq_ignore_ascii_case(&name)
        }) {
            return format!(
                "Project {} ({}) already exists at {}; not creating another.",
                p.name, p.id, p.repo_path
            );
        }
        let occupied = std::fs::read_dir(&path).is_ok_and(|mut d| d.next().is_some());
        let made = if occupied {
            if !req.clone_url.is_empty() {
                return format!(
                    "create_project: {path} already exists and is not empty; not cloning into it"
                );
            }
            "registered the existing folder".to_owned()
        } else if !req.clone_url.is_empty() {
            if let Err(e) = recur_git::command(["clone", "--", &req.clone_url, &path])
                .env("GIT_TERMINAL_PROMPT", "0")
                .timeout(CLONE_TIMEOUT)
                .run()
                .await
            {
                return format!("create_project: git clone {} failed: {e}", req.clone_url);
            }
            format!("cloned {}", req.clone_url)
        } else {
            if let Err(e) = std::fs::create_dir_all(&path) {
                return format!("create_project: cannot create {path}: {e}");
            }
            if let Err(e) = initial_commit(&path).await {
                return format!("create_project: {e} in {path}");
            }
            "created a new git repo on main with an empty first commit".to_owned()
        };
        let saved = match self
            .add_project(Project {
                name,
                repo_path: path.clone(),
                ..Project::default()
            })
            .await
        {
            Ok(p) => p,
            Err(e) => return format!("create_project: {e}"),
        };
        let next = if saved.is_git {
            format!(
                "create_workspace with project_id {} starts work in it.",
                saved.id
            )
        } else {
            "It is not a git repo, so it cannot host workspaces until the human runs git init there.".to_owned()
        };
        format!(
            "Project {} ({}) at {path}: {made}. {next}",
            saved.name, saved.id
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestEngine, init_repo};

    fn req(name: &str, path: &str, clone_url: &str) -> CreateProjectRequest {
        CreateProjectRequest {
            name: name.into(),
            path: path.into(),
            clone_url: clone_url.into(),
        }
    }

    #[test]
    fn repo_name_reads_https_and_scp_urls() {
        assert_eq!(repo_name("https://github.com/org/repo.git"), "repo");
        assert_eq!(repo_name("git@github.com:org/repo.git"), "repo");
        assert_eq!(repo_name("https://example.com/repo/"), "repo");
    }

    #[tokio::test]
    async fn create_project_starts_a_repo_registers_a_folder_and_clones() {
        let te = TestEngine::new(None);
        let root = tempfile::tempdir().unwrap();
        let at = |n: &str| root.path().join(n).to_string_lossy().into_owned();

        let got = te
            .engine
            .create_project_from_app(req("Brand New", &at("brand-new"), ""))
            .await;
        assert!(got.contains("created a new git repo"), "{got}");
        let p = te.store.project("brand-new").expect("registered");
        assert_eq!(p.repo_path, at("brand-new"));
        assert!(recur_git::is_repo(&p.repo_path).await);
        assert!(
            recur_git::command(["rev-parse", "HEAD"])
                .current_dir(&p.repo_path)
                .run()
                .await
                .is_ok(),
            "an initial commit"
        );

        let again = te
            .engine
            .create_project_from_app(req("brand new", "", ""))
            .await;
        assert!(again.contains("already exists"), "{again}");

        std::fs::create_dir_all(at("notes")).unwrap();
        std::fs::write(root.path().join("notes/a.txt"), "keep").unwrap();
        let got = te
            .engine
            .create_project_from_app(req("Notes", &at("notes"), ""))
            .await;
        assert!(got.contains("registered the existing folder"), "{got}");
        assert!(got.contains("not a git repo"), "{got}");
        assert!(!root.path().join("notes/.git").exists(), "never written to");

        let mut set = te.store.settings();
        set.new_projects_dir = at("code");
        te.store.set_settings(set);
        let got = te
            .engine
            .create_project_from_app(req("Fresh", "", ""))
            .await;
        assert!(
            got.contains(&at("code/fresh")),
            "the new projects folder: {got}"
        );
        let got = te
            .engine
            .create_project_from_app(req("日本語", "", ""))
            .await;
        assert!(got.contains(&at("code/project")), "{got}");

        let src = init_repo();
        let got = te
            .engine
            .create_project_from_app(req("", "", &src.path))
            .await;
        assert!(got.contains("cloned"), "{got}");
        let cloned = te
            .store
            .projects()
            .unwrap()
            .into_iter()
            .find(|p| p.repo_path.ends_with(&slugify(&src.path)))
            .expect("clone registered beside the others");
        assert!(Path::new(&cloned.repo_path).join("README.md").exists());
        te.close().await;
    }
}
