//! The tree a view renders: sanitized HTML parsed the way `{@html}` parses it (fragment parsing
//! in a `<div>`), copied into owned nodes so it can be built off the UI thread.

use markup5ever_rcdom::{Handle, NodeData};

use crate::purify::dom::{self, HTML_NS, SVG_NS};

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Text(String),
    El(El),
}

#[derive(Clone, Debug, PartialEq)]
pub struct El {
    /// Local name (case-adjusted for SVG).
    pub tag: String,
    pub svg: bool,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

impl El {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn has_class(&self, class: &str) -> bool {
        self.attr("class")
            .is_some_and(|c| c.split_ascii_whitespace().any(|x| x == class))
    }

    pub fn is(&self, tag: &str) -> bool {
        !self.svg && self.tag == tag
    }

    /// `textContent`.
    pub fn text_content(&self) -> String {
        let mut out = String::new();
        fn walk(n: &[Node], out: &mut String) {
            for c in n {
                match c {
                    Node::Text(t) => out.push_str(t),
                    Node::El(e) => walk(&e.children, out),
                }
            }
        }
        walk(&self.children, &mut out);
        out
    }
}

/// A parsed document: the top-level nodes of the markdown container.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Doc {
    pub nodes: Vec<Node>,
}

fn convert(h: &Handle) -> Option<Node> {
    match &h.data {
        NodeData::Text { contents } => Some(Node::Text(contents.borrow().to_string())),
        NodeData::Element { name, .. } => {
            let ns = &*name.ns;
            if ns != HTML_NS && ns != SVG_NS {
                return None;
            }
            let source = dom::template_contents(h).unwrap_or_else(|| h.clone());
            let children = source
                .children
                .borrow()
                .iter()
                .filter_map(convert)
                .collect();
            Some(Node::El(El {
                tag: name.local.to_string(),
                svg: ns == SVG_NS,
                attrs: dom::attrs(h),
                children,
            }))
        }
        _ => None,
    }
}

impl Doc {
    /// Parse sanitized HTML as `el.innerHTML = html` would.
    pub fn parse(html: &str) -> Doc {
        if html.is_empty() {
            return Doc::default();
        }
        let frag = dom::parse_fragment_in_div(html);
        let nodes = frag
            .root
            .children
            .borrow()
            .iter()
            .filter_map(convert)
            .collect();
        Doc { nodes }
    }

    /// Every element in tree order.
    pub fn walk<'a>(&'a self, f: &mut impl FnMut(&'a El)) {
        fn go<'a>(nodes: &'a [Node], f: &mut impl FnMut(&'a El)) {
            for n in nodes {
                if let Node::El(e) = n {
                    f(e);
                    go(&e.children, f);
                }
            }
        }
        go(&self.nodes, f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fragments_like_inner_html() {
        let d = Doc::parse("<p>a <strong>b</strong></p>\n<pre><code>x</code></pre>");
        assert_eq!(d.nodes.len(), 3);
        let Node::El(p) = &d.nodes[0] else { panic!() };
        assert_eq!(p.tag, "p");
        assert_eq!(p.text_content(), "a b");
    }
}
