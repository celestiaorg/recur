//! The layering rules of `docs/ARCHITECTURE.md` ("Layering"), checked by this crate's tests
//! against every workspace manifest and the engine's sources:
//!
//! - a view crate (`crates/views/*`) depends on no other view crate except `recur-view-core`
//!   and `recur-view-widgets`, and may embed panes (`crates/panes/*`);
//! - `recur-view-core` depends on no view crate or pane, `recur-view-widgets` only on
//!   `recur-view-core`, and a pane only on those two and the panes it embeds ([`PANE_EMBEDS`]);
//! - no view crate or pane depends on `recur-app` or a binary;
//! - nothing from `recur-services` down depends on GPUI;
//! - `recur-engine` depends on no concrete provider crate, names no concrete forge or tracker
//!   module, and spells no provider, forge or tracker id.
//!
//! Dev-dependencies are exempt: specs mount the whole shell. The `gallery` feature is not, so
//! a scene reaches the shell and the other views only through `recur_view_core::gallery`.

/// The panes a pane embeds (the Observatory's Review tab is the review surface).
pub const PANE_EMBEDS: &[(&str, &[&str])] = &[("recur-view-observatory", &["recur-view-review"])];

#[cfg(test)]
mod tests;
