//! Recur's domain model: projects, workspaces, sessions, loops, findings,
//! schedules, settings and their IDs. Pure data and rules, no I/O.
//!
//! Every type serializes with stable JSON field names, so `recur.db` JSON
//! columns and view-state files round-trip as equal JSON values:
//!
//! - String enums are [`string_enum!`] types whose `Other(String)` variant
//!   keeps unknown and empty values.
//! - `omitempty` fields skip their zero value; other fields always
//!   serialize. A non-`omitempty` slice or map is `Option<…>` because it can be
//!   `null`.
//! - Plain string consts (chat modes, block types, …) stay `&str` consts
//!   and the matching fields stay `String`.

#[macro_use]
mod macros;

mod cron;
mod serde_util;
mod text;

pub mod agent;
pub mod artifact;
pub mod briefing;
pub mod compatible;
pub mod escalation;
pub mod finding;
pub mod ids;
pub mod json;
pub mod mcp;
pub mod merge;
pub mod model;
pub mod observatory;
pub mod path;
pub mod plan;
pub mod policy;
pub mod pr;
pub mod project;
pub mod providers;
pub mod reaction;
pub mod schedule;
pub mod session;
pub mod settings;
pub mod shortid;
pub mod skills;
pub mod transcript;
pub mod workspace;

pub use agent::*;
pub use artifact::*;
pub use briefing::*;
pub use compatible::*;
pub use escalation::*;
pub use finding::*;
pub use ids::*;
pub use mcp::*;
pub use merge::*;
pub use model::*;
pub use observatory::*;
pub use plan::*;
pub use policy::*;
pub use pr::*;
pub use project::*;
pub use reaction::*;
pub use schedule::*;
pub use session::*;
pub use settings::*;
pub use skills::*;
pub use text::html_escape;
pub use transcript::*;
pub use workspace::*;
