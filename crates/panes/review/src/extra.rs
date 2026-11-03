//! A diff row's trusted `extraHtml`: backend risk flags (`.flag`) and the inline comment cards
//! of seeded diffs (`.icmt .cmt`), laid out with the styles those classes get. Elements
//! without a rule lay out as plain blocks and inline text.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, Div, IntoElement, ParentElement, SharedString, Styled, Window, div, px,
};
use recur_content::render::doc::{El, Node};
use recur_ui::fonts::Family;
use recur_ui::text::{InlineBox, LineHeight, Span, Text, TextStyled, span};
use recur_ui::{Color, Theme, Tokens, svg_markup};

use crate::html::inline_style;
use crate::style::{HUNK_BG, REACT_BG, REACT_BORDER, hsla};
use recur_view_widgets::avatar::{avatar_with, parse_avatar_css};

/// Render `extraHtml` under a row. `key` scopes element ids.
pub fn extra_html(html: &str, key: &str, window: &mut Window, cx: &App) -> AnyElement {
    let nodes = crate::html::parse(html);
    let mut col = div().flex().flex_col();
    for (i, n) in nodes.iter().enumerate() {
        if let Some(el) = block(n, &format!("{key}-{i}"), window, cx) {
            col = col.child(el);
        }
    }
    col.into_any_element()
}

fn block(n: &Node, key: &str, window: &mut Window, cx: &App) -> Option<AnyElement> {
    let t = Theme::of(cx).tokens;
    match n {
        Node::Text(s) if s.trim().is_empty() => None,
        Node::Text(s) => Some(
            Text::from_spans(vec![span(collapse(s))])
                .id(SharedString::from(format!("x-{key}")))
                .line_height(LineHeight::Scale(1.45))
                .into_any_element(),
        ),
        Node::El(e) if e.has_class("flag") => Some(flag(e, key, &t)),
        Node::El(e) if e.has_class("icmt") => Some(
            div()
                .flex()
                .flex_col()
                .bg(hsla(t.rail))
                .border_t_1()
                .border_b_1()
                .border_color(hsla(t.line))
                .children(children_blocks(e, key, window, cx))
                .into_any_element(),
        ),
        Node::El(e) if e.has_class("icmt-pad") => Some(
            div()
                .pt(px(12.))
                .pr(px(16.))
                .pb(px(12.))
                .pl(px(92.))
                .children(children_blocks(e, key, window, cx))
                .into_any_element(),
        ),
        Node::El(e) if e.has_class("cmt") => Some(cmt(e, key, window, cx)),
        Node::El(e) => Some(
            div()
                .flex()
                .flex_col()
                .children(children_blocks(e, key, window, cx))
                .into_any_element(),
        ),
    }
}

fn children_blocks(e: &El, key: &str, window: &mut Window, cx: &App) -> Vec<AnyElement> {
    e.children
        .iter()
        .enumerate()
        .filter_map(|(i, c)| block(c, &format!("{key}-{i}"), window, cx))
        .collect()
}

fn child_el<'a>(e: &'a El, class: &str) -> Option<&'a El> {
    e.children.iter().find_map(|n| match n {
        Node::El(c) if c.has_class(class) => Some(c),
        _ => None,
    })
}

/// Collapse white space as `white-space: normal` does.
fn collapse(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars() {
        if c.is_ascii_whitespace() {
            space = true;
        } else {
            if space {
                out.push(' ');
                space = false;
            }
            out.push(c);
        }
    }
    if space {
        out.push(' ');
    }
    out
}

/// The SVG element back as markup (`outerHTML`).
fn svg_source(e: &El) -> String {
    fn go(e: &El, out: &mut String) {
        out.push('<');
        out.push_str(&e.tag);
        if e.tag == "svg" && e.attr("xmlns").is_none() {
            out.push_str(" xmlns=\"http://www.w3.org/2000/svg\"");
        }
        for (k, v) in &e.attrs {
            out.push_str(&format!(" {k}=\"{}\"", v.replace('"', "&quot;")));
        }
        out.push('>');
        for c in &e.children {
            match c {
                Node::El(k) => go(k, out),
                Node::Text(s) => out.push_str(s),
            }
        }
        out.push_str(&format!("</{}>", e.tag));
    }
    let mut s = String::new();
    go(e, &mut s);
    s
}

