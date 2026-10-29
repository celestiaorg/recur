//! The Wails Update Manifest provider: one JSON document describing the
//! latest release and its per-platform artifacts, served statically (one
//! manifest per channel) or dynamically (the server reads the platform /
//! arch / version query parameters).

use std::collections::BTreeMap;
use std::io::Write;
use std::time::Duration;

use base64::Engine;
use futures::future::BoxFuture;
use serde::Deserialize;

use super::{Artifact, CheckRequest, Provider, ProviderError, Release, Verification, semver};

/// The newest manifest `schemaVersion` understood; newer ones are rejected
/// rather than half-parsed.
const SCHEMA_VERSION_MAX: i64 = 1;

/// A manifest is typically well under 100 KiB.
const MAX_MANIFEST_BYTES: usize = 8 * 1024 * 1024;

const ARTIFACT_URL_KEY: &str = "endpoint.artifact.url";

/// Endpoint provider configuration.
#[derive(Debug, Clone, Default)]
pub struct EndpointConfig {
    /// The manifest URL. `{{platform}}`, `{{arch}}`, `{{version}}` and
    /// `{{channel}}` are substituted per check; whatever they do not consume
    /// is appended as query parameters (channel only when set).
    pub url: String,
    /// Requested channel, also enforced client-side: a manifest declaring a
    /// different non-empty channel means "no update".
    pub channel: String,
    /// Sent with every manifest request, and with artifact downloads only on
    /// the manifest's host without an https→http downgrade.
    pub headers: BTreeMap<String, String>,
    /// A custom client. `None` uses one with a 30 second total timeout.
    /// Redirects drop `Authorization` when they cross hosts or downgrade.
    pub client: Option<reqwest::Client>,
    /// Fail a request when response headers take longer than this (Go's
    /// `Transport.ResponseHeaderTimeout`); the body is not bounded by it.
    pub response_header_timeout: Option<Duration>,
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct EndpointError(String);

fn err(msg: impl std::fmt::Display) -> ProviderError {
    Box::new(EndpointError(msg.to_string()))
}

/// Provider for a Wails update manifest.
#[derive(Debug, Clone)]
pub struct EndpointProvider {
    cfg: EndpointConfig,
    client: reqwest::Client,
}

impl EndpointProvider {
    pub fn new(cfg: EndpointConfig) -> Result<Self, ProviderError> {
        if cfg.url.trim().is_empty() {
            return Err(err("endpoint: URL is required"));
        }
        // reqwest strips Authorization (and other sensitive headers) when a
        // redirect changes host or port, which covers an https→http downgrade
        // on the same host; it also stops after 10 redirects.
        let client = match &cfg.client {
            Some(c) => c.clone(),
            None => recur_http::client(|b| b.timeout(Duration::from_secs(30))),
        };
        Ok(Self { cfg, client })
    }

    async fn send(&self, req: reqwest::RequestBuilder) -> Result<reqwest::Response, String> {
        let sent = match self.cfg.response_header_timeout {
            Some(t) => tokio::time::timeout(t, req.send())
                .await
                .map_err(|_| "net/http: timeout awaiting response headers".to_string())?,
            None => req.send().await,
        };
        sent.map_err(|e| e.to_string())
    }

