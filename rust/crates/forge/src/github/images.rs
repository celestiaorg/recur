//! Authenticated fetch of GitHub-hosted comment images. User attachments
//! 404 without a token, so requests carry `gh auth token`; redirects may only
//! go to GitHub's attachment S3 buckets, and credentials never follow them
//! off GitHub.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use url::Url;

use super::GitHub;
use crate::{ForgeError, ForgeErrorKind, ImageBytes};

pub(crate) const MAX_BYTES: usize = 8 << 20;
const TIMEOUT: Duration = Duration::from_secs(20);
const TOKEN_TTL: Duration = Duration::from_secs(30);
const MAX_REDIRECTS: usize = 5;

/// One upstream answer: status, headers the proxy reads, and up to
/// `max_bytes + 1` body bytes.
#[derive(Debug, Clone, Default)]
pub struct ImageResponse {
    pub status: u16,
    pub reason: String,
    pub content_type: String,
    pub location: String,
    pub body: Vec<u8>,
}

/// Issues one GET without following redirects. The default is reqwest;
/// tests substitute their own.
#[async_trait]
pub trait ImageTransport: Send + Sync {
    async fn get(
        &self,
        url: &Url,
        headers: &[(&str, String)],
        max_bytes: usize,
    ) -> Result<ImageResponse, String>;
}

pub(crate) struct HttpTransport {
    client: reqwest::Client,
}

impl HttpTransport {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default();
        Self { client }
    }
}

#[async_trait]
impl ImageTransport for HttpTransport {
    async fn get(
        &self,
        url: &Url,
        headers: &[(&str, String)],
        max_bytes: usize,
    ) -> Result<ImageResponse, String> {
        let mut req = self.client.get(url.clone());
        for (k, v) in headers {
            req = req.header(*k, v);
        }
        let mut res = req.send().await.map_err(|e| e.to_string())?;
        let header = |name: reqwest::header::HeaderName| {
            res.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_owned()
        };
        let content_type = header(reqwest::header::CONTENT_TYPE);
        let location = header(reqwest::header::LOCATION);
        let status = res.status();
        let mut body = Vec::new();
        while body.len() <= max_bytes {
            match res.chunk().await.map_err(|e| e.to_string())? {
                Some(chunk) => body.extend_from_slice(&chunk),
                None => break,
            }
        }
        body.truncate(max_bytes + 1);
        Ok(ImageResponse {
            status: status.as_u16(),
            reason: status.canonical_reason().unwrap_or_default().to_owned(),
            content_type,
            location,
            body,
        })
    }
}

/// Image URLs the proxy may fetch: githubusercontent.com hosts and GitHub
/// attachment paths, https only, no userinfo.
pub fn allowed_image_url(u: &Url) -> bool {
    if u.scheme() != "https" || !u.username().is_empty() || u.password().is_some() {
        return false;
    }
    let host = u.host_str().unwrap_or_default().to_lowercase();
    if host == "githubusercontent.com" || host.ends_with(".githubusercontent.com") {
        return true;
    }
    if host != "github.com" && host != "www.github.com" {
        return false;
    }
    let p = u.path();
    if p == "/user-attachments" || p.starts_with("/user-attachments/") {
        return true;
    }
    let parts: Vec<&str> = p.trim_start_matches('/').split('/').collect();
    parts.len() >= 3 && parts[2] == "assets"
}

/// Redirect targets: allowed image URLs, or GitHub's user-asset S3 buckets
/// (only as redirects, so the proxy is not an open S3 fetcher).
pub fn allowed_image_redirect(u: &Url) -> bool {
    if allowed_image_url(u) {
        return true;
    }
    if u.scheme() != "https" || !u.username().is_empty() || u.password().is_some() {
        return false;
    }
    let host = u.host_str().unwrap_or_default().to_lowercase();
    if !host.ends_with(".amazonaws.com") || !host.contains(".s3.") {
        return false;
    }
    host.starts_with("github-production-user-asset-") || host == "github-cloud.s3.amazonaws.com"
}

/// 401/403/404 pass through; anything else is a bad gateway.
fn status_from_upstream(code: u16) -> u16 {
    if matches!(code, 401 | 403 | 404) {
        code
    } else {
        502
    }
}

