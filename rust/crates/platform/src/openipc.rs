//! Unix-socket protocol between the CLI (`recur-next .`) and the running
//! desktop app. The CLI never touches SQLite; it only hands an absolute path
//! to the app, which runs OpenPath.

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

/// One JSON value the CLI writes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// Empty means focus the app only.
    #[serde(default)]
    pub path: String,
}

/// One JSON value the app writes back.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub path: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub project_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub workspace_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub file: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub created: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub loose: bool,
}

impl Response {
    pub fn failure(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: error.into(),
            ..Self::default()
        }
    }

    pub fn shutting_down() -> Self {
        Self::failure(ERR_SHUTTING_DOWN)
    }
}

/// `Response::error` when this instance is quitting. Clients retry (claim or
/// call again) rather than treating it as a permanent OpenPath failure.
pub const ERR_SHUTTING_DOWN: &str = "shutting down";

/// A decoded response that is not a completed handoff.
pub fn retryable(res: &Response) -> bool {
    !res.ok && res.error == ERR_SHUTTING_DOWN
}

/// The per-data-dir socket.
pub fn sock_path(data_dir: &Path) -> PathBuf {
    data_dir.join("open.sock")
}

/// The per-data-dir flock file that decides which process may open the
/// store. Released when the holding file closes (or the process exits).
pub fn lock_path(data_dir: &Path) -> PathBuf {
    data_dir.join("instance.lock")
}

/// How long the CLI waits for a single connect attempt.
pub const DIAL_TIMEOUT: Duration = Duration::from_millis(400);

/// How long a connected call waits for OpenPath (git budgets plus a queued
/// request ahead of this one).
pub const CALL_TIMEOUT: Duration = Duration::from_secs(60);

