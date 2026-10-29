//! `FilesChangedView.svelte`: Review → Changes. The toolbar (file list toggle and mode, file
//! count against the base, findings link, totals, unified / split, fold all), the file list
//! panel with its filter and findings-only toggle, and the stacked diffs for the branch vs its
//! base, the working tree vs HEAD, or one commit, loaded with its queue and scope rules.

use recur_ui::guard::Guarded;
use recur_view_core::{ObserveState as _, Topics};
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, Focusable, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Render, SharedString, StatefulInteractiveElement,
    Styled, Subscription, Task, Window, canvas, div, px,
};
use recur_code::glob::{CompiledGlob, is_excluded_by};
use recur_code::view::changed_tree::sort_files_by_changed_tree;
use recur_code::view::finding_paths::findings_for_path;
use recur_domain::{Commit, FileDiff};
use recur_engine::view::js;
use recur_ui::fonts::Family;
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::tooltip::titled;
use recur_ui::widgets::{Axis, Edge, resize_handle};
use recur_ui::{IconName, Theme, icon};
use recur_view_core::AppState;
use recur_view_core::prefs::PanelKey;

use crate::commit_meta::commit_sha;
use crate::diff_file::{AddFindingFn, DiffFileHooks, NoteMode};
use crate::file_list::{FileListProps, ReviewFileList};
use crate::findings::FindingsByPath;
use crate::review_diff::{
    DiffLayout, FileListMode, read_diff_layout, read_file_list_mode, read_review_sidebar,
    reuse_file_diffs, write_diff_layout, write_file_list_mode, write_review_sidebar,
};
use crate::stack::{CardProps, FileStack, ImageRevs};
use crate::style::{hsla, icon_btn, seg, seg_btn};

/// Side-by-side needs this much pane width; narrower panes fall back to unified.
pub const SPLIT_MIN_PX: f32 = 720.;
/// The review surface stacks the file list over the diffs below this width
/// (`@container obs-review (max-width: 520px)`).
pub const STACK_BELOW_PX: f32 = 520.;

/// Where Review → Changes reads files from.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum FilesScope {
    /// The branch vs its base (`DiffVsBase`).
    #[default]
    Base,
    /// The working tree vs HEAD (`DiffWorking`).
    Working,
    /// One commit (`CommitDiff`, or the files it carries).
    Commit(Box<Commit>),
}

/// `FilesChangedView`'s props.
#[derive(Clone, Default)]
pub struct FilesProps {
    pub project_id: String,
    pub workspace_id: String,
    pub base_label: String,
    pub refresh: u64,
    pub active: bool,
    pub scope: FilesScope,
    /// The host's file count for the Changes header (`count`).
    pub count: Option<usize>,
    pub exclude_matchers: Option<Rc<Vec<CompiledGlob>>>,
    pub note_mode: NoteMode,
    pub findings_by_path: Option<Rc<FindingsByPath>>,
    pub dirty_paths: Option<Rc<HashSet<String>>>,
    pub can_step_older: bool,
    pub can_step_newer: bool,
    pub step_older_label: String,
    pub step_newer_label: String,
    /// The view state to restore once (`restore`).
    pub restore: Option<(String, f32)>,
}

/// What the view reports.
#[derive(Clone, Debug, PartialEq)]
pub enum FilesEvent {
    /// `onClearScope` (← all commits).
    ClearScope,
    /// `onCommitFiles`: a commit's diff loaded.
    CommitFiles(Commit, Vec<FileDiff>),
    /// `onOpenFile(path, line?)`.
    OpenFile(String, Option<i64>),
    /// `onOpenFinding`.
    OpenFinding(recur_domain::Finding),
    /// `onShowFindings`.
    ShowFindings,
    /// `onStepScope(dir)`: -1 older, +1 newer.
    StepScope(i8),
    /// `bind:diffTotals` / `bind:fileCount` changed.
    Totals { add: i64, del: i64, files: usize },
}

impl EventEmitter<FilesEvent> for FilesChangedView {}

/// `(files_gen, exclude matchers, scoped)` and, for the matched list, the filter query, the
/// findings-only toggle and the findings map.
type VisibleKey = (u64, usize, bool);
type MatchedKey = (VisibleKey, String, bool, usize);

#[derive(Default)]
struct MatchedMemo {
    visible: Option<(VisibleKey, Vec<Rc<FileDiff>>)>,
    matched: Option<(MatchedKey, Vec<Rc<FileDiff>>)>,
}

pub struct FilesChangedView {
    state: Entity<AppState>,
    /// The file list width the view last drew with.
    seen_list_w: Option<f32>,
    /// Mount the open line notes' text boxes here (see `note_portal`); off when a host above a
    /// cached view of this one mounts them.
    note_portals_here: bool,
    props: FilesProps,
    files: Vec<Rc<FileDiff>>,
    /// Bumped whenever `files` changes (keys `matched_memo`).
    files_gen: u64,
    /// The last `visible_files` / `matched_files` and the inputs they were computed from: the
    /// toolbar, the list and the stack read them several times a render, and a large diff
    /// has thousands of paths to glob, filter and sort.
    matched_memo: std::cell::RefCell<MatchedMemo>,
    files_truncated: bool,
    loading: bool,
    error: bool,
    diff_layout: DiffLayout,
    file_list_mode: FileListMode,
    sidebar_open: bool,
    filter: Entity<InputState>,
    findings_only: bool,
    pane_width: f32,
    surface_width: f32,
    pub(crate) stack: FileStack,
    list: Entity<ReviewFileList>,
    /// The card stack as its own cached view (see [`StackPane`]), built on first render.
    stack_pane: Option<Entity<StackPane>>,
    add_finding: Option<AddFindingFn>,
    restored_once: bool,
    restore_root: String,
    load_gen: u64,
    load_running: bool,
    load_queued: bool,
    files_scope: String,
    load_scope: String,
    load_task: Option<Task<()>>,
    last_totals: Option<(i64, i64, usize)>,
    focus_pending: bool,
    _subs: Vec<Subscription>,
}

