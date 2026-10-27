//! `TabStrip.svelte` with `lib/tab-strip-scroll.ts` and `lib/tab-strip-drag.svelte.ts`: a
//! horizontally scrolling tab row with pinned leading/trailing actions, active-tab reveal,
//! keep-alive scroll restore, and drag (or ⌥←/⌥→) reorder.

use crate::motion::{self, Easing};
use crate::theme::Theme;
use crate::widgets::tab_chip::TabChip;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Bounds, CursorStyle, ElementId, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels,
    Point, RenderOnce, ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Window,
    canvas, deferred, div, point, px,
};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Pointer travel before a press becomes a drag (`CHAT_TAB_DRAG_THRESHOLD`).
pub const DRAG_THRESHOLD: f32 = 4.;
const FLIP: Duration = Duration::from_millis(180);
const SETTLE: Duration = Duration::from_millis(200);
const EDGE: f32 = 28.;
const EDGE_STEP: f32 = 12.;
/// Matches Safari's smooth `scrollIntoView` for a tab hop.
const SMOOTH: Duration = Duration::from_millis(280);

/// `reorderByInsert`: move `from` so it lands at `insert_at` in the list without it.
pub fn reorder_by_insert<T: Clone>(list: &[T], from: isize, insert_at: usize) -> Vec<T> {
    if from < 0 || from as usize >= list.len() {
        return list.to_vec();
    }
    let from = from as usize;
    let item = list[from].clone();
    let mut others: Vec<T> = list
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != from)
        .map(|(_, t)| t.clone())
        .collect();
    let at = insert_at.min(others.len());
    others.insert(at, item);
    others
}

/// `insertIndexFromPointerX`: the first midpoint right of the pointer, else append.
pub fn insert_index_from_pointer_x(mids: &[f32], x: f32) -> usize {
    mids.iter().position(|m| x < *m).unwrap_or(mids.len())
}

/// `restoreStripOrder`: the snapshot order for keys still live, then any new keys.
pub fn restore_strip_order<T: Clone, K: PartialEq + std::hash::Hash + Eq>(
    current: &[T],
    snapshot: &[T],
    key: impl Fn(&T) -> K,
) -> Vec<T> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for s in snapshot {
        let k = key(s);
        if seen.contains(&k) {
            continue;
        }
        if let Some(live) = current.iter().find(|c| key(c) == k) {
            out.push(live.clone());
            seen.insert(k);
        }
    }
    for c in current {
        if seen.insert(key(c)) {
            out.push(c.clone());
        }
    }
    out
}

/// `scrollDeltaToReveal` (`lib/workspace-nav.ts`).
pub fn scroll_delta_to_reveal(
    row_top: f32,
    row_bottom: f32,
    view_top: f32,
    view_bottom: f32,
    top_inset: f32,
    bottom_inset: f32,
) -> f32 {
    let visible_top = view_top + top_inset;
    let visible_bottom = view_bottom - bottom_inset;
    if visible_bottom <= visible_top || row_top < visible_top {
        return row_top - visible_top;
    }
    if row_bottom > visible_bottom {
        return row_bottom - visible_bottom;
    }
    0.
}

/// `nextStripScrollLeft`: the scroll offset that puts a chip at `child_left`/`child_width`
/// (content coordinates) in view.
pub fn next_strip_scroll_left(
    scroll_left: f32,
    client_width: f32,
    scroll_width: f32,
    child_left: f32,
    child_width: f32,
) -> f32 {
    let delta = scroll_delta_to_reveal(
        child_left,
        child_left + child_width,
        scroll_left,
        scroll_left + client_width,
        0.,
        0.,
    );
    let max = (scroll_width - client_width).max(0.);
    (scroll_left + delta).clamp(0., max)
}

/// `stripScrollIsProgrammatic`: the offset is one we wrote, or a clamp to a new max.
pub fn strip_scroll_is_programmatic(scroll_left: f32, last_write: Option<f32>, max: f32) -> bool {
    let Some(written) = last_write else {
        return false;
    };
    (scroll_left - written).abs() < 1.5 || (scroll_left >= max - 1.5 && written > scroll_left + 1.5)
}

/// One tab: its stable key and chip.
pub struct TabItem {
    pub key: SharedString,
    pub chip: TabChip,
}

