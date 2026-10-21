use std::io::{BufRead, BufReader, Read};
use std::net::TcpListener;
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::json;
use sha2::{Digest, Sha512};

use super::*;

#[derive(Debug, Clone, Default)]
struct Seen {
    path: String,
    query: String,
    headers: BTreeMap<String, String>,
}

impl Seen {
    fn header(&self, name: &str) -> &str {
        self.headers
            .get(&name.to_lowercase())
            .map(String::as_str)
            .unwrap_or_default()
    }

    fn query_map(&self) -> BTreeMap<String, String> {
        url::form_urlencoded::parse(self.query.as_bytes())
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    }
}

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    fn json(v: serde_json::Value) -> Self {
        Self {
            status: 200,
            headers: vec![],
            body: serde_json::to_vec(&v).expect("json"),
        }
    }

    fn status(status: u16) -> Self {
        Self {
            status,
            headers: vec![],
            body: vec![],
        }
    }

    fn bytes(body: &[u8]) -> Self {
        Self {
            status: 200,
            headers: vec![],
            body: body.to_vec(),
        }
    }
}

/// A one-connection-at-a-time HTTP/1.1 server on localhost.
struct Server {
    url: String,
}

fn serve(handler: impl Fn(&Seen, &str) -> Reply + Send + Sync + 'static) -> Server {
    let ln = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", ln.local_addr().expect("addr"));
    let base = url.clone();
    std::thread::spawn(move || {
        for conn in ln.incoming() {
            let Ok(mut conn) = conn else { return };
            let mut reader = BufReader::new(conn.try_clone().expect("clone"));
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let target = line.split_whitespace().nth(1).unwrap_or("/").to_string();
            let (path, query) = target.split_once('?').unwrap_or((&target, ""));
            let mut seen = Seen {
                path: path.to_string(),
                query: query.to_string(),
                headers: BTreeMap::new(),
            };
            loop {
                let mut h = String::new();
                if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
                    break;
                }
                if let Some((k, v)) = h.split_once(':') {
                    seen.headers
                        .insert(k.trim().to_lowercase(), v.trim().to_string());
                }
            }
            let reply = handler(&seen, &base);
            let mut out = format!(
                "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n",
                reply.status,
                reply.body.len()
            );
            for (k, v) in &reply.headers {
                out.push_str(&format!("{k}: {v}\r\n"));
            }
            out.push_str("\r\n");
            let _ = conn.write_all(out.as_bytes());
            let _ = conn.write_all(&reply.body);
            let _ = conn.flush();
            let _ = reader.read(&mut [0u8; 0]);
        }
    });
    Server { url }
}

fn req() -> CheckRequest {
    CheckRequest {
        current_version: "1.0.0".into(),
        platform: "darwin".into(),
        arch: "arm64".into(),
    }
}

fn provider(url: String) -> EndpointProvider {
    EndpointProvider::new(EndpointConfig {
        url,
        ..EndpointConfig::default()
    })
    .expect("provider")
}

#[test]
fn requires_url() {
    assert!(EndpointProvider::new(EndpointConfig::default()).is_err());
    assert!(
        EndpointProvider::new(EndpointConfig {
            url: "   ".into(),
            ..EndpointConfig::default()
        })
        .is_err()
    );
}

#[tokio::test]
async fn dynamic_query_params() {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let srv = serve({
        let seen = Arc::clone(&seen);
        move |s, _| {
            *seen.lock() = s.clone();
            Reply::json(json!({
                "version": "2.0.0",
                "channel": "stable",
                "name": "Big Release",
                "notes": "## Changes",
                "artifacts": [{"url": "/dl/app-darwin-arm64.zip", "platform": "darwin", "arch": "arm64", "size": 42}],
            }))
        }
    });
    let p = EndpointProvider::new(EndpointConfig {
        url: format!("{}/manifest", srv.url),
        channel: "stable".into(),
        headers: BTreeMap::from([("Authorization".to_string(), "License abc-123".to_string())]),
        ..EndpointConfig::default()
    })
    .expect("provider");
    let rel = p.check(&req()).await.expect("check").expect("release");
    let s = seen.lock().clone();
    let q = s.query_map();
    for (k, v) in [
        ("platform", "darwin"),
        ("arch", "arm64"),
        ("version", "1.0.0"),
        ("channel", "stable"),
    ] {
        assert_eq!(q.get(k).map(String::as_str), Some(v), "query {k}");
    }
    assert_eq!(s.header("Authorization"), "License abc-123");
    assert_eq!(s.header("Accept"), "application/json");
    assert_eq!(
        (rel.version.as_str(), rel.name.as_str(), rel.notes.as_str()),
        ("2.0.0", "Big Release", "## Changes")
    );
    assert_eq!(rel.artifact.filename, "app-darwin-arm64.zip");
    assert_eq!(rel.artifact.filetype, "zip");
    assert_eq!(rel.artifact.size, 42);
    assert_eq!(
        rel.metadata.get(ARTIFACT_URL_KEY).and_then(|v| v.as_str()),
        Some(format!("{}/dl/app-darwin-arm64.zip", srv.url).as_str())
    );
}