    /// Substitutes placeholders, then appends what they did not consume.
    fn build_url(&self, req: &CheckRequest) -> Result<String, ProviderError> {
        let mut raw = self.cfg.url.clone();
        let params = [
            ("platform", req.platform.as_str(), true),
            ("arch", req.arch.as_str(), true),
            ("version", req.current_version.as_str(), true),
            (
                "channel",
                self.cfg.channel.as_str(),
                !self.cfg.channel.is_empty(),
            ),
        ];
        let mut pending: Vec<(&str, &str)> = Vec::new();
        for (name, value, send) in params {
            let placeholder = format!("{{{{{name}}}}}");
            if raw.contains(&placeholder) {
                raw = raw.replace(&placeholder, &path_escape(value));
            } else if send {
                pending.push((name, value));
            }
        }
        let mut u =
            url::Url::parse(&raw).map_err(|e| err(format!("endpoint: invalid URL: {e}")))?;
        if !pending.is_empty() {
            let mut q: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for (k, v) in u.query_pairs() {
                q.entry(k.into_owned()).or_default().push(v.into_owned());
            }
            for (k, v) in pending {
                q.insert(k.to_string(), vec![v.to_string()]);
            }
            let encoded: Vec<String> = q
                .iter()
                .flat_map(|(k, vs)| {
                    vs.iter()
                        .map(move |v| format!("{}={}", query_escape(k), query_escape(v)))
                })
                .collect();
            u.set_query(Some(&encoded.join("&")));
        }
        Ok(u.to_string())
    }
}

impl Provider for EndpointProvider {
    fn name(&self) -> &str {
        "endpoint"
    }

    fn check<'a>(
        &'a self,
        req: &'a CheckRequest,
    ) -> BoxFuture<'a, Result<Option<Release>, ProviderError>> {
        Box::pin(async move {
            if req.current_version.is_empty() {
                return Err(err("endpoint: CurrentVersion is required"));
            }
            let endpoint_url = self.build_url(req)?;
            let mut http = self
                .client
                .get(&endpoint_url)
                .header("Accept", "application/json");
            for (k, v) in &self.cfg.headers {
                http = http.header(k, v);
            }
            let resp = self
                .send(http)
                .await
                .map_err(|e| err(format!("endpoint: fetch manifest: {e}")))?;
            let status = resp.status().as_u16();
            // 204 from servers that compared versions, 404 from static hosts
            // with nothing published yet.
            if status == 204 || status == 404 {
                return Ok(None);
            }
            if !(200..300).contains(&status) {
                return Err(err(format!(
                    "endpoint: manifest request failed: HTTP {status}"
                )));
            }
            let body = read_limited(resp, MAX_MANIFEST_BYTES).await?;
            let m: Manifest = serde_json::from_slice(&body)
                .map_err(|e| err(format!("endpoint: decode manifest: {e}")))?;
            if m.schema_version > SCHEMA_VERSION_MAX {
                return Err(err(format!(
                    "endpoint: manifest schemaVersion {} is newer than supported version {SCHEMA_VERSION_MAX}",
                    m.schema_version
                )));
            }
            if m.version.is_empty() {
                return Err(err("endpoint: manifest missing version"));
            }
            if !self.cfg.channel.is_empty()
                && !m.channel.is_empty()
                && !self.cfg.channel.eq_ignore_ascii_case(&m.channel)
            {
                return Ok(None);
            }
            if !semver::is_newer(&m.version, &req.current_version) {
                return Ok(None);
            }
            let art = pick_artifact(&m.artifacts, &req.platform, &req.arch).ok_or_else(|| {
                err(format!(
                    "endpoint: manifest {} has no artifact for {}/{}",
                    m.version, req.platform, req.arch
                ))
            })?;
            if art.url.is_empty() {
                return Err(err(format!(
                    "endpoint: artifact for {}/{} has no url",
                    req.platform, req.arch
                )));
            }
            let resolved = resolve_url(&endpoint_url, &art.url)
                .map_err(|e| err(format!("endpoint: artifact url: {e}")))?;
            let filename = if art.filename.is_empty() {
                filename_from_url(&resolved)
            } else {
                art.filename.clone()
            };
            let filetype = if art.filetype.is_empty() {
                filetype_from_filename(&filename)
            } else {
                art.filetype.clone()
            };
            let verification = build_verification(art).map_err(|e| {
                err(format!(
                    "endpoint: manifest {}, artifact for {}/{}: {e}",
                    m.version, req.platform, req.arch
                ))
            })?;
            let mut metadata = m.metadata.unwrap_or_default();
            metadata.insert(ARTIFACT_URL_KEY.to_string(), resolved.into());
            Ok(Some(Release {
                version: semver::trim_prefix(&m.version).to_string(),
                channel: m.channel,
                name: m.name,
                notes: m.notes,
                published_at: chrono::DateTime::parse_from_rfc3339(&m.published_at).ok(),
                artifact: Artifact {
                    filename,
                    filetype,
                    size: art.size,
                    platform: req.platform.clone(),
                    arch: req.arch.clone(),
                },
                verification,
                metadata,
                provider: String::new(),
            }))
        })
    }

    fn download<'a>(
        &'a self,
        rel: &'a Release,
        dst: &'a mut (dyn Write + Send),
        on_progress: &'a mut (dyn FnMut(i64, i64) + Send),
    ) -> BoxFuture<'a, Result<(), ProviderError>> {
        Box::pin(async move {
            if rel.metadata.is_empty() {
                return Err(err("endpoint: release missing metadata"));
            }
            let url = rel
                .metadata
                .get(ARTIFACT_URL_KEY)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| err("endpoint: release metadata missing artifact URL"))?;
            let mut http = self
                .client
                .get(url)
                .header("Accept", "application/octet-stream");
            if headers_allowed_for(&self.cfg.url, url) {
                for (k, v) in &self.cfg.headers {
                    http = http.header(k, v);
                }
            }
            let mut resp = self
                .send(http)
                .await
                .map_err(|e| err(format!("endpoint: download: {e}")))?;
            let status = resp.status().as_u16();
            if !(200..300).contains(&status) {
                return Err(err(format!("endpoint: download: HTTP {status}")));
            }
            let mut total = rel.artifact.size;
            if total == 0
                && let Some(len) = resp.content_length().filter(|l| *l > 0)
            {
                total = len as i64;
            }
            let mut written = 0i64;
            while let Some(chunk) = resp.chunk().await.map_err(err)? {
                dst.write_all(&chunk).map_err(err)?;
                written += chunk.len() as i64;
                on_progress(written, total);
            }
            Ok(())
        })
    }
}

