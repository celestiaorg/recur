//! The `POST {base}/v1/responses` client: streamed turns with a
//! JSON fallback, the Chat Completions route for endpoints without
//! Responses, and the Settings pings.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use recur_domain::{CompatibleApi, CompatibleProvider};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tokio_util::sync::CancellationToken;

use crate::chat::{ChatStream, chat_body, decode_chat_completion};
use crate::chatgpt::chatgpt_body;
use crate::sse::{SseParser, decode_response, parse_sse};
use crate::types::{Event, Request, user_text};

const MAX_ERROR_BODY: usize = 1 << 20;
const MAX_JSON_BODY: usize = 32 << 20;
const PING_TIMEOUT: Duration = Duration::from_secs(30);

/// A failed request. `Canceled` means the turn's token fired.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClientError {
    #[error("context canceled")]
    Canceled,
    #[error("{0}")]
    Failed(String),
}

/// Adds per-request credentials, so rotating OAuth tokens are never frozen
/// into a runner.
#[async_trait::async_trait]
pub trait Authorize: Send + Sync {
    async fn authorize(&self, headers: &mut HeaderMap) -> Result<(), String>;
}

/// Talks to one OpenAI-compatible endpoint.
///
/// The stream client has no overall timeout: the turn's cancellation (Settings
/// agent turn timeout) is the ceiling, because a fixed one killed local models
/// that generate slower than it. Responses are never requested compressed: a
/// gzipped event stream only surfaces tokens when the compressor flushes, so
/// the chat would look frozen while the model is still generating.
#[derive(Clone)]
pub struct Client {
    pub base_url: String,
    pub api_key: String,
    pub extra_headers: BTreeMap<String, String>,
    http: reqwest::Client,
    /// The ChatGPT Codex subscription contract: `{base}/responses`, the
    /// stateless streaming body, and OAuth from `authorize` on every request.
    pub chatgpt: bool,
    authorize: Option<Arc<dyn Authorize>>,
    /// The wire APIs Settings → Test recorded for the endpoint.
    pub api: CompatibleApi,
    /// Set once an untested endpoint answered Chat Completions after
    /// refusing Responses, so later rounds skip the failing route.
    chat_learned: Arc<AtomicBool>,
}

impl Client {
    pub fn new(p: &CompatibleProvider) -> Self {
        Self {
            base_url: p.base_url.clone(),
            api_key: p.api_key.clone(),
            extra_headers: p.extra_headers.clone(),
            http: recur_http::default_client(),
            chatgpt: false,
            authorize: None,
            api: p.api,
            chat_learned: Arc::default(),
        }
    }

    /// Shares what an untested endpoint taught about its route with other
    /// clients for the same endpoint (one per turn).
    pub fn with_chat_learned(mut self, learned: Arc<AtomicBool>) -> Self {
        self.chat_learned = learned;
        self
    }

    /// Whether turns go to `/v1/chat/completions` instead of Responses.
    pub fn uses_chat_completions(&self) -> bool {
        if self.chatgpt {
            return false;
        }
        match self.api {
            CompatibleApi::ChatCompletions => true,
            CompatibleApi::Responses | CompatibleApi::Both => false,
            CompatibleApi::Unknown => self.chat_learned.load(Ordering::Relaxed),
        }
    }

    /// The Chat Completions URL for this client.
    pub fn chat_endpoint(&self) -> String {
        let base = self.base_url.trim().trim_end_matches('/');
        format!("{base}/v1/chat/completions")
    }

    /// Speaks the ChatGPT Codex route, authorizing each request with `auth`.
    pub fn with_chatgpt(mut self, auth: Arc<dyn Authorize>) -> Self {
        self.chatgpt = true;
        self.authorize = Some(auth);
        self
    }

    /// Uses `http` for every request (tests, shared pools).
    pub fn with_http(mut self, http: reqwest::Client) -> Self {
        self.http = http;
        self
    }

    /// The Responses URL for this client.
    pub fn endpoint(&self) -> String {
        let base = self.base_url.trim().trim_end_matches('/');
        if self.chatgpt {
            format!("{base}/responses")
        } else {
            format!("{base}/v1/responses")
        }
    }

