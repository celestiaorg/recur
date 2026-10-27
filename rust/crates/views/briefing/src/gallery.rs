//! Parity scenes: the `briefing` list in `rust/tools/ui-parity/scenes.json` (Inbox, Issues,
//! Schedules), built like the home scenes (`recur_view_core::gallery::build_in`).

use gpui_kit::{AnyView, App, Window};
use recur_ui::gallery::SceneSpec;

pub fn specs() -> Vec<SceneSpec> {
    recur_view_core::gallery::specs_in("briefing")
}

pub fn exists(name: &str) -> bool {
    specs().iter().any(|s| s.name == name)
}

pub fn build(name: &str, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    crate::issues::reset_saved_view(cx);
    cx.set_global(crate::schedule_expanded::ScheduleExpanded::default());
    recur_view_core::gallery::build_in("briefing", name, window, cx)
}
