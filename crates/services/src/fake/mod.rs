//! Typed in-memory fakes of every service, so view specs and the desktop's
//! `fake` feature run without a backend.
//!
//! [`Fake`] implements each service trait over a typed [`Seed`] (the e2e
//! store dump plus what calls add as they run). Every method has a default
//! (seed reads, in-memory mutations, canned answers, or an empty value) and an
//! optional per-method override in [`Overrides`]: a closure over the seed and
//! the call's owned arguments that answers now or later ([`hold`]).
//!
//! ```no_run
//! # async fn demo() -> recur_services::Result<()> {
//! use recur_services::fake::{Fake, Overrides};
//! let fake = Fake::fresh(Overrides::default().list_files(|_, _| Ok(vec!["README.md".into()])));
//! let services = fake.services();
//! let files = services.code.list_files("storefront", "LOOP-2229").await?;
//! # let _ = files;
//! # Ok(()) }
//! ```

mod attention;
mod seed;
mod services;

use std::future::Future;
use std::sync::Arc;

use futures::future::BoxFuture;
use parking_lot::Mutex;

pub use attention::ChatAttention;
pub use seed::Seed;
pub use services::Overrides;

use crate::Services;
use crate::events::{AppEvent, EventHub};

/// What an override answers: a result now, or one that arrives later.
pub enum Answer<R> {
    Now(R),
    Later(BoxFuture<'static, R>),
}

impl<R> From<R> for Answer<R> {
    fn from(r: R) -> Self {
        Self::Now(r)
    }
}

impl<R: Send + 'static> Answer<R> {
    pub fn later(f: impl Future<Output = R> + Send + 'static) -> Self {
        Self::Later(Box::pin(f))
    }

    async fn settle(self) -> R {
        match self {
            Self::Now(r) => r,
            Self::Later(f) => f.await,
        }
    }
}

/// An answer held in flight until [`Release::send`]. A dropped release leaves
/// the call pending.
pub fn hold<R: Send + 'static>() -> (Answer<R>, Release<R>) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let later = Answer::later(async move {
        match rx.await {
            Ok(r) => r,
            Err(_) => std::future::pending().await,
        }
    });
    (later, Release(tx))
}

/// Completes a [`hold`].
pub struct Release<R>(tokio::sync::oneshot::Sender<R>);

impl<R> Release<R> {
    pub fn send(self, r: R) {
        let _ = self.0.send(r);
    }
}

/// A borrowed service argument as the owned value an override receives.
pub trait Own {
    type O;
    fn own(self) -> Self::O;
}

impl<T: ToOwned + ?Sized> Own for &T {
    type O = T::Owned;
    fn own(self) -> T::Owned {
        self.to_owned()
    }
}

macro_rules! own_by_value {
    ($($t:ty),*) => {$(
        impl Own for $t {
            type O = $t;
            fn own(self) -> $t {
                self
            }
        }
    )*};
}

own_by_value!(bool, i64, Option<bool>);

