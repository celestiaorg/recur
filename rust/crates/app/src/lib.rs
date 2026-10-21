//! The GPUI application: windows, titlebar, sidebar, and every view.

use gpui_kit::*;

struct Shell;

impl Render for Shell {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child("Recur")
    }
}

/// Starts the GPUI application and opens the main window.
pub fn run() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| cx.new(|_| Shell))
            .expect("open main window");
    });
}