#[tokio::test]
async fn placeholders_consume_params() {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let srv = serve({
        let seen = Arc::clone(&seen);
        move |s, _| {
            *seen.lock() = s.clone();
            Reply::json(json!({"version": "2.0.0", "artifacts": [{"url": "app.zip"}]}))
        }
    });
    let p = EndpointProvider::new(EndpointConfig {
        url: format!(
            "{}/updates/{{{{platform}}}}/{{{{arch}}}}/{{{{channel}}}}.json",
            srv.url
        ),
        channel: "beta".into(),
        ..EndpointConfig::default()
    })
    .expect("provider");
    let rel = p.check(&req()).await.expect("check").expect("release");
    let s = seen.lock().clone();
    assert_eq!(s.path, "/updates/darwin/arm64/beta.json");
    assert_eq!(
        s.query, "version=1.0.0",
        "only unconsumed params travel as query"
    );
    assert_eq!(
        rel.metadata.get(ARTIFACT_URL_KEY).and_then(|v| v.as_str()),
        Some(format!("{}/updates/darwin/arm64/app.zip", srv.url).as_str())
    );
}

#[tokio::test]
async fn up_to_date_versions() {
    for version in ["1.0.0", "0.9.9", "v1.0.0"] {
        let srv = serve(move |_, _| {
            Reply::json(json!({"version": version, "artifacts": [{"url": "app.zip"}]}))
        });
        assert_eq!(
            provider(srv.url).check(&req()).await.expect("check"),
            None,
            "{version}"
        );
    }
}

#[tokio::test]
async fn no_content_and_not_found() {
    for status in [204, 404] {
        let srv = serve(move |_, _| Reply::status(status));
        assert_eq!(
            provider(srv.url).check(&req()).await.expect("check"),
            None,
            "{status}"
        );
    }
}

#[tokio::test]
async fn server_error() {
    let srv = serve(|_, _| Reply::status(500));
    let e = provider(srv.url).check(&req()).await.expect_err("500");
    assert!(e.to_string().contains("HTTP 500"), "{e}");
}

#[tokio::test]
async fn channel_mismatch() {
    let srv = serve(|_, _| {
        Reply::json(
            json!({"version": "2.0.0", "channel": "beta", "artifacts": [{"url": "app.zip"}]}),
        )
    });
    let p = EndpointProvider::new(EndpointConfig {
        url: srv.url,
        channel: "stable".into(),
        ..EndpointConfig::default()
    })
    .expect("provider");
    assert_eq!(p.check(&req()).await.expect("check"), None);
}

#[tokio::test]
async fn schema_version_too_new() {
    let srv = serve(|_, _| {
        Reply::json(
            json!({"schemaVersion": 2, "version": "2.0.0", "artifacts": [{"url": "app.zip"}]}),
        )
    });
    assert!(provider(srv.url).check(&req()).await.is_err());
}

#[tokio::test]
async fn artifact_selection_aliases() {
    let srv = serve(|_, _| {
        Reply::json(json!({"version": "2.0.0", "artifacts": [
            {"url": "win.exe", "platform": "win", "arch": "x86_64"},
            {"url": "mac.zip", "platform": "macos", "arch": "aarch64"},
            {"url": "linux.tgz", "platform": "linux", "arch": "x86_64"},
        ]}))
    });
    let rel = provider(srv.url)
        .check(&req())
        .await
        .expect("check")
        .expect("release");
    assert_eq!(rel.artifact.filename, "mac.zip");
}

#[tokio::test]
async fn wildcard_artifact() {
    let srv = serve(|_, _| {
        Reply::json(json!({"version": "2.0.0", "artifacts": [{"url": "universal.zip"}]}))
    });
    let rel = provider(srv.url)
        .check(&req())
        .await
        .expect("check")
        .expect("release");
    assert_eq!(rel.artifact.filename, "universal.zip");
}

#[tokio::test]
async fn no_matching_artifact() {
    let srv = serve(|_, _| {
        Reply::json(
            json!({"version": "2.0.0", "artifacts": [{"url": "win.exe", "platform": "windows", "arch": "amd64"}]}),
        )
    });
    assert!(provider(srv.url).check(&req()).await.is_err());
}

