//! `lib/mermaid.ts`, `MarkdownHtml`'s hydration and `MermaidPreview.svelte`.
//!
//! mermaid.js renders in WebKit ([`engine`]); its SVG is sanitized as `sanitizeMermaidSvg`
//! does, measured once in the same engine (text runs and their start positions, after
//! `unclipMermaidSvg`), and drawn natively ([`diagram`]): geometry rasterized through
//! CoreGraphics at the device size, labels as recur-ui text at the positions WebKit laid out.

pub mod diagram;
pub mod engine;
pub mod preview;

use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

use gpui_kit::{App, Context, Task};
use recur_ui::{Theme, ThemeId};
use serde_json::{Value, json};

use crate::render::block::MermaidSlot;
use crate::view::MarkdownView;
pub use diagram::Diagram;
pub use preview::MermaidPreview;

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// `themeVariablesFromCSS`: the active chrome's tokens.
pub fn theme_variables(theme: &Theme) -> Value {
    let t = &theme.tokens;
    let hex = |c: recur_ui::Color| c.to_hex();
    json!({
        "background": hex(t.pane),
        "primaryColor": hex(t.pane_2),
        "primaryTextColor": hex(t.text),
        "primaryBorderColor": hex(t.line_bright),
        "secondaryColor": hex(t.pane),
        "tertiaryColor": hex(t.bg),
        "lineColor": hex(t.muted),
        "textColor": hex(t.text),
        "mainBkg": hex(t.pane_2),
        "nodeBorder": hex(t.line_bright),
        "clusterBkg": hex(t.pane),
        "titleColor": hex(t.text),
        "edgeLabelBackground": hex(t.pane),
    })
}

/// `configure`: what `mermaid.initialize` receives before every render.
pub fn config(theme: &Theme) -> Value {
    let light = ThemeId::is_light(theme.id);
    json!({
        "startOnLoad": false,
        "securityLevel": "strict",
        "htmlLabels": false,
        "theme": if light { "default" } else { "dark" },
        "themeVariables": theme_variables(theme),
        "fontFamily": "ui-sans-serif, system-ui, sans-serif",
        "flowchart": { "padding": 12 },
    })
}

/// A render's ids: `md-mmd-N` for markdown fences, `file-mmd-N` for whole files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Markdown,
    File,
}

fn next_id(origin: Origin) -> String {
    let n = SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    match origin {
        Origin::Markdown => format!("md-mmd-{n}"),
        Origin::File => format!("file-mmd-{n}"),
    }
}

fn render_op(origin: Origin, source: &str, theme: &Theme) -> Value {
    json!({ "op": "render", "id": next_id(origin), "source": source, "config": config(theme) })
}

fn error_of(v: &Value) -> Option<String> {
    v.get("error").and_then(Value::as_str).map(str::to_string)
}

/// Render `source` and build the diagram (async: resolves on the app's run loop).
pub async fn render(origin: Origin, source: String, theme: Theme) -> Result<Rc<Diagram>, String> {
    let out = engine::call(render_op(origin, &source, &theme))
        .await
        .map_err(|_| "Mermaid render failed".to_string())?
        .map_err(|e| e.0)?;
    if let Some(e) = error_of(&out) {
        return Err(e);
    }
    let svg = out.get("svg").and_then(Value::as_str).unwrap_or("").to_string();
    let clean = diagram::sanitize_svg(&svg);
    let measured = engine::call(json!({ "op": "measure", "svg": clean }))
        .await
        .map_err(|_| "Mermaid render failed".to_string())?
        .map_err(|e| e.0)?;
    Diagram::build(&clean, &measured).map(Rc::new)
}

/// [`render`], spinning the main run loop until done (headless renders and tests).
pub fn render_blocking(origin: Origin, source: &str, theme: &Theme) -> Result<Rc<Diagram>, String> {
    let out = engine::call_blocking(render_op(origin, source, theme)).map_err(|e| e.0)?;
    if let Some(e) = error_of(&out) {
        return Err(e);
    }
    let svg = out.get("svg").and_then(Value::as_str).unwrap_or("").to_string();
    let clean = diagram::sanitize_svg(&svg);
    let measured = engine::call_blocking(json!({ "op": "measure", "svg": clean })).map_err(|e| e.0)?;
    Diagram::build(&clean, &measured).map(Rc::new)
}

/// Draws a diagram into a markdown `.md-mermaid` slot.
fn slot(d: Rc<Diagram>) -> MermaidSlot {
    MermaidSlot::Ready(Rc::new(move |max_w, window| d.inline_element(max_w, window)))
}

/// Whether the hydration should render synchronously (no application run loop to answer
/// the webview: headless parity renders).
pub fn blocking(cx: &App) -> bool {
    cx.try_global::<BlockingRenders>().is_some()
}

/// Installed by headless hosts (the parity tool) that have no running application loop.
pub struct BlockingRenders;

impl gpui_kit::Global for BlockingRenders {}

/// A hydration: done on the spot (headless) or running.
pub enum Hydration {
    Done(Vec<MermaidSlot>),
    Running(Task<()>),
}

/// `hydrateMermaid`: each placeholder in order; empty sources are hidden; failures keep the
/// source with the error border.
pub fn hydrate(sources: Vec<String>, cx: &mut Context<MarkdownView>) -> Hydration {
    let theme = Theme::of(cx).clone();
    if blocking(cx) {
        return Hydration::Done(
            sources
                .iter()
                .map(|src| {
                    if src.trim().is_empty() {
                        MermaidSlot::Empty
                    } else {
                        render_blocking(Origin::Markdown, src, &theme).map_or(MermaidSlot::Error, slot)
                    }
                })
                .collect(),
        );
    }
    Hydration::Running(cx.spawn(async move |this, cx| {
        for (i, src) in sources.into_iter().enumerate() {
            let s = if src.trim().is_empty() {
                MermaidSlot::Empty
            } else {
                match render(Origin::Markdown, src, theme.clone()).await {
                    Ok(d) => slot(d),
                    Err(_) => MermaidSlot::Error,
                }
            };
            if this.update(cx, |v, cx| v.set_mermaid(i, s, cx)).is_err() {
                return;
            }
        }
    }))
}
