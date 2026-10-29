//! A port of DOMPurify 3.4.16's `sanitize` for string input, with the options Recur passes
//! (`ADD_ATTR`, `ADD_TAGS`, `USE_PROFILES`, `HTML_INTEGRATION_POINTS`) and the hook
//! `lib/markdown.ts` installs on the shared instance. It walks the same DOM a browser's
//! `DOMParser` builds (html5ever is a spec parser) in `NodeIterator` order and serializes it
//! the way WebKit's `innerHTML` does.

pub mod dom;
mod lists;

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::OnceLock;

use markup5ever_rcdom::{Handle, NodeData};

use crate::js::{self, Re};
use dom::{HTML_NS, MATHML_NS, SVG_NS};
use lists::*;

/// The `afterSanitizeAttributes` hook `lib/markdown.ts` registers on the app's one
/// DOMPurify instance, which every sanitize call (markdown, LSP docs, Mermaid) passes through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hook {
    None,
    /// Links open externally; outside SVG, `style` and `id` go and only Recur's own classes
    /// stay.
    Recur,
}

/// `ALLOWED_CLASSES` in `lib/markdown.ts`.
pub const ALLOWED_CLASSES: &[&str] = &[
    "mdref",
    "mdref-file",
    "mdref-code",
    "md-mermaid",
    "md-mermaid-src",
    "sh-kw",
    "sh-str",
    "sh-com",
    "sh-num",
    "sh-flag",
    "sh-var",
    "sh-op",
    "sh-prompt",
];

/// The parsed configuration (`_parseConfig`).
#[derive(Clone, Debug)]
pub struct Config {
    allowed_tags: HashSet<String>,
    allowed_attr: HashSet<String>,
    html_integration_points: HashSet<String>,
    hook: Hook,
}

fn set(lists: &[&[&str]]) -> HashSet<String> {
    lists
        .iter()
        .flat_map(|l| l.iter())
        .map(|s| s.to_lowercase())
        .collect()
}

impl Config {
    /// `DOMPurify.sanitize(html)` (the default allowlists).
    pub fn default_with(hook: Hook) -> Config {
        let mut tags = set(&[HTML_TAGS, SVG_TAGS, SVG_FILTERS, MATHML_TAGS]);
        tags.insert("#text".into());
        Config {
            allowed_tags: tags,
            allowed_attr: set(&[HTML_ATTRS, SVG_ATTRS, MATHML_ATTRS, XML_ATTRS]),
            html_integration_points: set(&[&["annotation-xml"]]),
            hook,
        }
    }

    /// `sanitizeMarkdown`: `{ ADD_ATTR: ['target'] }`.
    pub fn markdown() -> Config {
        let mut c = Config::default_with(Hook::Recur);
        c.allowed_attr.insert("target".into());
        c
    }

    /// `sanitizeMermaidSvg`: `USE_PROFILES: { svg, svgFilters }`, `ADD_TAGS: ['foreignobject']`,
    /// `ADD_ATTR: ['dominant-baseline']`, `HTML_INTEGRATION_POINTS: { foreignobject: true }`.
    pub fn mermaid_svg() -> Config {
        let mut tags = set(&[SVG_TAGS, SVG_FILTERS]);
        tags.insert("#text".into());
        tags.insert("foreignobject".into());
        let mut attrs = set(&[SVG_ATTRS, XML_ATTRS]);
        attrs.insert("dominant-baseline".into());
        Config {
            allowed_tags: tags,
            allowed_attr: attrs,
            html_integration_points: set(&[&["foreignobject"]]),
            hook: Hook::Recur,
        }
    }
}

fn props() -> &'static (HashSet<&'static str>, HashSet<&'static str>) {
    static P: OnceLock<(HashSet<&'static str>, HashSet<&'static str>)> = OnceLock::new();
    P.get_or_init(|| {
        (
            DOCUMENT_PROPS.iter().copied().collect(),
            FORM_PROPS.iter().copied().collect(),
        )
    })
}

struct Regexes {
    data_attr: Re,
    aria_attr: Re,
    is_allowed_uri: Re,
    attr_whitespace: Re,
    custom_element: Re,
    element_markup_probe: Re,
    comment_markup_probe: Re,
    fallback_tag_close: Re,
    unsafe_attr_value: Re,
    whitespace: Re,
}

