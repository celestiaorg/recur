//! `ShellService` (Go `shell.go`): the last top-level view (`shell.json`,
//! write coalesced and flushed on quit), the schedules dashboard accordion
//! (`schedule-view.json`) and the Issues page layout (`issues-view.json`),
//! all under the data root so they survive restarts.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;

use super::rt::rt;
use super::{AppServices, err};
use crate::Result;
use crate::api::{IssuesView, ShellService, ShellState};

pub(crate) const SHELL_STATE_FILE: &str = "shell.json";
pub(crate) const SCHEDULE_VIEW_FILE: &str = "schedule-view.json";
pub(crate) const ISSUES_VIEW_FILE: &str = "issues-view.json";
pub(crate) const SHELL_SAVE_DEBOUNCE: Duration = Duration::from_millis(250);
const SHELL_SAVE_BACKOFF_MAX: Duration = Duration::from_secs(30);

#[derive(Default)]
struct Inner {
    timer: Option<JoinHandle<()>>,
    loaded: bool,
    last: ShellState,
    pending: Option<ShellState>,
    fail_delay: Duration,
    fail_logged: bool,
    schedule_loaded: bool,
    expanded: Vec<String>,
}

#[derive(Default)]
pub(crate) struct ShellStore(Mutex<Inner>);

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct ScheduleView {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    expanded: Vec<String>,
}

/// The persistable views, in sync with `snapshotShell` /
/// `resolveShellRestore` (welcome is first-run only and never saved).
pub fn known_shell_view(v: &str) -> bool {
    matches!(
        v,
        "detail"
            | "escalation"
            | "prdetail"
            | "project"
            | "inbox"
            | "briefing"
            | "settings"
            | "pr"
            | "schedules"
            | "issues"
    )
}

/// Keeps only the ids a view uses; an unknown view is the zero state.
pub fn clamp_shell_state(mut st: ShellState) -> ShellState {
    match st.view.as_str() {
        "detail" | "escalation" => {
            st.pr_id.clear();
            st.project_id.clear();
        }
        "prdetail" => {
            st.workspace_id.clear();
            st.project_id.clear();
        }
        "project" => {
            st.workspace_id.clear();
            st.pr_id.clear();
        }
        "inbox" | "briefing" | "settings" | "pr" | "schedules" | "issues" => {
            st.workspace_id.clear();
            st.pr_id.clear();
            st.project_id.clear();
        }
        _ => return ShellState::default(),
    }
    st
}