type Reorder = Rc<dyn Fn(Vec<SharedString>, Vec<SharedString>, &mut Window, &mut App)>;
type CanStart = Rc<dyn Fn(&App) -> bool>;

struct Press {
    key: SharedString,
    start: Point<Pixels>,
    offset: Point<Pixels>,
    size: gpui_kit::Size<Pixels>,
}

#[derive(Default)]
struct Drag {
    press: Option<Press>,
    active: bool,
    from: Option<SharedString>,
    /// Float top-left, window coordinates.
    pos: Point<Pixels>,
    pointer_x: f32,
    order: Vec<SharedString>,
    before: Vec<SharedString>,
    flips: HashMap<SharedString, (f32, Instant)>,
    settle: Option<(Point<Pixels>, Point<Pixels>, Instant)>,
    suppress_click: bool,
}

struct State {
    scroll: ScrollHandle,
    applied_restore: Option<u64>,
    last_key: Option<SharedString>,
    last_nonce: u64,
    hold_restore: bool,
    restore_frames: u8,
    restore_left: f32,
    user_pinned: bool,
    anim: Option<(f32, f32, Instant)>,
    last_write: Option<f32>,
    /// Last wheel scroll, which shows the overlay thumb.
    wheeled: Option<Instant>,
    /// What the last reveal saw: the active key, its chip's width and the chip count. The web
    /// strip reveals on a key change, a chip resize or a chip list mutation, not every frame.
    revealed: Option<(SharedString, f32, usize)>,
    drag: Drag,
}

impl Default for State {
    fn default() -> State {
        State {
            scroll: ScrollHandle::new(),
            applied_restore: None,
            last_key: None,
            last_nonce: 0,
            hold_restore: false,
            restore_frames: 0,
            restore_left: 0.,
            user_pinned: false,
            anim: None,
            wheeled: None,
            revealed: None,
            last_write: None,
            drag: Drag::default(),
        }
    }
}

const PLACEHOLDER: &str = "\u{0}__ph__";

/// `<TabStrip label chrome activeKey restoreScroll revealNonce leading trailing>`.
#[derive(IntoElement)]
pub struct TabStrip {
    id: ElementId,
    label: SharedString,
    chrome: bool,
    flush: bool,
    grow: bool,
    active_key: Option<SharedString>,
    restore_scroll: Option<(f32, u64)>,
    reveal_nonce: u64,
    scroll: Option<ScrollHandle>,
    items: Vec<TabItem>,
    leading: Vec<AnyElement>,
    trailing: Vec<AnyElement>,
    on_reorder: Option<Reorder>,
    can_start: Option<CanStart>,
}

pub fn tab_strip(id: impl Into<ElementId>, label: impl Into<SharedString>) -> TabStrip {
    TabStrip {
        id: id.into(),
        label: label.into(),
        chrome: false,
        flush: false,
        grow: false,
        active_key: None,
        restore_scroll: None,
        reveal_nonce: 0,
        scroll: None,
        items: Vec::new(),
        leading: Vec::new(),
        trailing: Vec::new(),
        on_reorder: None,
        can_start: None,
    }
}

impl TabStrip {
    /// Inside a `.obs-side-head.tabbed` header: no padding of its own, filling the row.
    pub fn flush(mut self, on: bool) -> Self {
        self.flush = on;
        self
    }

    /// Fill the row it sits in, keeping its own padding (a grid `1fr` cell: the Observatory's
    /// file tabs).
    pub fn grow(mut self, on: bool) -> Self {
        self.grow = on;
        self
    }

