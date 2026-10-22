//! `ReviewSurface.svelte`: Review changes in the workspace Observatory. Changes (Files changed
//! vs the base) and Commits (uncommitted work and the branch's commits), with a commit or the
//! working tree opened as a Files changed scope and stepped through newest-first.

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, InteractiveElement, IntoElement, ParentElement,
    Render, Styled, Subscription, Task, Window, div,
};
use recur_code::glob::CompiledGlob;
use recur_domain::{Commit, FileDiff, Finding};
use recur_ui::Theme;
use recur_view_core::AppState;

use crate::commit_meta::commit_sha;
use crate::commits::{CommitsEvent, CommitsView};
use crate::diff_file::{AddFindingFn, NoteMode};
use crate::files_changed::{FilesChangedView, FilesEvent, FilesProps, FilesScope};
use crate::findings::FindingsByPath;
use crate::review_diff::ReviewTab;
use crate::review_scope::{
    TapeItem, can_step_on_tape, review_tape, review_tape_index, step_tape, tape_step_label,
};
use crate::style::hsla;

/// `ReviewSurface`'s props.
#[derive(Clone, Default)]
pub struct SurfaceProps {
    pub project_id: String,
    pub workspace_id: String,
    pub base_label: String,
    pub refresh: u64,
    pub active: bool,
    pub tab: ReviewTab,
    pub count: usize,
    pub exclude_matchers: Option<Rc<Vec<CompiledGlob>>>,
    pub findings_by_path: Option<Rc<FindingsByPath>>,
    pub dirty_paths: Option<Rc<std::collections::HashSet<String>>>,
    pub working_dirty_count: usize,
    /// `restore`: (selected path, scrollTop).
    pub restore: Option<(String, f32)>,
}

/// The `bind:` values the Observatory reads back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SurfaceBound {
    pub commit_count: usize,
    pub commit_scope_hash: String,
    pub working_open: bool,
    pub add: i64,
    pub del: i64,
    pub file_count: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SurfaceEvent {
    /// `onTabChange`.
    TabChange(ReviewTab),
    /// `onOpenFile(path, line?)`.
    OpenFile(String, Option<i64>),
    /// `onOpenFinding`.
    OpenFinding(Box<Finding>),
    /// `onShowFindings`.
    ShowFindings,
    /// A bound value changed.
    Bound(SurfaceBound),
}

/// `ReviewViewState`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReviewViewState {
    pub tab: ReviewTab,
    pub selected_path: String,
    pub scroll_top: f32,
}

impl EventEmitter<SurfaceEvent> for ReviewSurface {}

