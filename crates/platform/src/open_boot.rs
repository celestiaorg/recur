//! Launch-time single-instance flow (root `open_boot.go`): claim the data dir
//! or hand the launch to the running instance, park IPC requests until the
//! services exist, and replay opens the UI missed.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};
use tokio_util::sync::CancellationToken;

use crate::openipc::{self, Handler, Instance, IpcError, Request, Response};

/// Pulls `--open PATH` / `--open=PATH` out of argv. Returns the paths and the
/// remaining args. A trailing `--open` without a value is dropped.
pub fn peel_open_args(args: &[String]) -> (Vec<String>, Vec<String>) {
    let mut opens = Vec::new();
    let mut rest = Vec::with_capacity(args.len());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--open" {
            if let Some(p) = it.next() {
                opens.push(p.clone());
            }
        } else if let Some(p) = a.strip_prefix("--open=") {
            opens.push(p.to_string());
        } else {
            rest.push(a.clone());
        }
    }
    (opens, rest)
}

/// Asks a live instance to take this launch instead of starting a second
/// process. OS notification clicks, Finder re-opens and
/// `open "Recur Next.app" --args --open PATH` all enter `main`. Empty `paths`
/// means focus only. True when the running instance accepted every request
/// (the caller exits without opening the store).
pub fn handoff_to_running_instance(data_dir: &Path, paths: &[String]) -> bool {
    let sock = openipc::sock_path(data_dir);
    let focus_only = [String::new()];
    let paths = if paths.is_empty() {
        &focus_only[..]
    } else {
        paths
    };
    for p in paths {
        let res = match openipc::call(&sock, &Request { path: p.clone() }) {
            Ok(res) => res,
            Err(err) => {
                // Dial error: no listener. EOF/timeout: the owner may be
                // dying. Either way this path was not delivered, so retry the
                // claim rather than exiting.
                if !err.is_dial() {
                    tracing::warn!("handoff: {err}");
                }
                return false;
            }
        };
        if openipc::retryable(&res) {
            tracing::warn!("handoff: {}", res.error);
            return false;
        }
        if !res.ok && !res.error.is_empty() {
            tracing::warn!("handoff: {}", res.error);
        }
    }
    true
}

/// How long a launch keeps retrying while a dying owner drains in-flight
/// OpenPath (bounded by [`openipc::CALL_TIMEOUT`]) and engine shutdown (5s)
/// before its flock frees.
// simplify: server OpenPath has no deadline; a hung git past CALL_TIMEOUT
// still loses. Bound serve_listener if that shows up.
pub const CLAIM_RETRY_FOR: Duration = Duration::from_secs(60 + 8);

/// Outcome of [`claim_or_handoff`].
#[derive(Debug)]
pub enum Claimed {
    /// This process owns the data dir.
    Owner(Instance),
    /// A running instance took the launch; exit.
    HandedOff,
}

