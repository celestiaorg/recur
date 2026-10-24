//! Project settings (`Project.svelte` with `MiniScriptEditor.svelte`): one project's identity,
//! repository, run settings, schedules, pull request defaults, skill and MCP overrides.
//!
//! # Embedding
//!
//! Call [`register`] after `recur_view_briefing::register` (the Schedules section embeds its
//! list).

mod color_panel;
#[cfg(feature = "gallery")]
pub mod gallery;
pub mod mini;
pub mod pic;
pub mod project;
#[cfg(test)]
mod spec;

use recur_view_core::{ViewContext, ViewName, ViewRegistry};

/// Registers the `project` route.
pub fn register(r: &mut ViewRegistry) {
    r.view(&[ViewName::Project], |vcx: &ViewContext, window, cx| {
        project::build(vcx, window, cx).into()
    });
}
