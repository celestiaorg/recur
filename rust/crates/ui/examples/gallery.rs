//! The recur-ui widget gallery: every parity scene (`rust/tools/ui-parity/scenes.json`), live and
//! interactive, in any theme.
//!
//! ```text
//! cargo run -p recur-ui --example gallery
//! ```

use gpui_kit::{
    AnyView, App, AppContext, Bounds, Context, Hsla, InteractiveElement, IntoElement,
    ParentElement, Render, StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions,
    div, point, prelude::FluentBuilder, px, size,
};
use recur_ui::gallery::{self, SceneSpec};
use recur_ui::style::{Tbtn, TbtnSize, body, button_label, tbtn, window_root};
use recur_ui::theme::{Theme, ThemeId};
use recur_ui::widgets::scroll_area;
use recur_ui::{TextStyled, text};

struct Gallery {
    specs: Vec<SceneSpec>,
    current: usize,
    theme: Option<ThemeId>,
    view: Option<AnyView>,
}

impl Gallery {
    /// Show scene `ix`, in its own theme unless one is picked.
    fn show(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.current = ix;
        let spec = &self.specs[ix];
        Theme::set(self.theme.unwrap_or(spec.theme_id()), cx);
        // Scenes read the theme as they build (tints, seeded toasts), so build after switching.
        self.view = gallery::build(&spec.name, window, cx);
        cx.notify();
    }
}

impl Render for Gallery {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx).tokens;
        let spec = self.specs[self.current].clone();
        let b = TbtnSize::BASE;

        let list = self.specs.iter().enumerate().map(|(i, s)| {
            let on = i == self.current;
            div()
                .id(("scene", i))
                .py(px(5.))
                .px(px(12.))
                .when(on, |d| d.bg(t.pane_2))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| this.show(i, window, cx)))
                .child(
                    text(s.name.clone())
                        .mono()
                        .size(11.)
                        .color(if on { t.text } else { t.muted })
                        .nowrap(),
                )
        });
        let themes = std::iter::once(None)
            .chain(ThemeId::ALL.into_iter().map(Some))
            .enumerate()
            .map(|(i, id)| {
                let on = id == self.theme;
                let label = id.map_or("Scene theme", |t| t.name());
                tbtn(
                    ("theme", i),
                    if on { Tbtn::On } else { Tbtn::Default },
                    b,
                    None,
                    cx,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.theme = id;
                    let current = this.current;
                    this.show(current, window, cx);
                }))
                .child(button_label(label, b.font_size, 500.))
            });

        let frame = div()
            .relative()
            .flex_none()
            .w(px(spec.width))
            .h(px(spec.height))
            .overflow_hidden()
            .border(px(1.))
            .border_color(Hsla::from(t.line))
            .children(self.view.clone());

        window_root(
            body(cx)
                .size_full()
                .flex()
                .child(
                    div()
                        .flex_none()
                        .w(px(240.))
                        .h_full()
                        .flex()
                        .flex_col()
                        .bg(t.rail)
                        .border_r(px(1.))
                        .border_color(Hsla::from(t.line))
                        .child(scroll_area("scenes").child(div().py(px(8.)).children(list))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(16.))
                        .p(px(20.))
                        .child(div().flex().flex_wrap().gap(px(6.)).children(themes))
                        .child(
                            text(format!(
                                "{}  ·  {}×{}  ·  {}",
                                spec.name, spec.width, spec.height, spec.theme
                            ))
                            .mono()
                            .size(11.)
                            .color(t.faint),
                        )
                        .child(frame),
                ),
        )
    }
}

fn main() {
    gpui_kit::application()
        .with_assets(recur_ui::Assets)
        .run(|cx: &mut App| {
            gpui_kit::init(cx);
            recur_ui::init(cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(80.), px(80.)),
                    size: size(px(1280.), px(820.)),
                })),
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                let specs = gallery::specs();
                cx.new(|cx| {
                    let mut g = Gallery {
                        specs,
                        current: 0,
                        theme: None,
                        view: None,
                    };
                    g.show(0, window, cx);
                    g
                })
            })
            .expect("open the gallery window");
            cx.activate(true);
        });
}
