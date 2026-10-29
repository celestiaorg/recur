//! Gallery scenes: the `settings` list in `crates/ui/gallery/scenes.json`: the whole shell
//! with every view crate
//! registered, over the e2e seed edited by the scene's fixture in `content/settings.json`, then
//! Settings on `section`, then the scene's `act`.

use std::sync::Arc;

use gpui_kit::{
    AnyView, App, AppContext as _, Context, Entity, IntoElement, ParentElement, Render, Styled,
    Window, div,
};
use recur_services::mock::{MockBridge, MockOptions, Reply};
use recur_ui::gallery::SceneSpec;
use recur_view_core::{AppState, LocalStorage, ViewContext, gallery};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::view::SettingsView;

const SCENES_JSON: &str = include_str!("../../../ui/gallery/scenes.json");
const FIXTURES: &str = include_str!("../../../ui/gallery/content/settings.json");

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct SettingsScene {
    name: String,
    theme: String,
    fixture: String,
    section: String,
    act: String,
}

#[derive(Deserialize)]
struct Specs {
    #[serde(default)]
    settings: Vec<Value>,
}

fn parsed() -> Vec<Value> {
    serde_json::from_str::<Specs>(SCENES_JSON)
        .map(|s| s.settings)
        .unwrap_or_default()
}

/// Every Settings scene, in file order.
pub fn specs() -> Vec<SceneSpec> {
    parsed()
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect()
}

pub fn exists(name: &str) -> bool {
    specs().iter().any(|s| s.name == name)
}

/// A fixture of `content/settings.json`.
pub fn fixture(name: &str) -> Value {
    serde_json::from_str::<Value>(FIXTURES).unwrap_or(Value::Null)["fixtures"][name].clone()
}

/// The fixture's canned replies; `{"$pending": true}` holds the call forever.
pub fn resolvers(fx: &Value) -> MockOptions {
    let mut opts = MockOptions::default();
    if let Some(m) = fx["resolvers"].as_object() {
        for (k, v) in m {
            let v = v.clone();
            let pending = v["$pending"].as_bool() == Some(true);
            opts = opts.resolver(k, move |_, _, _| {
                if pending {
                    Reply::Later(Box::pin(std::future::pending::<Reply>()))
                } else {
                    Reply::value(v.clone())
                }
            });
        }
    }
    opts
}

struct SceneView {
    state: Entity<AppState>,
    shell: AnyView,
    bridge: Arc<MockBridge>,
    scene: SettingsScene,
    navigated: bool,
    frames: u8,
}

impl SceneView {
    fn render_view(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        if !self.navigated {
            let state = self.state.clone();
            if state.read(cx).booted {
                self.navigated = true;
                let section = self.scene.section.clone();
                cx.defer(move |cx| state.update(cx, |s, cx| s.open_settings(Some(&section), cx)));
            }
            cx.notify();
        } else if self.frames < 3 {
            self.frames += 1;
            if self.frames == 3 {
                let act = self.scene.act.clone();
                let bridge = self.bridge.clone();
                cx.defer(move |cx| run_act(&act, &bridge, cx));
            }
            cx.notify();
        }
        div().size_full().child(self.shell.clone())
    }
}

impl Render for SceneView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

/// The scene's interaction (`act`).
fn run_act(act: &str, bridge: &Arc<MockBridge>, cx: &mut App) {
    match act {
        "compat-modal" => {
            if let Some(v) = SettingsView::current(cx) {
                let Some(w) = cx.active_window() else { return };
                let _ = w.update(cx, |_, window, cx| {
                    v.update(cx, |v, cx| v.open_compat_modal(window, cx))
                });
            }
        }
        "install" => {
            if let Some(v) = SettingsView::current(cx) {
                v.update(cx, |v, cx| v.install_bridge(cx));
            }
            let _ = bridge.dispatch_wire(
                "cursor.install",
                json!({ "phase": "download", "message": "Downloading Cursor SDK…", "percent": 42 }),
            );
        }
        "keybinding-menu" => {
            use recur_view_widgets::keybinding_menu::{KeybindingMenuItem, open_keybinding_menu};
            let item = |id: &str, label: &str, category: &str, order: i64| KeybindingMenuItem {
                id: id.into(),
                label: label.into(),
                category: category.into(),
                order,
            };
            open_keybinding_menu(
                vec![
                    item("a", "Sort Lines Ascending", "Text", 10),
                    item("b", "Sort Lines Descending", "Text", 20),
                    item("c", "Format JSON", "JSON", 10),
                ],
                std::rc::Rc::new(|_, _, _| {}),
                cx,
            );
        }
        _ => {}
    }
}

pub fn build(name: &str, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let raw = parsed().into_iter().find(|v| v["name"] == name)?;
    let scene: SettingsScene = serde_json::from_value(raw).ok()?;
    // Scenes share one app: no view state carries over.
    gallery::reset(cx);
    cx.set_global(crate::ModalSlot(None));
    let registry = gallery::registry(cx);
    gallery::run_inits_once(&registry, cx);
    let fx = fixture(&scene.fixture);
    let bridge = MockBridge::new(gallery::scene_seed(&fx["seed"]), resolvers(&fx));
    let services = bridge.services();
    let mut storage = LocalStorage::in_memory();
    storage.set("recur.theme", scene.theme.clone());
    let state: Entity<AppState> = AppState::install(services.clone(), storage, cx);
    let ctx = ViewContext {
        state: state.clone(),
        services,
    };
    let shell = (cx.global::<gallery::SceneHost>().shell)(ctx, registry, window, cx);
    Some(
        cx.new(|_| SceneView {
            state,
            shell,
            bridge,
            scene,
            navigated: false,
            frames: 0,
        })
        .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scene_has_a_fixture() {
        assert!(!specs().is_empty());
        for v in parsed() {
            let s: SettingsScene = serde_json::from_value(v).expect("scene");
            assert!(fixture(&s.fixture).is_object(), "{}", s.name);
        }
    }
}
