//! GitHub-flavored markdown to sanitized HTML for chat and file previews.
//! Chat content is LLM output, so raw HTML never reaches a view unsanitized.

mod host;
mod parse;

use std::sync::OnceLock;

pub use host::{
    CACHE_ENTRY_MAX_BYTES, CACHE_MAX_BYTES, DOC_CACHE_MAX_BYTES, RenderError, clear_markdown_cache,
    markdown_cache_stats_for_tests, markdown_doc_cache_stats, markdown_doc_cached,
    markdown_doc_is_cached, markdown_render_inline, peek_markdown_cache, remember_markdown,
    render_markdown_async, render_markdown_cached, set_markdown_cache_max_bytes_for_tests,
    set_markdown_render_inline,
};
pub use parse::{
    GH_IMAGE_PATH, ParseOptions, escape_text, github_image_src, is_github_image_url, parse_markdown,
};

/// The class names sanitized HTML keeps; every other class goes.
const ALLOWED_CLASSES: &[&str] = &[
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

/// The sanitizer for markdown and other untrusted HTML: ammonia's GitHub-like allowlist plus
/// task-list checkboxes, Recur's classes, `data:` images, and links that open externally.
/// No `id` or `style` survives.
fn sanitizer() -> &'static ammonia::Builder<'static> {
    static B: OnceLock<ammonia::Builder<'static>> = OnceLock::new();
    B.get_or_init(|| {
        let mut b = ammonia::Builder::default();
        b.add_tags(["input", "section", "tfoot"])
            .add_tag_attributes("input", ["type", "checked", "disabled"])
            .add_generic_attributes(["class"])
            // The native layout draws neither, so they go with their text.
            .add_clean_content_tags(["svg", "math"])
            .add_url_schemes(["data"])
            .set_tag_attribute_value("a", "target", "_blank")
            .attribute_filter(|tag, attr, value| match attr {
                "class" => {
                    let kept: Vec<&str> = value
                        .split_ascii_whitespace()
                        .filter(|c| ALLOWED_CLASSES.contains(c))
                        .collect();
                    (!kept.is_empty()).then(|| kept.join(" ").into())
                }
                "href" | "src"
                    if tag != "img"
                        && value
                            .trim_start()
                            .get(..5)
                            .is_some_and(|s| s.eq_ignore_ascii_case("data:")) =>
                {
                    None
                }
                _ => Some(value.into()),
            });
        b
    })
}

/// `sanitizeMarkdown`.
pub fn sanitize_markdown(html: &str) -> String {
    if html.is_empty() {
        return String::new();
    }
    sanitizer().clean(html).to_string()
}

/// `renderMarkdown`: parse, then sanitize.
pub fn render_markdown(src: &str, opts: ParseOptions) -> String {
    if src.is_empty() {
        return String::new();
    }
    sanitize_markdown(&parse_markdown(src, opts))
}

#[cfg(test)]
mod tests;
