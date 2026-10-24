//! `ResizeHandle.svelte`: the 6px drag strip on a panel edge (`.vsplit` / `.hsplit`). The
//! panel store stays with the caller: the handle reports sizes, the caller clamps and keeps
//! them.

use crate::style::is_focus_visible;
use crate::theme::Theme;
use crate::tooltip::titled;
use gpui_kit::{
    App, CursorStyle, DispatchPhase, ElementId, FocusHandle, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, MouseMoveEvent, MouseUpEvent, ParentElement, RenderOnce,
    StatefulInteractiveElement, Styled, Window, canvas, deferred, div, px,
};
use std::rc::Rc;

/// Which way the handle drags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Width (`.vsplit`).
    X,
    /// Height (`.hsplit`, the terminal dock).
    Y,
}

/// Which edge of the panel the handle sits on. `End` grows the panel when dragging right (or
/// down); `Start` grows it when dragging left (or up).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Start,
    End,
}

/// The size a drag asks for: start size plus pointer travel toward the growing side, capped
/// at `max` (the layout-aware ceiling the caller computed at press).
pub fn drag_size(edge: Edge, start_size: f32, start: f32, pointer: f32, max: f32) -> f32 {
    let delta = pointer - start;
    let want = match edge {
        Edge::End => start_size + delta,
        Edge::Start => start_size - delta,
    };
    want.min(max)
}

/// Keyboard step: ±8px, ±24px with Shift, toward the growing side.
pub fn key_step(axis: Axis, edge: Edge, key: &str, shift: bool) -> Option<f32> {
    let step = if shift { 24. } else { 8. };
    let (less, more) = match axis {
        Axis::X => ("left", "right"),
        Axis::Y => ("up", "down"),
    };
    let sign = match (key, edge) {
        (k, Edge::End) if k == less => -1.,
        (k, Edge::End) if k == more => 1.,
        (k, Edge::Start) if k == less => 1.,
        (k, Edge::Start) if k == more => -1.,
        _ => return None,
    };
    Some(sign * step)
}

type Live = Rc<dyn Fn(f32, &mut Window, &mut App)>;
type Done = Rc<dyn Fn(&mut Window, &mut App)>;
type Max = Rc<dyn Fn(&mut Window, &mut App) -> f32>;

#[derive(Default)]
struct State {
    dragging: Option<(f32, f32, f32)>,
}

/// `<ResizeHandle panel edge axis>`. Place it inside a `relative()` panel.
#[derive(IntoElement)]
pub struct ResizeHandle {
    id: ElementId,
    axis: Axis,
    edge: Edge,
    size: f32,
    focus: FocusHandle,
    max: Option<Max>,
    on_live: Live,
    on_commit: Done,
    on_step: Live,
}

/// `size` is the panel's current size. Wire [`ResizeHandle::on_live`] (while dragging: clamp
/// and apply without persisting), [`ResizeHandle::on_commit`] (release: persist) and
/// [`ResizeHandle::on_step`] (arrow keys: apply and persist).
pub fn resize_handle(
    id: impl Into<ElementId>,
    axis: Axis,
    edge: Edge,
    size: f32,
    focus: &FocusHandle,
) -> ResizeHandle {
    ResizeHandle {
        id: id.into(),
        axis,
        edge,
        size,
        focus: focus.clone(),
        max: None,
        on_live: Rc::new(|_, _, _| {}),
        on_commit: Rc::new(|_, _| {}),
        on_step: Rc::new(|_, _, _| {}),
    }
}

impl ResizeHandle {
    pub fn on_live(mut self, f: impl Fn(f32, &mut Window, &mut App) + 'static) -> Self {
        self.on_live = Rc::new(f);
        self
    }
    pub fn on_commit(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_commit = Rc::new(f);
        self
    }
    pub fn on_step(mut self, f: impl Fn(f32, &mut Window, &mut App) + 'static) -> Self {
        self.on_step = Rc::new(f);
        self
    }
    /// The layout-aware ceiling, measured when a drag starts (keep the code column usable).
    pub fn max(mut self, f: impl Fn(&mut Window, &mut App) -> f32 + 'static) -> Self {
        self.max = Some(Rc::new(f));
        self
    }
}

