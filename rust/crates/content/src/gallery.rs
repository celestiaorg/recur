//! Parity scenes: the `content` list in `rust/tools/ui-parity/scenes.json`, built the way
//! `web/scenes/Content.svelte` mounts the Svelte components, over the same fixtures.

use gpui_kit::{
    AnyView, App, AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, Window,
    div, px,
};
use recur_ui::gallery::SceneSpec;
use recur_ui::style::body;
use serde::Deserialize;

use crate::render::style::MarkdownClass;
use crate::view::MarkdownView;

const SCENES_JSON: &str = include_str!("../../../tools/ui-parity/scenes.json");
pub const CHAT_MD: &str = include_str!("../../../tools/ui-parity/content/chat.md");
pub const README_MD: &str = include_str!("../../../tools/ui-parity/content/readme.md");
pub const STREAMING_MD: &str = include_str!("../../../tools/ui-parity/content/streaming.md");
pub const SEQUENCE_MD: &str = include_str!("../../../tools/ui-parity/content/sequence.md");
pub const FLOW_MMD: &str = include_str!("../../../tools/ui-parity/content/flow.mmd");
pub const LOGO_SVG: &str = include_str!("../../../tools/ui-parity/content/logo.svg");
pub const PAGE_PDF: &[u8] = include_bytes!("../../../tools/ui-parity/content/page.pdf");
pub const SCREENSHOT_PNG: &[u8] = include_bytes!("../../../tools/ui-parity/content/screenshot.png");

#[derive(Deserialize)]
struct Specs {
    #[serde(default)]
    content: Vec<SceneSpec>,
}

/// Every content scene, in file order.
pub fn specs() -> Vec<SceneSpec> {
    serde_json::from_str::<Specs>(SCENES_JSON)
        .map(|s| s.content)
        .unwrap_or_default()
}

pub fn exists(name: &str) -> bool {
    specs().iter().any(|s| s.name == name)
}

/// A padded markdown block on the page body (`.pad` in `Content.svelte`).
struct Padded(Entity<MarkdownView>);

impl Render for Padded {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        body(cx)
            .size_full()
            .flex()
            .flex_col()
            .child(div().p(px(16.)).child(self.0.clone()))
    }
}

/// A padded live stream.
struct Streaming(Entity<crate::StreamingMarkdown>);

impl Render for Streaming {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        body(cx)
            .size_full()
            .flex()
            .flex_col()
            .child(div().p(px(16.)).child(self.0.clone()))
    }
}

/// A preview filling the window (`.content-scene`).
struct Full(AnyView);

impl Render for Full {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        body(cx).size_full().flex().flex_col().child(self.0.clone())
    }
}

fn preview(kind: crate::preview::PreviewKind, content: &str, path: &str, window: &mut Window, cx: &mut App) -> AnyView {
    let scroll = std::rc::Rc::new(std::cell::RefCell::new(crate::preview::scroll::PreviewScrollStore::new()));
    let p = cx.new(|cx| crate::preview::view::FilePreview::new(kind, content.to_string(), path.to_string(), scroll, window, cx));
    AnyView::from(cx.new(|_| Full(p.into())))
}

pub fn build(name: &str, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    // No application loop answers the webview here: Mermaid renders on the spot.
    cx.set_global(crate::mermaid::BlockingRenders);
    if std::env::var_os("RECUR_PROBE").is_some() {
        let html = std::fs::read_to_string("/tmp/probe.html").unwrap_or_default();
        eprintln!("probe {:?}", crate::mermaid::engine::call_blocking(serde_json::json!({"op": "probe", "html": html})));
    }
    use crate::preview::PreviewKind;
    let md = |text: &'static str, cx: &mut App| {
        let v = cx.new(|cx| MarkdownView::new(text, MarkdownClass::ChatCanvas, false, cx));
        AnyView::from(cx.new(|_| Padded(v)))
    };
    Some(match name {
        "markdown/chat" | "markdown/paper" => md(CHAT_MD, cx),
        "mermaid/sequence" => md(SEQUENCE_MD, cx),
        "mermaid/flowchart" => preview(PreviewKind::Mermaid, FLOW_MMD, "flow.mmd", window, cx),
        "file-preview/markdown" => preview(PreviewKind::Markdown, README_MD, "README.md", window, cx),
        "file-preview/svg" => preview(PreviewKind::Svg, LOGO_SVG, "logo.svg", window, cx),
        "markdown/streaming" => {
            // The worker's output, produced on the spot so the headless frame has it.
            let render = std::rc::Rc::new(|src: &str, _: &str| {
                let (tx, rx) = futures::channel::oneshot::channel();
                let html = crate::markdown::render_markdown(src, Default::default())
                    .map(std::sync::Arc::from)
                    .map_err(|e| crate::markdown::RenderError::Failed(e.to_string()));
                let _ = tx.send(html);
                rx
            });
            let s = cx.new(|cx| {
                let mut s = crate::StreamingMarkdown::with_renderer(render, cx);
                s.set(STREAMING_MD, true, false, cx);
                s
            });
            AnyView::from(cx.new(|_| Streaming(s)))
        }
        _ => return None,
    })
}
