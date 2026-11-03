//! Recur's persistence: SQLite (`recur.db`), schema migrations, the managed
//! data-dir paths, the app log, and the demo seed.
//!
//! [`Store`] holds the in-memory working set (projects, workspaces, agents,
//! feed, findings, PRs, escalations, inbox, policy, settings) and writes every
//! mutation through to SQLite. Sessions and schedules live only in SQLite. The
//! database file, its JSON columns and the `__migrations` bookkeeping are
//! stable, and [`Store::open`] refuses a database that recorded migrations this
//! build does not ship.
//!
//! Getters whose result can be `null` return `Option` (`None` is JSON `null`),
//! following `recur-domain`.
//!
//! # Locking
//!
//! - `state`: one store-wide `RwLock`. Getters read-lock; mutations write-lock
//!   and write through to SQLite while holding it, DB first so a failed write
//!   leaves memory unchanged. Mutation callbacks (`update_workspace`,
//!   `update_pr`, …) run under it and must not call back into the store.
//! - feed: `prepend_feed` / `upsert_feed` / `patch_feed_by_tool_use_id` take
//!   a feed mutex, edit memory under `state`, release it, then write SQLite,
//!   so feed writes are ordered without stalling readers. Order: feed →
//!   state.
//! - sessions: session writes that allocate an idx (and forks, deletes,
//!   patches) take a session mutex; reads take none. `delete_closed_chat`
//!   takes session → state.
//! - the connection: a single SQLite connection behind a mutex. A transaction
//!   holds it from begin to commit. `append_event` takes no store lock.

pub mod applog;
mod db;
mod error;
mod migrations;
pub mod paths;
mod schedules;
mod seed;
mod seed_prs;
pub mod seeddump;
mod sessions;
mod store;

pub use error::{Error, Result};
pub use migrations::latest_schema_version;
pub use schedules::{ACTIVE_RUN_FILTER, RUN_SELECT};
pub use store::{PrKey, Store};

#[cfg(test)]
mod testenv;
#[cfg(test)]
mod tests;
