//! Open http(s) links in the OS browser (`lib/open-external.ts`). Absolute http(s) always
//! leaves the app; relative in-app hrefs never do.

use gpui_kit::App;

/// `isExternalHttpUrl`: an absolute http(s) URL.
pub fn is_external_http_url(href: &str) -> bool {
    let raw = href.trim();
    if raw.is_empty() {
        return false;
    }
    url::Url::parse(raw).is_ok_and(|u| u.scheme() == "http" || u.scheme() == "https")
}

/// The href to open for a link, `None` when the click should stay in the app
/// (`externalHrefFromAnchor`). The serialized URL normalizes like `new URL(raw).href`.
pub fn external_href(href: &str) -> Option<String> {
    let raw = href.trim();
    if !is_external_http_url(raw) {
        return None;
    }
    url::Url::parse(raw).ok().map(|u| u.to_string())
}

/// `openExternalUrl`: no-op for anything but http(s).
pub fn open_external_url(href: &str, cx: &mut App) {
    if let Some(url) = external_href(href) {
        cx.open_url(&url);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN: &str = "http://localhost:3000";

    #[test]
    fn accepts_any_absolute_http_url_including_this_origin() {
        assert!(is_external_http_url("https://github.com/org/repo"));
        assert!(is_external_http_url("http://example.com/a?q=1"));
        assert!(is_external_http_url(&format!("{ORIGIN}/foo")));
    }

    #[test]
    fn rejects_relative_hrefs_and_other_schemes() {
        assert!(!is_external_http_url("/workspaces/x/screenshots/a.png"));
        assert!(!is_external_http_url("javascript:alert(1)"));
        assert!(!is_external_http_url("data:text/html,hi"));
        assert!(!is_external_http_url("file:///etc/passwd"));
        assert!(!is_external_http_url(""));
        assert!(!is_external_http_url("not a url"));
    }

    #[test]
    fn external_href_normalizes_and_skips_in_app_links() {
        assert_eq!(
            external_href("https://example.com/path").as_deref(),
            Some("https://example.com/path")
        );
        assert_eq!(
            external_href("https://example.com").as_deref(),
            Some("https://example.com/")
        );
        assert_eq!(
            external_href(&format!("{ORIGIN}/evil")).as_deref(),
            Some("http://localhost:3000/evil")
        );
        assert_eq!(external_href("/workspaces/id/screenshots/x.png"), None);
        assert_eq!(external_href("javascript:alert(1)"), None);
    }
}
