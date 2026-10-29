//! The app chrome, code editor and terminal theme picks on [`AppState`], each with its
//! "sync with system" light/dark pair (`lib/stores.ts`), and the catalogs the Appearance menu
//! lists.

use std::cmp::Ordering;

use gpui_kit::{App, Context, SharedString};
use recur_editor::theme::catalog::EditorThemeCard;
use recur_editor::theme::catalog::{
    DEFAULT_THEME_ID, PINNED_EDITOR_THEME_KEYS, compare_for_cycle, cycle_key, looks_like_theme_key,
};
use recur_services::api::ThemeInfo;
use recur_ui::theme::{Theme, ThemeId};
use recur_ui::widgets::ToastStore;

use crate::prefs::{self, SyncName, TerminalPrefs};
use crate::state::AppState;

/// `isLightTheme`: light app-chrome palettes.
pub fn is_light_theme(key: &str) -> bool {
    ThemeId::from_key(key).is_light()
}

fn name_cmp(a: &str, b: &str) -> Ordering {
    recur_editor::text::locale_compare(a, b)
}

impl AppState {
    pub(crate) fn init_themes(&mut self) {
        // `initTheme` → `applyTheme(saved)`: a legacy key is stored under its current name.
        let saved = self.appearance.theme.clone();
        self.apply_app_theme(&saved);
        self.reapply_syncs();
    }

    /// Paint the app chrome theme this state resolves to.
    pub fn apply_theme_global(&self, cx: &mut App) {
        let id = ThemeId::from_key(&self.appearance.theme);
        if Theme::of(cx).id != id {
            Theme::set(id, cx);
        }
    }

    fn apply_app_theme(&mut self, key: &str) {
        let id = ThemeId::from_key(key);
        self.appearance.theme = id.key().into();
        prefs::save_theme(&mut self.storage, id.key());
    }

    /// Every sync applies its pick for the current system appearance while on (the
    /// `systemDark.subscribe(run)` in each `createSystemSync`).
    fn reapply_syncs(&mut self) {
        let dark = self.system_dark;
        if let Some(pick) = self.appearance.sync.pick(dark).map(str::to_string) {
            self.apply_app_theme(&pick);
        }
        if let Some(pick) = self.editor_sync.pick(dark).map(str::to_string) {
            self.apply_editor_theme(&pick);
        }
        if let Some(pick) = self.terminal_sync.pick(dark).map(str::to_string) {
            self.patch_terminal_theme(&pick);
        }
    }

    /// The native dark-mode answer (`system.theme`, window appearance changes).
    pub fn set_native_dark(&mut self, dark: bool, cx: &mut Context<Self>) {
        self.native_dark = Some(dark);
        prefs::save_system_dark(&mut self.storage, dark);
        if self.system_dark != dark {
            self.system_dark = dark;
            self.reapply_syncs();
        }
        self.apply_theme_global(cx);
        cx.notify();
    }

    // ---- app chrome ----

    /// `setTheme`: an explicit pick applies and turns system follow off.
    pub fn set_theme(&mut self, key: &str, cx: &mut Context<Self>) {
        self.apply_app_theme(key);
        self.appearance.sync.auto = false;
        SyncName::Theme.save_auto(&mut self.storage, false);
        self.apply_theme_global(cx);
        cx.notify();
    }

    /// `setThemeAuto`.
    pub fn set_theme_auto(&mut self, on: bool, cx: &mut Context<Self>) {
        self.appearance.sync.auto = on;
        SyncName::Theme.save_auto(&mut self.storage, on);
        self.reapply_syncs();
        self.apply_theme_global(cx);
        cx.notify();
    }

    /// `setSystemTheme(mode, key)`.
    pub fn set_system_theme(&mut self, dark: bool, key: &str, cx: &mut Context<Self>) {
        if dark {
            self.appearance.sync.dark = key.into();
        } else {
            self.appearance.sync.light = key.into();
        }
        SyncName::Theme.save_pick(&mut self.storage, dark, key);
        self.reapply_syncs();
        self.apply_theme_global(cx);
        cx.notify();
    }