fn regexes() -> &'static Regexes {
    static R: OnceLock<Regexes> = OnceLock::new();
    R.get_or_init(|| Regexes {
        data_attr: Re::new(r"^data-[\-\w.·-￿]+$", ""),
        aria_attr: Re::new(r"^aria-[\-\w]+$", ""),
        is_allowed_uri: Re::new(
            r"^(?:(?:(?:f|ht)tps?|mailto|tel|callto|sms|cid|xmpp|matrix):|[^a-z]|[a-z+.\-]+(?:[^a-z+.\-:]|$))",
            "i",
        ),
        attr_whitespace: Re::new(
            r"[\u0000-   ᠎ -  　]",
            "g",
        ),
        custom_element: Re::new(r"^[a-z][.\w]*(-[.\w]+)+$", "i"),
        element_markup_probe: Re::new(r"<[/\w!]", "g"),
        comment_markup_probe: Re::new(r"<[/\w]", "g"),
        fallback_tag_close: Re::new(r"<\/no(script|embed|frames)", "i"),
        unsafe_attr_value: Re::new(
            r"((--!?|])>)|<\/(style|script|title|xmp|textarea|noscript|iframe|noembed|noframes)",
            "i",
        ),
        whitespace: Re::new(r"\s+", ""),
    })
}

fn literal_text_close(tag: &str) -> Option<Re> {
    const NAMES: &[&str] = &[
        "style",
        "script",
        "xmp",
        "iframe",
        "noembed",
        "noframes",
        "plaintext",
        "noscript",
    ];
    NAMES
        .contains(&tag)
        .then(|| Re::new(&format!("</{tag}(?=[\\t\\n\\f\\r />])"), "i"))
}

const FORBID_CONTENTS: &[&str] = &[
    "annotation-xml",
    "audio",
    "colgroup",
    "desc",
    "foreignobject",
    "head",
    "iframe",
    "math",
    "mi",
    "mn",
    "mo",
    "ms",
    "mtext",
    "noembed",
    "noframes",
    "noscript",
    "plaintext",
    "script",
    "selectedcontent",
    "style",
    "svg",
    "template",
    "thead",
    "title",
    "video",
    "xmp",
];

const DATA_URI_TAGS: &[&str] = &["audio", "video", "img", "source", "image", "track"];

const URI_SAFE_ATTRIBUTES: &[&str] = &[
    "alt",
    "class",
    "for",
    "id",
    "label",
    "name",
    "pattern",
    "placeholder",
    "role",
    "summary",
    "title",
    "value",
    "style",
    "xmlns",
];

const MATHML_TEXT_INTEGRATION_POINTS: &[&str] = &["mi", "mo", "mn", "ms", "mtext"];
const COMMON_SVG_AND_HTML_ELEMENTS: &[&str] = &["title", "style", "font", "a", "script"];

const RESERVED_CUSTOM_ELEMENT_NAMES: &[&str] = &[
    "annotation-xml",
    "color-profile",
    "font-face",
    "font-face-format",
    "font-face-name",
    "font-face-src",
    "font-face-uri",
    "missing-glyph",
];

/// Properties `_isClobbered` reads off a form; a named control shadowing one condemns it.
const CLOBBER_PROPS: &[&str] = &[
    "nodeName",
    "textContent",
    "removeChild",
    "attributes",
    "removeAttribute",
    "removeAttributeNode",
    "getAttributeNode",
    "setAttribute",
    "namespaceURI",
    "insertBefore",
    "hasChildNodes",
    "nodeType",
    "childNodes",
];

fn all_svg_tags() -> &'static HashSet<String> {
    static S: OnceLock<HashSet<String>> = OnceLock::new();
    S.get_or_init(|| set(&[SVG_TAGS, SVG_FILTERS, SVG_DISALLOWED]))
}

fn all_mathml_tags() -> &'static HashSet<String> {
    static S: OnceLock<HashSet<String>> = OnceLock::new();
    S.get_or_init(|| set(&[MATHML_TAGS, MATHML_DISALLOWED]))
}

