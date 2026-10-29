//! Language-server hover and completion HTML, with fences tinted by
//! `codehl` and DOMPurify (plus the app-wide hook) applied before display.

use std::sync::OnceLock;

use recur_textmate::highlight_html_code_blocks;

use crate::js::Re;
use crate::purify::{self, Config, Hook};

/// `stripDangerousHtml`: belt-and-braces regex pass after DOMPurify.
pub fn strip_dangerous_html(html: &str) -> String {
    static RES: OnceLock<[Re; 3]> = OnceLock::new();
    let [script, handler, js] = RES.get_or_init(|| {
        [
            Re::new(r"<script\b[\s\S]*?<\/script>", "gi"),
            Re::new(r#"\son[a-z]+\s*=\s*("[^"]*"|'[^']*'|[^\s>]+)"#, "gi"),
            Re::new(r"javascript:", "gi"),
        ]
    });
    js.replace(&handler.replace(&script.replace(html, ""), ""), "")
}

/// `sanitizeLspHTML`.
pub fn sanitize_lsp_html(html: &str) -> String {
    static CFG: OnceLock<Config> = OnceLock::new();
    let tinted = highlight_html_code_blocks(html);
    let clean = purify::sanitize(
        &tinted,
        CFG.get_or_init(|| Config::default_with(Hook::Recur)),
    );
    strip_dangerous_html(&clean)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_removes_img_onerror() {
        let out = strip_dangerous_html("<img src=x onerror=\"alert(1)\">");
        assert!(!out.contains("onerror"));
        assert!(!out.contains("alert"));
    }

    #[test]
    fn strips_event_handlers_then_highlights_fences() {
        let html = "<p>hi</p><img src=x onerror=\"alert(1)\"><pre><code class=\"language-js\">const x = 1</code></pre>";
        let out = sanitize_lsp_html(html);
        assert!(!out.contains("onerror"));
        assert!(!out.contains("alert"));
        assert!(out.contains("sh-kw"));
    }

    #[test]
    fn tints_hover_fences_after_the_hook_strips_language_classes() {
        let go = sanitize_lsp_html(
            "<pre><code class=\"language-go\">func (e *Engine) send(ok bool) string</code></pre>",
        );
        assert!(go.contains("class=\"sh-kw\">func</span>"));
        assert!(go.contains("class=\"sh-kw\">bool</span>"));
        let ts = sanitize_lsp_html(
            "<pre><code class=\"language-typescript\">(alias) function toErrorResponse(error: unknown): void</code></pre>",
        );
        assert!(ts.contains("class=\"sh-kw\">function</span>"));
        assert!(ts.contains("class=\"sh-kw\">unknown</span>"));
    }
}
