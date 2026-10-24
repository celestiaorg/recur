//! Parity scenes: the `palettes` list in `rust/tools/ui-parity/scenes.json`, built the way
//! `web/scenes/Views.svelte` mounts `App.svelte`: the whole shell over the e2e seed edited by
//! the scene's fixture in `content/views.json`, then the scene's `route` once booted. Only the
//! palettes are registered, so the main area stays empty as the reference hides it.

use std::collections::BTreeMap;
use std::rc::Rc;

use gpui_kit::{AnyView, App, Window};
use recur_ui::gallery::SceneSpec;
use recur_view_core::ViewRegistry;
use serde::Deserialize;
use serde_json::Value;

const SCENES_JSON: &str = include_str!("../../../../tools/ui-parity/scenes.json");
const FIXTURES: &str = include_str!("../../../../tools/ui-parity/content/views.json");

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
    serde_json::from_str::<Value>(SCENES_JSON)
        .ok()
        .and_then(|v| v.get("palettes").and_then(|l| l.as_array()).cloned())
        .unwrap_or_default()
}

/// Every palette scene, in file order.
pub fn specs() -> Vec<SceneSpec> {
    parsed()
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect()
}

pub fn exists(name: &str) -> bool {
    specs().iter().any(|s| s.name == name)
}

fn fixture(name: &str) -> Value {
    serde_json::from_str::<Value>(FIXTURES).unwrap_or(Value::Null)["fixtures"][name].clone()
}

/// `route`: `palette`, `commands:{path}:{scope}`, `shortcuts`, `openin:{workspace}`,
/// `run:{workspace}` or `syntax:{path}`. A path stands in for the focused editor. Open in and
/// Run open 300ms after the workspace (a scene `wait` lets the clock reach it), once git status
/// is in.
pub fn build(name: &str, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let raw = parsed().into_iter().find(|v| v["name"] == name)?;
    let scene: PaletteScene = serde_json::from_value(raw).ok()?;
    recur_view_observatory::gallery::reset(cx);
    let mut registry = ViewRegistry::default();
    crate::register(&mut registry);
    let route = scene.route.clone();
    let navigate: recur_app::gallery::SceneHook = Box::new(move |state, cx| {
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
                "openin" | "run" => s.open_workspace(&arg, false, cx),
                "syntax" => s.open_syntax_mode_picker(cx),
                _ => {}
            }
        })
    });
    // Landing on the workspace closes palettes: open them once it has.
    let route = scene.route.clone();
    let act: recur_app::gallery::SceneHook = Box::new(move |state, cx| {
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
                _ => {}
            })
        })
        .detach();
    });
    let (view, _) = recur_app::gallery::view_scene(
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
