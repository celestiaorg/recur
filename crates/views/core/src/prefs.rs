//! The persisted UI prefs: panel sizes, Observatory panel sides, the
//! rail toggle, the app / editor / terminal theme picks with their system sync, terminal font
//! prefs, and the per-workspace terminal dock chrome.

use std::collections::{BTreeMap, HashSet};

use recur_services::api::TerminalDockChrome;
use recur_ui::theme::{Appearance, SystemSync, ThemeId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::local_storage::LocalStorage;

// ---- panel sizes ----

/// A resizable panel. Its size is a width, except `TerminalDock`, a height.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PanelKey {
    Rail,
    LoopInspector,
    ObsTree,
    ObsSide,
    PrFileList,
    MaestroDock,
    PrChatDock,
    TerminalDock,
    InboxChat,
}

impl PanelKey {
    pub const ALL: [PanelKey; 9] = [
        PanelKey::Rail,
        PanelKey::LoopInspector,
        PanelKey::ObsTree,
        PanelKey::ObsSide,
        PanelKey::PrFileList,
        PanelKey::MaestroDock,
        PanelKey::PrChatDock,
        PanelKey::TerminalDock,
        PanelKey::InboxChat,
    ];

    pub fn key(self) -> &'static str {
        match self {
            PanelKey::Rail => "rail",
            PanelKey::LoopInspector => "loopInspector",
            PanelKey::ObsTree => "obsTree",
            PanelKey::ObsSide => "obsSide",
            PanelKey::PrFileList => "prFileList",
            PanelKey::MaestroDock => "maestroDock",
            PanelKey::PrChatDock => "prChatDock",
            PanelKey::TerminalDock => "terminalDock",
            PanelKey::InboxChat => "inboxChat",
        }
    }

    /// `PANEL_DEFAULTS`.
    pub fn default_size(self) -> f32 {
        match self {
            PanelKey::Rail => 272.,
            PanelKey::LoopInspector => 312.,
            PanelKey::ObsTree => 220.,
            PanelKey::ObsSide => 260.,
            PanelKey::PrFileList => 250.,
            PanelKey::MaestroDock => 600.,
            PanelKey::PrChatDock => 360.,
            PanelKey::TerminalDock => 260.,
            PanelKey::InboxChat => 360.,
        }
    }

    /// `PANEL_BOUNDS`: a generous safety range; the Observatory docks are capped more precisely
    /// at drag time.
    pub fn bounds(self) -> (f32, f32) {
        match self {
            PanelKey::Rail => (180., 600.),
            PanelKey::LoopInspector => (220., 720.),
            PanelKey::ObsTree => (140., 2000.),
            PanelKey::ObsSide => (160., 2000.),
            PanelKey::PrFileList => (160., 900.),
            PanelKey::MaestroDock => (260., 2000.),
            PanelKey::PrChatDock => (260., 2000.),
            PanelKey::TerminalDock => (80., 2000.),
            PanelKey::InboxChat => (280., 2000.),
        }
    }
}

/// An Observatory tool panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CodePanelKey {
    Maestro,
    Plan,
    Findings,
    Files,
    Scm,
    Outline,
    Problems,
    Search,
    Refs,
    Artifacts,
}

impl CodePanelKey {
    pub const ALL: [CodePanelKey; 10] = [
        CodePanelKey::Maestro,
        CodePanelKey::Plan,
        CodePanelKey::Findings,
        CodePanelKey::Files,
        CodePanelKey::Scm,
        CodePanelKey::Outline,
        CodePanelKey::Problems,
        CodePanelKey::Search,
        CodePanelKey::Refs,
        CodePanelKey::Artifacts,
    ];