    /// Standalone header chrome (40px, `--pane`, bottom border): the terminal dock.
    pub fn chrome(mut self, on: bool) -> Self {
        self.chrome = on;
        self
    }
    /// The selected tab; changing it (or the nonce) reveals that chip.
    pub fn active_key(mut self, key: impl Into<SharedString>) -> Self {
        self.active_key = Some(key.into());
        self
    }
    /// Keep-alive restore: apply `left` once for `id` instead of chasing the active chip.
    pub fn restore_scroll(mut self, left: f32, id: u64) -> Self {
        self.restore_scroll = Some((left, id));
        self
    }
    /// Bump to reveal the active chip even when the key is unchanged.
    pub fn reveal_nonce(mut self, n: u64) -> Self {
        self.reveal_nonce = n;
        self
    }
    /// Share the strip's scroller so the host can read its offset (keep-alive snapshots).
    pub fn scroll_handle(mut self, h: ScrollHandle) -> Self {
        self.scroll = Some(h);
        self
    }
    pub fn tab(mut self, key: impl Into<SharedString>, chip: TabChip) -> Self {
        self.items.push(TabItem {
            key: key.into(),
            chip,
        });
        self
    }
    pub fn leading(mut self, e: impl IntoElement) -> Self {
        self.leading.push(e.into_any_element());
        self
    }
    pub fn trailing(mut self, e: impl IntoElement) -> Self {
        self.trailing.push(e.into_any_element());
        self
    }
    /// Make the tabs drag-reorderable; `f(new_order, previous)` persists the order.
    pub fn on_reorder(
        mut self,
        f: impl Fn(Vec<SharedString>, Vec<SharedString>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_reorder = Some(Rc::new(f));
        self
    }
    /// Gate drag starts (e.g. not while a rename field is open).
    pub fn can_start(mut self, f: impl Fn(&App) -> bool + 'static) -> Self {
        self.can_start = Some(Rc::new(f));
        self
    }
}

/// `.tab-strip-act`: a 26px square icon action riding the strip (+, clear, maximize, …).
pub fn tab_strip_action(
    id: impl Into<ElementId>,
    on: bool,
    disabled: bool,
    cx: &App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let t = Theme::of(cx).tokens;
    let mut d = div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(26.))
        .rounded(px(5.));
    d = if on {
        d.text_color(t.amber).bg(t.amber_d)
    } else if disabled {
        d.text_color(t.faint.with_alpha(t.faint.a * 0.5))
    } else {
        d.text_color(t.faint)
    };
    if !disabled {
        d = d
            .cursor_pointer()
            .hover(move |s| s.text_color(Hsla::from(t.text)).bg(t.pane_2));
    }
    d
}

fn chip_index(order: &[SharedString], key: &str) -> Option<usize> {
    order.iter().position(|k| k == key)
}

impl TabStrip {
    fn render_view(self, window: &mut Window, cx: &mut App) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        let shared = self.scroll.clone();
        let state = window.use_keyed_state(self.id.clone(), cx, move |_, _| State {
            scroll: shared.unwrap_or_default(),
            ..State::default()
        });
        let keys: Vec<SharedString> = self.items.iter().map(|i| i.key.clone()).collect();
        let reorderable = self.on_reorder.is_some();

        // Live drag order, or the caller's order.
        state.update(cx, |s, _| {
            if !s.drag.active && s.drag.settle.is_none() {
                s.drag.order = keys.clone();
            } else {
                s.drag.order = restore_strip_order(&keys, &s.drag.order.clone(), |k| k.clone());
            }
        });

        reveal(&state, &self, window, cx);

        let (order, from, pos, press_size, flips, settle) = {
            let s = state.read(cx);
            (
                s.drag.order.clone(),
                if s.drag.active || s.drag.settle.is_some() {
                    s.drag.from.clone()
                } else {
                    None
                },
                s.drag.pos,
                s.drag.press.as_ref().map(|p| p.size),
                s.drag.flips.clone(),
                s.drag.settle,
            )
        };
        let scroll = state.read(cx).scroll.clone();