/// `NodeIterator` over `SHOW_ELEMENT | SHOW_COMMENT | SHOW_TEXT | SHOW_PROCESSING_INSTRUCTION
/// | SHOW_CDATA_SECTION`, with the DOM's pre-removing steps for its reference node.
struct Walk {
    root: Handle,
    reference: Handle,
    before: bool,
}

impl Walk {
    fn new(root: &Handle) -> Walk {
        Walk {
            root: root.clone(),
            reference: root.clone(),
            before: true,
        }
    }

    fn next(&mut self) -> Option<Handle> {
        let mut node = self.reference.clone();
        let mut before = self.before;
        loop {
            if !before {
                node = dom::following(&node, &self.root)?;
            }
            before = false;
            if !matches!(node.data, NodeData::Document | NodeData::Doctype { .. }) {
                break;
            }
        }
        self.reference = node.clone();
        self.before = false;
        Some(node)
    }

    /// Pre-removing steps for removing `node` (only ever the reference here).
    fn removing(&mut self, node: &Handle) {
        if Rc::ptr_eq(node, &self.root) || !Rc::ptr_eq(node, &self.reference) {
            return;
        }
        self.reference = match dom::previous_sibling(node) {
            Some(prev) => dom::last_inclusive_descendant(&prev),
            None => dom::parent(node).unwrap_or_else(|| self.root.clone()),
        };
    }
}

struct Sanitizer<'c> {
    cfg: &'c Config,
}

