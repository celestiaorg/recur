//! Navigation on [`AppState`] (`go`, `openWorkspace`, `openPR`, `openProject`, `routeTarget`,
//! keyboard workspace cycling and the shell snapshot in `lib/stores.ts`).

use std::time::Duration;

use gpui_kit::Context;
use recur_domain::Project;

use crate::chat_route::{parse_chat_route_target, parse_workspace_route_target};
use crate::nav::{Crumb, CrumbStyle, ViewName, seg};
use crate::pr::pr_id_for_chat_workspace;
use crate::shell_state::{ShellLive, ShellRestore, resolve_shell_restore, snapshot_shell};
use crate::state::{AppState, ChatRef, ObsJump, ScanKind, ShellEvent, WorkspaceScanEvent};
use crate::workspace_nav::{
    SCAN_COMMIT_MS, neighbor_workspace_id, project_id_from_main_scan, sidebar_workspace_ids,
};

impl AppState {
    fn crumb_for(&self, v: ViewName) -> Crumb {
        match v {
            ViewName::Detail | ViewName::Escalation => {
                let l = self.selected_workspace();
                let name = l
                    .map(|l| {
                        [&l.short, &l.branch, &l.id]
                            .into_iter()
                            .find(|s| !s.is_empty())
                            .cloned()
                            .unwrap_or_default()
                    })
                    .unwrap_or_default();
                let mut c = vec![
                    seg(
                        l.map(|l| l.project_id.clone()).unwrap_or_default(),
                        CrumbStyle::Bold,
                    ),
                    seg(" / ", CrumbStyle::Plain),
                    seg(name, CrumbStyle::On),
                ];
                if let Some(branch) = l.map(|l| &l.branch).filter(|b| !b.is_empty()) {
                    c.push(seg(" ", CrumbStyle::Plain));
                    c.push(seg(branch.clone(), CrumbStyle::Branch));
                }
                c
            }
            ViewName::Schedules => vec![seg("Schedules", CrumbStyle::BoldOn)],
            ViewName::Issues => vec![seg("Issues", CrumbStyle::BoldOn)],
            ViewName::Pr => vec![seg("Pull requests", CrumbStyle::BoldOn)],
            ViewName::PrDetail => {
                let id = &self.selected_pr_id;
                let n = self.pr(id).map(|p| p.number).filter(|n| *n != 0);
                vec![
                    seg("Pull requests", CrumbStyle::Bold),
                    seg(" / ", CrumbStyle::Plain),
                    seg(
                        format!(
                            "#{}",
                            n.map(|n| n.to_string()).unwrap_or_else(|| id.clone())
                        ),
                        CrumbStyle::On,
                    ),
                ]
            }
            ViewName::Welcome => vec![seg("Get started", CrumbStyle::BoldOn)],
            ViewName::Inbox | ViewName::Briefing => vec![seg("Inbox", CrumbStyle::BoldOn)],
            ViewName::Settings => vec![seg("Settings", CrumbStyle::BoldOn)],
            v => vec![seg(v.as_str(), CrumbStyle::BoldOn)],
        }
    }

    /// `go`: navigate to a top-level view and update the breadcrumb. `focus` is the Overview
    /// section a Briefing landing scrolls to.
    pub fn go(&mut self, v: ViewName, focus: Option<&str>, cx: &mut Context<Self>) {
        // The old full-page escalation view is gone; the notification stream folded into
        // Briefing.
        let v = match v {
            ViewName::Escalation => ViewName::Detail,
            ViewName::Inbox => ViewName::Briefing,
            v => v,
        };
        if v == ViewName::Briefing
            && let Some(f) = focus
        {
            self.briefing_focus = f.to_string();
        }
        if v != ViewName::Detail {
            self.scan_cursor.clear();
            self.commit_workspace_scan("", cx);
        }
        self.view = v;
        let o = &mut self.overlays;
        o.palette = false;
        o.shortcuts = false;
        o.open_in = false;
        o.syntax_mode = false;
        o.run_menu = false;
        o.stop_menu = false;
        self.crumb = self.crumb_for(v);
        self.nav_changed(cx);
    }

    /// `openSettings`: Settings, optionally on a specific pane.
    pub fn open_settings(&mut self, section: Option<&str>, cx: &mut Context<Self>) {
        let section = match section {
            Some("autonomy" | "guardrails") => Some("loops"),
            s => s,
        };
        if let Some(s) = section {
            self.settings_section = s.to_string();
        }
        self.go(ViewName::Settings, None, cx);
    }

