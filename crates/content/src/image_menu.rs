//! Right-click Copy Image over an image, clamped into the window.

use gpui_kit::{
    App, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable, Image, ImageFormat,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window, deferred, div, px,
};
use recur_ui::guard::Guarded;
use recur_ui::shadow::{css_box_shadow, with_shadow};
use recur_ui::style::anchored_at;
use recur_ui::{TextStyled, Theme, rgba};

/// `openImageCtx`: the menu's top-left for a click at `(x, y)` in a `w`×`h` window.
pub fn menu_position(x: f32, y: f32, w: f32, h: f32) -> (f32, f32) {
    let (pad, mw, mh) = (8., 180., 40.);
    let mut x = x;
    let mut y = y;
    if x + mw > w - pad {
        x = w - mw - pad;
    }
    if y + mh > h - pad {
        y = h - mh - pad;
    }
    (x.max(pad), y.max(pad))
}

#[derive(Clone, Debug, PartialEq)]
pub enum ImageMenuEvent {
    Close,
    Toast { error: bool, message: String },
}

pub struct ImageContextMenu {
    x: f32,
    y: f32,
    png: Option<std::sync::Arc<Vec<u8>>>,
    focus: FocusHandle,
}

impl EventEmitter<ImageMenuEvent> for ImageContextMenu {}

impl Focusable for ImageContextMenu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// `copyImage`: PNG bytes onto the clipboard.
pub fn copy_png(png: Option<&std::sync::Arc<Vec<u8>>>, cx: &mut App) -> Result<(), String> {
    let png = png.ok_or_else(|| "No image to copy".to_string())?;
    cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
        ImageFormat::Png,
        png.as_ref().clone(),
    )));
    Ok(())
}

impl ImageContextMenu {
    pub fn new(
        click: (f32, f32),
        png: Option<std::sync::Arc<Vec<u8>>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let vp = window.viewport_size();
        let (x, y) = menu_position(click.0, click.1, f32::from(vp.width), f32::from(vp.height));
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        ImageContextMenu { x, y, png, focus }
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        cx.emit(ImageMenuEvent::Close);
        let ev = match copy_png(self.png.as_ref(), cx) {
            Ok(()) => ImageMenuEvent::Toast {
                error: false,
                message: "Copied image".into(),
            },
            Err(_) => ImageMenuEvent::Toast {
                error: true,
                message: "Could not copy image".into(),
            },
        };
        cx.emit(ev);
    }
}

impl ImageContextMenu {
    fn render_view(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        let item = div()
            .id("att-ctx-copy")
            .flex()
            .items_center()
            .gap(px(16.))
            .w_full()
            .px(px(10.))
            .py(px(7.))
            .rounded(px(5.))
            .text_color(t.text)
            .cursor_pointer()
            .hover(move |s| s.bg(t.pane_2))
            .on_click(cx.guarded_listener(|m, _, _, cx| m.copy(cx)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(recur_ui::text("Copy Image").ui().size(12.).leading_normal()),
            )
            .child(recur_ui::kbd::kbd("⌘C").size(11.).color(t.faint));
        let panel = recur_ui::style::card(t.pane, t.line_bright, 8.)
            .min_w(px(180.))
            .p(px(4.))
            .child(item);
        let menu = with_shadow(
            panel,
            &[css_box_shadow(0., 12., 32., 0., rgba(0, 0, 0, 0.45))],
            8.,
        );
        // `.att-ctx`'s `z-index: 230`: over the page's resize handles and the lightbox.
        deferred(
            div()
                .id("att-ctx")
                .track_focus(&self.focus)
                .key_context("ImageContextMenu")
                .absolute()
                .inset_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.guarded_listener(|_, _, _, cx| cx.emit(ImageMenuEvent::Close)),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.guarded_listener(|_, _, _, cx| cx.emit(ImageMenuEvent::Close)),
                )
                .capture_key_down(cx.guarded_listener(|_, e: &KeyDownEvent, _, cx| {
                    if e.keystroke.key == "escape" {
                        cx.stop_propagation();
                        cx.emit(ImageMenuEvent::Close);
                    }
                }))
                .child(anchored_at(
                    self.x,
                    self.y,
                    div()
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(menu),
                )),
        )
        .with_priority(230)
    }
}

impl Render for ImageContextMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_into_the_window() {
        assert_eq!(menu_position(100., 100., 800., 600.), (100., 100.));
        assert_eq!(menu_position(700., 590., 800., 600.), (612., 552.));
        assert_eq!(menu_position(2., 2., 800., 600.), (8., 8.));
    }
}
