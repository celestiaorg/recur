//! Application services: the UI-facing API over the engine (the former Wails-bound services)
//! plus typed app events.
//!
//! - [`api`]: one async trait per service ([`api::WorkspacesService`], …), the
//!   `app` package DTOs, and [`Services`], the aggregate views hold.
//! - [`events`]: [`AppEvent`] and the [`EventHub`] behind
//!   [`Services::subscribe`].
//! - `mock` (feature `mock`): the e2e mock bridge, serving the e2e seed.

pub mod api;
pub mod app;
pub mod events;
#[cfg(feature = "mock")]
pub mod mock;

mod error;

pub use api::{Backend, Services};
pub use app::{AppConfig, AppOptions, AppServices};
pub use error::{Error, Result};
pub use events::{AppEvent, EventHub};