async fn read_limited(mut resp: reqwest::Response, limit: usize) -> Result<Vec<u8>, ProviderError> {
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(err)? {
        let room = limit - body.len();
        body.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if body.len() >= limit {
            break;
        }
    }
    Ok(body)
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct Manifest {
    pub(super) schema_version: i64,
    pub(super) version: String,
    channel: String,
    name: String,
    notes: String,
    published_at: String,
    pub(super) artifacts: Vec<ManifestArtifact>,
    metadata: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct ManifestArtifact {
    pub(super) url: String,
    pub(super) filename: String,
    filetype: String,
    pub(super) size: i64,
    platform: String,
    arch: String,
    digest_algo: String,
    digest: String,
    signature_algo: String,
    signature: String,
}

/// The first artifact matching platform and arch; an empty platform or arch
/// matches anything, and document order breaks ties.
fn pick_artifact<'a>(
    arts: &'a [ManifestArtifact],
    platform: &str,
    arch: &str,
) -> Option<&'a ManifestArtifact> {
    let (want_plat, want_arch) = (platform.to_lowercase(), arch.to_lowercase());
    arts.iter().find(|a| {
        (a.platform.is_empty() || platform_matches(&a.platform, &want_plat))
            && (a.arch.is_empty() || arch_matches(&a.arch, &want_arch))
    })
}

/// Go `GOOS` values plus common aliases, case-insensitive.
fn platform_matches(published: &str, want: &str) -> bool {
    let p = published.to_lowercase();
    p == want
        || match want {
            "darwin" => matches!(p.as_str(), "macos" | "mac" | "osx"),
            "windows" => matches!(p.as_str(), "win" | "win32" | "win64"),
            _ => false,
        }
}

