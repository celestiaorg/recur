//! The Connect protocol with JSON payloads, as the bridge speaks it (Go
//! `cursor/connect.go`):
//!
//! - unary: `POST <base>/<service>/<method>`, `Content-Type: application/json`
//! - server stream: same path, `application/connect+json`, and both bodies
//!   are sequences of 5-byte-prefixed frames
//!
//! Hand-rolled rather than a Connect dependency: two call shapes over a
//! loopback socket, against a schema Cursor does not publish.

use std::error::Error as _;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::gojson::{null_default, trim, truncate_bytes};

const CONNECT_PROTOCOL_VERSION: &str = "1";

/// Marks the final frame of a server stream; its payload carries trailers
/// and, on failure, an `error`.
pub(crate) const END_STREAM_FLAG: u8 = 0x02;

/// Bounds one non-streaming RPC. Streams are not bounded here: a run takes as
/// long as the task does. SteerRun is unbounded too (it stays pending until
/// delivery or rejection).
pub(crate) const UNARY_TIMEOUT: Duration = Duration::from_secs(60);

/// A Connect-level failure returned by the bridge. The code tells a config
/// problem (`unauthenticated`) from a blip (`unavailable`). Opaque codes like
/// `[internal] internal error` carry nothing useful in `message`; `details`
/// and `raw` make them diagnosable and go to the bridge log, which the Node
/// bridge never writes itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConnectError {
    pub code: String,
    pub message: String,
    /// The raw `details` JSON; empty when absent.
    pub details: String,
    /// A truncated copy of the Connect error JSON, for the bridge log only.
    pub raw: String,
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.code.is_empty() {
            return f.write_str(&self.message);
        }
        write!(f, "cursor bridge: [{}] {}", self.code, self.message)
    }
}

impl ConnectError {
    /// An auth failure: a settings problem, not a retryable agent error.
    pub fn unauthenticated(&self) -> bool {
        self.code == "unauthenticated" || self.code == "permission_denied"
    }

    /// A missing RPC (SteerRun on an unpatched bridge).
    pub fn unimplemented(&self) -> bool {
        self.code == "unimplemented"
    }

    /// A provider or bridge blip worth retrying from the checkpoint (D56).
    /// `internal` is the opaque "[internal] internal error", almost always a
    /// Cursor-side fault.
    pub fn transient(&self) -> bool {
        matches!(
            self.code.as_str(),
            "internal" | "unavailable" | "resource_exhausted" | "deadline_exceeded" | "aborted"
        )
    }

    /// Send refused because the agent store still has a non-terminal
    /// activeRunId (a dropped Send stream, or a SQLite leftover).
    pub fn active_run_conflict(&self) -> bool {
        self.message
            .to_lowercase()
            .contains("already has active run")
    }
}

/// A Connect JSON error body, or `None` when it is not a usable one.
pub(crate) fn parse_connect_error(raw: &[u8]) -> Option<ConnectError> {
    #[derive(Deserialize)]
    struct Body {
        #[serde(default, deserialize_with = "null_default")]
        code: String,
        #[serde(default, deserialize_with = "null_default")]
        message: String,
        #[serde(default)]
        details: Option<Box<serde_json::value::RawValue>>,
    }
    let b: Body = serde_json::from_slice(raw).ok()?;
    if b.code.is_empty() && b.message.is_empty() {
        return None;
    }
    Some(ConnectError {
        code: b.code,
        message: b.message,
        details: b.details.map(|d| d.get().to_owned()).unwrap_or_default(),
        raw: truncate_bytes(trim(&String::from_utf8_lossy(raw)), 800),
    })
}

/// Any failure of a bridge call. `Display` is the full wrapped message, the
/// way Go's `%w` chains read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    message: String,
    connect: Option<Box<ConnectError>>,
    kind: Kind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Other,
    Canceled,
    /// The loopback socket refused or reset: the bridge process is gone.
    Unreachable,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

