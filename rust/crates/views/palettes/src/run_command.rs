//! `RunCommandPalette.svelte`: bundle commands and snippets for the focused file (⌘⇧P,
//! Navigate → Run Command…, the status bar's Bundles). Recently run ones lead, in frecency
//! order (`recur_view_observatory::command_usage`); snippets show their tab trigger (`ife⇥`).

use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, Focusable, InteractiveElement, IntoElement,
    KeystrokeEvent, ParentElement, Render, ScrollHandle, SharedString, StatefulInteractiveElement,
    Styled, Subscription, Window, div, px,
};
use recur_editor::snippet::{RegisteredSnippet, snippet_trigger_hint};
use recur_editor::text::locale_compare;
use recur_ui::style::{
    palette_empty, palette_group, palette_key, palette_row, palette_sub, style_text_input,
};
use recur_ui::widgets::ToastKind;
use recur_ui::{IconName, TextStyled, Theme};
use recur_view_core::AppState;
use recur_view_core::commands::{RegisteredCommand, categories};
use recur_view_observatory::command_usage::{
    RECENT_SECTION_SIZE, ranked_command_ids, record_command_usage,
};

use crate::chrome::{self, Kind, plain, step};

pub const RECENT: &str = "Recent";

/// What a row runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Command(String),
    Snippet(Box<RegisteredSnippet>),
}

/// `Row`.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: String,
    pub category: String,
    pub label: String,
    pub keys: Vec<String>,
    pub kb: String,
    pub hint: String,
    pub target: Target,
}

/// `rows`: palette-triggered commands, then snippets.
pub fn rows(cmds: &[RegisteredCommand], snips: &[RegisteredSnippet]) -> Vec<Row> {
    let mut out: Vec<Row> = cmds
        .iter()
        .filter(|c| c.triggers.iter().any(|t| t == "palette"))
        .map(|c| Row {
            id: c.id.clone(),
            category: c.category.clone(),
            label: c.label.clone(),
            keys: vec![
                c.category.clone(),
                c.id.clone(),
                c.interpreter.clone(),
                c.key_equivalent_display.clone(),
                c.key_equivalent.clone(),
            ],
            kb: c.key_equivalent_display.clone(),
            hint: String::new(),
            target: Target::Command(c.id.clone()),
        })
        .collect();
    out.extend(snips.iter().map(|s| Row {
        id: s.id.clone(),
        category: snippet_category(s),
        label: s.label.clone(),
        keys: vec![
            s.bundle_name.clone().unwrap_or_default(),
            s.id.clone(),
            s.tab_trigger.clone(),
            s.key_equivalent_display.clone().unwrap_or_default(),
            s.key_equivalent.clone().unwrap_or_default(),
        ],
        kb: s.key_equivalent_display.clone().unwrap_or_default(),
        hint: snippet_trigger_hint(&s.tab_trigger),
        target: Target::Snippet(Box::new(s.clone())),
    }));
    out
}

fn snippet_category(s: &RegisteredSnippet) -> String {
    s.bundle_name
        .clone()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Snippets".into())
}

/// `cats`: the command categories, then snippet bundles they lack in `localeCompare` order.
pub fn sections(cmds: &[RegisteredCommand], snips: &[RegisteredSnippet]) -> Vec<String> {
    let mut cats = categories(cmds);
    let mut extra: Vec<String> = Vec::new();
    for s in snips {
        let c = snippet_category(s);
        if !extra.contains(&c) && !cats.contains(&c) {
            extra.push(c);
        }
    }
    extra.sort_by(|a, b| locale_compare(a, b));
    cats.extend(extra);
    cats
}

/// `matches`.
pub fn matches(r: &Row, query: &str) -> bool {
    query.is_empty()
        || r.label.to_lowercase().contains(query)
        || r.category.to_lowercase().contains(query)
        || r.keys.iter().any(|k| k.to_lowercase().contains(query))
        || r.hint.to_lowercase().contains(query)
}

/// One rendered section: its title and `(flat index, row)` pairs.
pub type Section = (String, Vec<(usize, Row)>);

