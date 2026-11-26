//! The `.diffscroll` stack of file cards that Files changed (and PR detail's Files tab)
//! scroll through: every card on screen mounts a full [`DiffFile`], as do those near it up to
//! [`REVIEW_HYDRATE_MAX`] in all, and the rest are striped stubs at their measured height; the
//! scroll spy picks the file at the top, [`Anchored`] keeps the rows on screen in place as
//! heights change, and each live card's header sticks to the top of the pane while its body
//! scrolls under it.
//!
//! A host owns one [`FileStack`] and renders it with [`FileStack::render`] inside its own
//! entity; `stack_of` projects the host to the stack for the callbacks.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, SharedString, StatefulInteractiveElement, Styled,
    WeakEntity, Window, canvas, div, px,
};
use recur_code::view::finding_paths::findings_for_path;
use recur_content::images::workspace_file_url;
use recur_domain::{FileDiff, Pr};
use recur_ui::fonts::Family;
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::{IconName, Theme, icon};
use recur_view_core::AppState;

use crate::anchor::{Anchor, Anchored, LaidOut, Tracked};
use crate::card_editor::WorkingEditors;
use crate::diff_file::{DiffFile, DiffFileHooks, DiffFileProps, NoteMode};
use crate::findings::FindingsByPath;
use crate::pane::Pane;
use crate::review_diff::{DiffLayout, path_label};
use crate::review_window::{
    CardRange, REVIEW_HYDRATE_MAX, REVIEW_HYDRATE_OVERSCAN, REVIEW_VIEW_PAD_PX, header_is_stuck,
    in_view_paths, pick_hydrated, record_stub_height, reserved_card_height,
};
use crate::spy_pick::{file_at_scroll_top, step_path};
use crate::style::hsla;

/// How long a TOC click keeps the spy from re-picking while the pane settles.
const SPY_SUPPRESS: Duration = Duration::from_millis(700);
/// A card's bottom margin (`.cfile`).
const CARD_GAP: f32 = 14.;
/// A stub card: its header inside the 1px frame.
pub(crate) const STUB_CARD_H: f32 = recur_ui::style::CHROME_H + 2.;
/// How far beyond the viewport stub cards stay mounted.
const STACK_MOUNT_PAD_PX: f32 = 1200.;

/// The host's `onToggleCollapse` for a card: (path, collapsed).
pub type ToggleCard<H> = Rc<dyn Fn(&mut H, String, bool, &mut Context<H>)>;

/// The box the cards sit in inside the scroller: `.diffscroll`'s 14px padding, or PR detail's
/// `.commits-wrap.detail` (18px, centered at 1560px).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StackChrome {
    pub pad: f32,
    pub max_width: Option<f32>,
}

impl Default for StackChrome {
    fn default() -> Self {
        StackChrome {
            pad: 14.,
            max_width: None,
        }
    }
}

/// What every card's [`DiffFile`] gets besides its file.
#[derive(Clone, Default)]
pub struct CardProps {
    pub layout: DiffLayout,
    pub note_mode: Option<NoteMode>,
    pub comments: bool,
    pub collapsible: bool,
    pub open_file_title: Option<SharedString>,
    pub findings_by_path: Option<Rc<FindingsByPath>>,
    /// The selected file's highlight jump (`highlightLine`), PR detail.
    pub highlight_line: i64,
    pub posting_finding_ids: HashSet<String>,
    pub postable_finding_ids: HashSet<String>,
    pub hooks: DiffFileHooks,
    /// Where image files' previous and current revisions come from.
    pub image_revs: Option<ImageRevs>,
    /// The host's editable diffs, for the cards that mount one (the Uncommitted tab).
    pub editors: Option<Rc<dyn WorkingEditors>>,
}

/// The `/workspace-file` revisions an image card compares: `before` and `after` are `at`
/// values (`base`, a commit-ish or `A...B`); an empty `after` is the worktree, cached until
/// `t` changes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImageRevs {
    pub project_id: String,
    pub workspace_id: String,
    pub before: String,
    pub after: String,
    pub t: i64,
}