    pub fn key(self) -> &'static str {
        match self {
            CodePanelKey::Maestro => "maestro",
            CodePanelKey::Plan => "plan",
            CodePanelKey::Findings => "findings",
            CodePanelKey::Files => "files",
            CodePanelKey::Scm => "scm",
            CodePanelKey::Outline => "outline",
            CodePanelKey::Problems => "problems",
            CodePanelKey::Search => "search",
            CodePanelKey::Refs => "refs",
            CodePanelKey::Artifacts => "artifacts",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CodePanelKey::Maestro => "Maestro",
            CodePanelKey::Plan => "Plan",
            CodePanelKey::Findings => "Findings",
            CodePanelKey::Files => "Files",
            CodePanelKey::Scm => "Source Control",
            CodePanelKey::Outline => "Outline",
            CodePanelKey::Problems => "Problems",
            CodePanelKey::Search => "Search",
            CodePanelKey::Refs => "Refs",
            CodePanelKey::Artifacts => "Artifacts",
        }
    }

    /// `CODE_PANEL_SIDE_DEFAULTS`.
    pub fn default_side(self) -> PanelSide {
        match self {
            CodePanelKey::Maestro | CodePanelKey::Plan | CodePanelKey::Findings => PanelSide::Left,
            _ => PanelSide::Right,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelSide {
    Left,
    Right,
}

impl PanelSide {
    fn parse(v: &Value) -> Option<PanelSide> {
        match v.as_str()? {
            "left" => Some(PanelSide::Left),
            "right" => Some(PanelSide::Right),
            _ => None,
        }
    }

    fn key(self) -> &'static str {
        match self {
            PanelSide::Left => "left",
            PanelSide::Right => "right",
        }
    }
}

const PANEL_STORAGE_KEY: &str = "recur.panelWidths";
const CODE_PANEL_SIDES_KEY: &str = "recur.codePanelSides";
/// Prior factory defaults for the left dock: a user still on one adopts the new default.
const MAESTRO_DOCK_LEGACY_DEFAULTS: [f64; 2] = [400., 480.];

// `.obs-tb-left/right`: 8px pad, 28px hide toggle, 2px gap, then the icon group (2px margin,
// 4px pad, 1px rule) of 28px pane toggles with 1px gaps.
const OBS_TB_TOGGLE: f32 = 28.;
const OBS_TB_PAD: f32 = 16.;
const OBS_TB_GAP: f32 = 2.;
const OBS_TB_ICONS_CHROME: f32 = 7.;
const OBS_TB_SIDE_RULE: f32 = 1.;

/// The narrowest an Observatory dock may get and still show its header icons.
pub fn obs_toolbar_min_width(panel_count: usize) -> f32 {
    let n = panel_count as f32;
    let icons = if panel_count == 0 {
        0.
    } else {
        OBS_TB_ICONS_CHROME + n * OBS_TB_TOGGLE + (n - 1.).max(0.)
    };
    OBS_TB_PAD + OBS_TB_TOGGLE + OBS_TB_GAP + icons + OBS_TB_SIDE_RULE
}

/// Panel sizes plus the Observatory panel sides their floors depend on.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelLayout {
    pub sizes: BTreeMap<PanelKey, f32>,
    pub sides: BTreeMap<CodePanelKey, PanelSide>,
}

impl Default for PanelLayout {
    fn default() -> Self {
        Self {
            sizes: PanelKey::ALL
                .iter()
                .map(|k| (*k, k.default_size()))
                .collect(),
            sides: CodePanelKey::ALL
                .iter()
                .map(|k| (*k, k.default_side()))
                .collect(),
        }
    }
}

impl PanelLayout {
    pub fn size(&self, key: PanelKey) -> f32 {
        self.sizes
            .get(&key)
            .copied()
            .unwrap_or_else(|| key.default_size())
    }

    pub fn panels_on_side(&self, side: PanelSide) -> Vec<CodePanelKey> {
        CodePanelKey::ALL
            .into_iter()
            .filter(|k| self.sides.get(k) == Some(&side))
            .collect()
    }

    fn min(&self, key: PanelKey) -> f32 {
        let base = key.bounds().0;
        let side = match key {
            PanelKey::MaestroDock => PanelSide::Left,
            PanelKey::ObsSide => PanelSide::Right,
            _ => return base,
        };
        base.max(obs_toolbar_min_width(self.panels_on_side(side).len()))
    }

    /// `clampPanel`: rounds to whole px within the floor and the safety ceiling.
    pub fn clamp(&self, key: PanelKey, size: f32) -> f32 {
        size.max(self.min(key)).min(key.bounds().1).round()
    }

    /// `setPanelWidthLive`: move a panel edge without persisting.
    pub fn set_live(&mut self, key: PanelKey, size: f32) {
        let v = self.clamp(key, size);
        self.sizes.insert(key, v);
    }

    fn reclamp_observatory_docks(&mut self) {
        for k in [PanelKey::MaestroDock, PanelKey::ObsSide] {
            let v = self.size(k);
            self.set_live(k, v);
        }
    }