/// `flat` and `displaySections`: the recent rows still available here and matching (in
/// frecency order), then each category's remaining matches.
pub fn layout(
    rows: &[Row],
    cats: &[String],
    recent: &[String],
    q: &str,
) -> (Vec<Row>, Vec<Section>) {
    let query = q.trim().to_lowercase();
    let by_id = |id: &str| rows.iter().find(|r| r.id == id);
    let recent: Vec<&Row> = recent
        .iter()
        .filter_map(|id| by_id(id))
        .filter(|r| matches(r, &query))
        .collect();
    let in_recent = |r: &Row| recent.iter().any(|x| x.id == r.id);
    let mut flat: Vec<Row> = recent.iter().map(|r| (*r).clone()).collect();
    for g in cats {
        flat.extend(
            rows.iter()
                .filter(|r| r.category == *g && matches(r, &query) && !in_recent(r))
                .cloned(),
        );
    }
    let mut out: Vec<Section> = Vec::new();
    if !recent.is_empty() {
        let items: Vec<(usize, Row)> = recent
            .iter()
            .filter_map(|r| {
                flat.iter()
                    .position(|x| x.id == r.id)
                    .map(|i| (i, flat[i].clone()))
            })
            .collect();
        if !items.is_empty() {
            out.push((RECENT.into(), items));
        }
    }
    for g in cats {
        let items: Vec<(usize, Row)> = flat
            .iter()
            .enumerate()
            .filter(|(_, r)| r.category == *g && !in_recent(r))
            .map(|(i, r)| (i, r.clone()))
            .collect();
        if !items.is_empty() {
            out.push((g.clone(), items));
        }
    }
    (flat, out)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

pub struct RunCommandPalette {
    state: Entity<AppState>,
    input: Entity<InputState>,
    sel: usize,
    cats: Vec<String>,
    cmds: Vec<RegisteredCommand>,
    snips: Vec<RegisteredSnippet>,
    /// Re-read on each open so new runs reshuffle Recent.
    recent: Vec<String>,
    scroll: ScrollHandle,
    revealed: Option<(usize, Vec<String>)>,
    was_open: bool,
    _subs: Vec<Subscription>,
}

impl RunCommandPalette {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx));
        let me = cx.entity().downgrade();
        let subs = vec![
            cx.observe_in(&state, window, |this, _, window, cx| {
                this.sync_open(window, cx)
            }),
            cx.subscribe(&input, |this, _, _: &InputEvent, cx| {
                this.clamp_sel(cx);
                cx.notify();
            }),
            cx.intercept_keystrokes(move |ev, window, cx| {
                let Some(me) = me.upgrade() else { return };
                if me.update(cx, |p, cx| p.on_key(ev, window, cx)) {
                    cx.stop_propagation();
                }
            }),
        ];
        RunCommandPalette {
            state,
            input,
            sel: 0,
            cats: Vec::new(),
            cmds: Vec::new(),
            snips: Vec::new(),
            recent: Vec::new(),
            scroll: ScrollHandle::new(),
            revealed: None,
            was_open: false,
            _subs: subs,
        }
    }

    fn open(&self, cx: &App) -> bool {
        self.state.read(cx).overlays.commands
    }

    fn layout(&self, cx: &App) -> (Vec<Row>, Vec<Section>) {
        layout(
            &rows(&self.cmds, &self.snips),
            &self.cats,
            &self.recent,
            &self.input.read(cx).value(),
        )
    }

    fn clamp_sel(&mut self, cx: &App) {
        let n = self.layout(cx).0.len();
        if self.sel >= n {
            self.sel = n.saturating_sub(1);
        }
    }

    fn sync_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.open(cx);
        if open && !self.was_open {
            self.input.update(cx, |s, cx| s.set_value("", window, cx));
            self.sel = 0;
            self.recent =
                ranked_command_ids(self.state.read(cx).storage(), RECENT_SECTION_SIZE, now_ms());
            let focus = self.input.focus_handle(cx);
            window.defer(cx, move |window, cx| window.focus(&focus, cx));
            let (scope, path) = self.state.read(cx).current_editor_focus(cx);
            let cmds = self
                .state
                .update(cx, |s, cx| s.list_commands(&scope, &path, cx));
            let env = recur_view_observatory::textmate::editor_env(&self.state, cx);
            let snips = env.read(cx).snippets.list(&scope, &path);
            cx.spawn(async move |this, cx| {
                let (cmds, snips) = futures::join!(cmds, snips);
                let _ = this.update(cx, |this, cx| {
                    this.cats = sections(&cmds, &snips);
                    this.cmds = cmds;
                    this.snips = snips;
                    this.clamp_sel(cx);
                    cx.notify();
                });
            })
            .detach();
        }
        if !open && self.was_open {
            chrome::release_focus(&self.input, window, cx);
        }
        self.was_open = open;
        self.clamp_sel(cx);
        cx.notify();
    }

    fn close(&self, cx: &mut App) {
        self.state
            .update(cx, |s, cx| s.close_overlay(|o| o.commands = false, cx));
    }

    fn run(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.layout(cx).0.get(i).cloned() else {
            return;
        };
        self.close(cx);
        record_command_usage(&self.state, &r.id, cx);
        match r.target {
            Target::Command(id) => {
                // The editor's handler updates the window, so it runs once this update is out.
                let state = self.state.clone();
                cx.defer(move |cx| {
                    let task = AppState::run_editor_command(&state, &id, cx);
                    cx.spawn(async move |cx| {
                        if let Err(e) = task.await {
                            cx.update(|cx| recur_view_core::toast(ToastKind::Error, e, cx));
                        }
                    })
                    .detach();
                });
            }
            Target::Snippet(sn) => {
                if let Err(e) =
                    recur_view_observatory::snippets::run_editor_snippet(*sn, window, cx)
                {
                    recur_view_core::toast(ToastKind::Error, e, cx);
                }
            }
        }
    }

    fn on_key(&mut self, ev: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.open(cx) || !self.input.focus_handle(cx).is_focused(window) {
            return false;
        }
        let k = &ev.keystroke;
        if !plain(k) {
            return false;
        }
        match k.key.as_str() {
            "down" | "up" => {
                let n = self.layout(cx).0.len();
                self.sel = step(self.sel, n, k.key == "down");
                cx.notify();
                true
            }
            "enter" => {
                self.run(self.sel, window, cx);
                true
            }
            "escape" => {
                self.close(cx);
                true
            }
            _ => false,
        }
    }
}