impl ImageRevs {
    /// `before` and `after` of a PR, read from its project repo (its worktree when it has no
    /// project), so they resolve after the worktree is gone.
    pub fn pr(pr: &Pr, before: String, after: String) -> Self {
        ImageRevs {
            project_id: pr.project_id.clone(),
            workspace_id: if pr.project_id.is_empty() {
                pr.workspace_id.clone()
            } else {
                String::new()
            },
            before,
            after,
            t: 0,
        }
    }

    /// The previous and current srcs of `f` ("" for a side it does not have), or `None` when
    /// `f` is not a raster image without text rows.
    pub fn srcs(&self, f: &FileDiff) -> Option<(String, String)> {
        if recur_code::image_mime(&f.path).is_empty()
            || f.lines.as_ref().is_some_and(|l| !l.is_empty())
        {
            return None;
        }
        let url = |path: &str, at: &str| {
            let t = if at.is_empty() { self.t } else { 0 };
            workspace_file_url(&self.project_id, &self.workspace_id, path, t, at)
        };
        let old = if f.old_path.is_empty() {
            &f.path
        } else {
            &f.old_path
        };
        let before = match f.status.as_str() {
            "a" | "u" => String::new(),
            _ => url(old, &self.before),
        };
        let after = match f.status.as_str() {
            "d" => String::new(),
            _ => url(&f.path, &self.after),
        };
        Some((before, after))
    }
}

/// Where each card sits in scroll-content coordinates.
#[derive(Default)]
struct Cards {
    /// path → (window top, height) from the last frame.
    bounds: HashMap<String, (f32, f32)>,
    /// The scrolled column's window top in that frame.
    col_top: Rc<Cell<f32>>,
}

pub struct FileStack {
    state: Entity<AppState>,
    pub pane: Pane,
    pub focus: FocusHandle,
    files: Vec<Rc<FileDiff>>,
    selected: String,
    collapsed: HashSet<String>,
    hydrated: HashSet<String>,
    in_view: Vec<String>,
    measured: HashMap<String, f32>,
    cards: Rc<RefCell<Cards>>,
    spy_until: Option<Instant>,
    /// A TOC selection to scroll to once its card lays out.
    pending_select: Option<String>,
    /// The card just scrolled to, which the next frame lays out at the pane's top.
    scrolled_to: Option<String>,
    diff_files: HashMap<String, Entity<DiffFile>>,
    /// The cards the host last kept editors for.
    editing: HashSet<String>,
    props: CardProps,
    /// Bumped when the spy picks a file (`tocReveal`).
    pub toc_reveal: u64,
    seen_top: f32,
    active: bool,
    /// `refreshWindow` runs on scroll and after the list, selection or collapse changes (not
    /// every frame: a hydration swap that resizes cards must not feed back into itself).
    window_dirty: bool,
    /// How long a selection keeps the spy from re-picking (PR detail's Files tab: 1.4s).
    spy_suppress: Duration,
    /// A stub's header selects its file without scrolling (PR detail's commit diff).
    stub_selects_in_place: bool,
    chrome: StackChrome,
    #[cfg(test)]
    pub(crate) frames: Vec<FrameLog>,
}

#[cfg(test)]
pub(crate) type FrameLog = (HashMap<(String, usize), f32>, bool);

impl FileStack {
    pub fn new(state: Entity<AppState>, cx: &mut App) -> Self {
        FileStack {
            state,
            pane: Pane::new(),
            focus: cx.focus_handle(),
            files: Vec::new(),
            selected: String::new(),
            collapsed: HashSet::new(),
            hydrated: HashSet::new(),
            in_view: Vec::new(),
            measured: HashMap::new(),
            cards: Rc::default(),
            spy_until: None,
            pending_select: None,
            scrolled_to: None,
            diff_files: HashMap::new(),
            editing: HashSet::new(),
            props: CardProps::default(),
            toc_reveal: 0,
            seen_top: -1.,
            active: true,
            window_dirty: true,
            spy_suppress: SPY_SUPPRESS,
            stub_selects_in_place: false,
            chrome: StackChrome::default(),
            #[cfg(test)]
            frames: Vec::new(),
        }
    }