    /// `setCodePanelSide`.
    pub fn set_side(&mut self, key: CodePanelKey, side: PanelSide, storage: &mut LocalStorage) {
        self.sides.insert(key, side);
        storage.set(CODE_PANEL_SIDES_KEY, self.sides_json());
        self.reclamp_observatory_docks();
        self.persist(storage);
    }

    fn sides_json(&self) -> String {
        let body: Vec<String> = CodePanelKey::ALL
            .iter()
            .map(|k| format!("\"{}\":\"{}\"", k.key(), self.sides[k].key()))
            .collect();
        format!("{{{}}}", body.join(","))
    }

    /// `persistPanelWidths`: call once a drag settles.
    pub fn persist(&self, storage: &mut LocalStorage) {
        let body: Vec<String> = PanelKey::ALL
            .iter()
            .map(|k| format!("\"{}\":{}", k.key(), self.size(*k)))
            .collect();
        storage.set(PANEL_STORAGE_KEY, format!("{{{}}}", body.join(",")));
    }

    /// Panel widths, then the Observatory panel sides.
    pub fn load(storage: &LocalStorage) -> Self {
        let mut layout = PanelLayout::default();
        let saved: BTreeMap<String, Value> = storage
            .get(PANEL_STORAGE_KEY)
            .and_then(|raw| serde_json::from_str(raw).ok())
            .unwrap_or_default();
        for key in PanelKey::ALL {
            let Some(v) = saved.get(key.key()).and_then(Value::as_f64) else {
                continue;
            };
            if !v.is_finite()
                || (key == PanelKey::MaestroDock && MAESTRO_DOCK_LEGACY_DEFAULTS.contains(&v))
            {
                continue;
            }
            let c = layout.clamp(key, v as f32);
            layout.sizes.insert(key, c);
        }
        let sides: BTreeMap<String, Value> = storage
            .get(CODE_PANEL_SIDES_KEY)
            .and_then(|raw| serde_json::from_str(raw).ok())
            .unwrap_or_default();
        for key in CodePanelKey::ALL {
            if let Some(side) = sides.get(key.key()).and_then(PanelSide::parse) {
                layout.sides.insert(key, side);
            }
        }
        // overview → plan (the Activity/Overview panels were removed).
        if sides.get("plan").and_then(PanelSide::parse).is_none()
            && let Some(side) = sides.get("overview").and_then(PanelSide::parse)
        {
            layout.sides.insert(CodePanelKey::Plan, side);
        }
        layout.reclamp_observatory_docks();
        layout
    }
}

// ---- rail ----

const RAIL_COLLAPSED_KEY: &str = "recur.railCollapsed";

pub fn load_rail_collapsed(storage: &LocalStorage) -> bool {
    storage.get(RAIL_COLLAPSED_KEY) == Some("1")
}

pub fn save_rail_collapsed(storage: &mut LocalStorage, collapsed: bool) {
    storage.set(RAIL_COLLAPSED_KEY, if collapsed { "1" } else { "0" });
}

// ---- themes ----

const THEME_KEY: &str = "recur.theme";
const EDITOR_THEME_KEY: &str = "recur.editorTheme";
const SYSTEM_DARK_KEY: &str = "recur.systemDark";
const TERMINAL_PREFS_KEY: &str = "recur.terminalPrefs";

/// One `createSystemSync` (`recur.<name>Auto|Light|Dark`).
#[derive(Clone, Copy)]
pub enum SyncName {
    Theme,
    EditorTheme,
    TerminalTheme,
}

