//! Newline-delimited JSON-RPC over stdio without the `"jsonrpc":"2.0"`
//! field (Go `codex/rpc.go`): calls, notifications, and server→client
//! requests. One reader task dispatches every line in order; notification
//! handlers run on it synchronously, server requests each get a task.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use parking_lot::Mutex;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// A failed call. Display strings are Go's.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RpcError {
    #[error("codex rpc {code}: {message}")]
    Rpc { code: i64, message: String },
    #[error("context canceled")]
    Canceled,
    #[error("context deadline exceeded")]
    DeadlineExceeded,
    #[error("codex rpc decode {method} result: {err}")]
    Decode { method: String, err: String },
    #[error("{0}")]
    Io(String),
}

impl RpcError {
    pub fn is_canceled(&self) -> bool {
        matches!(self, Self::Canceled)
    }
}

/// Go's `context.Context` for one call chain: the turn's cancellation plus an
/// optional deadline.
#[derive(Debug, Clone, Default)]
pub struct Ctx {
    pub cancel: CancellationToken,
    pub deadline: Option<Instant>,
}

impl Ctx {
    pub fn new(cancel: CancellationToken) -> Self {
        Self {
            cancel,
            deadline: None,
        }
    }

    /// A child with a deadline no later than `d` from now.
    pub fn with_timeout(&self, d: std::time::Duration) -> Self {
        let at = Instant::now() + d;
        Self {
            cancel: self.cancel.clone(),
            deadline: Some(self.deadline.map_or(at, |cur| cur.min(at))),
        }
    }

    /// Runs `fut` until it finishes, the context is cancelled, or the
    /// deadline passes.
    pub async fn run<T>(
        &self,
        fut: impl Future<Output = Result<T, RpcError>>,
    ) -> Result<T, RpcError> {
        let deadline = async {
            match self.deadline {
                Some(d) => tokio::time::sleep_until(d).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            r = fut => r,
            _ = self.cancel.cancelled() => Err(RpcError::Canceled),
            _ = deadline => Err(RpcError::DeadlineExceeded),
        }
    }
}

/// Handles one server notification: `(method, raw params)`; params are empty
/// when absent.
pub type NotifyFn = Arc<dyn Fn(&str, &str) + Send + Sync>;

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Answers one server→client request (approvals, elicitations): `(method,
/// raw params)` to a result object or an error message.
pub type RequestFn = Arc<dyn Fn(String, String) -> BoxFuture<Result<Value, String>> + Send + Sync>;

type Response = Result<Option<Box<RawValue>>, RpcError>;

struct Inner {
    writer: tokio::sync::Mutex<Pin<Box<dyn AsyncWrite + Send>>>,
    pending: Mutex<HashMap<i64, oneshot::Sender<Response>>>,
    next_id: AtomicI64,
    on_notify: Mutex<Option<NotifyFn>>,
    on_request: Mutex<Option<RequestFn>>,
    read_err: Mutex<Option<String>>,
}

/// A bidirectional JSON-RPC connection.
#[derive(Clone)]
pub struct Conn {
    inner: Arc<Inner>,
}

#[derive(Serialize)]
struct Request<'a> {
    method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Box<RawValue>>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct WireError {
    code: Option<i64>,
    message: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Peek {
    #[serde(deserialize_with = "keep_null")]
    id: Option<Box<RawValue>>,
    #[serde(deserialize_with = "null_string")]
    method: String,
    error: Option<WireError>,
    #[serde(deserialize_with = "keep_null")]
    result: Option<Box<RawValue>>,
    #[serde(deserialize_with = "keep_null")]
    params: Option<Box<RawValue>>,
}

/// A present key keeps its raw text, `null` included (Go `json.RawMessage`).
fn keep_null<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Box<RawValue>>, D::Error> {
    Box::<RawValue>::deserialize(d).map(Some)
}

fn null_string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(d)?.unwrap_or_default())
}

impl Conn {
    /// Starts the reader task over `r`; requests are written to `w`.
    pub fn new<R, W>(r: R, w: W) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Send + 'static,
    {
        let conn = Self {
            inner: Arc::new(Inner {
                writer: tokio::sync::Mutex::new(Box::pin(w)),
                pending: Mutex::default(),
                next_id: AtomicI64::new(0),
                on_notify: Mutex::default(),
                on_request: Mutex::default(),
                read_err: Mutex::default(),
            }),
        };
        let reader = conn.clone();
        tokio::spawn(async move { reader.read_loop(r).await });
        conn
    }

