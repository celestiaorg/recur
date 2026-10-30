//! One file's diff: the header and the line-numbered, syntax-highlighted
//! rows (unified or split) with folding hunks, inline threads, pending notes, findings and the
//! line-note composer (click or drag the `+` to pick a range). Long files window their rows
//! against the scrolling pane ([`crate::review_window`]).

use recur_ui::guard::Guarded;
use recur_view_core::{ObserveState as _, Topics};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::base::input::{Textarea, TextareaMode, TextareaState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, ClickEvent, ClipboardItem, Context, DispatchPhase,
    Entity, FocusHandle, Hsla, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Render, Rgba, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, Window, canvas, div, px,
};
use recur_code::view::finding_paths::{
    extra_html_without_finding_flags, findings_covering_line, findings_ending_line,
    findings_on_line,
};
use recur_content::images::{self, ImageCache, Raster};
use recur_domain::{DiffLine, FileDiff, Finding, PendingLineComment, Pr, PrComment};
use recur_engine::view::comment_thread::{
    DiffSide, is_line_note_root, line_notes_covering_row, line_notes_on_row, same_comment_path,
    thread_replies,
};
use recur_engine::view::js;
use recur_ui::fonts::Family;
use recur_ui::text::{Align, LineHeight, Span, Text, TextStyled, span, text};
use recur_ui::tooltip::titled;
use recur_ui::widgets::ToastKind;
use recur_ui::{Color, IconName, Theme, ThemeId, icon};
use recur_view_core::{AppState, call, toast};
use recur_view_widgets::findings_ui::finding_sev_icon;
use recur_view_widgets::suggest::{SuggestAttach, attach_emoji, attach_user_mentions};

use crate::diff_rows::{
    OpenLine, Selecting, api_line, compose_placeholder, composer_on, draft_covers, draft_ends,
    draft_sel, draft_tip, find_line, hunk_of, line_no, line_nums, open_line_no, pending_covering,
    pending_on, row_num, row_side, split_slot_of_line, split_slots, unified_slot_of_line,
    unified_slots,
};
use crate::html::{cell_text, hunk_label};
use crate::line_note::{LineNoteThread, pr_conversation_locked};
use crate::note_portal::{Draft, Slot};
use crate::pane::Pane;
use crate::review_diff::{DiffLayout, path_label};
use crate::review_scroll::{JumpTarget, PANE_SCROLL_BIAS, pane_scroll_to, pick_diff_jump};
use crate::review_window::{
    DIFF_LINE_OVERSCAN, DIFF_LINE_PX, DIFF_LINE_WINDOW_MIN, LineWin, apply_slot_pin, cover_near,
    jump_line_window, line_window, slot_pad,
};
use crate::split_diff::{
    SplitRow, slot_of_split_highlight, slot_of_unified_highlight, split_diff_rows,
    split_row_data_line,
};
use crate::store::patch_pr_pending_comments;
use crate::style::{BLANK_BG, HUNK_BG, HUNK_HOVER_BG, PENDING_BG, hsla, icon_btn, label, note_btn};
use crate::syntax::{self, Highlighted};
use recur_editor::theme::EditorTheme;

/// Who the line-note composer writes for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NoteMode {
    /// Pending PR review comments (`AddPRComment`).
    #[default]
    Pr,
    /// Observatory findings (`onAddFinding`), new side only.
    Finding,
    /// Read-only.
    None,
}

type ToggleFn = Rc<dyn Fn(bool, &mut Window, &mut App)>;
type OpenFileFn = Rc<dyn Fn(String, Option<i64>, &mut Window, &mut App)>;
/// `onAddFinding(path, line, body, endLine)`, resolving to whether it was saved.
pub type AddFindingFn = Rc<dyn Fn(String, i64, String, i64, &mut Window, &mut App) -> Task<bool>>;
type FindingFn = Rc<dyn Fn(Finding, &mut Window, &mut App)>;

/// The callback props. `None` is "not passed".
#[derive(Clone, Default)]
pub struct DiffFileHooks {
    /// `onToggleCollapse`: the host owns `collapsed`.
    pub toggle_collapse: Option<ToggleFn>,
    /// `onOpenFile(path, line?)`: the header opens the file; ⌘-click a line number jumps there.
    pub open_file: Option<OpenFileFn>,
    pub add_finding: Option<AddFindingFn>,
    pub open_finding: Option<FindingFn>,
    pub post_finding: Option<FindingFn>,
}

/// `DiffFile`'s props.
#[derive(Clone, PartialEq)]
pub struct DiffFileProps {
    pub file: Rc<FileDiff>,
    pub highlight_line: i64,
    /// `comments`: false hides the comment affordances when `note_mode` is unset.
    pub comments: bool,
    pub note_mode: Option<NoteMode>,
    pub layout: DiffLayout,
    pub collapsible: bool,
    /// Controlled collapse (with `hooks.toggle_collapse`).
    pub collapsed: bool,
    pub open_file_title: SharedString,
    pub findings: Vec<Finding>,
    pub posting_finding_ids: HashSet<String>,
    pub postable_finding_ids: HashSet<String>,
    /// An image's previous and current revision srcs ("" for a side that does not exist),
    /// shown side by side in place of rows.
    pub image: Option<(String, String)>,
}

impl DiffFileProps {
    pub fn new(file: Rc<FileDiff>) -> Self {
        DiffFileProps {
            file,
            highlight_line: 0,
            comments: true,
            note_mode: None,
            layout: DiffLayout::Unified,
            collapsible: false,
            collapsed: false,
            open_file_title: "Open in editor — ⌘-click any line number to jump there".into(),
            findings: Vec::new(),
            posting_finding_ids: HashSet::new(),
            postable_finding_ids: HashSet::new(),
            image: None,
        }
    }
}

/// A row's hit boxes for the drag over the `+` controls (`[data-cmt-side][data-cmt-line]`).
#[derive(Clone, Copy, Debug)]
struct RowHit {
    side: DiffSide,
    line: i64,
    row: Bounds<Pixels>,
}

/// What the last frame laid out, in window coordinates.
#[derive(Default)]
struct Geom {
    root: Option<Bounds<Pixels>>,
    /// The pane's scroll offset when `root` was painted.
    painted_top: f32,
    head_h: f32,
    /// Rendered slots: index → (top, height).
    slots: HashMap<usize, (f32, f32)>,
    rows: Vec<RowHit>,
    /// `.icmt-thread[data-note-line]` tops.
    threads: HashMap<i64, f32>,
    /// `[data-line]` row tops.
    lines: HashMap<i64, f32>,
    /// The `+` hit boxes by (left side, line).
    addcs: HashMap<(bool, i64), Bounds<Pixels>>,
}

/// A row's (or split side's) classes: `.noted`, `.note-end`, `.drafting`, `.draft-end`,
/// `.draft-tip`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RowFlags {
    pub noted: bool,
    pub note_end: bool,
    pub drafting: bool,
    pub draft_end: bool,
    pub draft_tip: bool,
}

pub struct DiffFile {
    state: Entity<AppState>,
    props: DiffFileProps,
    hooks: DiffFileHooks,
    pane: Option<Pane>,
    internal_collapsed: bool,
    open_line: Option<OpenLine>,
    open_side: DiffSide,
    open_start: i64,
    /// The composer's text box, drawn by a host outside the stack while the card has a pane.
    draft_portal: Option<(Rc<Slot>, Entity<DraftView>)>,
    draft_line: usize,
    selecting: Option<Selecting>,
    draft: Entity<TextareaState>,
    _attach: (
        Entity<SuggestAttach<TextareaMode>>,
        Entity<SuggestAttach<TextareaMode>>,
    ),
    posting: bool,
    collapsed_hunks: HashSet<usize>,
    win: LineWin,
    slot_h: Vec<Option<f32>>,
    slot_sig: String,
    last_jump: i64,
    loaded_path: String,
    copied_path: bool,
    copy_timer: Option<Task<()>>,
    threads: HashMap<String, Entity<LineNoteThread>>,
    threads_seen: HashSet<String>,
    /// The `+` buttons' tab stops by (left side, line).
    addc_focus: RefCell<HashMap<(bool, i64), FocusHandle>>,
    geom: Rc<RefCell<Geom>>,
    pending_pin: Option<(usize, f32, f32)>,
    pending_jump: Option<i64>,
    /// The sticky `.df-head`'s `.stuck`.
    head_stuck: bool,
    /// The composer's placeholder (`composePlaceholder`).
    placeholder: String,
    /// What `refreshLineWindow` last ran for: it re-runs on pane scroll, a resize of the file,
    /// and changes to the slots, composer, drag or highlight, never for its own window change.
    line_window_key: String,
    /// What the card reads of the selected PR, kept until the store changes it.
    facts: Rc<PrFacts>,
    /// The code's syntax runs, for the file and editor environment epoch in `hl_for`.
    hl: Highlighted,
    hl_for: Option<(Rc<FileDiff>, u64)>,
    hl_task: Option<Task<()>>,
    /// The file's grammar scope, for the code area's fill.
    hl_scope: String,
    /// The editor theme the code area paints with, for the env epoch and app theme in `ed_for`.
    ed: EditorTheme,
    ed_for: Option<(u64, ThemeId)>,
    /// The previous and current rasters of [`DiffFileProps::image`].
    rasters: [Raster; 2],
    /// `.diff-file.selecting` hovered row for `draft-tip` hover state.
    _subs: Vec<Subscription>,
}

/// What a card reads of the selected PR: the PR without its files, commits and comments, the
/// file's pending comments and its line notes with their replies. A card never holds the PR
/// itself (tens of MB with its patches), and redraws only when these change.
#[derive(Default, PartialEq)]
struct PrFacts {
    head: Option<Rc<Pr>>,
    pending: Vec<PendingLineComment>,
    notes: Vec<PrComment>,
}

fn replies_to(notes: &[PrComment], root: &PrComment) -> Vec<PrComment> {
    thread_replies(notes, &root.id)
        .into_iter()
        .cloned()
        .collect()
}

/// The line notes on `path` and every reply to them.
fn file_notes(comments: &[PrComment], path: &str) -> Vec<PrComment> {
    let roots: Vec<&PrComment> = comments
        .iter()
        .filter(|c| same_comment_path(&c.path, path))
        .collect();
    let mut out: Vec<PrComment> = roots.iter().map(|c| (*c).clone()).collect();
    for root in &roots {
        out.extend(
            thread_replies(comments, &root.id)
                .into_iter()
                .filter(|c| !same_comment_path(&c.path, path))
                .cloned(),
        );
    }
    out
}

