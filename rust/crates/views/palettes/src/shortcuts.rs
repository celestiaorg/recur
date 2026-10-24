//! `ShortcutsHelp.svelte` (⌘⇧/) over the static cheatsheet of `lib/shortcuts.ts`, plus every
//! bound bundle command under its category.

use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, Focusable, InteractiveElement, IntoElement,
    KeystrokeEvent, ParentElement, Render, ScrollHandle, SharedString, Styled, Subscription,
    Window, div, px,
};
use recur_editor::text::locale_compare;
use recur_ui::style::{palette_empty, palette_group, palette_key, style_text_input};
use recur_ui::text::{TextStyled, text};
use recur_ui::{IconName, Theme};
use recur_view_core::AppState;

use crate::chrome::{self, Kind};

/// One cheatsheet row. Display glyphs match `Kbd.svelte`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortcutEntry {
    pub group: String,
    pub label: String,
    pub keys: String,
    /// Extra search terms beyond label and group.
    pub terms: Vec<String>,
}

pub const SHORTCUT_GROUPS: [&str; 6] = [
    "General",
    "Navigation",
    "Workbench",
    "Editor",
    "Explorer",
    "Terminal",
];

/// `SHORTCUTS`: (group, label, keys, terms).
pub const SHORTCUTS: &[(&str, &str, &str, &[&str])] = &[
    ("General", "Keyboard shortcuts", "⌘⇧/", &[]),
    (
        "General",
        "Go to (workspaces / PRs)",
        "⌘K",
        &["command palette", "search"],
    ),
    (
        "General",
        "Run command / snippets",
        "⌘⇧P",
        &["bundles", "command palette", "snippets", "tab trigger"],
    ),
    ("General", "New workspace", "⌘N", &[]),
    (
        "General",
        "New blank workspace",
        "⇧⌘N",
        &["quick create", "empty"],
    ),
    (
        "General",
        "Open file or folder",
        "⌥⌘O",
        &["open", "folder", "file"],
    ),
    (
        "General",
        "Open in…",
        "⌘O",
        &["external editor", "finder", "terminal"],
    ),
    ("General", "Toggle sidebar", "⌘B", &[]),
    ("General", "Settings", "⌘,", &[]),
    ("General", "Back", "esc", &["escape"]),
    ("Navigation", "Quick Open", "⌘P", &[]),
    ("Navigation", "Go to symbol", "⌘⇧O", &[]),
    ("Navigation", "Focus agent panel", "⌘1", &[]),
    ("Navigation", "Focus editor", "⌘2", &[]),
    ("Navigation", "Focus terminal", "⌘3", &[]),
    (
        "Navigation",
        "Add file / selection to chat",
        "⌘L",
        &["@ file", "mention", "reference"],
    ),
    ("Navigation", "Previous tab", "⌥⌘←", &[]),
    ("Navigation", "Next tab", "⌥⌘→", &[]),
    ("Navigation", "Previous workspace", "⌥⌘↑", &["cycle"]),
    ("Navigation", "Next workspace", "⌥⌘↓", &["cycle"]),
    ("Workbench", "Toggle left panel", "⌥⌘B", &[]),
    ("Workbench", "Toggle right panel", "⌥⌘⇧B", &[]),
    (
        "Workbench",
        "Toggle editor",
        "⌥⌘E",
        &["hide editor", "show editor"],
    ),
    (
        "Workbench",
        "Toggle review",
        "⌥⌘R",
        &["files changed", "commits", "diff"],
    ),
    (
        "Workbench",
        "Review: next / previous file",
        "J / K",
        &["diff", "review"],
    ),
    (
        "Workbench",
        "Open diff line in editor",
        "⌘Click",
        &["review", "diff", "jump"],
    ),
    ("Workbench", "Toggle terminal", "⌃`", &[]),
    ("Workbench", "Maximize terminal", "⌃⇧`", &[]),
    ("Workbench", "New tab", "⌘T", &["terminal", "agent"]),
    ("Workbench", "Close tab", "⌘W", &[]),
    ("Workbench", "Reopen closed editor tab", "⌘⇧T", &[]),
    ("Workbench", "Stop agent", "⌘⇧⌫", &["cancel", "interrupt"]),
    (
        "Workbench",
        "Cycle agent mode",
        "⌘.",
        &["chat mode", "ask", "plan", "review", "loop"],
    ),
    ("Workbench", "Split editor", "⌘\\", &[]),
    (
        "Workbench",
        "Run…",
        "⌘R",
        &["dev server", "run command", "run menu"],
    ),
    ("Workbench", "Stop run", "⌘⇧R", &["dev server", "interrupt"]),
    ("Workbench", "Reload from disk", "F5", &[]),
    (
        "Editor",
        "Cycle view",
        "⌘⇧V",
        &["code", "diff", "preview", "toggle"],
    ),
    (
        "Editor",
        "Previous / next editor theme",
        "⌥⌘[ ⌥⌘]",
        &["cycle", "color scheme"],
    ),
    ("Editor", "Save", "⌘S", &[]),
    ("Editor", "Find", "⌘F", &[]),
    (
        "Editor",
        "Find next / previous",
        "⌘G ⇧⌘G",
        &["Enter", "F3", "shift Enter", "shift F3"],
    ),
    ("Editor", "Go to line", "⌥⌘G", &[]),
    (
        "Editor",
        "Revert diff hunk",
        "⌥⌘Z",
        &["diff", "reject", "undo hunk"],
    ),
    ("Editor", "Match case (find)", "⌥⌘C", &["search panel"]),
    ("Editor", "Regexp (find)", "⌥⌘R", &["search panel", "regex"]),
    ("Editor", "Whole word (find)", "⌥⌘W", &["search panel"]),
    ("Editor", "Select next occurrence", "⌘D", &[]),
    (
        "Editor",
        "Add cursor above / below",
        "⌃⇧↑ ⌃⇧↓",
        &["multi cursor"],
    ),
    ("Editor", "Toggle comment", "⌘/", &[]),
    ("Editor", "Go to definition", "F12", &["⌘-click"]),
    ("Explorer", "Open file", "⌘↓", &[]),
    ("Explorer", "New file", "⌥⌘N", &[]),
    ("Explorer", "New folder", "⇧⌘N", &[]),
    ("Explorer", "Rename", "↩", &["enter", "F2"]),
    ("Explorer", "Delete", "⌘⌫", &[]),
    ("Explorer", "Cut / Copy / Paste", "⌘X ⌘C ⌘V", &[]),
    ("Explorer", "Duplicate", "⌘D", &[]),
    ("Explorer", "Copy path", "⌥⌘C", &[]),
    ("Explorer", "Copy relative path", "⇧⌥⌘C", &[]),
    ("Explorer", "Reveal in Finder", "⌥⌘R", &[]),
    ("Terminal", "New terminal tab", "⌘T", &[]),
    ("Terminal", "Close terminal tab", "⌘W", &[]),
    ("Terminal", "Previous / next tab", "⌥⌘← ⌥⌘→", &[]),
];

