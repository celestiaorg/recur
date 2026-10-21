//! A minimal HTTP/1.1 stand-in for the bridge (Go `httptest.Server`): each
//! request reaches a handler that can answer once or stream frames, and can
//! see the client hang up.

use std::future::Future;
use std::sync::Arc;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::net::tcp::OwnedWriteHalf;
use tokio_util::sync::CancellationToken;

pub(crate) struct Req {
    pub path: String,
    pub body: Vec<u8>,
}

impl Req {
    /// The JSON body; a Connect stream's single frame is unwrapped.
    pub fn json(&self) -> Value {
        let raw = if self.body.len() >= 5 && self.body[0] <= 2 && self.body.first() != Some(&b'{') {
            let n = u32::from_be_bytes([self.body[1], self.body[2], self.body[3], self.body[4]]) as usize;
            &self.body[5..(5 + n).min(self.body.len())]
        } else {
            &self.body[..]
        };
        serde_json::from_slice(raw).unwrap_or(Value::Null)
    }

    pub fn str(&self, key: &str) -> String {
        self.json().get(key).and_then(Value::as_str).unwrap_or("").to_owned()
    }

    pub fn ends(&self, suffix: &str) -> bool {
        self.path.ends_with(suffix)
    }
}

pub(crate) struct Resp {
    w: OwnedWriteHalf,
    started: bool,
    /// Fires when the client hangs up (Go `r.Context().Done()`).
    pub closed: CancellationToken,
}

impl Resp {
    /// A complete response.
    pub async fn status(mut self, code: u16, body: &str) {
        let head = format!(
            "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = self.w.write_all(head.as_bytes()).await;
        let _ = self.w.write_all(body.as_bytes()).await;
        let _ = self.w.flush().await;
        self.started = true;
    }

    pub async fn json(self, body: &str) {
        self.status(200, body).await;
    }

    pub async fn not_found(self) {
        self.status(404, "404 page not found\n").await;
    }

    /// Streams bytes, starting a 200 Connect stream on first use. Flushes.
    pub async fn write(&mut self, bytes: &[u8]) {
        if !self.started {
            self.started = true;
            let _ = self
                .w
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/connect+json\r\nConnection: close\r\n\r\n")
                .await;
        }
        let _ = self.w.write_all(bytes).await;
        let _ = self.w.flush().await;
    }

    /// A data frame.
    pub async fn frame(&mut self, payload: &str) {
        self.write(&crate::connect::encode_connect_frame(false, payload.as_bytes()))
            .await;
    }

    /// The end-stream frame.
    pub async fn end(&mut self, payload: &str) {
        self.write(&crate::connect::encode_connect_frame(true, payload.as_bytes()))
            .await;
    }
}

pub(crate) struct TestServer {
    pub url: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(crate) async fn serve<F, Fut>(handler: F) -> TestServer
where
    F: Fn(Req, Resp) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let ln = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}", ln.local_addr().expect("addr"));
    let handler = Arc::new(handler);
    let task = tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = ln.accept().await else {
                return;
            };
            let handler = handler.clone();
            tokio::spawn(async move {
                let (r, w) = sock.into_split();
                let mut r = BufReader::new(r);
                let mut line = String::new();
                if r.read_line(&mut line).await.unwrap_or(0) == 0 {
                    return;
                }
                let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
                let mut len = 0usize;
                loop {
                    let mut h = String::new();
                    if r.read_line(&mut h).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let h = h.trim_end();
                    if h.is_empty() {
                        break;
                    }
                    if let Some((k, v)) = h.split_once(':')
                        && k.eq_ignore_ascii_case("content-length")
                    {
                        len = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0; len];
                if r.read_exact(&mut body).await.is_err() {
                    return;
                }
                let closed = CancellationToken::new();
                {
                    let closed = closed.clone();
                    tokio::spawn(async move {
                        let mut buf = [0u8; 64];
                        loop {
                            match r.read(&mut buf).await {
                                Ok(0) | Err(_) => break,
                                Ok(_) => {}
                            }
                        }
                        closed.cancel();
                    });
                }
                let resp = Resp {
                    w,
                    started: false,
                    closed,
                };
                handler(Req { path, body }, resp).await;
            });
        }
    });
    TestServer { url, task }
}

/// Answers an unstarted response with Go's implicit empty 200 when the
/// handler dropped it without writing.
impl Drop for Resp {
    fn drop(&mut self) {
        if !self.started {
            let head = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            let _ = self.w.try_write(head);
        }
    }
}

/// A Python stand-in for cursor-sdk-bridge (Go `writeFakeBridge`): prints a
/// valid discovery line on stderr, serves GetVersion/Ping/Shutdown, and stays
/// up until killed.
pub(crate) const FAKE_BRIDGE: &str = r#"#!/usr/bin/env python3
import json, os, sys, threading, time
from http.server import BaseHTTPRequestHandler
from socketserver import ThreadingTCPServer

here = os.path.dirname(os.path.abspath(__file__))

def marker(name):
    return os.path.join(here, name)

def frame(flag, obj):
    b = json.dumps(obj).encode()
    return bytes([flag]) + len(b).to_bytes(4, "big") + b