impl DiffFile {
    pub fn new(
        state: Entity<AppState>,
        props: DiffFileProps,
        hooks: DiffFileHooks,
        pane: Option<Pane>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let draft = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(2)
                .placeholder("Comment…")
        });
        crate::style::style_textarea(&draft, cx);
        let emoji = attach_emoji(&draft, None, window, cx);
        let mentions = attach_user_mentions(&draft, state.clone(), None, window, cx);
        let env = recur_view_core::editor_env::editor_env(&state, cx);
        let subs = vec![
            cx.observe_state(&state, Topics::PRS, |this: &mut Self, _, cx| {
                this.refresh_facts(cx)
            }),
            cx.guarded_observe(&env, |this: &mut Self, _, cx| this.highlight(cx)),
        ];
        let mut me = DiffFile {
            state,
            props,
            hooks,
            pane,
            internal_collapsed: false,
            open_line: None,
            open_side: DiffSide::Right,
            open_start: 0,
            draft_portal: None,
            draft_line: 0,
            selecting: None,
            draft,
            _attach: (emoji, mentions),
            posting: false,
            collapsed_hunks: HashSet::new(),
            win: LineWin::new(0, DIFF_LINE_WINDOW_MIN),
            slot_h: Vec::new(),
            slot_sig: String::new(),
            last_jump: 0,
            loaded_path: String::new(),
            copied_path: false,
            copy_timer: None,
            threads: HashMap::new(),
            threads_seen: HashSet::new(),
            addc_focus: RefCell::default(),
            geom: Rc::default(),
            pending_pin: None,
            pending_jump: None,
            head_stuck: false,
            placeholder: "Comment…".into(),
            line_window_key: String::new(),
            facts: Rc::default(),
            hl: Vec::new(),
            hl_for: None,
            hl_task: None,
            hl_scope: String::new(),
            ed: EditorTheme::resolve(None, true, Theme::of(cx).id),
            ed_for: None,
            rasters: Default::default(),
            _subs: subs,
        };
        me.facts = Rc::new(me.read_facts(cx));
        me.highlight(cx);
        me.load_images(cx);
        me.reset_for_path();
        if me.props.highlight_line > 0 {
            me.begin_jump(me.props.highlight_line, window, cx);
        }
        me
    }

    /// Reset per-file UI only when the path changes: a poll that replaces the file with the
    /// same path must not dismiss an open composer.
    fn reset_for_path(&mut self) {
        let path = self.props.file.path.clone();
        if path == self.loaded_path {
            return;
        }
        self.loaded_path = path;
        self.internal_collapsed = false;
        self.open_line = None;
        self.collapsed_hunks.clear();
        self.addc_focus.borrow_mut().clear();
        self.win = LineWin::new(0, DIFF_LINE_WINDOW_MIN);
        self.slot_h.clear();
        self.slot_sig.clear();
        self.last_jump = 0;
    }

    pub fn props(&self) -> &DiffFileProps {
        &self.props
    }

    pub fn set_props(&mut self, props: DiffFileProps, window: &mut Window, cx: &mut Context<Self>) {
        if props == self.props {
            return;
        }
        let jump = props.highlight_line != self.props.highlight_line;
        self.props = props;
        self.facts = Rc::new(self.read_facts(cx));
        self.highlight(cx);
        self.load_images(cx);
        let path_changed = self.props.file.path != self.loaded_path;
        self.reset_for_path();
        if path_changed {
            self.draft.update(cx, |d, cx| d.set_value("", window, cx));
        }
        if jump {
            let target = self.props.highlight_line;
            if target == 0 {
                self.last_jump = 0;
                self.pending_jump = None;
            } else {
                self.begin_jump(target, window, cx);
            }
        }
        cx.notify();
    }

    /// Tokenizes the file's first lines ([`syntax::budgeted`]) off the UI thread once its
    /// grammar loads. Rows show their markup as is until then, and past the cap.
    fn highlight(&mut self, cx: &mut Context<Self>) {
        let env = recur_view_core::editor_env::editor_env(&self.state, cx);
        let (tm, epoch) = {
            let env = env.read(cx);
            (env.tm.clone(), env.epoch)
        };
        let file = self.props.file.clone();
        if self
            .hl_for
            .as_ref()
            .is_some_and(|(f, e)| Rc::ptr_eq(f, &file) && *e == epoch)
        {
            return;
        }
        self.hl_for = Some((file.clone(), epoch));
        let (grammar, color_map, pane_bg) = {
            let mut tm = tm.borrow_mut();
            let scope = tm
                .catalog
                .resolve_scope_for_path(&file.path, None)
                .unwrap_or_default();
            let grammar = (!scope.is_empty() && tm.catalog.load_grammar(&scope))
                .then(|| tm.catalog.engine().grammar(&scope))
                .flatten();
            let pane_bg = tm.catalog.active_pane_background(&scope);
            self.hl_scope = scope;
            (grammar, tm.catalog.color_map().to_vec(), pane_bg)
        };
        let Some(grammar) = grammar else {
            self.hl.clear();
            self.hl_task = None;
            cx.notify();
            return;
        };
        let lines = syntax::budgeted(file.lines.as_deref().unwrap_or_default()).to_vec();
        let job = cx.background_spawn(async move {
            syntax::highlight_lines(&grammar, &lines, &color_map, &pane_bg)
        });
        self.hl_task = Some(cx.spawn(async move |this, cx| {
            let hl = job.await;
            let _ = this.update(cx, |this, cx| {
                this.hl = hl;
                cx.notify();
            });
        }));
    }

    /// Resolves the editor theme the code area paints with when it or the app theme changed.
    fn sync_editor_theme(&mut self, cx: &mut Context<Self>) {
        let env = recur_view_core::editor_env::editor_env(&self.state, cx);
        let env = env.read(cx);
        let key = (env.epoch, Theme::of(cx).id);
        if self.ed_for != Some(key) {
            self.ed = EditorTheme::resolve(env.theme.as_deref(), env.dark_fallback, key.1);
            self.ed_for = Some(key);
        }
    }

    /// Loads the image sides whose src changed, through the shared [`ImageCache`].
    fn load_images(&mut self, cx: &mut Context<Self>) {
        let (before, after) = self.props.image.clone().unwrap_or_default();
        for (i, src) in [before, after].into_iter().enumerate() {
            if self.rasters[i].src == src {
                continue;
            }
            self.rasters[i] = Raster {
                src: src.clone(),
                ..Default::default()
            };
            if src.is_empty() {
                continue;
            }
            if let Some(d) = cx.try_global::<ImageCache>().and_then(|c| c.0.get(&src)) {
                self.rasters[i].image = Some(d.clone());
                continue;
            }
            let Some(fut) = images::fetch(&src, cx) else {
                self.rasters[i].failed = true;
                continue;
            };
            cx.spawn(async move |this, cx| {
                let res = cx.background_spawn(fut).await;
                let _ = this.update(cx, |this, cx| {
                    if this.rasters[i].src != src {
                        return;
                    }
                    match res {
                        Ok(d) => {
                            cx.default_global::<ImageCache>().0.insert(src, d.clone());
                            this.rasters[i].image = Some(d);
                        }
                        Err(_) => this.rasters[i].failed = true,
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    /// `.obs-image`'s Previous / Current pair for an image file.
    fn image_pair(&self, window: &Window, cx: &App) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let sides: Vec<(&str, &Raster)> = [
            ("Previous", &self.rasters[0]),
            ("Current", &self.rasters[1]),
        ]
        .into_iter()
        .filter(|(_, r)| !r.src.is_empty())
        .collect();
        let card_w = self
            .geom
            .borrow()
            .root
            .map_or(800., |b| f32::from(b.size.width));
        let n = sides.len().max(1) as f32;
        let cell_w = ((card_w - 48. - 28. * (n - 1.)) / n).max(40.);
        let scale = window.scale_factor();
        let figures = sides.into_iter().map(|(label, r)| {
            let pic = match &r.image {
                Some(d) => {
                    let ((w, h), _) =
                        recur_content::lightbox::fit((d.width, d.height), (cell_w, 360.));
                    let image = r
                        .at((w * scale).round() as u32, (h * scale).round() as u32)
                        .unwrap_or_else(|| d.image.clone());
                    recur_ui::shadow::with_shadow(
                        div().w(px(w)).h(px(h)).child(
                            canvas(
                                |_, _, _| {},
                                move |b, _, window, _| {
                                    let _ = window.paint_image(
                                        b,
                                        b,
                                        Default::default(),
                                        image.clone(),
                                        0,
                                        false,
                                    );
                                },
                            )
                            .size_full(),
                        ),
                        &[recur_ui::shadow::css_box_shadow(0., 0., 0., 1., t.line)],
                        0.,
                    )
                    .into_any_element()
                }
                None => text(if r.failed {
                    "Image unavailable"
                } else {
                    "Loading…"
                })
                .size(11.)
                .color(t.faint)
                .into_any_element(),
            };
            div()
                .debug_selector(move || format!("diff-image:{label}"))
                .min_w_0()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(8.))
                .child(pic)
                .child(text(label).size(11.).color(t.muted).nowrap())
        });
        crate::style::checkerboard(cx)
            .flex()
            .justify_center()
            .items_center()
            .gap(px(28.))
            .p(px(24.))
            .children(figures)
            .into_any_element()
    }

    pub fn set_hooks(&mut self, hooks: DiffFileHooks) {
        self.hooks = hooks;
    }

    /// Whether the stack's sticky header logic has `.df-head` stuck to the pane top.
    pub fn set_head_stuck(&mut self, stuck: bool, cx: &mut Context<Self>) {
        if stuck != self.head_stuck {
            self.head_stuck = stuck;
            cx.notify();
        }
    }

    /// The header height, from the last frame.
    pub fn head_height(&self) -> f32 {
        self.geom.borrow().head_h
    }

    pub fn set_pane(&mut self, pane: Option<Pane>) {
        self.pane = pane;
    }

    pub fn is_collapsed(&self) -> bool {
        if self.hooks.toggle_collapse.is_some() {
            self.props.collapsed
        } else {
            self.internal_collapsed
        }
    }

    fn lines(&self) -> &[DiffLine] {
        self.props.file.lines.as_deref().unwrap_or_default()
    }

    fn mode(&self) -> NoteMode {
        self.props.note_mode.unwrap_or(if self.props.comments {
            NoteMode::Pr
        } else {
            NoteMode::None
        })
    }

    fn read_facts(&self, cx: &App) -> PrFacts {
        if self.mode() != NoteMode::Pr {
            return PrFacts::default();
        }
        let s = self.state.read(cx);
        let Some(pr) = s.pr(&s.selected_pr_id) else {
            return PrFacts::default();
        };
        let path = &self.props.file.path;
        PrFacts {
            head: Some(Rc::new(recur_view_core::pr::pr_head(pr))),
            pending: pr
                .pending_comments
                .iter()
                .filter(|pc| pc.file == *path)
                .cloned()
                .collect(),
            notes: file_notes(pr.comments.as_deref().unwrap_or_default(), path),
        }
    }

    fn refresh_facts(&mut self, cx: &mut Context<Self>) {
        let facts = self.read_facts(cx);
        if facts != *self.facts {
            self.facts = Rc::new(facts);
            cx.notify();
        }
    }

    fn pr(&self) -> Option<&Pr> {
        self.facts.head.as_deref()
    }

    fn can_comment(&self, pr: Option<&Pr>) -> bool {
        let mode = self.mode();
        mode != NoteMode::None && (mode == NoteMode::Finding || !pr_conversation_locked(pr))
    }

    /// Findings are new-side only: left numbers would be stored as current-file lines.
    fn can_note_side(&self, side: DiffSide, pr: Option<&Pr>) -> bool {
        self.can_comment(pr) && (self.mode() != NoteMode::Finding || side == DiffSide::Right)
    }

    fn slots_unified(&self) -> Vec<usize> {
        if self.is_collapsed() {
            return Vec::new();
        }
        unified_slots(self.lines(), &self.collapsed_hunks)
    }

    fn slots_split(&self) -> Vec<SplitRow> {
        if self.props.layout != DiffLayout::Split || self.is_collapsed() {
            return Vec::new();
        }
        let rows = split_diff_rows(self.lines());
        split_slots(self.lines(), &rows, &self.collapsed_hunks)
    }

    fn slot_count(&self) -> usize {
        if self.props.layout == DiffLayout::Split {
            self.slots_split().len()
        } else {
            self.slots_unified().len()
        }
    }

    fn slot_of_line(&self, n: i64) -> isize {
        if self.props.layout == DiffLayout::Split {
            split_slot_of_line(self.lines(), &self.slots_split(), n)
        } else {
            unified_slot_of_line(self.lines(), &self.slots_unified(), n)
        }
    }

    /// The classes row `i` (or its split `side`) renders with.
    fn row_flags(
        &self,
        i: usize,
        side: Option<DiffSide>,
        pending: &[PendingLineComment],
        notes: &[PrComment],
    ) -> RowFlags {
        let l = &self.lines()[i];
        let path = &self.props.file.path;
        let sel = draft_sel(
            self.selecting,
            self.open_line,
            self.open_side,
            self.open_start,
        );
        // Split rows show findings on the right side only.
        let findings = side != Some(DiffSide::Left);
        RowFlags {
            noted: !line_notes_covering_row(notes, path, l, side).is_empty()
                || pending_covering(pending, path, l, side) > 0
                || (findings && !findings_covering_line(&self.props.findings, l).is_empty()),
            note_end: !line_notes_on_row(notes, path, l, side).is_empty()
                || !pending_on(pending, l, side).is_empty()
                || (findings && !findings_ending_line(&self.props.findings, l).is_empty()),
            drafting: draft_covers(sel, l, side),
            draft_end: draft_ends(sel, l, side),
            draft_tip: draft_tip(self.selecting, l, side),
        }
    }

    /// The classes of the row with number `n` on `side` (unified layout), for hosts and tests.
    pub fn row_classes(&self, n: i64, side: DiffSide, _cx: &App) -> Option<RowFlags> {
        let i = find_line(self.lines(), n, side)?;
        let split = self.props.layout == DiffLayout::Split;
        Some(self.row_flags(
            i,
            split.then_some(side),
            &self.facts.pending,
            &self.facts.notes,
        ))
    }

    /// The mounted line-note threads.
    pub fn thread_entities(&self) -> Vec<Entity<LineNoteThread>> {
        self.threads.values().cloned().collect()
    }

    #[cfg(test)]
    pub(crate) fn addc_focus(&self, side: DiffSide, line: i64) -> Option<FocusHandle> {
        self.addc_focus
            .borrow()
            .get(&(side == DiffSide::Left, line))
            .cloned()
    }

    /// The drag over the `+` controls, if one is in progress.
    pub fn selecting(&self) -> Option<Selecting> {
        self.selecting
    }

    // ---- composer ----

    fn open_composer(
        &mut self,
        line_i: usize,
        side: DiffSide,
        start: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_note_side(side, self.pr()) {
            return;
        }
        let line = self.lines()[line_i].clone();
        let (old_n, new_n) = line_nums(&line);
        let end = if side == DiffSide::Left { old_n } else { new_n };
        let start = if start > 0 && start < end { start } else { 0 };
        if composer_on(self.open_line, self.open_side, Some(&line), Some(side))
            && self.open_start == start
        {
            return;
        }
        if self
            .open_line
            .is_none_or(|o| o.new_n != new_n || o.old_n != old_n)
        {
            self.draft.update(cx, |d, cx| d.set_value("", window, cx));
        }
        self.open_line = Some(OpenLine { old_n, new_n });
        self.open_side = side;
        self.open_start = start;
        let ph = compose_placeholder(&line, side, start);
        self.placeholder = ph.clone();
        self.draft.update(cx, |d, cx| {
            d.set_placeholder(ph, window, cx);
            d.focus(window, cx);
        });
        self.open_portal(line_i, cx);
        cx.notify();
    }

    /// Open a line-note composer on the first new-side line that takes a note and focus it;
    /// whether it took focus (a card off screen has nothing to focus).
    pub fn open_first_note(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.can_note_side(DiffSide::Right, self.pr()) {
            return false;
        }
        let Some(i) = self
            .lines()
            .iter()
            .position(|l| l.r#type != "hunk" && l.r#type != "del" && !l.new.is_empty())
        else {
            return false;
        };
        self.open_composer(i, DiffSide::Right, 0, window, cx);
        gpui_kit::Focusable::focus_handle(self.draft.read(cx), cx).is_focused(window)
    }

    pub fn dismiss_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_line = None;
        self.open_start = 0;
        self.draft.update(cx, |d, cx| d.set_value("", window, cx));
        self.close_portal(cx);
        cx.notify();
    }

    /// Mounts the composer's text box for the host while the card has a pane.
    fn open_portal(&mut self, line_i: usize, cx: &mut Context<Self>) {
        self.draft_line = line_i;
        let Some(pane) = self.pane.clone() else {
            return;
        };
        let card = cx.entity();
        let (slot, view) = self
            .draft_portal
            .get_or_insert_with(|| {
                let view = cx.new(|_| DraftView {
                    card: card.downgrade(),
                });
                (Rc::new(Slot::default()), view)
            })
            .clone();
        pane.set_draft(Draft {
            card: card.entity_id(),
            slot,
            view: view.into(),
        });
    }

    fn close_portal(&mut self, cx: &mut Context<Self>) {
        if let Some(pane) = &self.pane {
            pane.clear_draft(cx.entity_id());
        }
    }

    /// The composer's card, live, at the size of its place in the card.
    fn draft_input(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (line_i, split) = (self.draft_line, self.props.layout == DiffLayout::Split);
        self.composer_card(line_i, split, true, window, cx)
            .size_full()
            .into_any_element()
    }

    /// ⌘↵ adds the note, Escape drops it.
    fn composer_key(
        &mut self,
        line_i: usize,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let k = &ev.keystroke;
        if k.key == "enter" && (k.modifiers.platform || k.modifiers.control) {
            cx.stop_propagation();
            self.submit(line_i, window, cx);
        } else if k.key == "escape" {
            cx.stop_propagation();
            self.dismiss_composer(window, cx);
        }
    }

    /// A line-note composer is open (`.icmt-compose`).
    pub fn composer_open(&self) -> bool {
        self.open_line.is_some()
    }

    /// The composer's text (`bind:value={draft}`).
    pub fn draft_text(&self, cx: &App) -> String {
        self.draft.read(cx).value().to_string()
    }

    /// The composer's placeholder text.
    pub fn compose_placeholder(&self) -> &str {
        &self.placeholder
    }

    pub fn draft_area(&self) -> &Entity<TextareaState> {
        &self.draft
    }

    pub fn open_line(&self) -> Option<OpenLine> {
        self.open_line
    }

    fn submit(&mut self, line_i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let body = js::trim(&self.draft_text(cx)).to_string();
        if body.is_empty() || self.posting || !self.can_comment(self.pr()) {
            return;
        }
        let line = self.lines()[line_i].clone();
        let path = self.props.file.path.clone();
        if self.mode() == NoteMode::Finding {
            let line_no = crate::html::js_int(&line.new);
            if line_no == 0 {
                return;
            }
            let Some(add) = self.hooks.add_finding.clone() else {
                return;
            };
            self.posting = true;
            cx.notify();
            let start = if self.open_start > 0 && self.open_start < line_no {
                self.open_start
            } else {
                line_no
            };
            let end_line = if start < line_no { line_no } else { 0 };
            let task = add(path, start, body, end_line, window, cx);
            cx.spawn_in(window, async move |this, cx| {
                let ok = task.await;
                let _ = this.update_in(cx, |d, window, cx| {
                    d.posting = false;
                    if ok {
                        d.draft.update(cx, |a, cx| a.set_value("", window, cx));
                        d.open_line = None;
                        d.open_start = 0;
                        toast(ToastKind::Success, "Noted", cx);
                    }
                    cx.notify();
                });
            })
            .detach();
            return;
        }
        let side = if self.props.layout == DiffLayout::Split {
            self.open_side
        } else {
            row_side(&line)
        };
        let signed = api_line(&line, side);
        let abs = signed.abs();
        let start = if self.open_start > 0 && self.open_start < abs {
            self.open_start
        } else {
            0
        };
        let start_signed = if start == 0 {
            0
        } else if side == DiffSide::Left {
            -start
        } else {
            start
        };
        let side_s = if side == DiffSide::Left {
            "LEFT"
        } else {
            "RIGHT"
        };
        let pc = PendingLineComment {
            id: format!("pend-local-{}", crate::line_note::now_ms()),
            file: path.clone(),
            line: abs,
            side: side_s.into(),
            start_line: start,
            start_side: if start > 0 {
                side_s.into()
            } else {
                String::new()
            },
            body: body.clone(),
            ..Default::default()
        };
        let pr_id = self.state.read(cx).selected_pr_id.clone();
        let pc_id = pc.id.clone();
        self.posting = true;
        patch_pr_pending_comments(
            &self.state,
            &pr_id,
            |prev| {
                let mut v = prev.to_vec();
                v.push(pc);
                v
            },
            cx,
        );
        self.draft.update(cx, |a, cx| a.set_value("", window, cx));
        self.open_line = None;
        self.open_start = 0;
        self.close_portal(cx);
        cx.notify();
        let services = self.state.read(cx).services().clone();
        let state = self.state.clone();
        cx.spawn(async move |this, cx| {
            let ok = call(
                cx,
                services
                    .prs
                    .add_pr_comment(&pr_id, &path, signed, &body, start_signed, ""),
                "Comment failed to save",
            )
            .await;
            let _ = this.update(cx, |d, cx| {
                d.posting = false;
                cx.notify();
                if ok.is_none() {
                    patch_pr_pending_comments(
                        &state,
                        &pr_id,
                        |prev| prev.iter().filter(|x| x.id != pc_id).cloned().collect(),
                        cx,
                    );
                    return;
                }
                toast(ToastKind::Success, "Saved — submit your review to post", cx);
            });
        })
        .detach();
    }

    fn remove_pending(&mut self, pc: PendingLineComment, cx: &mut Context<Self>) {
        remove_pending_comment(&self.state, pc, cx);
    }

    fn copy_path(&mut self, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.props.file.path.clone()));
        self.copied_path = true;
        cx.notify();
        self.copy_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1200))
                .await;
            let _ = this.update(cx, |d, cx| {
                d.copied_path = false;
                cx.notify();
            });
        }));
    }

    pub fn toggle_collapsed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.props.collapsible {
            return;
        }
        let next = !self.is_collapsed();
        match self.hooks.toggle_collapse.clone() {
            Some(f) => f(next, window, cx),
            None => {
                self.internal_collapsed = next;
                cx.notify();
            }
        }
    }

    pub fn toggle_hunk(&mut self, i: usize, cx: &mut Context<Self>) {
        if !self.collapsed_hunks.remove(&i) {
            self.collapsed_hunks.insert(i);
        }
        cx.notify();
    }

    pub fn collapsed_hunks(&self) -> &HashSet<usize> {
        &self.collapsed_hunks
    }

    pub fn line_window(&self) -> LineWin {
        self.win
    }

    // ---- range drag over the `+` controls ----

    fn begin_range(&mut self, line_i: usize, side: DiffSide, cx: &mut Context<Self>) {
        if !self.can_note_side(side, self.pr()) {
            return;
        }
        let n = row_num(&self.lines()[line_i], side);
        if n == 0 {
            return;
        }
        self.selecting = Some(Selecting {
            side,
            anchor: n,
            tip: n,
        });
        cx.notify();
    }

    /// `commentLineAt`: the row under the pointer whose `+` column is within 10px.
    fn comment_line_at(&self, x: f32, y: f32, side: DiffSide) -> i64 {
        let g = self.geom.borrow();
        for r in g.rows.iter().filter(|r| r.side == side) {
            let (top, bottom) = (f32::from(r.row.top()), f32::from(r.row.bottom()));
            if y < top || y >= bottom {
                continue;
            }
            let Some(a) = g.addcs.get(&(r.side == DiffSide::Left, r.line)) else {
                return 0;
            };
            let pad = 10.;
            if x < f32::from(a.left()) - pad || x > f32::from(a.right()) + pad {
                return 0;
            }
            return r.line;
        }
        0
    }

    fn move_range(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        let Some(sel) = self.selecting else { return };
        let n = self.comment_line_at(x, y, sel.side);
        if n == 0 || n == sel.tip {
            return;
        }
        self.selecting = Some(Selecting { tip: n, ..sel });
        cx.notify();
    }

    fn end_range(&mut self, fallback_i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sel) = self.selecting.take() else {
            return;
        };
        let start = sel.anchor.min(sel.tip);
        let end = sel.anchor.max(sel.tip);
        let target = find_line(self.lines(), end, sel.side).unwrap_or(fallback_i);
        self.open_composer(
            target,
            sel.side,
            if start < end { start } else { 0 },
            window,
            cx,
        );
        cx.notify();
    }

    // ---- line window and jumps ----

    fn begin_jump(&mut self, target: i64, window: &mut Window, cx: &mut Context<Self>) {
        if self.lines().is_empty() {
            return;
        }
        match self.hooks.toggle_collapse.clone() {
            Some(f) => cx.defer_in(window, move |_, window, cx| f(false, window, cx)),
            None => self.internal_collapsed = false,
        }
        let of = hunk_of(self.lines());
        let open: Vec<usize> = self
            .lines()
            .iter()
            .enumerate()
            .filter(|(i, l)| l.r#type != "hunk" && line_no(l) == target && of[*i] >= 0)
            .map(|(i, _)| of[i] as usize)
            .collect();
        for h in open {
            self.collapsed_hunks.remove(&h);
        }
        self.pending_jump = Some(target);
        cx.notify();
    }

    fn post_threads_on_file(&self) -> Vec<PrComment> {
        self.facts
            .notes
            .iter()
            .filter(|c| is_line_note_root(c) && same_comment_path(&c.path, &self.props.file.path))
            .cloned()
            .collect()
    }

    /// `refreshLineWindow`, each frame from the pane's geometry.
    fn refresh_line_window(&mut self, cx: &mut Context<Self>) {
        if self.is_collapsed() {
            return;
        }
        let count = self.slot_count();
        let Some(pane) = self.pane.clone().filter(|_| count > DIFF_LINE_WINDOW_MIN) else {
            if self.win != LineWin::new(0, count) {
                self.win = LineWin::new(0, count);
                cx.notify();
            }
            return;
        };
        let (root, painted_top, head_h) = {
            let g = self.geom.borrow();
            (g.root, g.painted_top, g.head_h)
        };
        let Some(root) = root else { return };
        let sig = format!(
            "{}:{:?}:{count}:{}",
            self.props.file.path,
            self.props.layout,
            f32::from(root.size.width)
        );
        if sig != self.slot_sig {
            self.slot_sig = sig;
            self.slot_h.clear();
        }
        let mut changed = self.measure();
        let scroll_top = pane.scroll_top();
        let head_h = if head_h > 0. {
            head_h
        } else {
            recur_ui::style::CHROME_H
        };
        let body_top = painted_top + (f32::from(root.origin.y) - pane.top()) + head_h;
        let mut win = line_window(
            count,
            body_top,
            scroll_top,
            pane.client_height(),
            DIFF_LINE_PX,
            DIFF_LINE_OVERSCAN,
            Some(&self.slot_h),
        );
        let hl = self.props.highlight_line;
        let jumping = hl > 0 && self.last_jump != hl;
        let jump_slot = if jumping {
            let threads = self.post_threads_on_file();
            let refs: Vec<&PrComment> = threads.iter().collect();
            if self.props.layout == DiffLayout::Split {
                slot_of_split_highlight(self.lines(), &self.slots_split(), hl, &refs)
            } else {
                slot_of_unified_highlight(self.lines(), &self.slots_unified(), hl, &refs)
            }
        } else {
            -1
        };
        win = jump_line_window(win, count, hl, self.last_jump, jump_slot);
        if !jumping {
            let mut pins = Vec::new();
            if let Some(o) = self.open_line {
                if o.old_n != 0 {
                    pins.push(o.old_n);
                }
                if o.new_n != 0 {
                    pins.push(o.new_n);
                }
            }
            if let Some(s) = self.selecting {
                pins.push(s.anchor);
                pins.push(s.tip);
            }
            for n in pins {
                win = cover_near(win, self.slot_of_line(n), count, DIFF_LINE_OVERSCAN);
            }
        }
        if win != self.win {
            let anchor = (!jumping)
                .then(|| {
                    self.geom
                        .borrow()
                        .slots
                        .get(&self.win.start)
                        .map(|(top, _)| (self.win.start, *top))
                })
                .flatten();
            self.pending_pin = anchor.map(|(slot, y)| (slot, y, scroll_top));
            self.win = win;
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    /// `measure`: the rendered slots' heights into the sparse height list.
    fn measure(&mut self) -> bool {
        let g = self.geom.borrow();
        let mut changed = false;
        for slot in self.win.start..self.win.end {
            let Some(&(_, h)) = g.slots.get(&slot) else {
                continue;
            };
            if self.slot_h.len() <= slot {
                self.slot_h.resize(slot + 1, None);
            }
            if h > 0. && self.slot_h[slot] != Some(h) {
                self.slot_h[slot] = Some(h);
                changed = true;
            }
        }
        changed
    }

    fn on_frame(&mut self, cx: &mut Context<Self>) {
        if let Some((slot, pin_y, captured)) = self.pending_pin.take()
            && let Some(pane) = &self.pane
        {
            let row_top = self.geom.borrow().slots.get(&slot).map(|(t, _)| *t);
            if row_top.is_none() && self.geom.borrow().slots.is_empty() {
                self.pending_pin = Some((slot, pin_y, captured));
            } else if let Some(top) =
                apply_slot_pin(pane.scroll_top(), row_top, Some(pin_y), captured)
            {
                pane.set_scroll_top(top);
                cx.notify();
            }
        }
        if let Some(target) = self.pending_jump {
            let (thread, row) = {
                let g = self.geom.borrow();
                (
                    g.threads.get(&target).copied(),
                    g.lines.get(&target).copied(),
                )
            };
            let el_top = match pick_diff_jump(target, thread.is_some(), row.is_some()) {
                JumpTarget::Thread => thread,
                JumpTarget::Row => row,
                JumpTarget::Root => None,
            };
            if let (Some(top), Some(pane)) = (el_top, &self.pane) {
                let to = pane_scroll_to(
                    pane.scroll_top(),
                    pane.top(),
                    pane.client_height(),
                    top,
                    PANE_SCROLL_BIAS,
                );
                pane.set_scroll_top(to);
                self.last_jump = target;
                self.pending_jump = None;
                cx.notify();
            } else if el_top.is_some() || self.pane.is_none() {
                self.last_jump = target;
                self.pending_jump = None;
            }
        }
        let key = {
            let g = self.geom.borrow();
            let size = g
                .root
                .map(|r| (f32::from(r.size.width), f32::from(r.size.height)));
            let mut hunks: Vec<&usize> = self.collapsed_hunks.iter().collect();
            hunks.sort();
            format!(
                "{:?}|{size:?}|{}|{:?}|{hunks:?}|{:?}|{}|{:?}|{}|{}|{}",
                self.pane.as_ref().map(Pane::scroll_top),
                self.slot_count(),
                self.props.layout,
                self.open_line,
                self.open_start,
                self.selecting,
                self.props.highlight_line,
                self.last_jump,
                self.is_collapsed(),
            )
        };
        if key != self.line_window_key {
            self.line_window_key = key;
            self.refresh_line_window(cx);
        }
    }
}

// ---- rendering ----

const GUTTER: f32 = 46.;
const CODE_PX: f32 = 12.;
const CODE_LH: f32 = 1.6;

impl DiffFile {
    /// The `.cd` cell's text; an empty cell has no line box.
    /// A row cell's key in the review's selection scope: the file, the line and the cell.
    fn sel_key(&self, i: usize, cell: u8) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (&self.props.file.path, i, cell).hash(&mut h);
        h.finish()
    }

    fn code_cell(&self, i: usize, id: SharedString, sel: u64) -> Option<Text> {
        (!self.lines()[i].code_html.is_empty()).then(|| self.code_text(i, id).selectable(sel, "\n"))
    }

    /// The code in the editor theme: highlighted runs in their colors, any other line in the
    /// theme's foreground.
    fn code_text(&self, i: usize, id: SharedString) -> Text {
        use recur_textmate::tm_font::{BOLD, ITALIC};
        let fg = self.ed.foreground;
        let html = &self.lines()[i].code_html;
        let mut col = 0usize;
        let spans: Vec<Span> = match self.hl.get(i) {
            Some(Some((src, runs))) if src == html => runs
                .iter()
                .map(|r| {
                    let mut s = span(expand_tabs(&r.text, &mut col)).color(r.color.unwrap_or(fg));
                    if r.font_style & BOLD != 0 {
                        s = s.weight(700.);
                    }
                    if r.font_style & ITALIC != 0 {
                        s = s.italic();
                    }
                    s
                })
                .collect(),
            _ => vec![span(expand_tabs(&cell_text(html), &mut col))],
        };
        Text::from_spans(spans)
            .id(id)
            .family(Family::Mono)
            .size(CODE_PX)
            .line_height(LineHeight::Scale(CODE_LH))
            .color(fg)
            .pre_wrap()
            .break_anywhere()
    }

    /// A line number in the editor's gutter colors, with its git gutter bar (`mark`).
    #[allow(clippy::too_many_arguments)]
    fn ln_cell(
        &self,
        s: &str,
        open_n: i64,
        drafting: bool,
        mark: Option<Rgba>,
        key: &str,
        sel: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let ed = &self.ed;
        let (color, bg): (Hsla, Hsla) = if drafting {
            (t.cyan.into(), t.cyan.fade(0.12).into())
        } else {
            (ed.gutter_foreground.into(), ed.gutter_background.into())
        };
        let open = self.hooks.open_file.clone();
        let path = self.props.file.path.clone();
        div()
            .id(SharedString::from(format!("ln-{key}")))
            .relative()
            .flex_none()
            .w(px(GUTTER))
            .px(px(8.))
            .border_r_1()
            .border_color(ed.gutter_border)
            .bg(bg)
            .when_some(mark, |d, c| {
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right_0()
                        .w(px(2.))
                        .bg(c),
                )
            })
            .child(
                label(
                    s.to_string(),
                    Family::Mono,
                    10.5,
                    LineHeight::Scale(CODE_LH),
                )
                .color(color)
                .align(Align::Right)
                // `.hline .ln` is `user-select: none`, but WebKit still copies its text.
                .selectable(sel, "\n"),
            )
            .on_click(move |ev, window, cx| {
                let Some(open) = open.clone() else { return };
                if open_n == 0 {
                    return;
                }
                let m = ev.modifiers();
                if !(m.platform || m.control) {
                    return;
                }
                cx.stop_propagation();
                open(path.clone(), Some(open_n), window, cx);
            })
            .into_any_element()
    }

    /// `.addc`: the `+` in the gutter, opacity 0 until its row (`group`) is hovered.
    #[allow(clippy::too_many_arguments)]
    fn addc(
        &self,
        line_i: usize,
        side: DiffSide,
        left: f32,
        group: SharedString,
        tip: bool,
        key: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let hline = CODE_PX * CODE_LH;
        let geom = self.geom.clone();
        let line = row_num(&self.lines()[line_i], side);
        let probe = canvas(
            move |b, _, _| {
                geom.borrow_mut()
                    .addcs
                    .insert((side == DiffSide::Left, line), b);
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        let focus = self
            .addc_focus
            .borrow_mut()
            .entry((side == DiffSide::Left, line))
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let square = div()
            .flex()
            .items_center()
            .justify_center()
            .w(px(16.))
            .h(px(16.))
            .rounded(px(4.))
            .bg(hsla(t.amber))
            .text_color(hsla(t.on_amber))
            .child(label("+", Family::Ui, 13., LineHeight::Px(13.)));
        let hit = div()
            .id(SharedString::from(format!("addc-{key}")))
            .debug_selector({
                let p = self.props.file.path.clone();
                move || format!("addc:{p}:{}:{line}", side_key(side))
            })
            .track_focus(&focus)
            .absolute()
            .top_0()
            .left(px(left))
            .w(px(16.))
            .flex()
            .flex_col()
            .cursor_pointer()
            .when(!tip, |d| {
                d.opacity(0.).group_hover(group.clone(), |s| s.opacity(1.))
            })
            .child(div().flex_none().h(px((hline - 16.) / 2.)))
            .child(square)
            .child(div().flex_none().h(px((hline - 16.) / 2.)))
            .child(probe)
            .on_mouse_down(
                MouseButton::Left,
                cx.guarded_listener(move |d, _, _, cx| {
                    cx.stop_propagation();
                    d.begin_range(line_i, side, cx);
                }),
            )
            // A quick click can land before the window-level release listener is painted.
            .on_mouse_up(
                MouseButton::Left,
                cx.guarded_listener(move |d, _, window, cx| d.end_range(line_i, window, cx)),
            )
            .on_click(cx.guarded_listener(move |d, e: &ClickEvent, window, cx| {
                if matches!(e, ClickEvent::Keyboard(_)) {
                    d.open_composer(line_i, side, 0, window, cx);
                }
            }));
        titled(
            SharedString::from(format!("addc-t-{key}")),
            "Comment on this line — drag to select a range",
            hit,
        )
        .into_any_element()
    }

    /// The git gutter bar of an added or deleted line.
    fn mark(&self, kind: &str) -> Option<Rgba> {
        match kind {
            "add" => Some(self.ed.gutter_added),
            "del" => Some(self.ed.gutter_deleted),
            _ => None,
        }
    }

    fn row_bg(&self, kind: &str, f: RowFlags, cx: &App) -> Option<Color> {
        let t = Theme::of(cx).tokens;
        if f.drafting {
            return Some(t.cyan.fade(0.16));
        }
        if f.noted && f.note_end {
            return Some(t.warn.fade(0.16));
        }
        match kind {
            "add" => Some(self.ed.merge().changed_line.into()),
            "del" => Some(self.ed.merge().deleted_chunk.into()),
            _ if f.noted => Some(t.cyan.fade(0.1)),
            _ => None,
        }
    }

    /// The left cyan bar of `.noted` (`box-shadow: inset 3px 0 0`) and the draft tick
    /// (`::before` on the `+` column).
    fn row_marks(f: RowFlags, addc_left: f32, cx: &App) -> Vec<AnyElement> {
        let t = Theme::of(cx).tokens;
        let mut v = Vec::new();
        if f.noted {
            v.push(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left_0()
                    .w(px(3.))
                    .bg(hsla(t.cyan))
                    .into_any_element(),
            );
        }
        if f.drafting {
            v.push(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(addc_left + (16. - 3.) / 2.))
                    .w(px(3.))
                    .bg(hsla(t.cyan))
                    .into_any_element(),
            );
        }
        v
    }

    fn record_row(&self, side: DiffSide, line: i64) -> impl IntoElement {
        let geom = self.geom.clone();
        canvas(
            move |b, _, _| {
                if line != 0 {
                    geom.borrow_mut().rows.push(RowHit { side, line, row: b });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }

    fn record_line(&self, line: i64) -> impl IntoElement {
        let geom = self.geom.clone();
        canvas(
            move |b, _, _| {
                if line != 0 {
                    geom.borrow_mut()
                        .lines
                        .entry(line)
                        .or_insert(f32::from(b.origin.y));
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }

    #[allow(clippy::too_many_arguments)]
    fn unified_row(
        &self,
        i: usize,
        slot: usize,
        pr: Option<&Pr>,
        pending: &[PendingLineComment],
        notes: &[PrComment],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let l = self.lines()[i].clone();
        let side = row_side(&l);
        let n = row_num(&l, side);
        let flags = self.row_flags(i, None, pending, notes);
        let group = SharedString::from(format!("hl-{}-{slot}", self.props.file.path));
        let key = format!("{slot}");
        let mut row = div()
            .group(group.clone())
            .debug_selector(|| {
                format!(
                    "hline:{}:{}:{}:{}",
                    self.props.file.path,
                    l.r#type,
                    side_key(side),
                    n
                )
            })
            .relative()
            .w_full()
            .flex()
            .when_some(self.row_bg(&l.r#type, flags, cx), |d, c| d.bg(hsla(c)))
            .children(Self::row_marks(flags, 80., cx))
            .child(self.ln_cell(
                &l.old,
                open_line_no(&l),
                flags.drafting,
                None,
                &format!("o{key}"),
                self.sel_key(i, 0),
                cx,
            ))
            .child(self.ln_cell(
                &l.new,
                open_line_no(&l),
                flags.drafting,
                self.mark(&l.r#type),
                &format!("n{key}"),
                self.sel_key(i, 1),
                cx,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .px(px(14.))
                    .children(self.code_cell(
                        i,
                        SharedString::from(format!("cd-{key}")),
                        self.sel_key(i, 2),
                    )),
            );
        if self.can_note_side(side, pr) {
            row = row.child(self.addc(
                i,
                side,
                80.,
                group,
                self.selecting.is_some() && flags.draft_tip,
                &key,
                cx,
            ));
        }
        row = row
            .child(self.record_row(side, n))
            .child(self.record_line(line_no(&l)));
        let _ = flags.draft_end;
        row.into_any_element()
    }

    fn hunk_row(
        &self,
        i: usize,
        slot: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let html = hunk_label(&self.lines()[i].code_html);
        let text_runs = cell_text(&html).to_string();
        let caret = if self.collapsed_hunks.contains(&i) {
            "▸"
        } else {
            "▾"
        };
        let _ = window;
        div()
            .id(SharedString::from(format!("hunk-{slot}")))
            .debug_selector({
                let p = self.props.file.path.clone();
                move || format!("hunk:{p}:{i}")
            })
            .w_full()
            .flex()
            .items_center()
            .py(px(2.))
            .bg(hsla(HUNK_BG))
            // `.hline.hunk:hover`.
            .hover(|st| st.bg(hsla(HUNK_HOVER_BG)))
            .cursor_pointer()
            .child(
                div().flex_none().w(px(22.)).flex().justify_center().child(
                    label(caret, Family::Mono, 11., LineHeight::Scale(CODE_LH)).color(t.indigo),
                ),
            )
            .child(
                div().flex_1().min_w_0().pl(px(4.)).pr(px(10.)).child(
                    text(text_runs)
                        .family(Family::Mono)
                        .size(CODE_PX)
                        .line_height(LineHeight::Scale(CODE_LH))
                        .color(t.indigo)
                        .truncate()
                        .selectable(self.sel_key(i, 7), "\n"),
                ),
            )
            .on_click(cx.guarded_listener(move |d, _, _, cx| d.toggle_hunk(i, cx)))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn split_side(
        &self,
        cell: Option<usize>,
        side: DiffSide,
        slot: usize,
        pr: Option<&Pr>,
        pending: &[PendingLineComment],
        notes: &[PrComment],
        group: SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let key = format!("{slot}{}", side_key(side));
        let Some(i) = cell else {
            return div()
                .flex_1()
                .min_w_0()
                .flex()
                .bg(hsla(BLANK_BG))
                .when(side == DiffSide::Left, |d| {
                    d.border_r_1().border_color(hsla(t.line_soft))
                })
                .child(
                    div()
                        .flex_none()
                        .w(px(GUTTER))
                        .border_r_1()
                        .border_color(self.ed.gutter_border)
                        .bg(self.ed.gutter_background),
                )
                .child(div().flex_1())
                .into_any_element();
        };
        let l = self.lines()[i].clone();
        let n = row_num(&l, side);
        let right = side == DiffSide::Right;
        let flags = self.row_flags(i, Some(side), pending, notes);
        let side_group = SharedString::from(format!("{group}-{}", side_key(side)));
        let num = if right { l.new.clone() } else { l.old.clone() };
        let mut d = div()
            .group(side_group.clone())
            .debug_selector(|| {
                format!(
                    "side:{}:{}:{}:{}",
                    self.props.file.path,
                    l.r#type,
                    side_key(side),
                    n
                )
            })
            .relative()
            .flex_1()
            .min_w_0()
            .flex()
            .when(side == DiffSide::Left, |d| {
                d.border_r_1().border_color(hsla(t.line_soft))
            })
            .when_some(self.row_bg(&l.r#type, flags, cx), |d, c| d.bg(hsla(c)))
            .children(Self::row_marks(flags, 38., cx))
            .child(self.ln_cell(
                &num,
                open_line_no(&l),
                flags.drafting,
                self.mark(&l.r#type),
                &key,
                self.sel_key(i, if right { 5 } else { 3 }),
                cx,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .px(px(14.))
                    .children(self.code_cell(
                        i,
                        SharedString::from(format!("cd-{key}")),
                        self.sel_key(i, if right { 6 } else { 4 }),
                    )),
            );
        if self.can_note_side(side, pr) {
            d = d.child(self.addc(
                i,
                side,
                38.,
                side_group,
                self.selecting.is_some() && flags.draft_tip,
                &key,
                cx,
            ));
        }
        d.child(self.record_row(side, n)).into_any_element()
    }

    /// The composer's card. `live` with the text box; without, an empty stand-in of the same
    /// size (a card whose text box a host draws, see [`crate::note_portal`]).
    fn composer_card(
        &self,
        line_i: usize,
        split: bool,
        live: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let t = Theme::of(cx).tokens;
        let mode = self.mode();
        let empty = !live || js::trim(&self.draft_text(cx)).is_empty();
        let posting = self.posting;
        let line = 12.5 * 1.5;
        let lh = recur_ui::style::button_label("x", 11., 400.)
            .measure(None, window)
            .2
            .height;
        let input = div()
            .id("df-compose-input")
            .debug_selector(|| "finding-note-input".into())
            .min_h(px(40.))
            .max_h(px(120.))
            .h(px(2. * line + 12.))
            .pt(px(8.))
            .px(px(10.))
            .pb(px(4.))
            .text_size(px(12.5))
            .line_height(px(line))
            .text_color(hsla(t.text))
            .when(posting, |d| d.opacity(0.55))
            .when(live, |d| {
                d.key_context("TextEntry")
                    .on_key_down(
                        cx.guarded_listener(move |d, ev: &KeyDownEvent, window, cx| {
                            d.composer_key(line_i, ev, window, cx)
                        }),
                    )
                    .child(Textarea::new(&self.draft))
            });
        let hint_text = if mode == NoteMode::Finding {
            "saves a finding on this line"
        } else {
            "pending until you submit the review"
        };
        let backdrop = t.bg;
        let foot = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .pt(px(2.))
            .px(px(8.))
            .pb(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .text_color(hsla(t.faint))
                    .child(recur_ui::kbd::kbd("⌘↵").size(11.).color(t.faint))
                    .child(text(hint_text).size(11.).nowrap()),
            )
            .child(
                div()
                    .ml_auto()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        note_btn(
                            "df-compose-cancel",
                            "Cancel",
                            false,
                            posting,
                            backdrop,
                            lh,
                            cx,
                        )
                        .debug_selector(|| "compose-cancel".into())
                        .when(!posting && live, |b| {
                            b.on_click(cx.guarded_listener(|d, _, window, cx| {
                                d.dismiss_composer(window, cx)
                            }))
                        }),
                    )
                    .child(
                        note_btn(
                            "df-compose-add",
                            if posting { "Saving…" } else { "Add" },
                            true,
                            posting || empty,
                            backdrop,
                            lh,
                            cx,
                        )
                        .debug_selector(|| "compose-add".into())
                        .when(!posting && !empty, |b| {
                            b.on_click(cx.guarded_listener(move |d, _, window, cx| {
                                d.submit(line_i, window, cx)
                            }))
                        }),
                    ),
            );
        recur_ui::style::card(t.bg, t.line, 10.)
            .flex()
            .flex_col()
            .overflow_hidden()
            .when(!split, |d| d.max_w(px(640.)))
            .child(input)
            .child(foot)
    }

    fn composer_box(
        &self,
        line_i: usize,
        split: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        // With a pane the host draws the card at this place (a key then redraws the card, not
        // the stack); this one lays out invisibly at the same size and records where it is
        // and what clips it each time the stack draws.
        let card = match (&self.draft_portal, &self.pane) {
            (Some((slot, _)), Some(pane)) => {
                let (slot, draws) = (slot.clone(), pane.clone());
                self.composer_card(line_i, split, false, window, cx)
                    .relative()
                    .invisible()
                    .child(
                        canvas(
                            move |b, window, _| {
                                slot.bounds.set(Some(b));
                                slot.clip.set(Some(window.content_mask().bounds));
                                slot.drawn.set(draws.draws());
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
            }
            _ => self.composer_card(line_i, split, true, window, cx),
        };
        div()
            .debug_selector(|| "icmt-compose".into())
            .bg(hsla(t.pane))
            .border_t_1()
            .border_b_1()
            .border_color(hsla(t.line))
            .when(!split, |d| {
                d.pt(px(10.)).pr(px(14.)).pb(px(12.)).pl(px(60.))
            })
            .when(split, |d| d.pt(px(10.)).pr(px(12.)).pb(px(12.)).pl(px(14.)))
            .child(card)
            .into_any_element()
    }

    fn pending_card(
        &self,
        pc: &PendingLineComment,
        split: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let pc2 = pc.clone();
        let group = SharedString::from(format!("pend-{}", pc.id));
        let _ = window;
        let del = div()
            .id(SharedString::from(format!("pend-del-{}", pc.id)))
            .debug_selector({
                let id = pc.id.clone();
                move || format!("pending-remove:{id}")
            })
            .ml_auto()
            .flex()
            .items_center()
            .justify_center()
            .w(px(22.))
            .h(px(22.))
            .rounded(px(5.))
            .cursor_pointer()
            .text_color(hsla(t.faint))
            .opacity(0.)
            .group_hover(group.clone(), |s| s.opacity(1.))
            .hover(|s| s.text_color(hsla(t.coral)).bg(hsla(t.pane.fade(0.7))))
            .child(icon(IconName::Trash).size(px(13.)))
            .on_click(cx.guarded_listener(move |d, _, _, cx| d.remove_pending(pc2.clone(), cx)));
        let card = recur_ui::style::card(t.bg, t.amber.mix(0.45, t.line), 10.)
            .group(group)
            .pt(px(8.))
            .px(px(10.))
            .pb(px(10.))
            .when(!split, |d| d.max_w(px(640.)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .mb(px(6.))
                    .child(
                        div()
                            .rounded(px(999.))
                            .bg(hsla(PENDING_BG))
                            .px(px(7.))
                            .py(px(2.))
                            .child(
                                label("Pending", Family::Mono, 10., LineHeight::Scale(1.45))
                                    .tracking(0.06)
                                    .uppercase()
                                    .color(t.amber),
                            ),
                    )
                    .child(titled(
                        SharedString::from(format!("pend-del-t-{}", pc.id)),
                        "Remove pending comment",
                        del,
                    )),
            )
            .child(
                Text::from_spans(vec![span(pc.body.clone())])
                    .id(SharedString::from(format!("pend-body-{}", pc.id)))
                    .size(12.5)
                    .line_height(LineHeight::Scale(1.5))
                    .color(t.text)
                    .pre_wrap(),
            );
        div()
            .debug_selector({
                let id = pc.id.clone();
                move || format!("icmt-pending:{id}")
            })
            .bg(hsla(t.pane))
            .border_t_1()
            .border_b_1()
            .border_color(hsla(t.line))
            .when(!split, |d| d.pt(px(10.)).px(px(14.)).pb(px(12.)))
            .when(split, |d| d.pt(px(10.)).pr(px(12.)).pb(px(12.)).pl(px(14.)))
            .child(card)
            .into_any_element()
    }

    fn finding_card(
        &self,
        f: &Finding,
        split: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let sev = if f.severity.is_empty() {
            "info".to_string()
        } else {
            f.severity.as_str().to_string()
        };
        let sev_color = match sev.as_str() {
            "medium" => t.amber,
            "high" | "critical" => t.coral,
            _ => t.muted,
        };
        let lh = recur_ui::style::button_label("x", 11., 400.)
            .measure(None, window)
            .2
            .height;
        let mut meta = div()
            .flex()
            .gap(px(8.))
            .mt(px(3.))
            .text_color(hsla(t.faint));
        if f.end_line > f.line {
            meta = meta.child(label(
                format!("lines {}–{}", f.line, f.end_line),
                Family::Mono,
                10.5,
                LineHeight::Scale(1.45),
            ));
        }
        if !f.notes.is_empty() {
            let n = f.notes.len();
            meta = meta.child(label(
                format!("{n} note{}", if n == 1 { "" } else { "s" }),
                Family::Mono,
                10.5,
                LineHeight::Scale(1.45),
            ));
        }
        let mut card = recur_ui::style::card(t.bg, t.line, 8.)
            .flex()
            .items_start()
            .gap(px(8.))
            .py(px(8.))
            .px(px(10.))
            .when(!split, |d| d.max_w(px(640.)))
            .child(
                div()
                    .flex_none()
                    .w(px(14.))
                    .h(px(14.))
                    .text_color(hsla(sev_color))
                    .child(icon(finding_sev_icon(&sev)).size(px(14.))),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .child(
                        Text::from_spans(vec![span(f.message.clone())])
                            .id(SharedString::from(format!("fmsg-{}", f.id)))
                            .size(12.5)
                            .line_height(LineHeight::Scale(1.4))
                            .color(t.text),
                    )
                    .child(meta),
            );
        let post = self.hooks.post_finding.clone();
        let open = self.hooks.open_finding.clone();
        if post.is_some() || open.is_some() {
            let mut actions = div()
                .flex_none()
                .ml_auto()
                .flex()
                .flex_col()
                .gap(px(4.))
                .items_end();
            if let Some(post) = post.filter(|_| self.props.postable_finding_ids.contains(&f.id)) {
                let busy = self.props.posting_finding_ids.contains(&f.id);
                let f2 = f.clone();
                actions = actions.child(titled(
                    SharedString::from(format!("fpost-t-{}", f.id)),
                    "Queue as a GitHub review comment",
                    note_btn(
                        SharedString::from(format!("fpost-{}", f.id)),
                        "Post as comment",
                        false,
                        busy,
                        t.bg,
                        lh,
                        cx,
                    )
                    .when(!busy, |b| {
                        b.on_click(move |_, window, cx| post(f2.clone(), window, cx))
                    }),
                ));
            }
            if let Some(open) = open {
                let f2 = f.clone();
                actions = actions.child(
                    note_btn(
                        SharedString::from(format!("fopen-{}", f.id)),
                        "Open in Findings",
                        false,
                        false,
                        t.bg,
                        lh,
                        cx,
                    )
                    .debug_selector({
                        let id = f.id.clone();
                        move || format!("open-in-findings:{id}")
                    })
                    .on_click(move |_, window, cx| open(f2.clone(), window, cx)),
                );
            }
            card = card.child(actions);
        }
        div()
            .debug_selector(|| format!("icmt-finding:{}", f.id))
            .bg(hsla(t.pane))
            .border_t_1()
            .border_b_1()
            .border_color(hsla(t.line))
            .when(!split, |d| d.pt(px(8.)).px(px(14.)).pb(px(10.)))
            .when(split, |d| d.pt(px(8.)).px(px(14.)).pb(px(10.)))
            .child(card)
            .into_any_element()
    }

    fn thread_block(
        &mut self,
        note: &PrComment,
        notes: &[PrComment],
        pr: Option<&Pr>,
        split: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        self.threads_seen.insert(note.id.clone());
        let entity = match self.threads.get(&note.id) {
            Some(e) => {
                e.update(cx, |th, cx| {
                    th.set(note.clone(), replies_to(notes, note), pr.cloned(), cx)
                });
                e.clone()
            }
            None => {
                let state = self.state.clone();
                let (n, items, p) = (note.clone(), replies_to(notes, note), pr.cloned());
                let e = cx.new(|cx| LineNoteThread::new(state, n, items, p, window, cx));
                self.threads.insert(note.id.clone(), e.clone());
                e
            }
        };
        let clip = self.pane.as_ref().map(Pane::bounds_cell);
        entity.update(cx, |th, _| th.set_clip(clip));
        let geom = self.geom.clone();
        let line = note.line;
        let probe = canvas(
            move |b, _, _| {
                geom.borrow_mut()
                    .threads
                    .entry(line)
                    .or_insert(f32::from(b.origin.y));
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        div()
            .debug_selector(|| format!("icmt-thread:{}:{}", self.props.file.path, note.line))
            .relative()
            .bg(hsla(t.pane))
            .border_t_1()
            .border_b_1()
            .border_color(hsla(t.line))
            .when(!split, |d| d.pt(px(10.)).px(px(14.)).pb(px(12.)))
            .when(split, |d| d.pt(px(10.)).pr(px(12.)).pb(px(12.)).pl(px(14.)))
            .child(div().when(!split, |d| d.max_w(px(640.))).child(entity))
            .child(probe)
            .into_any_element()
    }

    fn extra(&self, l: &DiffLine, key: &str, window: &mut Window, cx: &App) -> Option<AnyElement> {
        let html = if self.props.findings.is_empty() {
            l.extra_html.clone()
        } else {
            extra_html_without_finding_flags(&l.extra_html)
        };
        (!html.is_empty()).then(|| crate::extra::extra_html(&html, key, window, cx))
    }

    /// `commentBlock`: extra markup, then findings, threads, pending cards and the composer.
    #[allow(clippy::too_many_arguments)]
    fn comment_block(
        &mut self,
        i: usize,
        show: bool,
        pr: Option<&Pr>,
        pending: &[PendingLineComment],
        notes: &[PrComment],
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let l = self.lines()[i].clone();
        let mut out = Vec::new();
        if let Some(x) = self.extra(&l, key, window, cx) {
            out.push(x);
        }
        if !show {
            return out;
        }
        let findings: Vec<Finding> = findings_on_line(&self.props.findings, &l)
            .into_iter()
            .cloned()
            .collect();
        for f in &findings {
            out.push(self.finding_card(f, false, window, cx));
        }
        let mode = self.mode();
        if mode == NoteMode::None {
            return out;
        }
        if mode == NoteMode::Pr {
            let on: Vec<PrComment> = line_notes_on_row(notes, &self.props.file.path, &l, None)
                .into_iter()
                .cloned()
                .collect();
            for n in &on {
                out.push(self.thread_block(n, notes, pr, false, window, cx));
            }
            let p: Vec<PendingLineComment> =
                pending_on(pending, &l, None).into_iter().cloned().collect();
            for pc in &p {
                out.push(self.pending_card(pc, false, window, cx));
            }
        }
        if self.can_comment(pr) && composer_on(self.open_line, self.open_side, Some(&l), None) {
            out.push(self.composer_box(i, false, window, cx));
        }
        out
    }

    fn slot_wrapper(&self, slot: usize, children: Vec<AnyElement>) -> AnyElement {
        let geom = self.geom.clone();
        let probe = canvas(
            move |b, _, _| {
                geom.borrow_mut()
                    .slots
                    .insert(slot, (f32::from(b.origin.y), f32::from(b.size.height)));
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        div()
            .relative()
            .w_full()
            .flex()
            .flex_col()
            .children(children)
            .child(probe)
            .into_any_element()
    }

    fn head(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let collapsed = self.is_collapsed();
        let label_s = path_label(&self.props.file);
        let open = self.hooks.open_file.clone();
        let title = self.props.open_file_title.clone();
        let path_text = text(label_s.clone())
            .family(Family::Mono)
            .size(11.5)
            .line_height(LineHeight::Scale(1.45))
            .color(t.text)
            .truncate();
        let mut head = div()
            .id("df-head")
            .debug_selector({
                let p = self.props.file.path.clone();
                move || {
                    format!(
                        "df-head:{p}:{}",
                        if collapsed { "collapsed" } else { "open" }
                    )
                }
            })
            .relative()
            .flex()
            .items_center()
            .gap(px(10.))
            .h(px(recur_ui::style::CHROME_H))
            .px(px(13.))
            .flex_none()
            .bg(hsla(t.pane))
            .border_b_1()
            .border_color(hsla(t.line))
            .when(collapsed && !self.head_stuck, |d| d.rounded(px(8.)))
            .when(collapsed && self.head_stuck, |d| d.rounded_b(px(8.)))
            .when(!collapsed && !self.head_stuck, |d| d.rounded_t(px(8.)))
            .font_family(Family::Mono.primary())
            .text_size(px(11.5));
        if self.props.collapsible {
            head = head
                .cursor_pointer()
                .hover(|s| s.bg(hsla(t.pane_2)))
                .on_click(cx.guarded_listener(|d, _, window, cx| d.toggle_collapsed(window, cx)))
                .child(
                    div()
                        .debug_selector({
                            let p = self.props.file.path.clone();
                            move || format!("df-chev:{p}")
                        })
                        .flex_none()
                        .child(
                            icon(IconName::ChevronDown)
                                .size(px(12.))
                                .stroke_width(2.4)
                                .color(hsla(t.faint))
                                .rotate(if collapsed { -90. } else { 0. }),
                        ),
                );
        }
        let path_el = match open.clone() {
            Some(open) => {
                let path = self.props.file.path.clone();
                titled(
                    "df-path-t",
                    title.clone(),
                    div()
                        .id("df-path")
                        .debug_selector(|| "df-path".into())
                        .flex_initial()
                        .min_w_0()
                        .overflow_hidden()
                        .cursor_pointer()
                        .child(path_text)
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            open(path.clone(), None, window, cx)
                        }),
                )
                .into_any_element()
            }
            None => div()
                .flex_initial()
                .min_w_0()
                .overflow_hidden()
                .child(path_text)
                .into_any_element(),
        };
        head = head.child(div().flex_initial().min_w_0().child(path_el));
        let mut actions = div().flex().flex_none().items_center().gap(px(2.));
        if let Some(open) = open {
            let path = self.props.file.path.clone();
            actions = actions.child(titled(
                "df-open-t",
                title,
                icon_btn("df-open", IconName::Code, 24., 14., None, cx)
                    .debug_selector({
                        let p = self.props.file.path.clone();
                        move || format!("df-open:{p}")
                    })
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        open(path.clone(), None, window, cx)
                    }),
            ));
        }
        let copied = self.copied_path;
        actions = actions.child(titled(
            "df-copy-t",
            if copied { "Copied" } else { "Copy path" },
            icon_btn(
                "df-copy",
                if copied {
                    IconName::Check
                } else {
                    IconName::Copy
                },
                24.,
                14.,
                copied.then_some(t.emerald),
                cx,
            )
            .on_click(cx.guarded_listener(|d, _, _, cx| {
                cx.stop_propagation();
                d.copy_path(cx)
            })),
        ));
        head = head.child(actions);
        head = head.child(div().ml_auto().child(crate::style::add_del(
            self.props.file.diff.add,
            self.props.file.diff.del,
            Family::Mono,
            10.5,
            8.,
            LineHeight::Scale(1.45),
            cx,
        )));
        let geom = self.geom.clone();
        let probe = canvas(
            move |b, _, _| geom.borrow_mut().head_h = f32::from(b.size.height),
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        let _ = window;
        head.child(probe).into_any_element()
    }
}

/// `tab-size: 8` in a monospaced cell: each tab runs to the next multiple of eight columns.
pub fn expand_tabs(s: &str, col: &mut usize) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\t' => {
                let n = 8 - *col % 8;
                out.extend(std::iter::repeat_n(' ', n));
                *col += n;
            }
            '\n' => {
                out.push(ch);
                *col = 0;
            }
            _ => {
                out.push(ch);
                *col += 1;
            }
        }
    }
    out
}

fn side_key(side: DiffSide) -> &'static str {
    match side {
        DiffSide::Left => "left",
        DiffSide::Right => "right",
    }
}

impl DiffFile {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        self.sync_editor_theme(cx);
        {
            let mut g = self.geom.borrow_mut();
            g.slots.clear();
            g.rows.clear();
            g.threads.clear();
            g.lines.clear();
            g.addcs.clear();
        }
        self.threads_seen.clear();
        let facts = self.facts.clone();
        let pr = facts.head.as_deref();
        let pending = &facts.pending;
        let notes = &facts.notes;
        let collapsed = self.is_collapsed();
        let mut root = div()
            .id(SharedString::from(format!(
                "diff-file-{}",
                self.props.file.path
            )))
            .debug_selector(|| format!("diff-file:{}", self.props.file.path))
            .relative()
            .w_full()
            .flex()
            .flex_col();
        // In a pane the header floats over a placeholder, placed from each frame's scroll.
        let float_head = self.pane.clone().map(|pane| (pane, self.head(window, cx)));
        root = if float_head.is_some() {
            root.child(
                div()
                    .flex_none()
                    .h(px(self.geom.borrow().head_h.max(recur_ui::style::CHROME_H))),
            )
        } else {
            root.child(self.head(window, cx))
        };
        if !collapsed && self.props.image.is_some() {
            root = root.child(self.image_pair(window, cx));
        } else if !collapsed {
            let mut slots_el: Vec<AnyElement> = Vec::new();
            let split = self.props.layout == DiffLayout::Split;
            let count = self.slot_count();
            let a = self.win.start.min(count);
            let b = self.win.end.min(count).max(a);
            if split {
                let slots = self.slots_split();
                let lines = self.lines().to_vec();
                for (s, row) in slots[a..b].iter().enumerate() {
                    let slot = a + s;
                    let mut kids: Vec<AnyElement> = Vec::new();
                    match *row {
                        SplitRow::Hunk { i } => kids.push(self.hunk_row(i, slot, window, cx)),
                        SplitRow::Pair { left, right, .. } => {
                            let group =
                                SharedString::from(format!("hl-{}-{slot}", self.props.file.path));
                            let data_line = split_row_data_line(&lines, row);
                            let l_el = self.split_side(
                                left.map(|c| c.i),
                                DiffSide::Left,
                                slot,
                                pr,
                                pending,
                                notes,
                                group.clone(),
                                cx,
                            );
                            let r_el = self.split_side(
                                right.map(|c| c.i),
                                DiffSide::Right,
                                slot,
                                pr,
                                pending,
                                notes,
                                group.clone(),
                                cx,
                            );
                            kids.push(
                                div()
                                    .group(group)
                                    .relative()
                                    .w_full()
                                    .flex()
                                    .font_family(Family::Mono.primary())
                                    .child(recur_ui::style::flex_share(1., l_el).min_w_0())
                                    .child(recur_ui::style::flex_share(1., r_el).min_w_0())
                                    .child(self.record_line(data_line))
                                    .into_any_element(),
                            );
                            if let Some(c) = left {
                                kids.extend(self.comment_block(
                                    c.i,
                                    false,
                                    pr,
                                    pending,
                                    notes,
                                    &format!("{slot}l"),
                                    window,
                                    cx,
                                ));
                            }
                            if let Some(c) = right
                                && left.is_none_or(|lc| lc.i != c.i)
                            {
                                kids.extend(self.comment_block(
                                    c.i,
                                    false,
                                    pr,
                                    pending,
                                    notes,
                                    &format!("{slot}r"),
                                    window,
                                    cx,
                                ));
                            }
                            let mode = self.mode();
                            let ll = left.map(|c| lines[c.i].clone());
                            let rl = right.map(|c| lines[c.i].clone());
                            let path = self.props.file.path.clone();
                            let comp_l = composer_on(
                                self.open_line,
                                self.open_side,
                                ll.as_ref(),
                                Some(DiffSide::Left),
                            );
                            let comp_r = composer_on(
                                self.open_line,
                                self.open_side,
                                rl.as_ref(),
                                Some(DiffSide::Right),
                            );
                            let pend_l = ll.as_ref().map_or(Vec::new(), |l| {
                                pending_on(pending, l, Some(DiffSide::Left))
                                    .into_iter()
                                    .cloned()
                                    .collect::<Vec<_>>()
                            });
                            let pend_r = rl.as_ref().map_or(Vec::new(), |l| {
                                pending_on(pending, l, Some(DiffSide::Right))
                                    .into_iter()
                                    .cloned()
                                    .collect::<Vec<_>>()
                            });
                            let notes_l: Vec<PrComment> = ll.as_ref().map_or(Vec::new(), |l| {
                                line_notes_on_row(notes, &path, l, Some(DiffSide::Left))
                                    .into_iter()
                                    .cloned()
                                    .collect()
                            });
                            let notes_r: Vec<PrComment> = rl.as_ref().map_or(Vec::new(), |l| {
                                line_notes_on_row(notes, &path, l, Some(DiffSide::Right))
                                    .into_iter()
                                    .cloned()
                                    .collect()
                            });
                            let finds_r: Vec<Finding> = rl.as_ref().map_or(Vec::new(), |l| {
                                findings_on_line(&self.props.findings, l)
                                    .into_iter()
                                    .cloned()
                                    .collect()
                            });
                            let pr_mode = mode == NoteMode::Pr;
                            let show = mode != NoteMode::None
                                && (comp_l
                                    || comp_r
                                    || (pr_mode
                                        && (!pend_l.is_empty()
                                            || !pend_r.is_empty()
                                            || !notes_l.is_empty()
                                            || !notes_r.is_empty()))
                                    || !finds_r.is_empty());
                            if show {
                                let can = self.can_comment(pr);
                                let mut lcol = div().flex_1().min_w_0().flex().flex_col();
                                let mut rcol = div().flex_1().min_w_0().flex().flex_col();
                                if pr_mode {
                                    for n in &notes_l {
                                        lcol = lcol.child(
                                            self.thread_block(n, notes, pr, true, window, cx),
                                        );
                                    }
                                    for pc in &pend_l {
                                        lcol = lcol.child(self.pending_card(pc, true, window, cx));
                                    }
                                }
                                if can
                                    && comp_l
                                    && let Some(c) = left
                                {
                                    lcol = lcol.child(self.composer_box(c.i, true, window, cx));
                                }
                                if pr_mode {
                                    for n in &notes_r {
                                        rcol = rcol.child(
                                            self.thread_block(n, notes, pr, true, window, cx),
                                        );
                                    }
                                    for pc in &pend_r {
                                        rcol = rcol.child(self.pending_card(pc, true, window, cx));
                                    }
                                }
                                for f in &finds_r {
                                    rcol = rcol.child(self.finding_card(f, true, window, cx));
                                }
                                if can
                                    && comp_r
                                    && let Some(c) = right
                                {
                                    rcol = rcol.child(self.composer_box(c.i, true, window, cx));
                                }
                                kids.push(div().flex().child(lcol).child(rcol).into_any_element());
                            }
                        }
                    }
                    slots_el.push(self.slot_wrapper(slot, kids));
                }
            } else {
                let slots = self.slots_unified();
                for (s, &i) in slots[a..b].iter().enumerate() {
                    let slot = a + s;
                    let kids = if self.lines()[i].r#type == "hunk" {
                        vec![self.hunk_row(i, slot, window, cx)]
                    } else {
                        let mut v = vec![self.unified_row(i, slot, pr, pending, notes, cx)];
                        v.extend(self.comment_block(
                            i,
                            true,
                            pr,
                            pending,
                            notes,
                            &slot.to_string(),
                            window,
                            cx,
                        ));
                        v
                    };
                    slots_el.push(self.slot_wrapper(slot, kids));
                }
            }
            let pad_top = slot_pad(Some(&self.slot_h), 0, a, DIFF_LINE_PX);
            let pad_bot = slot_pad(Some(&self.slot_h), b, count, DIFF_LINE_PX);
            root = root.child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .bg(self.ed.content_background(&self.hl_scope))
                    .child(div().flex_none().h(px(pad_top)))
                    .children(slots_el)
                    .child(div().flex_none().h(px(pad_bot))),
            );
        }
        // Drop threads whose rows left the window; their drafts park in the store.
        let seen = std::mem::take(&mut self.threads_seen);
        self.threads.retain(|k, _| seen.contains(k));
        let entity = cx.entity().downgrade();
        let geom = self.geom.clone();
        let selecting = self.selecting.is_some();
        let pane = self.pane.clone();
        let frame = canvas(
            move |b, _, cx| {
                let mut g = geom.borrow_mut();
                g.root = Some(b);
                g.painted_top = pane.as_ref().map_or(0., Pane::scroll_top);
                drop(g);
                let e = entity.clone();
                cx.defer(move |cx| {
                    if let Some(e) = e.upgrade() {
                        e.update(cx, |d, cx| d.on_frame(cx));
                    }
                });
            },
            {
                let entity = cx.entity().downgrade();
                move |_, _, window, _| {
                    if !selecting {
                        return;
                    }
                    let e1 = entity.clone();
                    window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
                        if phase != DispatchPhase::Capture {
                            return;
                        }
                        if let Some(e) = e1.upgrade() {
                            e.update(cx, |d, cx| {
                                d.move_range(f32::from(ev.position.x), f32::from(ev.position.y), cx)
                            });
                        }
                    });
                    let e2 = entity.clone();
                    window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
                        if phase != DispatchPhase::Bubble || ev.button != MouseButton::Left {
                            return;
                        }
                        if let Some(e) = e2.upgrade() {
                            e.update(cx, |d, cx| {
                                if let Some(sel) = d.selecting {
                                    let fallback =
                                        find_line(d.lines(), sel.anchor, sel.side).unwrap_or(0);
                                    d.end_range(fallback, window, cx);
                                }
                            });
                        }
                    });
                }
            },
        )
        .absolute()
        .inset_0();
        let _ = t;
        let layer = float_head.map(|(pane, head)| StickyHead {
            pane,
            head_h: self.geom.borrow().head_h.max(recur_ui::style::CHROME_H),
            child: head,
        });
        root.children(layer).child(frame)
    }
}

/// `.df-head`'s `position: sticky; top: 0` inside its card: a layer over the card that puts
/// the header at the pane top during prepaint, from the scroll being drawn. Placing it from
/// the previous frame's scroll made it ride each scroll step, then snap back.
struct StickyHead {
    pane: Pane,
    head_h: f32,
    child: AnyElement,
}

impl IntoElement for StickyHead {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl gpui_kit::Element for StickyHead {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<gpui_kit::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&gpui_kit::GlobalElementId>,
        _: Option<&gpui_kit::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui_kit::LayoutId, ()) {
        let child = self.child.request_layout(window, cx);
        let mut layer = div().absolute().inset_0().flex().flex_col();
        let style = layer.style().clone();
        let mut s = gpui_kit::Style::default();
        gpui_kit::Refineable::refine(&mut s, &style);
        (window.request_layout(s, [child], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&gpui_kit::GlobalElementId>,
        _: Option<&gpui_kit::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let top = self.pane.bounds().map_or(bounds.top(), |b| b.top());
        let max = (f32::from(bounds.size.height) - self.head_h).max(0.);
        let dy = f32::from(top - bounds.top()).clamp(0., max);
        window.with_element_offset(gpui_kit::point(px(0.), px(dy)), |window| {
            self.child.prepaint(window, cx)
        });
    }

    fn paint(
        &mut self,
        _: Option<&gpui_kit::GlobalElementId>,
        _: Option<&gpui_kit::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

impl Render for DiffFile {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

/// Drops a pending line comment from the selected PR (an empty-body `AddPRComment` on its signed
/// lines), restoring it if the call fails. The diff's pending cards and the review modal share it.
pub fn remove_pending_comment(state: &Entity<AppState>, pc: PendingLineComment, cx: &mut App) {
    let side = if pc.side.is_empty() {
        "RIGHT"
    } else {
        pc.side.as_str()
    };
    let signed = if side == "LEFT" { -pc.line } else { pc.line };
    let start = pc.start_line;
    let start_side = [&pc.start_side, &pc.side]
        .into_iter()
        .find(|s| !s.is_empty())
        .map_or("RIGHT", |s| s.as_str());
    let start_signed = if start > 0 && start < pc.line {
        if start_side == "LEFT" { -start } else { start }
    } else {
        0
    };
    let pr_id = state.read(cx).selected_pr_id.clone();
    let id = pc.id.clone();
    patch_pr_pending_comments(
        state,
        &pr_id,
        |prev| prev.iter().filter(|x| x.id != id).cloned().collect(),
        cx,
    );
    let services = state.read(cx).services().clone();
    let state = state.clone();
    let finding = if pc.finding_id.is_empty() {
        pc.id.clone()
    } else {
        pc.finding_id.clone()
    };
    cx.spawn(async move |cx| {
        let ok = call(
            cx,
            services
                .prs
                .add_pr_comment(&pr_id, &pc.file, signed, "", start_signed, &finding),
            "Could not remove pending comment",
        )
        .await;
        if ok.is_none() {
            cx.update(|cx| {
                patch_pr_pending_comments(
                    &state,
                    &pr_id,
                    |prev| {
                        let mut v = prev.to_vec();
                        v.push(pc);
                        v
                    },
                    cx,
                )
            });
        }
    })
    .detach();
}

/// The open composer's text box, drawn by the host (see [`crate::note_portal`]).
pub struct DraftView {
    card: gpui_kit::WeakEntity<DiffFile>,
}

impl DraftView {
    fn render_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match self.card.upgrade() {
            Some(card) => card.update(cx, |d, cx| d.draft_input(window, cx)),
            None => div().into_any_element(),
        }
    }
}

impl gpui_kit::Render for DraftView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}
