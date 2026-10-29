//! The DOM DOMPurify walks: html5ever's spec parser (what `DOMParser` and `innerHTML` run)
//! into an rcdom tree, the mutations the sanitizer makes, and WebKit's HTML fragment
//! serialization (`innerHTML`).

use std::rc::Rc;

use html5ever::tendril::TendrilSink;
use html5ever::{LocalName, Namespace, QualName, parse_document, parse_fragment};
use markup5ever_rcdom::{Handle, Node, NodeData, RcDom};

pub const HTML_NS: &str = "http://www.w3.org/1999/xhtml";
pub const SVG_NS: &str = "http://www.w3.org/2000/svg";
pub const MATHML_NS: &str = "http://www.w3.org/1998/Math/MathML";

/// `new DOMParser().parseFromString(html, 'text/html')` (no browsing context, so scripting
/// is disabled), returning the document.
pub fn parse_html_document(html: &str) -> Handle {
    let opts = html5ever::ParseOpts {
        tree_builder: html5ever::tree_builder::TreeBuilderOpts {
            scripting_enabled: false,
            ..Default::default()
        },
        ..Default::default()
    };
    parse_document(RcDom::default(), opts).one(html).document
}

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

pub fn parent(node: &Handle) -> Option<Handle> {
    let weak = node.parent.take();
    let up = weak.as_ref().and_then(|w| w.upgrade());
    node.parent.set(weak);
    up
}

pub fn children(node: &Handle) -> Vec<Handle> {
    node.children.borrow().clone()
}

pub fn template_contents(node: &Handle) -> Option<Handle> {
    match &node.data {
        NodeData::Element {
            template_contents, ..
        } => template_contents.borrow().clone(),
        _ => None,
    }
}

pub fn element_name(node: &Handle) -> Option<&QualName> {
    match &node.data {
        NodeData::Element { name, .. } => Some(name),
        _ => None,
    }
}

pub fn is_element(node: &Handle) -> bool {
    matches!(node.data, NodeData::Element { .. })
}

pub fn namespace(node: &Handle) -> Option<&str> {
    element_name(node).map(|n| &*n.ns)
}

/// `localName`: what a spec parser stored (case-adjusted for SVG).
pub fn local_name(node: &Handle) -> Option<&str> {
    element_name(node).map(|n| &*n.local)
}

/// Whether `node` is the HTML element `tag`.
pub fn is_html(node: &Handle, tag: &str) -> bool {
    element_name(node).is_some_and(|n| &*n.ns == HTML_NS && &*n.local == tag)
}

/// `nodeName`: uppercased for HTML elements, as is for foreign ones, `#text` and friends.
pub fn node_name(node: &Handle) -> String {
    match &node.data {
        NodeData::Element { name, .. } => {
            let q = qualified(name);
            if &*name.ns == HTML_NS {
                q.to_ascii_uppercase()
            } else {
                q
            }
        }
        NodeData::Text { .. } => "#text".into(),
        NodeData::Comment { .. } => "#comment".into(),
        NodeData::ProcessingInstruction { target, .. } => target.to_string(),
        NodeData::Document => "#document".into(),
        NodeData::Doctype { name, .. } => name.to_string(),
    }
}

fn qualified(name: &QualName) -> String {
    match &name.prefix {
        Some(p) if !p.is_empty() => format!("{p}:{}", name.local),
        _ => name.local.to_string(),
    }
}