    /// Registers (or with `None` clears) the notification handler. Clearing
    /// does not wait for a callback already running.
    pub fn on_notification(&self, f: Option<NotifyFn>) {
        *self.inner.on_notify.lock() = f;
    }

    pub fn on_server_request(&self, f: Option<RequestFn>) {
        *self.inner.on_request.lock() = f;
    }

    /// Sends a request and waits for its response. A `null` or absent result
    /// decodes as `R::default()`, like Go unmarshaling `null`.
    pub async fn call<P, R>(&self, method: &str, params: &P) -> Result<R, RpcError>
    where
        P: Serialize + ?Sized,
        R: DeserializeOwned + Default,
    {
        // Go keeps writing after EOF and waits on its context; failing fast
        // gives the same error the pending calls got.
        if let Some(e) = self.inner.read_err.lock().clone() {
            return Err(RpcError::Rpc {
                code: -32001,
                message: e,
            });
        }
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().insert(id, tx);
        let _forget = Forget { conn: self, id };
        self.write(&Request {
            method,
            id: Some(id),
            params: Some(raw(params)?),
        })
        .await?;
        let resp = rx.await.map_err(|_| RpcError::Rpc {
            code: -32001,
            message: "connection closed".into(),
        })??;
        let raw = match resp {
            Some(r) if r.get() != "null" => r,
            _ => return Ok(R::default()),
        };
        let decode = |e: serde_json::Error| RpcError::Decode {
            method: method.into(),
            err: e.to_string(),
        };
        let mut v: Value = serde_json::from_str(raw.get()).map_err(decode)?;
        strip_nulls(&mut v);
        serde_json::from_value(v).map_err(decode)
    }

    /// Sends a notification (no id).
    pub async fn notify<P: Serialize + ?Sized>(
        &self,
        method: &str,
        params: &P,
    ) -> Result<(), RpcError> {
        self.write(&Request {
            method,
            id: None,
            params: Some(raw(params)?),
        })
        .await
    }

    async fn write<T: Serialize>(&self, msg: &T) -> Result<(), RpcError> {
        let mut line = serde_json::to_string(msg).map_err(|e| RpcError::Io(e.to_string()))?;
        line.push('\n');
        let mut w = self.inner.writer.lock().await;
        let res = async {
            w.write_all(line.as_bytes()).await?;
            w.flush().await
        }
        .await;
        res.map_err(|e| RpcError::Io(format!("write: {}", crate::proc::go_errno(&e))))
    }

    async fn read_loop<R: AsyncRead + Unpin>(self, r: R) {
        let mut r = BufReader::with_capacity(1024 * 1024, r);
        let mut line = Vec::new();
        let err = loop {
            line.clear();
            match r.read_until(b'\n', &mut line).await {
                Ok(0) => break "EOF".to_owned(),
                Ok(_) => {}
                Err(e) => break crate::proc::go_errno(&e),
            }
            while matches!(line.last(), Some(b'\n' | b'\r')) {
                line.pop();
            }
            if !line.is_empty() {
                self.dispatch(&line);
            }
        };
        *self.inner.read_err.lock() = Some(err.clone());
        let pending: Vec<_> = self.inner.pending.lock().drain().collect();
        for (_, tx) in pending {
            let _ = tx.send(Err(RpcError::Rpc {
                code: -32001,
                message: err.clone(),
            }));
        }
    }

    fn dispatch(&self, line: &[u8]) {
        let Ok(peek) = serde_json::from_slice::<Peek>(line) else {
            return;
        };
        match (peek.id, peek.method.is_empty()) {
            // A response to our request: id, no method.
            (Some(id), true) => {
                let id = match id.get() {
                    "null" => 0,
                    raw => match serde_json::from_str::<i64>(raw) {
                        Ok(id) => id,
                        Err(_) => return,
                    },
                };
                let Some(tx) = self.inner.pending.lock().remove(&id) else {
                    return;
                };
                let resp = match peek.error {
                    Some(e) => Err(RpcError::Rpc {
                        code: e.code.unwrap_or_default(),
                        message: e.message.unwrap_or_default(),
                    }),
                    None => Ok(peek.result),
                };
                let _ = tx.send(resp);
            }
            // A server→client request: method and id.
            (Some(id), false) => {
                let handler = self.inner.on_request.lock().clone();
                let conn = self.clone();
                let params = peek.params.map(|p| p.get().to_owned()).unwrap_or_default();
                tokio::spawn(async move {
                    conn.answer(id, peek.method, params, handler).await;
                });
            }
            // A notification: method, no id.
            (None, false) => {
                let handler = self.inner.on_notify.lock().clone();
                if let Some(f) = handler {
                    let params = peek.params.as_ref().map_or("", |p| p.get());
                    f(&peek.method, params);
                }
            }
            (None, true) => {}
        }
    }

