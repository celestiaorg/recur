//! The first-run path and the creation flows: Welcome, New workspace (with its modal
//! flows) and New project.
//!
//! # Embedding
//!
//! Call [`register`] after `recur_view_chat::register`. It registers the `welcome` route and
//! the New workspace and New project modals in their overlay slots; each shows itself from
//! `AppState::overlays`.

mod dialog;
pub mod new_project;
pub mod new_task;
pub mod new_workspace;
pub mod schedule_editor;
#[cfg(test)]
mod spec;
pub mod welcome;
mod wordmark;

use recur_view_core::{OverlaySlot, ViewContext, ViewName, ViewRegistry};

/// Registers the Welcome route and the creation modals.
pub fn register(r: &mut ViewRegistry) {
    r.view(&[ViewName::Welcome], |vcx: &ViewContext, window, cx| {
        welcome::build(vcx, window, cx).into()
    });
    r.overlay(OverlaySlot::NewTask, |vcx: &ViewContext, window, cx| {
        new_task::build(vcx, window, cx).into()
    });
    r.overlay(
        OverlaySlot::ScheduleEditor,
        |vcx: &ViewContext, window, cx| schedule_editor::build(vcx, window, cx).into(),
    );
    r.overlay(OverlaySlot::NewProject, |vcx: &ViewContext, window, cx| {
        new_project::build(vcx, window, cx).into()
    });
}