    /// `openWorkspace`: always lands on workspace detail. An escalated loop opens on its Loop
    /// chat. `scan` is a keyboard-cycle land, which commits separately.
    pub fn open_workspace(&mut self, id: &str, scan: bool, cx: &mut Context<Self>) {
        if !scan {
            self.scan_cursor = id.to_string();
            self.commit_workspace_scan(id, cx);
        }
        if self
            .pending_chat_focus
            .as_ref()
            .is_some_and(|p| p.workspace_id != id)
        {
            self.pending_chat_focus = None;
        }
        self.selected_workspace_id = id.to_string();
        if self
            .workspace(id)
            .is_some_and(|l| l.state.as_str() == "escalated")
            && self.pending_chat_focus.is_none()
        {
            self.pending_chat_focus = Some(ChatRef {
                workspace_id: id.to_string(),
                code: "MA".into(),
            });
        }
        self.go(ViewName::Detail, None, cx);
    }

    /// `openWorkspaceChat`: a workspace with a specific chat tab (ask_user toast / inbox). A
    /// hidden PR chat host opens its PR.
    pub fn open_workspace_chat(&mut self, workspace_id: &str, code: &str, cx: &mut Context<Self>) {
        let code = code.trim();
        if !code.is_empty() {
            self.pending_chat_focus = Some(ChatRef {
                workspace_id: workspace_id.to_string(),
                code: code.to_string(),
            });
        }
        let host_pr = pr_id_for_chat_workspace(workspace_id, &self.workspaces);
        if !host_pr.is_empty() {
            self.pr_agent_dock_open = true;
            // Same PR already selected: keep its drafts and tab.
            if self.selected_pr_id == host_pr {
                self.go(ViewName::PrDetail, None, cx);
            } else {
                self.open_pr(&host_pr, cx);
            }
            return;
        }
        self.open_workspace(workspace_id, false, cx);
    }

    /// `openPR`: PR detail on the conversation tab with fresh drafts.
    pub fn open_pr(&mut self, id: &str, cx: &mut Context<Self>) {
        self.selected_pr_id = id.to_string();
        self.overlays.pr_review = None;
        cx.emit(ShellEvent::PrOpened(id.to_string()));
        self.go(ViewName::PrDetail, None, cx);
    }

    /// `openProject`: project settings; an empty id opens the quick-create modal instead.
    pub fn open_project(&mut self, id: &str, cx: &mut Context<Self>) {
        if id.is_empty() {
            self.open_new_project(false, cx);
            return;
        }
        self.project_form_id = id.to_string();
        self.go(ViewName::Project, None, cx);
    }

    /// The project being worked in: the Project screen, or the selected workspace's.
    pub fn focused_project_id(&self) -> String {
        if self.view == ViewName::Project && !self.project_form_id.is_empty() {
            return self.project_form_id.clone();
        }
        self.selected_workspace()
            .map(|w| w.project_id.clone())
            .unwrap_or_default()
    }

    /// The git project ⌘N / ⇧⌘N targets: the focused one if it is git; the first git project
    /// only when nothing is focused.
    pub fn git_project_for_new_workspace(&self) -> Option<&Project> {
        let mut all = self.projects.iter().filter(|p| p.is_git);
        let id = self.focused_project_id();
        if id.is_empty() {
            return all.next();
        }
        all.find(|p| p.id == id)
    }

    /// `routeTarget`: inbox / palette targets (`loop:ID`, `workspace:ID:plan`, `pr:482`,
    /// `view:pr`, `chat:ID:PA2`, `question:ID`).
    pub fn route_target(&mut self, target: &str, cx: &mut Context<Self>) {
        let (kind, rest) = target.split_once(':').unwrap_or((target, ""));
        match kind {
            "workspace" | "loop" => {
                let Some((id, panel)) = parse_workspace_route_target(target) else {
                    return;
                };
                self.open_workspace(&id, false, cx);
                if let Some(panel) = panel {
                    self.observatory_jump = Some(ObsJump {
                        workspace_id: id,
                        panel: panel.as_str().into(),
                        ..Default::default()
                    });
                    cx.notify();
                }
            }
            "pr" => self.open_pr(rest, cx),
            "view" => {
                if let Some(v) = ViewName::parse(rest) {
                    self.go(v, None, cx);
                }
            }
            "chat" => match parse_chat_route_target(target) {
                Some((ws, code)) => self.open_workspace_chat(&ws, &code, cx),
                None => self.go(ViewName::Briefing, None, cx),
            },
            "question" => {
                let q = self
                    .user_questions
                    .iter()
                    .find(|q| q.id == rest)
                    .map(|q| (q.workspace_id.clone(), q.agent_code.clone()));
                match q {
                    Some((ws, code)) if !ws.is_empty() => self.open_workspace_chat(&ws, &code, cx),
                    _ => self.go(ViewName::Briefing, None, cx),
                }
            }
            _ => {}
        }
    }

    // ---- keyboard workspace cycling ----

    fn emit_workspace_scan(&mut self, kind: ScanKind, workspace_id: &str, cx: &mut Context<Self>) {
        // A sentinel commit still waits on OpenMainWorkspace: stay pending so the previous
        // workspace is not marked seen and git is not fetched for a row not yet open.
        self.scan_pending = kind == ScanKind::Land
            || (kind == ScanKind::Commit && project_id_from_main_scan(workspace_id).is_some());
        self.scan_event = WorkspaceScanEvent {
            seq: self.scan_event.seq + 1,
            kind,
            workspace_id: workspace_id.to_string(),
        };
        cx.notify();
    }

