//! What every whole-window parity scene shares (`crates/ui/gallery/scenes.json`): the e2e
//! seed with a scene's edits, a fixture's canned replies, and [`view_scene`], the shell over
//! the mock bridge, navigated once booted, as `web/scenes/Views.svelte` mounts `App.svelte`.
//!
//! The shell and the full view registry sit above every view crate, so the parity tool
//! installs them as a [`SceneHost`] before it builds a scene; view crates reach them only
//! through it.

use std::collections::BTreeMap;

use gpui_kit::{
    AnyView, App, AppContext as _, Context, Entity, Global, IntoElement, ParentElement, Render,
    Styled, Window, div,
};
use recur_services::mock::{MockBridge, MockOptions, Seed};
use recur_ui::gallery::SceneSpec;
use serde::Deserialize;
use serde_json::Value;

use crate::{AppState, LocalStorage, ViewContext, ViewName, ViewRegistry};

const SCENES_JSON: &str = include_str!("../../../ui/gallery/scenes.json");
const FIXTURES: &str = include_str!("../../../ui/gallery/content/views.json");
const SEED_JSON: &str = include_str!("../../../services/src/mock/seed.json");

/// The parts of a whole-window scene above the view crates.
pub struct SceneHost {
    /// `recur_app::Shell` over `ViewContext` with `ViewRegistry`.
    pub shell: fn(ViewContext, ViewRegistry, &mut Window, &mut App) -> AnyView,
    /// The registry the desktop builds: every view crate registered.
    pub registry: fn() -> ViewRegistry,
    /// Clears the app globals view crates keep, since scenes share one app.
    pub reset: fn(&mut App),
}

impl Global for SceneHost {}

/// [`SceneHost::registry`].
pub fn registry(cx: &App) -> ViewRegistry {
    (cx.global::<SceneHost>().registry)()
}

/// [`SceneHost::reset`].
pub fn reset(cx: &mut App) {
    (cx.global::<SceneHost>().reset)(cx)
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct ViewScene {
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

/// A fixture of `content/views.json`.
pub fn fixture(name: &str) -> Value {
    serde_json::from_str::<Value>(FIXTURES).unwrap_or(Value::Null)["fixtures"][name].clone()
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
    state: Entity<AppState>,
    shell: AnyView,
    navigate: Option<SceneHook>,
    act: Option<SceneHook>,
    frames: u8,
}

impl ViewSceneView {
    fn render_view(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let state = self.state.clone();
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
    let ctx = ViewContext {
        state: state.clone(),
        services,
    };
    let shell = (cx.global::<SceneHost>().shell)(ctx, registry, window, cx);
    let view = cx
        .new(|_| ViewSceneView {
            state,
            shell,
            navigate: Some(navigate),
            act,
            frames: 0,
        })
        .into();
    (view, bridge)
}

struct InitsRan;
impl Global for InitsRan {}

/// Runs the registry's inits (actions, key bindings, globals) the first time a scene builds.
pub fn run_inits_once(registry: &ViewRegistry, cx: &mut App) {
    if cx.try_global::<InitsRan>().is_none() {
        cx.set_global(InitsRan);
        for init in &registry.inits {
            init(cx);
        }
    }
}

/// A scene of `list` (`home`, `briefing`, `projects`): the shell with the full registry over the
/// fixture, then the scene's `route` once booted.
pub fn build_in(list: &str, name: &str, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let raw = parsed_in(list).into_iter().find(|v| v["name"] == name)?;
    let scene: ViewScene = serde_json::from_value(raw).ok()?;
    reset(cx);
    let mut registry = registry(cx);
    run_inits_once(&registry, cx);
    // `Views.svelte`: only the routed view shows in `.main`; modal routes leave it empty.
    let kind = scene.route.split(':').next().unwrap_or("");
    let shown = match kind {
        "welcome" => Some(ViewName::Welcome),
        "briefing" => Some(ViewName::Briefing),
        "issues" => Some(ViewName::Issues),
        "schedules" => Some(ViewName::Schedules),
        "project" => Some(ViewName::Project),
        _ => None,
    };
    registry
        .views
        .retain(|v| shown.is_some_and(|r| v.routes.contains(&r)));
    let route = scene.route.clone();
    let navigate: SceneHook = Box::new(move |state, cx| {
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
    let (view, _) = view_scene(
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
    fn every_scene_has_a_fixture() {
        for v in ["home", "briefing", "projects"]
            .into_iter()
            .flat_map(parsed_in)
        {
            let s: ViewScene = serde_json::from_value(v).expect("scene");
            assert!(fixture(&s.fixture).is_object(), "{}", s.name);
        }
    }
}
