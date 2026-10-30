//! Sanitized HTML parsed the way `innerHTML` does (html5ever is a spec parser) into an rcdom
//! tree, for the native layout and the Mermaid diagram.

use html5ever::tendril::TendrilSink;
use html5ever::{LocalName, Namespace, QualName, parse_fragment};
use markup5ever_rcdom::{Handle, NodeData, RcDom};

pub const HTML_NS: &str = "http://www.w3.org/1999/xhtml";
pub const SVG_NS: &str = "http://www.w3.org/2000/svg";

/// A parsed fragment: `root` holds the top-level nodes; the document keeps the tree alive.
pub struct Fragment {
    _document: Handle,
    pub root: Handle,
}

/// `el.innerHTML = html` on a `<div>` in the live document (scripting enabled).
pub fn parse_fragment_in_div(html: &str) -> Fragment {
    let dom = parse_fragment(
        RcDom::default(),
        html5ever::ParseOpts::default(),
        QualName::new(None, Namespace::from(HTML_NS), LocalName::from("div")),
        Vec::new(),
    )
    .one(html);
    // The fragment parser roots its nodes under a synthetic `<html>` element.
    let root = dom
        .document
        .children
        .borrow()
        .first()
        .cloned()
        .unwrap_or(dom.document.clone());
    Fragment {
        _document: dom.document,
        root,
    }
}

pub fn template_contents(node: &Handle) -> Option<Handle> {
    match &node.data {
        NodeData::Element {
            template_contents, ..
        } => template_contents.borrow().clone(),
        _ => None,
    }
}

fn element_name(node: &Handle) -> Option<&QualName> {
    match &node.data {
        NodeData::Element { name, .. } => Some(name),
        _ => None,
    }
}

pub fn namespace(node: &Handle) -> Option<&str> {
    element_name(node).map(|n| &*n.ns)
}

/// `localName`: what a spec parser stored (case-adjusted for SVG).
pub fn local_name(node: &Handle) -> Option<&str> {
    element_name(node).map(|n| &*n.local)
}

/// An attribute's `name` (its qualified name).
pub fn attr_name(a: &html5ever::Attribute) -> String {
    match &a.name.prefix {
        Some(p) if !p.is_empty() => format!("{p}:{}", a.name.local),
        _ => a.name.local.to_string(),
    }
}

pub fn attrs(node: &Handle) -> Vec<(String, String)> {
    match &node.data {
        NodeData::Element { attrs, .. } => attrs
            .borrow()
            .iter()
            .map(|a| (attr_name(a), a.value.to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

pub fn get_attr(node: &Handle, name: &str) -> Option<String> {
    match &node.data {
        NodeData::Element { attrs, .. } => attrs
            .borrow()
            .iter()
            .find(|a| attr_name(a) == name)
            .map(|a| a.value.to_string()),
        _ => None,
    }
}
