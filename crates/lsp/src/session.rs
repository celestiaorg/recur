//! One long-lived language server process for a workspace: stdio framing,
//! events in stdout order, process-group teardown.

use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncWrite, BufReader};
use tokio::sync::mpsc;

use crate::frame::{read_frame, write_frame};
use crate::servers::ServerSpec;

/// A session event for the editor (`lsp.message` / `lsp.exit`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LspEvent {
    /// One JSON-RPC body from the server, headers stripped.
    Message { id: String, message: String },
    /// The server's stdout closed; no more messages follow.
    Exit { id: String },
}

#[derive(Serialize)]
struct MessagePayload<'a> {
    id: &'a str,
    message: &'a str,
}

#[derive(Serialize)]
struct ExitPayload<'a> {
    id: &'a str,
}

impl LspEvent {
    /// The bus event name.
    pub fn name(&self) -> &'static str {
        match self {
            LspEvent::Message { .. } => "lsp.message",
            LspEvent::Exit { .. } => "lsp.exit",
        }
    }

    /// The event payload (`{id, message}` / `{id}`).
    pub fn payload(&self) -> serde_json::Value {
        match self {
            LspEvent::Message { id, message } => {
                serde_json::to_value(MessagePayload { id, message }).unwrap_or_default()
            }
            LspEvent::Exit { id } => serde_json::to_value(ExitPayload { id }).unwrap_or_default(),
        }
    }

    pub fn id(&self) -> &str {
        match self {
            LspEvent::Message { id, .. } | LspEvent::Exit { id } => id,
        }
    }
}

/// Receives session events in order, one at a time.
pub type Emitter = Arc<dyn Fn(LspEvent) + Send + Sync>;

type Stdin = Box<dyn AsyncWrite + Send + Unpin>;

/// Events queue behind a bounded channel so a slow UI applies backpressure
/// to the reader instead of growing without bound.
const EVENT_BUFFER: usize = 64;
const READ_BUFFER: usize = 256 << 10;

pub struct Session {
    pub id: String,
    pub workspace_id: String,
    pub server_key: String,
    pub root_uri: String,
    pub root_path: String,
    pub vue_plugin_path: String,
    pgid: Option<i32>,
    stdin: Arc<tokio::sync::Mutex<Option<Stdin>>>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    closed: bool,
    exit_sent: bool,
    events: Option<mpsc::Sender<LspEvent>>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("id", &self.id)
            .field("workspace_id", &self.workspace_id)
            .field("server_key", &self.server_key)
            .finish_non_exhaustive()
    }
}