        let mut by_key: HashMap<SharedString, TabChip> =
            self.items.into_iter().map(|i| (i.key, i.chip)).collect();
        let now_flip = |key: &SharedString, cx: &App| -> f32 {
            flips.get(key).map_or(0., |(dx, start)| {
                dx * (1. - motion::eased(*start, FLIP, Easing::SETTLE, cx))
            })
        };
        let mut floating: Option<TabChip> = None;
        let mut chips: Vec<AnyElement> = Vec::new();
        for key in &order {
            let Some(mut chip) = by_key.remove(key) else {
                continue;
            };
            let dx = now_flip(key, cx);
            if from.as_ref() == Some(key) {
                // `.tab-chip-ph`: the slot the dragged chip will land in.
                let sz = press_size.unwrap_or_default();
                let ph_dx = now_flip(&SharedString::from(PLACEHOLDER), cx);
                chips.push(
                    div()
                        .relative()
                        .left(px(ph_dx))
                        .flex_none()
                        .w(sz.width)
                        .h_full()
                        .bg(t.pane_2)
                        .child(
                            div()
                                .absolute()
                                .top(px(1.))
                                .left(px(1.))
                                .right(px(1.))
                                .bottom(px(1.))
                                .border(px(1.))
                                .border_dashed()
                                .border_color(Hsla::from(t.line)),
                        )
                        .into_any_element(),
                );
                floating = Some(chip.floating(true).reorderable(true));
                continue;
            }
            if reorderable {
                let st = state.clone();
                let k = key.clone();
                let can = self.can_start.clone();
                chip = chip
                    .reorderable(true)
                    .on_press(move |e: &MouseDownEvent, window, cx| {
                        press(&st, &k, e, can.as_ref(), window, cx)
                    });
                let st = state.clone();
                let k = key.clone();
                let reorder = self.on_reorder.clone();
                let can = self.can_start.clone();
                chip = chip.on_key_down(move |e: &KeyDownEvent, window, cx| {
                    let m = &e.keystroke.modifiers;
                    if !m.alt || m.platform || m.control || m.shift {
                        return false;
                    }
                    let dir = match e.keystroke.key.as_str() {
                        "left" => -1,
                        "right" => 1,
                        _ => return false,
                    };
                    if can.as_ref().is_some_and(|c| !c(cx)) {
                        return false;
                    }
                    move_by(&st, &k, dir, reorder.as_ref(), window, cx);
                    true
                });
            }
            let st = state.clone();
            // The click that ends a drag is not a tab click.
            chip = chip.click_guard(move |cx| {
                st.update(cx, |s, _| std::mem::take(&mut s.drag.suppress_click))
            });
            chips.push(
                div()
                    .relative()
                    .left(px(dx))
                    .flex()
                    .h_full()
                    .child(chip)
                    .into_any_element(),
            );
        }

        // Window-level pointer tracking while a press or drag is live.
        let tracker = {
            let st = state.clone();
            let reorder = self.on_reorder.clone();
            canvas(
                |_, _, _| {},
                move |bounds, _, window, cx| track(&st, bounds, reorder.clone(), window, cx),
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
        };

        let overflow = f32::from(scroll.max_offset().x) > 0.;
        let mut strip = div()
            .id((self.id.clone(), "scroll"))
            .flex_1()
            .min_w(px(if self.chrome { 80. } else { 0. }))
            .self_stretch()
            .flex()
            .items_stretch()
            .gap(px(2.))
            .overflow_x_scroll()
            .track_scroll(&scroll)
            .role(gpui_kit::accesskit::Role::TabList)
            .aria_label(self.label.clone())
            .children(chips);
        {
            let state = state.clone();
            strip = strip.on_scroll_wheel(move |_, window, cx| {
                let now = crate::motion::now(cx);
                state.update(cx, |s, _| s.wheeled = Some(now));
                window.refresh();
            });
        }
        // `scrollbar-width: thin` is a macOS overlay scroller in WebKit: no layout space, and
        // only shown while the user scrolls.
        let shown = state
            .read(cx)
            .wheeled
            .is_some_and(|at| at.elapsed() < OVERLAY_SCROLLER);
        if shown {
            window.request_animation_frame();
        }
        let thumb = (overflow && shown).then(|| scrollbar(&scroll, t.line));

        let flush = self.flush;
        let mut root = div()
            .id(self.id)
            .relative()
            .flex()
            .items_stretch()
            .gap(px(6.))
            .when(!flush, |d| d.pl(px(8.)).pr(px(4.)))
            .when(flush || self.grow, |d| d.flex_1())
            .min_w_0()
            .min_h_0()
            .child(tracker);
        if self.chrome {
            root = root
                .flex_none()
                .h(px(40.))
                .bg(t.pane)
                .border_b(px(1.))
                .border_color(Hsla::from(t.line))
                .overflow_hidden();
        }
        let side = |items: Vec<AnyElement>| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .self_center()
                .gap(px(2.))
                .children(items)
        };
        if !self.leading.is_empty() {
            root = root.child(side(self.leading));
        }
        root = root.child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .flex()
                .child(strip)
                .children(thumb),
        );
        if !self.trailing.is_empty() {
            root = root.child(side(self.trailing));
        }
        if let (Some(chip), Some(sz)) = (floating, press_size) {
            let (at, lift) = match settle {
                Some((from, to, start)) => {
                    let p = motion::eased(start, SETTLE, Easing::SETTLE, cx);
                    (from + (to - from) * p, 1. - p)
                }
                None => (pos, 1.),
            };
            let chip = chip.lift(lift);
            root = root.child(
                deferred(crate::style::anchored_at(
                    at.x.into(),
                    at.y.into(),
                    div()
                        .w(sz.width)
                        .h(sz.height)
                        .flex()
                        .items_stretch()
                        .child(chip),
                ))
                .with_priority(400),
            );
        }
        root
    }
}