    pub fn set_chrome(&mut self, chrome: StackChrome) {
        self.chrome = chrome;
    }

    pub fn set_spy_suppress(&mut self, d: Duration) {
        self.spy_suppress = d;
    }

    pub fn set_stub_selects_in_place(&mut self, on: bool) {
        self.stub_selects_in_place = on;
    }

    /// A stub header in a commit diff: select it in place, so it hydrates where it is.
    pub fn select_in_place(&mut self, path: &str) {
        self.spy_until = Some(Instant::now() + SPY_SUPPRESS);
        self.selected = path.to_string();
        self.window_dirty = true;
    }

    pub fn files(&self) -> &[Rc<FileDiff>] {
        &self.files
    }

    pub fn selected(&self) -> &str {
        &self.selected
    }

    pub fn collapsed(&self) -> &HashSet<String> {
        &self.collapsed
    }

    pub fn hydrated(&self) -> &HashSet<String> {
        &self.hydrated
    }

    /// The rows on screen in the frame just drawn, by path and slot, and whether a stub shows.
    #[cfg(test)]
    fn log_frame(&mut self, cx: &App) {
        let Some(view) = self.pane.bounds() else {
            return;
        };
        let (a, b) = (f32::from(view.top()), f32::from(view.bottom()));
        let mut rows = HashMap::new();
        for (p, e) in &self.diff_files {
            for (slot, top) in e.read(cx).slot_tops() {
                if top >= a && top < b {
                    rows.insert((p.clone(), slot), top);
                }
            }
        }
        let blank = self.cards.borrow().bounds.iter().any(|(p, &(top, h))| {
            top + h > a && top < b && !self.hydrated.contains(p) && !self.collapsed.contains(p)
        });
        self.frames.push((rows, blank));
    }

    pub fn diff_file(&self, path: &str) -> Option<&Entity<DiffFile>> {
        self.diff_files.get(path)
    }

    pub fn set_active(&mut self, active: bool) {
        if active && !self.active {
            self.window_dirty = true;
        }
        self.active = active;
    }

    pub fn set_props(&mut self, props: CardProps) {
        if props.editors.is_none() {
            self.editing.clear();
        }
        self.props = props;
    }

    /// New matched files (in display order). Keeps the selection when it is still listed,
    /// else selects the first (the `matchedFiles` effect).
    pub fn set_files(&mut self, files: Vec<Rc<FileDiff>>) {
        let same = files.len() == self.files.len()
            && files
                .iter()
                .zip(&self.files)
                .all(|(a, b)| Rc::ptr_eq(a, b) || a.path == b.path);
        if !same {
            self.window_dirty = true;
        }
        self.files = files;
        if self.files.is_empty() {
            self.selected.clear();
        } else if !self.files.iter().any(|f| f.path == self.selected) {
            self.selected = self.files[0].path.clone();
        }
        let live: HashSet<&str> = self.files.iter().map(|f| f.path.as_str()).collect();
        self.diff_files.retain(|p, _| live.contains(p.as_str()));
    }

    /// Restore a selection without scrolling (`applyRestore`).
    pub fn set_selected(&mut self, path: &str) {
        self.selected = path.to_string();
        self.window_dirty = true;
    }

    pub fn any_expanded(&self) -> bool {
        self.files.iter().any(|f| !self.collapsed.contains(&f.path))
    }

    pub fn toggle_collapsed(&mut self, path: &str, next: bool) {
        self.window_dirty = true;
        if next {
            self.collapsed.insert(path.to_string());
        } else {
            self.collapsed.remove(path);
        }
    }

    pub fn toggle_fold_all(&mut self) {
        self.window_dirty = true;
        self.collapsed = if self.any_expanded() {
            self.files.iter().map(|f| f.path.clone()).collect()
        } else {
            HashSet::new()
        };
    }

    fn spy_suppressed(&self) -> bool {
        self.spy_until.is_some_and(|t| Instant::now() < t)
    }

    /// `selectPath`: select, expand, and scroll its card to the top once laid out.
    pub fn select_path(&mut self, path: &str) {
        self.selected = path.to_string();
        self.collapsed.remove(path);
        self.spy_until = Some(Instant::now() + self.spy_suppress);
        self.pending_select = Some(path.to_string());
        self.window_dirty = true;
    }