/// Indented JSON (Go `json.MarshalIndent(v, "", "  ")`) written through
/// `<path>.tmp` and renamed (Go `writeDataJSON`).
pub(crate) fn write_data_json<T: Serialize>(path: &Path, v: &T) -> std::io::Result<()> {
    let data = recur_engine::artifacts::marshal_indent(v)
        .ok_or_else(|| std::io::Error::other("json: unsupported value"))?;
    let tmp = PathBuf::from(format!("{}.tmp", path.display()));
    std::fs::write(&tmp, data)?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

fn normalize_expanded(ids: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(ids.len());
    for id in ids {
        let id = id.trim();
        if !id.is_empty() && !out.iter().any(|x| x == id) {
            out.push(id.to_owned());
        }
    }
    out
}

impl AppServices {
    fn shell_path(&self, name: &str) -> PathBuf {
        self.data_dir().join(name)
    }

    fn load_shell_state(&self) -> ShellState {
        std::fs::read(self.shell_path(SHELL_STATE_FILE))
            .ok()
            .and_then(|data| serde_json::from_slice::<ShellState>(&data).ok())
            .map(clamp_shell_state)
            .unwrap_or_default()
    }

    fn ensure_shell_loaded(&self, st: &mut Inner) {
        if !st.loaded {
            st.last = self.load_shell_state();
            st.loaded = true;
        }
    }

    fn ensure_schedule_view_loaded(&self, st: &mut Inner) {
        if st.schedule_loaded {
            return;
        }
        st.expanded = std::fs::read(self.shell_path(SCHEDULE_VIEW_FILE))
            .ok()
            .and_then(|data| serde_json::from_slice::<ScheduleView>(&data).ok())
            .map(|v| normalize_expanded(&v.expanded))
            .unwrap_or_default();
        st.schedule_loaded = true;
    }

    /// Arms the coalesced write (Go `scheduleLocked`).
    fn schedule_shell_flush(&self, st: &mut Inner) {
        if let Some(t) = st.timer.take() {
            t.abort();
        }
        let d = if st.fail_delay > Duration::ZERO {
            st.fail_delay
        } else {
            SHELL_SAVE_DEBOUNCE
        };
        let me = self.me.clone();
        st.timer = Some(tokio::spawn(async move {
            tokio::time::sleep(d).await;
            if let Some(s) = me.upgrade() {
                s.shell_flush_from_timer();
            }
        }));
    }

    fn shell_flush_from_timer(&self) {
        let mut st = self.shell.0.lock();
        // This timer is the one being flushed; dropping its handle (not
        // aborting it) lets the write below finish.
        st.timer = None;
        self.shell_flush_locked(&mut st);
    }

    /// Writes any coalesced snapshot (Go `Flush`). The lock is held across
    /// the write so overlapping flushes cannot share `shell.json.tmp`. A
    /// failed write keeps the snapshot pending and retries with backoff; only
    /// the first failure (and the recovery) is logged.
    pub(crate) fn shell_flush(&self) {
        let mut st = self.shell.0.lock();
        if let Some(t) = st.timer.take() {
            t.abort();
        }
        self.shell_flush_locked(&mut st);
    }

    fn shell_flush_locked(&self, st: &mut Inner) {
        let Some(snap) = st.pending.clone() else {
            return;
        };
        if let Err(e) = write_data_json(&self.shell_path(SHELL_STATE_FILE), &snap) {
            if !st.fail_logged {
                tracing::warn!("shell: persist: {e}");
                st.fail_logged = true;
            }
            if st.fail_delay == Duration::ZERO {
                st.fail_delay = SHELL_SAVE_DEBOUNCE * 2;
            } else if st.fail_delay < SHELL_SAVE_BACKOFF_MAX {
                st.fail_delay = (st.fail_delay * 2).min(SHELL_SAVE_BACKOFF_MAX);
            }
            self.schedule_shell_flush(st);
            return;
        }
        if st.fail_logged {
            tracing::info!("shell: persist recovered");
        }
        st.fail_logged = false;
        st.fail_delay = Duration::ZERO;
        st.pending = None;
    }

    pub(crate) fn shell_state(&self) -> ShellState {
        let mut st = self.shell.0.lock();
        self.ensure_shell_loaded(&mut st);
        st.last.clone()
    }

    pub(crate) fn set_shell_state_sync(&self, next: &ShellState) {
        if !known_shell_view(&next.view) {
            return;
        }
        let next = clamp_shell_state(next.clone());
        let mut st = self.shell.0.lock();
        self.ensure_shell_loaded(&mut st);
        if st.last == next {
            return;
        }
        st.last = next.clone();
        st.pending = Some(next);
        self.schedule_shell_flush(&mut st);
    }

    pub(crate) fn expanded_schedules(&self) -> Vec<String> {
        let mut st = self.shell.0.lock();
        self.ensure_schedule_view_loaded(&mut st);
        st.expanded.clone()
    }

    pub(crate) fn set_expanded_schedules_sync(&self, ids: &[String]) -> Result<()> {
        let ids = normalize_expanded(ids);
        let mut st = self.shell.0.lock();
        self.ensure_schedule_view_loaded(&mut st);
        if st.expanded == ids {
            return Ok(());
        }
        st.expanded = ids.clone();
        write_data_json(
            &self.shell_path(SCHEDULE_VIEW_FILE),
            &ScheduleView { expanded: ids },
        )
        .map_err(err)
    }

    pub(crate) fn issues_view(&self) -> IssuesView {
        std::fs::read(self.shell_path(ISSUES_VIEW_FILE))
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default()
    }

    pub(crate) fn set_issues_view_sync(&self, v: &IssuesView) -> Result<()> {
        let _st = self.shell.0.lock();
        write_data_json(&self.shell_path(ISSUES_VIEW_FILE), v).map_err(err)
    }

    #[cfg(test)]
    pub(crate) fn shell_fail_delay(&self) -> Duration {
        self.shell.0.lock().fail_delay
    }

    /// A fresh service: the next read comes from disk.
    #[cfg(test)]
    pub(crate) fn shell_forget(&self) {
        *self.shell.0.lock() = Inner::default();
    }
}

#[async_trait]
impl ShellService for AppServices {
    async fn flush(&self) -> Result<()> {
        rt!(self, => |s| {
            s.shell_flush();
            Ok(())
        })
    }

    async fn get_expanded_schedules(&self) -> Result<Vec<String>> {
        rt!(self, => |s| Ok(s.expanded_schedules()))
    }

    async fn get_issues_view(&self) -> Result<IssuesView> {
        rt!(self, => |s| Ok(s.issues_view()))
    }

    async fn get_shell_state(&self) -> Result<ShellState> {
        rt!(self, => |s| Ok(s.shell_state()))
    }

    async fn set_expanded_schedules(&self, ids: &[String]) -> Result<()> {
        rt!(self, ids => |s| s.set_expanded_schedules_sync(&ids))
    }

    async fn set_issues_view(&self, v: &IssuesView) -> Result<()> {
        rt!(self, v => |s| s.set_issues_view_sync(&v))
    }

    async fn set_shell_state(&self, st: &ShellState) -> Result<()> {
        rt!(self, st => |s| {
            s.set_shell_state_sync(&st);
            Ok(())
        })
    }
}
