//! Opens a file in the code editor with the shipped bundles' grammars and
//! themes: `cargo run -p recur-editor --example editor -- <file>`.
//!
//! Options: `--theme <id>` (bundle editor theme, default `recur`),
//! `--app-theme <key>` (app chrome, default `default`), `--merge <base>`
//! (unified merge view against a base file), `--keymap vim|emacs`,
//! `--wrap`.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, Styled, Window,
    WindowBounds, WindowOptions, px, size,
};
use recur_editor::highlight::TextMate;
use recur_editor::prefs::{EditorKeymap, EditorPrefs};
use recur_editor::view::{EditorEvent, EditorKind, EditorOptions, EditorView};
use recur_textmate::{GrammarInfo, LanguageDataFields, ParsedTmTheme, parse_theme_json};

struct Args {
    file: PathBuf,
    theme: String,
    app_theme: String,
    merge: Option<PathBuf>,
    keymap: EditorKeymap,
    wrap: bool,
}

fn args() -> Args {
    let mut a = Args {
        file: PathBuf::new(),
        theme: "recur".into(),
        app_theme: "default".into(),
        merge: None,
        keymap: EditorKeymap::Default,
        wrap: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--theme" => a.theme = it.next().unwrap_or_default(),
            "--app-theme" => a.app_theme = it.next().unwrap_or_default(),
            "--merge" => a.merge = it.next().map(PathBuf::from),
            "--keymap" => {
                a.keymap = match it.next().as_deref() {
                    Some("vim") => EditorKeymap::Vim,
                    Some("emacs") => EditorKeymap::Emacs,
                    _ => EditorKeymap::Default,
                }
            }
            "--wrap" => a.wrap = true,
            _ => a.file = PathBuf::from(arg),
        }
    }
    a
}

/// The window root as recur-ui wants it: `window_root` > `body` > editor,
/// with the title tooltip overlay last.
struct Root {
    editor: Entity<EditorView>,
    tips: Entity<recur_ui::tooltip::TitleTooltip>,
}

impl Root {
    fn render_view(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        recur_ui::style::window_root(
            recur_ui::style::body(cx)
                .size_full()
                .child(self.editor.clone())
                .child(self.tips.clone()),
        )
    }
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

/// The repository's shipped bundles.
fn bundles_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bundles")
}

/// Every grammar in `bundles` registered (with its language data), and the
/// editor theme `theme_id` parsed (`ensureTextMateBootstrap`).
fn shipped_textmate(bundles: &Path, theme_id: &str) -> (TextMate, Option<Rc<ParsedTmTheme>>) {
    let mut tm = TextMate::default();
    let roots = recur_bundles::DiscoverRoots {
        user_dir: String::new(),
        default_dir: bundles.display().to_string(),
    };
    let Ok(catalog) = recur_bundles::discover_catalog(&roots) else {
        return (tm, None);
    };
    for g in &catalog.grammars {
        let language_data =
            recur_bundles::resolve_language_data(&catalog.language_data, &g.scope_name)
                .and_then(|d| serde_json::to_value(d).ok())
                .and_then(|v| serde_json::from_value::<LanguageDataFields>(v).ok())
                .unwrap_or_default();
        let path = g.abs_path.clone();
        tm.catalog.add_grammar(
            GrammarInfo {
                scope_name: g.scope_name.clone(),
                id: g.id.clone(),
                path: g.path.clone(),
                language: Some(g.language.clone()).filter(|s| !s.is_empty()),
                file_types: Some(g.file_types.clone()),
                first_line_match: Some(g.first_line_match.clone()).filter(|s| !s.is_empty()),
                inject_to: Some(g.inject_to.clone()).filter(|v| !v.is_empty()),
                embedded_languages: Some(g.embedded_languages.clone().into_iter().collect())
                    .filter(|m: &std::collections::HashMap<_, _>| !m.is_empty()),
                bundle_name: g.bundle_name.clone(),
                source_root: g.source_root.clone(),
                language_data,
            },
            Rc::new(move || std::fs::read_to_string(&path).ok()),
        );
    }
    let theme = catalog
        .themes
        .iter()
        .find(|t| t.id == theme_id)
        .and_then(|t| std::fs::read_to_string(&t.abs_path).ok())
        .and_then(|text| parse_theme_json(&text, theme_id).ok())
        .map(Rc::new);
    if let Some(t) = theme.clone() {
        tm.activate_theme(t);
    }
    (tm, theme)
}

fn main() {
    let a = args();
    if a.file.as_os_str().is_empty() {
        eprintln!(
            "usage: editor <file> [--theme id] [--app-theme key] [--merge base] [--keymap vim|emacs] [--wrap]"
        );
        std::process::exit(2);
    }
    let doc = std::fs::read_to_string(&a.file).unwrap_or_default();
    let original = a
        .merge
        .as_ref()
        .map(|p| std::fs::read_to_string(p).unwrap_or_default());
    gpui_kit::application()
        .with_assets(recur_ui::Assets)
        .run(move |cx: &mut App| {
            gpui_kit::init(cx);
            recur_ui::init(cx);
            recur_ui::Theme::set(recur_ui::ThemeId::from_key(&a.app_theme), cx);
            let (tm, theme) = shipped_textmate(&bundles_dir(), &a.theme);
            let tm = Rc::new(RefCell::new(tm));
            let dark = theme
                .as_ref()
                .is_none_or(|t| t.ui_theme == recur_textmate::UiTheme::Dark);
            let prefs = EditorPrefs {
                keymap: a.keymap,
                line_wrapping: a.wrap,
                ..Default::default()
            };
            let mut options = EditorOptions::new(&a.file.display().to_string(), &doc);
            options.kind = match original {
                Some(o) => EditorKind::Merge { original: o },
                None => EditorKind::Code,
            };
            options.prefs = prefs;
            options.theme = theme;
            options.dark_fallback = dark;
            options.textmate = Some(tm);
            let bounds = Bounds::centered(None, size(px(1000.), px(700.)), cx);
            let opened = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                cx,
                move |window, cx| {
                    let editor = cx.new(|cx| EditorView::new(options, window, cx));
                    cx.subscribe(&editor, |_, e: &EditorEvent, _| match e {
                        EditorEvent::DocChanged(_) | EditorEvent::Cursor { .. } => {}
                        other => eprintln!("{other:?}"),
                    })
                    .detach();
                    editor.update(cx, |e, cx| e.focus(window, cx));
                    let tips = recur_ui::tooltip::TitleTooltip::new(cx);
                    cx.new(|_| Root { editor, tips })
                },
            );
            if let Err(e) = opened {
                eprintln!("{e}");
                cx.quit();
            }
        });
}
