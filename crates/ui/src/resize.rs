//! `resize: vertical` as WebKit draws and drags it on macOS: the `textAreaResizeCorner` bitmap
//! (`assets/webkit`, from WebCore's resources) at the bottom-right corner of the padding box,
//! and a drag on the corner that sets the box's height (`RenderLayer::resize`).

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, Bounds, CursorStyle, Div, ElementId, FocusHandle, InteractiveElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Styled, Window, canvas,
    div, px,
};

const CORNER_1X: &[u8] = include_bytes!("../assets/webkit/textAreaResizeCorner.png");
const CORNER_2X: &[u8] = include_bytes!("../assets/webkit/textAreaResizeCorner@2x.png");
/// The corner image's CSS size.
const CORNER: f32 = 8.;
/// The hit square: `cornerRect`'s scrollbar thickness when the box has no scrollbars.
const HIT: f32 = 15.;

#[derive(Default)]
struct State {
    /// The inline `height` a drag set.
    height: Option<f32>,
    /// `minimumSizeForResizing`: the smallest height the box had when a drag moved, so a drag
    /// never shrinks it below where it started.
    min: Option<f32>,
    /// The border box as last laid out.
    bounds: Option<Bounds<Pixels>>,
    /// While dragging, the pointer's offset from the bottom edge at mouse down.
    grab: Option<f32>,
}

/// The box with `resize: vertical`: `field` is its border box (positioned, with `border` px
/// borders); its min and max heights clamp the drag as they clamp the inline height in CSS.
/// Pressing the corner focuses the textarea (`focus`), as WebKit's mouse down does.
pub fn vertical(
    id: impl Into<ElementId>,
    field: Div,
    border: f32,
    focus: FocusHandle,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    let id = id.into();
    let state = window.use_keyed_state(id.clone(), cx, |_, _| State::default());
    let s = state.read(cx);
    let (height, dragging) = (s.height, s.grab.is_some());
    let record = state.clone();
    let drag = state.clone();
    let start = state.clone();
    field
        .when_some(height, |d, h| d.h(px(h)))
        .child(
            canvas(
                move |bounds, _, cx| {
                    record.update(cx, |s, _| s.bounds = Some(bounds.dilate(px(border))));
                },
                move |_, _, window, _| {
                    if !dragging {
                        return;
                    }
                    let (moved, up) = (drag.clone(), drag.clone());
                    window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                        if phase.bubble() {
                            moved.update(cx, |s, cx| {
                                if s.drag_to(f32::from(e.position.y)) {
                                    cx.notify();
                                }
                            });
                        }
                    });
                    window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                        if phase.bubble() {
                            up.update(cx, |s, cx| {
                                s.grab = None;
                                cx.notify();
                            });
                        }
                    });
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
        .child(
            div()
                .id(ElementId::Name(format!("{id}-resizer").into()))
                .debug_selector({
                    let id = id.clone();
                    move || format!("resizer:{id}")
                })
                .absolute()
                .right_0()
                .bottom_0()
                .size(px(HIT))
                .cursor(CursorStyle::Arrow)
                .on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    // After dispatch: a field blurs on a mouse down outside its own text box.
                    let focus = focus.clone();
                    window.defer(cx, move |window, cx| window.focus(&focus, cx));
                    start.update(cx, |s, cx| {
                        if let Some(b) = s.bounds {
                            let y = f32::from(e.position.y).round();
                            s.grab = Some(y - f32::from(b.bottom()));
                            cx.notify();
                        }
                    });
                })
                // Over the box's border, which GPUI paints after its children.
                .child(gpui_kit::deferred(
                    crate::icon::raster_icon(0x7265_7369_7a65, corner)
                        .absolute()
                        .right_0()
                        .bottom_0()
                        .size(px(CORNER)),
                )),
        )
}

impl State {
    /// `RenderLayer::resize` for a pointer at `y`: the new height, rounded to a CSS pixel and
    /// never below the size the box had when the drag began. True when it changed.
    fn drag_to(&mut self, y: f32) -> bool {
        let (Some(grab), Some(b)) = (self.grab, self.bounds) else {
            return false;
        };
        let current = f32::from(b.size.height);
        let min = self.min.map_or(current, |m| m.min(current));
        self.min = Some(min);
        let h = (y.round() - grab - f32::from(b.top())).max(min).round();
        let changed = self.height != Some(h);
        self.height = Some(h);
        changed
    }
}

/// The corner bitmap at `w` x `h` device pixels, premultiplied RGBA.
fn corner(w: u32, h: u32, scale: f32) -> Option<Vec<u8>> {
    let png = if scale >= 2. { CORNER_2X } else { CORNER_1X };
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .ok()?
        .into_rgba8();
    let img = if img.dimensions() == (w, h) {
        img
    } else {
        image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle)
    };
    let mut px = img.into_raw();
    for p in px.as_chunks_mut::<4>().0 {
        let a = u32::from(p[3]);
        for c in &mut p[..3] {
            *c = ((u32::from(*c) * a + 127) / 255) as u8;
        }
    }
    Some(px)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{point, size};

    fn dragging(top: f32, height: f32, grab: f32) -> State {
        State {
            bounds: Some(Bounds::new(
                point(px(0.), px(top)),
                size(px(100.), px(height)),
            )),
            grab: Some(grab),
            ..State::default()
        }
    }

    #[test]
    fn a_drag_follows_the_pointer_from_where_it_grabbed() {
        // Grabbed 3px above the bottom edge of a box at 100..188.
        let mut s = dragging(100., 88., -3.);
        assert!(s.drag_to(240.4));
        assert_eq!(s.height, Some(143.));
        assert!(!s.drag_to(240.2), "the same rounded height is no change");
    }

    #[test]
    fn a_drag_never_shrinks_below_the_starting_size() {
        let mut s = dragging(100., 88., 0.);
        s.drag_to(120.);
        assert_eq!(s.height, Some(88.));
        s.bounds = Some(Bounds::new(
            point(px(0.), px(100.)),
            size(px(100.), px(150.)),
        ));
        s.drag_to(160.);
        assert_eq!(s.height, Some(88.), "the minimum is the smallest size seen");
    }

    #[test]
    fn the_corner_bitmap_decodes_at_both_scales() {
        assert_eq!(corner(16, 16, 2.).map(|p| p.len()), Some(16 * 16 * 4));
        assert_eq!(corner(8, 8, 1.).map(|p| p.len()), Some(8 * 8 * 4));
    }
}
