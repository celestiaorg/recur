//! Named run commands (`lib/run-commands.ts`) and the run sessions they start in Observatory
//! terminals (the store half of `lib/run-dev.ts`; the timers and calls live on
//! [`crate::AppState`]).

use std::collections::{BTreeMap, HashSet};

use recur_domain::{Project, RunCommand, Workspace, default_run, effective_runs};

/// Named runs from project.toml `[[run]]` (`projectRuns`).
pub fn project_runs(p: Option<&Project>) -> Vec<RunCommand> {
    p.map(effective_runs).unwrap_or_default()
}

/// The pinned command when it still exists, otherwise the first (`defaultRun`).
pub fn project_default_run(p: Option<&Project>) -> Option<RunCommand> {
    let p = p?;
    let run = default_run(p);
    (!run.cmd.is_empty()).then_some(run)
}

/// `runLabel`: "Run dev" / "Stop test".
pub fn run_label(verb: &str, name: &str) -> String {
    let n = name.trim();
    format!("{verb} {}", if n.is_empty() { "dev" } else { n })
}

/// Substring match on name or command. An empty query keeps the incoming order.
pub fn filter_runs(list: &[RunCommand], query: &str) -> Vec<RunCommand> {
    let q = query.trim().to_lowercase();
    list.iter()
        .filter(|r| {
            q.is_empty() || r.name.to_lowercase().contains(&q) || r.cmd.to_lowercase().contains(&q)
        })
        .cloned()
        .collect()
}

/// Move the pinned run to the front; no-op if it isn't in the list.
pub fn order_runs(mut list: Vec<RunCommand>, pinned: &str) -> Vec<RunCommand> {
    if pinned.is_empty() {
        return list;
    }
    if let Some(i) = list.iter().position(|r| r.name == pinned)
        && i > 0
    {
        let r = list.remove(i);
        list.insert(0, r);
    }
    list
}

pub fn project_for_workspace<'a>(
    workspace_id: &str,
    workspaces: &[Workspace],
    projects: &'a [Project],
) -> Option<&'a Project> {
    let ws = workspaces.iter().find(|w| w.id == workspace_id)?;
    projects.iter().find(|p| p.id == ws.project_id)
}

/// `workspaceRun`: the pinned (or first) run command for a workspace.
pub fn workspace_run(
    workspace_id: &str,
    workspaces: &[Workspace],
    projects: &[Project],
) -> Option<RunCommand> {
    project_default_run(project_for_workspace(workspace_id, workspaces, projects))
}

/// `workspaceDevCmd`: the default run command string, or "".
pub fn workspace_dev_cmd(
    workspace_id: &str,
    workspaces: &[Workspace],
    projects: &[Project],
) -> String {
    workspace_run(workspace_id, workspaces, projects)
        .map(|r| r.cmd)
        .unwrap_or_default()
}

pub fn workspace_runs(
    workspace_id: &str,
    workspaces: &[Workspace],
    projects: &[Project],
) -> Vec<RunCommand> {
    project_runs(project_for_workspace(workspace_id, workspaces, projects))
}

/// The latest pending "run" request; the terminal dock consumes matching ones.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunDevRequest {
    pub workspace_id: String,
    pub cmd: String,
    pub name: String,
    pub nonce: u64,
}

/// Run names in start order, as `runSessionBy`'s object keys keep insertion order.
type ByName = indexmap::IndexMap<String, String>;

/// Run sessions per workspace. In memory only: the PTY outlives a closed dock, so this
/// survives unmount.
#[derive(Default)]
pub struct RunStore {
    pub request: Option<RunDevRequest>,
    /// Live session id per workspace per run name (`runSessionBy`).
    pub sessions: BTreeMap<String, ByName>,
    /// Dedicated tab per run name; survives stop so the next start types into the same shell
    /// (`runTabBy`).
    pub tabs: BTreeMap<String, ByName>,
    /// In-flight `InterruptTerminal` per session.
    pub(crate) inflight_stop: HashSet<String>,
    /// Ignore `term.idle` while Interrupt+Write is in flight.
    pub(crate) starting: HashSet<String>,
    /// Recur launched a named run on this session; `term.busy` may re-adopt it.
    pub(crate) launched: HashSet<String>,
    pub(crate) saw_busy: HashSet<String>,
}

fn set_in(map: &mut BTreeMap<String, ByName>, ws: &str, name: &str, session: &str) -> bool {
    let by = map.entry(ws.to_string()).or_default();
    if by.get(name).is_some_and(|s| s == session) {
        return false;
    }
    by.insert(name.to_string(), session.to_string());
    true
}

fn clear_in(
    map: &mut BTreeMap<String, ByName>,
    ws: &str,
    name: &str,
    session: Option<&str>,
) -> bool {
    let Some(by) = map.get_mut(ws) else {
        return false;
    };
    match by.get(name) {
        Some(s) if session.is_none_or(|want| want == s) => {}
        _ => return false,
    }
    by.shift_remove(name);
    if by.is_empty() {
        map.remove(ws);
    }
    true
}

