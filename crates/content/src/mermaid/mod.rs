//! Mermaid diagrams: markdown fence hydration and the whole-file preview.
//!
//! mmdr (`mermaid-rs-renderer`, pure Rust) lays the diagram out and writes its SVG off the UI
//! thread; the SVG is sanitized and drawn natively ([`diagram`]):
//! geometry rasterized at the device size, labels as recur-ui text where usvg shapes them.

pub mod diagram;
pub mod preview;

use std::rc::Rc;

use gpui_kit::{AsyncApp, Context, Task};
use recur_ui::{Theme, ThemeId};

use crate::render::block::MermaidSlot;
use crate::view::MarkdownView;
pub use diagram::Diagram;
pub use preview::{DiagramChanged, MermaidPreview};

/// Named faces first, so mmdr's text measurement and usvg's shaping resolve the same font.
const FONT: &str =
    r#""Helvetica Neue", Helvetica, Arial, "DejaVu Sans", "Liberation Sans", sans-serif"#;

/// `themeVariablesFromCSS`: mermaid's default or dark theme under the active chrome's tokens.
pub fn theme(theme: &Theme) -> mmdr::Theme {
    let t = &theme.tokens;
    let hex = |c: recur_ui::Color| c.to_hex();
    let mut m = if ThemeId::is_light(theme.id) {
        mmdr::Theme::mermaid_default()
    } else {
        mmdr::Theme::dark()
    };
    m.font_family = FONT.into();
    m.background = hex(t.pane);
    m.primary_color = hex(t.pane_2);
    m.primary_text_color = hex(t.text);
    m.primary_border_color = hex(t.line_bright);
    m.secondary_color = hex(t.pane);
    m.tertiary_color = hex(t.bg);
    m.line_color = hex(t.muted);
    m.text_color = hex(t.text);
    m.cluster_background = hex(t.pane);
    m.edge_label_background = hex(t.pane);
    m
}

/// mmdr's SVG for `source`. CPU-bound: call it off the UI thread.
pub fn svg(source: &str, theme: mmdr::Theme) -> Result<String, String> {
    let options = mmdr::RenderOptions {
        theme,
        ..Default::default()
    };
    let canvas = format!(r#" fill="{}"/>"#, options.theme.background);
    // Untrusted input (chat and files) must not take the app down with it.
    let svg = std::panic::catch_unwind(|| mmdr::render_with_options(source, options))
        .map_err(|_| "Mermaid render failed".to_string())?
        .map_err(|e| e.to_string())?;
    Ok(transparent(svg, &canvas))
}

/// mmdr paints the whole canvas first; mermaid.js leaves it to the surface the diagram sits on.
fn transparent(mut svg: String, canvas: &str) -> String {
    if let Some(start) = svg.find("<rect ")
        && let Some(len) = svg[start..].find("/>")
        && svg[start..start + len + 2].ends_with(canvas)
    {
        svg.replace_range(start..start + len + 2, "");
    }
    svg
}

fn build(svg: &str) -> Result<Rc<Diagram>, String> {
    Diagram::build(&diagram::sanitize_svg(svg)).map(Rc::new)
}

/// Lay `source` out on the background executor and build the diagram.
pub async fn render(source: String, theme: &Theme, cx: &AsyncApp) -> Result<Rc<Diagram>, String> {
    let theme = self::theme(theme);
    let svg = cx
        .background_executor()
        .spawn(async move { svg(&source, theme) })
        .await?;
    build(&svg)
}

/// [`render`] on the calling thread (tests).
pub fn render_blocking(source: &str, theme: &Theme) -> Result<Rc<Diagram>, String> {
    build(&svg(source, self::theme(theme))?)
}

/// Draws a diagram into a markdown `.md-mermaid` slot.
fn slot(d: Rc<Diagram>) -> MermaidSlot {
    MermaidSlot::Ready(Rc::new(move |max_w, window| {
        d.inline_element(max_w, window)
    }))
}

/// `hydrateMermaid`: each placeholder in order; empty sources are hidden; failures keep the
/// source with the error border.
pub fn hydrate(sources: Vec<String>, cx: &mut Context<MarkdownView>) -> Task<()> {
    let theme = Theme::of(cx).clone();
    cx.spawn(async move |this, cx| {
        for (i, src) in sources.into_iter().enumerate() {
            let s = if src.trim().is_empty() {
                MermaidSlot::Empty
            } else {
                match render(src, &theme, cx).await {
                    Ok(d) => slot(d),
                    Err(_) => MermaidSlot::Error,
                }
            };
            if this.update(cx, |v, cx| v.set_mermaid(i, s, cx)).is_err() {
                return;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_flowchart_with_native_labels() {
        let flow = include_str!("../../../ui/gallery/content/flow.mmd");
        let d = render_blocking(flow, &Theme::new(ThemeId::Charcoal)).expect("render");
        let mut words: Vec<&str> = d
            .runs
            .iter()
            .flat_map(|r| r.text.split_whitespace())
            .collect();
        words.sort_unstable();
        let mut want: Vec<&str> =
            "Write markdown Has fences? mermaid code Render diagram Highlight Show in chat"
                .split(' ')
                .collect();
        want.sort_unstable();
        assert_eq!(words, want);
        let (_, _, w, h) = d.view_box;
        assert!(w > 100. && h > 200., "viewBox {:?}", d.view_box);
        assert!(render_blocking("flowchart TD\n  A --> ", &Theme::new(ThemeId::Charcoal)).is_err());
        let svg = svg(
            "flowchart LR\n  A --> B",
            theme(&Theme::new(ThemeId::Charcoal)),
        )
        .unwrap();
        assert!(svg.contains("<rect "));
        assert!(!svg.contains(&format!(
            r#"fill="{}"/>"#,
            theme(&Theme::new(ThemeId::Charcoal)).background
        )));
    }
}
