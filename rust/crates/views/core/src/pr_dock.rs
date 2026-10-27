//! PR detail's agent dock: the bound workspace view's Observatory renders its left aside inside
//! PR detail. The workspace crate installs a [`PrDockHost`]; PR detail reaches the dock only
//! through it.

use gpui_kit::{AnyElement, AnyView, App, FocusHandle, Global, Subscription, Window};

/// The dock's hooks, installed by the crate that owns the bound workspace view.
pub struct PrDockHost {
    /// The Observatory of the workspace view on screen, once it has rendered.
    pub dock: fn(&App) -> Option<AnyView>,
    /// Renders `dock`'s left aside in its PR form.
    pub render: fn(&AnyView, &mut Window, &mut App) -> Option<AnyElement>,
    /// The dock's hide toggle.
    pub toggle_focus: fn(&AnyView, &App) -> Option<FocusHandle>,
    /// Calls `on_notify` whenever `dock` notifies (its elements render inside PR detail).
    pub observe: fn(&AnyView, Box<dyn Fn(&mut App)>, &mut App) -> Option<Subscription>,
}

impl Global for PrDockHost {}

/// The bound workspace's dock, when a host is installed and the dock has rendered.
pub fn dock(cx: &App) -> Option<(AnyView, &PrDockHost)> {
    let host = cx.try_global::<PrDockHost>()?;
    Some(((host.dock)(cx)?, host))
}
