//! A rendered Mermaid diagram drawn natively: its sanitized SVG's geometry rasterized at the
//! device size (text elements removed), and every label as recur-ui text at the start position
//! WebKit gave it. Labels WebKit transformed beyond translate-and-scale stay in the raster.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use gpui_kit::{
    AnyElement, Bounds, Corners, IntoElement, ParentElement, Pixels, RenderImage, Styled, Window,
    canvas, div, point, px, size,
};
use markup5ever_rcdom::{Handle, NodeData};
use recur_ui::fonts::Family;
use recur_ui::{Color, TextStyled};
use resvg::{tiny_skia, usvg};
use serde::Deserialize;

use crate::purify::{self, Config, dom};
use crate::render::exact::{hgap, vgap};

fn svg_config() -> &'static Config {
    static CFG: OnceLock<Config> = OnceLock::new();
    CFG.get_or_init(Config::mermaid_svg)
}

/// `sanitizeMermaidSvg`.
pub fn sanitize_svg(svg: &str) -> String {
    purify::sanitize(svg, svg_config())
}

/// One label run as WebKit laid it out (viewBox units).
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Run {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub family: String,
    pub weight: f32,
    pub italic: bool,
    pub fill: String,
    #[serde(rename = "fillOpacity")]
    pub fill_opacity: f32,
    pub opacity: f32,
    #[serde(default)]
    pub spacing: f32,
    #[serde(default)]
    pub decoration: String,
}

#[derive(Deserialize)]
struct Measured {
    width: f32,
    height: f32,
    x: f32,
    y: f32,
    runs: Vec<Run>,
    raster: Vec<usize>,
}

pub struct Diagram {
    tree: Arc<usvg::Tree>,
    /// viewBox (x, y, w, h).
    pub view_box: (f32, f32, f32, f32),
    /// Painted extent beyond the viewBox (`overflow: visible`), in viewBox units.
    content: (f32, f32, f32, f32),
    /// The width the SVG asks for: `style="max-width: Npx"` with `width="100%"`, or its width.
    pub natural_width: f32,
    /// `width="100%"`: the SVG fills its box up to `natural_width`.
    pub fluid: bool,
    pub runs: Vec<Run>,
    raster: RefCell<Option<((u32, u32), Arc<RenderImage>)>>,
}

fn is_svg(h: &Handle, tag: &str) -> bool {
    dom::namespace(h) == Some(dom::SVG_NS) && dom::local_name(h) == Some(tag)
}

fn find_svg(h: &Handle) -> Option<Handle> {
    if is_svg(h, "svg") {
        return Some(h.clone());
    }
    h.children.borrow().iter().find_map(find_svg)
}

fn xml_escape(s: &str, attr: bool, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attr => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
}