class H(BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(n) if n else b""
        ctype = "application/json"
        if self.path.endswith("/GetVersion"):
            out = b'{"bridgeVersion":"fake","protocolVersion":"sdk.v1","capabilities":["agent.create","agent.send"]}'
            if os.path.exists(marker("refuse-once")):
                os.remove(marker("refuse-once"))
                threading.Thread(target=httpd.server_close).start()
        elif self.path.endswith("/CreateAgent") or self.path.endswith("/ResumeAgent"):
            out = json.dumps({"agentId": "agent-fake-%d" % os.getpid()}).encode()
        elif self.path.endswith("/ListRuns"):
            out = b'{"items":[]}'
        elif self.path.endswith("/ListModels") or self.path.endswith("/Me"):
            key = json.loads(body or b"{}").get("options", {}).get("apiKey", "")
            if key == "sk-bad":
                out = b'{"code":"unauthenticated","message":"bad key"}'
                self.send_response(401)
                self.send_header("Content-Type", ctype)
                self.send_header("Content-Length", str(len(out)))
                self.end_headers()
                self.wfile.write(out)
                return
            if self.path.endswith("/Me"):
                out = b'{"user":{"apiKeyName":"Recur laptop","userEmail":"dev@example.com","userFirstName":"Ada"}}'
            else:
                out = b'{"items":[{"id":"gpt-5.6-sol","displayName":"GPT-5.6 Sol"},{"id":"default","displayName":"Auto"},{"id":"composer-2.5","displayName":"Composer 2.5"}]}'
        elif self.path.endswith("/Send"):
            mode = open(marker("send-mode")).read().strip() if os.path.exists(marker("send-mode")) else ""
            if "slow" in mode:
                time.sleep(0.4)
            ctype = "application/connect+json"
            out = frame(0, {"sdkMessage": {"type": "status", "message": {"type": "status", "run_id": "run-fake", "status": "RUNNING"}}})
            if "fail" not in mode:
                out += frame(0, {"result": {"runId": "run-fake", "status": "RUN_LIFECYCLE_STATUS_FINISHED", "result": {"runId": "run-fake", "status": "RUN_LIFECYCLE_STATUS_FINISHED", "result": "fake ok %d" % os.getpid(), "durationMs": "7"}}})
                out += frame(2, {})
        else:
            out = b"{}"
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)
    def log_message(self, *args):
        pass

print("bridge boot noise", file=sys.stderr, flush=True)
ThreadingTCPServer.daemon_threads = True
httpd = ThreadingTCPServer(("127.0.0.1", 0), H)
port = httpd.server_address[1]
ws = os.getcwd()
for i, a in enumerate(sys.argv):
    if a == "--workspace" and i + 1 < len(sys.argv):
        ws = sys.argv[i + 1]
disc = {
    "schemaVersion": 1,
    "serverVersion": "fake",
    "pid": os.getpid(),
    "transport": "tcp",
    "protocol": "connect",
    "host": "127.0.0.1",
    "port": port,
    "url": f"http://127.0.0.1:{port}",
    "authToken": "test-token",
    "workspaceRef": ws,
}
print("cursor-sdk-bridge ready " + json.dumps(disc), file=sys.stderr, flush=True)
print("post-ready diagnostic line", file=sys.stderr, flush=True)
try:
    httpd.serve_forever()
except Exception:
    pass
while True:
    time.sleep(1)
"#;

/// Writes the fake launcher at `dir/cursor-sdk-bridge`.
pub(crate) fn write_fake_bridge(dir: &std::path::Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).expect("mkdir");
    let bin = dir.join("cursor-sdk-bridge");
    std::fs::write(&bin, FAKE_BRIDGE).expect("write fake bridge");
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    bin
}

/// A fake bridge installed as Recur's own `<data>/cursor-bridge` tree, with
/// the minify fixtures the attribution patch needs (Go
/// `writeFakeCursorBridge`).
pub(crate) fn write_fake_cursor_install(data_dir: &std::path::Path) -> std::path::PathBuf {
    let root = data_dir.join("cursor-bridge");
    let bin = write_fake_bridge(&root.join("bin"));
    let entry = root.join("dist/bin/cursor-sdk-bridge.js");
    std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
    std::fs::write(&entry, "console.log(1)\n").unwrap();
    let cjs = root.join("node_modules/@cursor/sdk/dist/cjs/616.js");
    let esm = root.join("node_modules/@cursor/sdk/dist/esm/357.js");
    for p in [&cjs, &esm] {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    }
    std::fs::write(&cjs, "prefix;getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===m?new Pb(n):new yx(n,m);suffix").unwrap();
    std::fs::write(&esm, "prefix;getAgentSkills:r.workspaceServices.getAgentSkills};return void 0===b?new n.DvK(o):new l(o,b);suffix").unwrap();
    let skip = r#"function(e){return"summary"===e.type||"summary-started"===e.type||"summary-completed"===e.type}(e)||(yield null===(n=(t=this.options).onDelta)||void 0===n?void 0:n.call(t,{update:e}))"#;
    for p in [&cjs, &esm] {
        std::fs::write(p.parent().unwrap().join("index.js"), format!("prefix;{skip};suffix")).unwrap();
    }
    bin
}

/// Polls `ok` every 20ms until it holds or `d` elapses.
pub(crate) async fn wait_for(d: std::time::Duration, what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + d;
    while tokio::time::Instant::now() < deadline {
        if ok() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {what}");
}
