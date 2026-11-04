//! Import from Conductor (a discontinued macOS app): adopts its live worktrees
//! as hand-driven workspaces in place and copies each visible session into a
//! chat tab. Conductor kept no Claude transcript to resume, so each tab starts
//! a fresh provider session whose first send carries the copied transcript
//! (`forkInject`, as a forked chat does).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use recur_agent::AgentEvent;
use recur_domain::{AgentRole, PROVIDER_CLAUDE_CODE, PROVIDER_CODEX, PROVIDER_CURSOR, Project};
use recur_engine::session_config::AgentConfig;
use recur_engine::turn::mirror::SessionMirror;
use recur_store::Store;
use rusqlite::{Connection, OpenFlags};

use super::rt::blocking;
use super::{AppServices, err};
use crate::api::ConductorImport;
use crate::{Error, Result};

const CONDUCTOR_DB: &str = "Library/Application Support/com.conductor.app/conductor.db";

struct ConductorWorkspace {
    id: String,
    repo_name: String,
    root_path: String,
    default_branch: String,
    path: PathBuf,
    branch: String,
    title: String,
    base_branch: String,
}

struct ConductorSession {
    agent_type: String,
    model: String,
    title: String,
    /// (role, content) in send order.
    rows: Vec<(String, String)>,
}

