//! `SyntaxPalette.svelte`: the focused file's language mode (the editor status bar's language
//! button, ⌘K → Change language mode). A pick is remembered for the file
//! ([`EditorEnv::apply_language_mode`]); "Always open *.ext as…" sets a file-type
//! association instead (VS Code's `files.associations`).

use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, Focusable, InteractiveElement,
    IntoElement, KeystrokeEvent, MouseMoveEvent, ParentElement, Render, ScrollHandle,
    SharedString, StatefulInteractiveElement, Subscription, Window, div,
};
use recur_code::fuzzy::{fuzzy_filter_prepared, prepare_fuzzy};
use recur_textmate::{
    FileTypeAssociation, SYNTAX_PLAIN, SyntaxLanguage, TmCatalog, file_type_association_for_path,
};
use recur_ui::IconName;
use recur_ui::logic::hover_gate::HoverGate;
use recur_ui::style::{palette_empty, palette_group, palette_key, palette_row, palette_sub, style_text_input};
use recur_view_core::AppState;
use recur_view_observatory::textmate::{EditorEnv, editor_env};

use crate::chrome::{self, Kind, plain, step};

const OCCLUDER: &str = "syntax-mode";

/// A picker row: an action on this file or a language scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub label: String,
    pub psub: String,
    pub current: bool,
}

/// What the picker knows about the file, read from the catalog.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileModes {
    /// `detectScopeForPath`: the grammar the file types pick.
    pub detected: Option<String>,
    /// `resolveScopeForPath`: what applies (`None` is Plain Text).
    pub current: Option<String>,
    /// The per-file override (`Some("")` is Plain Text).
    pub path_override: Option<String>,
    pub assoc: Option<FileTypeAssociation>,
    /// The association's scope (`Some("")` is Plain Text).
    pub assoc_scope: Option<String>,
    /// Labels of `detected` and `assoc_scope`.
    pub detected_label: String,
    pub assoc_label: String,
}

impl FileModes {
    pub fn read(catalog: &TmCatalog, path: &str) -> FileModes {
        let modes = &catalog.syntax_mode;
        let detected = catalog.detect_scope_for_path(path, None);
        let assoc = file_type_association_for_path(path);
        let assoc_scope = assoc.as_ref().and_then(|a| modes.file_type_override(&a.key));
        FileModes {
            detected_label: detected
                .as_deref()
                .map(|s| catalog.language_label_for_scope(s))
                .unwrap_or_default(),
            assoc_label: assoc_scope
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(|s| catalog.language_label_for_scope(s))
                .unwrap_or_default(),
            current: catalog.resolve_scope_for_path(path, None),
            path_override: modes.path_override(path),
            detected,
            assoc,
            assoc_scope,
        }
    }
}

/// `actions`: Auto Detect, Plain Text and the association rows; in association mode, Plain
/// Text only.
pub fn actions(m: &FileModes, associate: bool) -> Vec<Row> {
    let row = |id: &str, label: String, psub: String, current: bool| Row {
        id: id.into(),
        label,
        psub,
        current,
    };
    if associate {
        return vec![row(
            "plain",
            "Plain Text".into(),
            String::new(),
            m.assoc_scope.as_deref() == Some(SYNTAX_PLAIN),
        )];
    }
    let mut rows = vec![
        row(
            "auto",
            "Auto Detect".into(),
            if m.detected.is_some() {
                m.detected_label.clone()
            } else {
                "Plain Text".into()
            },
            m.path_override.is_none() && m.assoc_scope.is_none(),
        ),
        row(
            "plain",
            "Plain Text".into(),
            String::new(),
            m.path_override.as_deref() == Some(SYNTAX_PLAIN)
                || (m.path_override.is_none() && m.current.is_none()),
        ),
    ];
    if let Some(a) = &m.assoc {
        let psub = match m.assoc_scope.as_deref() {
            None => String::new(),
            Some("") => "Plain Text".into(),
            Some(_) => m.assoc_label.clone(),
        };
        rows.push(row(
            "assoc",
            format!("Always open {} as…", a.label),
            psub,
            false,
        ));
        if m.assoc_scope.is_some() {
            rows.push(row(
                "assoc-clear",
                format!("Clear {} association", a.label),
                String::new(),
                false,
            ));
        }
    }
    rows
}

/// `visibleActions`: actions whose label or psub contains the query.
pub fn visible_actions(actions: Vec<Row>, q: &str) -> Vec<Row> {
    let query = q.trim().to_lowercase();
    if query.is_empty() {
        return actions;
    }
    actions
        .into_iter()
        .filter(|r| r.label.to_lowercase().contains(&query) || r.psub.to_lowercase().contains(&query))
        .collect()
}

