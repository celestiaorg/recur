//! Parity scenes: the `workspace` list in `rust/tools/ui-parity/scenes.json`, built the way
//! `web/scenes/Workspace.svelte` mounts the detail route for LOOP-2231, over
//! `content/workspace.json` (each scene's chat rows, tabs, sessions, facts and findings) and
//! the Observatory fixture of the scene's `session`. A scene can edit the e2e seed (`seed`).

use gpui_kit::{
    AnyView, App, AppContext as _, Context, Entity, IntoElement, ParentElement, Render, Styled,
    Window,
};
use recur_services::mock::{MockBridge, MockOptions, Reply};
use recur_ui::gallery::SceneSpec;
use recur_ui::style::body;
use recur_view_core::{AppState, LocalStorage, ViewContext};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::detail::WorkspaceDetail;

const SCENES_JSON: &str = include_str!("../../../../tools/ui-parity/scenes.json");
const FIXTURES: &str = include_str!("../../../../tools/ui-parity/content/workspace.json");
const CHAT: &str = include_str!("../../../../tools/ui-parity/content/chat.json");

#[derive(Deserialize)]
struct Specs {
    #[serde(default)]
    workspace: Vec<Value>,
}

fn parsed() -> Vec<Value> {
    serde_json::from_str::<Specs>(SCENES_JSON)
        .map(|s| s.workspace)
        .unwrap_or_default()
}

/// Every workspace scene, in file order.
pub fn specs() -> Vec<SceneSpec> {
    parsed()
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect()
}

pub fn exists(name: &str) -> bool {
    specs().iter().any(|s| s.name == name)
}

fn json_of(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or(Value::Null)
}

/// The scene's variant: the name after `workspace/`, without the theme suffix.
fn variant(name: &str) -> &str {
    let v = name.split_once('/').map_or("", |(_, v)| v);
    v.strip_suffix("-paper").unwrap_or(v)
}

/// capture.spec's `workspaceResolvers`: the Observatory's for the scene's `session`, then
/// the chat's.
pub fn resolvers(variant: &str) -> MockOptions {
    let fx = json_of(FIXTURES);
    let scene = fx["scenes"][variant].clone();
    let items = json_of(CHAT)[scene["items"].as_str().unwrap_or("")]["items"].clone();
    let list = |k: &str| match &scene[k] {
        Value::Null => json!([]),
        v => v.clone(),
    };
    let (agents, sessions, facts, findings) = (
        list("agents"),
        list("sessions"),
        list("facts"),
        list("findings"),
    );
    let mut opts =
        recur_view_observatory::gallery::resolvers(scene["session"].as_str().unwrap_or(""))
            .resolver("ListLoopAgents", move |_, _, _| {
                Reply::value(agents.clone())
            })
            .resolver("WorkspaceChatItems", move |_, _, _| {
                Reply::value(match &items {
                    Value::Null => json!([]),
                    v => v.clone(),
                })
            })
            .resolver("WorkspaceSessions", move |_, _, _| {
                Reply::value(sessions.clone())
            })
            .resolver("WorkspaceFacts", move |_, _, _| Reply::value(facts.clone()))
            .resolver("ListFindings", move |_, _, _| {
                Reply::value(findings.clone())
            });
    if let Some(chats) = scene.get("chats").filter(|v| v.is_array()).cloned() {
        opts = opts.resolver("ListWorkspaceChats", move |_, _, _| {
            Reply::value(chats.clone())
        });
    }
    opts
}

/// `.main` with the detail route in it.
struct WorkspaceScene(Entity<WorkspaceDetail>);

impl Render for WorkspaceScene {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        body(cx).size_full().flex().flex_col().child(self.0.clone())
    }
}

pub fn build(name: &str, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let spec = parsed().into_iter().find(|v| v["name"] == name)?;
    let theme = spec["theme"].as_str().unwrap_or("default").to_string();
    // Scenes share one app: no Observatory or chat state carries over.
    recur_view_observatory::gallery::reset(cx);
    cx.set_global(recur_view_chat::transcript_ui::TranscriptUiCache::default());
    cx.set_global(recur_view_chat::heap::ChatHeapCache::default());
    cx.set_global(recur_view_chat::strip::ChatStripCache::default());
    cx.set_global(recur_view_chat::hoist::HoistState::default());
    cx.set_global(recur_content::webhost::BlockingRenders);
    let bridge = MockBridge::new(
        recur_app::gallery::scene_seed(&spec["seed"]),
        resolvers(variant(name)),
    );
    let services = bridge.services();
    let mut storage = LocalStorage::in_memory();
    storage.set("recur.theme", theme);
    let state: Entity<AppState> = AppState::install(services.clone(), storage, cx);
    let ws = json_of(FIXTURES)["workspaceId"]
        .as_str()
        .unwrap_or("")
        .to_string();
    // `await loadInitial(); openWorkspace(id)`.
    let load = state.update(cx, |s, cx| s.load_initial(cx));
    let s2 = state.clone();
    cx.spawn(async move |cx| {
        let _ = load.await;
        let _ = s2.update(cx, |s, cx| s.open_workspace(&ws, false, cx));
    })
    .detach();
    let vcx = ViewContext { state, services };
    let detail = cx.new(|cx| WorkspaceDetail::new(&vcx, window, cx));
    Some(cx.new(|_| WorkspaceScene(detail)).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scene_has_a_fixture() {
        let fx = json_of(FIXTURES);
        let chat = json_of(CHAT);
        assert!(!specs().is_empty());
        for spec in specs() {
            let scene = &fx["scenes"][variant(&spec.name)];
            assert!(scene.is_object(), "{}", spec.name);
            assert!(chat[scene["items"].as_str().unwrap_or("")]["items"].is_array());
        }
    }
}
