//! Parity scenes: the `prs` list in `rust/tools/ui-parity/scenes.json`, built the way
//! `web/scenes/Prs.svelte` mounts `App.svelte`: the whole shell with every view crate
//! registered, over the e2e seed edited by the scene's fixture in `content/prs.json`, then the
//! dashboard (`route: "pr"`, `seg`) or PR detail (`route: "prdetail"`, `pr`, `tab`, `dock`,
//! `review`).

use gpui_kit::{
    AnyView, App, AppContext as _, Context, Entity, IntoElement, ParentElement, Render, Styled,
    Window, div,
};
use recur_services::mock::{MockBridge, MockOptions, Reply};
use recur_ui::gallery::SceneSpec;
use recur_view_core::{AppState, LocalStorage, PrReviewKind, ViewContext, ViewName, gallery};
use serde::Deserialize;
use serde_json::Value;

use crate::ui::{PrTab, ui};
use recur_view_core::pr_list_filter::LIST_SEGS;

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
    review: String,
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

struct PrSceneView {
    state: Entity<AppState>,
    shell: AnyView,
    scene: PrScene,
    navigated: bool,
    /// Frames since navigating; the tab is picked a few frames later, after PR detail's
    /// per-PR reset (which lands on Conversation), as `Prs.svelte` does.
    frames: u8,
}

impl PrSceneView {
    fn render_view(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        if !self.navigated {
            let state = self.state.clone();
            if state.read(cx).booted {
                self.navigated = true;
                let sc = self.scene.clone();
                let ui = ui(cx);
                cx.defer(move |cx| navigate(&state, &ui, &sc, cx));
            }
            cx.notify();
        } else if self.frames < 3 {
            self.frames += 1;
            if self.frames == 3 {
                let tab = match self.scene.tab.as_str() {
                    "commits" => Some(PrTab::Commits),
                    "files" => Some(PrTab::Files),
                    "checks" => Some(PrTab::Checks),
                    _ => None,
                };
                if let Some(tab) = tab {
                    let ui = ui(cx);
                    cx.defer(move |cx| ui.update(cx, |u, cx| u.set_tab(tab, cx)));
                }
                let review = match self.scene.review.as_str() {
                    "approve" => Some(PrReviewKind::Approve),
                    "request" => Some(PrReviewKind::Request),
                    _ => None,
                };
                if let Some(k) = review {
                    let state = self.state.clone();
                    cx.defer(move |cx| state.update(cx, |s, cx| s.open_pr_review(k, cx)));
                }
            }
            cx.notify();
        }
        div().size_full().child(self.shell.clone())
    }
}

impl Render for PrSceneView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
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
    gallery::reset(cx);
    crate::ui::reset(cx);
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
        cx.new(|_| PrSceneView {
            state,
            shell,
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
            let s: PrScene = serde_json::from_value(v).expect("scene");
            assert!(fixture(&s.fixture).is_object(), "{}", s.name);
        }
    }
}
