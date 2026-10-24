//! Parity scenes: the `observatory` list in `rust/tools/ui-parity/scenes.json`, built the way
//! `web/scenes/Observatory.svelte` mounts the Observatory for LOOP-2231, over
//! `content/observatory.json` (tree, files, git status, artifacts, a terminal, and each
//! scene's editor session).

use std::sync::Arc;

use base64::Engine as _;
use gpui_kit::{
    AnyView, App, AppContext as _, Context, Entity, IntoElement, ParentElement, Render, Styled,
    Window,
};
use recur_services::mock::{MockBridge, MockOptions, Reply};
use recur_ui::gallery::SceneSpec;
use recur_ui::style::body;
use recur_view_core::{AppState, LocalStorage};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::obs::{Observatory, ObservatoryProps, ObservatorySlots};

const SCENES_JSON: &str = include_str!("../../../../tools/ui-parity/scenes.json");
const FIXTURES: &str = include_str!("../../../../tools/ui-parity/content/observatory.json");

/// The fixture's raster and PDF files, by name.
const ASSETS: [(&str, &[u8]); 3] = [
    (
        "chart.png",
        include_bytes!("../../../../tools/ui-parity/content/chart.png"),
    ),
    (
        "screenshot.png",
        include_bytes!("../../../../tools/ui-parity/content/screenshot.png"),
    ),
    (
        "page.pdf",
        include_bytes!("../../../../tools/ui-parity/content/page.pdf"),
    ),
];

#[derive(Deserialize)]
struct Specs {
    #[serde(default)]
    observatory: Vec<SceneSpec>,
}

/// Every Observatory scene, in file order.
pub fn specs() -> Vec<SceneSpec> {
    serde_json::from_str::<Specs>(SCENES_JSON)
        .map(|s| s.observatory)
        .unwrap_or_default()
}

pub fn exists(name: &str) -> bool {
    specs().iter().any(|s| s.name == name)
}

fn fixtures() -> Value {
    serde_json::from_str(FIXTURES).unwrap_or(Value::Null)
}

fn asset(name: &str) -> Option<&'static [u8]> {
    ASSETS.iter().find(|(n, _)| *n == name).map(|(_, b)| *b)
}

fn arg(a: &[Value], i: usize) -> String {
    a.get(i).and_then(Value::as_str).unwrap_or("").to_string()
}

/// The scene's variant: the name after `observatory/`, without the theme suffix.
fn variant(name: &str) -> &str {
    let v = name.split_once('/').map_or("", |(_, v)| v);
    v.strip_suffix("-paper").unwrap_or(v)
}

/// The mock answering the Observatory's calls from the fixture (capture.spec's
/// `observatoryResolvers`).
pub fn bridge(variant: &str) -> Arc<MockBridge> {
    MockBridge::fresh(resolvers(variant))
}

/// Clears the crate's app globals: scenes share one app.
pub fn reset(cx: &mut App) {
    crate::reset_globals(cx);
}