impl From<ConnectError> for Error {
    fn from(c: ConnectError) -> Self {
        Self {
            message: c.to_string(),
            connect: Some(Box::new(c)),
            kind: Kind::Other,
        }
    }
}

impl Error {
    pub fn msg(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            connect: None,
            kind: Kind::Other,
        }
    }

    /// The turn's context was cancelled (Go `context.Canceled`).
    pub fn canceled() -> Self {
        Self {
            message: "context canceled".into(),
            connect: None,
            kind: Kind::Canceled,
        }
    }

    /// Prefixes the message, keeping what the error is (Go `%w`).
    pub fn wrap(mut self, prefix: impl std::fmt::Display) -> Self {
        self.message = format!("{prefix}: {}", self.message);
        self
    }

    /// Appends to the message, keeping what the error is.
    pub fn suffix(mut self, suffix: impl std::fmt::Display) -> Self {
        self.message = format!("{}{suffix}", self.message);
        self
    }

    /// The Connect error this wraps, if any (Go `errors.As(err, &*Error)`).
    pub fn connect(&self) -> Option<&ConnectError> {
        self.connect.as_deref()
    }

    pub fn is_canceled(&self) -> bool {
        self.kind == Kind::Canceled
    }

    /// Whether the local bridge process is gone or its port is dead (Go
    /// `BridgeUnreachable`): transport refused/reset, not Connect codes.
    pub fn bridge_unreachable(&self) -> bool {
        if self.kind == Kind::Unreachable {
            return true;
        }
        let msg = self.message.to_lowercase();
        [
            "connection refused",
            "connection reset",
            "broken pipe",
            "connect: connection refused",
        ]
        .iter()
        .any(|m| msg.contains(m))
    }

    /// Go `ActiveRunConflict(err)`.
    pub fn active_run_conflict(&self) -> bool {
        match self.connect.as_deref() {
            Some(c) => c.active_run_conflict(),
            None => self
                .message
                .to_lowercase()
                .contains("already has active run"),
        }
    }

    fn transport(prefix: String, e: &reqwest::Error) -> Self {
        let mut message = format!("{prefix}: {e}");
        let mut unreachable = e.is_connect();
        let mut src = e.source();
        while let Some(s) = src {
            message.push_str(&format!(": {s}"));
            if let Some(io) = s.downcast_ref::<std::io::Error>() {
                use std::io::ErrorKind as K;
                unreachable |= matches!(
                    io.kind(),
                    K::ConnectionRefused | K::ConnectionReset | K::BrokenPipe
                );
            }
            src = s.source();
        }
        Self {
            message,
            connect: None,
            kind: if unreachable {
                Kind::Unreachable
            } else {
                Kind::Other
            },
        }
    }
}

type LogFn = Arc<dyn Fn(&str) + Send + Sync>;

/// A Connect/JSON client for one bridge instance. Safe for concurrent use.
#[derive(Clone)]
pub struct Client {
    base_url: String,
    token: String,
    http: reqwest::Client,
    /// Receives diagnostic lines (Connect faults); the bridge wires it to its
    /// stderr log so opaque `[internal]` errors land in cursor-bridge.log.
    log: Option<LogFn>,
    pub(crate) unary_timeout: Duration,
}

