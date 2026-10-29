//! The last-open shell across restarts (`lib/shell-state.ts`). The snapshot lives in
//! `<DataDir>/shell.json` behind `ShellService`; this is the snapshot, the restore resolver,
//! and the coalescing writer.

use std::collections::HashSet;

use recur_services::api::ShellState;

use crate::ViewName;

/// Where boot lands after reading the saved shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellRestore {
    None,
    Workspace(String),
    Pr(String),
    Project(String),
    View(ViewName),
}

/// The ids that still exist at boot.
#[derive(Default)]
pub struct ShellLive {
    pub workspace_ids: HashSet<String>,
    pub pr_ids: HashSet<String>,
    pub project_ids: HashSet<String>,
}

const TOP_VIEWS: [ViewName; 6] = [
    ViewName::Inbox,
    ViewName::Briefing,
    ViewName::Settings,
    ViewName::Pr,
    ViewName::Schedules,
    ViewName::Issues,
];

/// `snapshotShell`: the snapshot written on each navigation.
pub fn snapshot_shell(
    view: ViewName,
    workspace_id: &str,
    pr_id: &str,
    project_id: &str,
) -> ShellState {
    let mut st = ShellState {
        view: view.as_str().into(),
        ..ShellState::default()
    };
    match view {
        ViewName::Detail | ViewName::Escalation => st.workspace_id = workspace_id.into(),
        ViewName::PrDetail => st.pr_id = pr_id.into(),
        ViewName::Project => st.project_id = project_id.into(),
        _ => {}
    }
    st
}

/// `resolveShellRestore`. Missing or archived targets fall through to `None` (Briefing stays
/// the default). Welcome is first-run only and never a restore target. A missing PR falls back
/// to the PR list.
pub fn resolve_shell_restore(saved: Option<&ShellState>, live: &ShellLive) -> ShellRestore {
    let Some(saved) = saved else {
        return ShellRestore::None;
    };
    let view = saved.view.trim();
    match view {
        "" => ShellRestore::None,
        "detail" | "escalation" => {
            let id = saved.workspace_id.trim();
            if !id.is_empty() && live.workspace_ids.contains(id) {
                ShellRestore::Workspace(id.into())
            } else {
                ShellRestore::None
            }
        }
        "prdetail" => {
            let id = saved.pr_id.trim();
            if !id.is_empty() && live.pr_ids.contains(id) {
                ShellRestore::Pr(id.into())
            } else {
                ShellRestore::View(ViewName::Pr)
            }
        }
        "project" => {
            let id = saved.project_id.trim();
            if !id.is_empty() && live.project_ids.contains(id) {
                ShellRestore::Project(id.into())
            } else {
                ShellRestore::None
            }
        }
        other => match ViewName::parse(other) {
            Some(v) if TOP_VIEWS.contains(&v) => ShellRestore::View(v),
            _ => ShellRestore::None,
        },
    }
}

/// Welcome is first-run routing, not a restorable shell: skip it so a peek at Get started
/// does not clobber the last real view.
pub fn persistable(st: &ShellState) -> bool {
    let view = st.view.trim();
    !view.is_empty() && view != "welcome"
}

/// The `writeShellState` coalescing: one write in flight, the latest snapshot queued behind
/// it, unchanged snapshots skipped, and a failed write left unrecorded so the next persist
/// retries it.
#[derive(Default)]
pub struct ShellWriter {
    last_written: Option<ShellState>,
    in_flight: bool,
    queued: Option<ShellState>,
}

impl ShellWriter {
    /// The snapshot to send now, if any. Call [`ShellWriter::finish`] when the send settles.
    pub fn offer(&mut self, st: ShellState) -> Option<ShellState> {
        if !persistable(&st) || self.last_written.as_ref() == Some(&st) {
            return None;
        }
        if self.in_flight {
            self.queued = Some(st);
            return None;
        }
        self.in_flight = true;
        Some(st)
    }