/// `.flag`: icon, `.ft` title and `.fm` message on the hunk wash.
fn flag(e: &El, key: &str, t: &Tokens) -> AnyElement {
    let svg = e.children.iter().find_map(|n| match n {
        Node::El(c) if c.svg && c.tag == "svg" => Some(svg_source(c)),
        _ => None,
    });
    let body = e.children.iter().find_map(|n| match n {
        Node::El(c) if !c.svg => Some(c),
        _ => None,
    });
    let ft = body
        .and_then(|b| child_el(b, "ft"))
        .map(|x| collapse(&x.text_content()))
        .unwrap_or_default();
    let fm = body
        .and_then(|b| child_el(b, "fm"))
        .map(|x| collapse(&x.text_content()))
        .unwrap_or_default();
    div()
        .flex()
        .gap(px(9.))
        .pt(px(10.))
        .pr(px(16.))
        .pb(px(10.))
        .pl(px(92.))
        .bg(hsla(HUNK_BG))
        .border_t_1()
        .border_b_1()
        .border_color(hsla(t.line))
        .text_size(px(11.5))
        .when_some(svg, |d, s| {
            d.child(
                div()
                    .flex_none()
                    .mt(px(2.))
                    .child(svg_markup(s).size(px(13.)).color(hsla(t.indigo))),
            )
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    Text::from_spans(vec![span(ft)])
                        .id(SharedString::from(format!("ft-{key}")))
                        .size(11.5)
                        .line_height(LineHeight::Scale(1.45))
                        .color(t.text),
                )
                .child(
                    div().mt(px(1.)).child(
                        Text::from_spans(vec![span(fm)])
                            .id(SharedString::from(format!("fm-{key}")))
                            .mono()
                            .size(10.)
                            .line_height(LineHeight::Scale(1.45))
                            .color(t.indigo),
                    ),
                ),
        )
        .into_any_element()
}

/// Inline content with `<code>` chips (`.cmt .cm code`) as spans.
fn inline_spans(nodes: &[Node], t: &Tokens, out: &mut Vec<Span>) {
    for n in nodes {
        match n {
            Node::Text(s) => out.push(span(collapse(s))),
            Node::El(e) if e.is("code") => out.push(
                span(collapse(&e.text_content()))
                    .mono()
                    .size(11.)
                    .boxed(InlineBox {
                        pad_top: 1.,
                        pad_bottom: 1.,
                        pad_left: 5.,
                        pad_right: 5.,
                        background: Some(t.pane_2.into()),
                        radius: 3.,
                        ..Default::default()
                    }),
            ),
            Node::El(e) => inline_spans(&e.children, t, out),
        }
    }
}

fn trim_spans(mut spans: Vec<Span>) -> Vec<Span> {
    if let Some(first) = spans.first_mut() {
        first.text = SharedString::from(first.text.trim_start().to_string());
    }
    if let Some(last) = spans.last_mut() {
        last.text = SharedString::from(last.text.trim_end().to_string());
    }
    spans.retain(|s| !s.text.is_empty() || s.inline_box.is_some());
    spans
}

/// `.cmt`: avatar, then `.cc` with the `.ch` header, `.cm` body and `.react` line.
fn cmt(e: &El, key: &str, window: &mut Window, cx: &App) -> AnyElement {
    let t = Theme::of(cx).tokens;
    let style = inline_style(e);
    let mut pad_y = 14.;
    let mut border = true;
    for (k, v) in &style {
        match (k.as_str(), v.as_str()) {
            ("padding", "0") => pad_y = 0.,
            ("border", "none") => border = false,
            _ => {}
        }
    }
    let mut row = div()
        .flex()
        .gap(px(12.))
        .py(px(pad_y))
        .min_w_0()
        .when(border, |d| d.border_b_1().border_color(hsla(t.line_soft)));
    if let Some(ca) = child_el(e, "ca") {
        let (fill, color) = parse_avatar_css(ca.attr("style").unwrap_or_default(), cx);
        row = row.child(avatar_with(
            &collapse(&ca.text_content()),
            fill,
            color,
            "",
            28.,
            6.,
            10.,
            LineHeight::Scale(1.45),
        ));
    }
    let mut cc = div().flex_1().min_w_0();
    if let Some(cc_el) = child_el(e, "cc") {
        for (i, n) in cc_el.children.iter().enumerate() {
            let Node::El(c) = n else { continue };
            let k = format!("{key}-cc{i}");
            if c.has_class("ch") {
                cc = cc.child(ch(c, &t));
            } else if c.has_class("cm") {
                let mut spans = Vec::new();
                inline_spans(&c.children, &t, &mut spans);
                cc = cc.child(
                    Text::from_spans(trim_spans(spans))
                        .id(SharedString::from(format!("cm-{k}")))
                        .size(12.5)
                        .line_height(LineHeight::Scale(1.55))
                        // `.cm` is also the comment token class: `color: var(--faint)`.
                        .color(t.faint)
                        .break_anywhere(),
                );
            } else if c.has_class("react") {
                cc = cc.child(react(c, &k, window, &t));
            } else if let Some(b) = block(n, &k, window, cx) {
                cc = cc.child(b);
            }
        }
    }
    row.child(cc).into_any_element()
}