/// Go `GOARCH` values plus common aliases, case-insensitive.
fn arch_matches(published: &str, want: &str) -> bool {
    let p = published.to_lowercase();
    p == want
        || match want {
            "amd64" => matches!(p.as_str(), "x86_64" | "x64"),
            "arm64" => p == "aarch64",
            "386" => matches!(p.as_str(), "i386" | "x86" | "ia32"),
            _ => false,
        }
}

/// Maps base64 digest / signature fields; malformed values are errors, since
/// dropping them would silently downgrade verification.
pub(super) fn build_verification(a: &ManifestArtifact) -> Result<Option<Verification>, String> {
    if a.digest.is_empty() && a.signature.is_empty() {
        return Ok(None);
    }
    let mut v = Verification::default();
    if !a.digest.is_empty() {
        v.digest = decode_b64(&a.digest).ok_or("artifact digest is not valid base64")?;
        v.digest_algo = a.digest_algo.to_lowercase();
    }
    if !a.signature.is_empty() {
        if a.signature_algo.is_empty() {
            return Err("artifact has a signature but no signatureAlgo".into());
        }
        v.signature = decode_b64(&a.signature).ok_or("artifact signature is not valid base64")?;
        v.signature_algo = a.signature_algo.to_lowercase();
    }
    Ok(Some(v))
}

/// Raw (unpadded) or standard base64, with Go's lenient trailing bits.
fn decode_b64(s: &str) -> Option<Vec<u8>> {
    use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
    let lenient = GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true);
    let raw = GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        lenient.with_decode_padding_mode(DecodePaddingMode::RequireNone),
    );
    let std = GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        lenient.with_decode_padding_mode(DecodePaddingMode::RequireCanonical),
    );
    raw.decode(s).or_else(|_| std.decode(s)).ok()
}

/// Resolves a possibly relative artifact URL against the manifest URL;
/// only http(s) results are allowed.
fn resolve_url(base: &str, reference: &str) -> Result<String, String> {
    let b = url::Url::parse(base).map_err(|e| e.to_string())?;
    let r = b.join(reference).map_err(|e| e.to_string())?;
    if r.scheme() != "http" && r.scheme() != "https" {
        return Err(format!("unsupported scheme {:?}", r.scheme()));
    }
    Ok(r.to_string())
}

/// Configured headers may follow only to the manifest's host (port
/// included) and never over an https→http downgrade.
fn headers_allowed_for(manifest_url: &str, target: &str) -> bool {
    let (Ok(m), Ok(t)) = (url::Url::parse(manifest_url), url::Url::parse(target)) else {
        return false;
    };
    let host = |u: &url::Url| {
        let h = u.host_str().unwrap_or_default().to_lowercase();
        match u.port() {
            Some(p) => format!("{h}:{p}"),
            None => h,
        }
    };
    host(&m) == host(&t) && !(m.scheme() == "https" && t.scheme() == "http")
}

pub(super) fn filename_from_url(u: &str) -> String {
    match u.rfind('/') {
        Some(i) if i + 1 < u.len() => {
            let name = &u[i + 1..];
            name.split('?').next().unwrap_or(name).to_string()
        }
        _ => u.to_string(),
    }
}

fn filetype_from_filename(name: &str) -> String {
    name.rfind('.')
        .map(|i| name[i + 1..].to_lowercase())
        .unwrap_or_default()
}

fn escape(s: &str, keep: impl Fn(u8) -> bool, space_plus: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') || keep(b) {
            out.push(b as char);
        } else if space_plus && b == b' ' {
            out.push('+');
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Go's `url.QueryEscape`.
fn query_escape(s: &str) -> String {
    escape(s, |_| false, true)
}

/// Go's `url.PathEscape` (a single path segment).
fn path_escape(s: &str) -> String {
    escape(
        s,
        |b| matches!(b, b'$' | b'&' | b'+' | b':' | b'=' | b'@'),
        false,
    )
}

#[cfg(test)]
mod tests;