    async fn answer(
        &self,
        id: Box<RawValue>,
        method: String,
        params: String,
        handler: Option<RequestFn>,
    ) {
        #[derive(Serialize)]
        struct ErrorOut {
            code: i64,
            message: String,
        }
        #[derive(Serialize)]
        struct Answer {
            #[serde(skip_serializing_if = "Option::is_none")]
            error: Option<ErrorOut>,
            id: Value,
            #[serde(skip_serializing_if = "Option::is_none")]
            result: Option<Value>,
        }
        let result = match handler {
            Some(h) => h(method.clone(), params).await,
            None => Err(format!("no handler for server request {method}")),
        };
        let id = serde_json::from_str(id.get()).unwrap_or_else(|_| Value::String(id.get().into()));
        let answer = match result {
            Ok(v) => Answer {
                error: None,
                id,
                result: Some(match v {
                    Value::Null => Value::Object(Default::default()),
                    v => sort_keys(v),
                }),
            },
            Err(message) => Answer {
                error: Some(ErrorOut {
                    code: -32000,
                    message,
                }),
                id,
                result: None,
            },
        };
        let _ = self.write(&answer).await;
    }
}

/// Drops a call's pending slot when the caller stops waiting.
struct Forget<'a> {
    conn: &'a Conn,
    id: i64,
}

impl Drop for Forget<'_> {
    fn drop(&mut self) {
        self.conn.inner.pending.lock().remove(&self.id);
    }
}

fn raw<P: Serialize + ?Sized>(params: &P) -> Result<Box<RawValue>, RpcError> {
    let s = serde_json::to_string(params).map_err(|e| RpcError::Io(e.to_string()))?;
    RawValue::from_string(s).map_err(|e| RpcError::Io(e.to_string()))
}

/// Removes `null` object members so they decode as defaults, as Go does.
pub fn strip_nulls(v: &mut Value) {
    match v {
        Value::Object(m) => {
            m.retain(|_, v| !v.is_null());
            m.values_mut().for_each(strip_nulls);
        }
        Value::Array(a) => a.iter_mut().for_each(strip_nulls),
        _ => {}
    }
}

