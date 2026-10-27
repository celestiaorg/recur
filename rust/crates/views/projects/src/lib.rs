//! Project settings (`Project.svelte`; its `MiniScriptEditor` lives in `recur_view_core::mini`): one project's identity,
//! repository, run settings, schedules, pull request defaults, skill and MCP overrides.
//!
//! # Embedding
//!
//! Call [`register`] once. The Schedules section embeds the Schedules list
//! (`recur_view_widgets::schedules::ScheduledPrompts`).

mod color_panel;
#[cfg(feature = "gallery")]
pub mod gallery;
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
