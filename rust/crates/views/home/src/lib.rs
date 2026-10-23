//! The first-run path and the creation flows: Welcome (`Welcome.svelte`), New workspace
//! (`NewTask.svelte` with the modal flows of `lib/new-workspace.ts`) and New project
//! (`NewProject.svelte`), with `_welcome.scss`.
//!
//! # Embedding
//!
//! Call [`register`] after `recur_view_chat::register`. It registers the `welcome` route and
//! the New workspace and New project modals in their overlay slots; each shows itself from
//! `AppState::overlays`.

#[cfg(feature = "gallery")]
pub mod gallery;
mod dialog;
pub mod new_project;
pub mod welcome;
mod wordmark;

use recur_view_core::{OverlaySlot, ViewContext, ViewName, ViewRegistry};

/// Registers the Welcome route and the creation modals.
pub fn register(r: &mut ViewRegistry) {
    r.view(&[ViewName::Welcome], |vcx: &ViewContext, window, cx| {
        welcome::build(vcx, window, cx).into()
    });
    r.overlay(OverlaySlot::NewProject, |vcx: &ViewContext, window, cx| {
        new_project::build(vcx, window, cx).into()
    });
}
