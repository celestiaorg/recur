//! Engine-fact markup in a row (`{@html}` of loop facts and commit / handoff notices): the
//! `<b>`, `<strong>`, `<code>`, `<a>` and `<br>` the engine emits, as one recur-ui text.

use gpui_kit::{AnyElement, IntoElement, SharedString};
use recur_content::render::doc::{Doc, Node};
use recur_ui::fonts::Family;
use recur_ui::text::{InlineBox, LineHeight, Span, Text, TextStyled, WhiteSpace, span};
use recur_ui::{Color, Tokens};

/// How the surrounding row styles the markup.
#[derive(Clone)]
pub struct InlineStyle {
    pub size: f32,
    pub color: Color,
    pub family: Family,
    /// `white-space: pre-wrap` with `br { display: none }` (`.loop-fact .tc-meta`); otherwise
    /// `white-space: normal` with real line breaks.
    pub pre_wrap: bool,
    /// `code` color and background.
    pub code_fg: Color,
    pub code_bg: Color,
    /// `b` / `strong`: weight and color (`.loop-wrap b` is muted 600).
    pub bold_weight: f32,
    pub bold_color: Option<Color>,
    pub link: Color,
    pub leading: Option<f32>,
}

impl InlineStyle {
    pub fn meta(size: f32, color: Color, t: &Tokens) -> InlineStyle {
        InlineStyle {
            size,
            color,
            family: Family::Ui,
            pre_wrap: false,
            code_fg: t.text,
            code_bg: t.pane_2,
            bold_weight: 700.,
            bold_color: None,
            link: t.cyan,
            leading: None,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Run {
    bold: bool,
    code: bool,
    link: bool,
}

fn collapse(s: &str, after_space: &mut bool) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_whitespace() {
            if !*after_space {
                out.push(' ');
                *after_space = true;
            }
        } else {
            out.push(c);
            *after_space = false;
        }
    }
    out
}

fn walk(
    nodes: &[Node],
    run: Run,
    st: &InlineStyle,
    out: &mut Vec<(String, Run)>,
    after_space: &mut bool,
) {
    for n in nodes {
        match n {
            Node::Text(t) => {
                let s = if st.pre_wrap {
                    t.clone()
                } else {
                    collapse(t, after_space)
                };
                if !s.is_empty() {
                    out.push((s, run));
                }
            }
            Node::El(e) => {
                let mut r = run;
                match e.tag.as_str() {
                    "br" => {
                        if !st.pre_wrap {
                            out.push(("\n".into(), run));
                            *after_space = true;
                        }
                        continue;
                    }
                    "b" | "strong" => r.bold = true,
                    "code" => r.code = true,
                    "a" => r.link = true,
                    _ => {}
                }
                walk(&e.children, r, st, out, after_space);
            }
        }
    }
}

/// The spans of `html`.
pub fn inline_spans(html: &str, st: &InlineStyle) -> Vec<Span> {
    let doc = Doc::parse(html);
    let mut runs = Vec::new();
    let mut after_space = true;
    walk(&doc.nodes, Run::default(), st, &mut runs, &mut after_space);
    if !st.pre_wrap
        && let Some((last, _)) = runs.last_mut()
        && last.ends_with(' ')
    {
        last.pop();
    }
    runs.into_iter()
        .filter(|(s, _)| !s.is_empty())
        .map(|(s, r)| {
            let mut sp = span(s);
            if r.code {
                sp = sp.mono().size(11.).color(st.code_fg).boxed(InlineBox {
                    pad_top: 1.,
                    pad_bottom: 1.,
                    pad_left: 5.,
                    pad_right: 5.,
                    background: Some(st.code_bg.into()),
                    radius: 3.,
                    ..InlineBox::default()
                });
            } else if r.link {
                sp = sp.color(st.link);
            }
            if r.bold {
                sp = sp.weight(st.bold_weight);
                if let Some(c) = st.bold_color {
                    sp = sp.color(c);
                }
            }
            sp
        })
        .collect()
}

/// `html` as a wrapping text in the row's style.
pub fn inline_html(id: SharedString, html: &str, st: InlineStyle) -> AnyElement {
    let mut t = Text::from_spans(inline_spans(html, &st))
        .id(id)
        .family(st.family.clone())
        .size(st.size)
        .color(st.color)
        .break_anywhere();
    if st.pre_wrap {
        t = t.white_space(WhiteSpace::PreWrap);
    }
    if let Some(l) = st.leading {
        t = t.line_height(LineHeight::Scale(l));
    }
    t.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_markup_becomes_styled_spans() {
        let t = recur_ui::ThemeId::default().tokens();
        let st = InlineStyle::meta(11., t.muted, &t);
        let spans = inline_spans(
            "Committed <code>abc</code> (+3 −1) to <b>main</b>.<br>next",
            &st,
        );
        let texts: Vec<&str> = spans.iter().map(|s| s.text.as_ref()).collect();
        assert_eq!(
            texts,
            [
                "Committed ",
                "abc",
                " (+3 −1) to ",
                "main",
                ".",
                "\n",
                "next"
            ]
        );
        assert!(spans[1].inline_box.is_some());
        assert_eq!(spans[3].style.weight, Some(700.));
        let pre = InlineStyle {
            pre_wrap: true,
            ..st.clone()
        };
        let spans = inline_spans("a<br>b", &pre);
        assert_eq!(
            spans.iter().map(|s| s.text.to_string()).collect::<Vec<_>>(),
            ["a", "b"]
        );
    }
}