impl RenderOnce for TabStrip {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        crate::guard::component(self, window, cx, Self::render_view)
    }
}

/// How long the overlay scroller stays after the last wheel scroll.
const OVERLAY_SCROLLER: Duration = Duration::from_millis(1000);

/// The thin horizontal scrollbar thumb (`scrollbar-color: var(--line)`, 4px, radius 2).
fn scrollbar(scroll: &ScrollHandle, color: crate::color::Color) -> AnyElement {
    let viewport = f32::from(scroll.bounds().size.width);
    let max = f32::from(scroll.max_offset().x);
    let content = viewport + max;
    let left = -f32::from(scroll.offset().x);
    if content <= 0. || viewport <= 0. {
        return div().into_any_element();
    }
    let thumb_w = (viewport * viewport / content).max(16.);
    let x = if max > 0. {
        left / max * (viewport - thumb_w)
    } else {
        0.
    };
    div()
        .absolute()
        .left(px(x))
        .bottom_0()
        .w(px(thumb_w))
        .h(px(4.))
        .rounded(px(2.))
        .bg(color)
        .into_any_element()
}

/// Reveal the active chip (or apply a keep-alive restore), following the `$effect`s in
/// `TabStrip.svelte` and the chase in `tab-strip-scroll.ts`.
fn reveal(state: &gpui_kit::Entity<State>, strip: &TabStrip, window: &mut Window, cx: &mut App) {
    let reduced = motion::reduced(cx);
    let order: Vec<SharedString> = state.read(cx).drag.order.clone();
    let now = crate::motion::now(cx);
    state.update(cx, |s, _| {
        let scroll_left = -f32::from(s.scroll.offset().x);
        let max = f32::from(s.scroll.max_offset().x);
        // A scroll we did not write pins the strip and cancels any chase.
        if s.last_write.is_some() && !strip_scroll_is_programmatic(scroll_left, s.last_write, max) {
            s.user_pinned = true;
            s.anim = None;
            s.last_write = None;
        }
        let Some(key) = strip.active_key.clone().filter(|k| !k.is_empty()) else {
            return;
        };
        let user_reveal = strip.reveal_nonce != s.last_nonce;
        s.last_nonce = strip.reveal_nonce;
        if let Some((left, id)) = strip.restore_scroll
            && s.applied_restore != Some(id)
            && !user_reveal
        {
            s.applied_restore = Some(id);
            s.hold_restore = true;
            s.user_pinned = true;
            s.last_key = Some(key.clone());
            s.restore_left = left;
            s.restore_frames = 3;
            s.anim = None;
        }
        if s.restore_frames > 0 {
            // Re-apply while this workspace's chips land (a frame or two after the key).
            s.restore_frames -= 1;
            s.scroll.set_offset(point(px(-s.restore_left), px(0.)));
            s.last_write = Some(s.restore_left);
            return;
        }
        if (s.hold_restore || s.user_pinned) && !user_reveal && Some(&key) == s.last_key.as_ref() {
            return;
        }
        s.hold_restore = false;
        s.user_pinned = false;
        s.last_key = Some(key.clone());
        if s.drag.active {
            return;
        }
        let Some(ix) = chip_index(&order, &key) else {
            return;
        };
        let (Some(child), true) = (
            s.scroll.bounds_for_item(ix),
            s.scroll.bounds().size.width > px(0.),
        ) else {
            return;
        };
        let seen = (key.clone(), f32::from(child.size.width), order.len());
        let same = s
            .revealed
            .as_ref()
            .is_some_and(|(k, w, n)| *k == seen.0 && (w - seen.1).abs() < 0.5 && *n == seen.2);
        if same && !user_reveal {
            return;
        }
        s.revealed = Some(seen);
        let view = s.scroll.bounds();
        let client = f32::from(view.size.width);
        // Scroll children report their unscrolled (content) bounds.
        let child_left = f32::from(child.left() - view.left());
        let next = next_strip_scroll_left(
            scroll_left,
            client,
            client + max,
            child_left,
            child.size.width.into(),
        )
        // WebKit keeps scroll offsets in whole CSS pixels (`scrollLeft = x` truncates).
        .trunc();
        if (next - scroll_left).abs() < 1. {
            if s.anim.is_some_and(|(_, to, _)| (to - next).abs() >= 1.) {
                s.anim = None;
            }
            return;
        }
        if reduced {
            s.scroll.set_offset(point(px(-next), px(0.)));
            s.last_write = Some(next);
        } else if s.anim.is_none_or(|(_, to, _)| (to - next).abs() >= 1.) {
            s.anim = Some((scroll_left, next, now));
        }
    });
    let anim = state.read(cx).anim;
    if let Some((from, to, start)) = anim {
        let p = motion::progress(start, SMOOTH, cx);
        let eased = 1. - (1. - p).powi(3);
        let left = from + (to - from) * eased;
        state.update(cx, |s, _| {
            s.scroll.set_offset(point(px(-left), px(0.)));
            s.last_write = Some(left);
            if p >= 1. {
                s.anim = None;
            }
        });
        if p < 1. {
            window.request_animation_frame();
        }
    }
}