impl RunStore {
    /// `runningRuns`: `(name, session id)` pairs.
    pub fn running_runs(&self, workspace_id: &str) -> Vec<(String, String)> {
        self.sessions
            .get(workspace_id)
            .map(|by| by.iter().map(|(n, s)| (n.clone(), s.clone())).collect())
            .unwrap_or_default()
    }

    pub fn is_any_run_running(&self, workspace_id: &str) -> bool {
        !self.running_runs(workspace_id).is_empty()
    }

    pub fn is_run_running(&self, workspace_id: &str, name: &str) -> bool {
        !name.is_empty()
            && self
                .sessions
                .get(workspace_id)
                .is_some_and(|by| by.contains_key(name))
    }

    pub fn run_tab_id(&self, workspace_id: &str, name: &str) -> String {
        self.tabs
            .get(workspace_id)
            .and_then(|by| by.get(name))
            .cloned()
            .unwrap_or_default()
    }

    pub fn run_name_for_session(&self, workspace_id: &str, session_id: &str) -> String {
        if session_id.is_empty() {
            return String::new();
        }
        [&self.sessions, &self.tabs]
            .into_iter()
            .filter_map(|m| m.get(workspace_id))
            .find_map(|by| by.iter().find(|(_, s)| *s == session_id))
            .map(|(n, _)| n.clone())
            .unwrap_or_default()
    }

    pub fn set_run_session(&mut self, workspace_id: &str, name: &str, session_id: &str) -> bool {
        if workspace_id.is_empty() || name.is_empty() || session_id.is_empty() {
            return false;
        }
        let changed = set_in(&mut self.sessions, workspace_id, name, session_id);
        self.set_run_tab(workspace_id, name, session_id) || changed
    }

    pub fn clear_run_session(
        &mut self,
        workspace_id: &str,
        name: &str,
        session_id: Option<&str>,
    ) -> bool {
        clear_in(&mut self.sessions, workspace_id, name, session_id)
    }

    pub fn clear_run_by_session_id(&mut self, session_id: &str) -> bool {
        if session_id.is_empty() {
            return false;
        }
        let before = self.sessions.len();
        let mut changed = false;
        for by in self.sessions.values_mut() {
            let n = by.len();
            by.retain(|_, s| s != session_id);
            changed |= by.len() != n;
        }
        self.sessions.retain(|_, by| !by.is_empty());
        changed || self.sessions.len() != before
    }

    pub fn set_run_tab(&mut self, workspace_id: &str, name: &str, session_id: &str) -> bool {
        if workspace_id.is_empty() || name.is_empty() || session_id.is_empty() {
            return false;
        }
        set_in(&mut self.tabs, workspace_id, name, session_id)
    }

    pub fn clear_run_tab(
        &mut self,
        workspace_id: &str,
        name: &str,
        session_id: Option<&str>,
    ) -> bool {
        clear_in(&mut self.tabs, workspace_id, name, session_id)
    }

    pub fn clear_run_tab_by_session_id(&mut self, workspace_id: &str, session_id: &str) -> bool {
        if workspace_id.is_empty() || session_id.is_empty() {
            return false;
        }
        let Some(by) = self.tabs.get_mut(workspace_id) else {
            return false;
        };
        let n = by.len();
        by.retain(|_, s| s != session_id);
        let changed = by.len() != n;
        if by.is_empty() {
            self.tabs.remove(workspace_id);
        }
        changed
    }

    /// `takeRunDevRequest`: claim a pending request for this workspace, once.
    pub fn take_request(&mut self, workspace_id: &str) -> Option<RunDevRequest> {
        if self.request.as_ref()?.workspace_id != workspace_id {
            return None;
        }
        self.request.take()
    }

    /// `waitForSessionStop`: an `InterruptTerminal` for this session is still in flight.
    pub fn stop_in_flight(&self, session_id: &str) -> bool {
        self.inflight_stop.contains(session_id)
    }

    pub fn mark_run_starting(&mut self, session_id: &str) {
        if session_id.is_empty() {
            return;
        }
        self.starting.insert(session_id.into());
        self.launched.insert(session_id.into());
        self.saw_busy.remove(session_id);
    }

    pub fn mark_run_started(&mut self, session_id: &str) {
        self.starting.remove(session_id);
    }

    /// `term.busy`: the session has a foreground command; re-adopt a launched run.
    pub(crate) fn on_busy(&mut self, session_id: &str) {
        self.saw_busy.insert(session_id.into());
        self.starting.remove(session_id);
        if !self.launched.contains(session_id) {
            return;
        }
        let hit = self.tabs.iter().find_map(|(ws, by)| {
            by.iter()
                .find(|(_, s)| *s == session_id)
                .map(|(n, _)| (ws.clone(), n.clone()))
        });
        if let Some((ws, name)) = hit {
            self.set_run_session(&ws, &name, session_id);
        }
    }

