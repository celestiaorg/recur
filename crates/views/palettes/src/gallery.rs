//! Gallery scenes: the `palettes` list in `crates/ui/gallery/scenes.json`: the whole shell
//! over the e2e seed edited by the scene's fixture in `content/views.json`, then the scene's
//! `route` once booted. Only the palettes are registered, so the main area stays empty.

use std::collections::BTreeMap;
use std::rc::Rc;

use gpui_kit::{AnyView, App, Window};
use recur_ui::gallery::SceneSpec;
use recur_view_core::ViewRegistry;
use recur_view_core::gallery::{SceneHook, fixture, parsed_in, reset, specs_in, view_scene};
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct PaletteScene {
    name: String,
    theme: String,
    fixture: String,
    route: String,
    storage: BTreeMap<String, String>,
}

fn parsed() -> Vec<Value> {
    parsed_in("palettes")
}

/// Every palette scene, in file order.
pub fn specs() -> Vec<SceneSpec> {
    specs_in("palettes")
}

pub fn exists(name: &str) -> bool {
    specs().iter().any(|s| s.name == name)
}

/// `route`: `palette`, `commands:{path}:{scope}`, `shortcuts`, `openin:{workspace}`,
/// `run:{workspace}`, `stop:{workspace}` (three runs started: test, dev, check) or
/// `syntax:{path}`. A path stands in for the focused editor. Open in and
/// Run open 300ms after the workspace (a scene `wait` lets the clock reach it), once git status
/// is in.
pub fn build(name: &str, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let raw = parsed().into_iter().find(|v| v["name"] == name)?;
    let scene: PaletteScene = serde_json::from_value(raw).ok()?;
    reset(cx);
    let mut registry = ViewRegistry::default();
    crate::register(&mut registry);
    let route = scene.route.clone();
    let navigate: SceneHook = Box::new(move |state, cx| {
        let mut parts = route.splitn(3, ':');
        let kind = parts.next().unwrap_or("").to_string();
        let arg = parts.next().unwrap_or("").to_string();
        let scope = parts.next().unwrap_or("").to_string();
        state.update(cx, |s, cx| {
            if matches!(kind.as_str(), "commands" | "syntax") {
                let focus = (scope.clone(), arg.clone());
                s.set_editor_focus_provider(Some(Rc::new(move |_| focus.clone())));
            }
            match kind.as_str() {
                "palette" => s.toggle_go_palette(cx),
                "commands" => s.toggle_commands_palette(cx),
                "shortcuts" => s.toggle_shortcuts(cx),
                "openin" | "run" | "stop" => s.open_workspace(&arg, false, cx),
                "syntax" => s.open_syntax_mode_picker(cx),
                _ => {}
            }
        })
    });
    // Landing on the workspace closes palettes: open them once it has.
    let route = scene.route.clone();
    let act: SceneHook = Box::new(move |state, cx| {
        let (kind, id) = route.split_once(':').unwrap_or((route.as_str(), ""));
        let (kind, id, state) = (kind.to_string(), id.to_string(), state.clone());
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(300))
                .await;
            state.update(cx, |s, cx| match kind.as_str() {
                "openin" => s.toggle_open_in(cx),
                "run" => {
                    s.open_run_menu(&id, cx);
                }
                "stop" => {
                    for (name, session) in [
                        ("test", "run-test"),
                        ("dev", "run-dev"),
                        ("check", "run-check"),
                    ] {
                        s.set_run_session(&id, name, session, cx);
                    }
                    s.open_stop_menu(&id, cx);
                }
                _ => {}
            })
        })
        .detach();
    });
    let (view, _) = view_scene(
        &fixture(&scene.fixture),
        &scene.theme,
        &scene.storage,
        registry,
        navigate,
        Some(act),
        window,
        cx,
    );
    Some(view)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scene_has_a_fixture_and_a_route() {
        for v in parsed() {
            let s: PaletteScene = serde_json::from_value(v).expect("scene");
            assert!(fixture(&s.fixture).is_object(), "{}", s.name);
            assert!(!s.route.is_empty(), "{}", s.name);
        }
    }
}
