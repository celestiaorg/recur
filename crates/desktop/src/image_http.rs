//! GPUI's `HttpClient` over `recur-http`, so `img(url)` can fetch remote images (GitHub
//! avatars). GPUI polls the future on its own executor; reqwest needs tokio, so each request
//! runs on the app runtime.

use futures::future::BoxFuture;
use gpui_kit::http_client::http::HeaderValue;
use gpui_kit::http_client::{AsyncBody, HttpClient, Request, Response, Url};

pub struct ImageHttp {
    client: reqwest::Client,
    rt: tokio::runtime::Handle,
}

impl ImageHttp {
    pub fn new(rt: tokio::runtime::Handle) -> Self {
        Self {
            client: recur_http::default_client(),
            rt,
        }
    }
}

impl HttpClient for ImageHttp {
    fn user_agent(&self) -> Option<&HeaderValue> {
        None
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }

    // simplify: sends no request body; image loads are bodiless GETs. Forward `AsyncBody` if
    // anything else starts using `cx.http_client()`.
    fn send(
        &self,
        req: Request<AsyncBody>,
    ) -> BoxFuture<'static, anyhow::Result<Response<AsyncBody>>> {
        let (parts, _) = req.into_parts();
        let send = self
            .client
            .request(parts.method, parts.uri.to_string())
            .headers(parts.headers)
            .send();
        let task = self.rt.spawn(async move {
            let resp = send.await?;
            let status = resp.status();
            let body = resp.bytes().await?;
            anyhow::Ok(
                Response::builder()
                    .status(status)
                    .body(AsyncBody::from(body))?,
            )
        });
        Box::pin(async move { task.await? })
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use futures::AsyncReadExt as _;

    use super::*;

    #[test]
    fn fetches_off_the_tokio_runtime() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}/a.png", listener.local_addr().expect("addr"));
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let _ = s.read(&mut [0u8; 1024]);
                let _ = s.write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-length: 3\r\nconnection: close\r\n\r\npng",
                );
            }
        });
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let client = ImageHttp::new(rt.handle().clone());
        let mut resp = futures::executor::block_on(client.get(&url, ().into(), true)).expect("get");
        let mut body = Vec::new();
        futures::executor::block_on(resp.body_mut().read_to_end(&mut body)).expect("body");
        assert!(resp.status().is_success());
        assert_eq!(body, b"png");
    }
}