/// The SVG subtree as XML for usvg, dropping the `<text>` elements labels replace.
fn to_xml(h: &Handle, root: bool, text_ix: &mut usize, keep: &[usize], out: &mut String) {
    match &h.data {
        NodeData::Element { name, attrs, .. } => {
            let local = &*name.local;
            if &*name.ns == dom::SVG_NS && local == "text" {
                let ix = *text_ix;
                *text_ix += 1;
                if !keep.contains(&ix) {
                    return;
                }
            }
            out.push('<');
            out.push_str(local);
            if root {
                out.push_str(r#" xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink""#);
            }
            for a in attrs.borrow().iter() {
                let n = dom::attr_name(a);
                if n == "xmlns" || n.starts_with("xmlns:") {
                    continue;
                }
                let n = match &*a.name.ns {
                    "http://www.w3.org/1999/xlink" => format!("xlink:{}", a.name.local),
                    "http://www.w3.org/XML/1998/namespace" => format!("xml:{}", a.name.local),
                    _ => n,
                };
                out.push(' ');
                out.push_str(&n);
                out.push_str("=\"");
                xml_escape(&a.value, true, out);
                out.push('"');
            }
            out.push('>');
            for c in h.children.borrow().iter() {
                to_xml(c, false, text_ix, keep, out);
            }
            out.push_str("</");
            out.push_str(local);
            out.push('>');
        }
        NodeData::Text { contents } => xml_escape(&contents.borrow(), false, out),
        _ => {}
    }
}

fn px_len(s: &str) -> Option<f32> {
    let s = s.trim().trim_end_matches("px");
    s.parse::<f32>().ok().filter(|v| *v > 0.)
}

impl Diagram {
    /// Build from the sanitized SVG markup and the engine's measurement.
    pub fn build(clean: &str, measured: &serde_json::Value) -> Result<Diagram, String> {
        if let Some(e) = measured.get("error").and_then(|e| e.as_str()) {
            return Err(e.to_string());
        }
        let m: Measured = serde_json::from_value(measured.clone()).map_err(|e| e.to_string())?;
        let frag = dom::parse_fragment_in_div(clean);
        let svg = find_svg(&frag.root).ok_or("Mermaid returned no SVG")?;
        let width_attr = dom::get_attr(&svg, "width").unwrap_or_default();
        let style = dom::get_attr(&svg, "style").unwrap_or_default();
        let max_width = style.split(';').find_map(|decl| {
            let (k, v) = decl.split_once(':')?;
            (k.trim() == "max-width").then(|| px_len(v)).flatten()
        });
        let fluid = width_attr.trim().ends_with('%');
        let natural_width = max_width
            .or_else(|| px_len(&width_attr))
            .unwrap_or(m.width);
        let mut xml = String::new();
        let mut ix = 0;
        to_xml(&svg, true, &mut ix, &m.raster, &mut xml);
        let tree = crate::svg::parse(&xml)?;
        let vb = (m.x, m.y, m.width, m.height);
        let bb = tree.root().abs_stroke_bounding_box();
        let (cx0, cy0) = ((bb.left() + vb.0).min(vb.0), (bb.top() + vb.1).min(vb.1));
        let (cx1, cy1) = (
            (bb.right() + vb.0).max(vb.0 + vb.2),
            (bb.bottom() + vb.1).max(vb.1 + vb.3),
        );
        Ok(Diagram {
            tree: Arc::new(tree),
            view_box: vb,
            content: (cx0, cy0, cx1 - cx0, cy1 - cy0),
            natural_width,
            fluid,
            runs: m.runs,
            raster: RefCell::new(None),
        })
    }

    /// The diagram's height at `width` (`height: auto` from the viewBox).
    pub fn height_at(&self, width: f32) -> f32 {
        if self.view_box.2 > 0. {
            width * self.view_box.3 / self.view_box.2
        } else {
            0.
        }
    }

    /// `.md-mermaid svg { max-width: 100%; height: auto }`.
    pub fn inline_element(self: &Rc<Self>, max_w: f32, window: &Window) -> AnyElement {
        let w = if self.fluid {
            max_w.min(self.natural_width)
        } else {
            self.natural_width.min(max_w)
        };
        let none = gpui_kit::Hsla::default();
        self.element(w, self.height_at(w), &HashMap::new(), (none, none), window)
    }

    /// The text blocks find searches: all labels form one block (the SVG is the nearest
    /// block box), concatenated in document order.
    pub fn text(&self) -> String {
        self.runs.iter().map(|r| r.text.as_str()).collect()
    }

    /// Per-run byte offsets in [`Diagram::text`].
    fn run_starts(&self) -> Vec<usize> {
        let mut at = 0;
        self.runs
            .iter()
            .map(|r| {
                let s = at;
                at += r.text.len();
                s
            })
            .collect()
    }

    /// Where a byte range of [`Diagram::text`] sits, at display scale `s`, relative to the
    /// diagram's top-left.
    pub fn range_rects(&self, range: Range<usize>, s: f32, window: &Window) -> Vec<(f32, f32, f32, f32)> {
        let mut out = Vec::new();
        for (r, start) in self.runs.iter().zip(self.run_starts()) {
            let end = start + r.text.len();
            let (a, b) = (range.start.max(start), range.end.min(end));
            if a >= b {
                continue;
            }
            let t = |x: &str| label_text(r, x, s).measure(None, window);
            let (_, h, m) = t(&r.text);
            let x0 = t(&r.text[..a - start]).0;
            let x1 = t(&r.text[..b - start]).0;
            let left = (r.x - self.view_box.0) * s;
            let top = (r.y - self.view_box.1) * s - m.baseline;
            out.push((left + x0, top, x1 - x0, h));
        }
        out
    }

    fn rasterize(&self, w: u32, h: u32, s: f32, scale: f32) -> Option<Arc<RenderImage>> {
        if let Some((sz, img)) = self.raster.borrow().as_ref()
            && *sz == (w, h)
        {
            return Some(img.clone());
        }
        let k = s * scale;
        let (cx, cy, _, _) = self.content;
        let ts = tiny_skia::Transform::from_row(k, 0., 0., k, -(cx - self.view_box.0) * k, -(cy - self.view_box.1) * k)
            .pre_translate(0., 0.);
        let img = crate::svg::to_render_image(crate::svg::rasterize(&self.tree, ts, w, h)?, w, h)?;
        *self.raster.borrow_mut() = Some(((w, h), img.clone()));
        Some(img)
    }

    /// The diagram at `w`×`h` CSS px, with find highlights (`(range, current)`) under labels.
    pub fn element(
        self: &Rc<Self>,
        w: f32,
        h: f32,
        highlights: &HashMap<usize, Vec<(Range<usize>, bool)>>,
        colors: (gpui_kit::Hsla, gpui_kit::Hsla),
        window: &Window,
    ) -> AnyElement {
        let s = if self.view_box.2 > 0. { w / self.view_box.2 } else { 1. };
        let me = self.clone();
        let (cx, cy, cw, ch) = self.content;
        let off = ((cx - self.view_box.0) * s, (cy - self.view_box.1) * s);
        let ext = (cw * s, ch * s);
        let geometry = canvas(
            |_, _, _| {},
            move |b: Bounds<Pixels>, _, window, _| {
                let scale = window.scale_factor();
                let dw = (ext.0 * scale).round() as u32;
                let dh = (ext.1 * scale).round() as u32;
                if let Some(img) = me.rasterize(dw, dh, s, scale) {
                    let o = point(b.origin.x + px(off.0), b.origin.y + px(off.1));
                    let r = Bounds {
                        origin: point(
                            px((f32::from(o.x) * scale).round() / scale),
                            px((f32::from(o.y) * scale).round() / scale),
                        ),
                        size: size(px(dw as f32 / scale), px(dh as f32 / scale)),
                    };
                    let _ = window.paint_image(r, r, Corners::default(), img, 0, false);
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let mut el = div().relative().flex_none().w(px(w)).h(px(h)).child(geometry);
        if let Some(marks) = highlights.get(&0) {
            for (range, current) in marks {
                for (x, y, rw, rh) in self.range_rects(range.clone(), s, window) {
                    el = el.child(
                        div()
                            .absolute()
                            .left(px(x))
                            .top(px(y))
                            .w(px(rw))
                            .h(px(rh))
                            .bg(if *current { colors.1 } else { colors.0 }),
                    );
                }
            }
        }
        for r in &self.runs {
            let text = label_text(r, &r.text, s);
            let (tw, _, m) = text.measure(None, window);
            let left = (r.x - self.view_box.0) * s;
            let top = (r.y - self.view_box.1) * s - m.baseline;
            // `pre`-like: spaces kept, never wrapped.
            let dp = 1. / window.scale_factor();
            let boxed = div().flex_none().w(px((tw / dp).ceil() * dp + dp)).child(text);
            el = el.child(placed(left, top, boxed, window));
        }
        el.into_any_element()
    }
}

/// A box at an exact fractional offset (negative offsets fall back to rounded insets).
fn placed(left: f32, top: f32, child: impl IntoElement, window: &Window) -> AnyElement {
    if left < 0. || top < 0. {
        return div().absolute().left(px(left)).top(px(top)).child(child).into_any_element();
    }
    div()
        .absolute()
        .top_0()
        .left_0()
        .flex()
        .flex_col()
        .child(vgap(top, window))
        .child(div().flex().flex_row().child(hgap(left, window)).child(child))
        .into_any_element()
}

/// The CSS font stack WebKit resolved, as a recur-ui family.
fn family(stack: &str) -> Family {
    let names: Vec<String> = stack
        .split(',')
        .map(|f| f.trim().trim_matches(['"', '\'']).to_string())
        .filter(|f| !f.is_empty())
        .collect();
    match names.first().map(String::as_str) {
        None | Some("ui-sans-serif" | "system-ui" | "-apple-system" | "BlinkMacSystemFont") => Family::System,
        Some("Inter") => Family::Ui,
        Some(first) => Family::Stack(first.to_string().into(), names[1..].iter().map(|n| n.clone().into()).collect()),
    }
}

fn label_text(r: &Run, text: &str, s: f32) -> recur_ui::Text {
    let fill = Color::parse(&r.fill).unwrap_or(recur_ui::hex(0x000000));
    let alpha = fill.a * r.fill_opacity * r.opacity;
    let mut t = recur_ui::text(text.to_string())
        .family(family(&r.family))
        .size(r.size * s)
        .weight(r.weight)
        .leading_normal()
        .color(fill.with_alpha(alpha))
        .tracking_px(r.spacing * s)
        .pre_wrap();
    if r.italic {
        t = t.italic();
    }
    if r.decoration.contains("underline") {
        t = t.underline(true);
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unclip_and_sanitize_keep_what_gradients_need() {
        let out = sanitize_svg(
            r#"<svg overflow="hidden" id="m1" style="max-width: 120px" class="flowchart"><style>#m1 .node rect{fill:red}</style><g class="node" overflow="hidden"><text>Hello</text></g><script>alert(1)</script><foreignObject width="40" height="12"><div class="nodeLabel" onclick="x()">Hi</div></foreignObject></svg>"#,
        );
        assert!(out.contains(r#"id="m1""#));
        assert!(out.contains("max-width: 120px"));
        assert!(out.contains("<style>"));
        assert!(!out.contains("script"));
        assert!(!out.contains("onclick"));
    }

    #[test]
    fn xml_drops_replaced_labels_and_declares_namespaces() {
        let frag = dom::parse_fragment_in_div(
            r#"<svg viewBox="0 0 10 10"><text>a</text><text>b</text><image xlink:href="data:,"/></svg>"#,
        );
        let svg = find_svg(&frag.root).unwrap();
        let mut xml = String::new();
        let mut ix = 0;
        to_xml(&svg, true, &mut ix, &[1], &mut xml);
        assert!(xml.starts_with(r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 10 10">"#));
        assert!(!xml.contains(">a<"));
        assert!(xml.contains(">b<"));
        assert!(xml.contains(r#"xlink:href="data:,""#));
        assert!(crate::svg::parse(&xml).is_ok());
    }

    #[test]
    fn maps_font_stacks() {
        assert_eq!(family("ui-sans-serif, system-ui, sans-serif"), Family::System);
        assert_eq!(family("\"trebuchet ms\", verdana"), Family::Stack("trebuchet ms".into(), vec!["verdana".into()]));
    }
}
