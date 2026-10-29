//! Orchestration: chats, loops, walker, governor, maestro, schedules, workspace lifecycle,
//! event bus, recovery.
//!
//! The [`Engine`] and its shared state, the typed [`bus`], the [`governor`],
//! the chat concurrency kernel ([`chatlife`]), the loop control plane
//! ([`loop_handle`], [`loop_registry`]), attention and the chat inbox,
//! workspace lifecycle ([`workspace`]), the worktree watcher ([`fswatch`]),
//! roles and guards, the [`host`] methods, the turn pipeline, loops, chat and
//! coordination, and pull requests with PR chat hosts ([`prs`], [`hoist`]).

pub mod app_chat;
pub mod artifacts;
pub mod ask_user;
pub mod attachments;
pub mod attention;
pub mod briefing;
pub mod bus;
pub mod chat;
pub mod chat_shared;
pub mod chatlife;
pub mod codes;
pub mod composer;
mod core;
mod deadline;
pub mod feed;
pub mod fswatch;
pub mod governor;
pub mod guard;
pub mod hoist;
pub mod host;
pub mod lifecycle;
pub mod loop_handle;
pub mod loop_registry;
pub mod loops;
pub mod mcp_attach;
pub mod permissions;
pub mod plan;
pub mod prs;
pub mod publish;
pub mod registry;
pub mod renames;
pub mod roles;
pub mod schedules;
pub mod session_config;
pub mod simplify;
pub mod terminals;
pub mod text;
pub mod turn;
pub mod view;
pub mod workspace;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

#[cfg(test)]
mod golden_e2_tests;
#[cfg(test)]
mod golden_e3_tests;
#[cfg(test)]
mod golden_e4_tests;
#[cfg(test)]
mod golden_e5_tests;
#[cfg(test)]
mod golden_tests;

pub use crate::core::{Engine, EngineError, GitSetup, Result, ScheduleControl, Tasks};
pub use bus::{Bus, EngineEvent};
pub use registry::{ForgeRegistry, ProviderRegistry, TrackerRegistry};