    /// POSTs `req` with `stream: true` and calls `emit` for each parsed event.
    /// A JSON (non-SSE) answer is decoded into the same events.
    pub async fn stream(
        &self,
        cancel: &CancellationToken,
        mut req: Request,
        emit: &mut (dyn FnMut(Event) + Send),
    ) -> Result<(), ClientError> {
        req.stream = true;
        req.store = false;
        let body = serde_json::to_string(&req)
            .map_err(|e| ClientError::Failed(format!("responses encode: {e}")))?;
        self.stream_json(cancel, body, emit).await
    }

    /// POSTs a pre-built JSON body (after flag wiring) as a stream.
    pub async fn stream_json(
        &self,
        cancel: &CancellationToken,
        body: String,
        emit: &mut (dyn FnMut(Event) + Send),
    ) -> Result<(), ClientError> {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(ClientError::Canceled),
            r = self.post_stream(body, emit) => r.map_err(ClientError::Failed),
        }
    }

    async fn post_stream(
        &self,
        body: String,
        emit: &mut (dyn FnMut(Event) + Send),
    ) -> Result<(), String> {
        if self.uses_chat_completions() {
            return self.post_chat_stream(&body, emit).await;
        }
        let responses_body = if self.chatgpt {
            chatgpt_body(&body)?
        } else {
            body.clone()
        };
        let url = self.endpoint();
        let mut headers = self.headers().await?;
        headers.insert(
            reqwest::header::ACCEPT,
            HeaderValue::from_static("text/event-stream"),
        );
        let mut resp = self
            .http
            .post(&url)
            .headers(headers)
            .body(responses_body)
            .send()
            .await
            .map_err(|e| format!("responses POST: {}", url_error("Post", &url, &e)))?;
        let status = resp.status();
        if !status.is_success() {
            let raw = read_limited(&mut resp, MAX_ERROR_BODY)
                .await
                .unwrap_or_default();
            let msg = String::from_utf8_lossy(&raw).trim().to_owned();
            let msg = if msg.is_empty() {
                status_text(status)
            } else {
                msg
            };
            let err = format!("responses {}: {}", status_text(status), clip(&msg, 800));
            // An untested endpoint without the Responses route: try Chat
            // Completions before failing the turn.
            if !self.chatgpt && self.api.is_unknown() && route_missing(status) {
                return match self.post_chat_stream(&body, emit).await {
                    Ok(()) => {
                        self.chat_learned.store(true, Ordering::Relaxed);
                        Ok(())
                    }
                    Err(chat) => Err(format!("{err}; {chat}")),
                };
            }
            return Err(err);
        }
        let ct = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        if ct.contains("text/event-stream") {
            let mut parser = SseParser::default();
            loop {
                match resp.chunk().await {
                    Ok(Some(chunk)) => parser.push(&chunk, emit),
                    Ok(None) => break,
                    Err(e) => return Err(err_chain(&e)),
                }
            }
            parser.finish(emit);
            return Ok(());
        }
        let raw = read_limited(&mut resp, MAX_JSON_BODY)
            .await
            .map_err(|e| format!("responses read: {e}"))?;
        if looks_like_sse(&raw) {
            parse_sse(&raw, emit);
            return Ok(());
        }
        let evs = decode_response(&raw).map_err(|e| format!("responses decode: {e}"))?;
        for ev in evs {
            emit(ev);
        }
        Ok(())
    }

    /// Sends a Responses body over Chat Completions and emits the chunks as
    /// Responses events.
    async fn post_chat_stream(
        &self,
        responses_body: &str,
        emit: &mut (dyn FnMut(Event) + Send),
    ) -> Result<(), String> {
        let body = chat_body(responses_body)?;
        let url = self.chat_endpoint();
        let mut headers = self.headers().await?;
        headers.insert(
            reqwest::header::ACCEPT,
            HeaderValue::from_static("text/event-stream"),
        );
        let mut resp = self
            .http
            .post(&url)
            .headers(headers)
            .body(body)
            .send()
            .await
            .map_err(|e| format!("chat completions POST: {}", url_error("Post", &url, &e)))?;
        let status = resp.status();
        if !status.is_success() {
            let raw = read_limited(&mut resp, MAX_ERROR_BODY)
                .await
                .unwrap_or_default();
            let msg = String::from_utf8_lossy(&raw).trim().to_owned();
            let msg = if msg.is_empty() {
                status_text(status)
            } else {
                msg
            };
            return Err(format!(
                "chat completions {}: {}",
                status_text(status),
                clip(&msg, 800)
            ));
        }
        let ct = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let mut chat = ChatStream::default();
        if ct.contains("text/event-stream") {
            let mut parser = SseParser::default();
            loop {
                match resp.chunk().await {
                    Ok(Some(chunk)) => {
                        parser.push_payloads(&chunk, &mut |p| chat.on_payload(p, emit));
                    }
                    Ok(None) => break,
                    Err(e) => return Err(err_chain(&e)),
                }
            }
            parser.finish_payloads(&mut |p| chat.on_payload(p, emit));
            chat.finish(emit);
            return Ok(());
        }
        let raw = read_limited(&mut resp, MAX_JSON_BODY)
            .await
            .map_err(|e| format!("chat completions read: {e}"))?;
        if looks_like_sse(&raw) {
            let mut parser = SseParser::default();
            parser.push_payloads(&raw, &mut |p| chat.on_payload(p, emit));
            parser.finish_payloads(&mut |p| chat.on_payload(p, emit));
            chat.finish(emit);
            return Ok(());
        }
        for ev in
            decode_chat_completion(&raw).map_err(|e| format!("chat completions decode: {e}"))?
        {
            emit(ev);
        }
        Ok(())
    }

    /// Sends a tiny non-tool request to verify the endpoint speaks Chat
    /// Completions. A token-capped answer still passes.
    pub async fn ping_chat(&self, model: &str) -> Result<(), String> {
        let model = model.trim();
        if model.is_empty() {
            return Err("model is required".into());
        }
        let body = serde_json::json!({
            "model": model,
            "messages": [{"role": "user", "content": "ping"}],
            "max_tokens": 16,
        })
        .to_string();
        let url = self.chat_endpoint();
        let mut resp = self
            .http
            .post(&url)
            .headers(self.headers().await?)
            .timeout(PING_TIMEOUT)
            .body(body)
            .send()
            .await
            .map_err(|e| url_error("Post", &url, &e))?;
        let status = resp.status();
        let raw = read_limited(&mut resp, MAX_ERROR_BODY)
            .await
            .unwrap_or_default();
        let text = String::from_utf8_lossy(&raw).into_owned();
        if !status.is_success() {
            let msg = text.trim();
            let msg = if msg.is_empty() {
                status_text(status)
            } else {
                msg.to_owned()
            };
            return Err(format!("{}: {}", status_text(status), clip(&msg, 800)));
        }
        let evs = decode_chat_completion(&raw)
            .map_err(|_| format!("not a Chat Completions body: {}", clip(&text, 800)))?;
        match evs.iter().find(|e| !e.error_message.is_empty()) {
            Some(e) => Err(e.error_message.clone()),
            None => Ok(()),
        }
    }

    /// Sends a tiny non-tool request to verify the endpoint speaks Responses.
    /// A token-capped probe that comes back incomplete still passes.
    pub async fn ping(&self, model: &str) -> Result<(), String> {
        let model = model.trim();
        if model.is_empty() {
            return Err("model is required".into());
        }
        let req = Request {
            model: model.into(),
            input: vec![user_text("ping")],
            max_output_tokens: 16,
            ..Default::default()
        };
        let body = serde_json::to_string(&req).map_err(|e| e.to_string())?;
        let url = self.endpoint();
        let mut resp = self
            .http
            .post(&url)
            .headers(self.headers().await?)
            .timeout(PING_TIMEOUT)
            .body(body)
            .send()
            .await
            .map_err(|e| url_error("Post", &url, &e))?;
        let status = resp.status();
        let raw = read_limited(&mut resp, MAX_ERROR_BODY)
            .await
            .unwrap_or_default();
        let text = String::from_utf8_lossy(&raw).into_owned();
        if !status.is_success() {
            let msg = text.trim();
            let msg = if msg.is_empty() {
                status_text(status)
            } else {
                msg.to_owned()
            };
            return Err(format!("{}: {}", status_text(status), clip(&msg, 800)));
        }
        let evs = decode_response(&raw)
            .map_err(|_| format!("not a Responses body: {}", clip(&text, 800)))?;
        let mut terminal = false;
        for ev in &evs {
            if !ev.error_message.is_empty() {
                return Err(ev.error_message.clone());
            }
            if ev.r#type == "error" || ev.r#type == "response.failed" {
                return Err("provider failed".into());
            }
            if matches!(ev.status.as_str(), "failed" | "cancelled" | "canceled") {
                return Err(format!("responses status {}", ev.status));
            }
            if matches!(
                ev.r#type.as_str(),
                "response.completed" | "response.incomplete"
            ) || matches!(ev.status.as_str(), "completed" | "incomplete")
            {
                terminal = true;
            }
        }
        if !terminal {
            return Err(format!("not a Responses body: {}", clip(&text, 800)));
        }
        Ok(())
    }

    /// Content type, bearer key, extra headers (which may override), then
    /// the authorizer.
    async fn headers(&self) -> Result<HeaderMap, String> {
        let mut h = HeaderMap::new();
        h.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        let key = self.api_key.trim();
        if !key.is_empty() {
            let v = HeaderValue::from_str(&format!("Bearer {key}"))
                .map_err(|_| "net/http: invalid header field value for \"Authorization\"")?;
            h.insert(reqwest::header::AUTHORIZATION, v);
        }
        for (k, v) in &self.extra_headers {
            if k.is_empty() {
                continue;
            }
            let name = HeaderName::from_bytes(k.as_bytes())
                .map_err(|_| format!("net/http: invalid header field name {k:?}"))?;
            let value = HeaderValue::from_str(v)
                .map_err(|_| format!("net/http: invalid header field value for {k:?}"))?;
            h.insert(name, value);
        }
        if let Some(auth) = &self.authorize {
            auth.authorize(&mut h).await?;
        }
        Ok(h)
    }
}

