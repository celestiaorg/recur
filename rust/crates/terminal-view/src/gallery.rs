//! Terminal parity scenes: the `terminal` list in `rust/tools/ui-parity/scenes.json`, built the
//! way `web/scenes/Terminal.svelte` mounts the Svelte pane. A scene is the dock body (find bar
//! when open, then the panes) around one pane whose session replays the scene's input bytes.

use recur_ui::guard::Guarded;
use std::sync::Arc;

use gpui_kit::{
    AnyView, App, AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled,
    Subscription, Window, div,
};
use recur_ui::gallery::SceneSpec;
use recur_ui::style::body;
use recur_ui::tooltip::TitleTooltip;
use recur_ui::{Theme, ThemeId};
use serde::Deserialize;

use crate::backend::TerminalBackend;
use crate::find_bar::{FindBarEvent, TerminalFindBar};
use crate::font::TerminalFont;
use crate::theme::{TerminalThemeInfo, ThemeContext};
use crate::view::{TerminalEvent, TerminalOptions, TerminalView};

const SCENES_JSON: &str = include_str!("../../../tools/ui-parity/scenes.json");

/// The terminal fields of a scene; the rest is a [`SceneSpec`].
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TerminalScene {
    name: String,
    /// Key into `terminalInputs`: the bytes the session replays on attach.
    input: String,
    /// The dock's find bar is open.
    #[serde(default)]
    find: bool,
    /// Bundle `[[terminalTheme]]` id; empty follows the app chrome.
    #[serde(default)]
    terminal_theme: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Specs {
    #[serde(default)]
    terminal: Vec<serde_json::Value>,
    #[serde(default)]
    terminal_inputs: std::collections::HashMap<String, String>,
}

fn parsed() -> Specs {
    serde_json::from_str(SCENES_JSON).unwrap_or(Specs {
        terminal: Vec::new(),
        terminal_inputs: Default::default(),
    })
}

/// Every terminal scene, in file order.
pub fn specs() -> Vec<SceneSpec> {
    parsed()
        .terminal
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect()
}

fn scene(name: &str) -> Option<(TerminalScene, Vec<u8>)> {
    let specs = parsed();
    let scene: TerminalScene = specs
        .terminal
        .into_iter()
        .filter_map(|v| serde_json::from_value::<TerminalScene>(v).ok())
        .find(|s| s.name == name)?;
    let input = specs.terminal_inputs.get(&scene.input)?.as_bytes().to_vec();
    Some((scene, input))
}

/// Whether `name` is a terminal scene with a known input.
pub fn exists(name: &str) -> bool {
    scene(name).is_some()
}

/// The shipped Terminal bundle's palettes, as `ListTerminalThemes` returns them.
pub fn shipped_terminal_themes() -> Vec<TerminalThemeInfo> {
    let Some(doc) = palette_json("bundle.toml").and_then(|s| s.parse::<toml::Table>().ok()) else {
        return Vec::new();
    };
    let tables = doc
        .get("terminalTheme")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    tables
        .iter()
        .filter_map(|t| {
            let s = |k: &str| t.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let path = s("path");
            let palette = palette_json(&path)?;
            Some(TerminalThemeInfo {
                id: s("id"),
                name: s("name"),
                ui_theme: match s("uiTheme").as_str() {
                    "" => "dark".into(),
                    ui => ui.into(),
                },
                order: t.get("order").and_then(|v| v.as_integer()).unwrap_or(0),
                palette: serde_json::from_str(palette).ok()?,
                path,
                bundle_name: "Terminal".into(),
                source_root: "default".into(),
            })
        })
        .collect()
}

static TERMINAL_BUNDLE_DIR: include_dir::Dir =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/../../../bundles/Terminal");

fn palette_json(path: &str) -> Option<&'static str> {
    TERMINAL_BUNDLE_DIR.get_file(path)?.contents_utf8()
}

/// A session that replays fixed bytes and drops everything else.
struct Replay(Vec<u8>);

impl TerminalBackend for Replay {
    fn attach(&self, _id: &str) -> Result<Vec<u8>, String> {
        Ok(self.0.clone())
    }

