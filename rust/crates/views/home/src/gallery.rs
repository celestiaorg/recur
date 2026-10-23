//! Parity scenes: the `home` list in `rust/tools/ui-parity/scenes.json`, built the way
//! `web/scenes/Views.svelte` mounts `App.svelte`: the whole shell over the e2e seed edited by
//! the scene's fixture in `content/views.json`, then the scene's `route` once booted.

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

#[derive(Deserialize)]
struct Specs {
    #[serde(default)]
    home: Vec<Value>,
}

fn parsed() -> Vec<Value> {
    serde_json::from_str::<Specs>(SCENES_JSON)
        .map(|s| s.home)
        .unwrap_or_default()
}

/// Every home scene, in file order.
pub fn specs() -> Vec<SceneSpec> {
    parsed()
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect()
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
    let raw = parsed().into_iter().find(|v| v["name"] == name)?;
    let scene: HomeScene = serde_json::from_value(raw).ok()?;
    reset(cx);
    let registry = registry();
    if cx.try_global::<InitsRan>().is_none() {
        cx.set_global(InitsRan);
        for init in &registry.inits {
            init(cx);
        }
    }
    let route = scene.route.clone();
    let navigate: recur_app::gallery::SceneHook = Box::new(move |state, cx| {
        state.update(cx, |s, cx| match route.as_str() {
            "welcome" => s.go(ViewName::Welcome, None, cx),
            "newtask" => s.open_new_task("", None, cx),
            "newproject" => s.open_new_project(false, cx),
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
        for v in parsed() {
            let s: HomeScene = serde_json::from_value(v).expect("scene");
            assert!(fixture(&s.fixture).is_object(), "{}", s.name);
        }
    }
}
