//! The per-workspace environment Recur injects into setup, teardown, named
//! runs, verify boots, and Observatory terminals.

mod dotenv;

use std::collections::{BTreeMap, BTreeSet};

use recur_domain::{Project, Workspace};

pub use dotenv::{DOTENV_FILES, load_dotenv_files, parse_dotenv};

/// The workspace-scoped environment Recur owns. `Project::env` (dotenv text)
/// values may reference these via `$NAME` / `${NAME}`, plus host vars and
/// keys from `.env` files next to the project / worktree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Vars {
    pub port: i64,
    pub workspace_id: String,
    pub worktree_path: String,
    /// The project checkout; .env files are loaded from here too.
    pub repo_path: String,
    pub project_id: String,
    /// The workspace git branch; injected as `$RECUR_BRANCH`.
    pub branch: String,
    /// Parsed from the project's Env dotenv text.
    pub project_env: BTreeMap<String, String>,
}

impl Vars {
    /// Builds Vars from a stored workspace and its project.
    pub fn from_workspace(ws: &Workspace, proj: &Project) -> Self {
        Self {
            port: ws.port,
            workspace_id: ws.id.clone(),
            worktree_path: ws.worktree_path.clone(),
            repo_path: proj.repo_path.clone(),
            project_id: ws.project_id.clone(),
            branch: ws.branch.trim().to_owned(),
            project_env: project_env(proj),
        }
    }

    /// The RECUR_* (and PORT) bindings for expansion and injection.
    /// RECUR_BRANCH is always present so `$RECUR_BRANCH` expands to empty
    /// instead of staying literal.
    pub fn recur_map(&self) -> BTreeMap<String, String> {
        let mut m = BTreeMap::from([
            ("RECUR_WORKSPACE_ID".to_owned(), self.workspace_id.clone()),
            ("RECUR_PROJECT_ID".to_owned(), self.project_id.clone()),
            ("RECUR_WORKTREE".to_owned(), self.worktree_path.clone()),
            ("RECUR_BRANCH".to_owned(), self.branch.trim().to_owned()),
        ]);
        if self.port > 0 {
            let p = self.port.to_string();
            m.insert("RECUR_PORT".to_owned(), p.clone());
            m.insert("PORT".to_owned(), p);
        }
        m
    }

    /// `base` (the host environment when `None`) merged with the workspace
    /// RECUR_* vars and the project's Env map, its values expanded against
    /// Recur vars, .env keys, and the host environment. Later sources win:
    /// host → Recur vars → project Env (protected keys stripped only when env
    /// came from project.toml). Gitignored .env files are read for that
    /// expansion; their keys are not merged into the result.
    pub fn environ(&self, base: Option<Vec<String>>) -> Vec<String> {
        let base = base.unwrap_or_else(host_environ);
        let recur = self.recur_map();
        let files = drop_protected_env(load_dotenv_files(&[&self.repo_path, &self.worktree_path]));
        let files = expand_map(&files, &recur, &BTreeMap::new());
        let out = merge_env(&base, &recur);
        if self.project_env.is_empty() {
            return out;
        }
        let expanded = expand_map(&self.project_env, &recur, &files);
        merge_env(&out, &expanded)
    }
}

/// The process environment as `KEY=VALUE` strings. Non-UTF-8 bytes are replaced
/// with U+FFFD.
pub fn host_environ() -> Vec<String> {
    std::env::vars_os()
        .map(|(k, v)| format!("{}={}", k.to_string_lossy(), v.to_string_lossy()))
        .collect()
}

fn project_env(proj: &Project) -> BTreeMap<String, String> {
    let env = parse_dotenv(&proj.env);
    if proj.has_project_file {
        return drop_protected_env(env);
    }
    env
}

/// Process-critical names a repo .env or project.toml env must not override
/// (PATH/HOME/SHELL and loader preload paths). Keys typed into Recur's
/// Environment field (no project.toml) are not stripped.
fn protected_env_key(k: &str) -> bool {
    let u = k.trim().to_uppercase();
    matches!(u.as_str(), "PATH" | "HOME" | "SHELL")
        || u.starts_with("LD_")
        || u.starts_with("DYLD_")
}

fn drop_protected_env(input: BTreeMap<String, String>) -> BTreeMap<String, String> {
    input
        .into_iter()
        .filter(|(k, _)| !protected_env_key(k))
        .collect()
}

