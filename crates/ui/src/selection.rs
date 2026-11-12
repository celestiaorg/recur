//! Text selection across text elements, as a page selects across its DOM.
//!
//! A [`Scope`] wraps a surface (a diff's card stack, a transcript, a markdown document). Every
//! [`crate::text::Text`] made `selectable` inside it registers its laid-out lines with the
//! scope as it prepaints. The scope turns presses and drags into a selection (anchor and focus,
//! each an item and a byte offset; double click selects a word, triple click the item, Shift
//! extends), and each item paints its share in the app's `::selection` color under its glyphs.
//! ⌘C copies the selected text of each item in document order, each followed by the separator
//! it declares when more follows (`"\n"` after a cell or row, as WebKit copies grid items,
//! `"\n\n"` after a paragraph with a bottom margin); ⌘A selects all.
//!
//! The scope collects its items afresh each time it prepaints, and keeps them while a cached
//! view replays it (a replay reuses the same geometry), or while a cached view inside it
//! replays (`Window::replayed_view`, vendored gpui-pre): a scope can span cached views, a
//! transcript's session cards or a PR conversation's items.
//! Document order is geometric: top to bottom, then left to right, which is DOM order for the
//! surfaces that use it. When the selection changes, the views holding its items are
//! notified, so a replayed paint never shows a stale highlight.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, Bounds, ClipboardItem, CursorStyle, Element, ElementId, Entity, EntityId,
    FocusHandle, GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId, InteractiveElement,
    IntoElement, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, Pixels, Point, ScrollHandle, Styled, Window, div, px,
};

/// What a text element gives the scope: its geometry and text, from its current layout.
pub trait Geometry {
    /// The item's text; offsets are byte offsets into it.
    fn text(&self) -> String;
    /// The nearest offset to `p` (window coordinates).
    fn offset_at(&self, p: Point<Pixels>) -> usize;
    /// The rectangles covering `range`, one per line.
    fn rects(&self, range: Range<usize>) -> Vec<Bounds<Pixels>>;
}