#[tokio::test]
async fn verification_mapping() {
    let payload = b"artifact bytes";
    let digest: [u8; 64] = Sha512::digest(payload).into();
    let key = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
    let sig = crate::updater::tests::sign_ed25519ph(&key, &digest);
    let (digest_b64, sig_b64) = (
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(digest),
        base64::engine::general_purpose::STANDARD.encode(&sig),
    );
    let srv = serve(move |_, _| {
        Reply::json(json!({"version": "2.0.0", "artifacts": [{
            "url": "app.zip",
            "digestAlgo": "SHA512",
            "digest": digest_b64,
            "signatureAlgo": "ed25519ph",
            "signature": sig_b64,
        }]}))
    });
    let rel = provider(srv.url)
        .check(&req())
        .await
        .expect("check")
        .expect("release");
    let v = rel.verification.expect("verification");
    assert_eq!(v.digest_algo, "sha512");
    assert_eq!(v.digest, digest);
    assert_eq!(v.signature_algo, "ed25519ph");
    assert_eq!(v.signature, sig);
    crate::updater::verify::run_verification(&digest, &v, key.verifying_key().as_bytes())
        .expect("mapped signature verifies under the updater's ed25519ph rules");
}

#[tokio::test]
async fn signature_without_algo_fails() {
    let srv = serve(|_, _| {
        Reply::json(
            json!({"version": "2.0.0", "artifacts": [{"url": "app.zip", "signature": "c29tZSBzaWduYXR1cmU="}]}),
        )
    });
    assert!(provider(srv.url).check(&req()).await.is_err());
}

#[tokio::test]
async fn undecodable_verification_fails() {
    let srv = serve(|_, _| {
        Reply::json(
            json!({"version": "2.0.0", "artifacts": [{"url": "app.zip", "digestAlgo": "sha512", "digest": "!!! not base64 !!!"}]}),
        )
    });
    assert!(provider(srv.url).check(&req()).await.is_err());
}

async fn download(p: &EndpointProvider, rel: &Release) -> (Vec<u8>, i64, i64, usize) {
    let mut buf = Vec::new();
    let (mut last_w, mut last_t, mut calls) = (0, 0, 0);
    let mut progress = |w: i64, t: i64| {
        last_w = w;
        last_t = t;
        calls += 1;
    };
    p.download(rel, &mut buf, &mut progress)
        .await
        .expect("download");
    (buf, last_w, last_t, calls)
}

#[tokio::test]
async fn download_streams_with_progress() {
    let payload: Vec<u8> = b"wails".repeat(100_000);
    let auth = Arc::new(Mutex::new(String::new()));
    let srv = serve({
        let (payload, auth) = (payload.clone(), Arc::clone(&auth));
        move |s, _| {
            if s.path == "/manifest" {
                Reply::json(
                    json!({"version": "2.0.0", "artifacts": [{"url": "/dl/app.zip", "size": payload.len()}]}),
                )
            } else {
                *auth.lock() = s.header("Authorization").to_string();
                Reply::bytes(&payload)
            }
        }
    });
    let p = EndpointProvider::new(EndpointConfig {
        url: format!("{}/manifest", srv.url),
        headers: BTreeMap::from([("Authorization".to_string(), "License k".to_string())]),
        ..EndpointConfig::default()
    })
    .expect("provider");
    let rel = p.check(&req()).await.expect("check").expect("release");
    let (buf, w, t, calls) = download(&p, &rel).await;
    assert_eq!(buf, payload);
    assert!(calls > 0);
    assert_eq!((w, t), (payload.len() as i64, payload.len() as i64));
    assert_eq!(
        *auth.lock(),
        "License k",
        "same-host download carries headers"
    );
}

#[tokio::test]
async fn cross_origin_download_drops_auth() {
    let cdn_auth = Arc::new(Mutex::new(None::<String>));
    let cdn = serve({
        let cdn_auth = Arc::clone(&cdn_auth);
        move |s, _| {
            *cdn_auth.lock() = Some(s.header("Authorization").to_string());
            Reply::bytes(b"bytes")
        }
    });
    let cdn_url = cdn.url.clone();
    let srv = serve(move |_, _| {
        Reply::json(
            json!({"version": "2.0.0", "artifacts": [{"url": format!("{cdn_url}/app.zip")}]}),
        )
    });
    let p = EndpointProvider::new(EndpointConfig {
        url: srv.url,
        headers: BTreeMap::from([("Authorization".to_string(), "License secret".to_string())]),
        ..EndpointConfig::default()
    })
    .expect("provider");
    let rel = p.check(&req()).await.expect("check").expect("release");
    download(&p, &rel).await;
    assert_eq!(
        cdn_auth.lock().as_deref(),
        Some(""),
        "CDN hit without Authorization"
    );
}