    // ---- code editor ----

    fn theme_disabled(&self, id: &str) -> bool {
        !id.is_empty()
            && self
                .settings
                .as_ref()
                .is_some_and(|s| s.disabled_bundle_themes.iter().any(|t| t == id))
    }

    /// `isEditorThemeKey`: a catalogued theme, or a shipped-looking id before the catalog
    /// loads.
    fn is_editor_theme_key(&self, key: &str) -> bool {
        !key.is_empty()
            && (self.editor_theme_catalog.iter().any(|t| t.id == key) || looks_like_theme_key(key))
    }

    fn resolve_editor_theme_key(&self, key: &str) -> String {
        if self.is_editor_theme_key(key) {
            key.to_string()
        } else {
            DEFAULT_THEME_ID.to_string()
        }
    }

    /// `fallbackEnabledEditorThemeId`.
    fn fallback_enabled_editor_theme(&self, current: &str) -> String {
        if !current.is_empty() && !self.theme_disabled(current) && self.is_editor_theme_key(current)
        {
            return current.to_string();
        }
        let enabled: Vec<&ThemeInfo> = self
            .editor_theme_catalog
            .iter()
            .filter(|t| !t.id.is_empty() && !self.theme_disabled(&t.id))
            .collect();
        for id in PINNED_EDITOR_THEME_KEYS {
            if enabled.iter().any(|t| t.id == id) {
                return id.to_string();
            }
        }
        if let Some(first) = enabled.first() {
            return first.id.clone();
        }
        if current.is_empty() {
            DEFAULT_THEME_ID.to_string()
        } else {
            current.to_string()
        }
    }

    /// `applyEditorTheme`.
    fn apply_editor_theme(&mut self, key: &str) {
        let resolved = self.resolve_editor_theme_key(key);
        let valid = self.fallback_enabled_editor_theme(&resolved);
        prefs::save_editor_theme(&mut self.storage, &valid);
        self.editor_theme = valid;
    }

    /// `setEditorTheme`: an explicit pick turns system follow off.
    pub fn set_editor_theme(&mut self, key: &str, cx: &mut Context<Self>) {
        self.apply_editor_theme(key);
        self.editor_sync.auto = false;
        SyncName::EditorTheme.save_auto(&mut self.storage, false);
        cx.notify();
    }

    pub fn set_editor_theme_auto(&mut self, on: bool, cx: &mut Context<Self>) {
        self.editor_sync.auto = on;
        SyncName::EditorTheme.save_auto(&mut self.storage, on);
        self.reapply_syncs();
        cx.notify();
    }

    pub fn set_system_editor_theme(&mut self, dark: bool, key: &str, cx: &mut Context<Self>) {
        if dark {
            self.editor_sync.dark = key.into();
        } else {
            self.editor_sync.light = key.into();
        }
        SyncName::EditorTheme.save_pick(&mut self.storage, dark, key);
        self.reapply_syncs();
        cx.notify();
    }

    /// `allEditorThemesForPicker`: enabled bundle themes, pinned first, then by name. Preview
    /// colors come from the theme's `[[theme]].preview`; the Settings picker fills the rest
    /// from the parsed theme.
    pub fn editor_themes_for_picker(&self) -> Vec<EditorThemeCard> {
        let pinned_rank = |id: &str| PINNED_EDITOR_THEME_KEYS.iter().position(|k| *k == id);
        let mut themes: Vec<&ThemeInfo> = self
            .editor_theme_catalog
            .iter()
            .filter(|t| !self.theme_disabled(&t.id))
            .collect();
        themes.sort_by(|a, b| match (pinned_rank(&a.id), pinned_rank(&b.id)) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => name_cmp(&a.name, &b.name),
        });
        themes
            .into_iter()
            .map(|t| {
                let p = &t.preview;
                let or = |v: &str, d: &str| {
                    if v.is_empty() {
                        d.to_string()
                    } else {
                        v.to_string()
                    }
                };
                let bg = or(&p.bg, "#1e1e1e");
                let fg = or(&p.fg, "#cccccc");
                let a = or(&p.name, &fg);
                EditorThemeCard {
                    key: t.id.clone(),
                    name: t.name.clone(),
                    dark: t.ui_theme != "light",
                    bg,
                    fg,
                    b: or(&p.r#fn, &a),
                    c: or(&p.string, &a),
                    a,
                    pinned: pinned_rank(&t.id).is_some(),
                }
            })
            .collect()
    }

