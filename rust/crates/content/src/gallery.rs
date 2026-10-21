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

pub fn build(name: &str, _window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let md = |text: &'static str, cx: &mut App| {
        let v = cx.new(|cx| MarkdownView::new(text, MarkdownClass::ChatCanvas, false, cx));
        AnyView::from(cx.new(|_| Padded(v)))
    };
    Some(match name {
        "markdown/chat" | "markdown/paper" => md(CHAT_MD, cx),
        "mermaid/sequence" => md(SEQUENCE_MD, cx),
        _ => return None,
    })
}