    /// `j` / `k` / ⌥↓ / ⌥↑ (`onDiffKeydown`).
    pub fn step(&mut self, dir: i8) -> bool {
        let paths: Vec<String> = self.files.iter().map(|f| f.path.clone()).collect();
        match step_path(&paths, &self.selected, dir).cloned() {
            Some(next) => {
                self.select_path(&next);
                true
            }
            None => false,
        }
    }

    fn ranges(&self) -> Vec<CardRange> {
        let cards = self.cards.borrow();
        let base = cards.col_top.get();
        let mut v: Vec<CardRange> = self
            .files
            .iter()
            .filter_map(|f| {
                // `offsetTop` / `offsetHeight`: pixel-snapped edges.
                cards.bounds.get(&f.path).map(|&(top, h)| CardRange {
                    path: f.path.clone(),
                    top: (top - base).round(),
                    bottom: (top - base + h).round(),
                })
            })
            .collect();
        v.sort_by(|a, b| a.top.total_cmp(&b.top));
        v
    }

    /// `refreshWindow`: which cards mount. Returns whether anything changed.
    fn refresh_window(&mut self) -> bool {
        let (top, view_h) = self.pane.viewport();
        let ranges = self.ranges();
        self.in_view = in_view_paths(&ranges, top, view_h, REVIEW_VIEW_PAD_PX);
        let paths: Vec<String> = self.files.iter().map(|f| f.path.clone()).collect();
        let mut next = pick_hydrated(
            &paths,
            &self.selected,
            &self.in_view,
            REVIEW_HYDRATE_MAX,
            REVIEW_HYDRATE_OVERSCAN,
        );
        // A card on screen is never a stub, however many small ones fit.
        next.extend(in_view_paths(&ranges, top, view_h, 0.));
        let swapped = next != self.hydrated;
        self.hydrated = next;
        swapped
    }

    /// What the last frame shows at `top`: a row of the card there and the card itself. Cards
    /// that were live come first: a stub's height is about to change.
    fn pick_anchors(&mut self, top: f32, cx: &mut App) -> (Vec<Anchor>, Option<(String, LaidOut)>) {
        let picked = {
            let cards = self.cards.borrow();
            let col = cards.col_top.get();
            let view_h = self.pane.client_height();
            let mut on_screen = self.files.iter().filter_map(|f| {
                let &(y, h) = cards.bounds.get(&f.path)?;
                let y = y - col;
                (y + h > top && y < top + view_h).then_some((&f.path, y, h))
            });
            match &self.scrolled_to {
                Some(p) => on_screen.find(|c| c.0 == p),
                None => {
                    let mut first = None;
                    on_screen
                        .find(|c| {
                            first.get_or_insert(*c);
                            self.diff_files.contains_key(c.0)
                        })
                        .or(first)
                }
            }
            .map(|(p, y, h)| (p.clone(), y, h, col))
        };
        let scrolled_to = self.scrolled_to.take().is_some();
        let Some((path, card_y, card_h, col)) = picked else {
            return (Vec::new(), None);
        };
        // At the very top, what appears above shows (`overflow-anchor` is off there too).
        if top <= 0. && !scrolled_to {
            return (Vec::new(), None);
        }
        let mut anchors = Vec::new();
        if !scrolled_to
            && let Some((at, y)) = self
                .diff_files
                .get(&path)
                .and_then(|d| d.update(cx, |d, _| d.track_slot_at(top + col)))
        {
            anchors.push(Anchor {
                at,
                y: y - col,
                card_h: None,
            });
        }
        let at = LaidOut::default();
        anchors.push(Anchor {
            at: at.clone(),
            y: card_y,
            card_h: Some(card_h),
        });
        (anchors, Some((path, at)))
    }