fn press(
    state: &gpui_kit::Entity<State>,
    key: &SharedString,
    e: &MouseDownEvent,
    can_start: Option<&CanStart>,
    window: &mut Window,
    cx: &mut App,
) {
    if can_start.is_some_and(|c| !c(cx)) {
        return;
    }
    state.update(cx, |s, _| {
        s.drag.suppress_click = false;
        if s.drag.active || s.drag.settle.is_some() {
            return;
        }
        let Some(ix) = chip_index(&s.drag.order, key) else {
            return;
        };
        let Some(b) = item_bounds(s, ix) else {
            return;
        };
        s.drag.press = Some(Press {
            key: key.clone(),
            start: e.position,
            offset: e.position - b.origin,
            size: b.size,
        });
    });
    window.refresh();
}

/// A chip's bounds where it shows (scroll children report unscrolled bounds).
fn item_bounds(s: &State, ix: usize) -> Option<gpui_kit::Bounds<gpui_kit::Pixels>> {
    let mut b = s.scroll.bounds_for_item(ix)?;
    b.origin += s.scroll.offset();
    Some(b)
}

/// Lefts of the rendered chips (and placeholder), in window coordinates.
fn chip_lefts(s: &State) -> HashMap<SharedString, f32> {
    let mut out = HashMap::new();
    for (i, key) in s.drag.order.iter().enumerate() {
        if let Some(b) = item_bounds(s, i) {
            let k = if s.drag.from.as_ref() == Some(key) {
                SharedString::from(PLACEHOLDER)
            } else {
                key.clone()
            };
            out.insert(k, f32::from(b.left()));
        }
    }
    out
}