impl Sanitizer<'_> {
    fn force_remove(&self, node: &Handle, walk: &mut Walk) {
        walk.removing(node);
        dom::detach(node);
    }

    /// `_isClobbered`: a form whose named controls (or, failing those, images) shadow a DOM
    /// property DOMPurify reads.
    fn is_clobbered(&self, node: &Handle) -> bool {
        if !dom::is_html(node, "form") {
            return false;
        }
        const LISTED: &[&str] = &[
            "button", "fieldset", "input", "object", "output", "select", "textarea",
        ];
        let mut listed = Vec::new();
        let mut imgs = Vec::new();
        fn walk(n: &Handle, listed: &mut Vec<Handle>, imgs: &mut Vec<Handle>) {
            for c in n.children.borrow().iter() {
                if let Some(name) = dom::element_name(c)
                    && &*name.ns == HTML_NS
                {
                    let local = &*name.local;
                    let image_button = local == "input"
                        && dom::get_attr(c, "type")
                            .is_some_and(|t| t.eq_ignore_ascii_case("image"));
                    if LISTED.contains(&local)
                        && !image_button
                        && dom::get_attr(c, "form").is_none()
                    {
                        listed.push(c.clone());
                    } else if local == "img" {
                        imgs.push(c.clone());
                    }
                }
                walk(c, listed, imgs);
            }
        }
        walk(node, &mut listed, &mut imgs);
        let named = |els: &[Handle], prop: &str| {
            els.iter().any(|e| {
                dom::get_attr(e, "id").as_deref() == Some(prop)
                    || dom::get_attr(e, "name").as_deref() == Some(prop)
            })
        };
        CLOBBER_PROPS
            .iter()
            .any(|p| named(&listed, p) || named(&imgs, p))
    }

    /// `_checkValidNamespace`.
    fn check_valid_namespace(&self, el: &Handle) -> bool {
        let (parent_ns, parent_tag) = match dom::parent(el).filter(dom::is_element) {
            Some(p) => (
                dom::namespace(&p).unwrap_or("").to_string(),
                dom::local_name(&p).unwrap_or("").to_lowercase(),
            ),
            None => (HTML_NS.to_string(), "template".to_string()),
        };
        let tag = dom::local_name(el).unwrap_or("").to_lowercase();
        let ns = dom::namespace(el).unwrap_or("");
        let hip = |t: &str| self.cfg.html_integration_points.contains(t);
        let mtip = |t: &str| MATHML_TEXT_INTEGRATION_POINTS.contains(&t);
        match ns {
            SVG_NS => {
                if parent_ns == HTML_NS {
                    return tag == "svg";
                }
                if parent_ns == MATHML_NS {
                    return tag == "svg" && (parent_tag == "annotation-xml" || mtip(&parent_tag));
                }
                all_svg_tags().contains(&tag)
            }
            MATHML_NS => {
                if parent_ns == HTML_NS {
                    return tag == "math";
                }
                if parent_ns == SVG_NS {
                    return tag == "math" && hip(&parent_tag);
                }
                all_mathml_tags().contains(&tag)
            }
            HTML_NS => {
                if parent_ns == SVG_NS && !hip(&parent_tag) {
                    return false;
                }
                if parent_ns == MATHML_NS && !mtip(&parent_tag) {
                    return false;
                }
                !all_mathml_tags().contains(&tag)
                    && (COMMON_SVG_AND_HTML_ELEMENTS.contains(&tag.as_str())
                        || !all_svg_tags().contains(&tag))
            }
            _ => false,
        }
    }

    /// `_isUnsafeNode`.
    fn is_unsafe_node(&self, node: &Handle, tag: &str) -> bool {
        let r = regexes();
        if dom::is_element(node)
            && !node.children.borrow().is_empty()
            && dom::first_element_child(node).is_none()
            && r.element_markup_probe.test(&dom::text_content(node))
            && r.element_markup_probe.test(&dom::inner_html(node))
        {
            return true;
        }
        if dom::namespace(node) == Some(HTML_NS)
            && let Some(close) = literal_text_close(tag)
            && (dom::first_element_child(node).is_some() || close.test(&dom::text_content(node)))
        {
            return true;
        }
        if matches!(node.data, NodeData::ProcessingInstruction { .. }) {
            return true;
        }
        if let NodeData::Comment { contents } = &node.data
            && r.comment_markup_probe.test(contents)
        {
            return true;
        }
        false
    }

    fn is_basic_custom_element(tag: &str) -> bool {
        !RESERVED_CUSTOM_ELEMENT_NAMES.contains(&tag.to_lowercase().as_str())
            && regexes().custom_element.test(tag)
    }

    /// `_sanitizeElements`: true when the node was removed.
    fn sanitize_element(&self, node: &Handle, root: &Handle, walk: &mut Walk) -> bool {
        if self.is_clobbered(node) {
            self.force_remove(node, walk);
            return true;
        }
        let tag = dom::node_name(node).to_lowercase();
        if self.is_unsafe_node(node, &tag) {
            self.force_remove(node, walk);
            return true;
        }
        if !self.cfg.allowed_tags.contains(&tag) {
            // `_sanitizeDisallowedNode` (no custom element handling configured).
            let _ = Self::is_basic_custom_element(&tag);
            if !FORBID_CONTENTS.contains(&tag.as_str())
                && !Rc::ptr_eq(node, root)
                && let Some(parent) = dom::parent(node)
            {
                for child in dom::children(node).into_iter().rev() {
                    dom::insert_before(&parent, &child, dom::next_sibling(node).as_ref());
                }
            }
            self.force_remove(node, walk);
            return true;
        }
        if dom::is_element(node) && !self.check_valid_namespace(node) {
            self.force_remove(node, walk);
            return true;
        }
        if matches!(tag.as_str(), "noscript" | "noembed" | "noframes")
            && regexes().fallback_tag_close.test(&dom::inner_html(node))
        {
            self.force_remove(node, walk);
            return true;
        }
        false
    }

    /// `_isValidAttribute`.
    fn is_valid_attribute(&self, lc_tag: &str, lc_name: &str, value: &str) -> bool {
        if lc_name == "patchsrc" || (lc_name == "for" && lc_tag != "label" && lc_tag != "output") {
            return false;
        }
        let (doc_props, form_props) = props();
        if (lc_name == "id" || lc_name == "name")
            && (doc_props.contains(value) || form_props.contains(value))
        {
            return false;
        }
        let r = regexes();
        let permitted = self.cfg.allowed_attr.contains(lc_name);
        if r.data_attr.test(lc_name) || r.aria_attr.test(lc_name) {
            return true;
        }
        if !permitted {
            return false;
        }
        if URI_SAFE_ATTRIBUTES.contains(&lc_name) {
            return true;
        }
        if r.is_allowed_uri.test(&r.attr_whitespace.replace(value, "")) {
            return true;
        }
        if matches!(lc_name, "src" | "xlink:href" | "href")
            && lc_tag != "script"
            && value.starts_with("data:")
            && DATA_URI_TAGS.contains(&lc_tag)
        {
            return true;
        }
        value.is_empty()
    }

    /// `_sanitizeAttributes`, then the `afterSanitizeAttributes` hook.
    fn sanitize_attributes(&self, node: &Handle) {
        if !dom::is_element(node) || self.is_clobbered(node) {
            return;
        }
        let lc_tag = dom::node_name(node).to_lowercase();
        let mut l = dom::attrs(node).len();
        while l > 0 {
            l -= 1;
            let all = dom::attrs(node);
            let Some((name, init)) = all.get(l).cloned() else {
                continue;
            };
            let lc_name = name.to_lowercase();
            let value = if name == "value" {
                init.clone()
            } else {
                js::trim(&init).to_string()
            };
            if regexes().unsafe_attr_value.test(&value)
                || (lc_name == "attributename" && value.contains("href"))
                || !self.is_valid_attribute(&lc_tag, &lc_name, &value)
            {
                self.remove_attribute(node, l, &name);
                continue;
            }
            if value != init {
                dom::set_attr(node, &name, &value);
            }
        }
        if self.cfg.hook == Hook::Recur {
            recur_hook(node);
        }
    }

    /// `_removeAttribute`: an `is` attribute stays, emptied.
    fn remove_attribute(&self, node: &Handle, index: usize, name: &str) {
        dom::remove_attr_at(node, index);
        if name == "is" {
            dom::set_attr(node, "is", "");
        }
    }

    fn walk(&self, root: &Handle) {
        let mut walk = Walk::new(root);
        while let Some(node) = walk.next() {
            self.sanitize_element(&node, root, &mut walk);
            if dom::parent(&node).is_some() || Rc::ptr_eq(&node, root) {
                self.sanitize_attributes(&node);
            }
            if let Some(content) = dom::template_contents(&node) {
                self.walk(&content);
            }
        }
    }
}

