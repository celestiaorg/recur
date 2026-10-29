//! `TerminalService`: the Observatory's terminal dock over
//! `recur_term::Manager`. It resolves a workspace to a directory and otherwise
//! forwards; coalescing, flow control and replay live in the term crate.

use async_trait::async_trait;
use base64::Engine as _;
use recur_term::{Info as TermInfo, OpenOptions};

use super::rt::rt;
use super::terminal_persist::Chrome;
use super::{AppServices, err};
use crate::api::{TerminalDockChrome, TerminalService};
use crate::{Error, Result};

fn dir_exists(p: &str) -> bool {
    !p.is_empty() && std::fs::metadata(p).is_ok_and(|m| m.is_dir())
}

impl AppServices {
    /// The directory a workspace's shells start in: the same
    /// worktree-then-repo precedence as the code browser. A recorded
    /// worktree that is gone is an error, never the project checkout.
    pub(crate) fn terminal_cwd(&self, workspace_id: &str) -> Result<String> {
        let Some(ws) = self.store().workspace(workspace_id) else {
            return Err(Error::new(format!(
                "workspace {:?} not found",
                workspace_id
            )));
        };
        let repo = self
            .store()
            .project(&ws.project_id)
            .map(|p| p.repo_path)
            .unwrap_or_default();
        match recur_code::resolve_root(&repo, &ws.worktree_path) {
            Ok(root) => Ok(root.to_string_lossy().into_owned()),
            Err(e @ recur_code::Error::WorktreeMissing(_)) => Err(Error::new(format!(
                "this workspace's folder no longer exists: {e}"
            ))),
            Err(e) => Err(Error::new(format!(
                "terminal: no directory for workspace {:?}: {e}",
                workspace_id
            ))),
        }
    }

    /// The workspace's `RECUR_*` vars plus the project's env, over the host
    /// environment.
    fn terminal_env(&self, workspace_id: &str) -> Vec<String> {
        let Some(mut ws) = self.store().workspace(workspace_id) else {
            return Vec::new();
        };
        let proj = self.store().project(&ws.project_id).unwrap_or_default();
        if ws.worktree_path.is_empty() {
            ws.worktree_path = self.terminal_cwd(workspace_id).unwrap_or_default();
        }
        recur_project::wsenv::Vars::from_workspace(&ws, &proj).environ(None)
    }

    fn open_terminal_sync(&self, workspace_id: &str, cols: i64, rows: i64) -> Result<TermInfo> {
        let cwd = self.terminal_cwd(workspace_id)?;
        let env = self.terminal_env(workspace_id);
        self.term
            .open(workspace_id, &cwd, cols as i32, rows as i32, &env)
            .map_err(err)
    }

    async fn restore_terminals(
        &self,
        workspace_id: &str,
        cols: i64,
        rows: i64,
    ) -> Result<Vec<TermInfo>> {
        let _guard = self.restore_mu.lock().await;
        let live = self.term.list(workspace_id);
        if !live.is_empty() {
            return Ok(live);
        }
        let Some(snap) = self.read_terminal_snapshot(workspace_id) else {
            return Ok(Vec::new());
        };
        let tabs = snap.tabs.unwrap_or_default();
        if tabs.is_empty() {
            return Ok(Vec::new());
        }
        // The workspace folder is resolved only for tabs whose own cwd is
        // gone: a missing worktree must not abort tabs that `cd`'d elsewhere.
        let mut fallback: Option<Result<String>> = None;
        let env = self.terminal_env(workspace_id);
        let mut out: Vec<TermInfo> = Vec::with_capacity(tabs.len());
        let mut active_id = String::new();
        for (i, tab) in tabs.into_iter().enumerate() {
            let cwd = if dir_exists(&tab.cwd) {
                tab.cwd
            } else {
                match fallback
                    .get_or_insert_with(|| self.terminal_cwd(workspace_id))
                    .clone()
                {
                    Ok(dir) => dir,
                    Err(e) => {
                        tracing::warn!("terminal: restore {workspace_id} tab {:?}: {e}", tab.title);
                        continue;
                    }
                }
            };
            let title = tab.title.clone();
            match self.term.open_with(
                workspace_id,
                &cwd,
                cols as i32,
                rows as i32,
                &env,
                OpenOptions {
                    title: tab.title,
                    replay: tab.replay,
                },
            ) {
                Ok(info) => {
                    if i as i64 == snap.active {
                        active_id.clone_from(&info.id);
                    }
                    out.push(info);
                }
                Err(e) => tracing::warn!("terminal: restore {workspace_id} tab {title:?}: {e}"),
            }
        }
        if out.is_empty() {
            return Ok(Vec::new());
        }
        if active_id.is_empty() {
            active_id.clone_from(&out[0].id);
        }
        self.term.set_active(workspace_id, &active_id);
        for info in &mut out {
            info.active = info.id == active_id;
        }
        Ok(out)
    }