/// The statuses a server answers for a route it does not have.
fn route_missing(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 404 | 405)
}

fn looks_like_sse(raw: &[u8]) -> bool {
    let s = String::from_utf8_lossy(raw);
    let s = s.trim();
    s.starts_with("event:") || s.starts_with("data:")
}

/// The status line text ("401 Unauthorized").
fn status_text(s: reqwest::StatusCode) -> String {
    match s.canonical_reason() {
        Some(r) => format!("{} {r}", s.as_u16()),
        None => s.as_u16().to_string(),
    }
}

/// Reads at most `limit` bytes of the body.
async fn read_limited(resp: &mut reqwest::Response, limit: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    while out.len() < limit {
        match resp.chunk().await {
            Ok(Some(c)) => {
                let take = (limit - out.len()).min(c.len());
                out.extend_from_slice(&c[..take]);
            }
            Ok(None) => break,
            Err(e) => return Err(err_chain(&e)),
        }
    }
    Ok(out)
}

/// A transport error as `Post "url": cause`.
pub(crate) fn url_error(method: &str, url: &str, e: &reqwest::Error) -> String {
    let cause = if e.is_timeout() {
        "context deadline exceeded (Client.Timeout exceeded while awaiting headers)".to_owned()
    } else {
        err_chain(e)
    };
    format!("{method} {url:?}: {cause}")
}

/// The innermost causes of an error joined with `: `, so markers like
/// "connection refused" stay visible to failure classification.
pub(crate) fn err_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut cur: Option<&(dyn std::error::Error + 'static)> = e.source();
    while let Some(err) = cur {
        let s = err.to_string();
        if !parts.iter().any(|p| p.contains(&s)) {
            parts.push(s);
        }
        cur = err.source();
    }
    if parts.is_empty() {
        return e.to_string();
    }
    parts.join(": ")
}

/// `s` cut to at most `n` bytes on a char boundary, with `…` when cut.
fn clip(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_owned();
    }
    format!("{}…", &s[..s.floor_char_boundary(n)])
}

#[cfg(test)]
mod client_config_tests {
    use super::*;

    #[test]
    fn stream_http_has_no_hard_timeout() {
        let c = Client::new(&CompatibleProvider {
            base_url: "http://127.0.0.1:1".into(),
            ..Default::default()
        });
        let config = format!("{:?}", c.http).to_lowercase();
        assert!(!config.contains("timeout"), "{config}");
    }
}