    /// `editorThemesForCycle`: dark A–Z (Charcoal first), then light A–Z (Paper first).
    pub fn editor_themes_for_cycle(&self) -> Vec<EditorThemeCard> {
        let mut cards = self.editor_themes_for_picker();
        cards.sort_by(compare_for_cycle);
        cards
    }

    /// `cycleEditorTheme`: step through the themes and return the new `(key, name)`.
    pub fn cycle_editor_theme(&mut self, delta: i64, cx: &mut Context<Self>) -> (String, String) {
        let cards = self.editor_themes_for_cycle();
        let keys: Vec<String> = cards.iter().map(|c| c.key.clone()).collect();
        let current = self.resolve_editor_theme_key(&self.editor_theme);
        let key = if keys.is_empty() {
            current
        } else {
            cycle_key(&current, delta, &keys)
        };
        self.set_editor_theme(&key, cx);
        let name = cards
            .iter()
            .find(|c| c.key == key)
            .map(|c| c.name.clone())
            .unwrap_or_else(|| key.clone());
        (key, name)
    }

    /// The name the editor-theme toast shows for `key`.
    pub fn editor_theme_name(&self, key: &str) -> String {
        self.editor_themes_for_picker()
            .into_iter()
            .find(|t| t.key == key)
            .map(|t| t.name)
            .unwrap_or_else(|| key.to_string())
    }

    /// Settings changed: re-validate the editor theme against the disabled set.
    pub(crate) fn on_settings_changed(&mut self, _cx: &mut Context<Self>) {
        let current = self.editor_theme.clone();
        if self.theme_disabled(&current) {
            self.apply_editor_theme(&current);
        }
    }

    // ---- terminal ----

    fn patch_terminal_theme(&mut self, theme: &str) {
        self.terminal_prefs.theme = theme.to_string();
        self.terminal_prefs.save(&mut self.storage);
    }

    /// `setTerminalPrefs` for the font fields.
    pub fn set_terminal_prefs(
        &mut self,
        f: impl FnOnce(&mut TerminalPrefs),
        cx: &mut Context<Self>,
    ) {
        let mut next = self.terminal_prefs.clone();
        f(&mut next);
        let raw = serde_json::to_value(&next).ok();
        self.terminal_prefs = TerminalPrefs::normalize(raw.as_ref());
        self.terminal_prefs.save(&mut self.storage);
        cx.notify();
    }

    /// `setTerminalPrefs({ theme })`: an explicit pick turns system follow off.
    pub fn set_terminal_theme(&mut self, id: &str, cx: &mut Context<Self>) {
        self.patch_terminal_theme(id);
        self.terminal_sync.auto = false;
        SyncName::TerminalTheme.save_auto(&mut self.storage, false);
        cx.notify();
    }

    pub fn set_terminal_theme_auto(&mut self, on: bool, cx: &mut Context<Self>) {
        self.terminal_sync.auto = on;
        SyncName::TerminalTheme.save_auto(&mut self.storage, on);
        self.reapply_syncs();
        cx.notify();
    }

    pub fn set_system_terminal_theme(&mut self, dark: bool, key: &str, cx: &mut Context<Self>) {
        if dark {
            self.terminal_sync.dark = key.into();
        } else {
            self.terminal_sync.light = key.into();
        }
        SyncName::TerminalTheme.save_pick(&mut self.storage, dark, key);
        self.reapply_syncs();
        cx.notify();
    }

    // ---- editor and explorer prefs ----