/// Rebuilds objects with sorted keys: Go marshals `map[string]any` sorted,
/// and the workspace's `preserve_order` would otherwise keep input order.
pub fn sort_keys(v: Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut pairs: Vec<_> = m.into_iter().collect();
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(pairs.into_iter().map(|(k, v)| (k, sort_keys(v))).collect())
        }
        Value::Array(a) => Value::Array(a.into_iter().map(sort_keys).collect()),
        v => v,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, BufReader, DuplexStream, duplex};

    use super::*;

    /// A conn plus the server's ends of its pipes.
    fn pipe() -> (Conn, BufReader<DuplexStream>, DuplexStream) {
        let (client_r, server_w) = duplex(1 << 16);
        let (server_r, client_w) = duplex(1 << 16);
        (
            Conn::new(client_r, client_w),
            BufReader::new(server_r),
            server_w,
        )
    }

    async fn next_msg(r: &mut BufReader<DuplexStream>) -> Value {
        let mut line = String::new();
        r.read_line(&mut line).await.unwrap();
        serde_json::from_str(&line).unwrap()
    }

    async fn send(w: &mut DuplexStream, v: Value) {
        let mut b = serde_json::to_vec(&v).unwrap();
        b.push(b'\n');
        w.write_all(&b).await.unwrap();
    }

    #[tokio::test]
    async fn call_and_notification() {
        let (conn, mut sr, mut sw) = pipe();
        let (ntx, mut nrx) = tokio::sync::mpsc::unbounded_channel();
        conn.on_notification(Some(Arc::new(move |m: &str, _: &str| {
            let _ = ntx.send(m.to_owned());
        })));
        let server = tokio::spawn(async move {
            let msg = next_msg(&mut sr).await;
            assert_eq!(msg["method"], "initialize");
            assert!(msg.get("jsonrpc").is_none());
            send(&mut sw, json!({"id": msg["id"], "result": {"ok": true}})).await;
            send(
                &mut sw,
                json!({"method": "turn/completed", "params": {"turn": {"id": "t1", "status": "completed"}}}),
            )
            .await;
            (sr, sw)
        });
        let out: HashMap<String, Value> = conn.call("initialize", &json!({"x": 1})).await.unwrap();
        assert_eq!(out["ok"], true);
        let m = tokio::time::timeout(Duration::from_secs(2), nrx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(m, "turn/completed");
        drop(server.await.unwrap());
    }

    #[tokio::test]
    async fn server_request_auto_answer() {
        let (conn, mut sr, mut sw) = pipe();
        conn.on_server_request(Some(Arc::new(|m: String, p: String| {
            Box::pin(async move { crate::server::auto_approve(&m, &p) }) as BoxFuture<_>
        })));
        send(
            &mut sw,
            json!({"method": "mcpServer/elicitation/request", "id": 7, "params": {"serverName": "recur"}}),
        )
        .await;
        let msg = tokio::time::timeout(Duration::from_secs(2), next_msg(&mut sr))
            .await
            .unwrap();
        assert_eq!(msg["id"], 7);
        assert_eq!(msg["result"]["action"], "accept");
        assert!(msg.get("method").is_none());
    }

    #[tokio::test]
    async fn errors_and_eof_fail_pending() {
        let (conn, mut sr, mut sw) = pipe();
        let c = conn.clone();
        let call = tokio::spawn(async move { c.call::<_, Value>("x", &json!({})).await });
        let msg = next_msg(&mut sr).await;
        send(
            &mut sw,
            json!({"id": msg["id"], "error": {"code": -1, "message": "nope"}}),
        )
        .await;
        assert_eq!(
            call.await.unwrap().unwrap_err().to_string(),
            "codex rpc -1: nope"
        );

        let c = conn.clone();
        let call = tokio::spawn(async move { c.call::<_, Value>("y", &json!({})).await });
        next_msg(&mut sr).await;
        drop(sw);
        assert_eq!(
            call.await.unwrap().unwrap_err().to_string(),
            "codex rpc -32001: EOF"
        );
        let after = conn.call::<_, Value>("z", &json!({})).await.unwrap_err();
        assert_eq!(after.to_string(), "codex rpc -32001: EOF");
    }

    #[tokio::test]
    async fn null_result_is_default_and_ctx_cancels() {
        let (conn, mut sr, mut sw) = pipe();
        let c = conn.clone();
        let call = tokio::spawn(async move {
            c.call::<_, crate::types::Thread>("thread/start", &json!({}))
                .await
        });
        let msg = next_msg(&mut sr).await;
        send(&mut sw, json!({"id": msg["id"], "result": null})).await;
        assert_eq!(
            call.await.unwrap().unwrap(),
            crate::types::Thread::default()
        );

        let ctx = Ctx::new(CancellationToken::new());
        ctx.cancel.cancel();
        let err = ctx
            .run(conn.call::<_, Value>("slow", &json!({})))
            .await
            .unwrap_err();
        assert!(err.is_canceled());
        let ctx = Ctx::default().with_timeout(Duration::from_millis(20));
        assert_eq!(
            ctx.run(conn.call::<_, Value>("slow", &json!({})))
                .await
                .unwrap_err(),
            RpcError::DeadlineExceeded
        );
        assert!(
            conn.inner.pending.lock().is_empty(),
            "abandoned calls are forgotten"
        );
    }

    #[test]
    fn key_helpers() {
        let mut v = json!({"b": null, "a": {"z": 1, "y": null}});
        strip_nulls(&mut v);
        assert_eq!(v, json!({"a": {"z": 1}}));
        let sorted = sort_keys(json!({"b": 1, "a": {"d": 2, "c": 3}}));
        assert_eq!(
            serde_json::to_string(&sorted).unwrap(),
            r#"{"a":{"c":3,"d":2},"b":1}"#
        );
    }
}