pub struct ReviewSurface {
    state: Entity<AppState>,
    props: SurfaceProps,
    commit_scope: Option<Commit>,
    commit_files_by_hash: HashMap<String, Vec<FileDiff>>,
    commits: Vec<Commit>,
    commits_loading: bool,
    commits_error: bool,
    working_open: bool,
    scope_for_ws: String,
    bound: SurfaceBound,
    files: Entity<FilesChangedView>,
    commits_view: Entity<CommitsView>,
    load_gen: u64,
    load_task: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl ReviewSurface {
    pub fn new(
        state: Entity<AppState>,
        props: SurfaceProps,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let files =
            cx.new(|cx| FilesChangedView::new(state.clone(), FilesProps::default(), window, cx));
        let commits_view = cx.new(|_| CommitsView::new());
        let subs = vec![
            cx.subscribe_in(
                &files,
                window,
                |s: &mut Self, _, ev: &FilesEvent, window, cx| s.on_files(ev, window, cx),
            ),
            cx.subscribe_in(
                &commits_view,
                window,
                |s: &mut Self, _, ev: &CommitsEvent, window, cx| match ev {
                    CommitsEvent::Open(c) => s.open_commit((**c).clone(), window, cx),
                    CommitsEvent::OpenWorking => s.open_working(window, cx),
                },
            ),
        ];
        let mut s = ReviewSurface {
            state,
            props: SurfaceProps::default(),
            commit_scope: None,
            commit_files_by_hash: HashMap::new(),
            commits: Vec::new(),
            commits_loading: false,
            commits_error: false,
            working_open: false,
            scope_for_ws: String::new(),
            bound: SurfaceBound::default(),
            files,
            commits_view,
            load_gen: 0,
            load_task: None,
            _subs: subs,
        };
        s.set_props(props, window, cx);
        s
    }

    /// `onAddFinding`.
    pub fn set_add_finding(
        &mut self,
        f: Option<AddFindingFn>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.files.update(cx, |v, _| v.set_add_finding(f));
        self.push_files(window, cx);
    }

    pub fn files_view(&self) -> &Entity<FilesChangedView> {
        &self.files
    }

    pub fn commits_view(&self) -> &Entity<CommitsView> {
        &self.commits_view
    }

    pub fn props(&self) -> &SurfaceProps {
        &self.props
    }

    pub fn bound(&self) -> &SurfaceBound {
        &self.bound
    }

    pub fn commit_scope(&self) -> Option<&Commit> {
        self.commit_scope.as_ref()
    }

    /// A line-note composer is open in the Changes stack (live refresh waits for it).
    pub fn composer_open(&self, cx: &gpui_kit::App) -> bool {
        let files = self.files.read(cx);
        let stack = files.stack();
        stack
            .files()
            .iter()
            .filter_map(|f| stack.diff_file(&f.path))
            .any(|d| d.read(cx).composer_open())
    }

    pub fn working_open(&self) -> bool {
        self.working_open
    }

    pub fn set_props(&mut self, props: SurfaceProps, window: &mut Window, cx: &mut Context<Self>) {
        let prev = std::mem::replace(&mut self.props, props);
        // workspaceId is re-passed (same string) when the workspaces store refreshes; only a
        // new workspace resets the drill-down.
        if self.props.workspace_id != self.scope_for_ws {
            self.scope_for_ws = self.props.workspace_id.clone();
            self.commit_scope = None;
            self.commit_files_by_hash.clear();
            self.working_open = false;
            self.commits.clear();
            self.bound.commit_count = 0;
            self.bound.commit_scope_hash.clear();
            self.bound.file_count = 0;
        }
        if self.working_open && self.props.working_dirty_count == 0 {
            self.clear_scope_inner();
        }
        let p = &self.props;
        if p.active
            && (!prev.active
                || prev.project_id != p.project_id
                || prev.workspace_id != p.workspace_id
                || prev.refresh != p.refresh)
        {
            self.load_commits(cx);
        }
        self.push_files(window, cx);
        self.push_commits(cx);
        self.emit_bound(cx);
        cx.notify();
    }

    fn showing_commit(&self) -> bool {
        self.commit_scope.is_some()
    }

    fn files_open(&self) -> bool {
        self.props.tab == ReviewTab::Changes || self.showing_commit() || self.working_open
    }

    fn commits_list_open(&self) -> bool {
        self.props.tab == ReviewTab::Commits && !self.showing_commit() && !self.working_open
    }

    fn tape(&self) -> Vec<TapeItem> {
        review_tape(self.props.working_dirty_count > 0, &self.commits)
    }

    fn scope_idx(&self, tape: &[TapeItem]) -> isize {
        let sha = self
            .commit_scope
            .as_ref()
            .map(commit_sha)
            .unwrap_or_default();
        review_tape_index(tape, self.working_open, &sha)
    }

    fn push_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tape = self.tape();
        let idx = self.scope_idx(&tape);
        let (older, newer) = can_step_on_tape(idx, tape.len());
        let scope = if self.working_open {
            FilesScope::Working
        } else if let Some(c) = &self.commit_scope {
            FilesScope::Commit(Box::new(c.clone()))
        } else {
            FilesScope::Base
        };
        let p = &self.props;
        let props = FilesProps {
            project_id: p.project_id.clone(),
            workspace_id: p.workspace_id.clone(),
            base_label: p.base_label.clone(),
            refresh: p.refresh,
            active: p.active && self.files_open(),
            scope,
            count: Some(p.count),
            exclude_matchers: p.exclude_matchers.clone(),
            note_mode: NoteMode::Finding,
            findings_by_path: p.findings_by_path.clone(),
            dirty_paths: p.dirty_paths.clone(),
            can_step_older: older,
            can_step_newer: newer,
            step_older_label: tape_step_label(step_tape(&tape, idx, -1), -1).into(),
            step_newer_label: tape_step_label(step_tape(&tape, idx, 1), 1).into(),
            restore: p.restore.clone(),
        };
        self.files
            .update(cx, |v, cx| v.set_props(props, window, cx));
    }

    fn push_commits(&mut self, cx: &mut Context<Self>) {
        let (commits, loading, error) = (
            self.commits.clone(),
            self.commits_loading,
            self.commits_error,
        );
        let (base, dirty) = (
            self.props.base_label.clone(),
            self.props.working_dirty_count,
        );
        self.commits_view
            .update(cx, |v, cx| v.set(commits, loading, error, base, dirty, cx));
    }

    fn emit_bound(&mut self, cx: &mut Context<Self>) {
        let next = SurfaceBound {
            commit_count: self.bound.commit_count,
            commit_scope_hash: self.bound.commit_scope_hash.clone(),
            working_open: self.working_open,
            ..self.bound.clone()
        };
        self.bound = next.clone();
        cx.emit(SurfaceEvent::Bound(next));
    }

