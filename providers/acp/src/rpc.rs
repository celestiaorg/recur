//! Newline-delimited JSON-RPC 2.0 over the agent's stdio: calls,
//! notifications, and agent→client requests. One reader task dispatches every
//! line in order; notification handlers run on it synchronously, requests each
//! get a task. Requests without a handler answer method-not-found (the client
//! advertises no `fs/*` or `terminal/*` capability).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;

pub const METHOD_NOT_FOUND: i64 = -32601;
/// ACP `auth_required`.
pub const AUTH_REQUIRED: i64 = -32000;

/// A failed call: the agent's error, or `code` 0 when the pipe closed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcError {
    fn closed() -> Self {
        Self {
            code: 0,
            message: "agent closed the connection".into(),
        }
    }
}

pub type NotifyFn = Arc<dyn Fn(&str, &Value) + Send + Sync>;
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;
/// Answers one agent request: `(method, params)` to a result or an error.
pub type RequestFn = Arc<dyn Fn(String, Value) -> BoxFuture<Result<Value, RpcError>> + Send + Sync>;

/// Calls awaiting an answer; `None` once the agent closed stdout.
type Pending = Option<HashMap<i64, oneshot::Sender<Result<Value, RpcError>>>>;

struct Inner {
    writer: tokio::sync::Mutex<Pin<Box<dyn AsyncWrite + Send>>>,
    pending: Mutex<Pending>,
    next_id: AtomicI64,
    on_notify: Mutex<Option<NotifyFn>>,
    on_request: Mutex<Option<RequestFn>>,
}

#[derive(Clone)]
pub struct Conn {
    inner: Arc<Inner>,
}

impl Conn {
    pub fn new(
        reader: impl AsyncRead + Send + Unpin + 'static,
        writer: impl AsyncWrite + Send + 'static,
    ) -> Self {
        let conn = Self {
            inner: Arc::new(Inner {
                writer: tokio::sync::Mutex::new(Box::pin(writer)),
                pending: Mutex::new(Some(HashMap::new())),
                next_id: AtomicI64::new(1),
                on_notify: Mutex::default(),
                on_request: Mutex::default(),
            }),
        };
        let c = conn.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    c.dispatch(v);
                }
            }
            for (_, tx) in c.inner.pending.lock().take().unwrap_or_default() {
                let _ = tx.send(Err(RpcError::closed()));
            }
        });
        conn
    }

    pub fn on_notification(&self, f: Option<NotifyFn>) {
        *self.inner.on_notify.lock() = f;
    }

    pub fn on_request(&self, f: Option<RequestFn>) {
        *self.inner.on_request.lock() = f;
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        match self.inner.pending.lock().as_mut() {
            Some(p) => p.insert(id, tx),
            None => return Err(RpcError::closed()),
        };
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        if let Err(e) = self.write(&msg).await {
            if let Some(p) = self.inner.pending.lock().as_mut() {
                p.remove(&id);
            }
            return Err(e);
        }
        rx.await.unwrap_or_else(|_| Err(RpcError::closed()))
    }

    pub async fn notify(&self, method: &str, params: Value) -> Result<(), RpcError> {
        self.write(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .await
    }

    async fn write(&self, msg: &Value) -> Result<(), RpcError> {
        let mut line = serde_json::to_vec(&sort_keys(msg.clone())).unwrap_or_default();
        line.push(b'\n');
        let mut w = self.inner.writer.lock().await;
        let io = |e: std::io::Error| RpcError {
            code: 0,
            message: format!("write to agent: {e}"),
        };
        w.write_all(&line).await.map_err(io)?;
        w.flush().await.map_err(io)
    }

    fn dispatch(&self, v: Value) {
        let method = v.get("method").and_then(Value::as_str);
        let params = v.get("params").cloned().unwrap_or(Value::Null);
        match (method, v.get("id")) {
            (Some(m), Some(id)) => {
                let (m, id) = (m.to_owned(), id.clone());
                let handler = self.inner.on_request.lock().clone();
                let c = self.clone();
                tokio::spawn(async move {
                    let res = match handler {
                        Some(h) => h(m.clone(), params).await,
                        None => Err(RpcError {
                            code: METHOD_NOT_FOUND,
                            message: format!("method not found: {m}"),
                        }),
                    };
                    let msg = match res {
                        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
                        Err(e) => {
                            json!({"jsonrpc": "2.0", "id": id, "error": {"code": e.code, "message": e.message}})
                        }
                    };
                    let _ = c.write(&msg).await;
                });
            }
            (Some(m), None) => {
                let handler = self.inner.on_notify.lock().clone();
                if let Some(h) = handler {
                    h(m, &params);
                }
            }
            (None, Some(id)) => {
                let Some(id) = id.as_i64() else { return };
                let Some(tx) = self
                    .inner
                    .pending
                    .lock()
                    .as_mut()
                    .and_then(|p| p.remove(&id))
                else {
                    return;
                };
                let res = match v.get("error") {
                    Some(e) => Err(RpcError {
                        code: e.get("code").and_then(Value::as_i64).unwrap_or_default(),
                        message: e
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("agent error")
                            .to_owned(),
                    }),
                    None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = tx.send(res);
            }
            (None, None) => {}
        }
    }
}

/// Keys in sorted order at every level, so the wire bytes do not depend on
/// whether a workspace build enables serde_json `preserve_order`.
fn sort_keys(v: Value) -> Value {
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
    use super::*;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn call_notify_and_requests() {
        let (client_end, agent_end) = tokio::io::duplex(1 << 16);
        let (cr, cw) = tokio::io::split(client_end);
        let (mut ar, mut aw) = tokio::io::split(agent_end);
        let conn = Conn::new(cr, cw);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = Arc::clone(&seen);
        conn.on_notification(Some(Arc::new(move |m: &str, p: &Value| {
            s.lock().push(format!("{m} {p}"));
        })));
        let call = tokio::spawn({
            let conn = conn.clone();
            async move { conn.call("initialize", json!({"x": 1})).await }
        });
        let mut buf = vec![0u8; 4096];
        let n = ar.read(&mut buf).await.unwrap();
        assert_eq!(
            std::str::from_utf8(&buf[..n]).unwrap(),
            "{\"id\":1,\"jsonrpc\":\"2.0\",\"method\":\"initialize\",\"params\":{\"x\":1}}\n"
        );
        aw.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{\"a\":1}}\n{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"fs/read_text_file\",\"params\":{}}\n{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n")
            .await
            .unwrap();
        assert_eq!(call.await.unwrap().unwrap(), json!({"ok": true}));
        assert_eq!(seen.lock().clone(), ["session/update {\"a\":1}"]);
        let n = ar.read(&mut buf).await.unwrap();
        let reply: Value = serde_json::from_slice(&buf[..n]).unwrap();
        assert_eq!(reply["id"], 7);
        assert_eq!(reply["error"]["code"], METHOD_NOT_FOUND);

        let call = tokio::spawn({
            let conn = conn.clone();
            async move { conn.call("session/new", json!({})).await }
        });
        let _ = ar.read(&mut buf).await.unwrap();
        aw.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"error\":{\"code\":-32000,\"message\":\"Authentication required\"}}\n")
            .await
            .unwrap();
        let err = call.await.unwrap().unwrap_err();
        assert_eq!(
            (err.code, err.message.as_str()),
            (AUTH_REQUIRED, "Authentication required")
        );
        drop(aw);
        drop(ar);
        assert_eq!(conn.call("x", Value::Null).await.unwrap_err().code, 0);
    }
}