impl Session {
    /// Spawns the server in its own process group under `root_path`.
    pub(crate) fn start(
        id: String,
        workspace_id: String,
        root_path: String,
        root_uri: String,
        spec: &ServerSpec,
        vue_plugin_path: String,
        emit: Emitter,
    ) -> std::io::Result<Arc<Self>> {
        let mut cmd = tokio::process::Command::new(&spec.command);
        cmd.args(&spec.args)
            .current_dir(&root_path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .process_group(0)
            .kill_on_drop(false);
        for kv in &spec.env {
            match kv.split_once('=') {
                Some((k, v)) if !k.is_empty() => {
                    cmd.env(k, v);
                }
                _ => {}
            }
        }
        let mut child = cmd.spawn()?;
        let pgid = child.id().map(|p| p as i32);
        let stdin = child.stdin.take().map(|s| Box::new(s) as Stdin);
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let s = Self::with_io(
            id,
            workspace_id,
            spec.key.clone(),
            root_uri,
            root_path,
            vue_plugin_path,
            pgid,
            stdin,
            emit,
        );
        if let Some(stdout) = stdout {
            s.spawn_reader(stdout);
        }
        if let Some(stderr) = stderr {
            let key = spec.key.clone();
            tokio::spawn(log_stderr(key, stderr));
        }
        let (key, me) = (spec.key.clone(), Arc::downgrade(&s));
        tokio::spawn(async move {
            let res = child.wait().await;
            let closed = me.upgrade().is_none_or(|s| s.is_closed());
            if let Ok(st) = res
                && !st.success()
                && !closed
            {
                tracing::debug!(server = %key, status = %st, "lsp server exited");
            }
        });
        Ok(s)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn with_io(
        id: String,
        workspace_id: String,
        server_key: String,
        root_uri: String,
        root_path: String,
        vue_plugin_path: String,
        pgid: Option<i32>,
        stdin: Option<Stdin>,
        emit: Emitter,
    ) -> Arc<Self> {
        let (tx, mut rx) = mpsc::channel::<LspEvent>(EVENT_BUFFER);
        tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                emit(ev);
            }
        });
        Arc::new(Self {
            id,
            workspace_id,
            server_key,
            root_uri,
            root_path,
            vue_plugin_path,
            pgid,
            stdin: Arc::new(tokio::sync::Mutex::new(stdin)),
            state: Mutex::new(State {
                events: Some(tx),
                ..Default::default()
            }),
        })
    }

    pub(crate) fn spawn_reader(self: &Arc<Self>, stdout: impl AsyncRead + Send + Unpin + 'static) {
        let s = Arc::clone(self);
        tokio::spawn(async move {
            let mut r = BufReader::with_capacity(READ_BUFFER, stdout);
            loop {
                match read_frame(&mut r).await {
                    Ok(body) => {
                        let message = String::from_utf8_lossy(&body).into_owned();
                        s.send_event(LspEvent::Message {
                            id: s.id.clone(),
                            message,
                        })
                        .await;
                    }
                    Err(e) => {
                        if !s.is_closed() && e.kind() != std::io::ErrorKind::UnexpectedEof {
                            tracing::debug!(server = %s.server_key, err = %e, "lsp read ended");
                        }
                        break;
                    }
                }
            }
            s.emit_exit().await;
            s.stop_events();
        });
    }

    /// Queues an event behind earlier ones; dropped once events stopped.
    pub(crate) async fn send_event(&self, ev: LspEvent) {
        let tx = self.state.lock().events.clone();
        if let Some(tx) = tx {
            let _ = tx.send(ev).await;
        }
    }

    pub(crate) fn stop_events(&self) {
        self.state.lock().events = None;
    }

    /// Sends `lsp.exit` once and marks the session closed.
    pub(crate) async fn emit_exit(&self) {
        {
            let mut st = self.state.lock();
            if st.exit_sent {
                return;
            }
            st.exit_sent = true;
            st.closed = true;
        }
        self.send_event(LspEvent::Exit {
            id: self.id.clone(),
        })
        .await;
    }

    pub fn is_closed(&self) -> bool {
        self.state.lock().closed
    }

    /// Sends one JSON-RPC body (no headers) to the server.
    pub async fn write(&self, message: &str) -> Result<(), String> {
        if self.is_closed() {
            return Err("lsp session closed".into());
        }
        let mut stdin = self.stdin.lock().await;
        let Some(w) = stdin.as_mut() else {
            return Err("lsp session closed".into());
        };
        write_frame(w, message.as_bytes())
            .await
            .map_err(|e| e.to_string())
    }

    /// Terminates the server's process group and closes stdin. Returns
    /// without waiting for a write in progress.
    pub fn close(&self) {
        {
            let mut st = self.state.lock();
            if st.closed {
                return;
            }
            st.closed = true;
        }
        let taken = match self.stdin.try_lock() {
            Ok(mut g) => {
                g.take();
                true
            }
            Err(_) => false,
        };
        if !taken && let Ok(rt) = tokio::runtime::Handle::try_current() {
            let stdin = Arc::clone(&self.stdin);
            rt.spawn(async move {
                stdin.lock().await.take();
            });
        }
        if let Some(pgid) = self.pgid
            && pgid > 0
        {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(pgid),
                nix::sys::signal::Signal::SIGTERM,
            );
        }
    }
}

