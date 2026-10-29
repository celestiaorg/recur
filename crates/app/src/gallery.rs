//! Shell parity scenes: the `shell` list in `crates/ui/gallery/scenes.json`. Each scene is
//! the full window (titlebar, sidebar, an empty main area) over the mock bridge, as
//! `web/scenes/Shell.svelte` mounts `App.svelte` with every view hidden. A scene can edit the
//! e2e seed (`seed`), preset local storage (`storage`), and scroll the project tree
//! (`treeScroll`) once it has loaded.

use std::collections::BTreeMap;

use gpui_kit::{
    AnyView, App, AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, Window,
    div,
};
use recur_services::mock::{MockBridge, MockOptions};
use recur_ui::gallery::SceneSpec;
pub use recur_view_core::gallery::{
    SceneHook, fixture_resolvers, patch_seed, scene_seed, view_scene,
};
use recur_view_core::{AppState, LocalStorage, ViewContext, ViewRegistry};
use serde::Deserialize;
use serde_json::Value;

use crate::{Shell, ShellOptions};

const SCENES_JSON: &str = include_str!("../../ui/gallery/scenes.json");

/// The shell fields of a scene; the rest is a [`SceneSpec`].
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShellScene {
    name: String,
    theme: String,
    #[serde(default)]
    seed: Value,
    #[serde(default)]
    storage: BTreeMap<String, String>,
    #[serde(default)]
    tree_scroll: f32,
}

#[derive(Deserialize)]
struct Specs {
    #[serde(default)]
    shell: Vec<Value>,
}

fn parsed() -> Vec<Value> {
    serde_json::from_str::<Specs>(SCENES_JSON)
        .map(|s| s.shell)
        .unwrap_or_default()
}

/// Every shell scene, in file order.
pub fn specs() -> Vec<SceneSpec> {
    parsed()
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect()
}

fn scene(name: &str) -> Option<ShellScene> {
    parsed()
        .into_iter()
        .filter_map(|v| serde_json::from_value::<ShellScene>(v).ok())
        .find(|s| s.name == name)
}

pub fn exists(name: &str) -> bool {
    scene(name).is_some()
}

/// The window content: the shell, plus the one-time tree scroll once the data is in.
struct ShellSceneView {
    shell: Entity<Shell>,
    tree_scroll: f32,
}

impl ShellSceneView {
    fn render_view(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        if self.tree_scroll > 0. {
            let sidebar = self.shell.read(cx).sidebar.clone();
            if sidebar.read(cx).tree_ready(cx) {
                sidebar.update(cx, |s, cx| {
                    s.set_tree_scroll(self.tree_scroll);
                    cx.notify();
                });
                self.tree_scroll = 0.;
            }
            cx.notify();
        }
        div().size_full().child(self.shell.clone())
    }
}

impl Render for ShellSceneView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

pub fn build(name: &str, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let scene = scene(name)?;
    let bridge = MockBridge::new(scene_seed(&scene.seed), MockOptions::default());
    let services = bridge.services();
    let mut storage = LocalStorage::in_memory();
    storage.set("recur.theme", scene.theme.clone());
    for (k, v) in &scene.storage {
        storage.set(k, v.clone());
    }
    let state: Entity<AppState> = AppState::install(services.clone(), storage, cx);
    let ctx = ViewContext { state, services };
    let shell = cx.new(|cx| {
        Shell::new(
            ctx,
            ViewRegistry::default(),
            ShellOptions::default(),
            window,
            cx,
        )
    });
    let tree_scroll = scene.tree_scroll;
    Some(cx.new(|_| ShellSceneView { shell, tree_scroll }).into())
}

/// [`recur_view_core::gallery::SceneHost::shell`]: the shell over `ctx` with `registry`.
pub fn scene_shell(
    ctx: ViewContext,
    registry: ViewRegistry,
    window: &mut Window,
    cx: &mut App,
) -> AnyView {
    cx.new(|cx| Shell::new(ctx, registry, ShellOptions::default(), window, cx))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scene_parses() {
        for spec in specs() {
            assert!(exists(&spec.name), "{}", spec.name);
        }
    }
}