struct Item {
    bounds: Bounds<Pixels>,
    sep: &'static str,
    view: Option<EntityId>,
    /// The views drawing the item when it registered, outermost first.
    stack: Vec<EntityId>,
    /// The scope's pass that registered it last.
    pass: u64,
    geom: Rc<dyn Geometry>,
    hitbox: Hitbox,
    /// The scroller clipping the item, which a drag from it scrolls toward the pointer.
    scroller: Option<ScrollHandle>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pos {
    key: u64,
    offset: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Granularity {
    Char,
    Word,
    Item,
}

#[derive(Default)]
pub struct State {
    items: HashMap<u64, Item>,
    /// Counts the scope's prepaints; an item from an earlier pass stays while a cached view
    /// holding it replays (its elements do not prepaint, so it does not register again).
    pass: u64,
    anchor: Option<Pos>,
    focus: Option<Pos>,
    /// The anchor's span for word and item granularity (the drag extends from it).
    anchor_span: Option<(Pos, Pos)>,
    drag: Option<Granularity>,
    /// Where the drag's pointer last moved to; `None` until it moves, so a click never scrolls.
    pointer: Option<Point<Pixels>>,
    /// Whether the press landed inside the anchor's scroller; a drag from beside it never
    /// scrolls it, as in browsers.
    pressed_in_scroller: bool,
    disabled: bool,
    handle: Option<FocusHandle>,
    /// Each selected item's range, computed once per change of items or selection.
    ranges: RefCell<Option<HashMap<u64, Range<usize>>>>,
}

thread_local! {
    static STACK: RefCell<Vec<Entity<State>>> = const { RefCell::new(Vec::new()) };
}

/// Registers a selectable text element with the innermost scope (from its prepaint).
pub fn register(
    key: u64,
    sep: &'static str,
    bounds: Bounds<Pixels>,
    geom: Rc<dyn Geometry>,
    scroller: Option<ScrollHandle>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(scope) = STACK.with(|s| s.borrow().last().cloned()) else {
        return;
    };
    let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
    // Hit-test and order by the visible part, so text clipped by a scroller (a capped code
    // fence) does not claim presses over what follows it.
    let visible = bounds.intersect(&window.content_mask().bounds);
    let bounds = if visible.is_empty() { bounds } else { visible };
    let view = window.current_view();
    let stack = window.view_stack().to_vec();
    scope.update(cx, |s, _| {
        s.ranges.take();
        let pass = s.pass;
        s.items.insert(
            key,
            Item {
                bounds,
                sep,
                view: Some(view),
                stack,
                pass,
                geom,
                hitbox,
                scroller,
            },
        );
    });
}

/// The selected byte range of the item `key` in the innermost scope (from its paint).
pub fn selected(key: u64, cx: &App) -> Option<Range<usize>> {
    let scope = STACK.with(|s| s.borrow().last().cloned())?;
    scope.read(cx).range_of(key)
}

/// `::selection { background: color-mix(in srgb, var(--amber) 30%, transparent) }`.
pub fn highlight(cx: &App) -> gpui_kit::Hsla {
    crate::Theme::of(cx).tokens.amber.with_alpha(0.3).into()
}

fn order(b: &Bounds<Pixels>) -> (i64, i64) {
    (
        (f32::from(b.origin.y) * 64.).round() as i64,
        (f32::from(b.origin.x) * 64.).round() as i64,
    )
}

impl State {
    /// Live items in document order.
    fn ordered(&self) -> Vec<(u64, &Item)> {
        let mut v: Vec<(u64, &Item)> = self.items.iter().map(|(k, it)| (*k, it)).collect();
        v.sort_by_key(|(k, it)| (order(&it.bounds), *k));
        v
    }

    fn rank(&self, ordered: &[(u64, &Item)], p: Pos) -> Option<(usize, usize)> {
        ordered
            .iter()
            .position(|(k, _)| *k == p.key)
            .map(|i| (i, p.offset))
    }

    /// The selection's ends in document order, as (item index, offset).
    fn span(&self, ordered: &[(u64, &Item)]) -> Option<((usize, usize), (usize, usize))> {
        let (a, f) = (self.anchor?, self.focus?);
        let (a, f) = (self.rank(ordered, a)?, self.rank(ordered, f)?);
        Some(if a <= f { (a, f) } else { (f, a) })
    }

    fn range_of(&self, key: u64) -> Option<Range<usize>> {
        self.anchor?;
        let mut cache = self.ranges.borrow_mut();
        let ranges = cache.get_or_insert_with(|| self.all_ranges());
        ranges.get(&key).cloned()
    }

    fn all_ranges(&self) -> HashMap<u64, Range<usize>> {
        let ordered = self.ordered();
        let mut out = HashMap::new();
        let Some(((si, so), (ei, eo))) = self.span(&ordered) else {
            return out;
        };
        for (i, (key, item)) in ordered.iter().enumerate().take(ei + 1).skip(si) {
            let start = if i == si { so } else { 0 };
            let end = if i == ei { eo } else { item.geom.text().len() };
            if start < end {
                out.insert(*key, start..end);
            }
        }
        out
    }

    /// The nearest item to `p`: the one containing it, else the closest line above or below.
    fn hit(&self, p: Point<Pixels>) -> Option<Pos> {
        let ordered = self.ordered();
        let dist = |b: &Bounds<Pixels>| {
            let dy = if p.y < b.top() {
                f32::from(b.top() - p.y)
            } else if p.y > b.bottom() {
                f32::from(p.y - b.bottom())
            } else {
                0.
            };
            let dx = if p.x < b.left() {
                f32::from(b.left() - p.x)
            } else if p.x > b.right() {
                f32::from(p.x - b.right())
            } else {
                0.
            };
            (dy, dx)
        };
        let (key, item) = ordered.into_iter().min_by(|a, b| {
            let (da, db) = (dist(&a.1.bounds), dist(&b.1.bounds));
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })?;
        Some(Pos {
            key,
            offset: item.geom.offset_at(p),
        })
    }

    /// The span a press at `p` selects at `g`.
    fn unit_at(&self, p: Pos, g: Granularity) -> (Pos, Pos) {
        let Some(item) = self.items.get(&p.key) else {
            return (p, p);
        };
        let text = item.geom.text();
        let r = match g {
            Granularity::Char => p.offset..p.offset,
            Granularity::Word => word_at(&text, p.offset),
            Granularity::Item => 0..text.len(),
        };
        (
            Pos {
                key: p.key,
                offset: r.start,
            },
            Pos {
                key: p.key,
                offset: r.end,
            },
        )
    }

    /// The selected text, each item's part followed by its separator when another follows.
    pub fn text(&self) -> String {
        let ordered = self.ordered();
        let Some(((si, so), (ei, eo))) = self.span(&ordered) else {
            return String::new();
        };
        let mut out = String::new();
        let mut sep = "";
        for (i, (_, item)) in ordered.iter().enumerate().take(ei + 1).skip(si) {
            let text = item.geom.text();
            let start = if i == si { so } else { 0 };
            let end = if i == ei { eo } else { text.len() };
            let part = text.get(start.min(end)..end).unwrap_or("");
            if part.is_empty() {
                continue;
            }
            if !out.is_empty() {
                out.push_str(sep);
            }
            out.push_str(part);
            sep = item.sep;
        }
        out
    }

    fn views(&self) -> Vec<EntityId> {
        let mut v: Vec<EntityId> = self.items.values().filter_map(|i| i.view).collect();
        v.sort();
        v.dedup();
        v
    }
}

/// A word (letters, digits and `_`), a run of spaces, or one other character around `at`.
fn word_at(text: &str, at: usize) -> Range<usize> {
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            0
        } else if c.is_whitespace() {
            1
        } else {
            2
        }
    };
    let at = at.min(text.len());
    let after = text[at..].chars().next();
    let before = text[..at].chars().next_back();
    let Some(c) = after.or(before) else {
        return at..at;
    };
    let k = class(c);
    let base = if after.is_some() {
        at
    } else {
        at - c.len_utf8()
    };
    if k == 2 {
        return base..base + c.len_utf8();
    }
    let start = text[..base]
        .char_indices()
        .rev()
        .take_while(|(_, ch)| class(*ch) == k)
        .last()
        .map_or(base, |(i, _)| i);
    let end = text[base..]
        .char_indices()
        .find(|(_, ch)| class(*ch) != k)
        .map_or(text.len(), |(i, _)| base + i);
    start..end
}