impl ResizeHandle {
    fn render_view(self, window: &mut Window, cx: &mut App) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| State::default());
        let dragging = state.read(cx).dragging.is_some();
        let lit = t.amber.fade(0.55);
        let focused = is_focus_visible(&self.focus, window);
        let (axis, edge) = (self.axis, self.edge);
        let cursor = match axis {
            Axis::X => CursorStyle::ResizeColumn,
            Axis::Y => CursorStyle::ResizeRow,
        };
        let mut d = div()
            .id(self.id.clone())
            .absolute()
            .track_focus(&self.focus)
            .cursor(cursor)
            .aria_label("Resize panel")
            .hover(move |s| s.bg(lit));
        d = match (axis, edge) {
            (Axis::X, Edge::End) => d.top_0().bottom_0().w(px(6.)).right(px(-3.)),
            (Axis::X, Edge::Start) => d.top_0().bottom_0().w(px(6.)).left(px(-3.)),
            (Axis::Y, Edge::End) => d.left_0().right_0().h(px(6.)).bottom(px(-3.)),
            (Axis::Y, Edge::Start) => d.left_0().right_0().h(px(6.)).top(px(-3.)),
        };
        if dragging || focused {
            d = d.bg(lit);
        }
        {
            let state = state.clone();
            let max = self.max.clone();
            let size = self.size;
            d = d.on_mouse_down(MouseButton::Left, move |e, window, cx| {
                cx.stop_propagation();
                let start = match axis {
                    Axis::X => f32::from(e.position.x),
                    Axis::Y => f32::from(e.position.y),
                };
                let cap = max
                    .as_ref()
                    .map_or(f32::INFINITY, |m| m(window, cx).max(size));
                state.update(cx, |s, _| s.dragging = Some((start, size, cap)));
                window.refresh();
            });
        }
        {
            let step = self.on_step.clone();
            let size = self.size;
            d = d.on_key_down(move |e: &KeyDownEvent, window, cx| {
                if let Some(delta) =
                    key_step(axis, edge, &e.keystroke.key, e.keystroke.modifiers.shift)
                {
                    cx.stop_propagation();
                    step(size + delta, window, cx);
                }
            });
        }
        // While dragging, the whole window resizes (`body.resizing-panels` cursor) and follows
        // the pointer even outside the strip (pointer capture).
        let tracker = {
            let state = state.clone();
            let live = self.on_live.clone();
            let commit = self.on_commit.clone();
            canvas(
                |_, _, _| {},
                move |_, _, window, cx| {
                    let Some((start, start_size, cap)) = state.read(cx).dragging else {
                        return;
                    };
                    window.set_window_cursor_style(cursor);
                    let live = live.clone();
                    window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                        if phase != DispatchPhase::Capture {
                            return;
                        }
                        let p = match axis {
                            Axis::X => f32::from(e.position.x),
                            Axis::Y => f32::from(e.position.y),
                        };
                        live(drag_size(edge, start_size, start, p, cap), window, cx);
                    });
                    let state = state.clone();
                    let commit = commit.clone();
                    window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
                        if phase != DispatchPhase::Capture || e.button != MouseButton::Left {
                            return;
                        }
                        state.update(cx, |s, _| s.dragging = None);
                        commit(window, cx);
                        window.refresh();
                    });
                },
            )
            .size_full()
        };
        // `title="Drag to resize"`; `z-index: 8` paints the handle over its panel's border,
        // which GPUI would otherwise draw on top of the panel's children.
        let id = self.id;
        deferred(titled((id, "tip"), "Drag to resize", d.child(tracker))).with_priority(8)
    }
}

impl RenderOnce for ResizeHandle {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        crate::guard::component(self, window, cx, Self::render_view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drags_toward_the_growing_side_and_caps() {
        assert_eq!(drag_size(Edge::End, 300., 100., 140., f32::INFINITY), 340.);
        assert_eq!(
            drag_size(Edge::Start, 300., 100., 140., f32::INFINITY),
            260.
        );
        assert_eq!(drag_size(Edge::End, 300., 100., 400., 450.), 450.);
    }

    #[test]
    fn arrow_steps() {
        assert_eq!(key_step(Axis::X, Edge::End, "right", false), Some(8.));
        assert_eq!(key_step(Axis::X, Edge::End, "left", true), Some(-24.));
        assert_eq!(key_step(Axis::X, Edge::Start, "left", false), Some(8.));
        assert_eq!(key_step(Axis::Y, Edge::Start, "up", false), Some(8.));
        assert_eq!(key_step(Axis::Y, Edge::End, "up", false), Some(-8.));
        assert_eq!(key_step(Axis::X, Edge::End, "up", false), None);
    }
}