impl Client {
    pub(crate) fn new(base_url: &str, token: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            token: token.to_owned(),
            // No global timeout: streams outlive it. Loopback never goes
            // through a proxy (Go's ProxyFromEnvironment skips 127.0.0.1).
            http: reqwest::Client::builder()
                .no_proxy()
                .build()
                .unwrap_or_default(),
            log: None,
            unary_timeout: UNARY_TIMEOUT,
        }
    }

    pub(crate) fn with_log(mut self, log: LogFn) -> Self {
        self.log = Some(log);
        self
    }

    pub(crate) fn note(&self, line: &str) {
        if let Some(log) = &self.log
            && !line.is_empty()
        {
            log(line);
        }
    }

    fn url(&self, service: &str, method: &str) -> String {
        format!("{}/{service}/{method}", self.base_url)
    }

    fn request(
        &self,
        service: &str,
        method: &str,
        body: Vec<u8>,
        streaming: bool,
    ) -> reqwest::RequestBuilder {
        let ct = if streaming {
            "application/connect+json"
        } else {
            "application/json"
        };
        let mut req = self
            .http
            .post(self.url(service, method))
            .header("Content-Type", ct)
            .header("Accept", ct)
            .header("Connect-Protocol-Version", CONNECT_PROTOCOL_VERSION)
            .body(body);
        if !self.token.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.token));
        }
        req
    }

    /// One request/response RPC bounded by the unary timeout. Returns the raw
    /// response body.
    pub(crate) async fn unary<I: Serialize>(
        &self,
        cancel: &CancellationToken,
        service: &str,
        method: &str,
        input: &I,
    ) -> Result<Vec<u8>, Error> {
        match tokio::time::timeout(
            self.unary_timeout,
            self.unary_ctx(cancel, service, method, input),
        )
        .await
        {
            Ok(r) => r,
            Err(_) => Err(Error::msg(format!(
                "cursor bridge: {service}/{method}: context deadline exceeded"
            ))),
        }
    }

    /// `unary` without the extra deadline: the caller's token is the only
    /// cancellation (SteerRun waits for terminal acknowledgement).
    pub(crate) async fn unary_ctx<I: Serialize>(
        &self,
        cancel: &CancellationToken,
        service: &str,
        method: &str,
        input: &I,
    ) -> Result<Vec<u8>, Error> {
        let body = recur_domain::json::to_string(input)
            .map_err(|e| Error::msg(format!("cursor bridge: marshal {service}/{method}: {e}")))?;
        let call = async {
            let resp = self
                .request(service, method, body.into_bytes(), false)
                .send()
                .await
                .map_err(|e| Error::transport(format!("cursor bridge: {service}/{method}"), &e))?;
            let status = resp.status();
            let raw = read_limited(resp, 64 << 20).await.map_err(|e| {
                Error::transport(format!("cursor bridge: read {service}/{method}"), &e)
            })?;
            // Connect reports application errors as non-2xx with a JSON
            // {code, message[, details]} body the bridge never echoes.
            if !status.is_success() {
                if let Some(e) = parse_connect_error(&raw) {
                    self.note(&format!(
                        "recur: connect {service}/{method} → http {} {}",
                        status.as_u16(),
                        e.raw
                    ));
                    return Err(e.into());
                }
                return Err(Error::msg(format!(
                    "cursor bridge: {service}/{method}: http {}: {}",
                    status.as_u16(),
                    truncate_bytes(&String::from_utf8_lossy(&raw), 400)
                )));
            }
            Ok(raw)
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(Error::canceled()),
            r = call => r,
        }
    }

    /// A server-streaming RPC: `on_msg` sees each data frame. Returns at the
    /// end-stream frame, when the server closes, when `on_msg` fails, or on
    /// cancel.
    pub(crate) async fn stream<I: Serialize>(
        &self,
        cancel: &CancellationToken,
        service: &str,
        method: &str,
        input: &I,
        mut on_msg: impl FnMut(&[u8]) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let payload = recur_domain::json::to_string(input)
            .map_err(|e| Error::msg(format!("cursor bridge: marshal {service}/{method}: {e}")))?;
        let body = encode_frame(0, payload.as_bytes());
        let send = self.request(service, method, body, true).send();
        let mut resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(Error::canceled()),
            r = send => r.map_err(|e| Error::transport(format!("cursor bridge: {service}/{method}"), &e))?,
        };
        let status = resp.status();
        if !status.is_success() {
            let raw = read_limited(resp, 1 << 20).await.unwrap_or_default();
            if let Some(e) = parse_connect_error(&raw) {
                self.note(&format!(
                    "recur: connect {service}/{method} → http {} {}",
                    status.as_u16(),
                    e.raw
                ));
                return Err(e.into());
            }
            return Err(Error::msg(format!(
                "cursor bridge: {service}/{method}: http {}: {}",
                status.as_u16(),
                truncate_bytes(&String::from_utf8_lossy(&raw), 400)
            )));
        }

        let mut buf: Vec<u8> = Vec::new();
        let mut eof = false;
        loop {
            if cancel.is_cancelled() {
                return Err(Error::canceled());
            }
            // Header: [flags][4-byte big-endian length].
            while buf.len() < 5 && !eof {
                eof = !read_more(&mut resp, &mut buf, cancel, service, method, "header").await?;
            }
            if buf.len() < 5 {
                // Closed without an explicit end frame: a clean end; the
                // caller decides whether a missing result is an error.
                return Ok(());
            }
            let flags = buf[0];
            let size = u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]) as usize;
            if size > 64 << 20 {
                return Err(Error::msg(format!(
                    "cursor bridge: {service}/{method}: frame too large ({size} bytes)"
                )));
            }
            while buf.len() < 5 + size && !eof {
                eof = !read_more(&mut resp, &mut buf, cancel, service, method, "body").await?;
            }
            if buf.len() < 5 + size {
                return Err(Error::msg(format!(
                    "cursor bridge: {service}/{method}: read frame body: unexpected EOF"
                )));
            }
            let frame: Vec<u8> = buf.drain(..5 + size).skip(5).collect();
            if flags & END_STREAM_FLAG != 0 {
                let res = decode_end_stream(&frame);
                if let Err(e) = &res
                    && let Some(c) = e.connect()
                {
                    self.note(&format!(
                        "recur: connect {service}/{method} end-stream {}",
                        c.raw
                    ));
                }
                return res;
            }
            on_msg(&frame)?;
        }
    }
}