/// The scope's state (keyed by `id` in the current view).
pub fn state(id: impl Into<ElementId>, window: &mut Window, cx: &mut App) -> Entity<State> {
    window.use_keyed_state(id, cx, |_, cx| State {
        handle: Some(cx.focus_handle()),
        ..State::default()
    })
}

/// A surface whose selectable text selects across elements. `disabled` stops new selections
/// (`.diff-file.selecting { user-select: none }` while a range of lines is dragged).
pub fn scope(
    state: &Entity<State>,
    disabled: bool,
    child: impl IntoElement,
    cx: &mut App,
) -> AnyElement {
    let focus = state.update(cx, |s, _| {
        s.disabled = disabled;
        s.handle.clone()
    });
    let (down, keys) = (state.clone(), state.clone());
    let mut d = div().size_full().child(Host {
        state: state.clone(),
        child: child.into_any_element(),
    });
    if let Some(f) = &focus {
        d = d.track_focus(f);
    }
    d.on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, window, cx| {
        press(&down, e, window, cx);
    })
    .on_key_down(move |e: &KeyDownEvent, window, cx| {
        let k = &e.keystroke;
        if !k.modifiers.secondary(cx) || k.modifiers.shift || k.modifiers.alt {
            return;
        }
        match k.key.as_str() {
            "c" => {
                let text = keys.read(cx).text();
                if !text.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                    cx.stop_propagation();
                }
            }
            "a" => {
                select_all(&keys, cx);
                window.refresh();
                cx.stop_propagation();
            }
            _ => {}
        }
    })
    .into_any_element()
}

fn notify_views(views: &[EntityId], cx: &mut App) {
    for v in views {
        cx.notify(*v);
    }
}

fn select_all(state: &Entity<State>, cx: &mut App) {
    let views = state.update(cx, |s, _| {
        s.ranges.take();
        let ordered = s.ordered();
        if let (Some((first, _)), Some((last, item))) = (ordered.first(), ordered.last()) {
            let (first, last, len) = (*first, *last, item.geom.text().len());
            s.anchor = Some(Pos {
                key: first,
                offset: 0,
            });
            s.focus = Some(Pos {
                key: last,
                offset: len,
            });
        }
        s.views()
    });
    notify_views(&views, cx);
}