/// The static cheatsheet.
pub fn shortcuts() -> Vec<ShortcutEntry> {
    SHORTCUTS
        .iter()
        .map(|(group, label, keys, terms)| ShortcutEntry {
            group: group.to_string(),
            label: label.to_string(),
            keys: keys.to_string(),
            terms: terms.iter().map(|t| t.to_string()).collect(),
        })
        .collect()
}

/// A bound bundle command as a cheatsheet row under its category.
pub fn bundle_entry(c: &recur_view_core::commands::RegisteredCommand) -> ShortcutEntry {
    ShortcutEntry {
        group: c.category.clone(),
        label: c.label.clone(),
        keys: c.key_equivalent_display.clone(),
        terms: vec![
            "bundle".into(),
            "command".into(),
            c.category.clone(),
            c.key_equivalent.clone(),
            c.key_equivalent_raw.clone(),
        ],
    }
}

/// `groups`: the built-in groups, then the bundle categories they lack in `localeCompare`
/// order.
pub fn groups(bundle: &[ShortcutEntry]) -> Vec<String> {
    let mut extra: Vec<String> = Vec::new();
    for s in bundle {
        if !extra.contains(&s.group) {
            extra.push(s.group.clone());
        }
    }
    extra.sort_by(|a, b| locale_compare(a, b));
    SHORTCUT_GROUPS
        .iter()
        .map(|g| g.to_string())
        .chain(
            extra
                .into_iter()
                .filter(|g| !SHORTCUT_GROUPS.contains(&g.as_str())),
        )
        .collect()
}