async fn log_stderr(key: String, stderr: impl AsyncRead + Unpin) {
    use tokio::io::AsyncBufReadExt;
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let msg = line.trim();
        if !msg.is_empty() {
            tracing::debug!(server = %key, msg = %msg, "lsp stderr");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use std::time::Duration;

    use super::*;

    fn recorder() -> (Emitter, Arc<Mutex<Vec<LspEvent>>>) {
        let got: Arc<Mutex<Vec<LspEvent>>> = Arc::default();
        let log = Arc::clone(&got);
        (Arc::new(move |ev| log.lock().push(ev)), got)
    }

    fn session(stdin: Option<Stdin>, emit: Emitter) -> Arc<Session> {
        Session::with_io(
            "s1".into(),
            "w".into(),
            "k".into(),
            String::new(),
            String::new(),
            String::new(),
            None,
            stdin,
            emit,
        )
    }

    async fn wait_for(got: &Arc<Mutex<Vec<LspEvent>>>, n: usize) {
        for _ in 0..200 {
            if got.lock().len() >= n {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    fn msg(m: &str) -> LspEvent {
        LspEvent::Message {
            id: "s1".into(),
            message: m.into(),
        }
    }

    #[tokio::test]
    async fn send_event_preserves_order() {
        let got: Arc<Mutex<Vec<String>>> = Arc::default();
        let log = Arc::clone(&got);
        let emit: Emitter = Arc::new(move |ev| {
            std::thread::sleep(Duration::from_millis(2));
            if let LspEvent::Message { message, .. } = ev {
                log.lock().push(message);
            }
        });
        let s = session(None, emit);
        for m in ["a", "b", "c"] {
            s.send_event(msg(m)).await;
        }
        s.stop_events();
        for _ in 0..200 {
            if got.lock().len() == 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(*got.lock(), ["a", "b", "c"]);
    }

    /// Released flag plus the parked writer's waker.
    type Gate = Arc<Mutex<(bool, Option<std::task::Waker>)>>;

    fn open(gate: &Gate) {
        let mut g = gate.lock();
        g.0 = true;
        if let Some(w) = g.1.take() {
            w.wake();
        }
    }

    /// A writer that blocks until released.
    struct BlockWriter {
        started: Option<tokio::sync::oneshot::Sender<()>>,
        gate: Gate,
    }

    impl AsyncWrite for BlockWriter {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            if let Some(tx) = self.started.take() {
                let _ = tx.send(());
            }
            let mut g = self.gate.lock();
            if g.0 {
                return Poll::Ready(Ok(buf.len()));
            }
            g.1 = Some(cx.waker().clone());
            Poll::Pending
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn close_does_not_wait_on_write() {
        let (tx, started) = tokio::sync::oneshot::channel();
        let gate: Gate = Arc::default();
        let w = BlockWriter {
            started: Some(tx),
            gate: Arc::clone(&gate),
        };
        let (emit, _) = recorder();
        let s = session(Some(Box::new(w)), emit);
        let writer = tokio::spawn({
            let s = Arc::clone(&s);
            async move { s.write(r#"{"jsonrpc":"2.0"}"#).await }
        });
        tokio::time::timeout(Duration::from_secs(1), started)
            .await
            .unwrap()
            .unwrap();
        let t = std::time::Instant::now();
        s.close();
        assert!(
            t.elapsed() < Duration::from_secs(1),
            "Close blocked on Write"
        );
        open(&gate);
        tokio::time::timeout(Duration::from_secs(1), writer)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn write_rejects_after_close() {
        let (emit, _) = recorder();
        let s = session(Some(Box::new(tokio::io::sink())), emit);
        s.close();
        assert_eq!(s.write("{}").await.unwrap_err(), "lsp session closed");
    }

    #[tokio::test]
    async fn close_after_stop_events_is_fine() {
        let (emit, _) = recorder();
        let s = session(Some(Box::new(tokio::io::sink())), emit);
        s.send_event(msg("m")).await;
        s.emit_exit().await;
        s.stop_events();
        s.close();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_send_stop_and_close() {
        for _ in 0..100 {
            let (emit, _) = recorder();
            let s = session(Some(Box::new(tokio::io::sink())), emit);
            let a = tokio::spawn({
                let s = Arc::clone(&s);
                async move {
                    for _ in 0..20 {
                        s.send_event(msg("m")).await;
                    }
                }
            });
            let b = tokio::spawn({
                let s = Arc::clone(&s);
                async move { s.emit_exit().await }
            });
            let c = tokio::spawn({
                let s = Arc::clone(&s);
                async move {
                    s.close();
                    s.stop_events();
                }
            });
            a.await.unwrap();
            b.await.unwrap();
            c.await.unwrap();
        }
    }

    #[tokio::test]
    async fn exit_follows_stdout_messages() {
        let (emit, got) = recorder();
        let s = session(None, emit);
        let body = [crate::frame::frame(b"a"), crate::frame::frame(b"b")].concat();
        s.spawn_reader(std::io::Cursor::new(body));
        wait_for(&got, 3).await;
        assert_eq!(
            *got.lock(),
            [msg("a"), msg("b"), LspEvent::Exit { id: "s1".into() }]
        );
        assert!(s.is_closed());
    }

    #[test]
    fn event_payloads() {
        assert_eq!(msg("x").name(), "lsp.message");
        assert_eq!(
            msg("x").payload(),
            serde_json::json!({"id": "s1", "message": "x"})
        );
        let exit = LspEvent::Exit { id: "s1".into() };
        assert_eq!(
            (exit.name(), exit.payload()),
            ("lsp.exit", serde_json::json!({"id": "s1"}))
        );
    }
}
