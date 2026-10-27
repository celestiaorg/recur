//! The [`PrDockHost`] over the bound workspace view's Observatory.

use gpui_kit::{AnyElement, AnyView, App, Entity, FocusHandle, Subscription, Window};
use recur_view_core::pr_dock::PrDockHost;
use recur_view_observatory::Observatory;

use crate::detail::WorkspaceDetail;

pub(crate) fn host() -> PrDockHost {
    PrDockHost {
        dock,
        render,
        toggle_focus,
        observe,
    }
}

fn observatory(dock: &AnyView) -> Option<Entity<Observatory>> {
    dock.clone().downcast::<Observatory>().ok()
}

fn dock(cx: &App) -> Option<AnyView> {
    let detail = WorkspaceDetail::current(cx)?;
    let view = detail.read(cx).bound_view(cx)?;
    Some(view.read(cx).observatory().clone().into())
}

fn render(dock: &AnyView, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    observatory(dock)?.update(cx, |o, cx| o.render_pr_agent_dock(window, cx))
}

fn toggle_focus(dock: &AnyView, cx: &App) -> Option<FocusHandle> {
    observatory(dock)?.read(cx).pr_dock_toggle_focus()
}

fn observe(dock: &AnyView, on_notify: Box<dyn Fn(&mut App)>, cx: &mut App) -> Option<Subscription> {
    let obs = observatory(dock)?;
    Some(cx.observe(&obs, move |_, cx| {
        recur_ui::guard::catch("pr dock observe", || on_notify(cx));
    }))
}
