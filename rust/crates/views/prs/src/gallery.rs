//! Parity scenes: the `prs` list in `rust/tools/ui-parity/scenes.json`, built the way
//! `web/scenes/Prs.svelte` mounts `App.svelte`: the whole shell with every view crate
//! registered, over the e2e seed edited by the scene's fixture in `content/prs.json`, then the
//! dashboard (`route: "pr"`, `seg`) or PR detail (`route: "prdetail"`, `pr`, `tab`, `dock`).

use gpui_kit::{
    AnyView, App, AppContext as _, Context, Entity, Global, IntoElement, ParentElement, Render,
    Styled, Window, div,
};
use recur_app::{Shell, ShellOptions};
use recur_services::mock::{MockBridge, MockOptions, Reply};
use recur_ui::gallery::SceneSpec;
use recur_view_core::{AppState, LocalStorage, ViewContext, ViewName, ViewRegistry};
use serde::Deserialize;
use serde_json::Value;

use crate::list_filter::LIST_SEGS;
use crate::ui::{PrTab, ui};

const SCENES_JSON: &str = include_str!("../../../../tools/ui-parity/scenes.json");
const FIXTURES: &str = include_str!("../../../../tools/ui-parity/content/prs.json");

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct PrScene {
    name: String,
    theme: String,
    fixture: String,
    route: String,
    pr: String,
    tab: String,
    dock: String,
    seg: String,
}

#[derive(Deserialize)]
struct Specs {
    #[serde(default)]
    prs: Vec<Value>,
}

fn parsed() -> Vec<Value> {
    serde_json::from_str::<Specs>(SCENES_JSON)
        .map(|s| s.prs)
        .unwrap_or_default()
}

/// Every PR scene, in file order.
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

/// The fixture's canned replies (`resolvers`), as capture.spec's `prsResolvers`.
pub fn resolvers(fx: &Value) -> MockOptions {
    let mut opts = MockOptions::default();
    if let Some(m) = fx["resolvers"].as_object() {
        for (k, v) in m {
            let v = v.clone();
            opts = opts.resolver(k, move |_, _, _| Reply::value(v.clone()));
        }
    }
    opts
}

/// The registry the desktop builds, as these scenes mount the shell.
pub fn registry() -> ViewRegistry {
    let mut r = ViewRegistry::default();
    recur_view_chat::register(&mut r);
    recur_view_review::register(&mut r);
    recur_view_observatory::register(&mut r);
    recur_view_workspace::register(&mut r);
    crate::register(&mut r);
    r
}

struct InitsRan;
impl Global for InitsRan {}

struct PrSceneView {
    shell: Entity<Shell>,
    scene: PrScene,
    navigated: bool,
}

impl Render for PrSceneView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.navigated {
            let state = self.shell.read(cx).state().clone();
            if state.read(cx).booted {
                self.navigated = true;
                let sc = self.scene.clone();
                let ui = ui(cx);
                cx.defer(move |cx| navigate(&state, &ui, &sc, cx));
            }
            cx.notify();
        }
        div().size_full().child(self.shell.clone())
    }
}

fn navigate(state: &Entity<AppState>, ui: &Entity<crate::ui::PrUi>, sc: &PrScene, cx: &mut App) {
    if sc.route == "prdetail" && !sc.pr.is_empty() {
        state.update(cx, |s, cx| {
            s.open_pr(&sc.pr, cx);
            if sc.dock == "closed" {
                s.pr_agent_dock_open = false;
            }
            cx.notify();
        });
        let tab = match sc.tab.as_str() {
            "commits" => Some(PrTab::Commits),
            "files" => Some(PrTab::Files),
            "checks" => Some(PrTab::Checks),
            "conv" => Some(PrTab::Conv),
            _ => None,
        };
        if let Some(tab) = tab {
            ui.update(cx, |u, cx| u.set_tab(tab, cx));
        }
    } else {
        let seg = LIST_SEGS
            .iter()
            .find(|(_, l)| l.eq_ignore_ascii_case(&sc.seg))
            .map(|(s, _)| *s);
        if let Some(seg) = seg {
            ui.update(cx, |u, cx| {
                u.seg = seg;
                cx.notify();
            });
        }
        state.update(cx, |s, cx| s.go(ViewName::Pr, None, cx));
    }
}

pub fn build(name: &str, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let raw = parsed().into_iter().find(|v| v["name"] == name)?;
    let scene: PrScene = serde_json::from_value(raw).ok()?;
    // Scenes share one app: no view state carries over.
    recur_view_observatory::gallery::reset(cx);
    cx.set_global(recur_view_chat::transcript_ui::TranscriptUiCache::default());
    cx.set_global(recur_view_chat::heap::ChatHeapCache::default());
    cx.set_global(recur_view_chat::strip::ChatStripCache::default());
    cx.set_global(recur_view_chat::hoist::HoistState::default());
    cx.set_global(recur_content::webhost::BlockingRenders);
    crate::ui::reset(cx);
    let registry = registry();
    if cx.try_global::<InitsRan>().is_none() {
        cx.set_global(InitsRan);
        for init in &registry.inits {
            init(cx);
        }
    }
    let fx = fixture(&scene.fixture);
    let bridge = MockBridge::new(recur_app::gallery::scene_seed(&fx["seed"]), resolvers(&fx));
    let services = bridge.services();
    let mut storage = LocalStorage::in_memory();
    storage.set("recur.theme", scene.theme.clone());
    let state: Entity<AppState> = AppState::install(services.clone(), storage, cx);
    let ctx = ViewContext { state, services };
    let shell = cx.new(|cx| Shell::new(ctx, registry, ShellOptions::default(), window, cx));
    Some(
        cx.new(|_| PrSceneView {
            shell,
            scene,
            navigated: false,
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
            let s: PrScene = serde_json::from_value(v).expect("scene");
            assert!(fixture(&s.fixture).is_object(), "{}", s.name);
        }
    }
}
