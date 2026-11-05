//! Portable Recur project settings in `<repo>/.recur/project.toml`, so
//! setup/teardown/runs/env/copy-files can be committed and shared. That file
//! is the only source for run-config; the settings page writes it (creating
//! it if needed). Machine-local fields (path, sidebar, pinned run, skills)
//! stay in Recur's database.

mod write;

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

use recur_domain::{Project, RunCommand, effective_runs};
use serde::Deserialize;

pub use write::{WriteError, write};

use crate::home::expand_home;

/// The repo-relative location of the committed project file.
pub const REL_PATH: &str = ".recur/project.toml";

const FILE_HEADER: &str =
    "# Recur project settings: setup, teardown, named runs, app URL, env, and files to copy.
# Safe to commit and share. Keep secrets in gitignored dotenv for the app to load;
# Recur copies them via copy_files and does not export them.
";

/// The project.toml shape. Unknown keys are ignored on read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct File {
    pub setup_cmd: String,
    pub teardown_cmd: String,
    pub app_url: String,
    pub env: String,
    pub copy_files: String,
    pub conventional_commits: bool,
    #[serde(rename = "run")]
    pub runs: Vec<FileRun>,
}

/// One `[[run]]` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct FileRun {
    pub name: String,
    pub cmd: String,
}

struct CacheEnt {
    mtime: SystemTime,
    size: u64,
    file: File,
    err: Option<String>,
}

#[derive(Default)]
struct Cache {
    parsed: HashMap<String, CacheEnt>,
    /// path → mtime of the last logged parse error.
    logged: HashMap<String, Option<SystemTime>>,
}

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Mutex::default);

fn cache() -> std::sync::MutexGuard<'static, Cache> {
    CACHE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The absolute project.toml path for a repo, or "" when repo_path is empty.
pub fn path(repo_path: &str) -> String {
    let root = repo_root(repo_path);
    if root.is_empty() {
        return String::new();
    }
    crate::home::join(&root, REL_PATH)
}

/// Whether `<repo>/.recur/project.toml` is present as a file.
pub fn exists(repo_path: &str) -> bool {
    let p = path(repo_path);
    !p.is_empty() && std::fs::metadata(&p).is_ok_and(|m| !m.is_dir())
}

/// Replaces p's run-config with `.recur/project.toml`. A missing file yields
/// empty run-config. A malformed file is logged once per change, sets
/// `project_file_error`, and also yields empty run-config so the project
/// stays visible and settings can rewrite the file.
pub fn overlay(mut p: Project) -> Project {
    p.has_project_file = exists(&p.repo_path);
    clear_run_config(&mut p);
    if !p.has_project_file {
        return p;
    }
    match load(&p.repo_path) {
        Ok(f) => apply(&mut p, &f),
        Err(err) => {
            log_parse_err(&p.repo_path, &err);
            p.project_file_error = err;
        }
    }
    p
}

fn clear_run_config(p: &mut Project) {
    p.setup_cmd.clear();
    p.teardown_cmd.clear();
    p.app_url.clear();
    p.env.clear();
    p.copy_files.clear();
    p.conventional_commits = false;
    p.run_commands.clear();
    p.project_file_error.clear();
}

pub(crate) fn load(repo_path: &str) -> Result<File, String> {
    let path = path(repo_path);
    if path.is_empty() {
        return Ok(File::default());
    }
    let st = match std::fs::metadata(&path) {
        Ok(st) => st,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            invalidate(&path);
            return Ok(File::default());
        }
        Err(e) => return Err(format!("stat {path}: {e}")),
    };
    if st.is_dir() {
        return Err(format!("parse {REL_PATH}: is a directory"));
    }
    let mtime = st.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let size = st.len();
    if let Some(e) = cache().parsed.get(&path)
        && e.mtime == mtime
        && e.size == size
    {
        return match &e.err {
            Some(err) => Err(err.clone()),
            None => Ok(e.file.clone()),
        };
    }

    let data = std::fs::read(&path).map_err(|e| format!("open {path}: {e}"))?;
    let (file, err) = match parse(&data) {
        Ok(f) => (f, None),
        Err(e) => (File::default(), Some(format!("parse {REL_PATH}: {e}"))),
    };
    cache().parsed.insert(
        path,
        CacheEnt {
            mtime,
            size,
            file: file.clone(),
            err: err.clone(),
        },
    );
    match err {
        Some(err) => Err(err),
        None => Ok(file),
    }
}

fn parse(data: &[u8]) -> Result<File, String> {
    let text = std::str::from_utf8(data).map_err(|_| "toml: invalid UTF-8".to_owned())?;
    toml::from_str(text).map_err(|e: toml::de::Error| format!("toml: {}", e.message()))
}

fn log_parse_err(repo_path: &str, err: &str) {
    let path = path(repo_path);
    let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    {
        let mut c = cache();
        if c.logged.get(&path) == Some(&mtime) {
            return;
        }
        c.logged.insert(path, mtime);
    }
    tracing::warn!("recur: {REL_PATH}: {err}");
}

pub(crate) fn invalidate(path: &str) {
    if path.is_empty() {
        return;
    }
    let mut c = cache();
    c.parsed.remove(path);
    c.logged.remove(path);
}

/// Copies file fields onto p. [`overlay`] clears run-config first so
/// omitted keys stay empty.
pub fn apply(p: &mut Project, f: &File) {
    p.setup_cmd = f.setup_cmd.clone();
    p.teardown_cmd = f.teardown_cmd.clone();
    p.app_url = f.app_url.clone();
    p.env = f.env.clone();
    p.copy_files = f.copy_files.clone();
    p.conventional_commits = f.conventional_commits;
    p.run_commands = domain_runs(&f.runs);
}

fn repo_root(repo_path: &str) -> String {
    expand_home(repo_path).trim().to_owned()
}

fn domain_runs(runs: &[FileRun]) -> Vec<RunCommand> {
    let p = Project {
        run_commands: runs
            .iter()
            .map(|r| RunCommand {
                name: r.name.clone(),
                cmd: r.cmd.clone(),
            })
            .collect(),
        ..Default::default()
    };
    effective_runs(&p)
}

#[cfg(test)]
mod tests;
