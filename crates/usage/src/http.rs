//! The JSON HTTP helper every fetcher uses.

use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

use crate::ctx::{Ctx, CtxError};

pub(crate) const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_BODY: usize = 1 << 20;

/// The default client: a 15 s overall timeout per request.
pub fn default_client() -> reqwest::Client {
    recur_http::client(|b| b.timeout(DEFAULT_HTTP_TIMEOUT))
}

/// Sends one request and reads at most 1 MiB of the answer. Empty header
/// values are not sent. Transport failures read `Post "url": cause`.
pub(crate) async fn do_json(
    ctx: &Ctx,
    client: &reqwest::Client,
    method: reqwest::Method,
    url: &str,
    body: Option<Vec<u8>>,
    headers: &[(&str, String)],
) -> Result<(u16, Vec<u8>), String> {
    let verb = go_method(&method);
    let mut h = HeaderMap::new();
    for (k, v) in headers {
        if v.is_empty() {
            continue;
        }
        let name = HeaderName::from_bytes(k.as_bytes())
            .map_err(|_| format!("net/http: invalid header field name {k:?}"))?;
        let value = HeaderValue::from_str(v)
            .map_err(|_| format!("net/http: invalid header field value for {k:?}"))?;
        h.insert(name, value);
    }
    let mut req = client.request(method, url).headers(h);
    if let Some(b) = body {
        req = req.body(b);
    }
    let send = async {
        let mut resp = req.send().await.map_err(|e| url_error(verb, url, &e))?;
        let status = resp.status().as_u16();
        let mut raw = Vec::new();
        while raw.len() < MAX_BODY {
            match resp.chunk().await {
                Ok(Some(c)) => {
                    let take = (MAX_BODY - raw.len()).min(c.len());
                    raw.extend_from_slice(&c[..take]);
                }
                Ok(None) => break,
                Err(e) => return Err(err_chain(&e)),
            }
        }
        Ok((status, raw))
    };
    match ctx.run(send).await {
        Ok(r) => r,
        Err(e) => Err(format!("{verb} {url:?}: {e}")),
    }
}

fn go_method(m: &reqwest::Method) -> &'static str {
    match *m {
        reqwest::Method::POST => "Post",
        reqwest::Method::PUT => "Put",
        reqwest::Method::DELETE => "Delete",
        reqwest::Method::HEAD => "Head",
        reqwest::Method::PATCH => "Patch",
        _ => "Get",
    }
}

fn url_error(verb: &str, url: &str, e: &reqwest::Error) -> String {
    let cause = if e.is_timeout() {
        format!(
            "{} (Client.Timeout exceeded while awaiting headers)",
            CtxError::DeadlineExceeded
        )
    } else {
        err_chain(e)
    };
    format!("{verb} {url:?}: {cause}")
}

/// The causes of an error joined with `: `.
fn err_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut cur = e.source();
    while let Some(err) = cur {
        let s = err.to_string();
        if !parts.iter().any(|p| p.contains(&s)) {
            parts.push(s);
        }
        cur = err.source();
    }
    if parts.is_empty() {
        e.to_string()
    } else {
        parts.join(": ")
    }
}

/// `HTTP <status>: <first 180 bytes of the body>`.
pub(crate) fn http_err(status: u16, raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let msg = text.trim();
    let msg = if msg.len() > 180 {
        String::from_utf8_lossy(&msg.as_bytes()[..180]).into_owned()
    } else {
        msg.to_owned()
    };
    if msg.is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status}: {msg}")
    }
}

pub(crate) fn is_rate_limited(status: u16) -> bool {
    status == 429
}

pub(crate) fn is_auth_err(status: u16) -> bool {
    status == 401 || status == 403
}

/// A transport failure row ("Could not reach …").
pub(crate) fn is_reachability_error(msg: &str) -> bool {
    msg.starts_with("Could not reach")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_err_clips_like_go() {
        assert_eq!(http_err(500, b""), "HTTP 500");
        assert_eq!(http_err(500, b" boom \n"), "HTTP 500: boom");
        let long = "x".repeat(300);
        assert_eq!(
            http_err(502, long.as_bytes()),
            format!("HTTP 502: {}", "x".repeat(180))
        );
    }
}
