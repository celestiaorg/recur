//! TextMate grammar / theme catalog (`tm-registry.ts`, plus the theme pick
//! from `tm-bootstrap.ts`). Grammars and themes come from bundles as inputs;
//! file contents load lazily through the loaders the app registers.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use serde::{Deserialize, Serialize};

use crate::engine::{GrammarSourceText, TmEngine};
use crate::fold::TmFoldMarkers;
use crate::jsregex::JsRegex;
use crate::lang_data::LanguageDataFields;
use crate::node::Node;
use crate::syntax_mode::{SYNTAX_PLAIN, SyntaxMode, file_type_association_for_path};
use crate::tm_theme::{
    ParsedTmTheme, ThemeParseError, UiTheme, is_text_mate_text_scope, parse_theme_json,
};

/// A bundle `[[grammar]]` as the app lists it (Go `GrammarInfo`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GrammarInfo {
    pub scope_name: String,
    pub id: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_types: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_line_match: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inject_to: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedded_languages: Option<HashMap<String, String>>,
    pub bundle_name: String,
    pub source_root: String,
    #[serde(flatten)]
    pub language_data: LanguageDataFields,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemePreviewInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fg: Option<String>,
    #[serde(rename = "fn", skip_serializing_if = "Option::is_none")]
    pub fn_: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub string: Option<String>,
}

/// A bundle `[[theme]]` as the app lists it (Go `ThemeInfo`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ThemeInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub ui_theme: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<i64>,
    pub preview: ThemePreviewInfo,
    pub bundle_name: String,
    pub source_root: String,
}

/// Reads a grammar file's text.
pub type GrammarLoader = Rc<dyn Fn() -> Option<String>>;

/// `foldingStartMarker` / `foldingStopMarker` from grammar JSON text.
pub fn extract_fold_marker_sources(content: &str) -> Option<TmFoldMarkers> {
    if !content.trim_start().starts_with('{') {
        return None;
    }
    let node = Node::parse_json(content).ok()?;
    let start = node
        .str_field("foldingStartMarker")
        .filter(|s| !s.is_empty())?;
    let stop = node
        .str_field("foldingStopMarker")
        .filter(|s| !s.is_empty())?;
    Some(TmFoldMarkers {
        start_source: start.to_string(),
        stop_source: stop.to_string(),
    })
}

#[derive(Default)]
struct GrammarStore {
    loaders: HashMap<String, GrammarLoader>,
    /// Scope -> meta in registration order (a re-add keeps its position).
    meta: Vec<(String, GrammarInfo)>,
    fold_markers: HashMap<String, Option<TmFoldMarkers>>,
    disabled_grammar_ids: HashSet<String>,
}

impl GrammarStore {
    fn meta(&self, scope: &str) -> Option<&GrammarInfo> {
        self.meta.iter().find(|(s, _)| s == scope).map(|(_, m)| m)
    }

    fn is_disabled(&self, id: &str) -> bool {
        !id.is_empty() && self.disabled_grammar_ids.contains(id)
    }

    fn load_source(&mut self, scope: &str) -> Option<GrammarSourceText> {
        if self.meta(scope).is_some_and(|m| self.is_disabled(&m.id)) {
            return None;
        }
        let loader = self.loaders.get(scope)?.clone();
        let content = loader()?;
        self.fold_markers
            .entry(scope.to_string())
            .or_insert_with(|| extract_fold_marker_sources(&content));
        let path_hint = self
            .meta(scope)
            .map(path_hint)
            .unwrap_or_else(|| format!("{scope}.tmLanguage.json"));
        Some(GrammarSourceText { content, path_hint })
    }
}

fn path_hint(meta: &GrammarInfo) -> String {
    if meta.path.is_empty() {
        format!("{}.tmLanguage.json", meta.scope_name)
    } else {
        meta.path.clone()
    }
}