/// `flat`: entries whose label, group, keys or terms contain the query, in group order.
pub fn filter(entries: &[ShortcutEntry], groups: &[String], q: &str) -> Vec<ShortcutEntry> {
    let query = q.trim().to_lowercase();
    let hit = |s: &&ShortcutEntry| {
        query.is_empty()
            || s.label.to_lowercase().contains(&query)
            || s.group.to_lowercase().contains(&query)
            || s.keys.to_lowercase().contains(&query)
            || s.terms.iter().any(|t| t.to_lowercase().contains(&query))
    };
    let matched: Vec<&ShortcutEntry> = entries.iter().filter(hit).collect();
    groups
        .iter()
        .flat_map(|g| matched.iter().filter(move |s| s.group == *g).cloned())
        .cloned()
        .collect()
}

pub struct ShortcutsHelp {
    state: Entity<AppState>,
    input: Entity<InputState>,
    bundle: Vec<ShortcutEntry>,
    scroll: ScrollHandle,
    was_open: bool,
    _subs: Vec<Subscription>,
}

impl ShortcutsHelp {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx));
        let me = cx.entity().downgrade();
        let subs = vec![
            cx.observe_in(&state, window, |this, _, window, cx| {
                this.sync_open(window, cx)
            }),
            cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.intercept_keystrokes(move |ev, window, cx| {
                let Some(me) = me.upgrade() else { return };
                if me.update(cx, |p, cx| p.on_key(ev, window, cx)) {
                    cx.stop_propagation();
                }
            }),
        ];
        ShortcutsHelp {
            state,
            input,
            bundle: Vec::new(),
            scroll: ScrollHandle::new(),
            was_open: false,
            _subs: subs,
        }
    }

    fn open(&self, cx: &gpui_kit::App) -> bool {
        self.state.read(cx).overlays.shortcuts
    }

    fn close(&self, cx: &mut Context<Self>) {
        self.state
            .update(cx, |s, cx| s.close_overlay(|o| o.shortcuts = false, cx));
    }

    fn sync_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.open(cx);
        if open && !self.was_open {
            self.input.update(cx, |s, cx| s.set_value("", window, cx));
            let focus = self.input.focus_handle(cx);
            window.defer(cx, move |window, cx| window.focus(&focus, cx));
            let bound = self.state.update(cx, |s, cx| s.list_bound_commands(cx));
            cx.spawn(async move |this, cx| {
                let cmds = bound.await;
                let _ = this.update(cx, |this, cx| {
                    this.bundle = cmds.iter().map(bundle_entry).collect();
                    cx.notify();
                });
            })
            .detach();
        }
        if !open && self.was_open {
            chrome::release_focus(&self.input, window, cx);
        }
        self.was_open = open;
        cx.notify();
    }

    fn on_key(&mut self, ev: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.open(cx) || !self.input.focus_handle(cx).is_focused(window) {
            return false;
        }
        if ev.keystroke.key == "escape" {
            self.close(cx);
            return true;
        }
        false
    }
}