/// `langRows`: every language, or the best 80 fuzzy matches of `label language scope`.
pub fn lang_rows(langs: &[SyntaxLanguage], q: &str, current: Option<&str>, associate: bool) -> Vec<Row> {
    let hits: Vec<SyntaxLanguage> = if q.trim().is_empty() {
        langs.to_vec()
    } else {
        let prepared = prepare_fuzzy(langs, |l| {
            format!("{} {} {}", l.label, l.language, l.scope_name)
        });
        fuzzy_filter_prepared(&prepared, q, 80)
            .into_iter()
            .map(|h| h.item)
            .collect()
    };
    hits.into_iter()
        .map(|l| Row {
            current: !associate && current == Some(l.scope_name.as_str()),
            id: l.scope_name,
            label: l.label,
            psub: String::new(),
        })
        .collect()
}

pub struct SyntaxPalette {
    state: Entity<AppState>,
    input: Entity<InputState>,
    sel: usize,
    associate: bool,
    path: String,
    hover: HoverGate,
    scroll: ScrollHandle,
    revealed: Option<(usize, Vec<String>)>,
    env: Option<Entity<EditorEnv>>,
    was_open: bool,
    _subs: Vec<Subscription>,
}

impl SyntaxPalette {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx));
        let me = cx.entity().downgrade();
        let subs = vec![
            cx.observe_in(&state, window, |this, _, window, cx| {
                this.sync_open(window, cx)
            }),
            cx.subscribe(&input, |this, _, _: &InputEvent, cx| {
                this.list_changed(cx);
                cx.notify();
            }),
            cx.intercept_keystrokes(move |ev, window, cx| {
                let Some(me) = me.upgrade() else { return };
                if me.update(cx, |p, cx| p.on_key(ev, window, cx)) {
                    cx.stop_propagation();
                }
            }),
        ];
        SyntaxPalette {
            state,
            input,
            sel: 0,
            associate: false,
            path: String::new(),
            hover: HoverGate::default(),
            scroll: ScrollHandle::new(),
            revealed: None,
            env: None,
            was_open: false,
            _subs: subs,
        }
    }

    fn open(&self, cx: &App) -> bool {
        self.state.read(cx).overlays.syntax_mode
    }

    fn sync_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.open(cx);
        if open && !self.was_open {
            self.path = self.state.read(cx).current_editor_focus(cx).1;
            self.input.update(cx, |s, cx| s.set_value("", window, cx));
            self.sel = 0;
            self.associate = false;
            if self.env.is_none() {
                // The catalog's grammars register asynchronously; follow them.
                let env = editor_env(&self.state, cx);
                self._subs.push(cx.observe(&env, |this, _, cx| {
                    this.list_changed(cx);
                    cx.notify();
                }));
                self.env = Some(env);
            }
            let focus = self.input.focus_handle(cx);
            window.defer(cx, move |window, cx| window.focus(&focus, cx));
        }
        if open != self.was_open {
            if !open && self.was_open {
            chrome::release_focus(&self.input, window, cx);
        }
        self.was_open = open;
            self.hover.reset();
            let state = self.state.clone();
            cx.defer(move |cx| state.update(cx, |s, cx| s.set_chat_occluder(OCCLUDER, open, cx)));
        }
        self.clamp_sel(cx);
        cx.notify();
    }

    /// `flat` changed: the hover gate re-arms and the selection stays in range.
    fn list_changed(&mut self, cx: &App) {
        self.hover.reset();
        self.clamp_sel(cx);
    }

    /// `(visibleActions, langRows, the languages section title, file modes)`.
    fn lists(&self, cx: &App) -> (Vec<Row>, Vec<Row>, String, FileModes) {
        let Some(env) = &self.env else {
            return (Vec::new(), Vec::new(), "Languages".into(), FileModes::default());
        };
        let tm = env.read(cx).tm.clone();
        let tm = tm.borrow();
        let catalog = &tm.catalog;
        let q = self.input.read(cx).value().to_string();
        let modes = if self.path.is_empty() {
            FileModes::default()
        } else {
            FileModes::read(catalog, &self.path)
        };
        let acts = if self.path.is_empty() {
            Vec::new()
        } else {
            actions(&modes, self.associate)
        };
        let langs = lang_rows(
            &catalog.list_syntax_languages(),
            &q,
            modes.current.as_deref(),
            self.associate,
        );
        let title = match (&modes.assoc, self.associate) {
            (Some(a), true) => a.label.clone(),
            _ => "Languages".into(),
        };
        (visible_actions(acts, &q), langs, title, modes)
    }

    fn flat(&self, cx: &App) -> Vec<Row> {
        let (mut a, l, _, _) = self.lists(cx);
        a.extend(l);
        a
    }

    fn clamp_sel(&mut self, cx: &App) {
        let max = self.flat(cx).len().saturating_sub(1);
        if self.sel > max {
            self.sel = max;
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.associate = false;
        self.state
            .update(cx, |s, cx| s.close_overlay(|o| o.syntax_mode = false, cx));
    }

    fn pick(&mut self, scope: Option<&str>, cx: &mut Context<Self>) {
        let Some(env) = self.env.clone().filter(|_| !self.path.is_empty()) else {
            return;
        };
        let path = self.path.clone();
        let assoc = file_type_association_for_path(&path).filter(|_| self.associate);
        env.update(cx, |e, cx| match (&assoc, scope) {
            (Some(a), Some(scope)) => e.set_file_type_language(&a.key, Some(scope), Some(&path), cx),
            _ => e.apply_language_mode(&path, scope, cx),
        });
        self.close(cx);
    }

    fn activate(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        match id {
            "auto" => self.pick(None, cx),
            "plain" => self.pick(Some(SYNTAX_PLAIN), cx),
            "assoc" => {
                self.associate = true;
                self.sel = 0;
                self.hover.reset();
                self.input.update(cx, |s, cx| s.set_value("", window, cx));
            }
            "assoc-clear" => {
                if let (Some(a), Some(env)) =
                    (file_type_association_for_path(&self.path), self.env.clone())
                {
                    env.update(cx, |e, cx| e.set_file_type_language(&a.key, None, None, cx));
                }
                self.close(cx);
            }
            scope => {
                let scope = scope.to_string();
                self.pick(Some(&scope), cx)
            }
        }
        cx.notify();
    }

    fn run(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.flat(cx).get(i).cloned() {
            self.activate(&row.id, window, cx);
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
                let n = self.flat(cx).len();
                self.sel = step(self.sel, n, k.key == "down");
                cx.notify();
                true
            }
            "enter" => {
                self.run(self.sel, window, cx);
                true
            }
            "escape" => {
                if self.associate {
                    self.associate = false;
                    self.sel = 0;
                    self.input.update(cx, |s, cx| s.set_value("", window, cx));
                    cx.notify();
                } else {
                    self.close(cx);
                }
                true
            }
            _ => false,
        }
    }

    fn row(&self, idx: usize, r: &Row, cx: &mut Context<Self>) -> AnyElement {
        let on = idx == self.sel;
        let label = r.label.clone();
        let mut row = palette_row(SharedString::from(format!("syn-row:{}", r.id)), on, true, cx)
            .debug_selector(move || format!("syn-row:{label}{}", if on { ":sel" } else { "" }))
            .child(chrome::label(
                SharedString::from(format!("syn-pl:{}", r.id)),
                r.label.clone(),
                Kind::Command,
                cx,
            ));
        if !r.psub.is_empty() {
            row = row.child(palette_sub(r.psub.clone(), cx));
        }
        if r.current {
            row = row.child(palette_key("current", on, cx));
        }
        row.on_mouse_move(cx.listener(move |p, ev: &MouseMoveEvent, _, cx| {
            if p.hover
                .allows(f32::from(ev.position.x), f32::from(ev.position.y))
                && p.sel != idx
            {
                p.sel = idx;
                cx.notify();
            }
        }))
        .on_click(cx.listener(move |p, _, window, cx| p.run(idx, window, cx)))
        .into_any_element()
    }
}

