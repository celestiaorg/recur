//! The `.diffscroll` stack of file cards that Files changed (and PR detail's Files tab)
//! scroll through: at most [`REVIEW_HYDRATE_MAX`] cards mount a full [`DiffFile`], the rest are
//! striped stubs at their measured height; the scroll spy picks the file at the top, a
//! hydration swap keeps the viewport on its file, and each live card's header sticks to the top
//! of the pane while its body scrolls under it.
//!
//! A host owns one [`FileStack`] and renders it with [`FileStack::render`] inside its own
//! entity; `stack_of` projects the host to the stack for the callbacks.

use std::cell::RefCell;
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
use recur_domain::FileDiff;
use recur_ui::fonts::Family;
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::{IconName, Theme, icon};
use recur_view_core::AppState;

use crate::diff_file::{DiffFile, DiffFileHooks, DiffFileProps, NoteMode};
use crate::findings::FindingsByPath;
use crate::pane::Pane;
use crate::review_diff::{DiffLayout, path_label};
use crate::review_window::{
    CardRange, REVIEW_HYDRATE_MAX, REVIEW_HYDRATE_OVERSCAN, REVIEW_VIEW_PAD_PX, header_is_stuck,
    in_view_paths, pick_hydrated, record_stub_height, reserved_card_height,
};
use crate::spy_pick::{
    ViewportPin, apply_viewport_pin, capture_viewport_pin, file_at_scroll_top, step_path,
};
use crate::style::hsla;

/// How long a TOC click keeps the spy from re-picking while the pane settles.
const SPY_SUPPRESS: Duration = Duration::from_millis(700);

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
}

