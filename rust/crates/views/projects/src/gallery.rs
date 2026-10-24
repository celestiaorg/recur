//! Parity scenes: the `projects` list in `rust/tools/ui-parity/scenes.json` (project settings
//! and its embedded schedules), built like the home scenes
//! (`recur_view_home::gallery::build_in`).

use gpui_kit::{AnyView, App, Window};
use recur_ui::gallery::SceneSpec;

pub fn specs() -> Vec<SceneSpec> {
    recur_view_home::gallery::specs_in("projects")
}

pub fn exists(name: &str) -> bool {
    specs().iter().any(|s| s.name == name)
}

pub fn build(name: &str, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    recur_view_home::gallery::build_in(
        "projects",
        name,
        &|r| {
            recur_view_briefing::register(r);
            crate::register(r);
        },
        window,
        cx,
    )
}