/// An attribute's `name` (its qualified name).
pub fn attr_name(a: &html5ever::Attribute) -> String {
    qualified(&a.name)
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

/// `removeAttribute(name)` by qualified name.
pub fn remove_attr(node: &Handle, name: &str) {
    if let NodeData::Element { attrs, .. } = &node.data {
        attrs.borrow_mut().retain(|a| attr_name(a) != name);
    }
}

/// Remove the attribute at `index`.
pub fn remove_attr_at(node: &Handle, index: usize) {
    if let NodeData::Element { attrs, .. } = &node.data {
        let mut a = attrs.borrow_mut();
        if index < a.len() {
            a.remove(index);
        }
    }
}

/// `setAttribute(name, value)`: in place when present, appended otherwise.
pub fn set_attr(node: &Handle, name: &str, value: &str) {
    if let NodeData::Element { attrs, .. } = &node.data {
        let mut a = attrs.borrow_mut();
        if let Some(existing) = a.iter_mut().find(|a| attr_name(a) == name) {
            existing.value = value.into();
        } else {
            a.push(html5ever::Attribute {
                name: QualName::new(None, Namespace::from(""), LocalName::from(name)),
                value: value.into(),
            });
        }
    }
}

/// Detach `node` from its parent.
pub fn detach(node: &Handle) {
    let Some(p) = parent(node) else { return };
    p.children.borrow_mut().retain(|c| !Rc::ptr_eq(c, node));
    node.parent.set(None);
}

/// `parent.insertBefore(node, reference)` (`None` appends).
pub fn insert_before(parent_node: &Handle, node: &Handle, reference: Option<&Handle>) {
    detach(node);
    let mut kids = parent_node.children.borrow_mut();
    let at = reference
        .and_then(|r| kids.iter().position(|c| Rc::ptr_eq(c, r)))
        .unwrap_or(kids.len());
    kids.insert(at, node.clone());
    node.parent.set(Some(Rc::downgrade(parent_node)));
}

pub fn next_sibling(node: &Handle) -> Option<Handle> {
    let p = parent(node)?;
    let kids = p.children.borrow();
    let i = kids.iter().position(|c| Rc::ptr_eq(c, node))?;
    kids.get(i + 1).cloned()
}

pub fn previous_sibling(node: &Handle) -> Option<Handle> {
    let p = parent(node)?;
    let kids = p.children.borrow();
    let i = kids.iter().position(|c| Rc::ptr_eq(c, node))?;
    i.checked_sub(1).and_then(|j| kids.get(j).cloned())
}

pub fn first_element_child(node: &Handle) -> Option<Handle> {
    node.children
        .borrow()
        .iter()
        .find(|c| is_element(c))
        .cloned()
}

/// `textContent`: every descendant text node (template contents excluded).
pub fn text_content(node: &Handle) -> String {
    let mut out = String::new();
    fn walk(n: &Handle, out: &mut String) {
        for c in n.children.borrow().iter() {
            match &c.data {
                NodeData::Text { contents } => out.push_str(&contents.borrow()),
                NodeData::Element { .. } => walk(c, out),
                _ => {}
            }
        }
    }
    match &node.data {
        NodeData::Text { contents } => out.push_str(&contents.borrow()),
        NodeData::Comment { contents } => out.push_str(contents),
        NodeData::ProcessingInstruction { contents, .. } => out.push_str(contents),
        _ => walk(node, &mut out),
    }
    out
}

/// A comment's or processing instruction's `data`.
pub fn char_data(node: &Handle) -> Option<String> {
    match &node.data {
        NodeData::Comment { contents } => Some(contents.to_string()),
        NodeData::ProcessingInstruction { contents, .. } => Some(contents.to_string()),
        NodeData::Text { contents } => Some(contents.borrow().to_string()),
        _ => None,
    }
}

/// The last inclusive descendant of `node` in tree order.
pub fn last_inclusive_descendant(node: &Handle) -> Handle {
    let mut n = node.clone();
    loop {
        let last = n.children.borrow().last().cloned();
        match last {
            Some(l) => n = l,
            None => return n,
        }
    }
}

/// The node after `node` in tree order inside `root`.
pub fn following(node: &Handle, root: &Handle) -> Option<Handle> {
    if let Some(c) = node.children.borrow().first() {
        return Some(c.clone());
    }
    let mut n = node.clone();
    loop {
        if Rc::ptr_eq(&n, root) {
            return None;
        }
        if let Some(s) = next_sibling(&n) {
            return Some(s);
        }
        n = parent(&n)?;
    }
}

/// `Node.cloneNode(true)`.
pub fn deep_clone(node: &Handle) -> Handle {
    let data = match &node.data {
        NodeData::Element {
            name,
            attrs,
            template_contents,
            mathml_annotation_xml_integration_point,
        } => NodeData::Element {
            name: name.clone(),
            attrs: attrs.borrow().clone().into(),
            template_contents: template_contents.borrow().as_ref().map(deep_clone).into(),
            mathml_annotation_xml_integration_point: *mathml_annotation_xml_integration_point,
        },
        NodeData::Text { contents } => NodeData::Text {
            contents: contents.borrow().clone().into(),
        },
        NodeData::Comment { contents } => NodeData::Comment {
            contents: contents.clone(),
        },
        NodeData::ProcessingInstruction { target, contents } => NodeData::ProcessingInstruction {
            target: target.clone(),
            contents: contents.clone(),
        },
        NodeData::Doctype {
            name,
            public_id,
            system_id,
        } => NodeData::Doctype {
            name: name.clone(),
            public_id: public_id.clone(),
            system_id: system_id.clone(),
        },
        NodeData::Document => NodeData::Document,
    };
    let copy = Node::new(data);
    for c in node.children.borrow().iter() {
        let cc = deep_clone(c);
        cc.parent.set(Some(Rc::downgrade(&copy)));
        copy.children.borrow_mut().push(cc);
    }
    copy
}

const VOID: &[&str] = &[
    "area", "base", "basefont", "bgsound", "br", "col", "embed", "frame", "hr", "img", "input",
    "keygen", "link", "meta", "param", "source", "track", "wbr",
];

const RAW_TEXT_PARENTS: &[&str] = &[
    "style",
    "script",
    "xmp",
    "iframe",
    "noembed",
    "noframes",
    "plaintext",
];

fn escape(s: &str, attr: bool, out: &mut String) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '\u{a0}' => out.push_str("&nbsp;"),
            '"' if attr => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
}