/// Go `http.DetectContentType`, image signatures only (anything else is not
/// an image).
fn sniff_image(b: &[u8]) -> Option<&'static str> {
    let webp = b.len() >= 14 && &b[..4] == b"RIFF" && &b[8..14] == b"WEBPVP";
    [
        (&b"\x00\x00\x01\x00"[..], "image/x-icon"),
        (b"\x00\x00\x02\x00", "image/x-icon"),
        (b"BM", "image/bmp"),
        (b"GIF87a", "image/gif"),
        (b"GIF89a", "image/gif"),
    ]
    .into_iter()
    .find(|(sig, _)| b.starts_with(sig))
    .map(|(_, ct)| ct)
    .or(webp.then_some("image/webp"))
    .or_else(|| {
        b.starts_with(b"\x89PNG\x0D\x0A\x1A\x0A")
            .then_some("image/png")
    })
    .or_else(|| b.starts_with(b"\xFF\xD8\xFF").then_some("image/jpeg"))
}

/// Go's redirect rule for sensitive headers: keep them only on the same
/// host or a subdomain of it.
fn keeps_auth(initial: &Url, dest: &Url) -> bool {
    let host = |u: &Url| {
        let h = u.host_str().unwrap_or_default().to_lowercase();
        match u.port() {
            Some(p) => format!("{h}:{p}"),
            None => h,
        }
    };
    let (i, d) = (host(initial), host(dest));
    d == i || d.ends_with(&format!(".{i}"))
}

fn http_err(status: u16, message: impl Into<String>) -> ForgeError {
    ForgeError::new(ForgeErrorKind::Http(status), message)
}

impl GitHub {
    /// Go `cachedGHAuthToken`: `gh auth token`, reused for 30 s.
    async fn auth_token(&self) -> String {
        if let Some((tok, at)) = self.image_token.lock().clone()
            && !tok.is_empty()
            && at.elapsed() < TOKEN_TTL
        {
            return tok;
        }
        let res = tokio::time::timeout(
            Duration::from_secs(5),
            self.gh.stdout(std::path::Path::new(""), &["auth", "token"]),
        )
        .await;
        let Ok(Ok(out)) = res else {
            return String::new();
        };
        let tok = out.trim().to_owned();
        *self.image_token.lock() = Some((tok.clone(), Instant::now()));
        tok
    }

    /// Go `ServeGitHubImage`. `Ok(None)` when the URL is not a GitHub image.
    pub(crate) async fn image(&self, raw: &str) -> Result<Option<ImageBytes>, ForgeError> {
        let Ok(target) = Url::parse(raw.trim()) else {
            return Ok(None);
        };
        if !allowed_image_url(&target) {
            return Ok(None);
        }
        match tokio::time::timeout(TIMEOUT, self.fetch_image_bytes(target.clone())).await {
            Ok(res) => res.map(Some),
            Err(_) => Err(http_err(
                502,
                format!("Get {:?}: context deadline exceeded", target.as_str()),
            )),
        }
    }