    fn arm_workspace_scan_commit(&mut self, workspace_id: &str, cx: &mut Context<Self>) {
        let id = workspace_id.to_string();
        self.scan_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(SCAN_COMMIT_MS))
                .await;
            let _ = this.update(cx, |s, cx| {
                s.scan_timer = None;
                s.emit_workspace_scan(ScanKind::Commit, &id, cx);
            });
        }));
    }

    fn commit_workspace_scan(&mut self, workspace_id: &str, cx: &mut Context<Self>) {
        self.scan_timer = None;
        self.emit_workspace_scan(ScanKind::Commit, workspace_id, cx);
    }

    /// Clear the pending flag once a sentinel commit's workspace has opened.
    pub fn set_scan_pending(&mut self, pending: bool, cx: &mut Context<Self>) {
        if self.scan_pending != pending {
            self.scan_pending = pending;
            cx.notify();
        }
    }

    /// `cycleWorkspace`: the next (+1) or previous (-1) sidebar workspace, wrapping. A land
    /// peek-expands collapsed ancestors and commits after [`SCAN_COMMIT_MS`] of dwell.
    pub fn cycle_workspace(&mut self, delta: i32, cx: &mut Context<Self>) {
        if self.overlays.blocks_workspace_cycle() {
            return;
        }
        let show_hidden = self.show_hidden_projects();
        let projects: Vec<&Project> = self
            .projects
            .iter()
            .filter(|p| show_hidden || !p.hidden)
            .collect();
        let ids = sidebar_workspace_ids(&self.workspaces, &projects, &self.project_groups);
        let current = if self.scan_cursor.is_empty() {
            self.selected_workspace_id.clone()
        } else {
            self.scan_cursor.clone()
        };
        let Some(next) = neighbor_workspace_id(&ids, &current, delta) else {
            return;
        };
        self.scan_cursor = next.clone();
        if project_id_from_main_scan(&next).is_none() {
            self.open_workspace(&next, true, cx);
        }
        self.emit_workspace_scan(ScanKind::Land, &next, cx);
        self.arm_workspace_scan_commit(&next, cx);
    }

    // ---- shell persistence (`lib/shell-state.ts`) ----

    fn nav_changed(&mut self, cx: &mut Context<Self>) {
        self.persist_open_shell(cx);
        cx.notify();
    }

    /// Persist the last view only after boot restore, so the in-memory Briefing default
    /// cannot clobber a saved shell on the way up.
    pub fn enable_shell_persist(&mut self, cx: &mut Context<Self>) {
        if self.shell_persist {
            return;
        }
        self.shell_persist = true;
        self.persist_open_shell(cx);
    }

    /// Record a saved project id that changed outside `openProject` (a new project's save).
    pub fn set_project_form_id(&mut self, id: &str, cx: &mut Context<Self>) {
        self.project_form_id = id.to_string();
        self.nav_changed(cx);
    }

    fn persist_open_shell(&mut self, cx: &mut Context<Self>) {
        if !self.shell_persist {
            return;
        }
        let snap = snapshot_shell(
            self.view,
            &self.selected_workspace_id,
            &self.selected_pr_id,
            &self.project_form_id,
        );
        if let Some(st) = self.shell_writer.offer(snap) {
            self.write_shell_state(st, cx);
        }
    }

    fn write_shell_state(&mut self, st: recur_services::api::ShellState, cx: &mut Context<Self>) {
        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            let ok = services.shell.set_shell_state(&st).await.is_ok();
            let _ = this.update(cx, |s, cx| {
                if let Some(next) = s.shell_writer.finish(st, ok) {
                    s.write_shell_state(next, cx);
                }
            });
        })
        .detach();
    }

    /// The boot routing in App.svelte: Welcome for an empty app, else the restored shell.
    pub fn apply_boot_route(
        &mut self,
        saved: Option<&recur_services::api::ShellState>,
        cx: &mut Context<Self>,
    ) {
        if self.projects.is_empty() {
            self.go(ViewName::Welcome, None, cx);
            return;
        }
        let live = ShellLive {
            workspace_ids: self
                .workspaces
                .iter()
                .filter(|w| !w.archived)
                .map(|w| w.id.clone())
                .collect(),
            pr_ids: self.prs.iter().map(|p| p.id.clone()).collect(),
            project_ids: self.projects.iter().map(|p| p.id.clone()).collect(),
        };
        match resolve_shell_restore(saved, &live) {
            ShellRestore::None => {}
            ShellRestore::Workspace(id) => self.open_workspace(&id, false, cx),
            ShellRestore::Pr(id) => self.open_pr(&id, cx),
            ShellRestore::Project(id) => self.open_project(&id, cx),
            ShellRestore::View(v) => self.go(v, None, cx),
        }
    }
}