    fn detach(&self, _id: &str) {}

    fn ack(&self, _id: &str, _n: usize) {}

    fn write(&self, _id: &str, _data: &[u8]) {}

    fn resize(&self, _id: &str, _cols: u16, _rows: u16) {}

    fn clear_replay(&self, _id: &str) {}
}

/// The dock body: `.term-dock-body` on `--term-bg`, the find bar, then `.term-dock-panes`.
struct DockBody {
    terminal: Entity<TerminalView>,
    theme: ThemeContext,
    find: Option<Entity<TerminalFindBar>>,
    tooltip: Entity<TitleTooltip>,
    _subscriptions: Vec<Subscription>,
}

impl DockBody {
    fn render_view(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let app: ThemeId = Theme::of(cx).id;
        let bg = crate::element::gpui_color(self.theme.surface_bg(app));
        body(cx)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(bg)
            .children(self.find.clone())
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(self.terminal.clone()),
            )
            .child(self.tooltip.clone())
    }
}

impl Render for DockBody {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

/// Build the root view for a terminal scene, or `None` for an unknown name.
pub fn build(name: &str, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let (scene, input) = scene(name)?;
    let theme = ThemeContext {
        terminal_theme: scene.terminal_theme.clone(),
        catalog: shipped_terminal_themes(),
    };
    let terminal = cx.new(|cx| {
        TerminalView::new(
            TerminalOptions {
                session_id: format!("parity-{name}"),
                workspace_id: "parity".into(),
                active: true,
                frosted: false,
                font: TerminalFont::default(),
                theme: theme.clone(),
                workspace_root: String::new(),
                dock_focus: None,
            },
            Arc::new(Replay(input)),
            window,
            cx,
        )
    });
    let find = scene
        .find
        .then(|| cx.new(|cx| TerminalFindBar::new(window, cx)));
    let tooltip = TitleTooltip::new(cx);
    let view = cx.new(|cx| {
        let mut subs = vec![cx.guarded_subscribe(
            &terminal,
            |body: &mut DockBody, _, e: &TerminalEvent, cx| {
                if let (
                    TerminalEvent::FindResults {
                        index,
                        total,
                        valid,
                    },
                    Some(bar),
                ) = (e, &body.find)
                {
                    bar.update(cx, |b, cx| b.set_results(*index, *total, *valid, cx));
                }
            },
        )];
        if let Some(bar) = &find {
            let terminal = terminal.clone();
            subs.push(
                cx.guarded_subscribe(bar, move |_, bar, e: &FindBarEvent, cx| {
                    let (query, mut opts) = bar.read_with(cx, |b, cx| (b.query(cx), b.options()));
                    terminal.update(cx, |t, cx| match e {
                        FindBarEvent::Changed if query.is_empty() => t.clear_search(cx),
                        FindBarEvent::Changed => {
                            opts.incremental = true;
                            t.find_next(&query, opts, cx);
                        }
                        FindBarEvent::Next if !query.is_empty() => {
                            t.find_next(&query, opts, cx);
                        }
                        FindBarEvent::Previous if !query.is_empty() => {
                            t.find_previous(&query, opts, cx);
                        }
                        _ => {}
                    });
                }),
            );
        }
        DockBody {
            terminal,
            theme,
            find,
            tooltip,
            _subscriptions: subs,
        }
    });
    Some(view.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_terminal_scene_has_an_input() {
        let specs = specs();
        assert!(!specs.is_empty());
        let missing: Vec<&str> = specs
            .iter()
            .map(|s| s.name.as_str())
            .filter(|n| !exists(n))
            .collect();
        assert!(missing.is_empty(), "no input for {missing:?}");
    }

    #[test]
    fn shipped_palettes_load() {
        let themes = shipped_terminal_themes();
        let light = themes.iter().find(|t| t.id == "solarized-light").unwrap();
        assert_eq!(light.ui_theme, "light");
        assert_eq!(light.palette.background.as_deref(), Some("#fdf6e3"));
        assert!(themes.len() >= 15);
    }
}