    /// `term.idle` / `term.exit`: the run finished unless a restart is in flight.
    pub(crate) fn on_done(&mut self, session_id: &str) {
        self.launched.remove(session_id);
        if !self.starting.contains(session_id) {
            self.clear_run_by_session_id(session_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rc(name: &str, cmd: &str) -> RunCommand {
        RunCommand {
            name: name.into(),
            cmd: cmd.into(),
        }
    }

    fn proj(runs: Vec<RunCommand>, pinned: &str) -> Project {
        Project {
            id: "p".into(),
            run_commands: runs,
            pinned_run: pinned.into(),
            ..Project::default()
        }
    }

    /// `runningRuns` lists runs in start order (`runSessionBy`'s object keys), not by name.
    #[test]
    fn running_runs_keep_start_order() {
        let mut r = RunStore::default();
        r.set_run_session("ws", "test", "s1");
        r.set_run_session("ws", "dev", "s2");
        r.set_run_session("ws", "check", "s3");
        r.set_run_session("ws", "test", "s4");
        let names = |r: &RunStore| -> Vec<String> {
            r.running_runs("ws").into_iter().map(|(n, _)| n).collect()
        };
        assert_eq!(
            names(&r),
            ["test", "dev", "check"],
            "a restart keeps its place"
        );
        r.clear_run_session("ws", "dev", None);
        assert_eq!(names(&r), ["test", "check"]);
    }

    #[test]
    fn runs_and_default() {
        assert!(project_runs(Some(&proj(vec![], ""))).is_empty());
        let p = proj(
            vec![rc("dev", "wails3 task dev"), rc("test", "go test")],
            "",
        );
        assert_eq!(
            project_runs(Some(&p)),
            [rc("dev", "wails3 task dev"), rc("test", "go test")]
        );
        let p = proj(vec![rc("dev", "a"), rc("test", "b")], "test");
        assert_eq!(project_default_run(Some(&p)), Some(rc("test", "b")));
        let p = proj(vec![rc("dev", "a"), rc("test", "b")], "nope");
        assert_eq!(
            project_default_run(Some(&p)).map(|r| r.name),
            Some("dev".into())
        );
        assert_eq!(project_default_run(Some(&proj(vec![], ""))), None);
    }

    #[test]
    fn labels_filter_order() {
        assert_eq!(run_label("Run", "dev"), "Run dev");
        assert_eq!(run_label("Stop", "test"), "Stop test");
        assert_eq!(run_label("Run", "  "), "Run dev");
        let list = [
            rc("dev", "wails3 task dev"),
            rc("test:go", "wails3 task test:go"),
            rc("tidy", "wails3 task common:go:mod:tidy"),
        ];
        assert_eq!(filter_runs(&list, ""), list);
        assert_eq!(filter_runs(&list, "   "), list);
        let names = |v: Vec<RunCommand>| v.into_iter().map(|r| r.name).collect::<Vec<_>>();
        assert_eq!(names(filter_runs(&list, "tid")), ["tidy"]);
        assert_eq!(names(filter_runs(&list, "test:go")), ["test:go"]);
        let list = vec![rc("dev", ""), rc("test", ""), rc("lint", "")];
        assert_eq!(names(order_runs(list.clone(), "")), ["dev", "test", "lint"]);
        assert_eq!(names(order_runs(list, "test")), ["test", "dev", "lint"]);
    }

    #[test]
    fn session_bookkeeping() {
        let mut s = RunStore::default();
        assert!(s.set_run_session("ws", "dev", "t1"));
        assert!(!s.set_run_session("ws", "dev", "t1"));
        assert_eq!(
            s.running_runs("ws"),
            [("dev".to_string(), "t1".to_string())]
        );
        assert_eq!(s.run_tab_id("ws", "dev"), "t1");
        assert_eq!(s.run_name_for_session("ws", "t1"), "dev");
        assert!(!s.clear_run_session("ws", "dev", Some("other")));
        assert!(s.clear_run_session("ws", "dev", Some("t1")));
        assert!(!s.is_any_run_running("ws"));
        // The dedicated tab survives the stop.
        assert_eq!(s.run_name_for_session("ws", "t1"), "dev");

        s.mark_run_starting("t1");
        s.on_done("t1");
        assert!(
            !s.is_run_running("ws", "dev"),
            "idle while starting keeps nothing running"
        );
        s.mark_run_starting("t1");
        s.on_busy("t1");
        assert!(
            s.is_run_running("ws", "dev"),
            "busy re-adopts a launched run"
        );
        s.mark_run_started("t1");
        s.on_done("t1");
        assert!(!s.is_run_running("ws", "dev"));
        assert!(s.clear_run_tab_by_session_id("ws", "t1"));
        assert!(s.tabs.is_empty());
    }
}
