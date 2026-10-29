//! WebFetch / WebSearch card rules (frontend `lib/webtools.ts`): never a
//! JSON dump, friendly URLs.

use serde_json::Value;

fn str_of<'a>(raw: Option<&'a Value>, key: &str) -> &'a str {
    match raw.and_then(|v| v.get(key)) {
        Some(Value::String(s)) => s.trim(),
        _ => "",
    }
}

fn parse_fallback(fallback: &str) -> Option<Value> {
    let fb = fallback.trim();
    if !fb.starts_with('{') {
        return None;
    }
    serde_json::from_str::<Value>(fb)
        .ok()
        .filter(Value::is_object)
}

/// The http(s) URL from WebFetch args, else a bare-URL fallback (the feed
/// summary).
pub fn web_fetch_url(raw: Option<&Value>, fallback: &str) -> String {
    let parsed = parse_fallback(fallback);
    let from_args = match str_of(raw, "url") {
        "" => str_of(parsed.as_ref(), "url"),
        s => s,
    };
    if !from_args.is_empty() {
        return from_args.to_owned();
    }
    let fb = fallback.trim();
    let lower = fb.get(..8).unwrap_or(fb).to_ascii_lowercase();
    if (lower.starts_with("http://") || lower.starts_with("https://")) && !fb.starts_with('{') {
        return fb.to_owned();
    }
    String::new()
}

/// Claude's WebFetch extraction prompt; optional, never dumped as JSON.
pub fn web_fetch_prompt(raw: Option<&Value>, fallback: &str) -> String {
    match str_of(raw, "prompt") {
        "" => str_of(parse_fallback(fallback).as_ref(), "prompt").to_owned(),
        s => s.to_owned(),
    }
}

/// The WebSearch query. Claude uses `query`; some providers send
/// `search_term`.
pub fn web_search_query(raw: Option<&Value>, fallback: &str) -> String {
    let parsed = parse_fallback(fallback);
    for q in [
        str_of(raw, "query"),
        str_of(raw, "search_term"),
        str_of(parsed.as_ref(), "query"),
        str_of(parsed.as_ref(), "search_term"),
    ] {
        if !q.is_empty() {
            return q.to_owned();
        }
    }
    let fb = fallback.trim();
    if !fb.is_empty() && !fb.starts_with('{') {
        return fb.to_owned();
    }
    String::new()
}

/// Chip label: host + path, no scheme. Long paths keep the host and the last
/// segment (`docs.example.com/…/create-token`).
pub fn compact_url(url: &str) -> String {
    let raw = url.trim();
    if raw.is_empty() {
        return String::new();
    }
    let Ok(u) = url::Url::parse(raw) else {
        return raw.to_owned();
    };
    if u.scheme() != "http" && u.scheme() != "https" {
        return raw.to_owned();
    }
    let mut host = u.host_str().unwrap_or_default().to_owned();
    if let Some(p) = u.port() {
        host = format!("{host}:{p}");
    }
    if host
        .get(..4)
        .is_some_and(|w| w.eq_ignore_ascii_case("www."))
    {
        host.drain(..4);
    }
    let path = u.path().trim_end_matches('/');
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let mut tail = String::new();
    if let Some(q) = u.query().filter(|q| !q.is_empty()) {
        tail.push('?');
        tail.push_str(q);
    }
    if let Some(f) = u.fragment().filter(|f| !f.is_empty()) {
        tail.push('#');
        tail.push_str(f);
    }
    match parts.len() {
        0 => host + &tail,
        1 | 2 => format!("{host}/{}{tail}", parts.join("/")),
        n => format!("{host}/…/{}{tail}", parts[n - 1]),
    }
}

/// True only when `text` is a JSON object or array. Fetched READMEs often
/// start with a badge link (`[![CI](…)](…)`), which stays on the markdown
/// path.
pub fn web_result_is_json(text: &str) -> bool {
    let t = text.trim();
    if !t.starts_with('{') && !t.starts_with('[') {
        return false;
    }
    serde_json::from_str::<serde::de::IgnoredAny>(t).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const COINBASE: &str =
        "https://docs.cdp.coinbase.com/api-reference/rest-api/onramp-offramp/create-session-token";

    #[test]
    fn fetch_url() {
        let args = json!({"url": COINBASE, "prompt": "List the fields. Is clientIp required?"});
        assert_eq!(web_fetch_url(Some(&args), ""), COINBASE);
        assert_eq!(web_fetch_url(None, COINBASE), COINBASE);
        assert_eq!(
            web_fetch_url(None, &format!(r#"{{"url":"{COINBASE}"}}"#)),
            COINBASE
        );
        assert_eq!(web_fetch_url(Some(&json!({})), ""), "");
        assert_eq!(web_fetch_url(None, "HTTPS://X.Y"), "HTTPS://X.Y");
        assert_eq!(web_fetch_url(None, "ftp://x"), "");
    }

    #[test]
    fn fetch_prompt() {
        let args = json!({"url": COINBASE, "prompt": "  Is clientIp required?  "});
        assert_eq!(web_fetch_prompt(Some(&args), ""), "Is clientIp required?");
        assert_eq!(web_fetch_prompt(Some(&json!({"url": COINBASE})), ""), "");
        assert_eq!(
            web_fetch_prompt(None, r#"{"prompt":"from summary"}"#),
            "from summary"
        );
        assert_eq!(web_fetch_prompt(None, ""), "");
    }

    #[test]
    fn search_query() {
        assert_eq!(
            web_search_query(Some(&json!({"query": "onramp session token"})), ""),
            "onramp session token"
        );
        assert_eq!(
            web_search_query(Some(&json!({"search_term": "clientIp required"})), ""),
            "clientIp required"
        );
        assert_eq!(web_search_query(None, "bare query"), "bare query");
        assert_eq!(web_search_query(None, r#"{"query":"x"}"#), "x");
    }

    #[test]
    fn compact() {
        assert_eq!(
            compact_url(COINBASE),
            "docs.cdp.coinbase.com/…/create-session-token"
        );
        assert_eq!(compact_url("https://example.com/"), "example.com");
        assert_eq!(
            compact_url("https://www.example.com/docs/fetch"),
            "example.com/docs/fetch"
        );
        assert_eq!(
            compact_url("https://example.com/a?q=1#top"),
            "example.com/a?q=1#top"
        );
        assert_eq!(
            compact_url("https://example.com:8443/a/"),
            "example.com:8443/a"
        );
        assert_eq!(compact_url("https://example.com:443/a"), "example.com/a");
        assert_eq!(compact_url("mailto:x@y.z"), "mailto:x@y.z");
        assert_eq!(compact_url("not a url"), "not a url");
        assert_eq!(compact_url(""), "");
    }

    #[test]
    fn result_is_json() {
        assert!(web_result_is_json(r#"{"error":"not found"}"#));
        assert!(web_result_is_json(r#"  [{"title":"a"}]  "#));
        assert!(!web_result_is_json(
            "[![CI](https://img.shields.io/badge.svg)](https://github.com/org/repo)\n# README"
        ));
        assert!(!web_result_is_json("{docs}\nA page about JSON Schema."));
        assert!(!web_result_is_json(""));
        assert!(!web_result_is_json("# heading"));
    }
}
