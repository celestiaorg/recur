//! Blocking renders, and on Linux the WebKitGTK thread the HTML preview's pages run on.

/// Whether PDF pages and Mermaid diagrams render synchronously (gallery scenes, which run no
/// application loop).
pub fn blocking(cx: &gpui_kit::App) -> bool {
    cx.try_global::<BlockingRenders>().is_some()
}

/// Installed by hosts that have no running application loop (the gallery scenes).
pub struct BlockingRenders;

impl gpui_kit::Global for BlockingRenders {}

/// GPUI's Linux event loop is not GLib's, so WebKitGTK runs on a thread of its own that
/// initializes GTK and owns the default main context.
#[cfg(target_os = "linux")]
pub(crate) use gtk::context as gtk_context;

#[cfg(target_os = "linux")]
mod gtk {
    use std::sync::OnceLock;

    use webkit6::glib;

    /// The WebKitGTK thread's main context, started on first use. GTK needs a display
    /// (`DISPLAY` or `WAYLAND_DISPLAY`); without one every call fails with GTK's reason.
    pub(crate) fn context() -> Result<&'static glib::MainContext, &'static str> {
        static CONTEXT: OnceLock<Result<glib::MainContext, String>> = OnceLock::new();
        CONTEXT
            .get_or_init(|| {
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::Builder::new()
                    .name("recur-webkit".into())
                    .spawn(move || match webkit6::gtk::init() {
                        Ok(()) => {
                            let _ = tx.send(Ok(glib::MainContext::default()));
                            glib::MainLoop::new(None, false).run();
                        }
                        Err(e) => {
                            let _ = tx.send(Err(e.to_string()));
                        }
                    })
                    .map_err(|e| e.to_string())?;
                rx.recv().map_err(|e| e.to_string())?
            })
            .as_ref()
            .map_err(String::as_str)
    }
}