#[derive(Debug, thiserror::Error)]
pub enum ClaimError {
    #[error("another {} is running this data dir", crate::identity::APP_NAME)]
    StillHeld,
    #[error(transparent)]
    Ipc(#[from] IpcError),
}

/// Owns `data_dir` (flock plus bound socket) before the store opens. When
/// another process holds it, hands off; when that owner disappears mid-wait,
/// retries the claim instead of exiting empty-handed. Blocking.
pub fn claim_or_handoff(data_dir: &Path, paths: &[String]) -> Result<Claimed, ClaimError> {
    let deadline = Instant::now() + CLAIM_RETRY_FOR;
    loop {
        match Instance::claim(data_dir) {
            Ok(inst) => return Ok(Claimed::Owner(inst)),
            Err(IpcError::Held) => {}
            Err(err) => return Err(err.into()),
        }
        if handoff_to_running_instance(data_dir, paths) {
            return Ok(Claimed::HandedOff);
        }
        if Instant::now() >= deadline {
            return Err(ClaimError::StillHeld);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Parks IPC requests until [`IpcGate::set`] installs the real handler, so the
/// claim can accept before the store opens without dropping a handoff. Stop
/// and admission share one lock: after [`IpcGate::stop`] returns, no request
/// reaches the real handler.
#[derive(Default)]
pub struct IpcGate {
    state: Mutex<GateState>,
    cv: Condvar,
}

#[derive(Default)]
struct GateState {
    stopped: bool,
    handler: Option<Handler>,
}

impl IpcGate {
    pub fn new() -> Arc<Self> {
        Arc::default()
    }

    pub fn set(&self, handler: Handler) {
        self.state.lock().handler = Some(handler);
        self.cv.notify_all();
    }

    pub fn stop(&self) {
        self.state.lock().stopped = true;
        self.cv.notify_all();
    }

    /// Blocks until a handler is installed or the gate stops.
    pub fn handle(&self, req: Request) -> Response {
        let mut st = self.state.lock();
        while !st.stopped && st.handler.is_none() {
            self.cv.wait(&mut st);
        }
        if st.stopped {
            return Response::shutting_down();
        }
        let handler = st.handler.clone().expect("handler set");
        drop(st);
        handler(req)
    }
}

type Emit<T> = Arc<dyn Fn(T) + Send + Sync>;
/// Posts a native un-minimise/show/focus to the UI thread. The callback must
/// run from that posted block (or immediately when there is no window).
type Focus = Arc<dyn Fn(Box<dyn FnOnce() + Send>) + Send + Sync>;

/// Replays the last OpenPath onto the UI if it happened before the UI
/// subscribed (cold start via `recur-next .` / `--open`), and focuses the
/// window for handoffs.
pub struct OpenHub<T> {
    state: Mutex<HubState<T>>,
    cv: Condvar,
}

struct HubState<T> {
    ui_ready: bool,
    app_live: bool,
    revoked: bool,
    delivered: bool,
    last: Option<T>,
    emit: Option<Emit<T>>,
    focus: Option<Focus>,
    focusing: usize,
}

impl<T> Default for OpenHub<T> {
    fn default() -> Self {
        Self {
            state: Mutex::new(HubState {
                ui_ready: false,
                app_live: false,
                revoked: false,
                delivered: false,
                last: None,
                emit: None,
                focus: None,
                focusing: 0,
            }),
            cv: Condvar::new(),
        }
    }
}

impl<T: Clone + Send + 'static> OpenHub<T> {
    pub fn new() -> Arc<Self> {
        Arc::default()
    }

    /// Records an OpenPath result. Delivered now if the UI is ready,
    /// otherwise replayed by [`OpenHub::ui_ready_now`].
    pub fn note(&self, res: T) {
        let mut st = self.state.lock();
        st.last = Some(res);
        st.delivered = st.ui_ready;
    }

    /// The UI subscribed to `app.open`; emit a pending result once.
    pub fn ui_ready_now(&self) {
        let mut st = self.state.lock();
        st.ui_ready = true;
        if !st.delivered
            && let (Some(last), Some(emit)) = (st.last.clone(), st.emit.clone())
        {
            emit(last);
            st.delivered = true;
        }
    }

    pub fn set_hooks(
        &self,
        emit: Option<impl Fn(T) + Send + Sync + 'static>,
        focus: Option<impl Fn(Box<dyn FnOnce() + Send>) + Send + Sync + 'static>,
    ) {
        let mut st = self.state.lock();
        st.emit = emit.map(|f| Arc::new(f) as Emit<T>);
        st.focus = focus.map(|f| Arc::new(f) as Focus);
    }

    /// The application finished launching; focus hooks may run.
    pub fn mark_app_live(&self) {
        self.state.lock().app_live = true;
    }

    /// Makes later [`OpenHub::bring_forward`] calls return false. Call before
    /// the window is destroyed so a draining handler cannot show it.
    pub fn drop_ui(&self) {
        let mut st = self.state.lock();
        st.app_live = false;
        st.focus = None;
        st.revoked = true;
        self.cv.notify_all();
    }

    /// Blocks until every admitted focus hook has returned (the post, not the
    /// show callback). Call after [`revoke_open_ui`] when the window closes.
    pub fn wait_focus(&self) {
        let mut st = self.state.lock();
        while st.focusing > 0 {
            self.cv.wait(&mut st);
        }
    }

    /// Focuses the window. False after [`OpenHub::drop_ui`], so IPC can answer
    /// shutting-down instead of OK. True means the focus hook's callback ran
    /// and the hub is not revoked: posting to the main queue is not enough,
    /// because macOS terminate can return without draining it. Blocking.
    pub fn bring_forward(self: &Arc<Self>) -> bool {
        let mut st = self.state.lock();
        if st.revoked {
            return false;
        }
        let Some(focus) = st.focus.clone().filter(|_| st.app_live) else {
            return true;
        };
        st.focusing += 1;
        drop(st);

        let applied = Arc::new(AtomicBool::new(false));
        let hub = Arc::clone(self);
        let signal = {
            let (hub, applied) = (Arc::clone(self), Arc::clone(&applied));
            Box::new(move || {
                if !applied.swap(true, Ordering::SeqCst) {
                    let _st = hub.state.lock();
                    hub.cv.notify_all();
                }
            }) as Box<dyn FnOnce() + Send>
        };
        std::thread::spawn(move || {
            if !hub.state.lock().revoked {
                focus(signal);
            }
            let mut st = hub.state.lock();
            st.focusing -= 1;
            hub.cv.notify_all();
        });

        let mut st = self.state.lock();
        while !st.revoked && !applied.load(Ordering::SeqCst) {
            self.cv.wait(&mut st);
        }
        !st.revoked
    }
}

/// Drops focus hooks, then stops IPC admission, so a later handoff gets
/// shutting-down instead of OK from a no-op focus. Safe to call repeatedly
/// (closing the last window and Quit both reach it).
pub fn revoke_open_ui<T: Clone + Send + 'static>(hub: &OpenHub<T>, gate: &IpcGate) {
    hub.drop_ui();
    gate.stop();
}

/// Stops the IPC server started by [`start_open_ipc`].
pub struct OpenIpcStop {
    stop: Box<dyn FnOnce() + Send>,
}

impl OpenIpcStop {
    /// Revokes the UI, stops accepting and joins in-flight requests. Blocking;
    /// call from the shutdown hook before engine shutdown and store close.
    /// The flock stays held until the [`Instance`] closes.
    pub fn stop(self) {
        (self.stop)();
    }
}

/// Serves the claimed socket on `rt`, routing requests through `gate`.
pub fn start_open_ipc<T: Clone + Send + 'static>(
    rt: &tokio::runtime::Handle,
    inst: &mut Instance,
    gate: Arc<IpcGate>,
    hub: Arc<OpenHub<T>>,
) -> OpenIpcStop {
    let Some(ln) = inst.take_listener() else {
        return OpenIpcStop {
            stop: Box::new(|| {}),
        };
    };
    let token = CancellationToken::new();
    let (finished_tx, finished_rx) = std::sync::mpsc::channel::<()>();
    let handler: Handler = {
        let gate = Arc::clone(&gate);
        Arc::new(move |req| gate.handle(req))
    };
    let sock = inst.sock().to_path_buf();
    rt.spawn({
        let token = token.clone();
        async move {
            if let Err(err) = openipc::serve_listener(token, ln, sock, handler).await {
                tracing::warn!("open ipc: {err}");
            }
            drop(finished_tx);
        }
    });
    OpenIpcStop {
        stop: Box::new(move || {
            revoke_open_ui(&hub, &gate);
            token.cancel();
            let _ = finished_rx.recv();
        }),
    }
}

/// Installs the real IPC handler once services exist. `open_path` is
/// `WorkspacesService::open_path`; its result is noted on the hub (replayed
/// to the UI) and converted into the wire response.
pub fn attach_open_ipc<T, F>(gate: &IpcGate, hub: Arc<OpenHub<T>>, open_path: F)
where
    T: Clone + Send + Into<Response> + 'static,
    F: Fn(&str) -> Result<T, String> + Send + Sync + 'static,
{
    gate.set(Arc::new(move |req: Request| {
        if req.path.trim().is_empty() {
            if !hub.bring_forward() {
                return Response::shutting_down();
            }
            return Response {
                ok: true,
                ..Response::default()
            };
        }
        let res = match open_path(&req.path) {
            Ok(res) => res,
            Err(err) => return Response::failure(err),
        };
        hub.note(res.clone());
        if !hub.bring_forward() {
            return Response::shutting_down();
        }
        let mut wire: Response = res.into();
        wire.ok = true;
        wire
    }));
}

/// Opens `--open` paths from this launch before the window exists; the hub
/// replays the last one when the UI becomes ready.
pub fn apply_cold_opens<T, F>(hub: &OpenHub<T>, paths: &[String], open_path: F)
where
    T: Clone + Send + 'static,
    F: Fn(&str) -> Result<T, String>,
{
    for p in paths {
        let p = p.trim();
        if p.is_empty() {
            continue;
        }
        match open_path(p) {
            Ok(res) => hub.note(res),
            Err(err) => tracing::warn!("open {p}: {err}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openipc::tests::{short_dir, wait_for_sock};
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn peels_open_args() {
        let (opens, rest) = peel_open_args(&strings(&[
            "--open",
            "/tmp/a",
            "--other",
            "--open=/tmp/b",
            "x",
        ]));
        assert_eq!(opens, ["/tmp/a", "/tmp/b"]);
        assert_eq!(rest, ["--other", "x"]);
        let (opens, rest) = peel_open_args(&[]);
        assert!(opens.is_empty() && rest.is_empty());
        let (opens, rest) = peel_open_args(&strings(&["--open"]));
        assert!(opens.is_empty(), "{opens:?}");
        assert!(rest.is_empty(), "flag dropped: {rest:?}");
    }

    #[test]
    fn handoff_without_listener_fails() {
        let dir = short_dir();
        assert!(!handoff_to_running_instance(dir.path(), &[]));
        assert!(!handoff_to_running_instance(
            dir.path(),
            &strings(&["/tmp/x"])
        ));
    }

    /// Serves `dir`'s socket on a background runtime; requests are sent to the
    /// returned channel and answered by `reply`.
    fn serve_dir(
        dir: &Path,
        reply: impl Fn(&Request) -> Response + Send + Sync + 'static,
    ) -> (
        tokio::runtime::Runtime,
        CancellationToken,
        mpsc::Receiver<Request>,
    ) {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let stop = CancellationToken::new();
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let handler: Handler = Arc::new(move |req| {
            let res = reply(&req);
            let _ = tx.lock().send(req);
            res
        });
        let sock = openipc::sock_path(dir);
        rt.spawn({
            let (stop, sock) = (stop.clone(), sock.clone());
            async move { openipc::serve(stop, &sock, handler).await }
        });
        rt.block_on(wait_for_sock(&sock));
        (rt, stop, rx)
    }

    fn ok_echo(req: &Request) -> Response {
        Response {
            ok: true,
            path: req.path.clone(),
            ..Response::default()
        }
    }

    #[test]
    fn handoff_focuses_live_app() {
        let dir = short_dir();
        let (_rt, stop, rx) = serve_dir(dir.path(), ok_echo);
        assert!(handoff_to_running_instance(dir.path(), &[]));
        let req = rx.recv_timeout(Duration::from_secs(2)).expect("request");
        assert_eq!(req.path, "", "focus only");
        stop.cancel();
    }

    #[test]
    fn handoff_forwards_open_paths() {
        let dir = short_dir();
        let (_rt, stop, rx) = serve_dir(dir.path(), ok_echo);
        let paths = strings(&["/tmp/a", "/tmp/b"]);
        assert!(handoff_to_running_instance(dir.path(), &paths));
        for want in &paths {
            assert_eq!(
                &rx.recv_timeout(Duration::from_secs(2)).expect("req").path,
                want
            );
        }
        stop.cancel();
    }

    #[test]
    fn handoff_connected_drop_is_not_handoff() {
        let dir = short_dir();
        let ln =
            std::os::unix::net::UnixListener::bind(openipc::sock_path(dir.path())).expect("listen");
        std::thread::spawn(move || {
            for conn in ln.incoming() {
                drop(conn);
            }
        });
        assert!(!handoff_to_running_instance(dir.path(), &[]));
    }

    fn serve_instance(
        mut inst: Instance,
        reply: impl Fn(&Request) -> Response + Send + Sync + 'static,
    ) -> (tokio::runtime::Runtime, CancellationToken, Instance) {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let stop = CancellationToken::new();
        let ln = inst.take_listener().expect("listener");
        let handler: Handler = Arc::new(move |req| reply(&req));
        rt.spawn(openipc::serve_listener(
            stop.clone(),
            ln,
            inst.sock().to_path_buf(),
            handler,
        ));
        (rt, stop, inst)
    }

    #[test]
    fn claim_or_handoff_when_held() {
        let dir = short_dir();
        let inst = Instance::claim(dir.path()).expect("claim");
        let (_rt, stop, _inst) = serve_instance(inst, ok_echo);
        let got = claim_or_handoff(dir.path(), &[]).expect("claim_or_handoff");
        assert!(matches!(got, Claimed::HandedOff), "{got:?}");
        stop.cancel();
    }

    #[test]
    fn claim_or_handoff_reclaims_after_release() {
        let dir = short_dir();
        let mut held = Instance::claim(dir.path()).expect("claim");
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            held.close();
        });
        let got = claim_or_handoff(dir.path(), &[]).expect("claim_or_handoff");
        assert!(matches!(got, Claimed::Owner(_)), "{got:?}");
    }

    #[test]
    fn claim_or_handoff_retries_on_shutdown() {
        let dir = short_dir();
        let held = Instance::claim(dir.path()).expect("claim");
        let (rt, stop, mut held) = serve_instance(held, |_| Response::shutting_down());
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            stop.cancel();
            held.close();
            drop(rt);
        });
        let got = claim_or_handoff(dir.path(), &[]).expect("claim_or_handoff");
        assert!(matches!(got, Claimed::Owner(_)), "{got:?}");
    }