    /// `syncStuckHeaders`. The header itself follows the pane top as it draws (`StickyHead`).
    fn sync_stuck_headers(&mut self, ranges: &[CardRange], top: f32, cx: &mut App) {
        for r in ranges {
            let Some(df) = self.diff_files.get(&r.path) else {
                continue;
            };
            let stuck = header_is_stuck(r.top, r.bottom, top);
            df.update(cx, |d, cx| d.set_head_stuck(stuck, cx));
        }
    }

    /// After each frame: selection scrolls, the spy, and whether other cards should mount.
    fn on_frame(&mut self, cx: &mut App) -> bool {
        let mut changed = false;
        if let Some(path) = self.pending_select.take() {
            match self.ranges().iter().find(|r| r.path == path) {
                Some(r) => {
                    // Unclamped: the frame that expands a card near the end clamps it.
                    self.pane.scroll_to_unclamped(r.top);
                    self.scrolled_to = Some(path);
                    changed = true;
                }
                None => self.pending_select = Some(path),
            }
        }
        let top = self.pane.scroll_top();
        let scrolled = (top - self.seen_top).abs() > 0.01;
        self.seen_top = top;
        // `spyFromOffsets`: on scroll and on layout changes (the IntersectionObserver).
        if !self.spy_suppressed() {
            let pick = file_at_scroll_top(&self.ranges(), top);
            if !pick.is_empty() && pick != self.selected {
                self.selected = pick;
                self.toc_reveal += 1;
                self.window_dirty = true;
                changed = true;
            }
        }
        let dirty = std::mem::take(&mut self.window_dirty);
        if self.active && (scrolled || dirty) {
            changed |= self.refresh_window();
            let ranges = self.ranges();
            self.sync_stuck_headers(&ranges, self.pane.viewport().0, cx);
        }
        changed
    }

    /// `recordStubHeight` for every live card.
    fn record_heights(&mut self) {
        let cards = self.cards.borrow();
        for f in &self.files {
            let Some(&(_, h)) = cards.bounds.get(&f.path) else {
                continue;
            };
            record_stub_height(
                &mut self.measured,
                &f.path,
                !self.hydrated.contains(&f.path),
                self.collapsed.contains(&f.path),
                h,
            );
        }
    }

    fn card_props(&self, f: &Rc<FileDiff>) -> DiffFileProps {
        let p = &self.props;
        let mut props = DiffFileProps::new(f.clone());
        props.layout = p.layout;
        props.note_mode = p.note_mode;
        props.comments = p.comments;
        props.collapsible = p.collapsible;
        props.collapsed = self.collapsed.contains(&f.path);
        if let Some(t) = &p.open_file_title {
            props.open_file_title = t.clone();
        }
        props.findings = p
            .findings_by_path
            .as_deref()
            .map(|by| findings_for_path(by, &f.path).to_vec())
            .unwrap_or_default();
        props.highlight_line = if f.path == self.selected {
            p.highlight_line
        } else {
            0
        };
        props.posting_finding_ids = p.posting_finding_ids.clone();
        props.postable_finding_ids = p.postable_finding_ids.clone();
        props.image = p.image_revs.as_ref().and_then(|r| r.srcs(f));
        props
    }