fn reorder_by_pointer_x(s: &mut State, x: f32, now: Instant) {
    let Some(from) = s.drag.from.clone() else {
        return;
    };
    let Some(from_ix) = chip_index(&s.drag.order, &from) else {
        return;
    };
    let mut mids = Vec::new();
    let mut widths: HashMap<SharedString, f32> = HashMap::new();
    for (i, key) in s.drag.order.iter().enumerate() {
        let Some(b) = item_bounds(s, i) else {
            return;
        };
        widths.insert(key.clone(), b.size.width.into());
        if *key != from {
            mids.push(f32::from(b.left()) + f32::from(b.size.width) / 2.);
        }
    }
    let insert_at = insert_index_from_pointer_x(&mids, x);
    let next = reorder_by_insert(&s.drag.order, from_ix as isize, insert_at);
    if next == s.drag.order {
        return;
    }
    // FLIP: slide every moved chip from where it was.
    let before = chip_lefts(s);
    let start_x = item_bounds(s, 0).map_or(0., |b| f32::from(b.left()));
    let mut x0 = start_x;
    for key in &next {
        let k = if *key == from {
            SharedString::from(PLACEHOLDER)
        } else {
            key.clone()
        };
        if let Some(old) = before.get(&k) {
            let dx = old - x0;
            if dx.abs() >= 0.5 {
                s.drag.flips.insert(k, (dx, now));
            }
        }
        x0 += widths.get(key).copied().unwrap_or(0.) + 2.;
    }
    s.drag.order = next;
}

fn move_by(
    state: &gpui_kit::Entity<State>,
    key: &SharedString,
    dir: isize,
    reorder: Option<&Reorder>,
    window: &mut Window,
    cx: &mut App,
) {
    let (next, prev) = {
        let s = state.read(cx);
        if s.drag.active || s.drag.settle.is_some() {
            return;
        }
        let list = s.drag.order.clone();
        let Some(from) = chip_index(&list, key) else {
            return;
        };
        let to = (from as isize + dir).max(0) as usize;
        (reorder_by_insert(&list, from as isize, to), list)
    };
    if next == prev {
        return;
    }
    state.update(cx, |s, _| s.drag.order = next.clone());
    if let Some(f) = reorder {
        f(next, prev, window, cx);
    }
}

