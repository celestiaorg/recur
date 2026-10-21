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

impl Render for ShellSceneView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
