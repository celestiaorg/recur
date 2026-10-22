//! The workspace view (`WorkspaceView.svelte`, `WorkspaceDetail.svelte`): the Observatory with
//! the agent chat in its left dock, the chat tab strip, the dock's mode / model / loop pickers,
//! the plan panel and head, the worktree setup states, and the detail route that binds a
//! workspace (or a PR's agent workspace) to it.
//!
//! # Embedding
//!
//! Call [`register`] on the shell's registry: it registers the `detail` route (kept laid out
//! while another route shows, as `#v-detail`). [`detail::WorkspaceDetail`] binds the selected
//! workspace and keeps the previous one mounted as a parked hot shell;
//! [`detail::WorkspaceDetail::bound_view`] is the [`view::WorkspaceView`] on screen (PR detail
//! renders its agent dock through the view's Observatory, `render_pr_agent_dock`).
//!
//! The view reads backend events after `AppState` applied them
//! ([`recur_view_core::AppliedEvent`]), so store-derived checks (a tab's agent running) see
//! the same state the Svelte listeners did.

pub mod detail;
#[cfg(feature = "gallery")]
pub mod gallery;
mod head;
mod overlay;
pub mod plan_head;
pub mod plan_panel;
pub mod tab_order;
mod toolbar;
pub mod uuid;
pub mod view;

#[cfg(test)]
mod spec;

use gpui_kit::AppContext as _;
use recur_view_core::{ViewContext, ViewName, ViewRegistry};

pub use detail::WorkspaceDetail;
pub use view::{WorkspaceProps, WorkspaceView};

/// Registers the detail route.
pub fn register(r: &mut ViewRegistry) {
    r.view(&[ViewName::Detail], |vcx: &ViewContext, window, cx| {
        cx.new(|cx| WorkspaceDetail::new(vcx, window, cx)).into()
    })
    .keep_laid_out();
}