impl SyncName {
    fn prefix(self) -> &'static str {
        match self {
            SyncName::Theme => "recur.theme",
            SyncName::EditorTheme => "recur.editorTheme",
            SyncName::TerminalTheme => "recur.terminalTheme",
        }
    }

    pub fn defaults(self) -> SystemSync {
        let (light, dark) = match self {
            SyncName::Theme => (ThemeId::Paper.key(), ThemeId::Charcoal.key()),
            SyncName::EditorTheme => ("paper", "recur"),
            SyncName::TerminalTheme => (TERMINAL_MATCH_EDITOR, TERMINAL_MATCH_EDITOR),
        };
        SystemSync {
            auto: false,
            light: light.into(),
            dark: dark.into(),
        }
    }

    /// `sync.load()`.
    pub fn load(self, storage: &LocalStorage) -> SystemSync {
        let d = self.defaults();
        let key = |suffix: &str| format!("{}{suffix}", self.prefix());
        SystemSync {
            light: storage
                .get(&key("Light"))
                .map(str::to_string)
                .unwrap_or(d.light),
            dark: storage
                .get(&key("Dark"))
                .map(str::to_string)
                .unwrap_or(d.dark),
            auto: storage.get(&key("Auto")) == Some("1"),
        }
    }

    pub fn save_auto(self, storage: &mut LocalStorage, on: bool) {
        storage.set(
            &format!("{}Auto", self.prefix()),
            if on { "1" } else { "0" },
        );
    }

    pub fn save_pick(self, storage: &mut LocalStorage, dark: bool, value: &str) {
        let suffix = if dark { "Dark" } else { "Light" };
        storage.set(&format!("{}{suffix}", self.prefix()), value);
    }
}

/// `initTheme`: the saved chrome theme and its sync.
pub fn load_appearance(storage: &LocalStorage) -> Appearance {
    let saved = storage.get(THEME_KEY).unwrap_or("default");
    Appearance {
        theme: ThemeId::from_key(saved).key().into(),
        sync: SyncName::Theme.load(storage),
    }
}

pub fn save_theme(storage: &mut LocalStorage, key: &str) {
    storage.set(THEME_KEY, key);
}

pub fn load_editor_theme(storage: &LocalStorage) -> String {
    storage.get(EDITOR_THEME_KEY).unwrap_or("recur").to_string()
}

pub fn save_editor_theme(storage: &mut LocalStorage, key: &str) {
    storage.set(EDITOR_THEME_KEY, key);
}

/// The last native dark-mode answer, so startup paints the right pick before the platform
/// replies.
pub fn load_system_dark(storage: &LocalStorage) -> Option<bool> {
    storage.get(SYSTEM_DARK_KEY).map(|v| v == "1")
}

pub fn save_system_dark(storage: &mut LocalStorage, dark: bool) {
    storage.set(SYSTEM_DARK_KEY, if dark { "1" } else { "0" });
}

/// `''` follows the editor's font / the app chrome's palette.
pub const TERMINAL_MATCH_EDITOR: &str = "";
pub const TERMINAL_LINE_HEIGHTS: [f64; 3] = [1., 1.15, 1.35];
pub const EDITOR_FONT_SIZES: [u8; 6] = [10, 11, 12, 13, 14, 16];

/// Terminal font prefs. `font_face` '' and `font_size` 0 mean "match the editor".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalPrefs {
    pub font_face: String,
    pub font_size: u8,
    pub line_height: f64,
    /// Bundle `[[terminalTheme]]` id, or '' to follow the app chrome.
    pub theme: String,
    /// The sidebar's Terminals page (shells outside any workspace).
    pub page: bool,
}

impl Default for TerminalPrefs {
    fn default() -> Self {
        Self {
            font_face: TERMINAL_MATCH_EDITOR.into(),
            font_size: 0,
            line_height: 1.15,
            theme: TERMINAL_MATCH_EDITOR.into(),
            page: false,
        }
    }
}

impl TerminalPrefs {
    /// `normalizeTerminalPrefs` over a raw JSON object.
    pub fn normalize(raw: Option<&Value>) -> TerminalPrefs {
        let d = TerminalPrefs::default();
        let get = |k: &str| raw.and_then(|r| r.get(k));
        let line_height = get("lineHeight")
            .and_then(Value::as_f64)
            .filter(|v| TERMINAL_LINE_HEIGHTS.contains(v))
            .unwrap_or(d.line_height);
        let font_size = get("fontSize")
            .and_then(Value::as_f64)
            .filter(|v| *v == 0. || EDITOR_FONT_SIZES.iter().any(|s| f64::from(*s) == *v))
            .map(|v| v as u8)
            .unwrap_or(d.font_size);
        let string = |k: &str, fallback: String| {
            get(k)
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or(fallback)
        };
        TerminalPrefs {
            font_face: string("fontFace", d.font_face),
            font_size,
            line_height,
            theme: string("theme", d.theme),
            page: get("page").and_then(Value::as_bool).unwrap_or(d.page),
        }
    }

    pub fn load(storage: &LocalStorage) -> TerminalPrefs {
        let raw: Option<Value> = storage
            .get(TERMINAL_PREFS_KEY)
            .and_then(|s| serde_json::from_str(s).ok());
        TerminalPrefs::normalize(raw.as_ref())
    }

