//! Opens a file in the code editor with the shipped bundles' grammars and
//! themes: `cargo run -p recur-editor --example editor -- <file>`.
//!
//! Options: `--theme <id>` (bundle editor theme, default `recur`),
//! `--app-theme <key>` (app chrome, default `default`), `--merge <base>`
//! (unified merge view against a base file), `--keymap vim|emacs`,
//! `--wrap`.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, Styled, Window,
    WindowBounds, WindowOptions, px, size,
};
use recur_editor::prefs::{EditorKeymap, EditorPrefs};
use recur_editor::view::{EditorEvent, EditorKind, EditorOptions, EditorView};

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
            let (tm, theme) = recur_editor::gallery::shipped_textmate(
                &recur_editor::gallery::bundles_dir(),
                &a.theme,
            );
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