    /// `setEditorPrefs`: merge, normalize and persist.
    pub fn set_editor_prefs(
        &mut self,
        f: impl FnOnce(&mut recur_editor::prefs::EditorPrefs),
        cx: &mut Context<Self>,
    ) {
        let mut next = self.editor_prefs.clone();
        f(&mut next);
        let raw = serde_json::to_value(&next).unwrap_or_default();
        let merged = recur_editor::prefs::EditorPrefs::normalize(&raw);
        if let Ok(s) = serde_json::to_string(&merged) {
            self.storage.set(prefs::EDITOR_PREFS_KEY, s);
        }
        self.editor_prefs = merged;
        cx.notify();
    }

    /// `setEditorIndentForLang`: set or clear (`None`) a language's indent override.
    pub fn set_editor_indent_for_lang(
        &mut self,
        lang: &str,
        indent: Option<recur_editor::prefs::EditorIndent>,
        cx: &mut Context<Self>,
    ) {
        let lang = lang.to_string();
        self.set_editor_prefs(
            move |p| match indent {
                Some(i) => {
                    p.indent_by_lang.insert(lang, i);
                }
                None => {
                    p.indent_by_lang.remove(&lang);
                }
            },
            cx,
        );
    }

    /// `explorerPrefs.exclude`.
    pub fn explorer_exclude(&self) -> Vec<String> {
        prefs::explorer_exclude(&self.storage)
    }

    /// `setExplorerExclude`.
    pub fn set_explorer_exclude(&mut self, patterns: Vec<String>, cx: &mut Context<Self>) {
        prefs::save_explorer_exclude(&mut self.storage, &patterns);
        self.sync_file_index_exclude(cx);
        cx.notify();
    }

    /// `syncFileIndexExclude`: push `files.exclude` into the backend so the path index (Quick
    /// Open, `@` files) matches the explorer. Non-fatal: search still filters `.git` and
    /// `node_modules`.
    pub fn sync_file_index_exclude(&self, cx: &mut Context<Self>) {
        let patterns = self.explorer_exclude();
        let services = self.services.clone();
        gpui_kit::AppContext::background_spawn(cx, async move {
            if let Err(err) = services.code.set_file_index_exclude(&patterns).await {
                tracing::warn!(
                    "SetFileIndexExclude failed: {}",
                    crate::state::bridge_error_message(&err)
                );
            }
        })
        .detach();
    }

    /// `resetExplorerExclude`.
    pub fn reset_explorer_exclude(&mut self, cx: &mut Context<Self>) {
        let d: Vec<String> = prefs::DEFAULT_EXPLORER_EXCLUDE
            .iter()
            .map(|s| s.to_string())
            .collect();
        self.set_explorer_exclude(d, cx);
    }

    // ---- catalogs ----

    /// Load the bundle editor and terminal theme catalogs (`ensureTextMateBootstrap` and
    /// `ensureTerminalThemes` metadata). `force` re-reads after Reload Bundles.
    pub fn load_theme_catalogs(&mut self, cx: &mut Context<Self>) -> gpui_kit::Task<()> {
        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            let editor = services.bundle.list_themes().await.unwrap_or_default();
            let terminal = services
                .bundle
                .list_terminal_themes()
                .await
                .unwrap_or_default();
            let _ = this.update(cx, |s, cx| {
                s.editor_theme_catalog = editor;
                s.terminal_theme_catalog = terminal;
                // A saved key that was only plausible before the catalog loaded resolves now.
                let current = s.editor_theme.clone();
                s.apply_editor_theme(&current);
                cx.notify();
            });
        })
    }

    /// `toastEditorTheme`.
    pub fn toast_editor_theme(name: &str, cx: &mut App) {
        let store = ToastStore::global(cx);
        store.update(cx, |s, cx| s.editor_theme(name, cx));
    }

    /// The Appearance submenu titles for the app chrome themes.
    pub fn app_theme_menu(&self) -> Vec<(SharedString, SharedString)> {
        ThemeId::ALL
            .iter()
            .map(|t| (t.key().into(), t.name().into()))
            .collect()
    }
}
