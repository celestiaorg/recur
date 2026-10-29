//! Marked with Recur's renderers and fence highlighting. The output
//! is unsanitized; [`super::sanitize_markdown`] runs before anything renders it.

use std::cell::Cell;
use std::sync::OnceLock;

use recur_textmate::{highlight_code, lang_from_info};

use crate::emoji::expand_emoji_shortcodes;
use crate::js::{self, Re, encode_uri_component, utf16_len};
use crate::marked::{self, LexError, Parser, Renderer, Token};

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

/// Recur's renderer overrides (`marked.use({ renderer })`). `demote` is the WebFetch engine.
struct Recur {
    demote: bool,
    /// Depth of nested links, so a demoted image inside `[![alt](img)](href)` returns text.
    link_depth: Cell<usize>,
}

fn code_ref_info() -> &'static Re {
    static CODE_REF: OnceLock<Re> = OnceLock::new();
    re(&CODE_REF, r"^(\d+):(\d+):(.+)$", "")
}

impl Renderer for Recur {
    /// `renderCode`: mermaid placeholders, Cursor `start:end:path` cards, highlighted fences.
    fn code(&self, _p: &Parser, text: &str, lang: Option<&str>) -> String {
        let info = js::trim(lang.unwrap_or(""));
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

    /// `renderLink`: only short http(s) links stay clickable.
    fn link(&self, p: &Parser, href: &str, title: Option<&str>, tokens: &[Token]) -> String {
        self.link_depth.set(self.link_depth.get() + 1);
        let text = p.parse_inline(tokens);
        self.link_depth.set(self.link_depth.get() - 1);
        let plain = strip_tags(&text);
        static HTTP: OnceLock<Re> = OnceLock::new();
        let ok = !href.is_empty()
            && re(&HTTP, r"^https?:\/\/", "i").test(href)
            && utf16_len(&plain) <= 120;
        if !ok {
            return text;
        }
        let t = title
            .filter(|t| !t.is_empty())
            .map(|t| format!(" title=\"{}\"", escape_text(t)))
            .unwrap_or_default();
        format!("<a href=\"{}\"{t}>{text}</a>", escape_text(href))
    }

    fn html(&self, _p: &Parser, text: &str) -> String {
        if self.demote {
            // `renderHtml`: keep inner text, never the raw tags.
            return escape_text(&strip_tags(text));
        }
        // `renderHtmlWithGitHubImages`: GitHub-pasted `<img>`s survive (proxied).
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

    fn image(
        &self,
        _p: &Parser,
        href: &str,
        title: Option<&str>,
        text: &str,
        _tokens: &[Token],
    ) -> String {
        let t = title
            .filter(|t| !t.is_empty())
            .map(|t| format!(" title=\"{}\"", escape_text(t)))
            .unwrap_or_default();
        if !self.demote {
            // `renderLiveImage`.
            let src = github_image_src(js::trim(href));
            if src.is_empty() {
                return escape_text(text);
            }
            return format!(
                "<img src=\"{}\" alt=\"{}\"{t}>",
                escape_text(&src),
                escape_text(text)
            );
        }
        // `renderImage`: never emit `<img>`, fetched pages must not load third-party URLs.
        let label = match js::trim(text) {
            "" => "image",
            l => l,
        };
        if self.link_depth.get() > 0 {
            return escape_text(label);
        }
        let src = js::trim(href);
        static HTTP: OnceLock<Re> = OnceLock::new();
        if !src.is_empty() && re(&HTTP, r"^https?:\/\/", "i").test(src) && utf16_len(label) <= 120 {
            return format!(
                "<a href=\"{}\"{t}>{}</a>",
                escape_text(src),
                escape_text(label)
            );
        }
        escape_text(label)
    }
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

/// `parseMarkdown`: unsanitized HTML from marked and `highlightCode`.
pub fn parse_markdown(src: &str, opts: ParseOptions) -> Result<String, LexError> {
    if src.is_empty() {
        return Ok(String::new());
    }
    let prepared = normalize_code_refs(&expand_emoji_shortcodes(src, true));
    let renderer = Recur {
        demote: opts.demote_images,
        link_depth: Cell::new(0),
    };
    marked::parse(&prepared, &renderer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn does_not_sanitize() {
        let raw = parse_markdown("```js\nconst x = 1\n```", ParseOptions::default()).unwrap();
        assert!(raw.contains("sh-kw"));
    }

    #[test]
    fn code_refs_become_file_cards_even_mid_sentence() {
        let raw = parse_markdown(
            "see ```12:14:src/a.ts\n<div class=\"x\">\n``` now",
            ParseOptions::default(),
        )
        .unwrap();
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

    #[derive(serde::Deserialize)]
    struct Case {
        md: String,
        live: String,
        demote: String,
    }

    #[test]
    fn matches_markdown_parse_ts() {
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("../../tests/golden/parse.json")).unwrap();
        let mut failures = Vec::new();
        for (i, c) in cases.iter().enumerate() {
            for (demote, want) in [(false, &c.live), (true, &c.demote)] {
                let got = parse_markdown(
                    &c.md,
                    ParseOptions {
                        demote_images: demote,
                    },
                )
                .unwrap_or_else(|e| format!("ERROR: {e}"));
                if &got != want {
                    failures.push(format!(
                        "#{i} demote={demote}\n--- md\n{}\n--- want\n{want}\n--- got\n{got}",
                        c.md
                    ));
                }
            }
        }
        assert!(
            failures.is_empty(),
            "{} differ:\n{}",
            failures.len(),
            failures
                .iter()
                .take(6)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n\n")
        );
    }

    #[test]
    fn mermaid_fences_become_placeholders() {
        let raw =
            parse_markdown("```mermaid\ngraph TD\nA-->B\n```", ParseOptions::default()).unwrap();
        assert_eq!(
            raw,
            "<div class=\"md-mermaid\"><pre class=\"md-mermaid-src\"><code>graph TD\nA--&gt;B</code></pre></div>"
        );
    }
}
