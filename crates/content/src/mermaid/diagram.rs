//! A rendered Mermaid diagram drawn natively: its sanitized SVG's geometry rasterized at the
//! device size (text elements removed), and every label line as recur-ui text at the start
//! position usvg shaped it at. Labels transformed beyond translate-and-scale stay in the raster.

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

use crate::purify::{self, Config, dom};

fn svg_config() -> &'static Config {
    static CFG: OnceLock<Config> = OnceLock::new();
    CFG.get_or_init(Config::mermaid_svg)
}

/// `sanitizeMermaidSvg`.
pub fn sanitize_svg(svg: &str) -> String {
    purify::sanitize(svg, svg_config())
}

/// One label line as usvg laid it out (viewBox units, `y` the baseline).
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub family: String,
    pub weight: f32,
    pub italic: bool,
    pub fill: String,
    pub fill_opacity: f32,
    pub opacity: f32,
    pub spacing: f32,
    pub decoration: String,
    /// The run's advance in user units, when known (0: draw at recur-ui's own advance).
    pub advance: f32,
}

/// The id `to_xml` gives the `<text>` element at a document-order index.
const TEXT_ID: &str = "recur-text-";

/// A raster's device size and the bits of its sub-pixel offset.
type RasterKey = (u32, u32, u32, u32);

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
    raster: RefCell<Option<(RasterKey, Arc<RenderImage>)>>,
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