/// Paint-phase listeners for a live press/drag: threshold, follow, reorder, edge scroll,
/// release, settle and commit.
fn track(
    state: &gpui_kit::Entity<State>,
    strip_bounds: Bounds<Pixels>,
    reorder: Option<Reorder>,
    window: &mut Window,
    cx: &mut App,
) {
    let (pressed, active, settle, pointer_x) = {
        let s = state.read(cx);
        (
            s.drag.press.is_some(),
            s.drag.active,
            s.drag.settle,
            s.drag.pointer_x,
        )
    };
    // Finish the settle animation, then persist.
    if let Some((_, _, start)) = settle {
        if motion::progress(start, SETTLE, cx) >= 1. {
            let (order, before) = state.update(cx, |s, _| {
                s.drag.settle = None;
                s.drag.active = false;
                s.drag.press = None;
                s.drag.from = None;
                (s.drag.order.clone(), std::mem::take(&mut s.drag.before))
            });
            if order != before
                && let Some(f) = &reorder
            {
                f(order, before, window, cx);
            }
            window.refresh();
        } else {
            window.request_animation_frame();
        }
        return;
    }
    // Clear finished FLIPs.
    let has_flips = state.update(cx, |s, _| {
        s.drag.flips.retain(|_, (_, t)| t.elapsed() < FLIP);
        !s.drag.flips.is_empty()
    });
    if has_flips {
        window.request_animation_frame();
    }
    if !pressed {
        return;
    }
    if active {
        window.set_window_cursor_style(CursorStyle::ClosedHand);
        // Edge auto-scroll while the pointer sits in a 28px band.
        let left = f32::from(strip_bounds.left());
        let right = f32::from(strip_bounds.right());
        let step = if pointer_x < left + EDGE {
            -EDGE_STEP
        } else if pointer_x > right - EDGE {
            EDGE_STEP
        } else {
            0.
        };
        if step != 0. {
            let now = crate::motion::now(cx);
            state.update(cx, |s, _| {
                let cur = -f32::from(s.scroll.offset().x);
                let max = f32::from(s.scroll.max_offset().x);
                let next = (cur + step).clamp(0., max);
                s.scroll.set_offset(point(px(-next), px(0.)));
                s.last_write = Some(next);
                reorder_by_pointer_x(s, pointer_x, now);
            });
            window.request_animation_frame();
        }
    }
    let st = state.clone();
    window.on_mouse_event(move |e: &MouseMoveEvent, _, window, cx| {
        let now = crate::motion::now(cx);
        let changed = st.update(cx, |s, _| {
            let Some(p) = &s.drag.press else { return false };
            if !s.drag.active {
                let d = e.position - p.start;
                if f32::from(d.x).hypot(f32::from(d.y)) < DRAG_THRESHOLD {
                    return false;
                }
                s.drag.active = true;
                s.drag.from = Some(p.key.clone());
                s.drag.before = s.drag.order.clone();
                s.drag.suppress_click = true;
            }
            s.drag.pos = e.position - p.offset;
            s.drag.pointer_x = e.position.x.into();
            reorder_by_pointer_x(s, e.position.x.into(), now);
            true
        });
        if changed {
            window.refresh();
        }
    });
    let st = state.clone();
    window.on_mouse_event(move |e: &MouseUpEvent, _, window, cx| {
        if e.button != MouseButton::Left {
            return;
        }
        let now = crate::motion::now(cx);
        st.update(cx, |s, _| {
            if !s.drag.active {
                s.drag.press = None;
                s.drag.from = None;
                return;
            }
            // Settle the float onto the placeholder.
            let target = s
                .drag
                .from
                .as_ref()
                .and_then(|k| chip_index(&s.drag.order, k))
                .and_then(|i| item_bounds(s, i))
                .map_or(s.drag.pos, |b| b.origin);
            s.drag.settle = Some((s.drag.pos, target, now));
        });
        window.refresh();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reorder_by_insert_moves_into_gap() {
        let l = ["a", "b", "c"];
        assert_eq!(reorder_by_insert(&l, 2, 0), ["c", "a", "b"]);
        assert_eq!(reorder_by_insert(&l, 0, 2), ["b", "c", "a"]);
        assert_eq!(reorder_by_insert(&l, 1, 1), ["a", "b", "c"]);
        assert_eq!(reorder_by_insert(&["a", "b"], 0, 9), ["b", "a"]);
        assert_eq!(reorder_by_insert(&["a", "b"], -1, 0), ["a", "b"]);
        assert_eq!(reorder_by_insert(&["a", "b"], 2, 0), ["a", "b"]);
    }

    #[test]
    fn insert_index_from_midpoints() {
        assert_eq!(insert_index_from_pointer_x(&[10., 30., 50.], 5.), 0);
        assert_eq!(insert_index_from_pointer_x(&[10., 30., 50.], 30.), 2);
        assert_eq!(insert_index_from_pointer_x(&[10., 30., 50.], 29.), 1);
        assert_eq!(insert_index_from_pointer_x(&[10., 30., 50.], 80.), 3);
        assert_eq!(insert_index_from_pointer_x(&[], 40.), 0);
    }

    #[test]
    fn restore_order_keeps_live_keys() {
        let k = |s: &&str| s.to_string();
        assert_eq!(
            restore_strip_order(&["a", "c", "b"], &["c", "a", "b"], k),
            ["c", "a", "b"]
        );
        assert_eq!(
            restore_strip_order(&["a", "d"], &["c", "a", "b"], k),
            ["a", "d"]
        );
        assert_eq!(
            restore_strip_order(&["c", "a", "b", "d"], &["c", "a", "b"], k),
            ["c", "a", "b", "d"]
        );
    }

    #[test]
    fn next_scroll_left_like_tab_strip_scroll() {
        assert_eq!(next_strip_scroll_left(80., 200., 600., 100., 60.), 80.);
        assert_eq!(next_strip_scroll_left(80., 200., 600., 20., 40.), 20.);
        assert_eq!(next_strip_scroll_left(0., 200., 600., 180., 40.), 20.);
        assert_eq!(next_strip_scroll_left(0., 200., 250., 200., 80.), 50.);
        assert_eq!(next_strip_scroll_left(0., 400., 200., 0., 80.), 0.);
    }

    #[test]
    fn programmatic_scroll_detection() {
        assert!(!strip_scroll_is_programmatic(10., None, 100.));
        assert!(strip_scroll_is_programmatic(40.5, Some(40.), 300.));
        assert!(!strip_scroll_is_programmatic(60., Some(40.), 300.));
        // A between-frame clamp to the new max reads as ours.
        assert!(strip_scroll_is_programmatic(200., Some(280.), 200.));
        // A user scroll to the max (written below it) does not.
        assert!(!strip_scroll_is_programmatic(200., Some(150.), 200.));
    }
}