const LIVE_PROBE_TIMEOUT: Duration = Duration::from_millis(150);
const CONN_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    /// Another process already owns this data dir.
    #[error("openipc: instance already running")]
    Held,
    #[error("openipc lock: {0}")]
    Lock(io::Error),
    #[error("openipc listen: {0}")]
    Listen(io::Error),
    #[error("openipc chmod: {0}")]
    Chmod(io::Error),
    #[error("openipc: empty socket path")]
    EmptySock,
    #[error("openipc: already listening on {0}")]
    AlreadyListening(PathBuf),
    /// A failed connect: no live listener. Other call errors mean the app was
    /// reached and then failed or timed out; the CLI must not launch a second
    /// instance in that case.
    #[error("{0}")]
    Dial(io::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl IpcError {
    /// A connect failure (no listener or a stale socket).
    pub fn is_dial(&self) -> bool {
        matches!(self, Self::Dial(_))
    }
}

/// The exclusive owner of one data dir: flock plus a bound socket.
#[derive(Debug)]
pub struct Instance {
    lock: Option<File>,
    listener: Option<UnixListener>,
    sock: PathBuf,
}

impl Instance {
    /// Atomically owns `data_dir`: exclusive flock, then bind `open.sock`.
    /// [`IpcError::Held`] means another process won; hand off rather than
    /// opening the store.
    pub fn claim(data_dir: &Path) -> Result<Self, IpcError> {
        std::fs::create_dir_all(data_dir).map_err(IpcError::Lock)?;
        let sock = sock_path(data_dir);
        if socket_live(&sock) {
            return Err(IpcError::Held);
        }
        let lock = try_lock(&lock_path(data_dir))?;
        if socket_live(&sock) {
            return Err(IpcError::Held);
        }
        let listener = listen_sock(&sock)?;
        Ok(Self {
            lock: Some(lock),
            listener: Some(listener),
            sock,
        })
    }

    pub fn sock(&self) -> &Path {
        &self.sock
    }

    /// Hands the bound listener to [`serve_listener`]. The flock stays here
    /// until [`Instance::close`] (or drop).
    pub fn take_listener(&mut self) -> Option<UnixListener> {
        self.listener.take()
    }

    /// Releases the listener and the flock. A listener still held here is
    /// unlinked first (as Go's `UnixListener.Close` does), before the flock
    /// frees, so the next owner's fresh socket is never removed.
    pub fn close(&mut self) {
        if self.listener.take().is_some() {
            let _ = std::fs::remove_file(&self.sock);
        }
        self.lock = None;
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        self.close();
    }
}

fn try_lock(path: &Path) -> Result<File, IpcError> {
    use std::os::unix::fs::OpenOptionsExt;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(IpcError::Lock)?;
    match f.try_lock() {
        Ok(()) => Ok(f),
        Err(std::fs::TryLockError::WouldBlock) => Err(IpcError::Held),
        Err(std::fs::TryLockError::Error(err)) => Err(IpcError::Lock(err)),
    }
}

fn listen_sock(sock: &Path) -> Result<UnixListener, IpcError> {
    let _ = std::fs::remove_file(sock);
    let ln = UnixListener::bind(sock).map_err(IpcError::Listen)?;
    if let Err(err) = std::fs::set_permissions(sock, std::fs::Permissions::from_mode(0o600)) {
        drop(ln);
        if !socket_live(sock) {
            let _ = std::fs::remove_file(sock);
        }
        return Err(IpcError::Chmod(err));
    }
    Ok(ln)
}

fn dial(sock: &Path, timeout: Duration) -> io::Result<UnixStream> {
    use socket2::{Domain, SockAddr, Socket, Type};
    let s = Socket::new(Domain::UNIX, Type::STREAM, None)?;
    s.connect_timeout(&SockAddr::unix(sock)?, timeout)?;
    Ok(UnixStream::from(std::os::fd::OwnedFd::from(s)))
}

fn socket_live(sock: &Path) -> bool {
    dial(sock, LIVE_PROBE_TIMEOUT).is_ok()
}

/// A request handler. Runs on a blocking thread: it may wait on the IPC gate
/// and on OpenPath.
pub type Handler = Arc<dyn Fn(Request) -> Response + Send + Sync>;

/// Binds `sock` and serves it until `stop`. A live listener is left alone
/// (never stolen); a stale socket file is unlinked first.
pub async fn serve(stop: CancellationToken, sock: &Path, handle: Handler) -> Result<(), IpcError> {
    if sock.as_os_str().is_empty() {
        return Err(IpcError::EmptySock);
    }
    let probe = sock.to_path_buf();
    if tokio::task::spawn_blocking(move || socket_live(&probe))
        .await
        .unwrap_or(false)
    {
        return Err(IpcError::AlreadyListening(sock.to_path_buf()));
    }
    let ln = listen_sock(sock)?;
    serve_listener(stop, ln, sock.to_path_buf(), handle).await
}

/// Accepts on an already-bound listener (from [`Instance::claim`] or
/// [`serve`]). On stop it closes and unlinks the listener, then waits for
/// in-flight handlers.
pub async fn serve_listener(
    stop: CancellationToken,
    ln: UnixListener,
    sock: PathBuf,
    handle: Handler,
) -> Result<(), IpcError> {
    ln.set_nonblocking(true)?;
    let ln = tokio::net::UnixListener::from_std(ln)?;
    let mut inflight = tokio::task::JoinSet::new();
    let result = loop {
        tokio::select! {
            () = stop.cancelled() => break Ok(()),
            accepted = ln.accept() => match accepted {
                Ok((conn, _)) => {
                    let handle = Arc::clone(&handle);
                    inflight.spawn(serve_conn(conn, handle));
                }
                Err(err) => break Err(IpcError::Io(err)),
            },
            Some(_) = inflight.join_next(), if !inflight.is_empty() => {}
        }
    };
    // Closing a Go unix listener unlinks its socket; match that, then wait
    // for in-flight handlers.
    drop(ln);
    let _ = std::fs::remove_file(&sock);
    while inflight.join_next().await.is_some() {}
    result
}

async fn serve_conn(mut conn: tokio::net::UnixStream, handle: Handler) {
    let req = match tokio::time::timeout(CONN_DEADLINE, read_request(&mut conn)).await {
        Ok(Some(req)) => req,
        _ => {
            write_response(&mut conn, &Response::failure("invalid request")).await;
            return;
        }
    };
    // OpenPath can hold git budgets well past 5s; the handler has no deadline.
    let res = match tokio::task::spawn_blocking(move || handle(req)).await {
        Ok(res) => res,
        Err(_) => Response::failure("internal error"),
    };
    write_response(&mut conn, &res).await;
}

/// Reads one JSON value (like Go's `json.Decoder`), tolerating `null`.
async fn read_request(conn: &mut tokio::net::UnixStream) -> Option<Request> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let mut values = serde_json::Deserializer::from_slice(&buf).into_iter::<Option<Request>>();
        match values.next() {
            Some(Ok(req)) => return Some(req.unwrap_or_default()),
            Some(Err(err)) if !err.is_eof() => return None,
            _ => {}
        }
        let n = conn.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

async fn write_response(conn: &mut tokio::net::UnixStream, res: &Response) {
    let Ok(mut line) = serde_json::to_vec(res) else {
        return;
    };
    line.push(b'\n');
    let _ = tokio::time::timeout(CONN_DEADLINE, conn.write_all(&line)).await;
}

/// Sends `req` to a running app and returns its response. Blocking.
pub fn call(sock: &Path, req: &Request) -> Result<Response, IpcError> {
    let conn = dial(sock, DIAL_TIMEOUT).map_err(IpcError::Dial)?;
    let mut conn = Deadline::new(conn, CALL_TIMEOUT);
    let mut line = serde_json::to_vec(req)?;
    line.push(b'\n');
    conn.write_all(&line)?;
    let mut values = serde_json::Deserializer::from_reader(io::BufReader::new(&mut conn))
        .into_iter::<Response>();
    match values.next() {
        Some(res) => Ok(res?),
        None => Err(IpcError::Io(io::ErrorKind::UnexpectedEof.into())),
    }
}

/// A stream with one absolute deadline across all reads and writes, like
/// `net.Conn.SetDeadline`.
struct Deadline {
    conn: UnixStream,
    until: Instant,
}

impl Deadline {
    fn new(conn: UnixStream, timeout: Duration) -> Self {
        Self {
            conn,
            until: Instant::now() + timeout,
        }
    }

    fn remaining(&self) -> io::Result<Duration> {
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            Err(io::ErrorKind::TimedOut.into())
        } else {
            Ok(left)
        }
    }
}

