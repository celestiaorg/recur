//! PR detail inside a workspace: the Observatory's pull request view is PR detail bound to the
//! workspace's current PR. The PR crate installs a [`PrViewHost`]; the Observatory reaches PR
//! detail only through it.

use gpui_kit::{AnyView, App, Entity, Global, Window};

use crate::AppState;

/// PR detail's tab row and the active tab's pane.
#[derive(Clone)]
pub struct PrViewParts {
    pub head: AnyView,
    pub body: AnyView,
}

/// Builds PR detail for the workspace's current PR (`pr_for_workspace`).
pub struct PrViewHost {
    pub mount: fn(&Entity<AppState>, &str, &mut Window, &mut App) -> PrViewParts,
}

impl Global for PrViewHost {}

pub fn mount(
    state: &Entity<AppState>,
    workspace_id: &str,
    window: &mut Window,
    cx: &mut App,
) -> Option<PrViewParts> {
    let mount = cx.try_global::<PrViewHost>()?.mount;
    Some(mount(state, workspace_id, window, cx))
}