fn press(state: &Entity<State>, e: &MouseDownEvent, window: &mut Window, cx: &mut App) {
    if state.read(cx).disabled {
        return;
    }
    let g = match e.click_count {
        0 | 1 => Granularity::Char,
        2 => Granularity::Word,
        _ => Granularity::Item,
    };
    let views = state.update(cx, |s, _| {
        s.ranges.take();
        let before = s.views();
        let Some(p) = s.hit(e.position) else {
            s.anchor = None;
            s.focus = None;
            return before;
        };
        let (a, f) = s.unit_at(p, g);
        if e.modifiers.shift && s.anchor.is_some() && g == Granularity::Char {
            s.focus = Some(p);
        } else {
            s.anchor = Some(a);
            s.focus = Some(f);
            s.anchor_span = Some((a, f));
        }
        s.drag = Some(g);
        s.pointer = None;
        s.pressed_in_scroller = s
            .anchor
            .and_then(|a| s.items.get(&a.key)?.scroller.as_ref())
            .is_some_and(|h| h.bounds().contains(&e.position));
        before
    });
    if let Some(f) = state.read(cx).handle.clone() {
        window.focus(&f, cx);
        // Hosts that park focus on mouse down leave it with the scope.
        if state.read(cx).anchor.is_some() {
            window.prevent_default();
        }
    }
    notify_views(&views, cx);
    window.refresh();
}

fn drag_to(state: &Entity<State>, p: Point<Pixels>, cx: &mut App) {
    let views = state.update(cx, |s, _| {
        s.ranges.take();
        s.pointer = Some(p);
        let (Some(g), Some((a0, a1))) = (s.drag, s.anchor_span) else {
            return Vec::new();
        };
        let Some(hit) = s.hit(p) else {
            return Vec::new();
        };
        let (u0, u1) = s.unit_at(hit, g);
        let ordered = s.ordered();
        let before_anchor = match (s.rank(&ordered, u0), s.rank(&ordered, a0)) {
            (Some(x), Some(y)) => x < y,
            _ => false,
        };
        // Extending past the anchor's unit keeps the whole unit selected, as WebKit does.
        let (anchor, focus) = if before_anchor { (a1, u0) } else { (a0, u1) };
        if s.anchor == Some(anchor) && s.focus == Some(focus) {
            return Vec::new();
        }
        s.anchor = Some(anchor);
        s.focus = Some(focus);
        s.views()
    });
    notify_views(&views, cx);
}

/// Where a drag scrolls the anchor's scroller this frame: toward the pointer, faster the
/// further past the scroller's edge it is. `None` when the pointer is inside or the scroller
/// is already at its end on that side.
fn autoscroll_delta(state: &State) -> Option<(ScrollHandle, Point<Pixels>, Option<EntityId>)> {
    state.drag?;
    if !state.pressed_in_scroller {
        return None;
    }
    let p = state.pointer?;
    let item = state.items.get(&state.anchor?.key)?;
    let handle = item.scroller.clone()?;
    let (b, offset, max) = (handle.bounds(), handle.offset(), handle.max_offset());
    let axis = |v: Pixels, lo: Pixels, hi: Pixels, offset: Pixels, max: Pixels| {
        let past = if v < lo {
            f32::from(v - lo)
        } else if v > hi {
            f32::from(v - hi)
        } else {
            return offset;
        };
        let step = past.signum() * (past.abs() * 0.25 + 2.);
        (offset - px(step)).clamp(-max.abs(), px(0.))
    };
    let next = Point {
        x: axis(p.x, b.left(), b.right(), offset.x, max.x),
        y: axis(p.y, b.top(), b.bottom(), offset.y, max.y),
    };
    (next != offset).then_some((handle, next, item.view))
}

/// Clears the selection (a press outside the scope).
pub fn clear(state: &Entity<State>, cx: &mut App) {
    let views = state.update(cx, |s, _| {
        s.ranges.take();
        if s.anchor.is_none() {
            return Vec::new();
        }
        s.anchor = None;
        s.focus = None;
        s.views()
    });
    notify_views(&views, cx);
}