    /// A run of unmounted stub cards: one exact spacer as tall as the cards and their gaps,
    /// recording each card's bounds where it would have painted.
    fn skipped_cards(&self, run: Vec<(String, f32)>, first: bool) -> AnyElement {
        let total: f32 = run.iter().map(|(_, h)| h + CARD_GAP).sum();
        let cards = self.cards.clone();
        let probe = canvas(
            move |b, _, _| {
                let mut c = cards.borrow_mut();
                let mut y = f32::from(b.origin.y);
                for (path, h) in &run {
                    c.bounds.insert(path.clone(), (y, *h));
                    y += h + CARD_GAP;
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        div()
            .when(first, |d| d.mt(px(self.chrome.pad)))
            .relative()
            .child(div().flex_none().h(px(total)))
            .child(probe)
            .into_any_element()
    }

    /// The card views built so far.
    pub fn diff_files(&self) -> impl Iterator<Item = &Entity<DiffFile>> {
        self.diff_files.values()
    }

    /// The `.diffscroll` pane: `header` (scope headers, notices) above the cards, or `empty`
    /// in place of them. `stack_of` projects the host entity to this stack.
    #[allow(clippy::too_many_arguments)]
    pub fn render<H: 'static>(
        &mut self,
        header: Vec<AnyElement>,
        empty: Option<AnyElement>,
        stack_of: fn(&mut H) -> &mut FileStack,
        on_toggle: ToggleCard<H>,
        window: &mut Window,
        cx: &mut Context<H>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        self.pane.begin_draw();
        let scroll_top = self.pane.scroll_top();
        let (anchors, mut anchor_card) = self.pick_anchors(scroll_top, cx);
        // Before the draw, so the cards this frame scrolled to are in it.
        if self.active && (self.window_dirty || scroll_top != self.seen_top) {
            self.refresh_window();
        }
        let host: WeakEntity<H> = cx.entity().downgrade();
        let pad = self.chrome.pad;
        let mut col = div().flex().flex_col().w_full().pb(px(pad));
        let mut first = true;
        let mut push = |el: AnyElement, col: &mut gpui_kit::Div| {
            let wrapped = div().when(first, |d| d.mt(px(pad))).child(el);
            first = false;
            *col = std::mem::replace(col, div()).child(wrapped);
        };
        for h in header {
            push(h, &mut col);
        }
        if let Some(e) = empty {
            push(e, &mut col);
        } else {
            let files = self.files.clone();
            // Stubs far from the viewport stay unmounted (thousands of cards laid out each
            // frame cost seconds): each run of them is one exact spacer that still records
            // every card's bounds, so ranges, the spy and selection scrolls see all cards.
            let (top, view_h) = self.pane.viewport();
            let (lo, hi) = (
                top - STACK_MOUNT_PAD_PX,
                top + view_h.max(0.) + STACK_MOUNT_PAD_PX,
            );
            let mut y = 0.;
            let mut skipped: Vec<(String, f32)> = Vec::new();
            for f in &files {
                let live = self.hydrated.contains(&f.path);
                let collapsed = self.collapsed.contains(&f.path);
                let reserved = reserved_card_height(
                    f.lines.as_ref().map(Vec::len),
                    f.diff.add,
                    f.diff.del,
                    live,
                    collapsed,
                    self.measured.get(&f.path).copied(),
                );
                let stub_h = reserved.unwrap_or(0.).max(STUB_CARD_H);
                let est = self
                    .cards
                    .borrow()
                    .bounds
                    .get(&f.path)
                    .map_or(stub_h, |b| b.1);
                let near = y + est >= lo && y <= hi;
                y += est + CARD_GAP;
                if !(live || near || f.path == self.selected) {
                    self.diff_files.remove(&f.path);
                    skipped.push((f.path.clone(), stub_h));
                    continue;
                }
                if !skipped.is_empty() {
                    col = col.child(self.skipped_cards(std::mem::take(&mut skipped), first));
                    first = false;
                }
                let body = if live {
                    let mut props = self.card_props(f);
                    if let Some(e) = &self.props.editors {
                        props.editor = e.editor(f, window, cx);
                    }
                    let entity = match self.diff_files.get(&f.path) {
                        Some(e) => {
                            e.update(cx, |d, cx| d.set_props(props, window, cx));
                            e.clone()
                        }
                        None => {
                            let state = self.state.clone();
                            let pane = self.pane.clone();
                            let hooks = self.card_hooks(f, host.clone(), on_toggle.clone());
                            let at = self.cards.borrow().bounds.get(&f.path).copied();
                            let painted_top = self.pane.top() - self.cards.borrow().col_top.get();
                            let e = cx.new(|cx| {
                                let mut d =
                                    DiffFile::new(state, props, hooks, Some(pane), window, cx);
                                if let Some((top, h)) = at {
                                    d.place(top, h, painted_top);
                                }
                                d
                            });
                            self.diff_files.insert(f.path.clone(), e.clone());
                            e
                        }
                    };
                    let hooks = self.card_hooks(f, host.clone(), on_toggle.clone());
                    entity.update(cx, |d, _| d.set_hooks(hooks));
                    entity.into_any_element()
                } else {
                    let path = f.path.clone();
                    let h = host.clone();
                    div()
                        .id(SharedString::from(format!("stub-head-{}", f.path)))
                        .debug_selector({
                            let p = f.path.clone();
                            move || format!("stub-head:{p}")
                        })
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .h(px(recur_ui::style::CHROME_H))
                        .px(px(13.))
                        .rounded(px(8.))
                        .bg(hsla(t.pane))
                        .border_b_1()
                        .border_color(hsla(t.line))
                        .cursor_pointer()
                        .hover(|s| s.bg(hsla(t.pane_2)))
                        .font_family(Family::Mono.primary())
                        .text_size(px(11.5))
                        .child(
                            icon(IconName::ChevronDown)
                                .size(px(12.))
                                .stroke_width(2.4)
                                .color(hsla(t.faint))
                                .rotate(-90.),
                        )
                        .child(
                            div().flex_initial().min_w_0().child(
                                text(path_label(f))
                                    .family(Family::Mono)
                                    .size(11.5)
                                    .line_height(LineHeight::Scale(1.45))
                                    .color(t.text)
                                    .truncate(),
                            ),
                        )
                        .child(div().ml_auto().child(crate::style::add_del(
                            f.diff.add,
                            f.diff.del,
                            Family::Mono,
                            10.5,
                            8.,
                            LineHeight::Scale(1.45),
                            cx,
                        )))
                        .on_click(move |_, _, cx| {
                            let path = path.clone();
                            let _ = h.update(cx, |host, cx| {
                                let s = stack_of(host);
                                if s.stub_selects_in_place {
                                    s.select_in_place(&path);
                                } else {
                                    s.select_path(&path);
                                }
                                cx.notify();
                            });
                        })
                        .into_any_element()
                };
                if !live {
                    self.diff_files.remove(&f.path);
                }
                let cards = self.cards.clone();
                let path = f.path.clone();
                let probe = canvas(
                    move |b, _, _| {
                        cards.borrow_mut().bounds.insert(
                            path.clone(),
                            (f32::from(b.origin.y), f32::from(b.size.height)),
                        );
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0();
                let stripes = (!live).then_some(());
                let sel = format!("cfile:{}:{}", if live { "live" } else { "stub" }, f.path);
                let card = div()
                    .relative()
                    .p(px(1.))
                    .debug_selector(move || sel.clone())
                    .mb(px(CARD_GAP))
                    .when_some(reserved, |d, h| d.min_h(px(h)))
                    .child(cfile_frame(t.bg, t.line, stripes.is_some()))
                    .child(body)
                    .child(probe)
                    .into_any_element();
                let card = match anchor_card.take_if(|(p, _)| *p == f.path) {
                    Some((_, at)) => Tracked { child: card, at }.into_any_element(),
                    None => card,
                };
                col = col.child(div().when(first, |d| d.mt(px(pad))).child(card));
                first = false;
            }
            if !skipped.is_empty() {
                col = col.child(self.skipped_cards(skipped, first));
            }
            if let Some(e) = self.props.editors.clone() {
                let live: HashSet<String> = self
                    .files
                    .iter()
                    .filter(|f| self.hydrated.contains(&f.path))
                    .map(|f| f.path.clone())
                    .collect();
                if live != self.editing {
                    self.editing = live.clone();
                    e.retain(live, cx);
                }
            }
        }
        let paths: HashSet<&str> = self.files.iter().map(|f| f.path.as_str()).collect();
        self.cards
            .borrow_mut()
            .bounds
            .retain(|p, _| paths.contains(p.as_str()));
        let pane = self.pane.clone();
        let entity = host.clone();
        let frame = canvas(
            move |b, _, cx| {
                pane.set_bounds(b);
                let e = entity.clone();
                cx.defer(move |cx| {
                    let _ = e.update(cx, |host, cx| {
                        let s = stack_of(host);
                        #[cfg(test)]
                        s.log_frame(cx);
                        s.record_heights();
                        if s.on_frame(cx) {
                            cx.notify();
                        }
                    });
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        let key_host = host.clone();
        let col = Anchored {
            child: col.into_any_element(),
            pane: self.pane.clone(),
            scroll_top,
            anchors,
            col_top: self.cards.borrow().col_top.clone(),
        };
        div()
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(hsla(t.bg))
            .child(
                div()
                    .id("diffscroll")
                    .debug_selector(|| "diffscroll".into())
                    .track_focus(&self.focus)
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .on_key_down(move |ev: &KeyDownEvent, window, cx| {
                        // A card's editor takes its own keys.
                        if window
                            .context_stack()
                            .iter()
                            .any(|c| c.contains("RecurEditor"))
                        {
                            return;
                        }
                        let k = &ev.keystroke;
                        let m = &k.modifiers;
                        let plain = !m.platform && !m.control && !m.alt;
                        let dir = match k.key.as_str() {
                            "j" if plain => 1,
                            "down" if m.alt => 1,
                            "k" if plain => -1,
                            "up" if m.alt => -1,
                            _ => return,
                        };
                        cx.stop_propagation();
                        let _ = key_host.update(cx, |host, cx| {
                            if stack_of(host).step(dir) {
                                cx.notify();
                            }
                        });
                    })
                    .child(
                        recur_ui::widgets::scroll_area("diffscroll-sa")
                            .track(&self.pane.scroll)
                            .child(match self.chrome.max_width {
                                Some(w) => {
                                    div().w_full().max_w(px(w)).mx_auto().px(px(pad)).child(col)
                                }
                                None => div().px(px(pad)).child(col),
                            }),
                    ),
            )
            .child(frame)
            .into_any_element()
    }

    fn card_hooks<H: 'static>(
        &self,
        f: &Rc<FileDiff>,
        host: WeakEntity<H>,
        on_toggle: ToggleCard<H>,
    ) -> DiffFileHooks {
        let mut hooks = self.props.hooks.clone();
        let path = f.path.clone();
        hooks.toggle_collapse = Some(Rc::new(move |next, _, cx| {
            let path = path.clone();
            let on_toggle = on_toggle.clone();
            let _ = host.update(cx, |h, cx| on_toggle(h, path, next, cx));
        }));
        hooks
    }
}

/// The `.cfile` box as CSS paints it, before its children: the `--bg` background (shrunk a
/// device pixel under the opaque border), the stub stripes
/// (`repeating-linear-gradient(to bottom, transparent 0 19px, var(--line) 19px 20px)`), then
/// the 1px radius-9 border. GPUI would paint a border after the children; translucent rows
/// composite over the border's corner arcs instead.
fn cfile_frame(bg: recur_ui::Color, line: recur_ui::Color, stripes: bool) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let r = 9.;
            let dp = 1. / window.scale_factor();
            let inner = gpui_kit::Bounds::new(
                gpui_kit::point(b.origin.x + px(dp), b.origin.y + px(dp)),
                gpui_kit::size(b.size.width - px(2. * dp), b.size.height - px(2. * dp)),
            );
            window.paint_quad(
                gpui_kit::fill(inner, gpui_kit::Hsla::from(bg)).corner_radii(px(r - dp)),
            );
            if stripes {
                let h = f32::from(b.size.height);
                let mut y = 19.;
                while y + 1. <= h {
                    let mid = y + 0.5;
                    let inset = if mid < r {
                        r - (r * r - (r - mid) * (r - mid)).max(0.).sqrt()
                    } else if mid > h - r {
                        let d = mid - (h - r);
                        r - (r * r - d * d).max(0.).sqrt()
                    } else {
                        0.
                    };
                    let rect = gpui_kit::Bounds::new(
                        gpui_kit::point(b.origin.x + px(inset), b.origin.y + px(y)),
                        gpui_kit::size(b.size.width - px(2. * inset), px(1.)),
                    );
                    window.paint_quad(gpui_kit::fill(rect, gpui_kit::Hsla::from(line)));
                    y += 20.;
                }
            }
            window.paint_quad(gpui_kit::quad(
                b,
                px(r),
                gpui_kit::Hsla::transparent_black(),
                px(1.),
                gpui_kit::Hsla::from(line),
                gpui_kit::BorderStyle::Solid,
            ));
        },
    )
    .absolute()
    .inset_0()
}