/// The `afterSanitizeAttributes` hook from `lib/markdown.ts`.
fn recur_hook(el: &Handle) {
    let ns = dom::namespace(el).unwrap_or("");
    if ns == HTML_NS && dom::local_name(el) == Some("a") {
        dom::set_attr(el, "target", "_blank");
        dom::set_attr(el, "rel", "noopener noreferrer");
    }
    if ns == SVG_NS {
        return;
    }
    dom::remove_attr(el, "style");
    dom::remove_attr(el, "id");
    if let Some(class) = dom::get_attr(el, "class") {
        let kept: Vec<String> = regexes()
            .whitespace
            .split(&class)
            .into_iter()
            .flatten()
            .filter(|c| ALLOWED_CLASSES.contains(c))
            .map(str::to_string)
            .collect();
        if kept.is_empty() {
            dom::remove_attr(el, "class");
        } else {
            dom::set_attr(el, "class", &kept.join(" "));
        }
    }
}

/// `DOMPurify.sanitize(dirty, cfg)` for a string, returning `body.innerHTML`.
pub fn sanitize(dirty: &str, cfg: &Config) -> String {
    match sanitize_to_body(dirty, cfg) {
        Ok(doc) => dom::inner_html(&doc.body),
        Err(unchanged) => unchanged,
    }
}

/// A sanitized document. The document owns the tree (rcdom tears a subtree down when its
/// document drops), so it travels with the body.
pub struct Sanitized {
    _document: Handle,
    pub body: Handle,
}

/// The sanitized `<body>`; `Err` carries input DOMPurify returns as is (no `<`).
pub fn sanitize_to_body(dirty: &str, cfg: &Config) -> Result<Sanitized, String> {
    let empty = dirty.is_empty();
    let dirty = if empty { "<!-->" } else { dirty };
    if !dirty.contains('<') {
        return Err(dirty.to_string());
    }
    let leading = dirty.len() - dirty.trim_start_matches(['\r', '\n', '\t', ' ']).len();
    let doc = dom::parse_html_document(dirty);
    let Some(body) = dom::find_html_element(&doc, "body") else {
        return Err(String::new());
    };
    if leading > 0 && !empty {
        let first = body.children.borrow().first().cloned();
        dom::insert_before(&body, &dom::new_text(&dirty[..leading]), first.as_ref());
    }
    Sanitizer { cfg }.walk(&body);
    Ok(Sanitized {
        _document: doc,
        body,
    })
}

#[cfg(test)]
mod tests;
