//! comrak (GFM) with Recur's rendering and fence highlighting. The output
//! is unsanitized; [`super::sanitize_markdown`] runs before anything renders it.

use std::sync::OnceLock;

use comrak::nodes::{NodeHtmlBlock, NodeLink, NodeValue};
use comrak::{Arena, Node, Options};

use recur_textmate::{highlight_code, lang_from_info};

use crate::emoji::expand_emoji_shortcodes;
use crate::js::{self, Re, encode_uri_component};

/// Asset-server path that streams GitHub attachment bytes with the user's gh token.
pub const GH_IMAGE_PATH: &str = "/gh-image";

/// `ParseMarkdownOpts`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ParseOptions {
    /// WebFetch cards: turn images into links/alt instead of live `<img>`.
    pub demote_images: bool,
}

fn re(cell: &'static OnceLock<Re>, src: &str, flags: &str) -> &'static Re {
    cell.get_or_init(|| Re::new(src, flags))
}

/// `escapeText`.
pub fn escape_text(src: &str) -> String {
    src.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn strip_tags(s: &str) -> String {
    static TAGS: OnceLock<Re> = OnceLock::new();
    re(&TAGS, r"<[^>]+>", "g").replace(s, "")
}

/// `htmlAttr(tag, name)`: a quoted attribute's value with `&amp;` decoded.
fn html_attr(tag: &str, name: &str) -> String {
    let r = Re::new(&format!(r#"\b{name}\s*=\s*(?:"([^"]*)"|'([^']*)')"#), "i");
    r.exec(tag)
        .map(|m| m.get(1).or(m.get(2)).unwrap_or("").replace("&amp;", "&"))
        .unwrap_or_default()
}

/// `isGitHubImageURL`: GitHub-hosted upload / CDN URLs that need the user's token.
pub fn is_github_image_url(href: &str) -> bool {
    let Ok(u) = url::Url::parse(js::trim(href)) else {
        return false;
    };
    if u.scheme() != "https" {
        return false;
    }
    let host = u.host_str().unwrap_or("").to_lowercase();
    if host == "githubusercontent.com" || host.ends_with(".githubusercontent.com") {
        return true;
    }
    if host != "github.com" && host != "www.github.com" {
        return false;
    }
    static REPO_ASSETS: OnceLock<Re> = OnceLock::new();
    let path = u.path();
    path.starts_with("/user-attachments/")
        || re(&REPO_ASSETS, r"^/[^/]+/[^/]+/assets/", "").test(path)
}

/// `githubImageSrc`: GitHub image URLs move onto `/gh-image` so the asset server can
/// authenticate them; others pass through.
pub fn github_image_src(href: &str) -> String {
    let src = js::trim(href);
    if !is_github_image_url(src) {
        return src.to_string();
    }
    format!("{GH_IMAGE_PATH}?url={}", encode_uri_component(src))
}

fn code_ref_info() -> &'static Re {
    static CODE_REF: OnceLock<Re> = OnceLock::new();
    re(&CODE_REF, r"^(\d+):(\d+):(.+)$", "")
}

fn is_http(href: &str) -> bool {
    static HTTP: OnceLock<Re> = OnceLock::new();
    re(&HTTP, r"^https?:\/\/", "i").test(href)
}

/// `renderCode`: mermaid placeholders, Cursor `start:end:path` cards, highlighted fences.
fn render_code(text: &str, info: &str) -> String {
    let info = js::trim(info);
    let name = info
        .split(|c: char| js::is_js_space(c))
        .next()
        .unwrap_or("")
        .to_lowercase();
    if name == "mermaid" {
        return format!(
            "<div class=\"md-mermaid\"><pre class=\"md-mermaid-src\"><code>{}</code></pre></div>",
            escape_text(text)
        );
    }
    if let Some(m) = code_ref_info().exec(info) {
        return format!(
            "<div class=\"mdref\"><div class=\"mdref-file\">{}</div><pre class=\"mdref-code\"><code>{}</code></pre></div>",
            escape_text(m.str(3)),
            escape_text(text)
        );
    }
    format!(
        "<pre><code>{}</code></pre>\n",
        highlight_code(text, lang_from_info(info))
    )
}

/// Raw HTML from the source: its text, escaped. Live documents keep GitHub-pasted `<img>`s
/// (proxied), never other tags.
fn render_html(text: &str, demote: bool) -> String {
    if demote {
        return escape_text(&strip_tags(text));
    }
    static IMG: OnceLock<Re> = OnceLock::new();
    static SLOT: OnceLock<Re> = OnceLock::new();
    let mut imgs: Vec<String> = Vec::new();
    let with_slots = re(&IMG, r"<img\b[^>]*>", "gi").replace_with(text, |c| {
        let tag = c.full();
        let src = html_attr(tag, "src");
        if !is_github_image_url(&src) {
            return String::new();
        }
        let alt = html_attr(tag, "alt");
        imgs.push(format!(
            "<p><img src=\"{}\" alt=\"{}\"></p>",
            escape_text(&github_image_src(&src)),
            escape_text(&alt)
        ));
        format!("\u{0}IMG{}\u{0}", imgs.len() - 1)
    });
    re(&SLOT, "\u{0}IMG(\\d+)\u{0}", "g").replace_with(
        &escape_text(&strip_tags(&with_slots)),
        |c| {
            c.str(1)
                .parse::<usize>()
                .ok()
                .and_then(|i| imgs.get(i).cloned())
                .unwrap_or_default()
        },
    )
}

/// The UTF-16 length of the text a link shows (images count as nothing, as their tags strip).
fn label_len(node: Node<'_>) -> usize {
    node.children()
        .map(|c| match &c.data().value {
            NodeValue::Text(t) => t.encode_utf16().count(),
            NodeValue::Code(c) => c.literal.encode_utf16().count(),
            NodeValue::Raw(h) | NodeValue::HtmlInline(h) => strip_tags(h).encode_utf16().count(),
            NodeValue::Image(_) => 0,
            _ => label_len(c),
        })
        .sum()
}

fn replace<'a>(arena: &'a Arena<'a>, node: Node<'a>, value: NodeValue) {
    node.insert_before(arena.alloc(value.into()));
    node.detach();
}

/// `renderImage` with `demote`: never an `<img>`, fetched pages must not load third-party URLs.
fn demoted_image(nl: &NodeLink, alt: &str, in_link: bool) -> NodeValue {
    let label = match js::trim(alt) {
        "" => "image",
        l => l,
    };
    let src = js::trim(&nl.url);
    if in_link || src.is_empty() || !is_http(src) || label.encode_utf16().count() > 120 {
        return NodeValue::Text(label.to_string().into());
    }
    let title = match nl.title.as_str() {
        "" => String::new(),
        t => format!(" title=\"{}\"", escape_text(t)),
    };
    NodeValue::Raw(format!(
        "<a href=\"{}\"{title}>{}</a>",
        escape_text(src),
        escape_text(label)
    ))
}

/// Recur's rendering on comrak's tree: emoji shortcodes outside code, fences, raw HTML and
/// images first, then links, which stay clickable only as short http(s) links.
fn transform<'a>(arena: &'a Arena<'a>, root: Node<'a>, demote: bool) {
    let mut links = Vec::new();
    for node in root.descendants().collect::<Vec<_>>() {
        let value = node.data().value.clone();
        match value {
            NodeValue::CodeBlock(cb) => {
                let text = cb.literal.strip_suffix('\n').unwrap_or(&cb.literal);
                replace(arena, node, NodeValue::Raw(render_code(text, &cb.info)));
            }
            NodeValue::Text(text) if text.contains(':') => {
                node.data_mut().value =
                    NodeValue::Text(expand_emoji_shortcodes(&text, false).into());
            }
            NodeValue::HtmlBlock(NodeHtmlBlock { literal, .. })
            | NodeValue::HtmlInline(literal) => {
                let html = render_html(&expand_emoji_shortcodes(&literal, false), demote);
                replace(arena, node, NodeValue::Raw(html));
            }
            NodeValue::Image(nl) if demote => {
                let in_link = node
                    .ancestors()
                    .any(|a| matches!(a.data().value, NodeValue::Link(_)));
                replace(
                    arena,
                    node,
                    demoted_image(&nl, &node.collect_text(), in_link),
                );
            }
            NodeValue::Image(nl) => match github_image_src(&nl.url) {
                src if src.is_empty() => {
                    let alt = node.collect_text();
                    replace(arena, node, NodeValue::Text(alt.into()));
                }
                src => {
                    node.data_mut().value = NodeValue::Image(Box::new(NodeLink { url: src, ..*nl }))
                }
            },
            // The checkbox moves into the item's paragraph, so a loose item keeps it on the
            // text's line.
            NodeValue::TaskItem(task) => {
                if let Some(NodeValue::List(list)) = node.parent().map(|p| p.data().value.clone())
                    && let Some(para) = node
                        .first_child()
                        .filter(|c| matches!(c.data().value, NodeValue::Paragraph))
                {
                    let checked = if task.symbol.is_some() {
                        " checked=\"\""
                    } else {
                        ""
                    };
                    para.prepend(
                        arena.alloc(
                            NodeValue::Raw(format!(
                                "<input type=\"checkbox\"{checked} disabled=\"\"> "
                            ))
                            .into(),
                        ),
                    );
                    node.data_mut().value = NodeValue::Item(list);
                }
            }
            NodeValue::Link(nl) => links.push((node, nl)),
            _ => {}
        }
    }
    for (node, nl) in links {
        if !nl.url.is_empty() && is_http(&nl.url) && label_len(node) <= 120 {
            continue;
        }
        for child in node.children().collect::<Vec<_>>() {
            node.insert_before(child);
        }
        node.detach();
    }
}

fn options() -> Options<'static> {
    let mut o = Options::default();
    o.extension.strikethrough = true;
    o.extension.table = true;
    o.extension.autolink = true;
    o.extension.tasklist = true;
    o.render.hardbreaks = true;
    o
}