    async fn fetch_image_bytes(&self, target: Url) -> Result<ImageBytes, ForgeError> {
        let token = self.auth_token().await;
        let mut headers: Vec<(&str, String)> = Vec::new();
        if !token.is_empty() {
            headers.push(("Authorization", format!("Bearer {token}")));
        }
        headers.push(("User-Agent", "Recur".to_owned()));
        headers.push(("Accept", "image/*,*/*;q=0.8".to_owned()));
        let mut url = target.clone();
        let mut redirects = 0;
        let res = loop {
            let res = self
                .image_transport
                .get(&url, &headers, MAX_BYTES)
                .await
                .map_err(|e| http_err(502, format!("Get {:?}: {e}", url.as_str())))?;
            let redirect =
                matches!(res.status, 301 | 302 | 303 | 307 | 308) && !res.location.is_empty();
            if !redirect {
                break res;
            }
            let next = url
                .join(&res.location)
                .map_err(|e| http_err(502, format!("Get {:?}: {e}", url.as_str())))?;
            redirects += 1;
            if redirects >= MAX_REDIRECTS {
                return Err(http_err(
                    502,
                    format!("Get {:?}: too many redirects", next.as_str()),
                ));
            }
            if !allowed_image_redirect(&next) {
                return Err(http_err(
                    502,
                    format!("Get {:?}: redirect not allowed", next.as_str()),
                ));
            }
            if !keeps_auth(&target, &next) {
                headers.retain(|(k, _)| *k != "Authorization");
            }
            url = next;
        };
        if !(200..300).contains(&res.status) {
            return Err(http_err(
                status_from_upstream(res.status),
                format!("upstream {} {}", res.status, res.reason),
            ));
        }
        let mut ct = res.content_type.clone();
        if let Some(i) = ct.find(';') {
            ct = ct[..i].trim().to_owned();
        }
        if !ct.is_empty() && !ct.to_lowercase().starts_with("image/") {
            return Err(http_err(415, "not an image"));
        }
        if res.body.len() > MAX_BYTES {
            return Err(http_err(413, "image too large"));
        }
        if ct.is_empty() {
            ct = sniff_image(&res.body)
                .ok_or_else(|| http_err(415, "not an image"))?
                .to_owned();
        }
        Ok(ImageBytes {
            content_type: ct,
            bytes: res.body,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use parking_lot::Mutex;

    use super::*;
    use crate::cli::{Gh, write_script};

    const PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8,
        0xff, 0xff, 0x3f, 0x00, 0x05, 0xfe, 0x02, 0xfe, 0xdc, 0xcc, 0x59, 0xe7, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
    const ASSET: &str =
        "https://github.com/user-attachments/assets/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    const S3: &str =
        "https://github-production-user-asset-6210df.s3.amazonaws.com/1/shot.png?X-Amz-Signature=x";

    type Seen = Arc<Mutex<Vec<(String, Option<String>)>>>;

    struct Fake {
        seen: Seen,
        answer: Box<dyn Fn(&Url) -> ImageResponse + Send + Sync>,
    }

    #[async_trait]
    impl ImageTransport for Fake {
        async fn get(
            &self,
            url: &Url,
            headers: &[(&str, String)],
            _max: usize,
        ) -> Result<ImageResponse, String> {
            let auth = headers
                .iter()
                .find(|(k, _)| *k == "Authorization")
                .map(|(_, v)| v.clone());
            self.seen.lock().push((url.to_string(), auth));
            Ok((self.answer)(url))
        }
    }

    fn png() -> ImageResponse {
        ImageResponse {
            status: 200,
            reason: "OK".into(),
            content_type: "image/png".into(),
            body: PNG.to_vec(),
            ..Default::default()
        }
    }

    fn github(
        token: &str,
        answer: impl Fn(&Url) -> ImageResponse + Send + Sync + 'static,
    ) -> (GitHub, Seen, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let gh = write_script(dir.path(), "gh", &format!("#!/bin/sh\necho '{token}'\n"));
        let seen: Seen = Arc::default();
        let g = GitHub::with_gh(Gh::new(gh)).with_image_transport(Arc::new(Fake {
            seen: Arc::clone(&seen),
            answer: Box::new(answer),
        }));
        (g, seen, dir)
    }

    #[test]
    fn allowed_urls() {
        for ok in [
            "https://github.com/user-attachments/assets/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            "https://www.github.com/user-attachments/assets/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            "https://github.com/acme/app/assets/1/bbbbbbbb-cccc-dddd-eeee-ffffffffffff",
            "https://private-user-images.githubusercontent.com/1/shot.png?jwt=x",
            "https://user-images.githubusercontent.com/1/shot.png",
        ] {
            assert!(allowed_image_url(&Url::parse(ok).unwrap()), "{ok}");
        }
        for bad in [
            "https://github-production-user-asset-6210df.s3.amazonaws.com/1/shot.png",
            "https://evil.example/pixel.png",
            "https://github.com/login",
            "https://github.com/acme/app/blob/main/shot.png",
            "https://github.com.evil.example/user-attachments/assets/x",
            "https://notgithubusercontent.com/shot.png",
            "https://user:pass@github.com/user-attachments/assets/x",
            "https://github.com/user-attachments/../login",
            "http://github.com/user-attachments/assets/x",
            "ftp://github.com/user-attachments/assets/x",
        ] {
            assert!(!allowed_image_url(&Url::parse(bad).unwrap()), "{bad}");
        }
        assert!(Url::parse("/user-attachments/assets/x").is_err());
        assert!(allowed_image_redirect(&Url::parse(S3).unwrap()));
        assert!(!allowed_image_redirect(
            &Url::parse("http://github.com/user-attachments/assets/x").unwrap()
        ));
    }

    #[tokio::test]
    async fn proxies_with_bearer_token() {
        let (g, seen, _d) = github("test-token", |_| png());
        let got = g.image(ASSET).await.unwrap().unwrap();
        assert_eq!(got.content_type, "image/png");
        assert_eq!(got.bytes, PNG);
        assert_eq!(
            seen.lock()[0],
            (ASSET.to_owned(), Some("Bearer test-token".to_owned()))
        );
    }

    #[tokio::test]
    async fn rejects_off_allowlist_and_http_without_fetching() {
        let (g, seen, _d) = github("t", |_| png());
        for url in [
            "https://evil.example/pixel.png",
            "http://github.com/user-attachments/assets/x",
            S3,
            "",
        ] {
            assert_eq!(g.image(url).await.unwrap(), None, "{url}");
        }
        assert!(seen.lock().is_empty());
    }

    #[tokio::test]
    async fn rejects_html_upstream() {
        let (g, _seen, _d) = github("t", |_| ImageResponse {
            status: 200,
            content_type: "text/html; charset=utf-8".into(),
            body: b"<html>login</html>".to_vec(),
            ..Default::default()
        });
        assert_eq!(
            g.image(ASSET).await.unwrap_err().kind,
            ForgeErrorKind::Http(415)
        );
    }

    #[tokio::test]
    async fn public_image_without_token_and_sniffed_type() {
        let (g, seen, _d) = github("", |_| ImageResponse {
            content_type: String::new(),
            ..png()
        });
        let got = g
            .image("https://user-images.githubusercontent.com/1/shot.png")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.content_type, "image/png");
        assert_eq!(seen.lock()[0].1, None);
    }

    #[tokio::test]
    async fn follows_redirect_to_s3_without_credentials() {
        let (g, seen, _d) = github("test-token", |u| {
            if u.host_str() == Some("github.com") {
                ImageResponse {
                    status: 302,
                    location: S3.into(),
                    ..Default::default()
                }
            } else {
                png()
            }
        });
        let got = g.image(ASSET).await.unwrap().unwrap();
        assert_eq!(got.bytes, PNG);
        let seen = seen.lock();
        assert_eq!(seen.len(), 2);
        assert!(seen[1].0.contains("s3.amazonaws.com"));
        assert_eq!(seen[1].1, None, "Authorization leaked to S3");
    }

    #[tokio::test]
    async fn rejects_https_to_http_redirect_and_upstream_errors() {
        let (g, _seen, _d) = github("t", |_| ImageResponse {
            status: 302,
            location: "http://github.com/user-attachments/assets/x".into(),
            ..Default::default()
        });
        assert_eq!(
            g.image(ASSET).await.unwrap_err().kind,
            ForgeErrorKind::Http(502)
        );
        let (g, _seen, _d) = github("t", |_| ImageResponse {
            status: 404,
            reason: "Not Found".into(),
            ..Default::default()
        });
        let err = g.image(ASSET).await.unwrap_err();
        assert_eq!(err.kind, ForgeErrorKind::Http(404));
        assert_eq!(err.message, "upstream 404 Not Found");
        let (g, _seen, _d) = github("t", |_| ImageResponse {
            body: vec![0; MAX_BYTES + 1],
            ..png()
        });
        assert_eq!(
            g.image(ASSET).await.unwrap_err().kind,
            ForgeErrorKind::Http(413)
        );
    }

    #[test]
    fn sniffs_images() {
        assert_eq!(sniff_image(PNG), Some("image/png"));
        assert_eq!(sniff_image(b"GIF89a.."), Some("image/gif"));
        assert_eq!(sniff_image(b"\xFF\xD8\xFF\xE0"), Some("image/jpeg"));
        assert_eq!(
            sniff_image(b"RIFF\x00\x00\x00\x00WEBPVP8 "),
            Some("image/webp")
        );
        assert_eq!(sniff_image(b"<html>"), None);
    }
}
