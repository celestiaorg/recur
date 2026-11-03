//! Text fields as isolated views: a key renders the field alone while the frame replays
//! everything around it (`style::isolated_view`, Recur's gpui-pre patch in
//! `vendor/README.md`).

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::base::input::{InputBaseState, InputEvent, InputModeKind, Textarea, TextareaState};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EntityId, IntoElement, ParentElement as _,
    Render, StyleRefinement, Styled as _, Subscription, Window, canvas, div, px,
};

use crate::guard::Guarded as _;

type Content = Box<dyn Fn(&mut Window, &mut App) -> AnyElement>;

/// A text field as its own isolated view. `content` builds the field (`Textarea::new(&area)`,
/// `Input::new(&input)`); the box around it (padding, focus-on-press, key context) stays with
/// the host, which lays the field out at a size that does not depend on its content
/// ([`textarea_style`]). Anything the host derives from the field (a button enabled by a
/// non-empty value, the rows of a growing textarea) is the host's to redraw: notify it from
/// the field's change event when that changes.
pub struct FieldView {
    content: Content,
}

impl FieldView {
    pub fn new(
        content: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
        cx: &mut App,
    ) -> Entity<FieldView> {
        cx.new(|_| FieldView {
            content: Box::new(content),
        })
    }

    /// The field, isolated, laid out at `style`.
    pub fn element(this: &Entity<Self>, style: StyleRefinement, cx: &App) -> AnyElement {
        crate::style::isolated_view(this, style, cx)
    }

    fn render_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        (self.content)(window, cx)
    }
}

impl Render for FieldView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::guard::view(self, window, cx, Self::render_view)
    }
}

/// How many rows a growing textarea takes for its current text (gpui-base's auto-grow: its
/// wrapped rows within `min..=max`). The wrap count follows each edit before layout.
pub fn textarea_rows<M: InputModeKind>(
    area: &Entity<InputBaseState<M>>,
    (min, max): (usize, usize),
    cx: &App,
) -> usize {
    let rows = area.read(cx).display_map().wrap_row_count();
    rows.clamp(min.max(1), max.max(1))
}

/// A gpui-base textarea's own box (`flex_1`, full height, at least `rows` lines of
/// `line_height`: its text element's layout), as the style of the isolated view holding it.
pub fn textarea_style(rows: usize, line_height: f32) -> StyleRefinement {
    let mut d = div()
        .w_full()
        .flex_1()
        .h_full()
        .min_h(px(rows as f32 * line_height));
    d.style().clone()
}

/// A growing gpui-base textarea as an isolated field. Its host lays it out with
/// [`Self::element`] and redraws only when what it derives changes: whether the value is
/// empty (submit buttons) and the rows (the box's height).
pub struct IsolatedArea {
    view: Entity<FieldView>,
    area: Entity<TextareaState>,
    rows: (usize, usize),
    redraws: Rc<Cell<Option<EntityId>>>,
    _sub: Subscription,
}

impl IsolatedArea {
    /// `area` (auto-growing within `rows`), rendered by the view of `cx`.
    pub fn new<H: 'static>(
        area: &Entity<TextareaState>,
        rows: (usize, usize),
        cx: &mut Context<H>,
    ) -> Self {
        let view = {
            let area = area.clone();
            FieldView::new(move |_, _| Textarea::new(&area).into_any_element(), cx)
        };
        let shown = Cell::new(None);
        let redraws: Rc<Cell<Option<EntityId>>> = Rc::default();
        let target = redraws.clone();
        let sub = cx.guarded_subscribe(area, move |_, area, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                let now = (
                    area.read(cx).value().trim().is_empty(),
                    textarea_rows(&area, rows, cx),
                );
                if shown.replace(Some(now)) != Some(now) {
                    match target.get() {
                        Some(part) => gpui_kit::App::notify(cx, part),
                        None => cx.notify(),
                    }
                }
            }
        });
        IsolatedArea {
            view,
            area: area.clone(),
            rows,
            redraws,
            _sub: sub,
        }
    }

    /// Notify `part` (the [`Part`] drawing what derives from the field) instead of the host:
    /// a host notify also wakes everything observing the host (its cached panes, the items
    /// of a conversation).
    pub fn redraws(&self, part: EntityId) {
        self.redraws.set(Some(part));
    }

    /// The textarea, in a box whose text has `line_height`.
    pub fn element(&self, line_height: f32, cx: &App) -> AnyElement {
        let rows = textarea_rows(&self.area, self.rows, cx);
        FieldView::element(&self.view, textarea_style(rows, line_height), cx)
    }
}

/// The height an isolated view's content measured, shared with the host that lays it out:
/// for a view whose height follows content the host cannot compute (the chat's dock).
#[derive(Default)]
pub struct Fit {
    height: Cell<Option<f32>>,
    host: Cell<Option<EntityId>>,
}

impl Fit {
    /// The isolated view's content, measured; the host redraws when its height changes. The
    /// first measure is the height the host just laid it out at in place.
    pub fn content(self: &Rc<Self>, child: impl IntoElement) -> gpui_kit::Div {
        let fit = self.clone();
        let probe = canvas(
            move |b, _, cx| {
                let h = f32::from(b.size.height);
                if fit.height.replace(Some(h)).is_some_and(|was| was != h)
                    && let Some(host) = fit.host.get()
                {
                    cx.defer(move |cx| cx.notify(host));
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        // Bottom-anchored: in the frame the content's height changes, before the host lays
        // the view out again, it grows or shrinks at its top edge, so what sits at its
        // bottom stays put.
        div().size_full().flex().flex_col().justify_end().child(
            div()
                .relative()
                .w_full()
                .flex_none()
                .child(child)
                .child(probe),
        )
    }

    /// The host's side: `entity` isolated, full width, at its content's measured height. Until
    /// it has measured (a new view) it is laid out in place at its content's height, so its
    /// first frame is not drawn at a guess.
    pub fn view<V: Render>(
        self: &Rc<Self>,
        entity: &Entity<V>,
        window: &Window,
        cx: &App,
    ) -> AnyElement {
        self.host.set(Some(window.current_view()));
        let Some(h) = self.height.get() else {
            return div()
                .w_full()
                .flex_none()
                .child(entity.clone())
                .into_any_element();
        };
        let mut style = div().w_full().flex_none().h(px(h));
        crate::style::isolated_view(entity, style.style().clone(), cx)
    }
}

/// A region of `owner` as its own uncached view: rendered whenever `owner` renders, and alone
/// notifiable. Notifying it draws a frame that renders it and the views above it without
/// waking what observes `owner` ([`IsolatedArea::redraws`]). It adds no layout node.
pub struct Part<V: 'static> {
    owner: gpui_kit::WeakEntity<V>,
    body: fn(&mut V, &mut Window, &mut Context<V>) -> AnyElement,
}

impl<V: 'static> Part<V> {
    /// A part for `owner`'s region `body`, from `owner`'s own context.
    pub fn of(
        owner: &mut Context<V>,
        body: fn(&mut V, &mut Window, &mut Context<V>) -> AnyElement,
    ) -> Entity<Self> {
        let owner_ref = owner.entity().downgrade();
        owner.new(|_| Part {
            owner: owner_ref,
            body,
        })
    }

    fn render_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let body = self.body;
        match self.owner.upgrade() {
            Some(owner) => owner.update(cx, |o, cx| body(o, window, cx)),
            None => div().into_any_element(),
        }
    }
}

impl<V: 'static> Render for Part<V> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::guard::view(self, window, cx, Self::render_view)
    }
}
