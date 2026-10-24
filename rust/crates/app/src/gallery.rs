//! Shell parity scenes: the `shell` list in `rust/tools/ui-parity/scenes.json`. Each scene is
//! the full window (titlebar, sidebar, an empty main area) over the mock bridge, as
//! `web/scenes/Shell.svelte` mounts `App.svelte` with every view hidden. A scene can edit the
//! e2e seed (`seed`), preset local storage (`storage`), and scroll the project tree
//! (`treeScroll`) once it has loaded.

use std::collections::BTreeMap;

use gpui_kit::{
    AnyView, App, AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, Window,
    div,
};
use recur_services::mock::{MockBridge, MockOptions, Seed};
use recur_ui::gallery::SceneSpec;
use recur_view_core::{AppState, LocalStorage, ViewContext, ViewRegistry};
use serde::Deserialize;
use serde_json::Value;

use crate::{Shell, ShellOptions};

const SCENES_JSON: &str = include_str!("../../../tools/ui-parity/scenes.json");
const SEED_JSON: &str = include_str!("../../../../frontend/e2e/fixtures/seed.json");

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

fn merge(into: &mut Value, patch: &Value) {
    if let (Some(a), Some(b)) = (into.as_object_mut(), patch.as_object()) {
        for (k, v) in b {
            a.insert(k.clone(), v.clone());
        }
    }
}

/// Upsert `rows` into the array at `list` by `id` (merging fields into an existing row), and
/// mirror each into the `by_id` map when there is one.
fn upsert(seed: &mut Value, list: &str, by_id: Option<&str>, rows: &Value) {
    for row in rows.as_array().into_iter().flatten() {
        let id = row
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let arr = seed.as_object_mut().and_then(|o| {
            o.entry(list)
                .or_insert_with(|| Value::Array(Vec::new()))
                .as_array_mut()
        });
        let Some(arr) = arr else { continue };
        match arr
            .iter_mut()
            .find(|x| x.get("id").and_then(Value::as_str) == Some(id.as_str()))
        {
            Some(x) => merge(x, row),
            None => arr.push(row.clone()),
        }
        if let Some(map) = by_id
            && let Some(m) = seed.as_object_mut().and_then(|o| {
                o.entry(map)
                    .or_insert_with(|| Value::Object(Default::default()))
                    .as_object_mut()
            })
        {
            match m.get_mut(&id) {
                Some(x) => merge(x, row),
                None => {
                    m.insert(id.clone(), row.clone());
                }
            }
        }
    }
}

/// The scene's seed edits, applied the same way `capture.spec.ts` applies them: `projects`,
/// `loops` and `prs` upsert rows by id; `projectGroups` and `inbox` replace; `settings` merges.
pub fn patch_seed(seed: &mut Value, patch: &Value) {
    let Some(p) = patch.as_object() else { return };
    if let Some(rows) = p.get("projects") {
        upsert(seed, "projects", None, rows);
    }
    if let Some(rows) = p.get("loops") {
        upsert(seed, "loops", Some("loopById"), rows);
    }
    if let Some(rows) = p.get("prs") {
        upsert(seed, "prs", Some("prById"), rows);
    }
    for key in ["projectGroups", "inbox"] {
        if let Some(v) = p.get(key)
            && let Some(o) = seed.as_object_mut()
        {
            o.insert(key.to_string(), v.clone());
        }
    }
    if let Some(v) = p.get("settings")
        && let Some(s) = seed.get_mut("settings")
    {
        merge(s, v);
    }
}

/// `loadSeed()` with `patch` applied ([`patch_seed`]).
pub fn scene_seed(patch: &Value) -> Seed {
    let mut raw: Value = serde_json::from_str(SEED_JSON).unwrap_or(Value::Null);
    patch_seed(&mut raw, patch);
    let set_path = |l: &mut Value| {
        let empty = l
            .get("worktreePath")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty);
        if empty {
            let id = l.get("id").and_then(Value::as_str).unwrap_or_default();
            let path = format!("/mock/worktrees/{id}");
            if let Some(o) = l.as_object_mut() {
                o.insert("worktreePath".into(), Value::String(path));
            }
        }
    };
    if let Some(loops) = raw.get_mut("loops").and_then(Value::as_array_mut) {
        loops.iter_mut().for_each(set_path);
    }
    if let Some(m) = raw.get_mut("loopById").and_then(Value::as_object_mut) {
        m.values_mut().for_each(set_path);
    }
    Seed::from_json(&raw)
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