/// Where each card sits in scroll-content coordinates.
#[derive(Default)]
struct Cards {
    /// path → (window top, height) from the last frame.
    bounds: HashMap<String, (f32, f32)>,
    /// The scroll offset those bounds were painted at.
    painted_top: f32,
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
    last_pin: Option<ViewportPin>,
    pin_gen: u64,
    /// A hydration swap to pin after layout: (pin, captured scrollTop, generation).
    pending_pin: Option<(ViewportPin, f32, u64)>,
    /// A TOC selection to scroll to once its card lays out (two passes, as the two ticks).
    pending_select: Option<(String, u8)>,
    diff_files: HashMap<String, Entity<DiffFile>>,
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
}

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
            last_pin: None,
            pin_gen: 0,
            pending_pin: None,
            pending_select: None,
            diff_files: HashMap::new(),
            props: CardProps::default(),
            toc_reveal: 0,
            seen_top: -1.,
            active: true,
            window_dirty: true,
            spy_suppress: SPY_SUPPRESS,
            stub_selects_in_place: false,
            chrome: StackChrome::default(),
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
        self.pin_gen += 1;
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
        self.pin_gen += 1;
        self.pending_pin = None;
        self.pending_select = Some((path.to_string(), 2));
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
        let base = self.pane.top() - cards.painted_top;
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

    fn scroll_height(&self) -> f32 {
        self.pane.max_scroll() + self.pane.client_height()
    }

    /// `refreshWindow`: which cards mount, sticky headers, and the viewport pin across a
    /// hydration swap. Returns whether anything changed.
    fn refresh_window(&mut self, cx: &mut App) -> bool {
        let top = self.pane.scroll_top();
        let view_h = self.pane.client_height();
        let ranges = self.ranges();
        let pin = capture_viewport_pin(&ranges, top, view_h, self.scroll_height());
        self.last_pin = Some(pin.clone());
        let next_in_view = in_view_paths(&ranges, top, view_h, REVIEW_VIEW_PAD_PX);
        let paths: Vec<String> = self.files.iter().map(|f| f.path.clone()).collect();
        let next = pick_hydrated(
            &paths,
            &self.selected,
            &next_in_view,
            REVIEW_HYDRATE_MAX,
            REVIEW_HYDRATE_OVERSCAN,
        );
        let swapped = next != self.hydrated;
        let mut changed = false;
        if self.in_view != next_in_view {
            self.in_view = next_in_view;
        }
        if swapped {
            self.hydrated = next;
            changed = true;
        }
        self.sync_stuck_headers(&ranges, top, cx);
        if swapped && !self.spy_suppressed() {
            self.pin_gen += 1;
            self.pending_pin = Some((pin, top, self.pin_gen));
        }
        changed
    }

    /// `syncStuckHeaders` and the sticky `.df-head` itself: each live card's header follows
    /// the pane top inside its card.
    fn sync_stuck_headers(&mut self, ranges: &[CardRange], top: f32, cx: &mut App) {
        for r in ranges {
            let Some(df) = self.diff_files.get(&r.path) else {
                continue;
            };
            let stuck = header_is_stuck(r.top, r.bottom, top);
            let head_h = df.read(cx).head_height().max(40.);
            // Inside the 1px card border; the header stays within its `.diff-file`.
            let inner_top = r.top + 1.;
            let inner_bottom = r.bottom - 1.;
            let shift = (top - inner_top)
                .max(0.)
                .min((inner_bottom - inner_top - head_h).max(0.));
            df.update(cx, |d, cx| d.set_head_shift(shift, stuck, cx));
        }
    }

    /// Each frame: spy, window refresh, pending pins and selection scrolls.
    fn on_frame(&mut self, cx: &mut App) -> bool {
        let mut changed = false;
        if let Some((path, passes)) = self.pending_select.take() {
            let ranges = self.ranges();
            if let Some(r) = ranges.iter().find(|r| r.path == path) {
                self.pane.set_scroll_top(r.top);
                if passes > 1 {
                    self.pending_select = Some((path, passes - 1));
                } else {
                    let ranges = self.ranges();
                    self.last_pin = Some(capture_viewport_pin(
                        &ranges,
                        self.pane.scroll_top(),
                        self.pane.client_height(),
                        self.scroll_height(),
                    ));
                }
                changed = true;
            } else {
                self.pending_select = Some((path, passes));
            }
        }
        if let Some((pin, captured, generation)) = self.pending_pin.take()
            && generation == self.pin_gen
        {
            let ranges = self.ranges();
            if let Some(top) = apply_viewport_pin(
                self.pane.scroll_top(),
                self.pane.max_scroll(),
                &ranges,
                &pin,
                Some(captured),
            ) {
                self.pane.set_scroll_top(top);
                changed = true;
            }
            let ranges = self.ranges();
            self.last_pin = Some(capture_viewport_pin(
                &ranges,
                self.pane.scroll_top(),
                self.pane.client_height(),
                self.scroll_height(),
            ));
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
        if self.active && (scrolled || dirty) && self.refresh_window(cx) {
            changed = true;
        }
        changed
    }

    /// `recordStubHeight` for every live card, and `pinIfAtEnd` when one resizes.
    fn record_heights(&mut self) -> bool {
        let cards = self.cards.borrow();
        let mut resized = false;
        for f in &self.files {
            let Some(&(_, h)) = cards.bounds.get(&f.path) else {
                continue;
            };
            let stub = !self.hydrated.contains(&f.path);
            if record_stub_height(
                &mut self.measured,
                &f.path,
                stub,
                self.collapsed.contains(&f.path),
                h,
            ) {
                resized = true;
            }
        }
        drop(cards);
        if resized && self.last_pin.as_ref().is_some_and(|p| p.at_end) && !self.spy_suppressed() {
            self.pane.set_scroll_top(self.pane.max_scroll());
            return true;
        }
        false
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
        props
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
                let body = if live {
                    let props = self.card_props(f);
                    let entity = match self.diff_files.get(&f.path) {
                        Some(e) => {
                            e.update(cx, |d, cx| d.set_props(props, window, cx));
                            e.clone()
                        }
                        None => {
                            let state = self.state.clone();
                            let pane = self.pane.clone();
                            let hooks = self.card_hooks(f, host.clone(), on_toggle.clone());
                            let e = cx.new(|cx| {
                                DiffFile::new(state, props, hooks, Some(pane), window, cx)
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
                        .h(px(40.))
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
                    .mb(px(14.))
                    .when_some(reserved, |d, h| d.min_h(px(h)))
                    .child(cfile_frame(t.bg, t.line, stripes.is_some()))
                    .child(body)
                    .child(probe);
                col = col.child(div().when(first, |d| d.mt(px(pad))).child(card));
                first = false;
            }
        }
        self.cards
            .borrow_mut()
            .bounds
            .retain(|p, _| self.files.iter().any(|f| &f.path == p));
        let pane = self.pane.clone();
        let entity = host.clone();
        let painted = self.cards.clone();
        let frame = canvas(
            move |b, _, cx| {
                pane.set_bounds(b);
                painted.borrow_mut().painted_top = pane.scroll_top();
                let e = entity.clone();
                cx.defer(move |cx| {
                    let _ = e.update(cx, |host, cx| {
                        let s = stack_of(host);
                        let a = s.record_heights();
                        let b = s.on_frame(cx);
                        if a || b {
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
                    .on_key_down(move |ev: &KeyDownEvent, _, cx| {
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
/// composite over the border's corner arcs in the web view.
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