/// Wraps the scope's content: items register and paint inside it, and a drag follows the
/// pointer anywhere in the window.
struct Host {
    state: Entity<State>,
    child: AnyElement,
}

impl IntoElement for Host {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Host {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        // A flex column, so a pane inside that fills its parent (`flex_1`) still does.
        let mut style = gpui_kit::Style {
            display: gpui_kit::Display::Flex,
            flex_direction: gpui_kit::FlexDirection::Column,
            ..Default::default()
        };
        style.size.width = gpui_kit::relative(1.).into();
        style.size.height = gpui_kit::relative(1.).into();
        let child = self.child.request_layout(window, cx);
        (window.request_layout(style, [child], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.state.update(cx, |s, _| s.pass += 1);
        STACK.with(|s| s.borrow_mut().push(self.state.clone()));
        self.child.prepaint(window, cx);
        STACK.with(|s| s.borrow_mut().pop());
        // Keep what a replayed cached view holds (same bounds, so the same geometry), drop
        // the rest of the last pass: a view that rendered registered its items again.
        self.state.update(cx, |s, _| {
            let pass = s.pass;
            let rendered: std::collections::HashSet<EntityId> = s
                .items
                .values()
                .filter(|it| it.pass == pass)
                .filter_map(|it| it.view)
                .collect();
            let before = s.items.len();
            s.items.retain(|_, it| {
                it.pass == pass
                    || (!it.view.is_some_and(|v| rendered.contains(&v))
                        && it.stack.iter().any(|v| window.replayed_view(*v)))
            });
            if s.items.len() != before {
                s.ranges.take();
            }
        });
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        // Before the children paint, so a link or button over the text keeps its own cursor.
        let s = self.state.read(cx);
        if !s.disabled {
            for it in s.items.values() {
                window.set_cursor_style(CursorStyle::IBeam, &it.hitbox);
            }
        }
        STACK.with(|s| s.borrow_mut().push(self.state.clone()));
        self.child.paint(window, cx);
        STACK.with(|s| s.borrow_mut().pop());
        let state = self.state.clone();
        if state.read(cx).drag.is_some() {
            let st = state.clone();
            window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                if phase.bubble() && e.pressed_button == Some(MouseButton::Left) {
                    drag_to(&st, e.position, cx);
                    window.refresh();
                }
            });
            let st = state.clone();
            window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                if phase.bubble() {
                    st.update(cx, |s, _| s.drag = None);
                }
            });
            if autoscroll_delta(state.read(cx)).is_some() {
                let st = state.clone();
                window.on_next_frame(move |window, cx| {
                    let Some((handle, next, view)) = autoscroll_delta(st.read(cx)) else {
                        return;
                    };
                    handle.set_offset(next);
                    if let Some(v) = view {
                        cx.notify(v);
                    }
                    if let Some(p) = st.read(cx).pointer {
                        drag_to(&st, p, cx);
                    }
                    window.refresh();
                });
            }
        }
        if state.read(cx).anchor.is_some() {
            window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                if phase.capture() && !bounds.contains(&e.position) {
                    clear(&state, cx);
                    window.refresh();
                }
            });
        }
    }
}

/// Specs read the selected text.
pub fn selected_text(state: &Entity<State>, cx: &App) -> String {
    state.read(cx).text()
}

impl State {
    /// Test hook: whether anything is selected.
    pub fn has_selection(&self) -> bool {
        matches!((self.anchor, self.focus), (Some(a), Some(f)) if a != f)
    }
}

#[cfg(test)]
mod tests {
    use super::word_at;

    #[test]
    fn words_are_runs_of_word_characters_spaces_or_one_symbol() {
        let s = "  let foo_bar = x.y;";
        assert_eq!(&s[word_at(s, 7)], "foo_bar");
        assert_eq!(&s[word_at(s, 1)], "  ");
        assert_eq!(&s[word_at(s, 14)], "=");
        assert_eq!(&s[word_at(s, s.len())], ";");
        assert_eq!(word_at("", 0), 0..0);
    }
}
