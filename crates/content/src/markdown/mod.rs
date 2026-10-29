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

use crate::marked::LexError;
use crate::purify::{self, Config};

pub(crate) fn markdown_config() -> &'static Config {
    static CFG: OnceLock<Config> = OnceLock::new();
    CFG.get_or_init(Config::markdown)
}

/// `sanitizeMarkdown`.
pub fn sanitize_markdown(html: &str) -> String {
    if html.is_empty() {
        return String::new();
    }
    purify::sanitize(html, markdown_config())
}

/// `renderMarkdown`: parse, then sanitize.
pub fn render_markdown(src: &str, opts: ParseOptions) -> Result<String, LexError> {
    if src.is_empty() {
        return Ok(String::new());
    }
    Ok(sanitize_markdown(&parse_markdown(src, opts)?))
}

#[cfg(test)]
mod tests;