#[tokio::test]
async fn redirect_strips_auth() {
    let cdn_auth = Arc::new(Mutex::new(None::<String>));
    let cdn = serve({
        let cdn_auth = Arc::clone(&cdn_auth);
        move |s, _| {
            *cdn_auth.lock() = Some(s.header("Authorization").to_string());
            Reply::bytes(b"bytes")
        }
    });
    let cdn_url = cdn.url.clone();
    let srv = serve(move |s, _| {
        if s.path == "/manifest" {
            Reply::json(json!({"version": "2.0.0", "artifacts": [{"url": "/dl/app.zip"}]}))
        } else {
            Reply {
                status: 303,
                headers: vec![("Location".into(), format!("{cdn_url}/app.zip"))],
                body: vec![],
            }
        }
    });
    let p = EndpointProvider::new(EndpointConfig {
        url: format!("{}/manifest", srv.url),
        headers: BTreeMap::from([("Authorization".to_string(), "License secret".to_string())]),
        ..EndpointConfig::default()
    })
    .expect("provider");
    let rel = p.check(&req()).await.expect("check").expect("release");
    let (buf, ..) = download(&p, &rel).await;
    assert_eq!(buf, b"bytes");
    assert_eq!(
        cdn_auth.lock().as_deref(),
        Some(""),
        "Authorization leaked on cross-origin redirect"
    );
}

#[tokio::test]
async fn response_header_timeout() {
    let ln = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", ln.local_addr().expect("addr"));
    std::thread::spawn(move || {
        let _held: Vec<_> = ln.incoming().take(1).collect();
        std::thread::sleep(Duration::from_secs(5));
    });
    let p = EndpointProvider::new(EndpointConfig {
        url,
        response_header_timeout: Some(Duration::from_millis(200)),
        ..EndpointConfig::default()
    })
    .expect("provider");
    let e = p.check(&req()).await.expect_err("timeout");
    assert!(
        e.to_string().contains("timeout awaiting response headers"),
        "{e}"
    );
}

#[test]
fn build_url_preserves_existing_query() {
    let p = provider("https://example.com/updates?key=abc".into());
    let got = url::Url::parse(&p.build_url(&req()).expect("url")).expect("parse");
    let q: BTreeMap<String, String> = got
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    assert_eq!(q.get("key").map(String::as_str), Some("abc"));
    assert_eq!(q.get("platform").map(String::as_str), Some("darwin"));
    assert_eq!(q.get("arch").map(String::as_str), Some("arm64"));
    assert_eq!(q.get("version").map(String::as_str), Some("1.0.0"));
    assert!(
        !q.contains_key("channel"),
        "channel sent despite not being configured"
    );
    assert_eq!(
        got.query(),
        Some("arch=arm64&key=abc&platform=darwin&version=1.0.0"),
        "Go sorts keys"
    );
}

#[test]
fn resolve_rejects_non_http() {
    assert!(resolve_url("https://example.com/m.json", "file:///etc/passwd").is_err());
    assert_eq!(
        resolve_url("https://example.com/a/m.json", "app.zip").as_deref(),
        Ok("https://example.com/a/app.zip")
    );
}

#[test]
fn headers_allowed() {
    for (manifest, target, want) in [
        (
            "https://u.example.com/m.json",
            "https://u.example.com/app.zip",
            true,
        ),
        (
            "https://u.example.com/m.json",
            "http://u.example.com/app.zip",
            false,
        ),
        (
            "http://u.example.com/m.json",
            "https://u.example.com/app.zip",
            true,
        ),
        (
            "http://u.example.com/m.json",
            "http://u.example.com/app.zip",
            true,
        ),
        (
            "https://u.example.com/m.json",
            "https://cdn.example.com/app.zip",
            false,
        ),
        (
            "https://u.example.com:8443/m.json",
            "https://u.example.com/app.zip",
            false,
        ),
    ] {
        assert_eq!(
            headers_allowed_for(manifest, target),
            want,
            "{manifest} -> {target}"
        );
    }
}

#[test]
fn escapes_like_go() {
    assert_eq!(query_escape("a b&c=d~*"), "a+b%26c%3Dd~%2A");
    assert_eq!(path_escape("a b/c:d"), "a%20b%2Fc:d");
    assert_eq!(filename_from_url("https://x/dl/app.zip?sig=1"), "app.zip");
    assert_eq!(filetype_from_filename("App.TAR.GZ"), "gz");
    assert_eq!(decode_b64("AQI"), Some(vec![1, 2]));
    assert_eq!(decode_b64("AQI="), Some(vec![1, 2]));
    assert_eq!(decode_b64("!!"), None);
}