    pub fn save(&self, storage: &mut LocalStorage) {
        if let Ok(s) = serde_json::to_string(self) {
            storage.set(TERMINAL_PREFS_KEY, s);
        }
    }
}

// ---- editor and explorer prefs ----

/// `recur.editorPrefs`.
pub const EDITOR_PREFS_KEY: &str = "recur.editorPrefs";

/// `initEditorPrefs`: the saved prefs, normalized (defaults when missing or malformed).
pub fn load_editor_prefs(storage: &LocalStorage) -> recur_editor::prefs::EditorPrefs {
    storage
        .get(EDITOR_PREFS_KEY)
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .map(|v| recur_editor::prefs::EditorPrefs::normalize(&v))
        .unwrap_or_default()
}

/// `recur.explorerPrefs`.
pub const EXPLORER_PREFS_KEY: &str = "recur.explorerPrefs";

/// `DEFAULT_EXPLORER_EXCLUDE`: VCS dirs and OS junk.
pub const DEFAULT_EXPLORER_EXCLUDE: [&str; 5] = [
    "**/.git",
    "**/.svn",
    "**/.hg",
    "**/.DS_Store",
    "**/Thumbs.db",
];

/// `explorerPrefs.exclude` (`normalizeExplorerPrefs`): the saved globs (blank rows kept so
/// Add pattern stays editable), else the defaults.
pub fn explorer_exclude(storage: &LocalStorage) -> Vec<String> {
    let saved: Option<Value> = storage
        .get(EXPLORER_PREFS_KEY)
        .and_then(|raw| serde_json::from_str(raw).ok());
    match saved
        .as_ref()
        .and_then(|v| v.get("exclude"))
        .and_then(Value::as_array)
    {
        Some(list) => list
            .iter()
            .map(|p| match p {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect(),
        None => DEFAULT_EXPLORER_EXCLUDE
            .iter()
            .map(|s| s.to_string())
            .collect(),
    }
}

/// `setExplorerExclude`: `{"exclude":[…]}`.
pub fn save_explorer_exclude(storage: &mut LocalStorage, patterns: &[String]) {
    let v = serde_json::json!({ "exclude": patterns });
    storage.set(EXPLORER_PREFS_KEY, v.to_string());
}

// ---- terminal dock chrome ----

/// Per-workspace terminal dock layout (`terminalDockOpenBy` and friends). The dock costs a
/// real PTY per tab, so it is persisted per workspace on `terminals.json.gz` through
/// `SetTerminalDockChrome`, not in local storage.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DockChrome {
    pub open: HashSet<String>,
    pub maximized: HashSet<String>,
    pub editor_hidden: HashSet<String>,
    pub beside: HashSet<String>,
    pub beside_pct: BTreeMap<String, i64>,
    /// Workspaces whose chrome the user already toggled this session.
    pub touched: HashSet<String>,
}

fn flag(set: &mut HashSet<String>, id: &str, on: bool) {
    if on {
        set.insert(id.to_string());
    } else {
        set.remove(id);
    }
}

fn clamp_beside_pct(n: f64) -> i64 {
    if !n.is_finite() || n <= 0. {
        return 0;
    }
    n.clamp(20., 80.).round() as i64
}

impl DockChrome {
    pub fn is_open(&self, id: &str) -> bool {
        !id.is_empty() && self.open.contains(id)
    }

    pub fn is_maximized(&self, id: &str) -> bool {
        !id.is_empty() && self.maximized.contains(id)
    }

    pub fn is_editor_hidden(&self, id: &str) -> bool {
        !id.is_empty() && self.editor_hidden.contains(id)
    }

    pub fn is_beside(&self, id: &str) -> bool {
        !id.is_empty() && self.beside.contains(id)
    }

    /// `terminalBesidePct`: the saved split, or 50.
    pub fn beside_pct(&self, id: &str) -> i64 {
        match self.beside_pct.get(id) {
            Some(n) if (20..=80).contains(n) => *n,
            _ => 50,
        }
    }

    /// The row `SetTerminalDockChrome` persists for `id`; marks it touched.
    #[must_use]
    pub fn snapshot(&mut self, id: &str) -> TerminalDockChrome {
        self.touched.insert(id.to_string());
        let open = self.open.contains(id);
        TerminalDockChrome {
            workspace_id: String::new(),
            open,
            maximized: open && self.maximized.contains(id),
            editor_hidden: open && self.editor_hidden.contains(id),
            beside: self.beside.contains(id),
            beside_pct: clamp_beside_pct(self.beside_pct.get(id).copied().unwrap_or(0) as f64),
        }
    }

    /// `setTerminalDockOpen`. Hiding drops maximize and restores the editor (an empty middle
    /// panel is never useful).
    pub fn set_open(&mut self, id: &str, on: bool) {
        flag(&mut self.open, id, on);
        if !on {
            flag(&mut self.maximized, id, false);
            flag(&mut self.editor_hidden, id, false);
        }
    }

    /// `setTerminalDockMaximized`: maximizing implies showing.
    pub fn set_maximized(&mut self, id: &str, on: bool) {
        if on {
            self.set_open(id, true);
        }
        flag(&mut self.maximized, id, on);
    }

    /// `setEditorHidden`: hiding the editor opens the dock so the terminal can take the space.
    pub fn set_editor_hidden(&mut self, id: &str, on: bool) {
        if on {
            self.set_open(id, true);
        }
        flag(&mut self.editor_hidden, id, on);
    }

    pub fn set_beside(&mut self, id: &str, on: bool) {
        flag(&mut self.beside, id, on);
        if on {
            self.set_open(id, true);
        }
    }

    pub fn set_beside_pct(&mut self, id: &str, n: f64) -> bool {
        let pct = clamp_beside_pct(n);
        if self.beside_pct.get(id) == Some(&pct) {
            return false;
        }
        self.beside_pct.insert(id.to_string(), pct);
        true
    }

    /// `applyTerminalDockChrome`: merge the backend rows without reverting a toggle that
    /// landed while they loaded.
    pub fn apply(&mut self, rows: &[TerminalDockChrome]) {
        for r in rows {
            let id = r.workspace_id.trim();
            if id.is_empty() || self.touched.contains(id) {
                continue;
            }
            if r.open {
                self.open.insert(id.into());
                if r.maximized {
                    self.maximized.insert(id.into());
                }
                if r.editor_hidden {
                    self.editor_hidden.insert(id.into());
                }
            }
            if r.beside {
                self.beside.insert(id.into());
            }
            let n = clamp_beside_pct(r.beside_pct as f64);
            if n != 0 {
                self.beside_pct.insert(id.into(), n);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolbar_min_width() {
        assert_eq!(obs_toolbar_min_width(0), 47.);
        assert_eq!(obs_toolbar_min_width(3), 140.);
        assert_eq!(obs_toolbar_min_width(9), 314.);
    }

    #[test]
    fn left_dock_floor_follows_its_icons() {
        let mut storage = LocalStorage::in_memory();
        let mut l = PanelLayout::default();
        for k in CodePanelKey::ALL {
            l.set_side(k, PanelSide::Left, &mut storage);
        }
        l.set_live(PanelKey::MaestroDock, 80.);
        assert_eq!(
            l.size(PanelKey::MaestroDock),
            obs_toolbar_min_width(CodePanelKey::ALL.len())
        );
    }

    #[test]
    fn saved_narrow_right_dock_is_floored_after_sides_load() {
        let mut storage = LocalStorage::in_memory();
        storage.set(PANEL_STORAGE_KEY, r#"{"obsSide":80}"#);
        let sides: Vec<String> = CodePanelKey::ALL
            .iter()
            .map(|k| format!("\"{}\":\"right\"", k.key()))
            .collect();
        storage.set(CODE_PANEL_SIDES_KEY, format!("{{{}}}", sides.join(",")));
        let l = PanelLayout::load(&storage);
        assert_eq!(
            l.size(PanelKey::ObsSide),
            obs_toolbar_min_width(CodePanelKey::ALL.len())
        );
    }

    #[test]
    fn panel_widths_clamp_round_and_persist() {
        let mut storage = LocalStorage::in_memory();
        let mut l = PanelLayout::default();
        l.set_live(PanelKey::Rail, 99.);
        assert_eq!(l.size(PanelKey::Rail), 180.);
        l.set_live(PanelKey::Rail, 300.6);
        assert_eq!(l.size(PanelKey::Rail), 301.);
        l.persist(&mut storage);
        assert_eq!(PanelLayout::load(&storage).size(PanelKey::Rail), 301.);
        storage.set(PANEL_STORAGE_KEY, r#"{"maestroDock":480,"rail":9000}"#);
        let l = PanelLayout::load(&storage);
        assert_eq!(l.size(PanelKey::MaestroDock), 600.);
        assert_eq!(l.size(PanelKey::Rail), 600.);
    }

    #[test]
    fn overview_side_migrates_to_plan() {
        let mut storage = LocalStorage::in_memory();
        storage.set(CODE_PANEL_SIDES_KEY, r#"{"overview":"right"}"#);
        assert_eq!(
            PanelLayout::load(&storage).sides[&CodePanelKey::Plan],
            PanelSide::Right
        );
    }

    #[test]
    fn terminal_prefs_normalize() {
        let p = TerminalPrefs::normalize(Some(
            &serde_json::json!({"fontSize": 15, "lineHeight": 1.35, "theme": "dracula", "fontFace": 3, "page": true}),
        ));
        assert!(p.page);
        assert_eq!(p.font_size, 0);
        assert_eq!(p.line_height, 1.35);
        assert_eq!(p.theme, "dracula");
        assert_eq!(p.font_face, "");
        assert_eq!(TerminalPrefs::normalize(None), TerminalPrefs::default());
        let mut storage = LocalStorage::in_memory();
        p.save(&mut storage);
        assert_eq!(TerminalPrefs::load(&storage), p);
    }

    #[test]
    fn sync_load_defaults_and_saved() {
        let mut storage = LocalStorage::in_memory();
        let s = SyncName::EditorTheme.load(&storage);
        assert_eq!(
            (s.auto, s.light.as_str(), s.dark.as_str()),
            (false, "paper", "recur")
        );
        SyncName::EditorTheme.save_auto(&mut storage, true);
        SyncName::EditorTheme.save_pick(&mut storage, true, "nightowl");
        let s = SyncName::EditorTheme.load(&storage);
        assert_eq!((s.auto, s.dark.as_str()), (true, "nightowl"));
        storage.set(THEME_KEY, "tokyonight");
        assert_eq!(load_appearance(&storage).theme, "coolglow");
    }

    fn row(id: &str, f: impl FnOnce(&mut TerminalDockChrome)) -> TerminalDockChrome {
        let mut r = TerminalDockChrome {
            workspace_id: id.into(),
            ..TerminalDockChrome::default()
        };
        f(&mut r);
        r
    }

    #[test]
    fn dock_restores_maximize() {
        let mut d = DockChrome::default();
        d.apply(&[row("ws-1", |r| {
            r.open = true;
            r.maximized = true;
        })]);
        assert!(d.is_open("ws-1") && d.is_maximized("ws-1"));
    }

    #[test]
    fn dock_maximize_persists_open() {
        let mut d = DockChrome::default();
        d.set_maximized("ws-1", true);
        let s = d.snapshot("ws-1");
        assert!(s.open && s.maximized);
        d.set_open("ws-1", false);
        assert!(!d.is_maximized("ws-1"));
        let s = d.snapshot("ws-1");
        assert!(!s.open && !s.maximized);
    }

    #[test]
    fn dock_boot_merge_keeps_user_toggle() {
        let mut d = DockChrome::default();
        d.set_open("ws-1", false);
        let _ = d.snapshot("ws-1");
        d.apply(&[row("ws-1", |r| {
            r.open = true;
            r.maximized = true;
        })]);
        assert!(!d.is_open("ws-1") && !d.is_maximized("ws-1"));
    }

    #[test]
    fn dock_beside_restores_while_closed() {
        let mut d = DockChrome::default();
        d.apply(&[row("ws-1", |r| {
            r.beside = true;
            r.beside_pct = 40;
        })]);
        assert!(d.is_beside("ws-1") && !d.is_open("ws-1"));
        assert_eq!(d.beside_pct("ws-1"), 40);
        let mut d = DockChrome::default();
        d.apply(&[row("ws-1", |r| r.beside_pct = 40)]);
        assert_eq!(d.beside_pct("ws-1"), 40);
        assert!(!d.is_beside("ws-1"));
        let mut d = DockChrome::default();
        d.set_beside("ws-1", true);
        assert!(d.is_open("ws-1"));
        let s = d.snapshot("ws-1");
        assert!(s.open && s.beside);
    }
}
