//! Text fields as isolated views: a key renders the field alone while the frame replays
//! everything around it (`style::isolated_view`, Recur's gpui-pre patch in
//! `rust/vendor/README.md`).

use gpui_kit::base::input::{InputBaseState, InputModeKind};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, Render, StyleRefinement,
    Styled as _, Window, div, px,
};

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
