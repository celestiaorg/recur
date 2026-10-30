//! The side docks and the main column's surfaces: which panel each side shows, the left
//! gutter and PR-detail agent dock, branch review versus editor (and editor-hidden), focus
//! zones and the ⌘1 / ⌘2 / ⌘3 workbench focus, the terminal maximize overlay, findings, and
//! the editor's native full screen.

use gpui_kit::FocusHandle;
use recur_ui::guard::Guarded;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::{App, AppContext as _, Context, Task, Window};
use recur_domain::Finding;
use recur_ui::widgets::ToastKind;
use recur_view_core::prefs::{CodePanelKey, PanelSide};
use recur_view_core::{call, toast};
use recur_view_review::review_diff::{ReviewTab, write_review_tab};
use recur_view_review::surface::{ReviewSurface, SurfaceEvent, SurfaceProps};

use crate::model::{FocusZone, MainSurface, Pane};
use crate::obs::{
    Observatory, ObservatoryEvent, OpenRequest, OverlayRect, TERM_MAX_MS, TermMaxPhase,
};

impl Observatory {
    // ---- docks ----

    pub(crate) fn set_left_gutter_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.props.left_open.is_some() {
            cx.emit(ObservatoryEvent::ToggleLeft(open));
        } else {
            self.local_left_open = open;
        }
        self.persist_editor_session(cx);
        cx.notify();
    }

    pub(crate) fn set_show_side(&mut self, open: bool, cx: &mut Context<Self>) {
        self.show_side = open;
        self.persist_editor_session(cx);
        cx.notify();
    }

    /// The portaled PR dock's hide toggle, for PR detail's focus restore.
    pub fn pr_dock_toggle_focus(&self) -> Option<FocusHandle> {
        Some(self.pr_toggle_focus.clone())
    }

    /// The PR-detail agent dock, when that is the visible chat.
    pub(crate) fn set_pr_agent_dock_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.pr_agent_dock_open = open;
            cx.notify();
        });
        if !open {
            cx.emit(ObservatoryEvent::PrDetailFocus(false));
        }
    }

    /// ⌥⌘B.
    pub(crate) fn toggle_visible_left_panel(&mut self, cx: &mut Context<Self>) {
        if self.portal_agent_to_pr(cx) {
            let open = self.state.read(cx).pr_agent_dock_open;
            self.set_pr_agent_dock_open(!open, cx);
        } else if self.has_left(cx) {
            let open = self.left_gutter_open();
            self.set_left_gutter_open(!open, cx);
        }
    }

    /// Keep the active panel valid when Settings moves panels between sides.
    pub(crate) fn keep_panel_kinds_valid(&mut self, cx: &mut Context<Self>) {
        let left = self.panels_on(PanelSide::Left, cx);
        let right = self.panels_on(PanelSide::Right, cx);
        if !left.is_empty() && !left.contains(&self.left_kind) {
            self.left_kind = left[0];
        }
        if !right.is_empty() && !right.contains(&self.right_kind) {
            self.right_kind = right[0];
        }
    }

    /// The branch review, once it has been shown.
    pub fn review_surface(
        &self,
    ) -> Option<&gpui_kit::Entity<recur_view_review::surface::ReviewSurface>> {
        self.review_surface.as_ref()
    }

    /// Show the source control panel with its commit message box focused; whether it took
    /// focus.
    pub fn focus_commit_msg(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.select_code_panel(CodePanelKey::Scm, window, cx);
        self.inputs.focus_commit_msg(window, cx)
    }

    /// `selectCodePanel`: show a panel on its side (and load / focus what it needs).
    pub fn select_code_panel(
        &mut self,
        next: CodePanelKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let side = self.panel_side(next, cx);
        if self.props.main_blocked && side != PanelSide::Left {
            return;
        }
        if side == PanelSide::Left {
            self.left_kind = next;
            self.set_left_gutter_open(true, cx);
        } else {
            self.right_kind = next;
            self.set_show_side(true, cx);
        }
        if next == CodePanelKey::Findings {
            self.load_findings(cx).detach();
        }
        if next == CodePanelKey::Search {
            let input = self.inputs.query.clone();
            window.defer(cx, move |window, cx| {
                input.update(cx, |s, cx| {
                    s.focus(window, cx);
                    s.select_all(window, cx);
                });
            });
        }
        cx.notify();
    }

    /// `paneBadgeCount`.
    pub(crate) fn pane_badge_count(&self, key: CodePanelKey, cx: &App) -> usize {
        match key {
            CodePanelKey::Maestro => self.unseen_chat_count(cx),
            CodePanelKey::Scm => self.working_dirty_count(),
            CodePanelKey::Findings => self.open_findings_count(),
            CodePanelKey::Plan => self.plan_open_count(cx),
            CodePanelKey::Artifacts => self.unseen_artifact_count(cx),
            CodePanelKey::Problems => self.problem_counts(cx).total,
            _ => 0,
        }
    }

    /// `paneBadgeTitle`.
    pub(crate) fn pane_badge_title(&self, key: CodePanelKey, cx: &App) -> String {
        let (_, title) = self.panel_label(key);
        let n = self.pane_badge_count(key, cx);
        if n == 0 {
            return title;
        }
        match key {
            CodePanelKey::Maestro => format!("{title} ({n} unread)"),
            CodePanelKey::Scm => format!("{title} ({n})"),
            CodePanelKey::Findings => format!("{title} ({n} open)"),
            CodePanelKey::Plan => format!("{title} ({n} remaining)"),
            CodePanelKey::Artifacts => format!("{title} ({n} new)"),
            CodePanelKey::Problems => {
                let c = self.problem_counts(cx);
                format!("{title} ({}E {}W)", c.errors, c.warnings)
            }
            _ => title,
        }
    }

    /// The workspace's LSP problems (`problemsList`).
    pub(crate) fn problems(&self, cx: &App) -> Vec<crate::lsp::problems::LspProblem> {
        let all = crate::lsp::problems::peek_problems(cx);
        crate::lsp::problems::problems_for_workspace(&all, &self.props.workspace_id)
    }

    pub(crate) fn problem_counts(&self, cx: &App) -> crate::lsp::problems::ProblemCounts {
        crate::lsp::problems::count_problems(&self.problems(cx))
    }

    // ---- branch review ----

    /// `setMainSurface`: review only for git roots; persisted with the session.
    pub(crate) fn set_main_surface(&mut self, next: MainSurface, cx: &mut Context<Self>) {
        let pref = if !self.is_git(cx) && next == MainSurface::Review {
            MainSurface::Editor
        } else {
            next
        };
        if pref == self.main_surface_pref {
            return;
        }
        self.main_surface_pref = pref;
        self.persist_or_clear_editor_session(cx);
        cx.notify();
    }

    fn close_review_surface(&mut self, cx: &mut Context<Self>) {
        let restore = std::mem::take(&mut self.review_unhid_for);
        self.set_main_surface(MainSurface::Editor, cx);
        if !restore.is_empty() {
            self.state
                .update(cx, |s, cx| s.set_editor_hidden(&restore, true, cx));
        }
    }

    /// Review lives where editor-hidden hides; lift it and restore on close.
    fn unhide_for_review(&mut self, cx: &mut Context<Self>) {
        let id = self.props.workspace_id.clone();
        if !id.is_empty() && self.editor_hidden(cx) {
            self.review_unhid_for = id.clone();
            self.state
                .update(cx, |s, cx| s.set_editor_hidden(&id, false, cx));
        } else {
            self.review_unhid_for.clear();
        }
    }

    /// `toggleReview` (⌥⌘R, the toolbar button).
    pub(crate) fn toggle_review(&mut self, cx: &mut Context<Self>) {
        if self.props.main_blocked {
            return;
        }
        if self.main_surface(cx) == MainSurface::Review {
            self.close_review_surface(cx);
            return;
        }
        self.set_main_surface(MainSurface::Review, cx);
        self.unhide_for_review(cx);
    }

    /// `showReviewSurface`.
    pub(crate) fn show_review_surface(&mut self, cx: &mut Context<Self>) {
        if self.props.main_blocked || !self.is_git(cx) {
            return;
        }
        if self.main_surface(cx) != MainSurface::Review {
            self.set_main_surface(MainSurface::Review, cx);
            self.unhide_for_review(cx);
        }
    }

    /// `openWorkingReview`: the Commits tab's uncommitted scope.
    pub(crate) fn open_working_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.props.main_blocked || !self.is_git(cx) {
            return;
        }
        self.show_review_surface(cx);
        self.set_review_tab(ReviewTab::Commits, cx);
        self.sync_review_surface(window, cx);
        if let Some(r) = self.review_surface.clone() {
            r.update(cx, |r, cx| r.open_working(window, cx));
        }
    }

    /// `openTurnReview`: a chat turn's checkpoint diff as a commit scope.
    pub fn open_turn_review(
        &mut self,
        c: recur_domain::Commit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.props.main_blocked || !self.is_git(cx) {
            return;
        }
        self.show_review_surface(cx);
        self.set_review_tab(ReviewTab::Commits, cx);
        self.sync_review_surface(window, cx);
        if let Some(r) = self.review_surface.clone() {
            r.update(cx, |r, cx| r.open_commit(c, window, cx));
        }
    }

    /// Keyboard ⌥⌘R parks focus on the surface it opened (or back on the editor).
    pub(crate) fn park_after_review_toggle(
        &mut self,
        opening: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let token = self.root_token;
        cx.defer_in(window, move |o, window, cx| {
            if o.root_token != token {
                return;
            }
            o.sync_editors(window, cx);
            o.sync_review_surface(window, cx);
            if opening {
                if let Some(r) = o.review_surface.clone() {
                    let _ = r;
                    window.focus(&o.review_focus, cx);
                    return;
                }
                window.focus(&o.main_focus, cx);
                return;
            }
            let want_split = o.split_open && o.focused_pane == Pane::Split && !o.show_diff();
            let target = if want_split {
                o.split_editor.clone()
            } else if o.show_diff() {
                o.merge_editor.as_ref().map(|(_, e)| e.clone())
            } else {
                o.main_editor.clone()
            };
            match target {
                Some(e) => e.update(cx, |e, cx| e.focus(window, cx)),
                None => window.focus(&o.main_focus, cx),
            }
        });
    }

    pub(crate) fn set_review_tab(&mut self, next: ReviewTab, cx: &mut Context<Self>) {
        if next == ReviewTab::Changes
            && let Some(r) = self.review_surface.clone()
            && let Some(w) = self.window
        {
            let _ = w.update(cx, |_, window, cx| {
                r.update(cx, |r, cx| r.clear_commit_scope(window, cx))
            });
        }
        self.review_tab = next;
        self.state
            .update(cx, |s, _| write_review_tab(s.storage_mut(), next));
        cx.notify();
    }

    pub(crate) fn cycle_review_tab(&mut self, dir: i32, cx: &mut Context<Self>) {
        if !self.review_has_commits(cx) {
            return;
        }
        let tabs = [ReviewTab::Changes, ReviewTab::Commits];
        let i = tabs
            .iter()
            .position(|t| *t == self.review_strip_tab(cx))
            .unwrap_or(0) as i32;
        let next = tabs[((i + dir).rem_euclid(2)) as usize];
        self.set_review_tab(next, cx);
    }

    /// `showEditorSurface`.
    pub(crate) fn show_editor_surface(&mut self, cx: &mut Context<Self>) {
        self.review_unhid_for.clear();
        self.set_main_surface(MainSurface::Editor, cx);
        let id = self.props.workspace_id.clone();
        if !id.is_empty() && self.editor_hidden(cx) {
            self.state
                .update(cx, |s, cx| s.set_editor_hidden(&id, false, cx));
        }
    }

    /// ⌥⌘E.
    pub(crate) fn toggle_editor_surface(&mut self, cx: &mut Context<Self>) {
        let id = self.props.workspace_id.clone();
        if id.is_empty() || self.props.main_blocked {
            return;
        }
        self.review_unhid_for.clear();
        if self.main_surface(cx) != MainSurface::Editor || self.editor_hidden(cx) {
            self.show_editor_surface(cx);
            return;
        }
        self.state
            .update(cx, |s, cx| s.set_editor_hidden(&id, true, cx));
    }

    /// Terminal beside the editor instead of under it.
    pub(crate) fn toggle_term_beside(&mut self, cx: &mut Context<Self>) {
        let id = self.props.workspace_id.clone();
        if id.is_empty() || self.props.main_blocked || self.term_overlaying() {
            return;
        }
        let next = !self.term_beside(cx);
        self.state.update(cx, |s, cx| {
            s.set_terminal_beside(&id, next, cx);
            if next && s.dock.is_editor_hidden(&id) {
                s.set_editor_hidden(&id, false, cx);
            }
        });
    }

    /// The `ReviewSurface` props (`<ReviewSurface …>`), mounted for git roots.
    /// [`ReviewSig`] holds them comparably, so an unchanged frame skips the push.
    pub(crate) fn sync_review_surface(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_git(cx) {
            self.review_surface = None;
            return;
        }
        let excludes = crate::explorer::explorer_exclude(self.state.read(cx).storage());
        let findings = self.review_findings_by_path(cx);
        let mut dirty: Vec<String> = self.working_status.keys().cloned().collect();
        dirty.sort();
        let sig = ReviewSig {
            project_id: self.props.project_id.clone(),
            workspace_id: self.props.workspace_id.clone(),
            base_label: self.review_base(cx),
            refresh: self.review_refresh,
            active: self.main_surface(cx) == MainSurface::Review,
            tab: self.review_tab,
            count: self.review_paths(cx).len(),
            excludes,
            findings,
            dirty,
            restore: self.review_restore.clone(),
        };
        if self.review_surface.is_some() && self.review_sig.as_ref() == Some(&sig) {
            return;
        }
        self.review_sig = Some(sig.clone());
        let props = SurfaceProps {
            project_id: sig.project_id,
            workspace_id: sig.workspace_id,
            base_label: sig.base_label,
            refresh: sig.refresh,
            active: sig.active,
            tab: sig.tab,
            count: sig.count,
            exclude_matchers: Some(Rc::new(recur_code::glob::compile_excludes(&sig.excludes))),
            findings_by_path: Some(Rc::new(sig.findings)),
            working_dirty_count: sig.dirty.len(),
            dirty_paths: Some(Rc::new(sig.dirty.into_iter().collect())),
            restore: sig.restore,
        };
        match self.review_surface.clone() {
            Some(r) => r.update(cx, |r, cx| r.set_props(props, window, cx)),
            None => {
                let state = self.state.clone();
                let r = cx.new(|cx| ReviewSurface::new(state, props, window, cx));
                let weak = self.weak.clone();
                // The review host is cached: line notes' text boxes mount here, above it.
                let files = r.read(cx).files_view().clone();
                files.update(cx, |f, _| f.mount_note_portals_above());
                r.update(cx, |r, cx| {
                    let add: recur_view_review::diff_file::AddFindingFn =
                        Rc::new(move |path, line, body, end, _window, cx: &mut App| {
                            match weak.upgrade() {
                                Some(o) => o.update(cx, |o, cx| {
                                    o.add_review_finding(path, line, body, end, cx)
                                }),
                                None => Task::ready(false),
                            }
                        });
                    r.set_add_finding(Some(add), window, cx);
                });
                let sub =
                    cx.guarded_subscribe_in(&r, window, |o, _, ev: &SurfaceEvent, window, cx| {
                        match ev {
                            SurfaceEvent::TabChange(t) => o.set_review_tab(*t, cx),
                            SurfaceEvent::OpenFile(path, line) => {
                                o.open_path(
                                    OpenRequest::new(path.clone())
                                        .line(line.unwrap_or(0).max(0) as usize)
                                        .prefer_diff(true),
                                    window,
                                    cx,
                                )
                                .detach();
                            }
                            SurfaceEvent::OpenFinding(f) => o.open_review_finding(f, window, cx),
                            SurfaceEvent::ShowFindings => {
                                o.select_code_panel(CodePanelKey::Findings, window, cx)
                            }
                            SurfaceEvent::Bound(b) => {
                                if o.review_bound != *b {
                                    o.review_bound = b.clone();
                                    cx.notify();
                                }
                            }
                        }
                    });
                self.review_subs = vec![sub];
                self.review_surface = Some(r);
            }
        }
    }

    /// Bump the review stack's refresh after git status moves.
    pub(crate) fn sync_review_props_later(&mut self, cx: &mut Context<Self>) {
        cx.notify();
    }

    /// A line-comment composer is open in review (live refresh holds the stack still).
    pub(crate) fn review_composer_open(&self, cx: &App) -> bool {
        self.review_surface
            .as_ref()
            .is_some_and(|r| r.read(cx).composer_open(cx))
    }

    /// `reviewFindingsByPath`: open findings grouped by rel path.
    pub(crate) fn review_findings_by_path(
        &self,
        cx: &App,
    ) -> recur_view_review::findings::FindingsByPath {
        let mut out: recur_view_review::findings::FindingsByPath = Vec::new();
        for f in &self.findings {
            if !(f.status.is_empty() || f.status == "open") || f.file.is_empty() {
                continue;
            }
            let p = self.finding_rel_path(&f.file, cx);
            if p.is_empty() || p.starts_with('/') {
                continue;
            }
            match out.iter_mut().find(|(k, _)| *k == p) {
                Some((_, list)) => list.push(f.clone()),
                None => out.push((p, vec![f.clone()])),
            }
        }
        out
    }

    /// `openFindingsByPath`: explorer flags (count and worst severity).
    pub(crate) fn open_findings_by_path(
        &self,
        cx: &App,
    ) -> std::collections::HashMap<String, recur_view_review::findings::FindingFlag> {
        let mut map: std::collections::HashMap<String, recur_view_review::findings::FindingFlag> =
            std::collections::HashMap::new();
        for f in &self.findings {
            if !(f.status.is_empty() || f.status == "open") || f.file.is_empty() {
                continue;
            }
            let p = self.finding_rel_path(&f.file, cx);
            if p.is_empty() || p.starts_with('/') {
                continue;
            }
            let sev = f.severity.to_string();
            let sev = if sev.is_empty() {
                "info".to_string()
            } else {
                sev
            };
            match map.get_mut(&p) {
                None => {
                    map.insert(
                        p,
                        recur_view_review::findings::FindingFlag {
                            count: 1,
                            severity: sev,
                        },
                    );
                }
                Some(cur) => {
                    cur.count += 1;
                    if recur_view_review::findings::severity_rank(&sev)
                        > recur_view_review::findings::severity_rank(&cur.severity)
                    {
                        cur.severity = sev;
                    }
                }
            }
        }
        map
    }

    fn open_review_finding(&mut self, f: &Finding, window: &mut Window, cx: &mut Context<Self>) {
        if f.id.is_empty() {
            return;
        }
        let id = f.id.clone();
        self.findings_panel
            .update(cx, |p, cx| p.expand(vec![id], None, cx));
        self.select_code_panel(CodePanelKey::Findings, window, cx);
    }

    // ---- findings ----

    /// `loadFindings`.
    pub(crate) fn load_findings(&mut self, cx: &mut Context<Self>) -> Task<Vec<Finding>> {
        let id = self.props.workspace_id.clone();
        if id.is_empty() {
            self.findings.clear();
            return Task::ready(Vec::new());
        }
        let svc = self.services.clone();
        cx.spawn(async move |this, cx| {
            let f = call(cx, svc.observatory.list_findings(&id), "Findings failed").await;
            this.update(cx, |o, cx| {
                if let Some(f) = f
                    && o.props.workspace_id == id
                {
                    o.findings = f;
                    cx.notify();
                }
                o.findings.clone()
            })
            .unwrap_or_default()
        })
    }

    /// `addReviewFinding`: a review line note becomes an open finding.
    pub(crate) fn add_review_finding(
        &mut self,
        path: String,
        line: i64,
        body: String,
        end_line: i64,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        let id = self.props.workspace_id.clone();
        let trimmed = body.trim().to_string();
        if id.is_empty() || trimmed.is_empty() || path.trim().is_empty() {
            return Task::ready(false);
        }
        let svc = self.services.clone();
        cx.spawn(async move |this, cx| {
            match svc
                .observatory
                .create_finding(&id, &path, line, &trimmed, end_line)
                .await
            {
                Ok(_) => {
                    let _ = this.update(cx, |o, cx| o.load_findings(cx).detach());
                    true
                }
                Err(err) => {
                    cx.update(|cx| {
                        let m = err.message.clone();
                        toast(
                            ToastKind::Error,
                            if m.is_empty() {
                                "Could not save finding".into()
                            } else {
                                m
                            },
                            cx,
                        )
                    });
                    false
                }
            }
        })
    }

    /// `setFindingStatus`: optimistic, rolled back on failure.
    pub(crate) fn set_finding_status(&mut self, f: &Finding, status: &str, cx: &mut Context<Self>) {
        let id = self.props.workspace_id.clone();
        if id.is_empty() || !self.finding_busy_id.is_empty() {
            return;
        }
        let prev = f.status.clone();
        let fid = f.id.clone();
        self.finding_busy_id = fid.clone();
        for x in &mut self.findings {
            if x.id == fid {
                x.status = status.to_string();
            }
        }
        cx.notify();
        let svc = self.services.clone();
        let status = status.to_string();
        cx.spawn(async move |this, cx| {
            let res = svc.observatory.set_finding_status(&id, &fid, &status).await;
            let _ = this.update(cx, |o, cx| {
                match res {
                    Ok(updated) => {
                        for x in &mut o.findings {
                            if x.id == fid {
                                *x = updated.clone();
                            }
                        }
                    }
                    Err(err) => {
                        for x in &mut o.findings {
                            if x.id == fid {
                                x.status = prev.clone();
                            }
                        }
                        let m = err.message.clone();
                        toast(
                            ToastKind::Error,
                            if m.is_empty() {
                                "Could not update finding".into()
                            } else {
                                m
                            },
                            cx,
                        );
                    }
                }
                o.finding_busy_id.clear();
                cx.notify();
            });
        })
        .detach();
    }

    /// `addFindingNote`: a pending note shows at once.
    pub(crate) fn add_finding_note(&mut self, f: &Finding, body: &str, cx: &mut Context<Self>) {
        let id = self.props.workspace_id.clone();
        let trimmed = body.trim().to_string();
        if id.is_empty() || !self.finding_busy_id.is_empty() || trimmed.is_empty() {
            return;
        }
        let fid = f.id.clone();
        self.finding_busy_id = fid.clone();
        let prev_notes = f.notes.clone();
        for x in &mut self.findings {
            if x.id == fid {
                x.notes.push(recur_domain::FindingNote {
                    id: "pending".into(),
                    author: "human".into(),
                    body: trimmed.clone(),
                    created_at: chrono::Utc::now()
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                });
            }
        }
        cx.notify();
        let svc = self.services.clone();
        cx.spawn(async move |this, cx| {
            let res = svc.observatory.add_finding_note(&id, &fid, &trimmed).await;
            let _ = this.update(cx, |o, cx| {
                o.apply_finding_result(&fid, res, prev_notes, "Could not add note", cx);
            });
        })
        .detach();
    }

    /// `deleteFindingNote`.
    pub(crate) fn delete_finding_note(
        &mut self,
        f: &Finding,
        note_id: &str,
        cx: &mut Context<Self>,
    ) {
        let id = self.props.workspace_id.clone();
        if id.is_empty() || !self.finding_busy_id.is_empty() || note_id.is_empty() {
            return;
        }
        let fid = f.id.clone();
        self.finding_busy_id = fid.clone();
        let prev_notes = f.notes.clone();
        for x in &mut self.findings {
            if x.id == fid {
                x.notes.retain(|n| n.id != note_id);
            }
        }
        cx.notify();
        let svc = self.services.clone();
        let note_id = note_id.to_string();
        cx.spawn(async move |this, cx| {
            let res = svc
                .observatory
                .delete_finding_note(&id, &fid, &note_id)
                .await;
            let _ = this.update(cx, |o, cx| {
                o.apply_finding_result(&fid, res, prev_notes, "Could not delete note", cx);
            });
        })
        .detach();
    }

    fn apply_finding_result(
        &mut self,
        fid: &str,
        res: recur_services::Result<Finding>,
        prev_notes: Vec<recur_domain::FindingNote>,
        fail: &str,
        cx: &mut Context<Self>,
    ) {
        match res {
            Ok(updated) => {
                for x in &mut self.findings {
                    if x.id == fid {
                        *x = updated.clone();
                    }
                }
            }
            Err(err) => {
                for x in &mut self.findings {
                    if x.id == fid {
                        x.notes = prev_notes.clone();
                    }
                }
                let m = err.message.clone();
                toast(
                    ToastKind::Error,
                    if m.is_empty() { fail.to_string() } else { m },
                    cx,
                );
            }
        }
        self.finding_busy_id.clear();
        cx.notify();
    }

    // ---- focus ----

    /// `onObsFocusIn`: map the focused region to the tab-underline zone.
    /// Recompute the zone whenever focus enters or leaves `h` inside the Observatory
    /// (GPUI only reports entering a handle's subtree).
    pub(crate) fn watch_zone(
        &self,
        h: &gpui_kit::FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> [gpui_kit::Subscription; 2] {
        [
            cx.on_focus_in(h, window, |o, window, cx| o.on_obs_focus_in(window, cx)),
            cx.on_focus_out(h, window, |o, _, window, cx| {
                if o.focus.contains_focused(window, cx) {
                    o.on_obs_focus_in(window, cx);
                }
            }),
        ]
    }

    pub(crate) fn on_obs_focus_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Quick Open and the command prompts mount outside `.obs` in the page.
        if self.quick_open.read(cx).contains_focus(window, cx) || self.prompts_reply.is_some() {
            return;
        }
        let zone = if self
            .dock
            .as_ref()
            .is_some_and(|(_, d)| d.read(cx).contains_focus(window, cx))
        {
            FocusZone::Terminal
        } else if self.files_focus.contains_focused(window, cx)
            || self.scm_focus.contains_focused(window, cx)
        {
            FocusZone::Editor
        } else if self.left_focus.contains_focused(window, cx) {
            FocusZone::Agent
        } else if self.main_focus.contains_focused(window, cx)
            || self.tabs_focus.contains_focused(window, cx)
        {
            FocusZone::Editor
        } else {
            FocusZone::Other
        };
        if zone != self.focus_zone {
            self.focus_zone = zone;
            self.sync_side_effects(cx);
            // Focus listeners run inside the draw, where a notify does not schedule a frame.
            cx.defer_in(window, |_, _, cx| cx.notify());
        }
    }

    pub(crate) fn clear_term_max(&mut self, cx: &mut Context<Self>) {
        let id = self.props.workspace_id.clone();
        if !id.is_empty() {
            self.state
                .update(cx, |s, cx| s.set_terminal_dock_maximized(&id, false, cx));
        }
    }

    /// `focusAgentComposer`: keys into the visible agent composer (the host's chat).
    pub(crate) fn focus_agent_composer(&mut self, token: u64, cx: &mut Context<Self>) {
        if self.root_token != token {
            return;
        }
        cx.emit(ObservatoryEvent::FocusAgentComposer);
    }

    /// `showAgentPanel`: bring the transcript on screen without taking focus.
    pub fn show_agent_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_term_max(cx);
        if self.portal_agent_to_pr(cx) {
            self.set_pr_agent_dock_open(true, cx);
            return;
        }
        self.select_code_panel(CodePanelKey::Maestro, window, cx);
    }

    /// ⌘1 `focusAgentPanel`.
    pub(crate) fn focus_agent_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let token = self.root_token;
        if self.portal_agent_to_pr(cx) {
            self.clear_term_max(cx);
            self.set_pr_agent_dock_open(true, cx);
        } else if !self.has_left(cx)
            || !self
                .panels_on(PanelSide::Left, cx)
                .contains(&CodePanelKey::Maestro)
        {
            return Task::ready(());
        } else {
            self.clear_term_max(cx);
            self.left_kind = CodePanelKey::Maestro;
            self.set_left_gutter_open(true, cx);
        }
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(0))
                .await;
            let _ = this.update(cx, |o, cx| o.focus_agent_composer(token, cx));
        })
    }

    /// ⌘2 `focusCodePanel`.
    pub(crate) fn focus_code_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let token = self.root_token;
        self.clear_term_max(cx);
        let pane = if self.split_open && self.focused_pane == Pane::Split {
            Pane::Split
        } else {
            Pane::Main
        };
        self.focus_pane(pane, window, cx);
        cx.defer_in(window, move |o, window, cx| {
            if o.root_token != token {
                return;
            }
            let in_main = o.main_focus.contains_focused(window, cx)
                && !o
                    .dock
                    .as_ref()
                    .is_some_and(|(_, d)| d.read(cx).contains_focus(window, cx));
            if in_main {
                return;
            }
            o.focus_zone = FocusZone::Editor;
            window.focus(&o.main_focus, cx);
            cx.notify();
        });
    }

    /// ⌘3 `focusTerminalPanel`: open the dock and put keys in its shell (retrying while
    /// the first pane spawns).
    pub(crate) fn focus_terminal_panel(
        &mut self,
        token: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.props.workspace_id.clone();
        if id.is_empty() {
            return;
        }
        self.state
            .update(cx, |s, cx| s.set_terminal_dock_open(&id, true, cx));
        cx.spawn_in(window, async move |this, cx| {
            for i in 0..9 {
                if i > 0 {
                    cx.background_executor()
                        .timer(Duration::from_millis(40))
                        .await;
                }
                let done = this
                    .update_in(cx, |o, window, cx| {
                        if o.root_token != token || o.props.workspace_id != id {
                            return true;
                        }
                        o.ensure_dock(window, cx);
                        if let Some((_, d)) = o.dock.clone() {
                            d.update(cx, |d, cx| d.focus_active(window, cx));
                            return d.read(cx).contains_focus(window, cx);
                        }
                        false
                    })
                    .unwrap_or(true);
                if done {
                    return;
                }
            }
        })
        .detach();
    }

    /// `restoreChromeFocus`: keys back where this workspace last had them (A→B→A).
    pub(crate) fn restore_chrome_focus(
        &mut self,
        zone: FocusZone,
        token: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.props.main_blocked || self.root_token != token {
            return;
        }
        // Behind another route the shell is not laid out: focus there would leave keys no
        // path to the shell. Only the agent dock
        // shows, portaled into PR detail.
        let on_screen = self.state.read(cx).view == recur_view_core::ViewName::Detail
            || (zone == FocusZone::Agent && self.portal_agent_to_pr(cx));
        if !on_screen {
            return;
        }
        let code_or_terminal =
            |o: &mut Observatory, window: &mut Window, cx: &mut Context<Observatory>| {
                if o.editor_hidden(cx) && o.term_open(cx) && !o.props.workspace_id.is_empty() {
                    o.focus_terminal_panel(token, window, cx);
                    return;
                }
                o.focus_code_panel(window, cx);
            };
        match zone {
            FocusZone::Terminal => {
                if self.term_open(cx) && !self.props.workspace_id.is_empty() {
                    self.focus_terminal_panel(token, window, cx);
                    return;
                }
                code_or_terminal(self, window, cx);
            }
            FocusZone::Agent => {
                if self.maestro_panel_visible(cx) {
                    self.focus_agent_composer(token, cx);
                    return;
                }
                code_or_terminal(self, window, cx);
            }
            _ => {
                if self.editor_hidden(cx)
                    || self.view_mode != recur_editor::view_state::EditorViewMode::Code
                {
                    code_or_terminal(self, window, cx);
                    return;
                }
                self.sync_editors(window, cx);
                let target = if self.focused_pane == Pane::Split && self.split_open {
                    self.split_editor.clone()
                } else {
                    self.main_editor.clone()
                };
                let Some(t) = target else {
                    code_or_terminal(self, window, cx);
                    return;
                };
                t.update(cx, |e, cx| e.focus(window, cx));
                if t.read(cx).has_focus(window) {
                    self.focus_zone = FocusZone::Editor;
                }
            }
        }
    }

    // ---- the dock's chrome ----

    /// The dock opened on this workspace (toolbar, ⌃`): put keys in the shell. The terminal
    /// maximize phase follows the store.
    pub(crate) fn dock_state_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.props.workspace_id.clone();
        let open = self.term_open(cx);
        if id != self.term_open_for_ws {
            self.term_open_for_ws = id.clone();
            self.term_was_open = open;
        } else {
            if open && !self.term_was_open && !self.props.main_blocked {
                let token = self.root_token;
                self.focus_terminal_panel(token, window, cx);
            }
            self.term_was_open = open;
        }
        let max = self.term_maximized(cx);
        if id != self.term_max_for_ws {
            self.term_max_for_ws = id;
            self.term_max_phase = TermMaxPhase::Idle;
            self.term_max_anim = None;
        }
        if open && max {
            if matches!(self.term_max_phase, TermMaxPhase::Enter | TermMaxPhase::Max) {
                return;
            }
            self.animate_term_fill(window, cx);
            return;
        }
        if !open {
            self.term_max_phase = TermMaxPhase::Idle;
            self.term_max_anim = None;
            return;
        }
        if matches!(self.term_max_phase, TermMaxPhase::Idle | TermMaxPhase::Exit) {
            return;
        }
        self.animate_term_shrink(cx);
    }

    fn rect_of(
        b: gpui_kit::Bounds<gpui_kit::Pixels>,
        obs: gpui_kit::Bounds<gpui_kit::Pixels>,
    ) -> OverlayRect {
        OverlayRect {
            top: f32::from(b.origin.y - obs.origin.y),
            left: f32::from(b.origin.x - obs.origin.x),
            width: f32::from(b.size.width),
            height: f32::from(b.size.height),
        }
    }

    /// The whole Observatory, in its own coordinates.
    pub(crate) fn full_rect(&self) -> OverlayRect {
        let obs = self.bounds.obs.get();
        OverlayRect {
            top: 0.,
            left: 0.,
            width: f32::from(obs.size.width),
            height: f32::from(obs.size.height),
        }
    }

    /// `restoreTargetRect`: back to `.obs-main` when the editor was hidden, else the slot.
    pub(crate) fn restore_target_rect(&self) -> OverlayRect {
        let obs = self.bounds.obs.get();
        if self.overlay_keep_editor_hidden {
            return Self::rect_of(self.bounds.main.get(), obs);
        }
        let slot = self.bounds.slot.get();
        if slot.size.width > gpui_kit::px(0.) {
            Self::rect_of(slot, obs)
        } else {
            self.full_rect()
        }
    }

    /// `animateTermFill`: FLIP the dock from its slot to the whole Observatory.
    fn animate_term_fill(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let reversing = self.term_max_phase == TermMaxPhase::Exit;
        if !reversing {
            self.overlay_keep_editor_hidden = self.editor_hidden(cx);
        }
        let obs = self.bounds.obs.get();
        let dock = self.bounds.dock.get();
        if dock.size.width <= gpui_kit::px(0.) || obs.size.width <= gpui_kit::px(0.) {
            self.term_max_phase = TermMaxPhase::Max;
            self.overlay_rect = self.full_rect();
            cx.notify();
            return;
        }
        let from = if reversing {
            self.overlay_rect
        } else {
            Self::rect_of(dock, obs)
        };
        if !reversing {
            self.term_slot_h = from.height;
        }
        self.overlay_rect = from;
        self.term_max_phase = TermMaxPhase::Enter;
        let root = self.root_token;
        self.focus_terminal_panel(root, window, cx);
        self.term_max_anim = Some((from, recur_ui::motion::now(cx)));
        cx.notify();
    }

    /// `animateTermShrink`: back to the slot (or `.obs-main` when the editor was hidden).
    fn animate_term_shrink(&mut self, cx: &mut Context<Self>) {
        let obs = self.bounds.obs.get();
        let dock = self.bounds.dock.get();
        if dock.size.width <= gpui_kit::px(0.) || obs.size.width <= gpui_kit::px(0.) {
            self.term_max_phase = TermMaxPhase::Idle;
            self.term_max_anim = None;
            cx.notify();
            return;
        }
        let from = self.overlay_rect;
        self.term_max_phase = TermMaxPhase::Exit;
        self.term_max_anim = Some((from, recur_ui::motion::now(cx)));
        cx.notify();
    }

    /// Step the overlay rectangle on display frames over `TERM_MAX_MS`, then settle the phase.
    /// The terminals keep their grid while the box moves and refit once when it lands: a
    /// resize reflows the whole scrollback.
    pub(crate) fn step_term_anim(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((from, start)) = self.term_max_anim {
            let max = self.term_max_phase == TermMaxPhase::Enter;
            // The destination tracks layout while it animates (resize).
            let to = if max {
                self.full_rect()
            } else {
                self.restore_target_rect()
            };
            let t = recur_ui::motion::progress(start, Duration::from_millis(TERM_MAX_MS), cx);
            let e = recur_ui::motion::Easing::Bezier(0.22, 1., 0.36, 1.).apply(t);
            let lerp = |a: f32, b: f32| a + (b - a) * e;
            self.overlay_rect = OverlayRect {
                top: lerp(from.top, to.top),
                left: lerp(from.left, to.left),
                width: lerp(from.width, to.width),
                height: lerp(from.height, to.height),
            };
            if t >= 1. {
                self.term_max_anim = None;
                self.term_max_phase = if max {
                    TermMaxPhase::Max
                } else {
                    TermMaxPhase::Idle
                };
            } else {
                window.request_animation_frame();
            }
        }
        let held = self.term_max_anim.is_some();
        if let Some((_, d)) = &self.dock {
            let panes: Vec<_> = d.read(cx).panes.values().cloned().collect();
            for p in panes {
                p.update(cx, |v, cx| v.hold_size(held, cx));
            }
        }
    }

    // ---- full screen ----

    /// `setEditorFullscreen`: the editor pane fills a native full screen window.
    pub(crate) fn set_editor_fullscreen(
        &mut self,
        on: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if on == self.editor_fullscreen {
            return;
        }
        self.editor_fullscreen = on;
        if on {
            self.reveal_fs_exit(cx);
            self.window_was_fullscreen = window.is_fullscreen();
            if !self.window_was_fullscreen {
                window.toggle_fullscreen();
            }
            window.focus(&self.main_focus, cx);
        } else if !self.window_was_fullscreen && window.is_fullscreen() {
            window.toggle_fullscreen();
        }
        cx.notify();
    }

    /// The exit button shows for 1.5s after entering or on pointer movement.
    pub(crate) fn reveal_fs_exit(&mut self, cx: &mut Context<Self>) {
        self.fs_exit_visible = true;
        self.fs_exit_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1500))
                .await;
            let _ = this.update(cx, |o, cx| {
                o.fs_exit_visible = false;
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// The window left full screen on its own (green button, ⌃⌘F).
    pub(crate) fn window_fullscreen_changed(&mut self, window: &Window, cx: &mut Context<Self>) {
        let fs = window.is_fullscreen();
        if self.last_window_fullscreen && !fs && self.editor_fullscreen {
            self.editor_fullscreen = false;
            cx.notify();
        }
        self.last_window_fullscreen = fs;
    }
}

/// The comparable part of [`SurfaceProps`] (its matchers and sets are rebuilt from these).
#[derive(Clone, PartialEq)]
pub(crate) struct ReviewSig {
    project_id: String,
    workspace_id: String,
    base_label: String,
    refresh: u64,
    active: bool,
    tab: ReviewTab,
    count: usize,
    excludes: Vec<String>,
    findings: recur_view_review::findings::FindingsByPath,
    dirty: Vec<String>,
    restore: Option<(String, f32)>,
}