/// macOS rejects socket timeouts with EINVAL once the peer has closed; the
/// following read or write then reports the real state.
fn tolerate_closed(res: io::Result<()>) -> io::Result<()> {
    match res {
        Err(err) if err.raw_os_error() == Some(rustix::io::Errno::INVAL.raw_os_error()) => Ok(()),
        other => other,
    }
}

impl Read for Deadline {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        tolerate_closed(self.conn.set_read_timeout(Some(self.remaining()?)))?;
        self.conn.read(buf)
    }
}

impl Write for Deadline {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        tolerate_closed(self.conn.set_write_timeout(Some(self.remaining()?)))?;
        self.conn.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.conn.flush()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Darwin unix-socket paths cap at ~104 bytes; keep temp roots short.
    pub(crate) fn short_dir() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("oi")
            .tempdir_in("/tmp")
            .expect("tempdir")
    }

    pub(crate) async fn wait_for_sock(sock: &Path) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !sock.exists() {
            assert!(
                Instant::now() < deadline,
                "socket was not created: {}",
                sock.display()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn echo() -> Handler {
        Arc::new(|req: Request| Response {
            ok: true,
            path: req.path,
            workspace_id: "w".into(),
            ..Response::default()
        })
    }

    async fn call_async(sock: PathBuf, req: Request) -> Result<Response, IpcError> {
        tokio::task::spawn_blocking(move || call(&sock, &req))
            .await
            .expect("join")
    }

    #[tokio::test]
    async fn call_round_trip() {
        let dir = short_dir();
        let sock = sock_path(dir.path());
        let stop = CancellationToken::new();
        let server = tokio::spawn({
            let (stop, sock) = (stop.clone(), sock.clone());
            async move { serve(stop, &sock, echo()).await }
        });
        wait_for_sock(&sock).await;
        let res = call_async(
            sock.clone(),
            Request {
                path: "/tmp/x".into(),
            },
        )
        .await
        .expect("call");
        assert!(res.ok);
        assert_eq!(res.path, "/tmp/x");
        assert_eq!(res.workspace_id, "w");
        stop.cancel();
        server.await.expect("join").expect("serve");
        assert!(!sock.exists(), "socket removed after stop");
    }

    #[tokio::test]
    async fn serve_does_not_steal_live_socket() {
        let dir = short_dir();
        let sock = dir.path().join("s");
        let stop = CancellationToken::new();
        let first: Handler = Arc::new(|_| Response {
            ok: true,
            workspace_id: "first".into(),
            ..Response::default()
        });
        tokio::spawn({
            let (stop, sock) = (stop.clone(), sock.clone());
            async move { serve(stop, &sock, first).await }
        });
        wait_for_sock(&sock).await;

        let second: Handler = Arc::new(|_| Response {
            ok: true,
            workspace_id: "second".into(),
            ..Response::default()
        });
        let err = serve(CancellationToken::new(), &sock, second)
            .await
            .expect_err("second serve");
        assert!(matches!(err, IpcError::AlreadyListening(_)), "{err}");

        let res = call_async(sock, Request::default()).await.expect("call");
        assert_eq!(res.workspace_id, "first");
        stop.cancel();
    }

    #[test]
    fn dial_error_classification() {
        assert!(IpcError::Dial(io::ErrorKind::NotFound.into()).is_dial());
        assert!(!IpcError::Io(io::ErrorKind::NotFound.into()).is_dial());
        let dir = short_dir();
        let err = call(&sock_path(dir.path()), &Request::default()).expect_err("no listener");
        assert!(err.is_dial(), "{err}");
    }

    #[test]
    fn retryable_responses() {
        assert!(retryable(&Response::shutting_down()));
        assert!(!retryable(&Response {
            ok: true,
            ..Response::default()
        }));
        assert!(!retryable(&Response::failure("not a git repo")));
    }

    #[test]
    fn response_wire_format() {
        let res = Response {
            ok: true,
            project_id: "p".into(),
            created: true,
            ..Response::default()
        };
        assert_eq!(
            serde_json::to_string(&res).expect("json"),
            r#"{"ok":true,"projectId":"p","created":true}"#
        );
        let back: Response =
            serde_json::from_str(r#"{"ok":false,"error":"x","loose":true}"#).expect("parse");
        assert_eq!(
            back,
            Response {
                loose: true,
                ..Response::failure("x")
            }
        );
    }

    #[test]
    fn second_claim_is_held() {
        let dir = short_dir();
        let mut a = Instance::claim(dir.path()).expect("first claim");
        let err = Instance::claim(dir.path()).expect_err("second claim");
        assert!(matches!(err, IpcError::Held), "{err}");
        assert!(a.sock().exists());
        a.close();
        assert!(!a.sock().exists(), "close unlinks the socket it still owns");
        let mut c = Instance::claim(dir.path()).expect("claim after release");
        c.close();
    }

    #[test]
    fn concurrent_claims_have_one_winner() {
        let dir = short_dir();
        let path = dir.path().to_path_buf();
        let results: Vec<_> = (0..2)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || Instance::claim(&path))
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| h.join().expect("join"))
            .collect();
        let mut winners = 0;
        for r in results {
            match r {
                Ok(_) => winners += 1,
                Err(IpcError::Held) => {}
                Err(err) => panic!("concurrent claim error = {err}"),
            }
        }
        assert_eq!(winners, 1);
    }

    #[tokio::test]
    async fn claim_then_serve_listener_round_trip() {
        let dir = short_dir();
        let mut inst = Instance::claim(dir.path()).expect("claim");
        let stop = CancellationToken::new();
        let ln = inst.take_listener().expect("listener");
        tokio::spawn(serve_listener(
            stop.clone(),
            ln,
            inst.sock().to_path_buf(),
            echo(),
        ));
        let res = call_async(
            inst.sock().to_path_buf(),
            Request {
                path: "/tmp/x".into(),
            },
        )
        .await
        .expect("call");
        assert!(
            res.ok && res.path == "/tmp/x" && res.workspace_id == "w",
            "{res:?}"
        );
        stop.cancel();
    }

    #[tokio::test]
    async fn stop_waits_for_in_flight_handler() {
        let dir = short_dir();
        let mut inst = Instance::claim(dir.path()).expect("claim");
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = parking_lot::Mutex::new(release_rx);
        let handler: Handler = Arc::new(move |_| {
            let _ = started_tx.send(());
            let _ = release_rx.lock().recv();
            Response {
                ok: true,
                ..Response::default()
            }
        });
        let stop = CancellationToken::new();
        let ln = inst.take_listener().expect("listener");
        let mut done = tokio::spawn(serve_listener(
            stop.clone(),
            ln,
            inst.sock().to_path_buf(),
            handler,
        ));
        let sock = inst.sock().to_path_buf();
        tokio::task::spawn_blocking(move || call(&sock, &Request::default()));
        tokio::task::spawn_blocking(move || started_rx.recv_timeout(Duration::from_secs(2)))
            .await
            .expect("join")
            .expect("handler did not start");
        stop.cancel();
        assert!(
            tokio::time::timeout(Duration::from_millis(80), &mut done)
                .await
                .is_err(),
            "serve_listener returned before the handler finished"
        );
        release_tx.send(()).expect("release");
        tokio::time::timeout(Duration::from_secs(2), done)
            .await
            .expect("serve_listener did not return after the handler finished")
            .expect("join")
            .expect("stop");
    }

    #[tokio::test]
    async fn invalid_request_gets_error_response() {
        let dir = short_dir();
        let sock = sock_path(dir.path());
        let stop = CancellationToken::new();
        tokio::spawn({
            let (stop, sock) = (stop.clone(), sock.clone());
            async move { serve(stop, &sock, echo()).await }
        });
        wait_for_sock(&sock).await;
        let reply = tokio::task::spawn_blocking(move || {
            let mut c = UnixStream::connect(&sock).expect("connect");
            c.write_all(b"{nope}").expect("write");
            let mut out = String::new();
            c.read_to_string(&mut out).expect("read");
            out
        })
        .await
        .expect("join");
        assert_eq!(reply, "{\"ok\":false,\"error\":\"invalid request\"}\n");
        stop.cancel();
    }
}