impl FilesChangedView {
    pub fn new(
        state: Entity<AppState>,
        props: FilesProps,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let storage = state.read(cx).storage();
        let diff_layout = read_diff_layout(storage);
        let file_list_mode = read_file_list_mode(storage);
        let sidebar_open = read_review_sidebar(storage);
        let filter = recur_ui::style::text_input(&SharedString::from("Filter files"), window, cx);
        let me = cx.entity().downgrade();
        let list = cx.new(|_| {
            ReviewFileList::new(
                FileListProps::default(),
                Some(Rc::new(move |path, _, _, cx| {
                    let me = me.clone();
                    cx.defer(move |cx| {
                        let _ = me.update(cx, |v, cx| v.select_path(&path, cx));
                    });
                })),
            )
        });
        let subs = vec![
            cx.guarded_subscribe(&filter, |v: &mut Self, _, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change) {
                    v.sync_stack(cx);
                    cx.notify();
                }
            }),
            // The file list's width is all the view reads of the store.
            cx.observe_state(&state, Topics::PANELS, |v: &mut Self, state, cx| {
                let w = state.read(cx).panels.size(PanelKey::PrFileList);
                if v.seen_list_w != Some(w) {
                    v.seen_list_w = Some(w);
                    cx.notify();
                }
            }),
        ];
        let stack = FileStack::new(state.clone(), cx);
        let mut v = FilesChangedView {
            state,
            seen_list_w: None,
            props: FilesProps::default(),
            files: Vec::new(),
            files_gen: 0,
            matched_memo: Default::default(),
            files_truncated: false,
            loading: false,
            error: false,
            diff_layout,
            file_list_mode,
            sidebar_open,
            filter,
            findings_only: false,
            pane_width: 0.,
            surface_width: 0.,
            stack,
            list,
            stack_pane: None,
            note_portals_here: true,
            add_finding: None,
            restored_once: false,
            restore_root: String::new(),
            load_gen: 0,
            load_running: false,
            load_queued: false,
            files_scope: String::new(),
            load_scope: String::new(),
            load_task: None,
            last_totals: None,
            focus_pending: false,
            _subs: subs,
        };
        v.set_props(props, window, cx);
        v
    }

    /// `onAddFinding`.
    pub fn set_add_finding(&mut self, f: Option<AddFindingFn>) {
        self.add_finding = f;
    }

    pub fn props(&self) -> &FilesProps {
        &self.props
    }

    pub fn files(&self) -> &[Rc<FileDiff>] {
        &self.files
    }

    /// Leave the open line notes' text boxes to a host above (`Pane::portals`).
    pub fn mount_note_portals_above(&mut self) {
        self.note_portals_here = false;
    }

    pub fn stack(&self) -> &FileStack {
        &self.stack
    }

    pub fn file_list(&self) -> &Entity<ReviewFileList> {
        &self.list
    }

    pub fn filter_input(&self) -> &Entity<InputState> {
        &self.filter
    }

    pub fn sidebar_open(&self) -> bool {
        self.sidebar_open
    }

    pub fn diff_layout(&self) -> DiffLayout {
        self.diff_layout
    }

    pub fn file_list_mode(&self) -> FileListMode {
        self.file_list_mode
    }

    pub fn findings_only(&self) -> bool {
        self.findings_only
    }

    pub fn loading(&self) -> bool {
        self.loading
    }

    pub fn set_props(&mut self, props: FilesProps, window: &mut Window, cx: &mut Context<Self>) {
        let prev = std::mem::replace(&mut self.props, props);
        let p = self.props.clone();
        let scope_key = |s: &FilesScope| match s {
            FilesScope::Commit(c) => {
                if c.hash.is_empty() {
                    c.sha.clone()
                } else {
                    c.hash.clone()
                }
            }
            _ => String::new(),
        };
        let load = p.active
            && (!prev.active
                || prev.project_id != p.project_id
                || prev.workspace_id != p.workspace_id
                || scope_key(&prev.scope) != scope_key(&p.scope)
                || (prev.scope == FilesScope::Working) != (p.scope == FilesScope::Working)
                || (scope_key(&p.scope).is_empty() && prev.refresh != p.refresh));
        if p.scope != FilesScope::Working && prev.scope == FilesScope::Working {
            self.drop_working_snapshot();
        }
        if p.active && !prev.active {
            self.focus_pending = true;
        }
        self.stack.set_active(p.active);
        if load {
            self.load(cx);
        }
        self.sync_stack(cx);
        let _ = window;
        cx.notify();
    }

    fn vs_label(&self) -> String {
        let b = js::trim(&self.props.base_label);
        if b.is_empty() {
            "HEAD".into()
        } else {
            b.to_string()
        }
    }

    fn scoped(&self) -> bool {
        self.props.scope != FilesScope::Base
    }

    /// `visibleFiles`: explorer excludes apply only to the branch-vs-base list.
    pub fn visible_files(&self) -> Vec<Rc<FileDiff>> {
        let key = self.visible_key();
        if let Some((k, v)) = &self.matched_memo.borrow().visible
            && *k == key
        {
            return v.clone();
        }
        let v: Vec<Rc<FileDiff>> = match (&self.props.exclude_matchers, self.scoped()) {
            (Some(m), false) if !m.is_empty() => self
                .files
                .iter()
                .filter(|f| !is_excluded_by(&f.path, m))
                .cloned()
                .collect(),
            _ => self.files.clone(),
        };
        self.matched_memo.borrow_mut().visible = Some((key, v.clone()));
        v
    }

    fn visible_key(&self) -> VisibleKey {
        (
            self.files_gen,
            self.props
                .exclude_matchers
                .as_ref()
                .map_or(0, |m| Rc::as_ptr(m) as usize),
            self.scoped(),
        )
    }

    fn filter_q(&self, cx: &App) -> String {
        js::trim(&self.filter.read(cx).value()).to_lowercase()
    }

    /// `matchedFiles`.
    pub fn matched_files(&self, cx: &App) -> Vec<Rc<FileDiff>> {
        let q = self.filter_q(cx);
        let key = (
            self.visible_key(),
            q.clone(),
            self.findings_only,
            self.props
                .findings_by_path
                .as_ref()
                .map_or(0, |b| Rc::as_ptr(b) as usize),
        );
        if let Some((k, v)) = &self.matched_memo.borrow().matched
            && *k == key
        {
            return v.clone();
        }
        let v = self.compute_matched(q);
        self.matched_memo.borrow_mut().matched = Some((key, v.clone()));
        v
    }

    fn compute_matched(&self, q: String) -> Vec<Rc<FileDiff>> {
        let mut list: Vec<Rc<FileDiff>> = self
            .visible_files()
            .into_iter()
            .filter(|f| {
                q.is_empty()
                    || f.path.to_lowercase().contains(&q)
                    || f.old_path.to_lowercase().contains(&q)
            })
            .collect();
        if self.findings_only {
            let by = self.props.findings_by_path.clone();
            list.retain(|f| {
                by.as_deref()
                    .is_some_and(|b| !findings_for_path(b, &f.path).is_empty())
            });
        }
        sort_files_by_changed_tree(&list, |f| &f.path)
    }

    fn has_open_findings(&self) -> bool {
        self.props
            .findings_by_path
            .as_deref()
            .is_some_and(|b| b.iter().any(|(_, fs)| !fs.is_empty()))
    }

    fn finding_count(&self) -> usize {
        let Some(by) = self.props.findings_by_path.as_deref() else {
            return 0;
        };
        self.visible_files()
            .iter()
            .map(|f| findings_for_path(by, &f.path).len())
            .sum()
    }

    /// `headerCount`.
    pub fn header_count(&self, cx: &App) -> String {
        let matched = self.matched_files(cx).len();
        let visible = self.visible_files().len();
        let n = if self.scoped() {
            visible
        } else {
            self.props.count.unwrap_or(visible)
        };
        if !self.filter_q(cx).is_empty() || self.findings_only {
            return format!("{matched}/{n}");
        }
        (if self.scoped() { matched } else { n }).to_string()
    }

    fn split_fallback(&self) -> bool {
        self.diff_layout == DiffLayout::Split
            && self.pane_width > 0.
            && self.pane_width < SPLIT_MIN_PX
    }

    fn effective_layout(&self) -> DiffLayout {
        if self.split_fallback() {
            DiffLayout::Unified
        } else {
            self.diff_layout
        }
    }

    /// Image cards compare the scope's old side with its commit, or the worktree.
    fn image_revs(&self) -> ImageRevs {
        let (before, after) = match &self.props.scope {
            FilesScope::Base => ("base".to_string(), String::new()),
            FilesScope::Working => ("HEAD".to_string(), String::new()),
            FilesScope::Commit(c) => {
                let sha = if c.sha.is_empty() { &c.hash } else { &c.sha };
                (format!("{sha}^"), sha.clone())
            }
        };
        ImageRevs {
            project_id: self.props.project_id.clone(),
            workspace_id: self.props.workspace_id.clone(),
            before,
            after,
            t: self.props.refresh as i64,
        }
    }

    /// Push matched files and card props into the stack and the list.
    fn sync_stack(&mut self, cx: &mut Context<Self>) {
        let matched = self.matched_files(cx);
        self.stack.set_files(matched.clone());
        let me = cx.entity().downgrade();
        let me2 = me.clone();
        let me3 = me.clone();
        let hooks = DiffFileHooks {
            toggle_collapse: None,
            open_file: Some(Rc::new(move |path, line, _, cx| {
                let _ = me.update(cx, |_, cx| cx.emit(FilesEvent::OpenFile(path, line)));
            })),
            add_finding: self.add_finding.clone(),
            open_finding: Some(Rc::new(move |f, _, cx| {
                let _ = me2.update(cx, |_, cx| cx.emit(FilesEvent::OpenFinding(f)));
            })),
            post_finding: None,
        };
        let _ = me3;
        self.stack.set_props(CardProps {
            layout: self.effective_layout(),
            note_mode: Some(self.props.note_mode),
            comments: self.props.note_mode != NoteMode::None,
            collapsible: true,
            open_file_title: None,
            findings_by_path: self.props.findings_by_path.clone(),
            highlight_line: 0,
            posting_finding_ids: HashSet::new(),
            postable_finding_ids: HashSet::new(),
            hooks,
            image_revs: Some(self.image_revs()),
        });
        let list_props = FileListProps {
            files: matched.clone(),
            selected_path: self.stack.selected().to_string(),
            mode: self.file_list_mode,
            findings_by_path: self.props.findings_by_path.clone(),
            comments_by_path: None,
            dirty_paths: self.props.dirty_paths.clone(),
            reveal: self.stack.toc_reveal,
        };
        self.list.update(cx, |l, cx| l.set_props(list_props, cx));
        let add: i64 = matched.iter().map(|f| f.diff.add).sum();
        let del: i64 = matched.iter().map(|f| f.diff.del).sum();
        let totals = (add, del, matched.len());
        if self.last_totals != Some(totals) {
            self.last_totals = Some(totals);
            cx.emit(FilesEvent::Totals {
                add,
                del,
                files: matched.len(),
            });
        }
    }

    pub fn select_path(&mut self, path: &str, cx: &mut Context<Self>) {
        self.stack.select_path(path);
        self.sync_stack(cx);
        cx.notify();
    }

    pub fn set_diff_layout(&mut self, next: DiffLayout, cx: &mut Context<Self>) {
        self.diff_layout = next;
        self.state
            .update(cx, |s, _| write_diff_layout(s.storage_mut(), next));
        self.sync_stack(cx);
        cx.notify();
    }

    pub fn set_file_list_mode(&mut self, next: FileListMode, cx: &mut Context<Self>) {
        self.file_list_mode = next;
        self.state
            .update(cx, |s, _| write_file_list_mode(s.storage_mut(), next));
        self.sync_stack(cx);
        cx.notify();
    }

    pub fn set_sidebar_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.sidebar_open = open;
        self.state
            .update(cx, |s, _| write_review_sidebar(s.storage_mut(), open));
        cx.notify();
    }

    pub fn set_findings_only(&mut self, on: bool, cx: &mut Context<Self>) {
        self.findings_only = on;
        self.sync_stack(cx);
        cx.notify();
    }

    pub fn clear_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter.update(cx, |f, cx| f.set_value("", window, cx));
        self.findings_only = false;
        self.sync_stack(cx);
        cx.notify();
    }

    fn toggle_collapsed(&mut self, path: String, next: bool, cx: &mut Context<Self>) {
        self.stack.toggle_collapsed(&path, next);
        cx.notify();
    }

    pub fn toggle_fold_all(&mut self, cx: &mut Context<Self>) {
        self.stack.toggle_fold_all();
        cx.notify();
    }

    /// `getViewState`.
    pub fn view_state(&self) -> (String, f32) {
        (
            self.stack.selected().to_string(),
            self.stack.pane.scroll_top(),
        )
    }

    // ---- loading ----

    fn root(&self) -> String {
        format!("{}\0{}", self.props.project_id, self.props.workspace_id)
    }

    fn files_scope_key(&self) -> String {
        let root = self.root();
        match &self.props.scope {
            FilesScope::Working => format!("{root}\0working"),
            FilesScope::Commit(c) => {
                let sha = commit_sha(c);
                let sha = if sha.is_empty() { c.hash.clone() } else { sha };
                if sha.is_empty() {
                    format!("{root}\0base")
                } else {
                    format!("{root}\0commit:{sha}")
                }
            }
            FilesScope::Base => format!("{root}\0base"),
        }
    }

    fn adopt_files_scope(&mut self, scope: String) {
        if self.files_scope != scope {
            self.files.clear();
            self.files_gen += 1;
            self.files_truncated = false;
            self.loading = true;
            self.error = false;
        } else if self.files.is_empty() {
            self.loading = true;
        }
        self.load_scope = scope;
    }

    fn drop_working_snapshot(&mut self) {
        if !self.files_scope.ends_with("\0working") && !self.load_scope.ends_with("\0working") {
            return;
        }
        self.files.clear();
        self.files_gen += 1;
        self.files_truncated = false;
        self.files_scope.clear();
        self.load_scope.clear();
        self.error = false;
        self.loading = false;
        self.load_queued = false;
        self.load_gen += 1;
    }

    fn apply_restore(&mut self, got: &[FileDiff]) {
        let root = self.root();
        if self.restore_root != root {
            self.restore_root = root;
            self.restored_once = false;
        }
        let Some((path, top)) = self.props.restore.clone() else {
            return;
        };
        if self.restored_once || path.is_empty() || !got.iter().any(|f| f.path == path) {
            return;
        }
        self.restored_once = true;
        self.stack.set_selected(&path);
        self.stack.pane.set_scroll_top(top);
    }

    /// `load()`.
    pub fn load(&mut self, cx: &mut Context<Self>) {
        if !self.props.active {
            return;
        }
        let scope = self.files_scope_key();
        if self.load_running {
            self.load_queued = true;
            if self.load_scope != scope {
                self.adopt_files_scope(scope);
                self.load_gen += 1;
            }
            return;
        }
        self.load_running = true;
        self.load_queued = false;
        self.load_gen += 1;
        let generation = self.load_gen;
        self.adopt_files_scope(scope.clone());
        self.error = false;
        let services = self.state.read(cx).services().clone();
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        let scope_kind = self.props.scope.clone();
        let t0 = Instant::now();
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let res: Result<(Vec<FileDiff>, bool, Option<Commit>), ()> = match &scope_kind {
                FilesScope::Working => services
                    .code
                    .diff_working(&pid, &wid)
                    .await
                    .map(|r| (r.files.unwrap_or_default(), r.truncated, None))
                    .map_err(|_| ()),
                FilesScope::Commit(c) if c.files.as_ref().is_some_and(|f| !f.is_empty()) => {
                    Ok((c.files.clone().unwrap_or_default(), false, None))
                }
                FilesScope::Commit(c) => {
                    let sha = commit_sha(c);
                    let sha = if sha.is_empty() { c.hash.clone() } else { sha };
                    services
                        .code
                        .commit_diff(&pid, &wid, &sha)
                        .await
                        .map(|files| (files, false, Some((**c).clone())))
                        .map_err(|_| ())
                }
                FilesScope::Base => services
                    .code
                    .diff_vs_base(&pid, &wid)
                    .await
                    .map(|r| (r.files.unwrap_or_default(), r.truncated, None))
                    .map_err(|_| ()),
            };
            let _ = this.update(cx, |v, cx| {
                let shown = (v.files_gen, v.loading, v.error, v.files_truncated);
                let same_scope = v.files_scope == scope;
                if let Ok((got, _, Some(c))) = &res
                    && generation == v.load_gen
                {
                    cx.emit(FilesEvent::CommitFiles(c.clone(), got.clone()));
                }
                let stale = generation != v.load_gen || !v.props.active;
                if !stale {
                    match res {
                        Ok((got, truncated, _)) => {
                            let ms = t0.elapsed().as_millis();
                            if ms >= 250 {
                                let lines: usize = got
                                    .iter()
                                    .map(|f| f.lines.as_ref().map_or(0, Vec::len))
                                    .sum();
                                let kind = match v.props.scope {
                                    FilesScope::Working => "DiffWorking",
                                    FilesScope::Commit(_) => "CommitDiff",
                                    FilesScope::Base => "DiffVsBase",
                                };
                                tracing::warn!(
                                    "[review] {kind} {} files / {lines} lines in {ms}ms",
                                    got.len()
                                );
                            }
                            v.files_scope = scope.clone();
                            v.files_truncated = truncated;
                            let restore = got.clone();
                            if let Some(next) = reuse_file_diffs(&v.files, got) {
                                v.files = next;
                                v.files_gen += 1;
                                v.apply_restore(&restore);
                            }
                        }
                        Err(()) => {
                            v.error = true;
                            if v.files_scope != scope {
                                v.files.clear();
                                v.files_gen += 1;
                                v.files_truncated = false;
                            }
                        }
                    }
                }
                if generation == v.load_gen {
                    v.loading = false;
                }
                v.load_running = false;
                // A reload that found the same diff (a watcher event, a commit draft) leaves
                // the review and the Observatory around it as they were.
                if !same_scope || shown != (v.files_gen, v.loading, v.error, v.files_truncated) {
                    v.sync_stack(cx);
                    cx.notify();
                }
                if v.load_queued && v.props.active {
                    v.load(cx);
                }
            });
        }));
    }
}

