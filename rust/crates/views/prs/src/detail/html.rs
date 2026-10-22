//! The trusted inline markup PR detail renders with `{@html}` (summary paragraphs and bullets,
//! the focus callout, verify steps, location lines): text with `<b>`, `<code>` and friends,
//! laid out as one block of spans.

use recur_content::render::doc::{Doc, Node};
use recur_ui::text::{Span, span};

/// Spans for `html`, each styled by `style(tags, span)` with the tags it sits in (outermost
/// first).
pub fn html_spans(html: &str, style: &dyn Fn(&[&str], Span) -> Span) -> Vec<Span> {
    let doc = Doc::parse(html);
    let mut out = Vec::new();
    let mut tags: Vec<&str> = Vec::new();
    walk(&doc.nodes, &mut tags, style, &mut out);
    out
}

fn walk<'a>(
    nodes: &'a [Node],
    tags: &mut Vec<&'a str>,
    style: &dyn Fn(&[&str], Span) -> Span,
    out: &mut Vec<Span>,
) {
    for n in nodes {
        match n {
            Node::Text(t) => out.push(style(tags, span(t.clone()))),
            Node::El(e) => {
                if e.is("br") {
                    out.push(style(tags, span("\n")));
                    continue;
                }
                tags.push(e.tag.as_str());
                walk(&e.children, tags, style, out);
                tags.pop();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use recur_ui::text::TextStyled;

    use super::*;

    #[test]
    fn spans_carry_their_tags() {
        let spans = html_spans("Run <b>tests</b> in <code>e2e.ts</code>.", &|tags, s| {
            if tags.contains(&"b") {
                s.weight(600.)
            } else {
                s
            }
        });
        let texts: Vec<&str> = spans.iter().map(|s| s.text.as_ref()).collect();
        assert_eq!(texts, ["Run ", "tests", " in ", "e2e.ts", "."]);
        assert_eq!(spans[1].style.weight, Some(600.));
        assert_eq!(spans[0].style.weight, None);
    }
}
