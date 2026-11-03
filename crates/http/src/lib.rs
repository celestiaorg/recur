//! Builds `reqwest` clients that never panic. reqwest verifies TLS against the platform's CA
//! store, and building a client fails where there is none (a minimal Linux without
//! `ca-certificates`); `reqwest::Client::new()` and `Client::default()` panic then. [`client`]
//! falls back to an empty root store instead, so plain HTTP (loopback servers, local
//! providers) still works and every HTTPS request fails with a certificate error.

use reqwest::{Client, ClientBuilder};

/// The client `configure` describes, or the same one with no trusted roots when the platform
/// has no CA store.
pub fn client(configure: impl Fn(ClientBuilder) -> ClientBuilder) -> Client {
    match configure(Client::builder()).build() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("http: no CA store ({e}); HTTPS requests will fail");
            without_roots(configure)
        }
    }
}

/// [`client`] with reqwest's defaults.
pub fn default_client() -> Client {
    client(|b| b)
}

fn without_roots(configure: impl Fn(ClientBuilder) -> ClientBuilder) -> Client {
    configure(Client::builder().tls_certs_only([]))
        .build()
        .or_else(|_| Client::builder().tls_certs_only([]).build())
        .expect("a client with an empty root store builds")
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    use super::*;

    /// One plain HTTP server answering `ok`, for a single request.
    fn serve_once() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf);
                let _ = s.write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok",
                );
            }
        });
        format!("http://{addr}/")
    }

    #[tokio::test]
    async fn without_a_ca_store_http_works_and_https_fails() {
        let c = without_roots(|b| b.timeout(Duration::from_secs(5)).no_proxy());
        let body = c
            .get(serve_once())
            .send()
            .await
            .expect("plain HTTP")
            .text()
            .await
            .expect("body");
        assert_eq!(body, "ok");
        // A TLS handshake against a listener that is not TLS fails as a request error.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("https://{}/", listener.local_addr().expect("addr"));
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let _ = s.write_all(b"not tls");
            }
        });
        assert!(c.get(url).send().await.is_err());
    }

    #[test]
    fn the_configured_client_builds() {
        let _ = client(|b| b.timeout(Duration::from_secs(1)));
        let _ = default_client();
    }
}
