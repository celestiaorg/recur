//! A faithful port of `frontend/e2e/helpers/mock-bridge.ts`: every service
//! answered from the e2e seed, with the same in-memory mutations, canned
//! responses, emitted events and timing hooks, so a Rust view driven by it
//! reaches the state the Svelte view reaches under e2e and UI-parity runs.
//!
//! Like the TypeScript bridge it works at the JSON level. Each bound method
//! is a [`Resolver`] keyed by its Go name that gets the call's positional
//! arguments (as the Go-shaped JSON the Svelte UI sent), the mutable
//! [`Seed`], and the effective [`Table`] (so a batched method can delegate
//! to its single-item sibling and pick up a test's override). The typed
//! service traits encode arguments, dispatch, and decode the reply the way
//! Wails' `createFrom` did: `null` lists and maps become empty, a `null`
//! struct is a failed call, a `null` scalar is its zero value. A method with
//! no resolver answers `null`.
//!
//! ```no_run
//! # async fn demo() -> recur_services::Result<()> {
//! use recur_services::mock::{MockBridge, MockOptions, Reply};
//! let bridge = MockBridge::fresh(MockOptions::default().resolver("ListFiles", |_a, _s, _t| {
//!     Reply::value(serde_json::json!(["README.md"]))
//! }));
//! let services = bridge.services();
//! let mut events = services.subscribe();
//! let files = services.code.list_files("storefront", "LOOP-2229").await?;
//! # let _ = (files, events.try_recv());
//! # Ok(()) }
//! ```

mod attention;
mod catalog;
mod frontend;
mod js;
mod reply;
mod resolvers;
mod seed;
mod services;

pub mod fixtures;

use std::collections::HashMap;
use std::sync::Arc;

use futures::future::BoxFuture;
use parking_lot::Mutex;
use serde_json::Value;

pub use attention::ChatAttention;
pub use catalog::parse_toml_array_tables;
pub use seed::{ObjId, Seed};

use crate::events::{AppEvent, EventHub};
use crate::{Error, Result, Services};
use reply::FromReply;

/// A failed bound call (`new BridgeError(message)`): the UI sees a rejected
/// call with this message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeError {
    pub message: String,
}

impl BridgeError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Default for BridgeError {
    fn default() -> Self {
        Self::new("mock failure")
    }
}

/// What a resolver answers: a JSON value, a [`BridgeError`], or a reply that
/// arrives later (a resolver returning a promise; specs use it to hold a call
/// in flight).
pub enum Reply {
    Value(Value),
    Error(BridgeError),
    Later(BoxFuture<'static, Reply>),
}

impl Reply {
    pub fn value(v: impl Into<Value>) -> Self {
        Self::Value(v.into())
    }

    pub fn null() -> Self {
        Self::Value(Value::Null)
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::Error(BridgeError::new(message))
    }

    /// A reply held until `release` is called with the real one (the specs'
    /// deferred promise). Dropping `release` answers `null`.
    pub fn deferred() -> (Self, Release) {
        let (tx, rx) = tokio::sync::oneshot::channel::<Reply>();
        let later = Self::Later(Box::pin(async move { rx.await.unwrap_or(Reply::null()) }));
        (later, Release(tx))
    }

    async fn settle(mut self) -> std::result::Result<Value, BridgeError> {
        loop {
            match self {
                Self::Value(v) => return Ok(v),
                Self::Error(e) => return Err(e),
                Self::Later(f) => self = f.await,
            }
        }
    }

    /// The value a resolver sees when it calls a sibling synchronously and
    /// embeds the result: `JSON.stringify` turns a `BridgeError` into
    /// `{message}` and a pending promise into `{}`.
    pub fn embedded(self) -> Value {
        match self {
            Self::Value(v) => v,
            Self::Error(e) => serde_json::json!({ "message": e.message }),
            Self::Later(_) => Value::Object(Default::default()),
        }
    }
}

impl From<Value> for Reply {
    fn from(v: Value) -> Self {
        Self::Value(v)
    }
}

/// Completes a [`Reply::deferred`].
pub struct Release(tokio::sync::oneshot::Sender<Reply>);

impl Release {
    pub fn send(self, reply: Reply) {
        let _ = self.0.send(reply);
    }
}

/// Called with the Go name of a method that has no resolver.
pub type UnresolvedHook = Arc<dyn Fn(&str) + Send + Sync>;

/// One bound method: `(args, seed, table) -> reply`.
pub type Resolver = Arc<dyn Fn(&[Value], &mut Seed, &Table) -> Reply + Send + Sync>;

/// The effective resolvers: the base table, a test's overrides, and the
/// chat-attention wrappers `installBridge` adds.
#[derive(Clone, Default)]
pub struct Table {
    resolvers: HashMap<String, Resolver>,
}

impl Table {
    /// The base resolvers (`resolvers` in mock-bridge.ts).
    pub fn base() -> Self {
        resolvers::base()
    }

    pub fn get(&self, name: &str) -> Option<&Resolver> {
        self.resolvers.get(name)
    }

