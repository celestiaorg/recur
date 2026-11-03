//! LSP base-protocol framing: `Content-Length` headers around JSON-RPC
//! bodies.

use std::io;

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The bytes of one framed message.
pub fn frame(body: &[u8]) -> Vec<u8> {
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body);
    out
}

/// Writes one JSON-RPC message with its `Content-Length` header.
pub async fn write_frame<W: AsyncWrite + Unpin + ?Sized>(w: &mut W, body: &[u8]) -> io::Result<()> {
    w.write_all(&frame(body)).await?;
    w.flush().await
}

/// Reads one message body (headers stripped). EOF before a complete header
/// block is `UnexpectedEof`.
pub async fn read_frame<R: AsyncBufRead + Unpin + ?Sized>(r: &mut R) -> io::Result<Vec<u8>> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut raw = Vec::new();
        let n = r.read_until(b'\n', &mut raw).await?;
        if n == 0 || raw.last() != Some(&b'\n') {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let line = String::from_utf8_lossy(&raw);
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if line.to_lowercase().starts_with("content-length:") {
            let v = line["content-length:".len()..].trim();
            let n = v.parse::<usize>().map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("bad Content-Length: {e}"),
                )
            })?;
            content_length = Some(n);
        }
    }
    let n = content_length.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length header")
    })?;
    let mut body = vec![0u8; n];
    r.read_exact(&mut body).await?;
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn write_read_round_trip() {
        let payload = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let mut buf = Vec::new();
        write_frame(&mut buf, payload).await.unwrap();
        let got = read_frame(&mut &buf[..]).await.unwrap();
        assert_eq!(got, payload);
    }

    #[tokio::test]
    async fn header_rules() {
        let mut r: &[u8] = b"content-length: 2\r\nContent-Type: x\r\n\r\n{}";
        assert_eq!(read_frame(&mut r).await.unwrap(), b"{}");
        let mut r: &[u8] = b"X: 1\r\n\r\n{}";
        assert_eq!(
            read_frame(&mut r).await.unwrap_err().to_string(),
            "missing Content-Length header"
        );
        let mut r: &[u8] = b"Content-Length: x\r\n\r\n";
        assert!(
            read_frame(&mut r)
                .await
                .unwrap_err()
                .to_string()
                .starts_with("bad Content-Length")
        );
        let mut r: &[u8] = b"";
        assert_eq!(
            read_frame(&mut r).await.unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }
}
