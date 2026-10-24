//! Parity scenes: the `home` list in `rust/tools/ui-parity/scenes.json`, built the way
//! `web/scenes/Views.svelte` mounts `App.svelte`: the whole shell over the e2e seed edited by
//! the scene's fixture in `content/views.json`, then the scene's `route` once booted. The
//! briefing and projects galleries build their lists through [`build_in`].

use std::collections::BTreeMap;

use gpui_kit::{AnyView, App, Window};
use recur_ui::gallery::SceneSpec;
use recur_view_core::{ViewName, ViewRegistry};
use serde::Deserialize;
use serde_json::Value;

const SCENES_JSON: &str = include_str!("../../../../tools/ui-parity/scenes.json");
const FIXTURES: &str = include_str!("../../../../tools/ui-parity/content/views.json");

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct HomeScene {
    name: String,
    theme: String,
    fixture: String,
    route: String,
    storage: BTreeMap<String, String>,
}

/// The scenes of one list of `scenes.json` (`home`, `briefing`, `projects`).
pub fn parsed_in(list: &str) -> Vec<Value> {
    serde_json::from_str::<Value>(SCENES_JSON)
        .ok()
        .and_then(|v| v.get(list).and_then(|l| l.as_array()).cloned())
        .unwrap_or_default()
}

/// Every scene of `list`, in file order.
pub fn specs_in(list: &str) -> Vec<SceneSpec> {
    parsed_in(list)
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect()
}

/// Every home scene, in file order.
pub fn specs() -> Vec<SceneSpec> {
    specs_in("home")
}

pub fn exists(name: &str) -> bool {
    specs().iter().any(|s| s.name == name)
}

/// A fixture of `content/views.json`.
pub fn fixture(name: &str) -> Value {
    serde_json::from_str::<Value>(FIXTURES).unwrap_or(Value::Null)["fixtures"][name].clone()
}

/// The registry the desktop builds, as far as these scenes need it.
pub fn registry() -> ViewRegistry {
    let mut r = ViewRegistry::default();
    recur_view_chat::register(&mut r);
    recur_view_review::register(&mut r);
    recur_view_observatory::register(&mut r);
    recur_view_workspace::register(&mut r);
    recur_view_prs::register(&mut r);
    recur_view_settings::register(&mut r);
    crate::register(&mut r);
    r
}

struct InitsRan;
impl gpui_kit::Global for InitsRan {}

/// Clears the app globals view crates keep, since scenes share one app.
pub fn reset(cx: &mut App) {
    recur_view_observatory::gallery::reset(cx);
    cx.set_global(recur_view_chat::transcript_ui::TranscriptUiCache::default());
    cx.set_global(recur_view_chat::heap::ChatHeapCache::default());
    cx.set_global(recur_view_chat::strip::ChatStripCache::default());
    cx.set_global(recur_view_chat::hoist::HoistState::default());
    cx.set_global(recur_content::webhost::BlockingRenders);
}

pub fn build(name: &str, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    build_in("home", name, &|_| {}, window, cx)
}

/// A scene of `list`, the shell's registry extended by `extra` (the crates that sit above
/// this one).
pub fn build_in(
    list: &str,
    name: &str,
    extra: &dyn Fn(&mut ViewRegistry),
    window: &mut Window,
    cx: &mut App,
) -> Option<AnyView> {
    let raw = parsed_in(list).into_iter().find(|v| v["name"] == name)?;
    let scene: HomeScene = serde_json::from_value(raw).ok()?;
    reset(cx);
    let mut registry = registry();
    extra(&mut registry);
    if cx.try_global::<InitsRan>().is_none() {
        cx.set_global(InitsRan);
        for init in &registry.inits {
            init(cx);
        }
    }
    let route = scene.route.clone();
    let navigate: recur_app::gallery::SceneHook = Box::new(move |state, cx| {
        let mut parts = route.split(':');
        let kind = parts.next().unwrap_or("");
        let id = parts.next().unwrap_or("").to_string();
        state.update(cx, |s, cx| match kind {
            "welcome" => s.go(ViewName::Welcome, None, cx),
            "briefing" => s.go(ViewName::Briefing, None, cx),
            "issues" => s.go(ViewName::Issues, None, cx),
            "schedules" => s.go(ViewName::Schedules, None, cx),
            "newtask" => s.open_new_task("", None, cx),
            "newproject" => s.open_new_project(false, cx),
            "project" => s.open_project(&id, cx),
            _ => {}
        })
    });
    let (view, _) = recur_app::gallery::view_scene(
        &fixture(&scene.fixture),
        &scene.theme,
        &scene.storage,
        registry,
        navigate,
        None,
        window,
        cx,
    );
    Some(view)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scene_has_a_fixture() {
        for v in ["home", "briefing", "projects"]
            .into_iter()
            .flat_map(parsed_in)
        {
            let s: HomeScene = serde_json::from_value(v).expect("scene");
            assert!(fixture(&s.fixture).is_object(), "{}", s.name);
        }
    }
}