/// `normalizeCodeRefs`: a mid-sentence ```` ```start:end:path ```` fence moves onto its own
/// lines so its body always parses as an escaped code block.
fn normalize_code_refs(src: &str) -> String {
    static FENCE: OnceLock<Re> = OnceLock::new();
    static TRAIL: OnceLock<Re> = OnceLock::new();
    re(&FENCE, r"```(\d+:\d+:[^\n`]+)\n([\s\S]*?)```[ \t]*", "g").replace_with(src, |c| {
        let body = re(&TRAIL, r"\n?$", "").replace(c.str(2), "\n");
        format!("\n\n```{}\n{body}```\n\n", c.str(1))
    })
}

/// `parseMarkdown`: unsanitized HTML from comrak and `highlightCode`.
pub fn parse_markdown(src: &str, opts: ParseOptions) -> String {
    if src.is_empty() {
        return String::new();
    }
    let prepared = normalize_code_refs(src);
    let options = options();
    let arena = Arena::new();
    let root = comrak::parse_document(&arena, &prepared, &options);
    transform(&arena, root, opts.demote_images);
    let mut out = String::new();
    let _ = comrak::format_html(root, &options, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn does_not_sanitize() {
        let raw = parse_markdown("```js\nconst x = 1\n```", ParseOptions::default());
        assert!(raw.contains("sh-kw"));
    }

    #[test]
    fn code_refs_become_file_cards_even_mid_sentence() {
        let raw = parse_markdown(
            "see ```12:14:src/a.ts\n<div class=\"x\">\n``` now",
            ParseOptions::default(),
        );
        assert!(
            raw.contains("<div class=\"mdref-file\">src/a.ts</div>"),
            "{raw}"
        );
        assert!(raw.contains("&lt;div class=&quot;x&quot;&gt;"), "{raw}");
    }

    #[test]
    fn github_image_urls() {
        assert!(is_github_image_url(
            "https://github.com/user-attachments/assets/a"
        ));
        assert!(is_github_image_url(
            "https://github.com/acme/app/assets/1/b"
        ));
        assert!(is_github_image_url(
            "https://private-user-images.githubusercontent.com/x.png"
        ));
        assert!(!is_github_image_url("https://github.com/login"));
        assert!(!is_github_image_url(
            "http://github.com/user-attachments/assets/x"
        ));
        assert_eq!(
            github_image_src("https://github.com/user-attachments/assets/a"),
            "/gh-image?url=https%3A%2F%2Fgithub.com%2Fuser-attachments%2Fassets%2Fa"
        );
        assert_eq!(
            github_image_src(" https://x.test/a.png "),
            "https://x.test/a.png"
        );
    }

    #[test]
    fn mermaid_fences_become_placeholders() {
        let raw = parse_markdown("```mermaid\ngraph TD\nA-->B\n```", ParseOptions::default());
        assert_eq!(
            raw,
            "<div class=\"md-mermaid\"><pre class=\"md-mermaid-src\"><code>graph TD\nA--&gt;B</code></pre></div>"
        );
    }
}
