//! Parity scenes: the `home` list in `rust/tools/ui-parity/scenes.json`, built the way
//! `web/scenes/Views.svelte` mounts `App.svelte` (`recur_view_core::gallery::build_in`).

use gpui_kit::{AnyView, App, Window};
use recur_ui::gallery::SceneSpec;
use recur_view_core::gallery::{build_in, specs_in};

/// Every home scene, in file order.
pub fn specs() -> Vec<SceneSpec> {
    specs_in("home")
}

pub fn exists(name: &str) -> bool {
    specs().iter().any(|s| s.name == name)
}

pub fn build(name: &str, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    build_in("home", name, window, cx)
}
