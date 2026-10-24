//! Settings (`Settings.svelte`), with `CompatibleProviderCard.svelte`,
//! `CompatibleProviderModal.svelte`, `lib/compatible-providers.ts`, the catalogue rules of
//! `lib/providers.ts` and `lib/mono-fonts.ts`.
//!
//! # Embedding
//!
//! Call [`register`] after `recur_view_chat::register`. It registers the `settings` route (the
//! split nav and the active pane, `AppState::settings_section`) and the Add provider modal in
//! its overlay slot. The shell already routes ⌘, and the Settings… menu item to the route.

pub mod compatible;
pub mod field;
#[cfg(feature = "gallery")]
pub mod gallery;
pub mod mono_fonts;
pub mod nav;
pub mod rules;
pub mod sections;
#[cfg(test)]
mod spec;
pub mod style;
pub mod view;

use gpui_kit::{
    AnyView, AppContext as _, Context, Global, IntoElement, ParentElement, Render, Styled, Window,
    div,
};
use recur_view_core::{OverlaySlot, ViewContext, ViewName, ViewRegistry};

use crate::view::SettingsView;

/// The open Add provider modal, shown in its overlay slot.
#[derive(Default)]
pub struct ModalSlot(pub Option<AnyView>);

impl Global for ModalSlot {}

/// The overlay that shows [`ModalSlot`].
struct ModalHost;

impl ModalHost {
    fn render_view(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let mut d = div().absolute().top_0().left_0().size_full();
        match cx.try_global::<ModalSlot>().and_then(|s| s.0.clone()) {
            Some(v) => d = d.child(v),
            None => return div().into_any_element(),
        }
        d.into_any_element()
    }
}

impl Render for ModalHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

/// Registers the Settings route and its modal overlay.
pub fn register(r: &mut ViewRegistry) {
    r.view(&[ViewName::Settings], |vcx: &ViewContext, window, cx| {
        cx.new(|cx| SettingsView::new(vcx, window, cx)).into()
    });
    r.overlay(OverlaySlot::CompatibleProvider, |_: &ViewContext, _, cx| {
        cx.new(|cx: &mut Context<ModalHost>| {
            cx.observe_global::<ModalSlot>(|_, cx| cx.notify()).detach();
            ModalHost
        })
        .into()
    });
}
