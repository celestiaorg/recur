//! Gallery scenes (`editor/*` in `crates/ui/gallery/scenes.json`): the
//! code editor on a Go file with the shipped bundles' grammars and editor
//! themes.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::{AnyView, App, AppContext, Window};
use recur_textmate::{GrammarInfo, LanguageDataFields, ParsedTmTheme, parse_theme_json};

use crate::highlight::TextMate;
use crate::view::{EditorOptions, EditorView};

/// The scenes' document.
pub const DOC: &str = r#"package main

import (
	"fmt"
	"strings"
)

// greet joins names for the banner.
func greet(names []string) string {
	return "hello, " + strings.Join(names, ", ")
}

func main() {
	fmt.Println(greet([]string{"recur", "gpui"}))
}
"#;

/// The repository's shipped bundles.
pub fn bundles_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bundles")
}

/// Every grammar in `bundles` registered (with its language data), and the
/// editor theme `theme_id` parsed (`ensureTextMateBootstrap`).
pub fn shipped_textmate(bundles: &Path, theme_id: &str) -> (TextMate, Option<Rc<ParsedTmTheme>>) {
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

/// The scenes this gallery builds.
pub const NAMES: [&str; 4] = ["editor/code", "editor/paper", "editor/find", "editor/vim"];

/// Whether `name` is one of [`NAMES`].
pub fn exists(name: &str) -> bool {
    NAMES.contains(&name)
}

/// The view for an `editor/*` scene; `None` for other scenes.
pub fn build(name: &str, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    if !exists(name) {
        return None;
    }
    let variant = name.strip_prefix("editor/")?;
    let theme_id = if variant == "paper" { "paper" } else { "recur" };
    let (tm, theme) = shipped_textmate(&bundles_dir(), theme_id);
    let mut options = EditorOptions::new("main.go", DOC);
    options.dark_fallback = theme
        .as_ref()
        .is_none_or(|t| t.ui_theme == recur_textmate::UiTheme::Dark);
    options.theme = theme;
    if variant == "vim" {
        options.prefs.keymap = crate::prefs::EditorKeymap::Vim;
    }
    options.textmate = Some(Rc::new(RefCell::new(tm)));
    let view = cx.new(|cx| EditorView::new(options, window, cx));
    Some(view.into())
}