    /// The write of `sent` settled; returns the queued snapshot to send next, if any.
    pub fn finish(&mut self, sent: ShellState, ok: bool) -> Option<ShellState> {
        if ok {
            self.last_written = Some(sent);
        }
        self.in_flight = false;
        let next = self.queued.take()?;
        self.offer(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live() -> ShellLive {
        ShellLive {
            workspace_ids: ["WS-1", "WS-2"].iter().map(|s| s.to_string()).collect(),
            pr_ids: ["pr-9"].iter().map(|s| s.to_string()).collect(),
            project_ids: ["storefront"].iter().map(|s| s.to_string()).collect(),
        }
    }

    fn st(view: &str, ws: &str, pr: &str, project: &str) -> ShellState {
        ShellState {
            view: view.into(),
            workspace_id: ws.into(),
            pr_id: pr.into(),
            project_id: project.into(),
        }
    }

    #[test]
    fn snapshot_keeps_only_the_views_ids() {
        let s = |v| snapshot_shell(v, "WS-1", "pr-9", "storefront");
        assert_eq!(s(ViewName::Detail), st("detail", "WS-1", "", ""));
        assert_eq!(s(ViewName::Inbox), st("inbox", "", "", ""));
        assert_eq!(s(ViewName::Schedules), st("schedules", "", "", ""));
        assert_eq!(s(ViewName::Briefing), st("briefing", "", "", ""));
        assert_eq!(s(ViewName::PrDetail), st("prdetail", "", "pr-9", ""));
        assert_eq!(s(ViewName::Project), st("project", "", "", "storefront"));
    }

    #[test]
    fn restore() {
        let live = live();
        let r = |s: Option<ShellState>| resolve_shell_restore(s.as_ref(), &live);
        assert_eq!(r(None), ShellRestore::None);
        assert_eq!(r(Some(ShellState::default())), ShellRestore::None);
        assert_eq!(r(Some(st("nope", "", "", ""))), ShellRestore::None);
        assert_eq!(
            r(Some(st("detail", "WS-1", "", ""))),
            ShellRestore::Workspace("WS-1".into())
        );
        assert_eq!(
            r(Some(st("escalation", "WS-2", "", ""))),
            ShellRestore::Workspace("WS-2".into())
        );
        assert_eq!(r(Some(st("detail", "gone", "", ""))), ShellRestore::None);
        assert_eq!(r(Some(st("detail", "", "", ""))), ShellRestore::None);
        assert_eq!(
            r(Some(st("prdetail", "", "pr-9", ""))),
            ShellRestore::Pr("pr-9".into())
        );
        assert_eq!(
            r(Some(st("prdetail", "", "gone", ""))),
            ShellRestore::View(ViewName::Pr)
        );
        for (v, name) in [
            ("inbox", ViewName::Inbox),
            ("briefing", ViewName::Briefing),
            ("settings", ViewName::Settings),
            ("pr", ViewName::Pr),
            ("schedules", ViewName::Schedules),
            ("issues", ViewName::Issues),
        ] {
            assert_eq!(r(Some(st(v, "", "", ""))), ShellRestore::View(name));
        }
        assert_eq!(r(Some(st("welcome", "", "", ""))), ShellRestore::None);
        assert_eq!(
            r(Some(st("project", "", "", "storefront"))),
            ShellRestore::Project("storefront".into())
        );
        assert_eq!(r(Some(st("project", "", "", "gone"))), ShellRestore::None);
    }

    #[test]
    fn writer_coalesces_and_retries() {
        let mut w = ShellWriter::default();
        let a = st("pr", "", "", "");
        let b = st("settings", "", "", "");
        let c = st("issues", "", "", "");
        assert_eq!(w.offer(st("welcome", "", "", "")), None);
        assert_eq!(w.offer(a.clone()), Some(a.clone()));
        assert_eq!(w.offer(b.clone()), None);
        assert_eq!(w.offer(c.clone()), None);
        // Only the latest queued snapshot goes out after the first settles.
        assert_eq!(w.finish(a.clone(), true), Some(c.clone()));
        assert_eq!(w.finish(c.clone(), false), None);
        // A failed write is retried; a written one is skipped.
        assert_eq!(w.offer(c.clone()), Some(c.clone()));
        assert_eq!(w.finish(c.clone(), true), None);
        assert_eq!(w.offer(c), None);
        assert_eq!(w.offer(a.clone()), Some(a));
    }
}