    fn list_dock_chrome(&self) -> Vec<TerminalDockChrome> {
        self.store()
            .workspaces()
            .into_iter()
            .filter(|ws| !ws.archived && !ws.is_pr_chat_host())
            .filter_map(|ws| {
                let c = self.terminal_chrome(&ws.id).clamped();
                (!c.idle()).then_some(TerminalDockChrome {
                    workspace_id: ws.id,
                    open: c.open,
                    maximized: c.maximized,
                    editor_hidden: c.editor_hidden,
                    beside: c.beside,
                    beside_pct: c.beside_pct,
                })
            })
            .collect()
    }
}

#[async_trait]
impl TerminalService for AppServices {
    async fn ack_terminal(&self, id: &str, n: i64) -> Result<()> {
        rt!(self, id, n => |s| s.term.ack(&id, n.max(0) as usize).map_err(err))
    }

    async fn attach_terminal(&self, id: &str) -> Result<String> {
        rt!(self, id => |s| {
            s.term
                .attach(&id)
                .map(|b| base64::engine::general_purpose::STANDARD.encode(b))
                .map_err(err)
        })
    }

    async fn clear_terminal(&self, id: &str) -> Result<()> {
        rt!(self, id => |s| s.term.clear_replay(&id).map_err(err))
    }

    async fn close_terminal(&self, id: &str) -> Result<()> {
        rt!(self, id => |s| s.term.close(&id).map_err(err))
    }

    async fn detach_terminal(&self, id: &str) -> Result<()> {
        rt!(self, id => |s| s.term.detach(&id).map_err(err))
    }

    async fn interrupt_terminal(&self, id: &str) -> Result<()> {
        rt!(self, id => |s| s.term.interrupt(&id).map_err(err))
    }

    async fn list_terminal_dock_chrome(&self) -> Result<Vec<TerminalDockChrome>> {
        rt!(self, => |s| {
            super::rt::blocking(move || Ok(s.list_dock_chrome())).await
        })
    }

    async fn list_terminals(&self, workspace_id: &str) -> Result<Vec<TermInfo>> {
        rt!(self, workspace_id => |s| Ok(s.term.list(&workspace_id)))
    }

    async fn open_terminal(&self, workspace_id: &str, cols: i64, rows: i64) -> Result<TermInfo> {
        rt!(self, workspace_id, cols, rows => |s| s.open_terminal_sync(&workspace_id, cols, rows))
    }

    async fn reorder_terminals(&self, workspace_id: &str, ids: &[String]) -> Result<()> {
        rt!(self, workspace_id, ids => |s| s.term.reorder(&workspace_id, &ids).map_err(err))
    }

    async fn resize_terminal(&self, id: &str, cols: i64, rows: i64) -> Result<()> {
        rt!(self, id, cols, rows => |s| s.term.resize(&id, cols as i32, rows as i32).map_err(err))
    }

    async fn restore_workspace_terminals(
        &self,
        workspace_id: &str,
        cols: i64,
        rows: i64,
    ) -> Result<Vec<TermInfo>> {
        rt!(self, workspace_id, cols, rows => |s| {
            s.restore_terminals(&workspace_id, cols, rows).await
        })
    }

    async fn set_active_terminal(&self, workspace_id: &str, id: &str) -> Result<()> {
        rt!(self, workspace_id, id => |s| {
            s.term.set_active(&workspace_id, &id);
            Ok(())
        })
    }

    async fn set_terminal_dock_chrome(
        &self,
        workspace_id: &str,
        chrome: &TerminalDockChrome,
    ) -> Result<()> {
        rt!(self, workspace_id, chrome => |s| {
            if !workspace_id.is_empty() {
                s.set_terminal_chrome(&workspace_id, Chrome::from(&chrome));
            }
            Ok(())
        })
    }

    async fn set_terminal_title(&self, id: &str, title: &str) -> Result<()> {
        rt!(self, id, title => |s| s.term.set_title(&id, &title).map_err(err))
    }

    async fn write_terminal(&self, id: &str, data: &str) -> Result<()> {
        rt!(self, id, data => |s| s.term.write(&id, data.as_bytes()).map_err(err))
    }
}