/// [`bridge`]'s resolvers, for hosts that add their own on top (the workspace view's scenes).
pub fn resolvers(variant: &str) -> MockOptions {
    let fx = fixtures();
    let scene = fx["scenes"][variant].clone();
    let terminal = scene["terminal"].is_string();
    let (tree, files, base, outline) = (
        fx["tree"].clone(),
        fx["files"].clone(),
        fx["base"].clone(),
        fx["outline"].clone(),
    );
    let files2 = files.clone();
    let files3 = files.clone();
    let (git, working, staged, artifacts) = (
        fx["gitStatus"].clone(),
        fx["working"].clone(),
        fx["staged"].clone(),
        fx["artifacts"].clone(),
    );
    let changed: Vec<String> = git
        .as_object()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    let base_assets = fx["baseAssets"].clone();
    let term = fx["terminal"].clone();
    let term2 = term.clone();
    let session = scene["session"].clone();
    MockOptions::default()
        .resolver("ListTree", move |a, _, _| {
            Reply::value(tree.get(arg(a, 2)).cloned().unwrap_or(json!([])))
        })
        .resolver("ReadFile", move |a, _, _| {
            let path = arg(a, 2);
            let Some(f) = files.get(&path) else {
                return Reply::error(format!("no such file: {path}"));
            };
            let content = f["content"].as_str().unwrap_or("");
            let size = match f["asset"].as_str().and_then(asset) {
                Some(b) => b.len(),
                None => content.len(),
            };
            Reply::value(json!({
                "path": path,
                "language": f["language"].as_str().unwrap_or(""),
                "content": content,
                "size": size,
                "writable": !arg(a, 1).is_empty(),
                "mime": f["mime"].as_str().unwrap_or(""),
            }))
        })
        .resolver("FileAtBase", move |a, _, _| {
            let path = arg(a, 2);
            let text = base
                .get(&path)
                .or_else(|| files2.get(&path).map(|f| &f["content"]))
                .and_then(Value::as_str)
                .unwrap_or("");
            Reply::value(text)
        })
        .resolver("Outline", move |a, _, _| {
            Reply::value(outline.get(arg(a, 2)).cloned().unwrap_or(json!([])))
        })
        .resolver("GitStatus", move |_, _, _| Reply::value(git.clone()))
        .resolver("WorkingStatus", move |_, _, _| {
            Reply::value(working.clone())
        })
        .resolver("StagedFiles", move |_, _, _| Reply::value(staged.clone()))
        .resolver("ListChangedFiles", move |_, _, _| {
            Reply::value(changed.clone())
        })
        .resolver("ListArtifacts", move |_, _, _| {
            Reply::value(artifacts.clone())
        })
        .resolver("GetEditorSession", move |_, _, _| {
            Reply::value(session.clone())
        })
        .resolver("ListTerminals", move |_, _, _| {
            Reply::value(if terminal {
                json!([term["session"]])
            } else {
                json!([])
            })
        })
        .resolver("RestoreWorkspaceTerminals", move |_, _, _| {
            Reply::value(if terminal {
                json!([term2["session"]])
            } else {
                json!([])
            })
        })
        .resolver("AttachTerminal", {
            let input = fx["terminal"]["input"].as_str().unwrap_or("").to_string();
            move |_, _, _| {
                Reply::value(base64::engine::general_purpose::STANDARD.encode(input.as_bytes()))
            }
        })
        .resolver("FetchAsset", move |a, _, _| {
            let url = arg(a, 0);
            let Ok(u) = url::Url::parse(&format!("http://wails{url}")) else {
                return Reply::null();
            };
            let q = |k: &str| {
                u.query_pairs()
                    .find(|(n, _)| n == k)
                    .map(|(_, v)| v.into_owned())
            };
            let path = q("path").unwrap_or_default();
            let name = if q("at").as_deref() == Some("base") {
                base_assets[&path].as_str()
            } else {
                files3[&path]["asset"].as_str()
            };
            let Some(bytes) = name.and_then(asset) else {
                return Reply::value(json!({ "status": 404, "message": "not found" }));
            };
            let mime = if path.ends_with(".pdf") {
                "application/pdf"
            } else {
                "image/png"
            };
            Reply::value(json!({
                "contentType": mime,
                "cacheControl": "no-store",
                "body": base64::engine::general_purpose::STANDARD.encode(bytes),
            }))
        })
}

/// The workspace view's side: the Observatory filling the window.
struct ObservatoryScene(Entity<Observatory>);

impl ObservatoryScene {
    fn render_view(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        body(cx).size_full().flex().flex_col().child(self.0.clone())
    }
}

impl Render for ObservatoryScene {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

pub fn build(name: &str, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let spec = specs().into_iter().find(|s| s.name == name)?;
    crate::reset_globals(cx);
    // No application loop answers the webview here: pdf.js renders on the spot.
    cx.set_global(recur_content::webhost::BlockingRenders);
    let variant = variant(name);
    let fx = fixtures();
    let bridge = bridge(variant);
    let mut storage = LocalStorage::in_memory();
    storage.set("recur.theme", spec.theme.clone());
    let state: Entity<AppState> = AppState::install(bridge.services(), storage, cx);
    state.update(cx, |s, cx| {
        s.view = recur_view_core::ViewName::Detail;
        s.load_initial(cx).detach();
    });
    let ws = fx["workspaceId"].as_str().unwrap_or("").to_string();
    match fx["scenes"][variant]["terminal"].as_str() {
        Some("open") => state.update(cx, |s, cx| s.set_terminal_dock_open(&ws, true, cx)),
        Some("beside") => state.update(cx, |s, cx| s.set_terminal_beside(&ws, true, cx)),
        _ => {}
    }
    let props = ObservatoryProps {
        project_id: fx["projectId"].as_str().unwrap_or("").into(),
        workspace_id: ws,
        shell_active: true,
        ..Default::default()
    };
    let obs = cx.new(|cx| Observatory::new(state, props, ObservatorySlots::default(), window, cx));
    Some(cx.new(|_| ObservatoryScene(obs)).into())
}