impl Render for SyntaxPalette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open(cx) {
            return div().into_any_element();
        }
        style_text_input(&self.input, cx);
        let (acts, langs, lang_title, modes) = self.lists(cx);
        let filtered = !self.input.read(cx).value().trim().is_empty();
        let placeholder: SharedString = match (&modes.assoc, self.associate) {
            (Some(a), true) => format!("Select language for {}…", a.label).into(),
            _ => "Select language mode…".into(),
        };
        let head = chrome::head(
            Kind::Command,
            IconName::Code,
            &self.input,
            &placeholder,
            "esc",
            window,
            cx,
        );
        let mut rows: Vec<AnyElement> = Vec::new();
        let n = acts.len() + langs.len();
        let mut sel_child = None;
        if self.path.is_empty() {
            rows.push(
                palette_empty("Open a file to change its language", true, cx).into_any_element(),
            );
        } else if n == 0 {
            let msg = if filtered {
                "No matching languages"
            } else {
                "No languages loaded"
            };
            rows.push(palette_empty(msg, true, cx).into_any_element());
        } else {
            if !acts.is_empty() {
                rows.push(
                    palette_group("This file", true, cx)
                        .debug_selector(|| "syn-group:This file".into())
                        .into_any_element(),
                );
                for (i, r) in acts.iter().enumerate() {
                    if i == self.sel {
                        sel_child = Some(rows.len());
                    }
                    rows.push(self.row(i, r, cx));
                }
            }
            if !langs.is_empty() {
                let sel = format!("syn-group:{lang_title}");
                rows.push(
                    palette_group(lang_title, true, cx)
                        .debug_selector(move || sel)
                        .into_any_element(),
                );
                for (i, r) in langs.iter().enumerate() {
                    let idx = acts.len() + i;
                    if idx == self.sel {
                        sel_child = Some(rows.len());
                    }
                    rows.push(self.row(idx, r, cx));
                }
            }
        }
        let ids: Vec<String> = acts.iter().chain(&langs).map(|r| r.id.clone()).collect();
        let key = (self.sel, ids);
        if self.revealed.as_ref() != Some(&key) {
            if let Some(ix) = sel_child {
                self.scroll.scroll_to_item(ix);
            }
            self.revealed = Some(key);
        }
        let list = chrome::list("syn-list", Kind::Command, &self.scroll, rows);
        let enter = if self.associate {
            "set association"
        } else {
            "this file"
        };
        let foot = chrome::footer(
            Kind::Command,
            &[("↑↓", "navigate"), ("↵", enter), ("esc", "close")],
            cx,
        );
        let me = cx.entity().downgrade();
        chrome::frame(
            Kind::Command,
            Kind::Command.max_vh(),
            "syntax-palette",
            vec![
                head.into_any_element(),
                list.into_any_element(),
                foot.into_any_element(),
            ],
            move |_, cx| {
                let _ = me.update(cx, |p, cx| p.close(cx));
            },
            window,
            cx,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn go() -> FileModes {
        FileModes {
            detected: Some("source.go".into()),
            current: Some("source.go".into()),
            detected_label: "Go".into(),
            assoc: Some(FileTypeAssociation {
                key: "*.go".into(),
                label: "*.go".into(),
            }),
            ..Default::default()
        }
    }

    fn summary(rows: &[Row]) -> Vec<(String, String, bool)> {
        rows.iter()
            .map(|r| (r.id.clone(), r.psub.clone(), r.current))
            .collect()
    }

    fn s(id: &str, psub: &str, current: bool) -> (String, String, bool) {
        (id.into(), psub.into(), current)
    }

    #[test]
    fn auto_detect_is_current_without_overrides() {
        assert_eq!(
            summary(&actions(&go(), false)),
            [s("auto", "Go", true), s("plain", "", false), s("assoc", "", false)]
        );
        assert_eq!(actions(&go(), false)[2].label, "Always open *.go as…");
    }

    #[test]
    fn plain_text_is_current_for_an_explicit_pick_or_an_undetected_file() {
        let mut m = go();
        m.path_override = Some(String::new());
        m.current = None;
        assert_eq!(
            summary(&actions(&m, false))[..2],
            [s("auto", "Go", false), s("plain", "", true)]
        );
        let m = FileModes::default();
        assert_eq!(
            summary(&actions(&m, false)),
            [s("auto", "Plain Text", true), s("plain", "", true)]
        );
    }

    #[test]
    fn an_association_shows_its_language_and_a_clear_row() {
        let mut m = go();
        m.assoc_scope = Some("source.python".into());
        m.assoc_label = "Python".into();
        let rows = actions(&m, false);
        assert_eq!(
            summary(&rows),
            [
                s("auto", "Go", false),
                s("plain", "", false),
                s("assoc", "Python", false),
                s("assoc-clear", "", false)
            ]
        );
        assert_eq!(rows[3].label, "Clear *.go association");
        m.assoc_scope = Some(String::new());
        assert_eq!(actions(&m, false)[2].psub, "Plain Text");
        assert_eq!(summary(&actions(&m, true)), [s("plain", "", true)]);
    }

    #[test]
    fn actions_filter_on_label_and_psub() {
        let rows = visible_actions(actions(&go(), false), " GO ");
        assert_eq!(summary(&rows), [s("auto", "Go", true), s("assoc", "", false)]);
    }

    #[test]
    fn languages_fuzzy_filter_and_mark_the_current_one() {
        let langs: Vec<SyntaxLanguage> = [("source.go", "Go", "go"), ("source.python", "Python", "python")]
            .iter()
            .map(|(scope, label, lang)| SyntaxLanguage {
                scope_name: scope.to_string(),
                label: label.to_string(),
                language: lang.to_string(),
            })
            .collect();
        let all = lang_rows(&langs, "", Some("source.go"), false);
        assert_eq!(summary(&all), [s("source.go", "", true), s("source.python", "", false)]);
        assert!(!lang_rows(&langs, "", Some("source.go"), true)[0].current);
        let hits = lang_rows(&langs, "pyth", None, false);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "source.python");
    }
}