/// Reads the next body chunk into `buf`; false at EOF.
async fn read_more(
    resp: &mut reqwest::Response,
    buf: &mut Vec<u8>,
    cancel: &CancellationToken,
    service: &str,
    method: &str,
    what: &str,
) -> Result<bool, Error> {
    let chunk = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(Error::canceled()),
        c = resp.chunk() => c,
    };
    match chunk {
        Ok(Some(b)) => {
            buf.extend_from_slice(&b);
            Ok(true)
        }
        Ok(None) => Ok(false),
        Err(e) => {
            if cancel.is_cancelled() {
                return Err(Error::canceled());
            }
            Err(Error::transport(
                format!("cursor bridge: {service}/{method}: read frame {what}"),
                &e,
            ))
        }
    }
}

async fn read_limited(
    mut resp: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, reqwest::Error> {
    let mut out = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        let room = limit.saturating_sub(out.len());
        out.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

/// The terminal frame as an error, or `Ok` on success. A malformed trailer
/// is not worth failing an otherwise complete run.
pub(crate) fn decode_end_stream(frame: &[u8]) -> Result<(), Error> {
    if trim(&String::from_utf8_lossy(frame)).is_empty() {
        return Ok(());
    }
    #[derive(Deserialize)]
    struct End {
        #[serde(default)]
        error: Option<Box<serde_json::value::RawValue>>,
    }
    let Ok(end) = serde_json::from_slice::<End>(frame) else {
        return Ok(());
    };
    let Some(err) = end.error else {
        return Ok(());
    };
    match parse_connect_error(err.get().as_bytes()) {
        Some(e) => Err(e.into()),
        None => Ok(()),
    }
}

/// `[flags][4-byte big-endian length][payload]`.
pub(crate) fn encode_frame(flags: u8, payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(5 + payload.len());
    buf.push(flags);
    buf.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    buf.extend_from_slice(payload);
    buf
}

/// A Connect frame for tests and fakes (Go `EncodeConnectFrame`).
pub fn encode_connect_frame(end_stream: bool, payload: &[u8]) -> Vec<u8> {
    encode_frame(if end_stream { END_STREAM_FLAG } else { 0 }, payload)
}

#[cfg(test)]
mod tests;
