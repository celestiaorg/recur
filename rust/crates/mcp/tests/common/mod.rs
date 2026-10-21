//! A raw HTTP/1.1 client, so tests send exactly the bytes a CLI would.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

pub async fn request(port: u16, method: &str, path: &str, body: &str) -> HttpResponse {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut req =
        format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n");
    if method == "POST" {
        req.push_str("Content-Type: application/json\r\n");
    }
    if !body.is_empty() || method == "POST" {
        req.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    req.push_str("\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).await.unwrap();
    parse(&raw, method == "HEAD")
}

pub async fn post(url: &str, body: &str) -> HttpResponse {
    let rest = url.strip_prefix("http://127.0.0.1:").unwrap();
    let (port, path) = rest.split_once('/').unwrap();
    request(port.parse().unwrap(), "POST", &format!("/{path}"), body).await
}

fn parse(raw: &[u8], head: bool) -> HttpResponse {
    let text = String::from_utf8_lossy(raw).into_owned();
    let (head_part, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let mut lines = head_part.split("\r\n");
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let body = if head {
        String::new()
    } else {
        body.to_string()
    };
    HttpResponse {
        status,
        headers,
        body,
    }
}