    #[test]
    fn shutdown_response_is_not_handoff() {
        let dir = short_dir();
        let inst = Instance::claim(dir.path()).expect("claim");
        let (_rt, stop, _inst) = serve_instance(inst, |_| Response::shutting_down());
        assert!(!handoff_to_running_instance(dir.path(), &[]));
        stop.cancel();
    }

    #[test]
    fn partial_shutdown_is_not_handoff() {
        let dir = short_dir();
        let inst = Instance::claim(dir.path()).expect("claim");
        let (_rt, stop, _inst) = serve_instance(inst, |req| {
            if req.path == "/tmp/b" {
                Response::shutting_down()
            } else {
                ok_echo(req)
            }
        });
        assert!(!handoff_to_running_instance(
            dir.path(),
            &strings(&["/tmp/a", "/tmp/b"])
        ));
        stop.cancel();
    }

    #[test]
    fn claim_retry_covers_open_path_drain() {
        assert!(CLAIM_RETRY_FOR >= openipc::CALL_TIMEOUT + Duration::from_secs(8));
    }

    fn ok() -> Response {
        Response {
            ok: true,
            ..Response::default()
        }
    }

    #[test]
    fn gate_stop_is_authoritative() {
        let g = IpcGate::new();
        let called = Arc::new(AtomicUsize::new(0));
        g.set({
            let called = Arc::clone(&called);
            Arc::new(move |_| {
                called.fetch_add(1, Ordering::SeqCst);
                ok()
            })
        });
        g.stop();
        for _ in 0..50 {
            assert_eq!(g.handle(Request::default()), Response::shutting_down());
        }
        assert_eq!(called.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn gate_stop_prevents_admission() {
        let g = IpcGate::new();
        let calls = Arc::new(AtomicUsize::new(0));
        g.set({
            let calls = Arc::clone(&calls);
            Arc::new(move |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                ok()
            })
        });
        let threads: Vec<_> = (0..64)
            .map(|_| {
                let g = Arc::clone(&g);
                std::thread::spawn(move || g.handle(Request::default()))
            })
            .collect();
        g.stop();
        for t in threads {
            t.join().expect("join");
        }
        let after = calls.load(Ordering::SeqCst);
        for _ in 0..64 {
            assert_eq!(g.handle(Request::default()), Response::shutting_down());
        }
        assert_eq!(calls.load(Ordering::SeqCst), after);
    }

    #[test]
    fn gate_stop_unblocks_waiter() {
        let g = IpcGate::new();
        let (tx, rx) = mpsc::channel();
        {
            let g = Arc::clone(&g);
            std::thread::spawn(move || {
                let _ = tx.send(g.handle(Request::default()));
            });
        }
        g.stop();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).expect("unblocked"),
            Response::shutting_down()
        );
    }

    type Hub = OpenHub<String>;
    const NO_EMIT: Option<fn(String)> = None;

    #[test]
    fn drop_ui_skips_focus() {
        let h = Hub::new();
        let called = Arc::new(AtomicUsize::new(0));
        h.set_hooks(
            NO_EMIT,
            Some({
                let called = Arc::clone(&called);
                move |applied: Box<dyn FnOnce() + Send>| {
                    called.fetch_add(1, Ordering::SeqCst);
                    applied();
                }
            }),
        );
        h.mark_app_live();
        assert!(h.bring_forward());
        h.wait_focus();
        assert_eq!(called.load(Ordering::SeqCst), 1);
        h.drop_ui();
        assert!(!h.bring_forward());
        h.wait_focus();
        assert_eq!(called.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn bring_forward_waits_for_applied() {
        let h = Hub::new();
        let (posted_tx, posted_rx) = mpsc::channel::<Box<dyn FnOnce() + Send>>();
        let posted_tx = Mutex::new(posted_tx);
        h.set_hooks(
            NO_EMIT,
            Some(move |applied| {
                let _ = posted_tx.lock().send(applied);
            }),
        );
        h.mark_app_live();
        let (got_tx, got_rx) = mpsc::channel();
        {
            let h = Arc::clone(&h);
            std::thread::spawn(move || got_tx.send(h.bring_forward()));
        }
        let apply = posted_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("posted");
        assert!(
            got_rx.recv_timeout(Duration::from_millis(20)).is_err(),
            "bring_forward returned after post, want wait until applied"
        );
        apply();
        assert!(
            got_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("returned")
        );
        h.wait_focus();
    }

    #[test]
    fn bring_forward_false_if_revoked_before_applied() {
        let h = Hub::new();
        let (posted_tx, posted_rx) = mpsc::channel::<()>();
        let posted_tx = Mutex::new(posted_tx);
        h.set_hooks(
            NO_EMIT,
            Some(move |_applied| {
                let _ = posted_tx.lock().send(());
            }),
        );
        h.mark_app_live();
        let (got_tx, got_rx) = mpsc::channel();
        {
            let h = Arc::clone(&h);
            std::thread::spawn(move || got_tx.send(h.bring_forward()));
        }
        posted_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("posted");
        h.drop_ui();
        assert!(
            !got_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("returned")
        );
        h.wait_focus();
    }

    /// A focus hook that announces it started and blocks until released.
    fn blocking_focus(h: &Hub) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let (started_tx, release_rx) = (Mutex::new(started_tx), Mutex::new(release_rx));
        h.set_hooks(
            NO_EMIT,
            Some(move |applied: Box<dyn FnOnce() + Send>| {
                let _ = started_tx.lock().send(());
                let _ = release_rx.lock().recv();
                applied();
            }),
        );
        h.mark_app_live();
        (started_rx, release_tx)
    }

    #[test]
    fn wait_focus_drains_in_flight() {
        let h = Hub::new();
        let (started, release) = blocking_focus(&h);
        {
            let h = Arc::clone(&h);
            std::thread::spawn(move || h.bring_forward());
        }
        started
            .recv_timeout(Duration::from_secs(2))
            .expect("started");
        h.drop_ui();
        let (done_tx, done_rx) = mpsc::channel();
        {
            let h = Arc::clone(&h);
            std::thread::spawn(move || {
                h.wait_focus();
                let _ = done_tx.send(());
            });
        }
        assert!(
            done_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "wait_focus returned while focus still running"
        );
        release.send(()).expect("release");
        done_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("wait_focus returned");
    }

    #[test]
    fn revoke_rejects_handoff_during_wait_focus() {
        let g = IpcGate::new();
        let h = Hub::new();
        let (started, release) = blocking_focus(&h);
        g.set({
            let h = Arc::clone(&h);
            Arc::new(move |_| {
                h.bring_forward();
                ok()
            })
        });
        {
            let g = Arc::clone(&g);
            std::thread::spawn(move || g.handle(Request::default()));
        }
        started
            .recv_timeout(Duration::from_secs(2))
            .expect("started");
        revoke_open_ui(&h, &g);
        assert_eq!(g.handle(Request::default()), Response::shutting_down());
        release.send(()).expect("release");
        h.wait_focus();
    }

    #[derive(Clone)]
    struct Opened(String);

    impl From<Opened> for Response {
        fn from(o: Opened) -> Self {
            Response {
                path: o.0,
                workspace_id: "w".into(),
                ..Response::default()
            }
        }
    }

    #[test]
    fn attach_empty_path_after_drop_ui() {
        let g = IpcGate::new();
        let h = OpenHub::<Opened>::new();
        attach_open_ipc(&g, Arc::clone(&h), |_: &str| -> Result<Opened, String> {
            panic!("open_path called for a focus-only request")
        });
        h.set_hooks(
            None::<fn(Opened)>,
            Some(|_: Box<dyn FnOnce() + Send>| panic!("focus called after drop_ui")),
        );
        h.mark_app_live();
        h.drop_ui();
        assert_eq!(g.handle(Request::default()), Response::shutting_down());
    }

    #[test]
    fn attach_opens_notes_and_converts() {
        let g = IpcGate::new();
        let h = OpenHub::<Opened>::new();
        attach_open_ipc(&g, Arc::clone(&h), |p: &str| {
            if p == "/bad" {
                Err("not a git repo".to_string())
            } else {
                Ok(Opened(p.to_string()))
            }
        });
        let res = g.handle(Request {
            path: "/tmp/a".into(),
        });
        assert_eq!(
            res,
            Response {
                ok: true,
                path: "/tmp/a".into(),
                workspace_id: "w".into(),
                ..Response::default()
            }
        );
        assert_eq!(
            g.handle(Request {
                path: "/bad".into()
            }),
            Response::failure("not a git repo")
        );

        let emitted = Arc::new(Mutex::new(Vec::new()));
        h.set_hooks(
            Some({
                let emitted = Arc::clone(&emitted);
                move |o: Opened| emitted.lock().push(o.0)
            }),
            None::<fn(Box<dyn FnOnce() + Send>)>,
        );
        h.ui_ready_now();
        h.ui_ready_now();
        assert_eq!(*emitted.lock(), ["/tmp/a"], "pending open replayed once");
    }

    #[test]
    fn cold_opens_replay_last() {
        let h = OpenHub::<String>::new();
        apply_cold_opens(&h, &strings(&["  ", "/tmp/a", "/bad", "/tmp/b"]), |p| {
            if p == "/bad" {
                Err("nope".into())
            } else {
                Ok(p.to_string())
            }
        });
        let emitted = Arc::new(Mutex::new(Vec::new()));
        h.set_hooks(
            Some({
                let emitted = Arc::clone(&emitted);
                move |s: String| emitted.lock().push(s)
            }),
            None::<fn(Box<dyn FnOnce() + Send>)>,
        );
        h.ui_ready_now();
        assert_eq!(*emitted.lock(), ["/tmp/b"]);
        h.note("/tmp/c".into());
        h.ui_ready_now();
        assert_eq!(
            *emitted.lock(),
            ["/tmp/b"],
            "notes after ready are live, not replayed"
        );
    }

    #[test]
    fn copied_handle_returns_shutting_down_after_revoke() {
        let g = IpcGate::new();
        let h = Hub::new();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let (entered_tx, release_rx) = (Mutex::new(entered_tx), Mutex::new(release_rx));
        h.set_hooks(
            NO_EMIT,
            Some(|_: Box<dyn FnOnce() + Send>| panic!("focus called after drop_ui")),
        );
        h.mark_app_live();
        g.set({
            let h = Arc::clone(&h);
            Arc::new(move |_| {
                let _ = entered_tx.lock().send(());
                let _ = release_rx.lock().recv();
                if !h.bring_forward() {
                    return Response::shutting_down();
                }
                ok()
            })
        });
        let (done_tx, done_rx) = mpsc::channel();
        {
            let g = Arc::clone(&g);
            std::thread::spawn(move || {
                let _ = done_tx.send(g.handle(Request::default()));
            });
        }
        entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("entered");
        revoke_open_ui(&h, &g);
        release_tx.send(()).expect("release");
        assert_eq!(
            done_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("returned"),
            Response::shutting_down()
        );
    }

    #[test]
    fn wait_focus_is_authoritative() {
        let h = Hub::new();
        let n = Arc::new(AtomicUsize::new(0));
        h.set_hooks(
            NO_EMIT,
            Some({
                let n = Arc::clone(&n);
                move |applied: Box<dyn FnOnce() + Send>| {
                    std::thread::sleep(Duration::from_millis(5));
                    n.fetch_add(1, Ordering::SeqCst);
                    applied();
                }
            }),
        );
        h.mark_app_live();
        let threads: Vec<_> = (0..32)
            .map(|_| {
                let h = Arc::clone(&h);
                std::thread::spawn(move || h.bring_forward())
            })
            .collect();
        std::thread::sleep(Duration::from_millis(2));
        h.drop_ui();
        h.wait_focus();
        let after = n.load(Ordering::SeqCst);
        for _ in 0..32 {
            h.bring_forward();
        }
        for t in threads {
            t.join().expect("join");
        }
        assert_eq!(n.load(Ordering::SeqCst), after);
    }

    #[test]
    fn start_open_ipc_serves_through_gate_and_stops() {
        let dir = short_dir();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let mut inst = Instance::claim(dir.path()).expect("claim");
        let gate = IpcGate::new();
        let hub = Hub::new();
        let stop = start_open_ipc(rt.handle(), &mut inst, Arc::clone(&gate), Arc::clone(&hub));
        let sock = inst.sock().to_path_buf();
        let (tx, rx) = mpsc::channel();
        {
            let sock = sock.clone();
            std::thread::spawn(move || {
                let _ = tx.send(openipc::call(&sock, &Request::default()));
            });
        }
        assert!(
            rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "request parks until the gate has a handler"
        );
        attach_open_ipc(&gate, Arc::clone(&hub), |p: &str| Ok(p.to_string()));
        let res = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("answered")
            .expect("call");
        assert!(res.ok);
        stop.stop();
        assert!(!sock.exists(), "socket removed after stop");
        assert_eq!(gate.handle(Request::default()), Response::shutting_down());
        inst.close();
    }

    impl From<String> for Response {
        fn from(path: String) -> Self {
            Response {
                path,
                ..Response::default()
            }
        }
    }
}