/// A fixture's canned replies (`resolvers`); `{"$pending": true}` holds the call forever.
pub fn fixture_resolvers(fx: &Value) -> MockOptions {
    let mut opts = MockOptions::default();
    if let Some(m) = fx["resolvers"].as_object() {
        for (k, v) in m {
            let v = v.clone();
            let pending = v["$pending"].as_bool() == Some(true);
            opts = opts.resolver(k, move |_, _, _| {
                if pending {
                    recur_services::mock::Reply::Later(Box::pin(std::future::pending::<
                        recur_services::mock::Reply,
                    >()))
                } else {
                    recur_services::mock::Reply::value(v.clone())
                }
            });
        }
    }
    opts
}

/// What a view crate's whole-window scene does once the shell has booted: navigate, then
/// three frames later act (open a menu, a modal).
pub type SceneHook = Box<dyn Fn(&Entity<AppState>, &mut App)>;

struct ViewSceneView {
    shell: Entity<Shell>,
    navigate: Option<SceneHook>,
    act: Option<SceneHook>,
    frames: u8,
}

impl ViewSceneView {
    fn render_view(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let state = self.shell.read(cx).state().clone();
        if let Some(nav) = self.navigate.take() {
            if state.read(cx).booted {
                cx.defer(move |cx| nav(&state, cx));
            } else {
                self.navigate = Some(nav);
            }
            cx.notify();
        } else if self.frames < 3 {
            self.frames += 1;
            if self.frames == 3
                && let Some(act) = self.act.take()
            {
                cx.defer(move |cx| act(&state, cx));
            }
            cx.notify();
        }
        div().size_full().child(self.shell.clone())
    }
}

impl Render for ViewSceneView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

/// A whole-window scene for a view crate: the shell with `registry` over the e2e seed edited by
/// the fixture's `seed` (answering its `resolvers`), local storage with the theme and
/// `storage`, then `navigate` once booted and `act` three frames later. Returns the bridge for
/// scenes that dispatch events.
#[allow(clippy::too_many_arguments)]
pub fn view_scene(
    fx: &Value,
    theme: &str,
    storage: &BTreeMap<String, String>,
    registry: ViewRegistry,
    navigate: SceneHook,
    act: Option<SceneHook>,
    window: &mut Window,
    cx: &mut App,
) -> (AnyView, std::sync::Arc<MockBridge>) {
    let bridge = MockBridge::new(scene_seed(&fx["seed"]), fixture_resolvers(fx));
    let services = bridge.services();
    let mut st = LocalStorage::in_memory();
    st.set("recur.theme", theme.to_string());
    for (k, v) in storage {
        st.set(k, v.clone());
    }
    let state: Entity<AppState> = AppState::install(services.clone(), st, cx);
    let ctx = ViewContext { state, services };
    let shell = cx.new(|cx| Shell::new(ctx, registry, ShellOptions::default(), window, cx));
    let view = cx
        .new(|_| ViewSceneView {
            shell,
            navigate: Some(navigate),
            act,
            frames: 0,
        })
        .into();
    (view, bridge)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn seed_patch_upserts_rows_and_mirrors_by_id() {
        let mut seed = json!({
            "projects": [{"id": "a", "name": "A"}],
            "loops": [{"id": "L1", "state": "coding"}],
            "loopById": {"L1": {"id": "L1", "state": "coding"}},
            "settings": {"x": 1, "y": 2},
        });
        patch_seed(
            &mut seed,
            &json!({
                "projects": [{"id": "a", "groupId": "g"}, {"id": "b", "name": "B"}],
                "loops": [{"id": "L1", "state": "merged"}, {"id": "L2", "state": "queued"}],
                "projectGroups": [{"id": "g", "name": "G", "open": true}],
                "settings": {"y": 3},
            }),
        );
        assert_eq!(seed["projects"][0]["groupId"], "g");
        assert_eq!(seed["projects"][0]["name"], "A");
        assert_eq!(seed["projects"][1]["id"], "b");
        assert_eq!(seed["loops"][0]["state"], "merged");
        assert_eq!(seed["loopById"]["L1"]["state"], "merged");
        assert_eq!(seed["loopById"]["L2"]["state"], "queued");
        assert_eq!(seed["projectGroups"][0]["name"], "G");
        assert_eq!(seed["settings"], json!({"x": 1, "y": 3}));
    }

    #[test]
    fn every_scene_parses() {
        for spec in specs() {
            assert!(exists(&spec.name), "{}", spec.name);
        }
    }
}