    pub fn insert(
        &mut self,
        name: &str,
        f: impl Fn(&[Value], &mut Seed, &Table) -> Reply + Send + Sync + 'static,
    ) {
        self.resolvers.insert(name.to_owned(), Arc::new(f));
    }

    /// Calls another resolver (`table.ListTree(args, seed, table)`); a missing
    /// one throws in the TypeScript, here it is a failed call.
    pub fn call(&self, name: &str, args: &[Value], seed: &mut Seed) -> Reply {
        match self.resolvers.get(name) {
            Some(f) => f(args, seed, self),
            None => Reply::error(format!("table.{name} is not a function")),
        }
    }
}

/// `BridgeOptions`: per-test resolver overrides and an unresolved-call hook.
#[derive(Clone, Default)]
pub struct MockOptions {
    pub resolvers: HashMap<String, Resolver>,
    pub on_unresolved: Option<UnresolvedHook>,
}

impl MockOptions {
    /// Overrides (or adds) one resolver.
    pub fn resolver(
        mut self,
        name: &str,
        f: impl Fn(&[Value], &mut Seed, &Table) -> Reply + Send + Sync + 'static,
    ) -> Self {
        self.resolvers.insert(name.to_owned(), Arc::new(f));
        self
    }

    pub fn on_unresolved(mut self, f: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.on_unresolved = Some(Arc::new(f));
        self
    }
}

/// The installed bridge (`installBridge(page, seed, opts)`): owns the seed,
/// the effective table, the chat-attention state and the event hub.
pub struct MockBridge {
    seed: Mutex<Seed>,
    attention: Arc<Mutex<attention::State>>,
    table: Table,
    events: EventHub,
    on_unresolved: Option<UnresolvedHook>,
}

impl MockBridge {
    /// Installs the bridge over `seed`. Resolvers mutate it in place; pass a
    /// clone of a shared seed (fixtures.ts clones per test).
    pub fn new(seed: Seed, opts: MockOptions) -> Arc<Self> {
        let attention = Arc::new(Mutex::new(attention::State::default()));
        let mut table = Table::base();
        for (name, f) in opts.resolvers {
            table.resolvers.insert(name, f);
        }
        attention::install(&mut table, &attention);
        Arc::new(Self {
            seed: Mutex::new(seed),
            attention,
            table,
            events: EventHub::new(),
            on_unresolved: opts.on_unresolved,
        })
    }

    /// A bridge over a fresh clone of the e2e seed (the fixtures.ts `bridge`
    /// auto fixture).
    pub fn fresh(opts: MockOptions) -> Arc<Self> {
        Self::new(fixtures::seed(), opts)
    }

    /// The services the app holds, all answered by this bridge.
    pub fn services(self: &Arc<Self>) -> Services {
        Services::new(self.clone(), self.events.clone())
    }

    pub fn events(&self) -> &EventHub {
        &self.events
    }

    /// Reads or edits the backend state (a spec poking `seed`).
    pub fn with_seed<R>(&self, f: impl FnOnce(&mut Seed) -> R) -> R {
        f(&mut self.seed.lock())
    }

    /// `mockChatAttention(page)`: focus, occlusion and unseen chat tabs as
    /// the engine would track them.
    pub fn attention(&self) -> ChatAttention {
        self.attention.lock().view.clone()
    }

    /// `window._wails.dispatchWailsEvent(e)`: a backend event, as specs push
    /// them.
    pub fn dispatch(&self, event: AppEvent) {
        self.events.publish(event);
    }

    /// [`dispatch`](Self::dispatch) from a wire name and JSON data.
    pub fn dispatch_wire(
        &self,
        name: &str,
        data: Value,
    ) -> std::result::Result<(), crate::events::WireError> {
        self.dispatch(AppEvent::from_wire(name, data)?);
        Ok(())
    }

    /// `simulateChatTurnIdle`: finish a chat turn the way
    /// `Engine.updateAgentStatus` does: idle the agent, and raise unseen plus
    /// `attention.updated` / `inbox.updated` only when the tab is not really
    /// visible (unfocused, or the chat UI is occluded).
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

    /// One bound call: dispatch by Go method name, settle the reply, then
    /// publish what the call queued (the bridge dispatched those after
    /// `route.fulfill`). A failed call publishes nothing; its queue waits for
    /// the next successful call, as in the TypeScript.
    pub(crate) async fn call<R: FromReply>(
        &self,
        name: &'static str,
        args: Vec<Value>,
    ) -> Result<R> {
        let reply = {
            let mut seed = self.seed.lock();
            match self.table.get(name) {
                Some(f) => f(&args, &mut seed, &self.table),
                None => {
                    if let Some(hook) = &self.on_unresolved {
                        hook(name);
                    }
                    Reply::null()
                }
            }
        };
        let value = reply.settle().await.map_err(|e| Error::new(e.message))?;
        let events = {
            let seed = self.seed.lock();
            let mut attn = self.attention.lock();
            attention::take_pending(&seed, &mut attn)
        };
        for e in events {
            self.events.publish(e);
        }
        R::from_reply(name, value)
    }
}