fn ch(e: &El, t: &Tokens) -> Div {
    let mut d = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .mb(px(5.))
        .text_color(hsla(t.muted))
        .text_size(px(12.5));
    for n in &e.children {
        let Node::El(c) = n else { continue };
        let s = collapse(&c.text_content());
        if c.has_class("cn") {
            d = d.child(
                Text::from_spans(vec![span(s)])
                    .size(12.5)
                    .weight(600.)
                    .line_height(LineHeight::Scale(1.45))
                    .color(t.text)
                    .nowrap(),
            );
        } else if c.has_class("ct") {
            d = d.child(
                Text::from_spans(vec![span(s)])
                    .mono()
                    .size(9.5)
                    .line_height(LineHeight::Scale(1.45))
                    .color(t.faint)
                    .nowrap(),
            );
        }
    }
    d
}

/// `.react` is an inline-flex box: the dot, each text run and each `<code>` are flex items
/// `gap: 6px` apart.
fn react(e: &El, key: &str, _window: &mut Window, t: &Tokens) -> AnyElement {
    let style = inline_style(e);
    let mt = style
        .iter()
        .find(|(k, _)| k == "margin-top")
        .and_then(|(_, v)| v.strip_suffix("px")?.parse::<f32>().ok())
        .unwrap_or(9.);
    let mut items: Vec<AnyElement> = Vec::new();
    for (i, n) in e.children.iter().enumerate() {
        match n {
            Node::Text(s) => {
                let s = collapse(s);
                let s = s.trim();
                if s.is_empty() {
                    continue;
                }
                items.push(
                    Text::from_spans(vec![span(s.to_string())])
                        .id(SharedString::from(format!("rt-{key}-{i}")))
                        .mono()
                        .size(10.5)
                        .line_height(LineHeight::Scale(1.45))
                        .into_any_element(),
                );
            }
            // No rule styles `<code>` here: the UA `monospace`, which WebKit resolves to Menlo.
            Node::El(c) if c.is("code") => items.push(
                Text::from_spans(vec![span(collapse(&c.text_content()))])
                    .id(SharedString::from(format!("rc-{key}-{i}")))
                    .family(Family::Stack("Menlo".into(), Vec::new()))
                    .size(10.5)
                    .line_height(LineHeight::Scale(1.45))
                    .into_any_element(),
            ),
            Node::El(c) => {
                let st = inline_style(c);
                let num = |name: &str| {
                    st.iter()
                        .find(|(k, _)| k == name)
                        .and_then(|(_, v)| v.strip_suffix("px")?.parse::<f32>().ok())
                };
                let bg = st
                    .iter()
                    .find(|(k, _)| k == "background")
                    .and_then(|(_, v)| var_color(v, t));
                let (w, h) = (num("width").unwrap_or(0.), num("height").unwrap_or(0.));
                items.push(
                    div()
                        .flex_none()
                        .w(px(w))
                        .h(px(h))
                        .rounded(px(w.max(h)))
                        .when_some(bg, |d, c| d.bg(hsla(c)))
                        .into_any_element(),
                );
            }
        }
    }
    div()
        .flex()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .mt(px(mt))
                .py(px(4.))
                .px(px(10.))
                .rounded(px(6.))
                .border_1()
                .border_color(hsla(REACT_BORDER))
                .bg(hsla(REACT_BG))
                .text_color(hsla(t.cyan))
                .font_family(Family::Mono.primary())
                .text_size(px(10.5))
                .children(items),
        )
        .into_any_element()
}

fn var_color(v: &str, t: &Tokens) -> Option<Color> {
    let v = v.trim();
    if let Some(name) = v.strip_prefix("var(").and_then(|s| s.strip_suffix(')')) {
        return t.by_name(name);
    }
    Color::parse(v)
}