    fn on_files(&mut self, ev: &FilesEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            FilesEvent::ClearScope => self.clear_commit_scope(window, cx),
            FilesEvent::CommitFiles(c, files) => {
                self.cache_commit_files(c, files.clone(), window, cx)
            }
            FilesEvent::OpenFile(p, l) => cx.emit(SurfaceEvent::OpenFile(p.clone(), *l)),
            FilesEvent::OpenFinding(f) => cx.emit(SurfaceEvent::OpenFinding(Box::new(f.clone()))),
            FilesEvent::ShowFindings => cx.emit(SurfaceEvent::ShowFindings),
            FilesEvent::StepScope(dir) => self.step_scope(*dir, window, cx),
            FilesEvent::Totals { add, del, files } => {
                self.bound.add = *add;
                self.bound.del = *del;
                self.bound.file_count = *files;
                self.emit_bound(cx);
            }
        }
    }

    /// `openCommitScope`: show a caller-supplied diff (a chat turn's checkpoint) as a commit
    /// scope; its `files` must be filled since its hash is not on the branch.
    pub fn open_commit(&mut self, c: Commit, window: &mut Window, cx: &mut Context<Self>) {
        let hash = commit_sha(&c);
        let cached = (!hash.is_empty())
            .then(|| self.commit_files_by_hash.get(&hash))
            .flatten();
        self.working_open = false;
        self.commit_scope = Some(match cached {
            Some(files) if !files.is_empty() => Commit {
                files: Some(files.clone()),
                ..c
            },
            _ => c,
        });
        self.bound.commit_scope_hash = hash;
        cx.emit(SurfaceEvent::TabChange(ReviewTab::Commits));
        self.push_files(window, cx);
        self.emit_bound(cx);
        cx.notify();
    }

    /// `openWorkingScope`.
    pub fn open_working(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_scope = None;
        self.bound.commit_scope_hash.clear();
        self.working_open = true;
        cx.emit(SurfaceEvent::TabChange(ReviewTab::Commits));
        self.push_files(window, cx);
        self.emit_bound(cx);
        cx.notify();
    }

    fn clear_scope_inner(&mut self) {
        self.commit_scope = None;
        self.bound.commit_scope_hash.clear();
        self.working_open = false;
    }

    /// `clearCommitScope`.
    pub fn clear_commit_scope(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_scope_inner();
        self.push_files(window, cx);
        self.emit_bound(cx);
        cx.notify();
    }

    fn cache_commit_files(
        &mut self,
        c: &Commit,
        files: Vec<FileDiff>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let hash = commit_sha(c);
        if hash.is_empty() {
            return;
        }
        self.commit_files_by_hash
            .insert(hash.clone(), files.clone());
        if let Some(scope) = &mut self.commit_scope
            && commit_sha(scope) == hash
        {
            scope.files = Some(files);
            self.push_files(window, cx);
        }
    }

    fn step_scope(&mut self, dir: i8, window: &mut Window, cx: &mut Context<Self>) {
        let tape = self.tape();
        let idx = self.scope_idx(&tape);
        match step_tape(&tape, idx, dir).cloned() {
            Some(TapeItem::Working) => self.open_working(window, cx),
            Some(TapeItem::Commit(c)) => self.open_commit(*c, window, cx),
            None => {}
        }
    }

    fn load_commits(&mut self, cx: &mut Context<Self>) {
        if !self.props.active {
            return;
        }
        self.load_gen += 1;
        let generation = self.load_gen;
        self.commits_loading = true;
        self.commits_error = false;
        let services = self.state.read(cx).services().clone();
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let got = services.code.list_commits(&pid, &wid).await;
            let _ = this.update(cx, |s, cx| {
                if generation != s.load_gen {
                    return;
                }
                match got {
                    Ok(list) => {
                        s.commits = list;
                        s.bound.commit_count = s.commits.len();
                    }
                    Err(_) => {
                        s.commits_error = true;
                        s.bound.commit_count = s.commits.len();
                    }
                }
                s.commits_loading = false;
                s.push_commits(cx);
                s.emit_bound(cx);
                cx.notify();
            });
        }));
        self.push_commits(cx);
    }

    /// `getViewState`.
    pub fn view_state(&self, cx: &gpui_kit::App) -> ReviewViewState {
        let (selected_path, scroll_top) = self.files.read(cx).view_state();
        ReviewViewState {
            tab: self.props.tab,
            selected_path,
            scroll_top,
        }
    }
}

impl Render for ReviewSurface {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx).tokens;
        let mut root = div()
            .debug_selector(|| "obs-review-surface".into())
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(hsla(t.bg));
        if self.files_open() {
            root = root.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(self.files.clone()),
            );
        }
        if self.commits_list_open() {
            root = root.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(self.commits_view.clone()),
            );
        }
        root
    }
}
