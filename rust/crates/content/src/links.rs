//! `lib/open-external.ts`: absolute http(s) links leave the app for the OS browser, even when
//! they point at the app's own origin; relative hrefs do nothing natively.

use gpui_kit::App;

/// `isExternalHttpUrl`: the normalized URL when `href` is absolute http(s).
pub fn external_http_url(href: &str) -> Option<String> {
    let raw = crate::js::trim(href);
    if raw.is_empty() {
        return None;
    }
    let u = url::Url::parse(raw).ok()?;
    matches!(u.scheme(), "http" | "https").then(|| u.to_string())
}

/// `openExternalUrl`.
pub fn open_external_url(href: &str, cx: &mut App) {
    if let Some(url) = external_http_url(href) {
        cx.open_url(&url);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_absolute_http_urls_open() {
        assert_eq!(
            external_http_url(" https://x.test/a b").as_deref(),
            Some("https://x.test/a%20b")
        );
        assert_eq!(
            external_http_url("http://x.test").as_deref(),
            Some("http://x.test/")
        );
        assert!(external_http_url("/workspaces/1").is_none());
        assert!(external_http_url("javascript:alert(1)").is_none());
        assert!(external_http_url("mailto:a@b.c").is_none());
    }
}