impl Render for RunCommandPalette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open(cx) {
            return div().into_any_element();
        }
        style_text_input(&self.input, cx);
        let t = Theme::of(cx).tokens;
        let (flat, sections) = self.layout(cx);
        let filtered = !self.input.read(cx).value().trim().is_empty();
        let placeholder: SharedString = "Type a command or snippet…".into();
        let head = chrome::head(
            Kind::Command,
            IconName::Command,
            &self.input,
            &placeholder,
            "⌘⇧P",
            window,
            cx,
        );
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut sel_child = None;
        if flat.is_empty() {
            let msg = if filtered {
                "No matching commands or snippets"
            } else {
                "No commands or snippets for this file"
            };
            rows.push(palette_empty(msg, true, cx).into_any_element());
        } else {
            for (title, items) in &sections {
                rows.push(
                    palette_group(title.clone(), true, cx)
                        .debug_selector({
                            let t = title.clone();
                            move || format!("rc-group:{t}")
                        })
                        .into_any_element(),
                );
                let recent = title == RECENT;
                for (idx, r) in items {
                    let (idx, on) = (*idx, *idx == self.sel);
                    if on {
                        sel_child = Some(rows.len());
                    }
                    let key = format!("{title}:{}", r.id);
                    let label = r.label.clone();
                    let mut row =
                        palette_row(SharedString::from(format!("rc-row:{key}")), on, true, cx)
                            .debug_selector(move || {
                                format!("rc-row:{label}{}", if on { ":sel" } else { "" })
                            })
                            .child(chrome::label(
                                SharedString::from(format!("rc-pl:{key}")),
                                r.label.clone(),
                                Kind::Command,
                                cx,
                            ));
                    if recent {
                        row = row.child(palette_sub(r.category.clone(), cx));
                    }
                    if !r.hint.is_empty() {
                        row = row.child(
                            div().ml_auto().flex_none().child(
                                recur_ui::text(r.hint.clone())
                                    .mono()
                                    .size(11.)
                                    .color(t.faint)
                                    .nowrap(),
                            ),
                        );
                    }
                    if !r.kb.is_empty() {
                        let pk = palette_key(r.kb.clone(), on, cx);
                        // `.ptrig + .pk { margin-left: 8px }`.
                        row = row.child(if r.hint.is_empty() { pk } else { pk.ml(px(8.)) });
                    }
                    rows.push(
                        row.on_mouse_move(cx.listener(move |p, _, _, cx| {
                            if p.sel != idx {
                                p.sel = idx;
                                cx.notify();
                            }
                        }))
                        .on_click(cx.listener(move |p, _, window, cx| p.run(idx, window, cx)))
                        .into_any_element(),
                    );
                }
            }
        }
        let key = (self.sel, flat.iter().map(|r| r.id.clone()).collect());
        if self.revealed.as_ref() != Some(&key) {
            if let Some(ix) = sel_child {
                self.scroll.scroll_to_item(ix);
            }
            self.revealed = Some(key);
        }
        let list = chrome::list("rc-list", Kind::Command, &self.scroll, rows);
        let foot = chrome::footer(
            Kind::Command,
            &[("↑↓", "navigate"), ("↵", "run"), ("esc", "close")],
            cx,
        );
        let state = self.state.clone();
        chrome::frame(
            Kind::Command,
            Kind::Command.max_vh(),
            "cmd-palette",
            vec![
                head.into_any_element(),
                list.into_any_element(),
                foot.into_any_element(),
            ],
            move |_, cx| state.update(cx, |s, cx| s.close_overlay(|o| o.commands = false, cx)),
            window,
            cx,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(id: &str, label: &str, category: &str, order: i64, palette: bool) -> RegisteredCommand {
        RegisteredCommand {
            id: id.into(),
            label: label.into(),
            category: category.into(),
            order,
            scope: String::new(),
            file_glob: String::new(),
            triggers: if palette {
                vec!["palette".into()]
            } else {
                vec!["willSave".into()]
            },
            prompts: Vec::new(),
            input: String::new(),
            fallback_input: String::new(),
            output: String::new(),
            interpreter: "typescript".into(),
            bundle_name: String::new(),
            key_equivalent_raw: String::new(),
            key_equivalent: String::new(),
            key_equivalent_display: String::new(),
            language: String::new(),
            open_in_preview: false,
        }
    }

    fn snip(id: &str, label: &str, bundle: Option<&str>, trigger: &str) -> RegisteredSnippet {
        RegisteredSnippet {
            id: id.into(),
            label: label.into(),
            tab_trigger: trigger.into(),
            bundle_name: bundle.map(String::from),
            ..Default::default()
        }
    }

    fn ids(v: &[Row]) -> Vec<&str> {
        v.iter().map(|r| r.id.as_str()).collect()
    }

    #[test]
    fn palette_commands_then_snippets_with_their_trigger() {
        let cmds = [
            cmd("sort", "Sort Lines", "Text", 10, true),
            cmd("fmt", "gofmt", "Go", 20, false),
        ];
        let snips = [
            snip("ife", "If Err", Some("Go"), " ife "),
            snip("x", "X", None, ""),
        ];
        let r = rows(&cmds, &snips);
        assert_eq!(ids(&r), ["sort", "ife", "x"]);
        assert_eq!(r[1].hint, "ife⇥");
        assert_eq!(r[2].category, "Snippets");
        assert_eq!(r[2].hint, "");
    }

    #[test]
    fn snippet_bundles_follow_command_categories_in_collation_order() {
        let cmds = [
            cmd("a", "A", "Text", 10, true),
            cmd("b", "B", "Go", 5, true),
        ];
        let snips = [
            snip("s1", "S1", Some("Ruby"), ""),
            snip("s2", "S2", Some("Go"), ""),
            snip("s3", "S3", None, ""),
        ];
        assert_eq!(sections(&cmds, &snips), ["Go", "Text", "Ruby", "Snippets"]);
    }

    #[test]
    fn recent_leads_in_frecency_order_and_leaves_its_category() {
        let cmds = [
            cmd("a", "Alpha", "Text", 10, true),
            cmd("b", "Beta", "Text", 10, true),
            cmd("c", "Gamma", "Go", 20, true),
        ];
        let r = rows(&cmds, &[]);
        let cats = sections(&cmds, &[]);
        let recent = vec!["c".to_string(), "gone".to_string(), "a".to_string()];
        let (flat, secs) = layout(&r, &cats, &recent, "");
        assert_eq!(ids(&flat), ["c", "a", "b"]);
        let titles: Vec<&str> = secs.iter().map(|s| s.0.as_str()).collect();
        assert_eq!(titles, ["Recent", "Text"]);
        assert_eq!(
            secs[0].1.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
            [0, 1]
        );
        assert_eq!(secs[1].1[0].0, 2);
        // A query filters Recent too.
        let (flat, secs) = layout(&r, &cats, &recent, "alp");
        assert_eq!(ids(&flat), ["a"]);
        assert_eq!(secs[0].0, "Recent");
        let (flat, _) = layout(&r, &cats, &recent, "typescript");
        assert_eq!(flat.len(), 3);
    }
}
