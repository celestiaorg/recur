//! Orchestration: chats, loops, walker, governor, maestro, schedules, workspace lifecycle,
//! event bus, recovery.
//!
//! Port of Go `internal/engine`, built slice by slice (`docs/engine-port-plan.md`).
//! E1 (engine core) provides the [`Engine`] and its shared state, the typed
//! [`bus`], the [`governor`], the chat concurrency kernel ([`chatlife`]), the
//! loop control plane ([`loop_handle`], [`loop_registry`]), attention and the
//! chat inbox, workspace lifecycle ([`workspace`]), the worktree watcher
//! ([`fswatch`]), roles and guards, and the E1 [`host`] methods. Calls into
//! later slices go through the named hook points in `deferred.rs`.

pub mod attention;
pub mod bus;
pub mod chatlife;
pub mod codes;
mod core;
mod deferred;
pub mod fswatch;
pub mod governor;
pub mod guard;
pub mod host;
pub mod lifecycle;
pub mod loop_handle;
pub mod loop_registry;
pub mod publish;
pub mod registry;
pub mod roles;
pub mod session_config;
pub mod simplify;
pub mod terminals;
pub mod text;
pub mod view;
pub mod workspace;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

#[cfg(test)]
mod golden_tests;

pub use crate::core::{Engine, EngineError, GitSetup, Result, ScheduleControl, Tasks};
pub use bus::{Bus, EngineEvent};
pub use registry::{ForgeRegistry, ProviderRegistry, TrackerRegistry};