/// Generates [`Overrides`] (one slot and builder per method), the default
/// bodies, and each service trait's impl on [`Fake`]: the override when set,
/// else the default; a `before` block runs first either way (the attention
/// bookkeeping the engine does around a call). A successful call then
/// publishes the events it queued.
macro_rules! fake_services {
    ($(
        $svc:ident {$(
            async fn $m:ident(&$s:ident $(, $p:ident: $t:ty)* $(,)?) -> $ret:ty
            $(, before { $($pre:tt)* })?
            $body:block
        )*}
    )*) => {
        #[allow(unused_parens)]
        /// Per-method overrides: `Overrides::default().read_file(|seed, (project, ws, path)| …)`.
        /// One-argument methods get the argument itself, none get `()`.
        #[derive(Clone, Default)]
        pub struct Overrides {$($(
            $m: Option<std::sync::Arc<dyn Fn(&mut Seed, ($(<$t as Own>::O),*)) -> Answer<$ret> + Send + Sync>>,
        )*)*}

        #[allow(unused_parens)]
        impl Overrides {$($(
            pub fn $m<A: Into<Answer<$ret>>>(
                mut self,
                f: impl Fn(&mut Seed, ($(<$t as Own>::O),*)) -> A + Send + Sync + 'static,
            ) -> Self {
                self.$m = Some(std::sync::Arc::new(move |s, a| f(s, a).into()));
                self
            }
        )*)*}

        #[async_trait::async_trait]
        #[allow(clippy::too_many_arguments)]
        trait Defaults {$($(
            async fn $m(&self $(, $p: $t)*) -> $ret;
        )*)*}

        #[async_trait::async_trait]
        impl Defaults for Fake {$($(
            #[allow(unused_variables)]
            async fn $m(&$s $(, $p: $t)*) -> $ret $body
        )*)*}

        $(
            #[async_trait::async_trait]
            impl crate::api::$svc for Fake {$(
                async fn $m(&$s $(, $p: $t)*) -> $ret {
                    $($($pre)*)?
                    let answer = match &$s.overrides.$m {
                        Some(f) => {
                            let args = ($(Own::own($p)),*);
                            let mut seed = $s.seed.lock();
                            f(&mut seed, args)
                        }
                        None => Answer::Now(Defaults::$m($s $(, $p)*).await),
                    };
                    let r = answer.settle().await;
                    if r.is_ok() {
                        $s.publish_queued();
                    }
                    r
                }
            )*}
        )*
    };
}

pub(crate) use fake_services;

/// Every service over one typed [`Seed`], with chat attention tracked the way
/// the engine does and an event hub specs push to.
pub struct Fake {
    seed: Mutex<Seed>,
    attention: Mutex<attention::State>,
    overrides: Overrides,
    events: EventHub,
}

impl Fake {
    /// A fake over `seed`. Defaults mutate it in place.
    pub fn new(seed: Seed, overrides: Overrides) -> Arc<Self> {
        Arc::new(Self {
            seed: Mutex::new(seed),
            attention: Mutex::default(),
            overrides,
            events: EventHub::new(),
        })
    }

    /// A fake over a fresh copy of the e2e seed.
    pub fn fresh(overrides: Overrides) -> Arc<Self> {
        Self::new(Seed::e2e(), overrides)
    }

    /// The services the app holds, all answered by this fake.
    pub fn services(self: &Arc<Self>) -> Services {
        Services::new(self.clone(), self.events.clone())
    }

    pub fn events(&self) -> &EventHub {
        &self.events
    }

    /// Reads or edits the backend state.
    pub fn with_seed<R>(&self, f: impl FnOnce(&mut Seed) -> R) -> R {
        f(&mut self.seed.lock())
    }

    /// Focus, occlusion and unseen chat tabs as the engine would track them.
    pub fn attention(&self) -> ChatAttention {
        self.attention.lock().view.clone()
    }

    /// Publishes a backend event.
    pub fn dispatch(&self, event: AppEvent) {
        self.events.publish(event);
    }

    /// Finishes a chat turn the way `Engine.updateAgentStatus` does: idles
    /// the agent, and raises unseen plus `attention.updated` /
    /// `inbox.updated` only when the tab is not really visible (unfocused, or
    /// the chat UI is occluded).
    pub fn simulate_chat_turn_idle(&self, workspace_id: &str, code: &str) {
        let events = {
            let mut seed = self.seed.lock();
            let mut attn = self.attention.lock();
            attention::turn_idle(&mut seed, &mut attn, workspace_id, code)
        };
        for e in events {
            self.events.publish(e);
        }
    }

    fn publish_queued(&self) {
        let events = {
            let seed = self.seed.lock();
            let mut attn = self.attention.lock();
            attention::take_pending(&seed, &mut attn)
        };
        for e in events {
            self.events.publish(e);
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut Seed) -> R) -> R {
        f(&mut self.seed.lock())
    }
}