impl ShortcutsHelp {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        if !self.open(cx) {
            return div().into_any_element();
        }
        style_text_input(&self.input, cx);
        let groups = groups(&self.bundle);
        let mut all = shortcuts();
        all.extend(self.bundle.iter().cloned());
        let flat = filter(&all, &groups, &self.input.read(cx).value());
        let placeholder: SharedString = "Search keyboard shortcuts…".into();
        let head = chrome::head(
            Kind::Palette,
            IconName::Search,
            &self.input,
            &placeholder,
            "esc",
            window,
            cx,
        );
        let mut rows: Vec<AnyElement> = Vec::new();
        if flat.is_empty() {
            rows.push(palette_empty("No matches", false, cx).into_any_element());
        } else {
            for g in &groups {
                let items: Vec<&ShortcutEntry> = flat.iter().filter(|s| s.group == *g).collect();
                if items.is_empty() {
                    continue;
                }
                rows.push(palette_group(g.clone(), false, cx).into_any_element());
                for s in items {
                    rows.push(sc_row(s, cx));
                }
            }
        }
        let list = chrome::list("sc-list", Kind::Palette, &self.scroll, rows);
        let foot = chrome::footer(Kind::Palette, &[("⌘⇧/", "shortcuts"), ("esc", "close")], cx);
        chrome::frame(
            Kind::Palette,
            72.,
            "shortcuts-panel",
            vec![
                head.into_any_element(),
                list.into_any_element(),
                foot.into_any_element(),
            ],
            {
                let state = self.state.clone();
                move |_, cx| state.update(cx, |s, cx| s.close_overlay(|o| o.shortcuts = false, cx))
            },
            window,
            cx,
        )
    }
}

impl Render for ShortcutsHelp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