/// The serialized name of an attribute (HTML fragment serialization).
fn attr_serial_name(a: &html5ever::Attribute) -> String {
    let n = &a.name;
    match &*n.ns {
        "" => n.local.to_string(),
        "http://www.w3.org/XML/1998/namespace" => format!("xml:{}", n.local),
        "http://www.w3.org/2000/xmlns/" if &*n.local == "xmlns" => "xmlns".into(),
        "http://www.w3.org/2000/xmlns/" => format!("xmlns:{}", n.local),
        "http://www.w3.org/1999/xlink" => format!("xlink:{}", n.local),
        _ => qualified(n),
    }
}

fn serialize_node(node: &Handle, parent_raw: bool, out: &mut String) {
    match &node.data {
        NodeData::Element { name, attrs, .. } => {
            let tag = if &*name.ns == HTML_NS || &*name.ns == SVG_NS || &*name.ns == MATHML_NS {
                name.local.to_string()
            } else {
                qualified(name)
            };
            out.push('<');
            out.push_str(&tag);
            for a in attrs.borrow().iter() {
                out.push(' ');
                out.push_str(&attr_serial_name(a));
                out.push_str("=\"");
                escape(&a.value, true, out);
                out.push('"');
            }
            out.push('>');
            if &*name.ns == HTML_NS && VOID.contains(&&*name.local) {
                return;
            }
            serialize_children(node, out);
            out.push_str("</");
            out.push_str(&tag);
            out.push('>');
        }
        NodeData::Text { contents } => {
            if parent_raw {
                out.push_str(&contents.borrow());
            } else {
                escape(&contents.borrow(), false, out);
            }
        }
        NodeData::Comment { contents } => {
            out.push_str("<!--");
            out.push_str(contents);
            out.push_str("-->");
        }
        NodeData::ProcessingInstruction { target, contents } => {
            out.push_str("<?");
            out.push_str(target);
            out.push(' ');
            out.push_str(contents);
            out.push('>');
        }
        NodeData::Doctype { name, .. } => {
            out.push_str("<!DOCTYPE ");
            out.push_str(name);
            out.push('>');
        }
        NodeData::Document => serialize_children(node, out),
    }
}

fn serialize_children(node: &Handle, out: &mut String) {
    let raw = element_name(node)
        .is_some_and(|n| &*n.ns == HTML_NS && RAW_TEXT_PARENTS.contains(&&*n.local));
    let source = template_contents(node).unwrap_or_else(|| node.clone());
    for c in source.children.borrow().iter() {
        serialize_node(c, raw, out);
    }
}

/// `innerHTML`.
pub fn inner_html(node: &Handle) -> String {
    let mut out = String::new();
    serialize_children(node, &mut out);
    out
}

/// `outerHTML`.
pub fn outer_html(node: &Handle) -> String {
    let mut out = String::new();
    serialize_node(node, false, &mut out);
    out
}

/// The first `tag` element in tree order.
pub fn find_html_element(root: &Handle, tag: &str) -> Option<Handle> {
    if is_html(root, tag) {
        return Some(root.clone());
    }
    for c in root.children.borrow().iter() {
        if let Some(f) = find_html_element(c, tag) {
            return Some(f);
        }
    }
    None
}

pub fn new_text(text: &str) -> Handle {
    Node::new(NodeData::Text {
        contents: std::cell::RefCell::new(text.into()),
    })
}