/// The SVG subtree as XML for usvg: the root sized to its viewBox (`root`), each `<text>` given
/// its [`TEXT_ID`], and with `keep` only the `<text>` elements it lists.
fn to_xml(
    h: &Handle,
    root: Option<(f32, f32)>,
    text_ix: &mut usize,
    keep: Option<&[usize]>,
    out: &mut String,
) {
    match &h.data {
        NodeData::Element { name, attrs, .. } => {
            let local = &*name.local;
            let text = &*name.ns == dom::SVG_NS && local == "text";
            out.push('<');
            out.push_str(local);
            if text {
                let ix = *text_ix;
                *text_ix += 1;
                if keep.is_some_and(|k| !k.contains(&ix)) {
                    out.truncate(out.len() - local.len() - 1);
                    return;
                }
                out.push_str(&format!(r#" id="{TEXT_ID}{ix}""#));
            }
            if let Some((w, h)) = root {
                out.push_str(&format!(r#" xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="{w}" height="{h}""#));
            }
            for a in attrs.borrow().iter() {
                let n = dom::attr_name(a);
                if n == "xmlns"
                    || n.starts_with("xmlns:")
                    || (text && n == "id")
                    || (root.is_some() && matches!(n.as_str(), "width" | "height"))
                {
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
                to_xml(c, None, text_ix, keep, out);
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

/// Each `<text>` usvg laid out: a line drawn by translate-and-scale becomes a [`Run`]; the
/// indices of the rest stay in the raster.
fn labels(
    g: &usvg::Group,
    opacity: f32,
    vb: (f32, f32),
    db: &usvg::fontdb::Database,
    runs: &mut Vec<Run>,
    raster: &mut Vec<usize>,
) {
    for n in g.children() {
        let t = match n {
            usvg::Node::Group(c) => {
                labels(c, opacity * c.opacity().get(), vb, db, runs, raster);
                continue;
            }
            usvg::Node::Text(t) => t,
            _ => continue,
        };
        let Some(ix) = t.id().strip_prefix(TEXT_ID).and_then(|n| n.parse().ok()) else {
            continue;
        };
        let m = t.abs_transform();
        let simple_paint = t.layouted().iter().all(|s| {
            s.stroke.is_none()
                && s.fill
                    .as_ref()
                    .is_none_or(|f| matches!(f.paint(), usvg::Paint::Color(_)))
        });
        if !simple_paint || m.kx != 0. || m.ky != 0. || m.sx <= 0. || m.sx != m.sy {
            raster.push(ix);
            continue;
        }
        for span in t.layouted() {
            let Some(fill) = span.fill.as_ref().filter(|_| span.visible) else {
                continue;
            };
            let usvg::Paint::Color(c) = fill.paint() else {
                continue;
            };
            let mut line: Option<Run> = None;
            for g in &span.positioned_glyphs {
                let ts = g.transform();
                let (x, y) = (m.sx * ts.tx + m.tx + vb.0, m.sy * ts.ty + m.ty + vb.1);
                if let Some(r) = line.as_mut().filter(|r| r.y == y && x >= r.x) {
                    r.text.push_str(&g.text);
                    continue;
                }
                runs.extend(line.take());
                let face = db.face(g.font);
                line = Some(Run {
                    text: g.text.clone(),
                    x,
                    y,
                    size: span.font_size.get() * m.sx,
                    family: face
                        .and_then(|f| f.families.first())
                        .map_or_else(String::new, |(n, _)| n.clone()),
                    weight: face.map_or(400., |f| f32::from(f.weight.0)),
                    italic: face.is_some_and(|f| f.style != usvg::fontdb::Style::Normal),
                    fill: format!("#{:02x}{:02x}{:02x}", c.red, c.green, c.blue),
                    fill_opacity: fill.opacity().get(),
                    opacity,
                    spacing: 0.,
                    decoration: if span.underline.is_some() {
                        "underline".into()
                    } else {
                        String::new()
                    },
                    advance: 0.,
                });
            }
            runs.extend(line);
        }
    }
}

impl Diagram {
    /// Build from sanitized SVG markup.
    pub fn build(clean: &str) -> Result<Diagram, String> {
        let frag = dom::parse_fragment_in_div(clean);
        let svg = find_svg(&frag.root).ok_or("Mermaid returned no SVG")?;
        let width_attr = dom::get_attr(&svg, "width").unwrap_or_default();
        let style = dom::get_attr(&svg, "style").unwrap_or_default();
        let max_width = style.split(';').find_map(|decl| {
            let (k, v) = decl.split_once(':')?;
            (k.trim() == "max-width").then(|| px_len(v)).flatten()
        });
        let view_box: Vec<f32> = dom::get_attr(&svg, "viewBox")
            .unwrap_or_default()
            .split([' ', ','])
            .filter_map(|v| v.parse().ok())
            .collect();
        let vb = match view_box[..] {
            [x, y, w, h] if w > 0. && h > 0. => (x, y, w, h),
            _ => {
                let len = |n| {
                    dom::get_attr(&svg, n)
                        .and_then(|v| px_len(&v))
                        .unwrap_or(0.)
                };
                (0., 0., len("width"), len("height"))
            }
        };
        let fluid = width_attr.trim().ends_with('%');
        let natural_width = max_width.or_else(|| px_len(&width_attr)).unwrap_or(vb.2);
        let xml = |keep: Option<&[usize]>| {
            let mut xml = String::new();
            to_xml(&svg, Some((vb.2, vb.3)), &mut 0, keep, &mut xml);
            xml
        };
        let laid_out = crate::svg::parse(&xml(None))?;
        let (mut runs, mut raster) = (Vec::new(), Vec::new());
        labels(
            laid_out.root(),
            1.,
            (vb.0, vb.1),
            laid_out.fontdb(),
            &mut runs,
            &mut raster,
        );
        let tree = crate::svg::parse(&xml(Some(&raster)))?;
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
            runs,
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
        self.element(
            w,
            self.height_at(w),
            &HashMap::new(),
            (none, none),
            (0., 0.),
            window,
        )
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
    pub fn range_rects(
        &self,
        range: Range<usize>,
        s: f32,
        window: &Window,
    ) -> Vec<(f32, f32, f32, f32)> {
        let mut out = Vec::new();
        for (r, start) in self.runs.iter().zip(self.run_starts()) {
            let end = start + r.text.len();
            let (a, b) = (range.start.max(start), range.end.min(end));
            if a >= b {
                continue;
            }
            let tracking = run_tracking(r, s, window);
            let t = |x: &str| label_text(r, x, s, tracking).measure(None, window);
            let (_, h, m) = t(&r.text);
            let x0 = t(&r.text[..a - start]).0;
            let x1 = t(&r.text[..b - start]).0;
            let left = (r.x - self.view_box.0) * s;
            let top = (r.y - self.view_box.1) * s - m.baseline;
            out.push((left + x0, top, x1 - x0, h));
        }
        out
    }

    /// The geometry at display scale `s`, offset by the device-pixel fraction `frac` WebKit
    /// paints at (it never snaps a transformed SVG to the pixel grid).
    fn rasterize(
        &self,
        w: u32,
        h: u32,
        s: f32,
        scale: f32,
        frac: (f32, f32),
    ) -> Option<Arc<RenderImage>> {
        let key = (w, h, frac.0.to_bits(), frac.1.to_bits());
        if let Some((k, img)) = self.raster.borrow().as_ref()
            && *k == key
        {
            return Some(img.clone());
        }
        let k = s * scale;
        let (cx, cy, _, _) = self.content;
        let ts = tiny_skia::Transform::from_row(
            k,
            0.,
            0.,
            k,
            frac.0 - (cx - self.view_box.0) * k,
            frac.1 - (cy - self.view_box.1) * k,
        );
        let img = crate::svg::to_render_image(crate::svg::rasterize(&self.tree, ts, w, h)?, w, h)?;
        *self.raster.borrow_mut() = Some((key, img.clone()));
        Some(img)
    }

    /// The diagram at `w`×`h` CSS px, with find highlights (`(range, current)`) under labels.
    /// `nudge` is the sub-pixel part of the element's offset that layout snapped away.
    pub fn element(
        self: &Rc<Self>,
        w: f32,
        h: f32,
        highlights: &HashMap<usize, Vec<(Range<usize>, bool)>>,
        colors: (gpui_kit::Hsla, gpui_kit::Hsla),
        nudge: (f32, f32),
        window: &Window,
    ) -> AnyElement {
        let s = if self.view_box.2 > 0. {
            w / self.view_box.2
        } else {
            1.
        };
        let me = self.clone();
        let (cx, cy, cw, ch) = self.content;
        let off = (
            (cx - self.view_box.0) * s + nudge.0,
            (cy - self.view_box.1) * s + nudge.1,
        );
        let ext = (cw * s, ch * s);
        let geometry = canvas(
            |_, _, _| {},
            move |b: Bounds<Pixels>, _, window, _| {
                let scale = window.scale_factor();
                let ox = (f32::from(b.origin.x) + off.0) * scale;
                let oy = (f32::from(b.origin.y) + off.1) * scale;
                let frac = (ox - ox.floor(), oy - oy.floor());
                let dw = (ext.0 * scale + frac.0).ceil() as u32;
                let dh = (ext.1 * scale + frac.1).ceil() as u32;
                if let Some(img) = me.rasterize(dw, dh, s, scale, frac) {
                    let r = Bounds {
                        origin: point(px(ox.floor() / scale), px(oy.floor() / scale)),
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
        let mut el = div()
            .relative()
            .flex_none()
            .w(px(w))
            .h(px(h))
            .child(geometry);
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
            let text = label_text(r, &r.text, s, run_tracking(r, s, window));
            let (tw, _, m) = text.measure(None, window);
            let left = (r.x - self.view_box.0) * s + nudge.0;
            let top = (r.y - self.view_box.1) * s - m.baseline + nudge.1;
            // `pre`-like: spaces kept, never wrapped.
            let dp = 1. / window.scale_factor();
            let boxed = div()
                .flex_none()
                .w(px((tw / dp).ceil() * dp + dp))
                .child(text);
            el = el.child(placed(left, top, boxed));
        }
        el.into_any_element()
    }
}

/// A box at an offset.
fn placed(left: f32, top: f32, child: impl IntoElement) -> AnyElement {
    div()
        .absolute()
        .left(px(left))
        .top(px(top))
        .child(child)
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
        None | Some("ui-sans-serif" | "system-ui" | "-apple-system" | "BlinkMacSystemFont") => {
            Family::System
        }
        Some("Inter") => Family::Ui,
        Some(first) => Family::Stack(
            first.to_string().into(),
            names[1..].iter().map(|n| n.clone().into()).collect(),
        ),
    }
}

/// WebKit shapes SVG text at the device-scaled font size, whose tracking differs from the CSS
/// size's, so the run's spacing absorbs the difference to land on WebKit's advance.
fn run_tracking(r: &Run, s: f32, window: &Window) -> f32 {
    let base = r.spacing * s;
    let n = r.text.chars().count();
    if r.advance <= 0. || n == 0 {
        return base;
    }
    let (w, _, _) = label_text(r, &r.text, s, base).measure(None, window);
    base + (r.advance * s - w) / n as f32
}

fn label_text(r: &Run, text: &str, s: f32, tracking: f32) -> recur_ui::Text {
    let fill = Color::parse(&r.fill).unwrap_or(recur_ui::hex(0x000000));
    let alpha = fill.a * r.fill_opacity * r.opacity;
    let mut t = recur_ui::text(text.to_string())
        .family(family(&r.family))
        .size(r.size * s)
        .weight(r.weight)
        .leading_normal()
        .color(fill.with_alpha(alpha))
        .tracking_px(tracking)
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
        to_xml(&svg, Some((10., 10.)), &mut ix, Some(&[1]), &mut xml);
        assert!(xml.starts_with(r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="10" height="10" viewBox="0 0 10 10">"#));
        assert!(!xml.contains(">a<"));
        assert!(xml.contains(r#"<text id="recur-text-1">b<"#));
        assert!(xml.contains(r#"xlink:href="data:,""#));
        assert!(crate::svg::parse(&xml).is_ok());
    }

    #[test]
    fn maps_font_stacks() {
        assert_eq!(
            family("ui-sans-serif, system-ui, sans-serif"),
            Family::System
        );
        assert_eq!(
            family("\"trebuchet ms\", verdana"),
            Family::Stack("trebuchet ms".into(), vec!["verdana".into()])
        );
    }
}