/// `.sc-row`: the label and one `.pk` keycap per whitespace-separated chord.
fn sc_row(s: &ShortcutEntry, cx: &gpui_kit::App) -> AnyElement {
    let label = s.label.clone();
    div()
        .flex()
        .items_center()
        .gap(px(11.))
        .py(px(7.))
        .px(px(10.))
        .rounded(px(7.))
        .debug_selector(move || format!("sc-row:{label}"))
        .child(
            text(s.label.clone())
                .id(SharedString::from(format!("sc:{}:{}", s.group, s.label)))
                .size(13.)
                .color(Theme::of(cx).tokens.text),
        )
        .child(
            div()
                .ml_auto()
                .flex()
                .items_center()
                .gap(px(4.))
                .flex_none()
                .children(
                    s.keys
                        .split_whitespace()
                        .map(|chord| palette_key(chord.to_string(), false, cx)),
                ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(keys: &str, group: Option<&str>) -> ShortcutEntry {
        shortcuts()
            .into_iter()
            .find(|s| s.keys == keys && group.is_none_or(|g| s.group == g))
            .unwrap_or_else(|| panic!("no {keys}"))
    }

    fn entry(group: &str, label: &str, keys: &str, terms: &[&str]) -> ShortcutEntry {
        ShortcutEntry {
            group: group.into(),
            label: label.into(),
            keys: keys.into(),
            terms: terms.iter().map(|t| t.to_string()).collect(),
        }
    }

    #[test]
    fn lists_cmd_l_add_file_or_selection_to_chat_under_navigation() {
        assert_eq!(
            find("⌘L", None),
            entry(
                "Navigation",
                "Add file / selection to chat",
                "⌘L",
                &["@ file", "mention", "reference"]
            )
        );
    }

    #[test]
    fn lists_the_run_picker_and_stop_run_under_workbench() {
        assert_eq!(
            find("⌘R", None),
            entry(
                "Workbench",
                "Run…",
                "⌘R",
                &["dev server", "run command", "run menu"]
            )
        );
        assert_eq!(
            find("⌘⇧R", None),
            entry("Workbench", "Stop run", "⌘⇧R", &["dev server", "interrupt"])
        );
    }

    #[test]
    fn lists_blank_workspace_open_path_and_open_in_under_general() {
        assert_eq!(
            find("⇧⌘N", Some("General")),
            entry(
                "General",
                "New blank workspace",
                "⇧⌘N",
                &["quick create", "empty"]
            )
        );
        assert_eq!(
            find("⌥⌘O", Some("General")),
            entry(
                "General",
                "Open file or folder",
                "⌥⌘O",
                &["open", "folder", "file"]
            )
        );
        assert_eq!(
            find("⌘O", None),
            entry(
                "General",
                "Open in…",
                "⌘O",
                &["external editor", "finder", "terminal"]
            )
        );
    }

    #[test]
    fn lists_editor_chords() {
        assert_eq!(
            find("⌃⇧↑ ⌃⇧↓", None),
            entry(
                "Editor",
                "Add cursor above / below",
                "⌃⇧↑ ⌃⇧↓",
                &["multi cursor"]
            )
        );
        let revert = shortcuts()
            .into_iter()
            .find(|s| s.keys == "⌥⌘Z" && s.label == "Revert diff hunk")
            .unwrap();
        assert_eq!(
            revert,
            entry(
                "Editor",
                "Revert diff hunk",
                "⌥⌘Z",
                &["diff", "reject", "undo hunk"]
            )
        );
        assert_eq!(
            find("⌘⇧V", None),
            entry(
                "Editor",
                "Cycle view",
                "⌘⇧V",
                &["code", "diff", "preview", "toggle"]
            )
        );
        assert_eq!(
            find("⌥⌘[ ⌥⌘]", None),
            entry(
                "Editor",
                "Previous / next editor theme",
                "⌥⌘[ ⌥⌘]",
                &["cycle", "color scheme"]
            )
        );
    }

    #[test]
    fn lists_workspace_cycling_under_navigation() {
        assert_eq!(
            find("⌥⌘↑", None),
            entry("Navigation", "Previous workspace", "⌥⌘↑", &["cycle"])
        );
        assert_eq!(
            find("⌥⌘↓", None),
            entry("Navigation", "Next workspace", "⌥⌘↓", &["cycle"])
        );
    }

    #[test]
    fn lists_review_stepping_and_cmd_click_under_workbench() {
        assert_eq!(
            find("J / K", None),
            entry(
                "Workbench",
                "Review: next / previous file",
                "J / K",
                &["diff", "review"]
            )
        );
        assert_eq!(
            find("⌘Click", None),
            entry(
                "Workbench",
                "Open diff line in editor",
                "⌘Click",
                &["review", "diff", "jump"]
            )
        );
    }

    #[test]
    fn bundle_categories_follow_the_built_in_groups_in_collation_order() {
        let b = vec![
            entry("Text", "Uppercase", "⇧⌘U", &[]),
            entry("Editor", "Fmt", "⌃⌘F", &[]),
            entry("Go", "Run", "⌘⇧G", &[]),
        ];
        assert_eq!(
            groups(&b),
            [
                "General",
                "Navigation",
                "Workbench",
                "Editor",
                "Explorer",
                "Terminal",
                "Go",
                "Text"
            ]
        );
    }

    #[test]
    fn filters_by_label_group_keys_and_terms_in_group_order() {
        let all = shortcuts();
        let g = groups(&[]);
        let hits = filter(&all, &g, " TERMINAL ");
        assert!(hits.iter().any(|s| s.label == "Toggle terminal"));
        assert!(hits.iter().any(|s| s.label == "Open in…"));
        assert!(hits.iter().all(|s| {
            let q = "terminal";
            s.label.to_lowercase().contains(q)
                || s.group.to_lowercase().contains(q)
                || s.terms.iter().any(|t| t.to_lowercase().contains(q))
        }));
        let order: Vec<usize> = hits
            .iter()
            .map(|s| g.iter().position(|x| *x == s.group).unwrap())
            .collect();
        assert!(order.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(filter(&all, &g, "").len(), all.len());
        assert!(filter(&all, &g, "zzzz").is_empty());
    }
}