fn open(db: &Path) -> rusqlite::Result<Connection> {
    Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// The checkout a linked worktree belongs to, from its `.git` file
/// (`gitdir: <checkout>/.git/worktrees/<name>`). Conductor's `root_path`
/// goes stale when a repo moves; the worktree's pointer does not.
fn worktree_checkout(wt: &Path) -> Option<PathBuf> {
    let s = std::fs::read_to_string(wt.join(".git")).ok()?;
    let gitdir = wt.join(s.strip_prefix("gitdir:")?.trim());
    let dotgit = gitdir.parent()?.parent()?;
    if dotgit.file_name()? != ".git" {
        return None;
    }
    dotgit
        .parent()
        .filter(|p| p.is_dir())
        .map(Path::to_path_buf)
}

/// Every workspace Conductor has not archived. A NULL path is Conductor's
/// default layout under `~/conductor/workspaces`.
fn read_workspaces(db: &Path, home: &Path) -> rusqlite::Result<Vec<ConductorWorkspace>> {
    let c = open(db)?;
    let mut st = c.prepare(
        "SELECT w.local_id, COALESCE(r.name, ''), COALESCE(r.root_path, ''),
                COALESCE(r.default_branch, ''), COALESCE(w.directory_name, ''),
                COALESCE(w.branch, ''), COALESCE(w.workspace_path, ''),
                COALESCE(NULLIF(TRIM(w.pr_title), ''), NULLIF(TRIM(w.workspace_name), ''),
                         w.directory_name, ''),
                COALESCE(NULLIF(w.intended_target_branch, ''),
                         NULLIF(w.initialization_parent_branch, ''), r.default_branch, '')
         FROM workspaces w JOIN repos r ON r.id = w.repository_id
         WHERE COALESCE(w.state, '') != 'archived'
         ORDER BY w.created_at",
    )?;
    st.query_map([], |r| {
        let (repo_name, dir, path): (String, String, String) = (r.get(1)?, r.get(4)?, r.get(6)?);
        let path = match (path.is_empty(), dir.is_empty()) {
            (false, _) => PathBuf::from(path),
            (true, false) => home
                .join("conductor/workspaces")
                .join(&repo_name)
                .join(&dir),
            (true, true) => PathBuf::new(),
        };
        Ok(ConductorWorkspace {
            id: r.get(0)?,
            repo_name,
            root_path: r.get(2)?,
            default_branch: r.get(3)?,
            path,
            branch: r.get(5)?,
            title: r.get(7)?,
            base_branch: r.get(8)?,
        })
    })?
    .collect()
}

/// A workspace's visible sessions with their sent, uncancelled rows.
fn read_sessions(db: &Path, workspace_id: &str) -> rusqlite::Result<Vec<ConductorSession>> {
    let c = open(db)?;
    let mut st = c.prepare(
        "SELECT id, COALESCE(agent_type, ''), COALESCE(model, ''),
                COALESCE(NULLIF(title, 'Untitled'), '')
         FROM sessions WHERE workspace_id = ?1 AND COALESCE(is_hidden, 0) = 0
         ORDER BY created_at, rowid",
    )?;
    let heads = st
        .query_map([workspace_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                ConductorSession {
                    agent_type: r.get(1)?,
                    model: r.get(2)?,
                    title: r.get(3)?,
                    rows: Vec::new(),
                },
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut rows = c.prepare(
        "SELECT COALESCE(role, ''), COALESCE(content, '') FROM session_messages
         WHERE session_id = ?1 AND sent_at IS NOT NULL AND cancelled_at IS NULL
         ORDER BY created_at, rowid",
    )?;
    heads
        .into_iter()
        .map(|(id, mut s)| {
            s.rows = rows
                .query_map([&id], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(s)
        })
        .collect()
}

/// The session's provider and model ref. Claude ids are Conductor aliases
/// (`opus-4-8-1m`, `fable-5`, `sonnet`): `-1m` goes (those models run 1M
/// already), versioned ones get `claude-`, bare families stay CLI aliases.
fn model_ref(agent_type: &str, model: &str) -> (&'static str, String) {
    let model = model.trim();
    let provider = match agent_type {
        "codex" => PROVIDER_CODEX,
        "cursor" => PROVIDER_CURSOR,
        _ => PROVIDER_CLAUDE_CODE,
    };
    if model.is_empty() {
        return (provider, String::new());
    }
    if provider != PROVIDER_CLAUDE_CODE {
        return (provider, format!("{provider}/{model}"));
    }
    let id = model.strip_suffix("-1m").unwrap_or(model);
    if id.contains(|c: char| c.is_ascii_digit()) && !id.starts_with("claude-") {
        return (provider, format!("{provider}/claude-{id}"));
    }
    (provider, format!("{provider}/{id}"))
}

/// Projects Conductor's rows through the live turn path: user rows are the
/// prompt text, assistant rows Claude stream-json (Codex and Cursor sessions
/// too). A row that does not parse becomes plain assistant text; init is
/// dropped so the old provider session id never becomes a resume handle.
fn replay(
    store: Arc<Store>,
    session_id: &str,
    model: &str,
    provider: &str,
    rows: &[(String, String)],
) {
    let mut m = SessionMirror::new(store, session_id, model, provider);
    for (role, content) in rows {
        if role == "user" {
            m.finish();
            m.record_user("", content, "", &[]);
            continue;
        }
        let events =
            recur_provider_claude::stream::parse_line(content.as_bytes()).unwrap_or_else(|_| {
                vec![AgentEvent::Text {
                    text: content.clone(),
                    uuid: String::new(),
                    parent: String::new(),
                }]
            });
        for ev in events
            .iter()
            .filter(|e| !matches!(e, AgentEvent::Init { .. }))
        {
            m.event(ev);
        }
    }
    m.finish();
}

impl AppServices {
    /// Conductor's database, when this machine has one.
    pub fn conductor_db(&self) -> Option<PathBuf> {
        let p = self.conductor_home()?.join(CONDUCTOR_DB);
        p.is_file().then_some(p)
    }

    fn conductor_home(&self) -> Option<PathBuf> {
        self.config
            .home
            .clone()
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
    }

    pub(crate) async fn import_conductor_at(&self) -> Result<ConductorImport> {
        let mut out = ConductorImport::default();
        let (Some(db), Some(home)) = (self.conductor_db(), self.conductor_home()) else {
            return Ok(out);
        };
        let found = {
            let db = db.clone();
            blocking(move || read_workspaces(&db, &home).map_err(err)).await?
        };
        let mut taken: HashSet<PathBuf> = self
            .store()
            .workspaces()
            .iter()
            .filter(|w| !w.worktree_path.is_empty())
            .map(|w| canonical(Path::new(&w.worktree_path)))
            .collect();
        for cw in found {
            if !cw.path.is_dir() {
                out.skipped.push(format!("{}: worktree is gone", cw.title));
                continue;
            }
            let path = canonical(&cw.path);
            if !taken.insert(path.clone()) {
                out.skipped.push(format!("{}: already in Recur", cw.title));
                continue;
            }
            let ws = match self.conductor_project(&cw, &mut out).await {
                Ok(p) => self
                    .engine
                    .adopt_workspace(
                        &p.id,
                        &cw.title,
                        &cw.base_branch,
                        &path.to_string_lossy(),
                        &cw.branch,
                    )
                    .await
                    .map_err(|e| e.to_string()),
                Err(e) => Err(e.message),
            };
            let ws = match ws {
                Ok(ws) => ws,
                Err(e) => {
                    out.skipped.push(format!("{}: {e}", cw.title));
                    continue;
                }
            };
            out.workspaces_imported += 1;
            let sessions = {
                let (db, id) = (db.clone(), cw.id.clone());
                blocking(move || read_sessions(&db, &id).map_err(err)).await
            };
            match sessions {
                Ok(sessions) => {
                    for s in sessions.iter().filter(|s| !s.rows.is_empty()) {
                        if self.import_chat(&ws.id, s).await {
                            out.chats_imported += 1;
                        }
                    }
                }
                Err(e) => out.skipped.push(format!("{} chats: {e}", cw.title)),
            }
        }
        for s in &out.skipped {
            tracing::info!("conductor import: skipped {s}");
        }
        Ok(out)
    }

    /// The Recur project on the same checkout, else a new one.
    async fn conductor_project(
        &self,
        cw: &ConductorWorkspace,
        out: &mut ConductorImport,
    ) -> Result<Project> {
        let root = worktree_checkout(&cw.path).unwrap_or_else(|| PathBuf::from(&cw.root_path));
        let root = canonical(&root);
        let existing = self
            .store()
            .projects()
            .unwrap_or_default()
            .into_iter()
            .find(|p| !p.repo_path.is_empty() && canonical(Path::new(&p.repo_path)) == root);
        if let Some(p) = existing {
            return Ok(p);
        }
        if !root.is_absolute() || !root.is_dir() {
            return Err(Error::new("repository checkout is gone"));
        }
        let p = self
            .add_project_at(Project {
                name: cw.repo_name.clone(),
                repo_path: root.to_string_lossy().into_owned(),
                base_branch: cw.default_branch.clone(),
                ..Project::default()
            })
            .await?;
        out.projects_added += 1;
        Ok(p)
    }

    /// A new pair tab holding the session's transcript, marked so its first
    /// send injects it.
    async fn import_chat(&self, workspace_id: &str, s: &ConductorSession) -> bool {
        let (provider, model) = model_ref(&s.agent_type, &s.model);
        let eng = &self.engine;
        let code = eng.next_pair_chat_code(workspace_id);
        eng.ensure_pair_agent(workspace_id, &code, &model);
        let Some(sess) = eng.ensure_agent_session(
            workspace_id,
            AgentRole::Coder.as_str(),
            &code,
            "Agent",
            provider,
        ) else {
            return false;
        };
        let cfg = AgentConfig {
            code,
            name: "Agent".into(),
            title: s.title.clone(),
            model: model.clone(),
            fork_inject: true,
            ..AgentConfig::default()
        };
        let Ok(json) = serde_json::to_string(&cfg) else {
            return false;
        };
        if self
            .store()
            .update_session_config_json(&sess.id, &json)
            .is_err()
        {
            return false;
        }
        let (store, rows) = (Arc::clone(&eng.store), s.rows.clone());
        blocking(move || {
            replay(store, &sess.id, &model, provider, &rows);
            Ok(())
        })
        .await
        .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::model_ref;

    #[test]
    fn maps_conductor_models() {
        for (agent, model, want) in [
            ("claude", "opus-4-8-1m", "claude-code/claude-opus-4-8"),
            ("claude", "opus-1m", "claude-code/opus"),
            ("claude", "fable-5", "claude-code/claude-fable-5"),
            ("claude", "sonnet", "claude-code/sonnet"),
            ("codex", "gpt-5.5", "codex/gpt-5.5"),
            ("cursor", "composer-2.5", "cursor/composer-2.5"),
            ("claude", "", ""),
        ] {
            assert_eq!(model_ref(agent, model).1, want, "{agent} {model}");
        }
    }
}
