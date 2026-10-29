//! Every service call runs on the backend runtime: the UI's executor (which
//! is not a tokio context) awaits a task spawned on the runtime, so git, processes and
//! timers work and file I/O never runs on the UI thread.

use std::future::Future;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::AppServices;
use crate::{Error, Result};

/// Runs `$body` on the runtime with the listed arguments moved in as owned
/// values; the call keeps running if the caller stops waiting, as a Go
/// method without a context did.
macro_rules! rt {
    ($self:ident, $($a:ident),* => |$s:ident| $body:expr) => {{
        $(#[allow(clippy::clone_on_copy, clippy::useless_conversion)] let $a = $a.to_owned();)*
        $self.rt(move |$s| async move { $body }).await
    }};
}

/// [`rt!`] for cancellable methods: dropping
/// the call cancels `$c` and aborts the task.
macro_rules! rt_ctx {
    ($self:ident, $($a:ident),* => |$s:ident, $c:ident| $body:expr) => {{
        $(#[allow(clippy::clone_on_copy, clippy::useless_conversion)] let $a = $a.to_owned();)*
        $self.rt_ctx(move |$s, $c| async move { $body }).await
    }};
}

pub(crate) use {rt, rt_ctx};

fn join_error(e: tokio::task::JoinError) -> Error {
    if e.is_panic() {
        let p = e.into_panic();
        let msg = p
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| p.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panic".into());
        return Error::new(msg);
    }
    Error::new("context canceled")
}

/// Cancels and aborts the call's task when the caller drops it.
struct CancelOnDrop {
    token: CancellationToken,
    abort: tokio::task::AbortHandle,
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.token.cancel();
        self.abort.abort();
    }
}

impl AppServices {
    pub(crate) fn me(&self) -> Arc<AppServices> {
        self.me
            .upgrade()
            .expect("service calls run while the services are alive")
    }

    pub(crate) async fn rt<T, F, Fut>(&self, f: F) -> Result<T>
    where
        F: FnOnce(Arc<AppServices>) -> Fut,
        Fut: Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        self.handle
            .spawn(f(self.me()))
            .await
            .unwrap_or_else(|e| Err(join_error(e)))
    }

    pub(crate) async fn rt_ctx<T, F, Fut>(&self, f: F) -> Result<T>
    where
        F: FnOnce(Arc<AppServices>, CancellationToken) -> Fut,
        Fut: Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        let token = CancellationToken::new();
        let task = self.handle.spawn(f(self.me(), token.clone()));
        let _guard = CancelOnDrop {
            token,
            abort: task.abort_handle(),
        };
        task.await.unwrap_or_else(|e| Err(join_error(e)))
    }
}

impl AppServices {
    /// [`blocking`] from any thread (the UI executor included).
    pub(crate) async fn off_thread<T: Send + 'static>(
        &self,
        f: impl FnOnce() -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.handle
            .spawn_blocking(f)
            .await
            .unwrap_or_else(|e| Err(join_error(e)))
    }
}

/// Runs blocking file-system work (walks, searches, gzip) off the runtime's
/// worker threads.
pub(crate) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .unwrap_or_else(|e| Err(join_error(e)))
}
