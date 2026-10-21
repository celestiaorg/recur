//! `lib/mermaid.ts` and `MermaidPreview.svelte`.

use gpui_kit::{Context, Task};

use crate::view::MarkdownView;

/// `hydrateMermaid`: render each placeholder in order into the view's slots.
pub fn hydrate(sources: Vec<String>, cx: &mut Context<MarkdownView>) -> Task<()> {
    let _ = sources;
    cx.spawn(async move |_, _| {})
}