fn attach_grammars(engine: &mut TmEngine, store: &Rc<RefCell<GrammarStore>>) {
    let s = store.clone();
    engine.set_source_loader(Some(Box::new(move |scope| {
        s.borrow_mut().load_source(scope)
    })));
    for (_, meta) in &store.borrow().meta {
        engine.note_grammar(
            &meta.scope_name,
            &path_hint(meta),
            meta.inject_to.as_deref().unwrap_or(&[]),
        );
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxLanguage {
    pub scope_name: String,
    pub label: String,
    pub language: String,
}

type ColorMapListener = Box<dyn FnMut(u64)>;

pub struct TmCatalog {
    store: Rc<RefCell<GrammarStore>>,
    engine: TmEngine,
    chrome_paired: Option<TmEngine>,
    chrome_paired_theme_id: Option<String>,
    theme_meta: Vec<(String, ThemeInfo)>,
    theme_cache: HashMap<String, Rc<ParsedTmTheme>>,
    disabled_theme_ids: HashSet<String>,
    grammar_catalog_epoch: u64,
    color_map: Vec<String>,
    color_map_epoch: u64,
    color_map_subs: Vec<(u64, ColorMapListener)>,
    next_sub: u64,
    active_theme: Option<Rc<ParsedTmTheme>>,
    active_editor_background: String,
    active_root_fills: (Option<String>, Option<String>),
    pub syntax_mode: SyntaxMode,
}

impl Default for TmCatalog {
    fn default() -> Self {
        let store = Rc::new(RefCell::new(GrammarStore::default()));
        let mut engine = TmEngine::new();
        attach_grammars(&mut engine, &store);
        TmCatalog {
            store,
            engine,
            chrome_paired: None,
            chrome_paired_theme_id: None,
            theme_meta: Vec::new(),
            theme_cache: HashMap::new(),
            disabled_theme_ids: HashSet::new(),
            grammar_catalog_epoch: 0,
            color_map: Vec::new(),
            color_map_epoch: 0,
            color_map_subs: Vec::new(),
            next_sub: 0,
            active_theme: None,
            active_editor_background: "#1e1e1e".into(),
            active_root_fills: (None, None),
            syntax_mode: SyntaxMode::default(),
        }
    }
}

/// Awkward / acronym display names for the status bar and language picker.
const LANGUAGE_DISPLAY_ALIASES: &[(&str, &str)] = &[
    ("cpp", "C++"),
    ("csharp", "C#"),
    ("fsharp", "F#"),
    ("json", "JSON"),
    ("jsonc", "JSONC"),
    ("json5", "JSON5"),
    ("jsonl", "JSONL"),
    ("yaml", "YAML"),
    ("html", "HTML"),
    ("css", "CSS"),
    ("sql", "SQL"),
    ("xml", "XML"),
    ("svg", "SVG"),
    ("php", "PHP"),
    ("toml", "TOML"),
    ("jsx", "JSX"),
    ("tsx", "TSX"),
    ("objective-c", "Objective-C"),
    ("objective-cpp", "Objective-C++"),
    ("shellscript", "Shell"),
    ("wasm", "WebAssembly"),
];

pub fn language_display_name(id: &str) -> String {
    let key = id.trim();
    if key.is_empty() {
        return String::new();
    }
    let lower = key.to_lowercase();
    if let Some((_, alias)) = LANGUAGE_DISPLAY_ALIASES.iter().find(|(k, _)| *k == lower) {
        return alias.to_string();
    }
    key.split(['-', '_'])
        .map(|part| {
            let mut c = part.chars();
            match c.next() {
                Some(f) => f.to_uppercase().chain(c).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The editor theme to apply at startup: the saved one, else `recur`, else
/// any dark theme, else anything (skipping disabled themes).
pub fn pick_preferred_theme<'a>(
    themes: &'a [ThemeInfo],
    saved: &str,
    disabled: &HashSet<String>,
) -> Option<&'a ThemeInfo> {
    let ok = |t: &&ThemeInfo| !disabled.contains(&t.id);
    themes
        .iter()
        .filter(ok)
        .find(|t| t.id == saved)
        .or_else(|| themes.iter().filter(ok).find(|t| t.id == "recur"))
        .or_else(|| themes.iter().filter(ok).find(|t| t.ui_theme == "dark"))
        .or_else(|| themes.iter().find(ok))
}

impl TmCatalog {
    pub fn new() -> TmCatalog {
        TmCatalog::default()
    }

    pub fn engine(&self) -> &TmEngine {
        &self.engine
    }

    pub fn engine_mut(&mut self) -> &mut TmEngine {
        &mut self.engine
    }

    /// Sync disabled bundle grammar / theme ids from Settings.
    pub fn set_disabled_bundle_contribs(
        &mut self,
        grammar_ids: Option<&[String]>,
        theme_ids: Option<&[String]>,
    ) {
        if let Some(ids) = grammar_ids {
            let next: HashSet<String> = ids.iter().filter(|s| !s.is_empty()).cloned().collect();
            let mut store = self.store.borrow_mut();
            if store.disabled_grammar_ids != next {
                store.disabled_grammar_ids = next;
                self.grammar_catalog_epoch += 1;
            }
        }
        if let Some(ids) = theme_ids {
            self.disabled_theme_ids = ids.iter().filter(|s| !s.is_empty()).cloned().collect();
        }
    }

    pub fn is_grammar_disabled(&self, id: &str) -> bool {
        self.store.borrow().is_disabled(id)
    }

    pub fn is_theme_disabled(&self, id: &str) -> bool {
        !id.is_empty() && self.disabled_theme_ids.contains(id)
    }

    pub fn disabled_theme_ids(&self) -> &HashSet<String> {
        &self.disabled_theme_ids
    }

    /// Bumps when grammars register or toggle (status bar label recompute).
    pub fn grammar_catalog_epoch(&self) -> u64 {
        self.grammar_catalog_epoch
    }

    /// Markers recorded when the grammar last loaded.
    pub fn fold_marker_sources(&self, scope: &str) -> Option<TmFoldMarkers> {
        self.store
            .borrow()
            .fold_markers
            .get(scope)
            .cloned()
            .flatten()
    }

    /// Register a grammar loader for a scope (overwrites a prior one).
    pub fn add_grammar(&mut self, meta: GrammarInfo, loader: GrammarLoader) {
        let scope = meta.scope_name.clone();
        let hint = path_hint(&meta);
        let inject: Vec<String> = meta.inject_to.clone().unwrap_or_default();
        {
            let mut store = self.store.borrow_mut();
            store.loaders.insert(scope.clone(), loader);
            match store.meta.iter_mut().find(|(s, _)| *s == scope) {
                Some(slot) => slot.1 = meta,
                None => store.meta.push((scope.clone(), meta)),
            }
            store.fold_markers.remove(&scope);
        }
        self.engine.note_grammar(&scope, &hint, &inject);
        if let Some(e) = self.chrome_paired.as_mut() {
            e.note_grammar(&scope, &hint, &inject);
        }
        self.grammar_catalog_epoch += 1;
    }

    pub fn list_grammar_meta(&self) -> Vec<GrammarInfo> {
        self.store
            .borrow()
            .meta
            .iter()
            .map(|(_, m)| m.clone())
            .collect()
    }

    pub fn grammar_meta(&self, scope: &str) -> Option<GrammarInfo> {
        self.store.borrow().meta(scope).cloned()
    }

    /// Load + compile a grammar. True when the scope is ready.
    pub fn load_grammar(&mut self, scope: &str) -> bool {
        self.engine.ensure_grammar(scope).unwrap_or(false)
    }

    pub fn has_grammar(&self, scope: &str) -> bool {
        self.engine.has_grammar(scope)
    }

    /// A second tokenizer themed to `theme_id` (settings script editors), so
    /// they can paint without restyling the code editor.
    pub fn ensure_chrome_paired_engine(&mut self, theme_id: &str) -> Option<&mut TmEngine> {
        let parsed = self.theme_cache.get(theme_id)?.clone();
        if self.chrome_paired.is_none() {
            let mut e = TmEngine::new();
            attach_grammars(&mut e, &self.store);
            self.chrome_paired = Some(e);
        }
        let engine = self.chrome_paired.as_mut()?;
        if self.chrome_paired_theme_id.as_deref() != Some(theme_id) {
            engine.set_theme(parsed.raw_theme.clone());
            self.chrome_paired_theme_id = Some(theme_id.to_string());
        }
        Some(engine)
    }

    pub fn chrome_paired_theme_id(&self) -> Option<&str> {
        self.chrome_paired_theme_id.as_deref()
    }

    /// Free token caches for closed files. `force` drops documents a view
    /// still holds (the buffer itself was discarded, e.g. reload from disk).
    pub fn forget_docs<'a>(&mut self, doc_ids: impl IntoIterator<Item = &'a str>, force: bool) {
        for id in doc_ids {
            if force {
                self.engine.close_doc(id);
            } else {
                self.engine.close_if_idle(id);
            }
        }
    }

    /// Apply a theme to the main tokenizer. Re-applying the active parsed
    /// theme is a no-op (no cache flush, no repaint).
    pub fn set_active_theme(&mut self, parsed: Rc<ParsedTmTheme>) {
        if self
            .active_theme
            .as_ref()
            .is_some_and(|a| Rc::ptr_eq(a, &parsed))
        {
            return;
        }
        let map = self.engine.set_theme(parsed.raw_theme.clone());
        self.active_theme = Some(parsed);
        self.apply_color_map(map, true);
    }

    pub fn active_theme(&self) -> Option<&Rc<ParsedTmTheme>> {
        self.active_theme.as_ref()
    }

    pub fn color_map(&self) -> &[String] {
        &self.color_map
    }

    pub fn color_map_epoch(&self) -> u64 {
        self.color_map_epoch
    }

    pub fn on_color_map_change(&mut self, f: ColorMapListener) -> u64 {
        self.next_sub += 1;
        self.color_map_subs.push((self.next_sub, f));
        self.next_sub
    }

    pub fn off_color_map_change(&mut self, id: u64) {
        self.color_map_subs.retain(|(i, _)| *i != id);
    }

    /// Publish a colour map. `reset` when the engine dropped its documents:
    /// two themes can share a map, and listeners must still re-open.
    pub fn apply_color_map(&mut self, next: Vec<String>, reset: bool) {
        if !reset && self.color_map == next {
            return;
        }
        self.color_map = next;
        self.color_map_epoch += 1;
        let epoch = self.color_map_epoch;
        for (_, f) in self.color_map_subs.iter_mut() {
            f(epoch);
        }
    }

    pub fn set_active_editor_background(&mut self, color: &str) {
        let c = color.trim();
        if !c.is_empty() {
            self.active_editor_background = c.to_string();
        }
    }

    pub fn active_editor_background(&self) -> &str {
        &self.active_editor_background
    }

    pub fn set_active_root_backgrounds(&mut self, source: Option<&str>, text: Option<&str>) {
        let clean = |s: Option<&str>| {
            s.map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        self.active_root_fills = (clean(source), clean(text));
    }

    pub fn active_root_backgrounds(&self) -> Vec<String> {
        [&self.active_root_fills.0, &self.active_root_fills.1]
            .into_iter()
            .flatten()
            .cloned()
            .collect()
    }

    /// The fill tokens sit on for a grammar (`text` vs `source` root).
    pub fn active_pane_background(&self, scope: &str) -> String {
        let (source, text) = &self.active_root_fills;
        match (is_text_mate_text_scope(scope), text, source) {
            (true, Some(t), _) => t.clone(),
            (false, _, Some(s)) => s.clone(),
            _ => self.active_editor_background.clone(),
        }
    }

    pub fn register_theme_meta(&mut self, info: ThemeInfo, parsed: Rc<ParsedTmTheme>) {
        self.theme_cache.insert(info.id.clone(), parsed);
        match self.theme_meta.iter_mut().find(|(id, _)| *id == info.id) {
            Some(slot) => slot.1 = info,
            None => self.theme_meta.push((info.id.clone(), info)),
        }
    }

    pub fn theme_meta(&self, id: &str) -> Option<&ThemeInfo> {
        self.theme_meta
            .iter()
            .find(|(i, _)| i == id)
            .map(|(_, t)| t)
    }

    pub fn parsed_theme(&self, id: &str) -> Option<Rc<ParsedTmTheme>> {
        self.theme_cache.get(id).cloned()
    }

    pub fn list_theme_meta(&self) -> Vec<ThemeInfo> {
        self.theme_meta.iter().map(|(_, t)| t.clone()).collect()
    }

    /// Parse and cache a theme (the bundle's `uiTheme` wins over JSON `type`).
    pub fn load_and_register_theme(
        &mut self,
        info: ThemeInfo,
        read: impl FnOnce() -> Option<String>,
    ) -> Result<Rc<ParsedTmTheme>, ThemeParseError> {
        if let Some(cached) = self.theme_cache.get(&info.id) {
            return Ok(cached.clone());
        }
        let text = read().unwrap_or_default();
        let mut parsed = parse_theme_json(&text, &info.name)?;
        if let Some(ui) = UiTheme::parse(&info.ui_theme) {
            parsed.ui_theme = ui;
        }
        let parsed = Rc::new(parsed);
        self.register_theme_meta(info, parsed.clone());
        Ok(parsed)
    }

    /// Grammar claimed by fileTypes / firstLineMatch, ignoring overrides.
    pub fn detect_scope_for_path(&self, path: &str, first_line: Option<&str>) -> Option<String> {
        let store = self.store.borrow();
        let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let lower = base.to_lowercase();
        let ext = lower.rfind('.').map(|i| &lower[i + 1..]).unwrap_or("");

        if lower == ".env" || lower.starts_with(".env.") {
            for (_, g) in &store.meta {
                if store.is_disabled(&g.id) {
                    continue;
                }
                if g.language.as_deref() == Some("dotenv")
                    || g.scope_name == "source.dotenv"
                    || g.file_types
                        .iter()
                        .flatten()
                        .any(|ft| ft.to_lowercase() == "env")
                {
                    return Some(g.scope_name.clone());
                }
            }
        }

        let mut best: Option<(String, usize)> = None;
        for (_, g) in &store.meta {
            if store.is_disabled(&g.id) {
                continue;
            }
            for ft in g.file_types.iter().flatten() {
                let ft_lower = ft.to_lowercase();
                let needle = ft_lower.strip_prefix('.').unwrap_or(&ft_lower);
                let n = needle.chars().count();
                let suffix = format!(".{needle}");
                let score = if needle == lower {
                    1000 + n
                } else if !ext.is_empty() && needle == ext {
                    100 + n
                } else if needle.contains('.') && lower.ends_with(&suffix) {
                    200 + n
                } else if lower.ends_with(&suffix) {
                    50 + n
                } else {
                    continue;
                };
                if best.as_ref().is_none_or(|(_, b)| score > *b) {
                    best = Some((g.scope_name.clone(), score));
                }
            }
        }
        if let Some((scope, _)) = best {
            return Some(scope);
        }

        let first = first_line.filter(|f| !f.is_empty())?;
        for (_, g) in &store.meta {
            if store.is_disabled(&g.id) {
                continue;
            }
            let Some(src) = g.first_line_match.as_deref().filter(|s| !s.is_empty()) else {
                continue;
            };
            if JsRegex::new(src, "").is_some_and(|re| re.test(first)) {
                return Some(g.scope_name.clone());
            }
        }
        None
    }

    /// User override first, then detection. `None` is Plain Text.
    pub fn resolve_scope_for_path(&self, path: &str, first_line: Option<&str>) -> Option<String> {
        if let Some(o) = self.syntax_mode.override_for_path(path) {
            return (!o.is_empty()).then_some(o);
        }
        self.detect_scope_for_path(path, first_line)
    }

    pub fn language_label_for_scope(&self, scope: &str) -> String {
        if let Some(lang) = self
            .grammar_meta(scope)
            .and_then(|m| m.language)
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
        {
            return language_display_name(&lang);
        }
        for prefix in ["source.", "text."] {
            if scope.len() > prefix.len() && scope[..prefix.len()].eq_ignore_ascii_case(prefix) {
                let rest: String = scope[prefix.len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-'))
                    .collect();
                if !rest.is_empty() {
                    return language_display_name(&rest.replace('_', "-"));
                }
            }
        }
        scope.to_string()
    }

    pub fn language_label_for_path(&self, path: &str) -> String {
        self.resolve_scope_for_path(path, None)
            .map(|s| self.language_label_for_scope(&s))
            .unwrap_or_default()
    }

    /// Root grammars for the language-mode picker, sorted by label.
    pub fn list_syntax_languages(&self) -> Vec<SyntaxLanguage> {
        let mut out: Vec<SyntaxLanguage> = Vec::new();
        let mut seen = HashSet::new();
        for g in self.list_grammar_meta() {
            if !seen.insert(g.scope_name.clone())
                || g.inject_to.as_ref().is_some_and(|i| !i.is_empty())
            {
                continue;
            }
            out.push(SyntaxLanguage {
                label: self.language_label_for_scope(&g.scope_name),
                language: g
                    .language
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or("")
                    .to_string(),
                scope_name: g.scope_name,
            });
        }
        out.sort_by_key(|a| a.label.to_lowercase());
        out
    }

    /// Apply a language-mode pick: `None` = Auto Detect, `Some("")` = Plain
    /// Text. Picking what already applies clears the path override.
    pub fn apply_language_mode(&mut self, path: &str, scope: Option<&str>) {
        let assoc = file_type_association_for_path(path);
        let assoc_scope = assoc.and_then(|a| self.syntax_mode.file_type_override(&a.key));
        let detected = self.detect_scope_for_path(path, None);
        let without_path = match &assoc_scope {
            Some(s) => (!s.is_empty()).then(|| s.clone()),
            None => detected.clone(),
        };
        let Some(scope) = scope else {
            if assoc_scope.is_some() && detected != without_path {
                let pick = detected.unwrap_or_else(|| SYNTAX_PLAIN.to_string());
                self.syntax_mode.set_for_path(path, Some(&pick));
            } else {
                self.syntax_mode.set_for_path(path, None);
            }
            return;
        };
        if (scope.is_empty() && without_path.is_none())
            || (!scope.is_empty() && Some(scope) == without_path.as_deref())
        {
            self.syntax_mode.set_for_path(path, None);
            return;
        }
        self.syntax_mode.set_for_path(path, Some(scope));
    }

    /// Drop bundle grammars and themes before a reload.
    pub fn forget_bundle_contribs(&mut self) {
        {
            let mut store = self.store.borrow_mut();
            store.loaders.clear();
            store.meta.clear();
            store.fold_markers.clear();
        }
        self.theme_cache.clear();
        self.theme_meta.clear();
        self.chrome_paired_theme_id = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_source_without_a_path_loads() {
        let mut c = TmCatalog::new();
        c.add_grammar(
            GrammarInfo {
                scope_name: "source.x".into(),
                ..Default::default()
            },
            Rc::new(|| {
                Some(r#"{"scopeName":"source.x","patterns":[{"match":"x","name":"k"}]}"#.into())
            }),
        );
        assert!(c.load_grammar("source.x"));
    }

    fn grammar(id: &str, scope: &str, language: Option<&str>, file_types: &[&str]) -> GrammarInfo {
        GrammarInfo {
            id: id.into(),
            scope_name: scope.into(),
            path: format!("syntaxes/{scope}.tmLanguage.json"),
            language: language.map(Into::into),
            file_types: Some(file_types.iter().map(|s| s.to_string()).collect()),
            bundle_name: "Languages".into(),
            source_root: "default".into(),
            ..GrammarInfo::default()
        }
    }

    fn empty() -> GrammarLoader {
        Rc::new(|| Some("{}".to_string()))
    }

    #[test]
    fn matches_file_types_including_dotted_extensions() {
        let mut c = TmCatalog::new();
        c.add_grammar(
            grammar("recur.languages.demo", "source.demo", None, &["demo"]),
            empty(),
        );
        assert_eq!(
            c.resolve_scope_for_path("foo/bar.demo", None).as_deref(),
            Some("source.demo")
        );
        assert_eq!(c.resolve_scope_for_path("readme.md", None), None);
    }

    #[test]
    fn prefers_basename_matches_and_maps_env_files() {
        let mut c = TmCatalog::new();
        c.add_grammar(
            grammar(
                "recur.languages.dotenv",
                "source.dotenv",
                Some("dotenv"),
                &["env"],
            ),
            empty(),
        );
        c.add_grammar(
            grammar("recur.languages.go", "source.go", None, &["go", "go.mod"]),
            empty(),
        );
        assert_eq!(
            c.resolve_scope_for_path(".env", None).as_deref(),
            Some("source.dotenv")
        );
        assert_eq!(
            c.resolve_scope_for_path(".env.local", None).as_deref(),
            Some("source.dotenv")
        );
        assert_eq!(
            c.resolve_scope_for_path("go.mod", None).as_deref(),
            Some("source.go")
        );
    }

    #[test]
    fn language_label_prefers_grammar_language() {
        let mut c = TmCatalog::new();
        c.add_grammar(
            grammar(
                "recur.languages.solidity",
                "source.solidity",
                Some("solidity"),
                &["sol"],
            ),
            empty(),
        );
        assert_eq!(c.language_label_for_path("contracts/Token.sol"), "Solidity");
        assert_eq!(c.language_label_for_path("readme.unknown"), "");
    }

    #[test]
    fn skips_disabled_grammars() {
        let mut c = TmCatalog::new();
        c.add_grammar(
            grammar(
                "recur.languages.demo",
                "source.demo",
                Some("demo"),
                &["demo"],
            ),
            empty(),
        );
        assert_eq!(
            c.resolve_scope_for_path("foo/bar.demo", None).as_deref(),
            Some("source.demo")
        );
        c.set_disabled_bundle_contribs(Some(&["recur.languages.demo".into()]), None);
        assert_eq!(c.resolve_scope_for_path("foo/bar.demo", None), None);
        assert_eq!(c.language_label_for_path("foo/bar.demo"), "");
        c.set_disabled_bundle_contribs(Some(&[]), None);
        assert_eq!(
            c.resolve_scope_for_path("foo/bar.demo", None).as_deref(),
            Some("source.demo")
        );
    }

    #[test]
    fn honours_a_user_language_mode_override() {
        let mut c = TmCatalog::new();
        c.add_grammar(
            grammar("recur.languages.go", "source.go", Some("go"), &["go"]),
            empty(),
        );
        c.add_grammar(
            grammar(
                "recur.languages.yaml",
                "source.yaml",
                Some("yaml"),
                &["yaml", "yml"],
            ),
            empty(),
        );
        assert_eq!(
            c.resolve_scope_for_path("src/main.go", None).as_deref(),
            Some("source.go")
        );
        c.syntax_mode
            .set_for_path("src/main.go", Some("source.yaml"));
        assert_eq!(
            c.resolve_scope_for_path("src/main.go", None).as_deref(),
            Some("source.yaml")
        );
        assert_eq!(c.language_label_for_path("src/main.go"), "YAML");
        assert_eq!(
            c.detect_scope_for_path("src/main.go", None).as_deref(),
            Some("source.go")
        );
        c.syntax_mode.set_for_path("src/main.go", None);
        assert_eq!(
            c.resolve_scope_for_path("src/main.go", None).as_deref(),
            Some("source.go")
        );
        c.apply_language_mode("src/main.go", Some("source.yaml"));
        assert_eq!(
            c.syntax_mode.path_override("src/main.go").as_deref(),
            Some("source.yaml")
        );
        c.apply_language_mode("src/main.go", Some("source.go"));
        assert_eq!(c.syntax_mode.path_override("src/main.go"), None);
    }

    #[test]
    fn stores_a_path_override_against_a_file_type_association() {
        let mut c = TmCatalog::new();
        c.add_grammar(
            grammar(
                "recur.languages.markdown",
                "text.html.markdown",
                Some("markdown"),
                &["md"],
            ),
            empty(),
        );
        c.add_grammar(
            grammar(
                "recur.languages.yaml",
                "source.yaml",
                Some("yaml"),
                &["yaml", "yml"],
            ),
            empty(),
        );
        c.syntax_mode.set_for_file_type("*.md", Some("source.yaml"));
        assert_eq!(
            c.resolve_scope_for_path("README.md", None).as_deref(),
            Some("source.yaml")
        );
        c.apply_language_mode("README.md", Some("text.html.markdown"));
        assert_eq!(
            c.syntax_mode.path_override("README.md").as_deref(),
            Some("text.html.markdown")
        );
        assert_eq!(
            c.resolve_scope_for_path("README.md", None).as_deref(),
            Some("text.html.markdown")
        );
        c.apply_language_mode("README.md", None);
        assert_eq!(
            c.syntax_mode.path_override("README.md").as_deref(),
            Some("text.html.markdown")
        );
        c.apply_language_mode("README.md", Some("source.yaml"));
        assert_eq!(c.syntax_mode.path_override("README.md"), None);
        assert_eq!(
            c.resolve_scope_for_path("README.md", None).as_deref(),
            Some("source.yaml")
        );
    }

    #[test]
    fn omits_injection_only_grammars_from_the_picker() {
        let mut c = TmCatalog::new();
        c.add_grammar(
            grammar("recur.languages.go", "source.go", Some("go"), &["go"]),
            empty(),
        );
        let mut jsdoc = grammar("recur.languages.jsdoc", "source.jsdoc", Some("jsdoc"), &[]);
        jsdoc.inject_to = Some(vec!["source.js".into(), "source.ts".into()]);
        c.add_grammar(jsdoc, empty());
        let langs: Vec<String> = c
            .list_syntax_languages()
            .into_iter()
            .map(|l| l.scope_name)
            .collect();
        assert_eq!(langs, ["source.go"]);
    }

    #[test]
    fn display_names() {
        assert_eq!(language_display_name("go"), "Go");
        assert_eq!(language_display_name("cpp"), "C++");
        assert_eq!(language_display_name("json"), "JSON");
        assert_eq!(language_display_name("objective-c"), "Objective-C");
        assert_eq!(language_display_name("common_lisp"), "Common Lisp");
    }

    #[test]
    fn identical_color_maps_stay_quiet_unless_reset() {
        let mut c = TmCatalog::new();
        let hits = Rc::new(RefCell::new(0));
        c.apply_color_map(vec!["".into(), "#ccc".into(), "#111".into()], false);
        let epoch = c.color_map_epoch();
        let h = hits.clone();
        c.on_color_map_change(Box::new(move |_| *h.borrow_mut() += 1));
        c.apply_color_map(vec!["".into(), "#ccc".into(), "#111".into()], false);
        assert_eq!(c.color_map_epoch(), epoch);
        assert_eq!(*hits.borrow(), 0);
        c.apply_color_map(vec!["".into(), "#ddd".into()], false);
        assert_eq!(c.color_map_epoch(), epoch + 1);
        assert_eq!(*hits.borrow(), 1);
        c.apply_color_map(vec!["".into(), "#ddd".into()], true);
        assert_eq!(*hits.borrow(), 2);
    }

    #[test]
    fn fold_markers_come_from_grammar_json() {
        let src = extract_fold_marker_sources(
            r#"{"scopeName":"source.scala","foldingStartMarker":"/\\*\\*|\\{\\s*$","foldingStopMarker":"\\*\\*/|^\\s*}","patterns":[]}"#,
        );
        assert_eq!(
            src,
            Some(TmFoldMarkers {
                start_source: "/\\*\\*|\\{\\s*$".into(),
                stop_source: "\\*\\*/|^\\s*}".into(),
            })
        );
        assert_eq!(
            extract_fold_marker_sources(r#"{"scopeName":"source.go","patterns":[]}"#),
            None
        );
    }

    #[test]
    fn picks_the_saved_theme_then_recur_then_dark() {
        let t = |id: &str, ui: &str| ThemeInfo {
            id: id.into(),
            ui_theme: ui.into(),
            ..ThemeInfo::default()
        };
        let themes = vec![t("paper", "light"), t("glow", "dark"), t("recur", "dark")];
        let none = HashSet::new();
        assert_eq!(
            pick_preferred_theme(&themes, "paper", &none).unwrap().id,
            "paper"
        );
        assert_eq!(
            pick_preferred_theme(&themes, "gone", &none).unwrap().id,
            "recur"
        );
        let off = HashSet::from(["recur".to_string()]);
        assert_eq!(
            pick_preferred_theme(&themes, "gone", &off).unwrap().id,
            "glow"
        );
    }
}