// ---- rendering ----

impl FilesChangedView {
    fn toolbar(
        &self,
        list_w: f32,
        stacked: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let open = self.sidebar_open;
        let toggle_label = if open {
            "Hide file list"
        } else {
            "Show file list"
        };
        let toggle = titled(
            "fl-pane-toggle-t",
            toggle_label,
            div()
                .id("fl-pane-toggle")
                .debug_selector(move || format!("button:{toggle_label}"))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(28.))
                .rounded(px(5.))
                .cursor_pointer()
                .text_color(hsla(if open { t.text } else { t.faint }))
                .hover(|s| s.text_color(hsla(t.text)).bg(hsla(t.pane_2)))
                .child(icon(IconName::PanelLeft).size(px(16.)))
                .on_click(cx.guarded_listener(move |v, _, _, cx| v.set_sidebar_open(!open, cx))),
        );
        let mut cluster = div()
            .flex()
            .items_center()
            .h_full()
            .min_w_0()
            .px(px(8.))
            .border_r_1()
            .border_color(hsla(if open {
                t.line
            } else {
                recur_ui::color::TRANSPARENT
            }))
            .child(toggle);
        if open {
            let mode = self.file_list_mode;
            cluster = cluster.child(
                div().ml_auto().child(seg(
                    [
                        titled(
                            "fl-mode-flat-t",
                            "Flat list",
                            seg_btn(
                                "fl-mode-flat",
                                IconName::List,
                                mode == FileListMode::Flat,
                                None,
                                cx,
                            )
                            .debug_selector(|| {
                                format!("button:Flat list:{}", mode == FileListMode::Flat)
                            })
                            .on_click(cx.guarded_listener(
                                |v, _, _, cx| v.set_file_list_mode(FileListMode::Flat, cx),
                            )),
                        )
                        .into_any_element(),
                        titled(
                            "fl-mode-tree-t",
                            "Tree",
                            seg_btn(
                                "fl-mode-tree",
                                IconName::Workflow,
                                mode == FileListMode::Tree,
                                None,
                                cx,
                            )
                            .debug_selector(|| {
                                format!("button:Tree:{}", mode == FileListMode::Tree)
                            })
                            .on_click(cx.guarded_listener(
                                |v, _, _, cx| v.set_file_list_mode(FileListMode::Tree, cx),
                            )),
                        )
                        .into_any_element(),
                    ],
                    cx,
                )),
            );
        }
        let count = self.header_count(cx);
        let mut main = div()
            .flex()
            .items_center()
            .min_w_0()
            .h_full()
            .px(px(13.))
            .child(
                text(format!(
                    "{count} file{}",
                    if count == "1" { "" } else { "s" }
                ))
                .family(Family::Mono)
                .size(9.5)
                .tracking(0.1)
                .uppercase()
                .color(t.faint)
                .nowrap(),
            );
        let vs = match &self.props.scope {
            FilesScope::Working => Some(("vs HEAD".to_string(), "Compared to HEAD".to_string())),
            FilesScope::Base => {
                let l = self.vs_label();
                Some((format!("vs {l}"), format!("Compared to {l}")))
            }
            FilesScope::Commit(_) => None,
        };
        if let Some((s, title)) = vs {
            main = main.child(titled(
                "obs-review-vs-t",
                title,
                div()
                    .id("obs-review-vs")
                    .debug_selector(|| "obs-review-vs".into())
                    .ml(px(8.))
                    .child(
                        text(s)
                            .family(Family::Mono)
                            .size(11.)
                            .color(t.muted)
                            .nowrap(),
                    ),
            ));
        }
        let fc = self.finding_count();
        if fc > 0 {
            main = main.child(
                div()
                    .id("st-findings")
                    .ml(px(10.))
                    .cursor_pointer()
                    .text_color(hsla(t.amber))
                    .hover(|s| s.text_color(hsla(t.text)))
                    .child(recur_ui::style::button_label(
                        format!("{fc} finding{}", if fc == 1 { "" } else { "s" }),
                        11.,
                        400.,
                    ))
                    .on_click(cx.guarded_listener(|_, _, _, cx| cx.emit(FilesEvent::ShowFindings))),
            );
        }
        let matched = self.matched_files(cx);
        let add: i64 = matched.iter().map(|f| f.diff.add).sum();
        let del: i64 = matched.iter().map(|f| f.diff.del).sum();
        main = main.child(div().ml_auto().child(crate::style::add_del(
            add,
            del,
            Family::Mono,
            9.5,
            5.,
            LineHeight::Scale(1.45),
            cx,
        )));
        let fallback = self.split_fallback();
        let layout = self.diff_layout;
        let backdrop = t.pane;
        main = main.child(
            div().ml(px(8.)).child(seg(
                [
                    titled(
                        "dl-unified-t",
                        "Unified",
                        seg_btn(
                            "dl-unified",
                            IconName::Rows2,
                            layout == DiffLayout::Unified,
                            None,
                            cx,
                        )
                        .debug_selector(|| "button:Unified diff".into())
                        .on_click(cx.guarded_listener(|v, _, _, cx| {
                            v.set_diff_layout(DiffLayout::Unified, cx)
                        })),
                    )
                    .into_any_element(),
                    titled(
                        "dl-split-t",
                        if fallback {
                            "Too narrow for side-by-side — widen the pane or hide the file list"
                        } else {
                            "Side by side"
                        },
                        seg_btn(
                            "dl-split",
                            IconName::Columns2,
                            layout == DiffLayout::Split,
                            fallback.then_some(backdrop),
                            cx,
                        )
                        .debug_selector(|| "button:Side-by-side diff".into())
                        .on_click(cx.guarded_listener(
                            move |v, _, _, cx| {
                                if !fallback {
                                    v.set_diff_layout(DiffLayout::Split, cx)
                                }
                            },
                        )),
                    )
                    .into_any_element(),
                ],
                cx,
            )),
        );
        let any = self.stack.any_expanded();
        let fold_label = if any {
            "Collapse all files"
        } else {
            "Expand all files"
        };
        main = main.child(
            div().ml(px(4.)).child(titled(
                "fold-all-t",
                fold_label,
                icon_btn(
                    "fold-all",
                    if any {
                        IconName::FoldVertical
                    } else {
                        IconName::UnfoldVertical
                    },
                    26.,
                    15.,
                    None,
                    cx,
                )
                .debug_selector(move || format!("button:{fold_label}"))
                .on_click(cx.guarded_listener(|v, _, _, cx| v.toggle_fold_all(cx))),
            )),
        );
        let _ = window;
        let mut bar = div()
            .debug_selector(|| "review-toolbar".into())
            .flex()
            .items_center()
            .flex_none()
            .h(px(recur_ui::style::CHROME_H))
            .bg(hsla(t.pane))
            .border_b_1()
            .border_color(hsla(t.line))
            .overflow_hidden();
        bar = if open && !stacked {
            bar.child(div().flex_none().w(px(list_w)).h_full().child(cluster))
        } else {
            bar.child(div().flex_none().h_full().child(cluster))
        };
        bar.child(div().flex_1().min_w_0().h_full().child(main))
            .into_any_element()
    }

    fn file_list_panel(
        &self,
        width: f32,
        stacked: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let state = self.state.clone();
        let focus = cx.focus_handle();
        let handle = resize_handle("fl-resize", Axis::X, Edge::End, width, &focus)
            .on_live({
                let state = state.clone();
                move |w, _, cx| {
                    state.update(cx, |s, cx| s.set_panel_live(PanelKey::PrFileList, w, cx))
                }
            })
            .on_commit({
                let state = state.clone();
                move |_, cx| state.update(cx, |s, _| s.persist_panels())
            })
            .on_step(move |w, _, cx| {
                state.update(cx, |s, cx| s.set_panel(PanelKey::PrFileList, w, cx))
            });
        let mut filter = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .flex_none()
            .h(px(recur_ui::style::CHROME_H))
            .px(px(8.))
            .border_b_1()
            .border_color(hsla(t.line))
            .text_color(hsla(t.faint))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .flex_1()
                    .min_w_0()
                    .key_context("TextEntry")
                    .on_key_down(cx.guarded_listener(|v, ev: &KeyDownEvent, window, cx| {
                        if ev.keystroke.key == "escape" {
                            cx.stop_propagation();
                            v.filter.update(cx, |f, cx| f.set_value("", window, cx));
                            v.sync_stack(cx);
                            cx.notify();
                        }
                    }))
                    .child(icon(IconName::Search).size(px(12.)))
                    // WebKit's `type=search` field is 19px tall at 12px, text centered.
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h(px(19.))
                            .flex()
                            .items_center()
                            .child(recur_ui::style::text_field(
                                &self.filter,
                                &SharedString::from("Filter files"),
                                t.faint,
                                Family::Ui,
                                12.,
                                window,
                                cx,
                            )),
                    ),
            );
        if self.has_open_findings() {
            let on = self.findings_only;
            let label = if on {
                "Show all files"
            } else {
                "Show files with open findings"
            };
            let btn = div()
                .id("fl-findings-only")
                .debug_selector(move || format!("button:{label}:{on}"))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(26.))
                .rounded(px(5.))
                .cursor_pointer()
                .text_color(hsla(if on { t.amber } else { t.faint }))
                .when(on, |d| d.bg(hsla(t.amber_d)))
                .when(!on, |d| {
                    d.hover(|s| s.text_color(hsla(t.text)).bg(hsla(t.pane_2)))
                })
                .child(icon(IconName::HatGlasses).size(px(15.)).stroke_width(2.2))
                .on_click(cx.guarded_listener(move |v, _, _, cx| v.set_findings_only(!on, cx)));
            filter = filter.child(titled("fl-findings-only-t", label, btn));
        }
        div()
            .debug_selector(|| "filelist".into())
            .relative()
            .flex()
            .flex_col()
            .min_h_0()
            .min_w_0()
            .when(!stacked, |d| d.w(px(width)).flex_none().border_r_1())
            .when(stacked, |d| d.border_b_1().min_h(px(72.)))
            .border_color(hsla(t.line))
            .bg(hsla(t.bg))
            .when(!stacked, |d| d.child(handle))
            .child(filter)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    // Cached: scrolling the cards (or anything above) leaves the list's
                    // layout alone; it redraws when it notifies.
                    .child(recur_ui::style::cached_view(
                        &self.list,
                        gpui_kit::StyleRefinement::default().flex_1().min_h_0(),
                        cx,
                    )),
            )
            .into_any_element()
    }

    fn scope_nav(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let back = div()
            .id("cback")
            .debug_selector(|| "cback".into())
            .flex()
            .items_center()
            .gap(px(6.))
            .cursor_pointer()
            .text_color(hsla(t.muted))
            .hover(|s| s.text_color(hsla(t.text)))
            .child(recur_ui::style::button_label("← all commits", 11., 400.).family(Family::Mono))
            .on_click(cx.guarded_listener(|_, _, _, cx| cx.emit(FilesEvent::ClearScope)));
        let p = &self.props;
        let backdrop = t.bg;
        let older = if p.can_step_older {
            icon_btn("step-older", IconName::ChevronLeft, 26., 15., None, cx)
                .on_click(cx.guarded_listener(|_, _, _, cx| cx.emit(FilesEvent::StepScope(-1))))
        } else {
            crate::style::icon_btn_disabled(
                "step-older",
                IconName::ChevronLeft,
                26.,
                15.,
                backdrop,
                cx,
            )
        };
        let newer = if p.can_step_newer {
            icon_btn("step-newer", IconName::ChevronRight, 26., 15., None, cx)
                .on_click(cx.guarded_listener(|_, _, _, cx| cx.emit(FilesEvent::StepScope(1))))
        } else {
            crate::style::icon_btn_disabled(
                "step-newer",
                IconName::ChevronRight,
                26.,
                15.,
                backdrop,
                cx,
            )
        };
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .mb(px(14.))
            .child(back)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .flex_none()
                    .child(titled("step-older-t", p.step_older_label.clone(), older))
                    .child(titled("step-newer-t", p.step_newer_label.clone(), newer)),
            )
            .into_any_element()
    }

    fn file_count_suffix(&self) -> String {
        if self.loading && self.visible_files().is_empty() {
            return String::new();
        }
        let n = self.visible_files().len();
        // `{#if} · {n} files{/if}`: the block's leading space is trimmed.
        format!("· {n} file{}", if n == 1 { "" } else { "s" })
    }

    fn working_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        recur_ui::style::card(t.amber.mix(0.08, t.pane), t.line, 9.)
            .debug_selector(|| "working-hd".into())
            .flex()
            .flex_col()
            .gap(px(12.))
            .py(px(14.))
            .px(px(15.))
            .mb(px(14.))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(12.))
                    .child(crate::commits::cav_wip(30., 8., cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                text("Uncommitted changes")
                                    .size(14.)
                                    .weight(600.)
                                    .break_anywhere()
                                    .id("working-hd-t"),
                            )
                            .child(
                                div().mt(px(4.)).child(
                                    text(format!(
                                        "Working tree vs HEAD{}",
                                        self.file_count_suffix()
                                    ))
                                    .family(Family::Mono)
                                    .size(10.5)
                                    .color(t.faint)
                                    .id("working-hd-by"),
                                ),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn commit_header(&self, c: &Commit, cx: &mut Context<Self>) -> Option<AnyElement> {
        if c.message.is_empty() {
            return None;
        }
        Some(crate::commits::commit_header(
            c,
            &self.file_count_suffix(),
            cx,
        ))
    }

    fn empty(&self, s: impl Into<SharedString>, cx: &App) -> AnyElement {
        crate::commits::empty_state(s, cx)
    }
}

impl FilesChangedView {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        let panel = self.state.read(cx).panels.size(PanelKey::PrFileList);
        let stacked = self.surface_width > 0. && self.surface_width <= STACK_BELOW_PX;
        let list_w = if self.surface_width > 0. {
            panel.min(self.surface_width * 0.42)
        } else {
            panel
        };
        if self.focus_pending {
            self.focus_pending = false;
            let typing = window
                .focused(cx)
                .is_some_and(|f| f == self.filter.read(cx).focus_handle(cx));
            if !typing {
                window.focus(&self.stack.focus, cx);
            }
        }
        // `matchedFiles` and the list follow the inputs each frame.
        self.sync_stack(cx);
        let toolbar = self.toolbar(list_w, stacked, window, cx);
        let pane_probe = {
            let me = cx.entity().downgrade();
            let pane = self.stack.pane.clone();
            canvas(
                move |b, _, cx| {
                    // `contentRect`: inside the 14px padding and the scrollbar while it overflows.
                    let bar = if pane.max_scroll() > 0.5 {
                        recur_ui::widgets::scrollbar::layout_size(cx)
                    } else {
                        0.
                    };
                    let w = f32::from(b.size.width) - 28. - bar;
                    let me = me.clone();
                    cx.defer(move |cx| {
                        let _ = me.update(cx, |v, cx| {
                            if (v.pane_width - w).abs() > 0.5 {
                                v.pane_width = w;
                                cx.notify();
                            }
                        });
                    });
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
        };
        let stack_pane = self.stack_pane(cx);
        let mut wrap = div()
            .debug_selector(|| "diffwrap".into())
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .when(stacked && self.sidebar_open, |d| d.flex_col());
        if self.sidebar_open {
            wrap = wrap.child(self.file_list_panel(list_w, stacked, window, cx));
        }
        wrap = wrap.child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .child(recur_ui::style::cached_view(
                    &stack_pane,
                    gpui_kit::StyleRefinement::default()
                        .flex_1()
                        .min_h_0()
                        .min_w_0(),
                    cx,
                ))
                .child(pane_probe)
                .children(if self.note_portals_here {
                    self.stack.pane.portals()
                } else {
                    Vec::new()
                }),
        );
        let surface_probe = {
            let me = cx.entity().downgrade();
            canvas(
                move |b, _, cx| {
                    let w = f32::from(b.size.width);
                    let me = me.clone();
                    cx.defer(move |cx| {
                        let _ = me.update(cx, |v, cx| {
                            if (v.surface_width - w).abs() > 0.5 {
                                v.surface_width = w;
                                cx.notify();
                            }
                        });
                    });
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
        };
        div()
            .debug_selector(|| "review:changes".into())
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(hsla(t.bg))
            .child(toolbar)
            .child(wrap)
            .child(surface_probe)
    }
}

/// The card stack as a cached view: scrolling it re-renders the cards and the toolbar above,
/// not the file list beside it, and a redraw from elsewhere (typing in the chat beside the
/// review) reuses its layout. Any notify of the files view redraws it too.
pub struct StackPane {
    files: gpui_kit::WeakEntity<FilesChangedView>,
    _sub: Subscription,
}

impl StackPane {
    fn render_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match self.files.upgrade() {
            Some(files) => {
                // Code selects across rows and files, as the page selects across the stack.
                let sel = recur_ui::selection::state("review-selection", window, cx);
                let stack = recur_ui::style::cached_root(
                    files.update(cx, |f, cx| f.stack_element(window, cx)),
                );
                recur_ui::selection::scope(&sel, false, stack, cx)
            }
            None => div().into_any_element(),
        }
    }
}

impl Render for StackPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

impl FilesChangedView {
    fn stack_pane(&mut self, cx: &mut Context<Self>) -> Entity<StackPane> {
        if let Some(p) = &self.stack_pane {
            return p.clone();
        }
        let me = cx.entity();
        let pane = cx.new(|cx| StackPane {
            files: me.downgrade(),
            _sub: cx.guarded_observe(&me, |_, _, cx| cx.notify()),
        });
        self.stack_pane = Some(pane.clone());
        pane
    }

    /// The card stack with its header and empty state: [`StackPane`]'s content.
    fn stack_element(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let q = self.filter_q(cx);
        let matched_empty = self.stack.files().is_empty();
        let mut header: Vec<AnyElement> = Vec::new();
        match self.props.scope.clone() {
            FilesScope::Commit(c) => {
                header.push(self.scope_nav(cx));
                if let Some(h) = self.commit_header(&c, cx) {
                    header.push(h);
                }
            }
            FilesScope::Working => {
                header.push(self.scope_nav(cx));
                header.push(self.working_header(cx));
            }
            FilesScope::Base => {}
        }
        if self.files_truncated {
            header.push(self.empty(
                "Untracked file list truncated — more files exist vs HEAD",
                cx,
            ));
        }
        let empty = if self.loading && matched_empty && q.is_empty() {
            Some(self.empty("Loading diff…", cx))
        } else if self.error && matched_empty && q.is_empty() {
            Some(self.empty(
                match self.props.scope {
                    FilesScope::Working => "Could not load uncommitted diff",
                    FilesScope::Commit(_) => "Could not load this commit's diff",
                    FilesScope::Base => "Could not load files changed",
                },
                cx,
            ))
        } else if (!q.is_empty() || self.findings_only) && matched_empty {
            Some(crate::commits::empty_clear_filter(
                cx.guarded_listener(|v, _, window, cx| v.clear_filter(window, cx)),
                cx,
            ))
        } else if matched_empty {
            Some(self.empty(
                match self.props.scope {
                    FilesScope::Working => "No uncommitted changes".to_string(),
                    FilesScope::Commit(_) => "No files in this commit".to_string(),
                    FilesScope::Base => format!("No files changed since {}", self.vs_label()),
                },
                cx,
            ))
        } else {
            None
        };
        self.stack.render(
            header,
            empty,
            |v: &mut FilesChangedView| &mut v.stack,
            Rc::new(|v: &mut FilesChangedView, path, next, cx| v.toggle_collapsed(path, next, cx)),
            window,
            cx,
        )
    }
}

impl Render for FilesChangedView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}
