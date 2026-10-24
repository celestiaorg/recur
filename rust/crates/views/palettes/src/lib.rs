//! The palettes App.svelte mounts over every route: Go to (⌘K, `CommandPalette.svelte`), Run
//! Command (⌘⇧P), Open in (⌘O), Run (⌘R), the language-mode picker and the keyboard
//! shortcuts list (⌘⇧/). Each reads its open flag from `AppState::overlays`, owns its keys
//! while its query field has focus and renders nothing while closed. The shell's global keys
//! and menu items open them; the Stop palette (⌘⇧R) is `recur-view-chat`'s.
//!
//! The Run Command palette orders recently run commands by
//! `recur_view_observatory::command_usage`, and the language picker applies picks through the
//! Observatory's `EditorEnv`, whose status bar opens it.

mod chrome;
pub mod command;
#[cfg(feature = "gallery")]
pub mod gallery;
pub mod open_in;
pub mod run;
pub mod run_command;
pub mod shortcuts;
#[cfg(test)]
mod spec;
pub mod syntax;

use gpui_kit::AppContext as _;
use recur_view_core::{OverlaySlot, ViewContext, ViewRegistry};

/// Registers every palette in its App.svelte overlay slot.
pub fn register(r: &mut ViewRegistry) {
    r.overlay(OverlaySlot::CommandPalette, |vcx: &ViewContext, window, cx| {
        let state = vcx.state.clone();
        cx.new(|cx| command::CommandPalette::new(state, window, cx))
            .into()
    });
    r.overlay(OverlaySlot::RunCommandPalette, |vcx: &ViewContext, window, cx| {
        let state = vcx.state.clone();
        cx.new(|cx| run_command::RunCommandPalette::new(state, window, cx))
            .into()
    });
    r.overlay(OverlaySlot::OpenInPalette, |vcx: &ViewContext, window, cx| {
        let state = vcx.state.clone();
        cx.new(|cx| open_in::OpenInPalette::new(state, window, cx))
            .into()
    });
    r.overlay(OverlaySlot::RunPalette, |vcx: &ViewContext, window, cx| {
        let state = vcx.state.clone();
        cx.new(|cx| run::RunPalette::new(state, window, cx)).into()
    });
    r.overlay(OverlaySlot::SyntaxPalette, |vcx: &ViewContext, window, cx| {
        let state = vcx.state.clone();
        cx.new(|cx| syntax::SyntaxPalette::new(state, window, cx))
            .into()
    });
    r.overlay(OverlaySlot::ShortcutsHelp, |vcx: &ViewContext, window, cx| {
        let state = vcx.state.clone();
        cx.new(|cx| shortcuts::ShortcutsHelp::new(state, window, cx))
            .into()
    });
}