/// Substitutes `$KEY` / `${KEY}` from vars, then the process environment (so
/// `$HOME` works in project env values). Unknown references stay literal so
/// a typo does not silently become empty.
pub fn expand_known(s: &str, vars: &BTreeMap<String, String>) -> String {
    expand_known_with(s, vars, &BTreeMap::new())
}

fn expand_known_with(
    s: &str,
    vars: &BTreeMap<String, String>,
    extra: &BTreeMap<String, String>,
) -> String {
    if s.is_empty() {
        return String::new();
    }
    expand(s, |key| {
        if let Some(v) = vars.get(key).or_else(|| extra.get(key)) {
            return v.clone();
        }
        if let Ok(v) = std::env::var(key) {
            return v;
        }
        // Preserve the reference for the shell / later expansion.
        format!("${key}")
    })
}

fn expand_map(
    input: &BTreeMap<String, String>,
    recur: &BTreeMap<String, String>,
    extra: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    if input.is_empty() {
        return input.clone();
    }
    let mut raw_by_key = BTreeMap::new();
    let mut pass1 = BTreeMap::new();
    for (k, raw) in input {
        let k = k.trim();
        if k.is_empty() {
            continue;
        }
        raw_by_key.insert(k.to_owned(), raw.clone());
        pass1.insert(k.to_owned(), expand_known_with(raw, recur, extra));
    }
    // Expand the original text against sibling pass-1 values, not the pass-1
    // output: re-expanding substituted Recur/env values would interpolate a
    // `$FOO` inside a git branch or worktree path.
    let mut lookup = extra.clone();
    lookup.extend(pass1);
    raw_by_key
        .into_iter()
        .map(|(k, raw)| {
            let v = expand_known_with(&raw, recur, &lookup);
            (k, v)
        })
        .collect()
}

/// Copies base and overlays vals, replacing existing keys.
fn merge_env(base: &[String], vals: &BTreeMap<String, String>) -> Vec<String> {
    let drop: BTreeSet<&str> = vals.keys().map(String::as_str).collect();
    let mut out: Vec<String> = base
        .iter()
        .filter(|kv| match kv.find('=') {
            Some(i) if i > 0 => !drop.contains(&kv[..i]),
            _ => true,
        })
        .cloned()
        .collect();
    out.extend(vals.iter().map(|(k, v)| format!("{k}={v}")));
    out
}

/// Replaces `$var` and `${var}` using mapping. Invalid syntax (`${`, `${}`) is
/// eaten; a `$` not followed by a name stays.
pub fn expand(s: &str, mut mapping: impl FnMut(&str) -> String) -> String {
    let b = s.as_bytes();
    let mut buf: Option<String> = None;
    let mut i = 0;
    let mut j = 0;
    while j < b.len() {
        if b[j] == b'$' && j + 1 < b.len() {
            let out = buf.get_or_insert_with(|| String::with_capacity(2 * s.len()));
            out.push_str(&s[i..j]);
            let (name, w) = shell_name(&s[j + 1..]);
            if name.is_empty() && w > 0 {
                // Invalid syntax; eat the characters.
            } else if name.is_empty() {
                out.push('$');
            } else {
                out.push_str(&mapping(name));
            }
            j += w;
            i = j + 1;
        }
        j += 1;
    }
    match buf {
        None => s.to_owned(),
        Some(mut out) => {
            out.push_str(&s[i.min(s.len())..]);
            out
        }
    }
}

fn is_shell_special(c: u8) -> bool {
    matches!(
        c,
        b'*' | b'#' | b'$' | b'@' | b'!' | b'?' | b'-' | b'0'..=b'9'
    )
}

fn shell_name(s: &str) -> (&str, usize) {
    let b = s.as_bytes();
    if b[0] == b'{' {
        if b.len() > 2 && is_shell_special(b[1]) && b[2] == b'}' {
            return (&s[1..2], 3);
        }
        for i in 1..b.len() {
            if b[i] == b'}' {
                if i == 1 {
                    return ("", 2); // Bad syntax; eat "${}"
                }
                return (&s[1..i], i + 1);
            }
        }
        return ("", 1); // Bad syntax; eat "${"
    }
    if is_shell_special(b[0]) {
        return (&s[0..1], 1);
    }
    let n = b
        .iter()
        .take_while(|c| c.is_ascii_alphanumeric() || **c == b'_')
        .count();
    (&s[..n], n)
}

#[cfg(test)]
mod tests;
