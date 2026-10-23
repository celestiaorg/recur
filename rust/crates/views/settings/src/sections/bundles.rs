//! The Bundles panes: commands (and skipped packs), prompt actions, grammars, language
//! servers, themes and snippets, each turned on or off by id.

use gpui_kit::{
    AnyElement, App, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use recur_code::view::skills::toggle_disabled_name;
use recur_domain::Settings;
use recur_editor::text::locale_compare;
use recur_services::api::{GrammarInfo, ThemeInfo};
use recur_ui::fonts::weight;
use recur_ui::style::card;
use recur_ui::text::{InlineBox, LineHeight, TextStyled, span, text};
use recur_ui::widgets::{settings_group, settings_groups};
use recur_view_core::commands::RegisteredCommand;

use crate::field::FieldOpts;
use crate::style::{
    Row, RowsCtx, copy, ctrl, ctrl_text, hsla, lead, rows, search_box, skill_copy, skill_list, tog,
    tokens,
};
use crate::view::SettingsView;

const IN_GROUP: RowsCtx = RowsCtx {
    sides: true,
    in_group: true,
    more: false,
};

/// Which off-list a toggle edits.
#[derive(Clone, Copy)]
enum List {
    Commands,
    Prompts,
    Grammars,
    Themes,
    Snippets,
    Lsp,
}

fn list_mut(s: &mut Settings, l: List) -> &mut Vec<String> {
    match l {
        List::Commands => &mut s.disabled_bundle_commands,
        List::Prompts => &mut s.disabled_bundle_prompts,
        List::Grammars => &mut s.disabled_bundle_grammars,
        List::Themes => &mut s.disabled_bundle_themes,
        List::Snippets => &mut s.disabled_bundle_snippets,
        List::Lsp => &mut s.disabled_language_servers,
    }
}

fn list_of(s: &Settings, l: List) -> &[String] {
    match l {
        List::Commands => &s.disabled_bundle_commands,
        List::Prompts => &s.disabled_bundle_prompts,
        List::Grammars => &s.disabled_bundle_grammars,
        List::Themes => &s.disabled_bundle_themes,
        List::Snippets => &s.disabled_bundle_snippets,
        List::Lsp => &s.disabled_language_servers,
    }
}

/// Items grouped by bundle name (`fallback` when empty), groups A–Z, items in input order.
fn by_bundle<T: Clone>(
    items: &[T],
    name: impl Fn(&T) -> &str,
    fallback: &str,
) -> Vec<(String, Vec<T>)> {
    let mut groups: Vec<(String, Vec<T>)> = Vec::new();
    for it in items {
        let n = match name(it) {
            "" => fallback.to_string(),
            n => n.to_string(),
        };
        match groups.iter_mut().find(|(g, _)| *g == n) {
            Some((_, v)) => v.push(it.clone()),
            None => groups.push((n, vec![it.clone()])),
        }
    }
    groups.sort_by(|a, b| locale_compare(&a.0, &b.0));
    groups
}

/// `bundleGroups`: commands by category, categories by their lowest order then name, commands
/// by order then label.
pub fn command_groups(cmds: &[RegisteredCommand]) -> Vec<(String, Vec<RegisteredCommand>)> {
    let mut meta: Vec<(String, i64, Vec<RegisteredCommand>)> = Vec::new();
    for c in cmds {
        let cat = if c.category.is_empty() {
            "Commands".to_string()
        } else {
            c.category.clone()
        };
        match meta.iter_mut().find(|(k, _, _)| *k == cat) {
            Some((_, order, list)) => {
                if c.order < *order {
                    *order = c.order;
                }
                list.push(c.clone());
            }
            None => meta.push((cat, c.order, vec![c.clone()])),
        }
    }
    meta.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| locale_compare(&a.0, &b.0)));
    meta.into_iter()
        .map(|(cat, _, mut list)| {
            list.sort_by(|a, b| {
                a.order
                    .cmp(&b.order)
                    .then_with(|| locale_compare(&a.label, &b.label))
            });
            (cat, list)
        })
        .collect()
}

/// `grammarTitle`.
pub fn grammar_title(g: &GrammarInfo) -> String {
    let t = if !g.language.is_empty() {
        &g.language
    } else if !g.scope_name.is_empty() {
        &g.scope_name
    } else {
        &g.id
    };
    t.trim().to_string()
}

/// `grammarGroups`: filtered by the query over title, scope, id, bundle and file types, sorted
/// by title then id, grouped by bundle.
pub fn grammar_groups(list: &[GrammarInfo], query: &str) -> Vec<(String, Vec<GrammarInfo>)> {
    let q = query.trim().to_lowercase();
    let mut items: Vec<GrammarInfo> = list
        .iter()
        .filter(|g| {
            q.is_empty() || {
                let mut hay = vec![
                    grammar_title(g),
                    g.scope_name.clone(),
                    g.id.clone(),
                    g.bundle_name.clone(),
                ];
                hay.extend(g.file_types.iter().cloned());
                hay.join(" ").to_lowercase().contains(&q)
            }
        })
        .cloned()
        .collect();
    items.sort_by(|a, b| {
        locale_compare(
            &grammar_title(a).to_lowercase(),
            &grammar_title(b).to_lowercase(),
        )
        .then_with(|| locale_compare(&a.id, &b.id))
    });
    by_bundle(&items, |g| &g.bundle_name, "Grammars")
}

/// `themeGroups`: filtered over name, id, bundle and ui theme; ordered themes first by order,
/// unordered ones after, then by name; grouped by bundle.
pub fn theme_groups(list: &[ThemeInfo], query: &str) -> Vec<(String, Vec<ThemeInfo>)> {
    let q = query.trim().to_lowercase();
    let mut items: Vec<ThemeInfo> = list
        .iter()
        .filter(|t| {
            q.is_empty()
                || [t.name.as_str(), &t.id, &t.bundle_name, &t.ui_theme]
                    .join(" ")
                    .to_lowercase()
                    .contains(&q)
        })
        .cloned()
        .collect();
    items.sort_by(|a, b| {
        let (ao, bo) = (a.order, b.order);
        if ao != bo && (ao == 0 || bo == 0) {
            return if ao == 0 {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Less
            };
        }
        ao.cmp(&bo).then_with(|| locale_compare(&a.name, &b.name))
    });
    by_bundle(&items, |t| &t.bundle_name, "Themes")
}

/// `snippetBlurb`: the tab trigger and the shortcut.
pub fn snippet_blurb(s: &recur_editor::snippet::RegisteredSnippet) -> String {
    let mut bits = Vec::new();
    if !s.tab_trigger.is_empty() {
        bits.push(s.tab_trigger.clone());
    }
    if let Some(k) = s.key_equivalent_display.as_ref().filter(|k| !k.is_empty()) {
        bits.push(k.clone());
    }
    bits.join(" · ")
}

/// `snippetGroups`: filtered over label, id, trigger and bundle, grouped by bundle.
pub fn snippet_groups(
    list: &[recur_editor::snippet::RegisteredSnippet],
    query: &str,
) -> Vec<(String, Vec<recur_editor::snippet::RegisteredSnippet>)> {
    let q = query.trim().to_lowercase();
    let items: Vec<_> = list
        .iter()
        .filter(|s| {
            q.is_empty()
                || [
                    s.label.as_str(),
                    &s.id,
                    &s.tab_trigger,
                    s.bundle_name.as_deref().unwrap_or(""),
                ]
                .join(" ")
                .to_lowercase()
                .contains(&q)
        })
        .cloned()
        .collect();
    by_bundle(
        &items,
        |s| s.bundle_name.as_deref().unwrap_or(""),
        "Snippets",
    )
}

/// `promptGroups`: by slash then name, grouped by bundle.
pub fn prompt_groups(
    list: &[recur_services::api::PromptActionInfo],
) -> Vec<(String, Vec<recur_services::api::PromptActionInfo>)> {
    let mut items = list.to_vec();
    items.sort_by(|a, b| {
        locale_compare(&a.slash, &b.slash).then_with(|| locale_compare(&a.name, &b.name))
    });
    by_bundle(&items, |p| &p.bundle_name, "Prompts")
}

impl SettingsView {
    /// A bundle `.tog` that flips `id` on `list`.
    fn bundle_tog(&self, list: List, id: &str, label: &str, cx: &mut Context<Self>) -> AnyElement {
        let has = self.settings(cx).is_some();
        let on = self
            .settings(cx)
            .is_none_or(|s| !list_of(s, list).iter().any(|d| d == id));
        let mut t = tog(
            SharedString::from(format!("tog-{id}")),
            format!("{} {label}", if on { "Disable" } else { "Enable" }),
            on,
            !has,
            cx,
        );
        if has {
            let id = id.to_string();
            t = t.on_click(cx.listener(move |this, _, _, cx| {
                let id = id.clone();
                let save = this.patch(
                    false,
                    move |s| {
                        let next = toggle_disabled_name(list_of(s, list), &id, !on);
                        *list_mut(s, list) = next;
                    },
                    cx,
                );
                if matches!(list, List::Themes) {
                    // `setThemeEnabled`: the pickers and the View menu refresh from the
                    // catalog once saved.
                    let state = this.vcx.state.clone();
                    cx.spawn(async move |_, cx| {
                        save.await;
                        cx.update(|cx| state.update(cx, |_, cx| cx.notify()));
                    })
                    .detach();
                } else {
                    save.detach();
                }
            }));
        }
        t.into_any_element()
    }

    /// A loading / error / empty note in place of the list.
    fn state_note(
        &self,
        id: &str,
        loading: bool,
        err: &str,
        loading_msg: &str,
        window: &Window,
        cx: &App,
    ) -> Option<AnyElement> {
        if loading {
            return Some(
                ctrl_text(
                    SharedString::from(format!("{id}-msg")),
                    vec![span(loading_msg.to_string())],
                    window,
                    cx,
                )
                .into_any_element(),
            );
        }
        if !err.is_empty() {
            return Some(
                ctrl_text(
                    SharedString::from(format!("{id}-msg")),
                    vec![span(err.to_string())],
                    window,
                    cx,
                )
                .into_any_element(),
            );
        }
        None
    }

    pub(crate) fn commands_pane(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = tokens(cx);
        let mut out = settings_groups().child(lead(
            "cmds-lead",
            vec![span(
                "Palette, keybindings, and format-on-save. Off commands leave ⌘⇧P, shortcuts, and willSave.",
            )],
            cx,
        ));
        if let Some(n) = self.state_note(
            "cmds",
            self.cmds.loading,
            &self.cmds.err,
            "Loading commands…",
            window,
            cx,
        ) {
            return out.child(n).into_any_element();
        }
        let groups = command_groups(&self.cmds.rows);
        if groups.is_empty() && self.skips.is_empty() {
            return out
                .child(ctrl_text(
                    "cmds-msg",
                    vec![span("No bundle commands discovered.")],
                    window,
                    cx,
                ))
                .into_any_element();
        }
        if groups.is_empty() {
            out = out.child(ctrl_text(
                "cmds-msg",
                vec![span("No loadable bundle commands.")],
                window,
                cx,
            ));
        }
        for (cat, cmds) in groups {
            let mut list = Vec::new();
            for c in cmds {
                let mut control = ctrl().gap(px(10.));
                if !c.key_equivalent_display.is_empty() {
                    control = control.child(bundle_kbd(&c.key_equivalent_display, cx));
                }
                control = control.child(self.bundle_tog(List::Commands, &c.id, &c.label, cx));
                list.push(
                    Row::new()
                        .child(copy(&c.id, c.label.clone(), None, cx))
                        .child(control),
                );
            }
            out = out.child(
                settings_group(SharedString::from(format!("grp-cmd-{cat}")), cat.clone())
                    .child(rows(IN_GROUP, list, window, cx)),
            );
        }
        if !self.skips.is_empty() {
            let mut skips = div().flex().flex_col().gap(px(8.)).w_full();
            for (i, s) in self.skips.iter().enumerate() {
                let bg = t.coral.mix(0.08, t.pane);
                let border = t.coral.mix(0.35, t.line);
                let item = card(bg, border, 7.)
                    .py(px(8.))
                    .px(px(10.))
                    .child(
                        recur_ui::text::Text::from_spans(vec![
                            span(s.name.clone()),
                            span(" "),
                            span(s.source.clone())
                                .mono()
                                .size(10.)
                                .weight(weight::REGULAR)
                                .uppercase()
                                .tracking(0.04)
                                .color(t.faint)
                                .boxed(InlineBox {
                                    margin_left: 6.,
                                    ..Default::default()
                                }),
                        ])
                        .id(SharedString::from(format!("skip-name-{i}")))
                        .size(12.5)
                        .weight(weight::MEDIUM_PLUS)
                        .leading(1.45)
                        .color(t.text),
                    )
                    .child(
                        div().mt(px(3.)).child(
                            text(s.reason.clone())
                                .id(SharedString::from(format!("skip-reason-{i}")))
                                .size(11.5)
                                .leading(1.4)
                                .color(t.muted),
                        ),
                    );
                skips = skips.child(recur_ui::tooltip::titled(
                    SharedString::from(format!("skip-tip-{i}")),
                    s.dir.clone(),
                    item,
                ));
            }
            out = out.child(
                settings_group("grp-skips", "Skipped packs")
                    .blurb_spans(vec![
                        span(
                            "These directories were found but not loaded. Fix the manifest or run ",
                        ),
                        recur_ui::widgets::blurb_code("recur bundles lint"),
                        span("."),
                    ])
                    .child(skips),
            );
        }
        out.into_any_element()
    }

    pub(crate) fn prompts_pane(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut out = settings_groups().child(lead(
            "prompts-lead",
            vec![span(
                "Chat slash verbs. Off prompts leave the composer / menu and titlebar actions that invoke them.",
            )],
            cx,
        ));
        if let Some(n) = self.state_note(
            "prompts",
            self.prompts.loading,
            &self.prompts.err,
            "Loading prompt actions…",
            window,
            cx,
        ) {
            return out.child(n).into_any_element();
        }
        let groups = prompt_groups(&self.prompts.rows);
        if groups.is_empty() {
            return out
                .child(ctrl_text(
                    "prompts-msg",
                    vec![span("No prompt actions discovered.")],
                    window,
                    cx,
                ))
                .into_any_element();
        }
        for (bundle, items) in groups {
            let mut list = Vec::new();
            for p in items {
                let d = if !p.description.is_empty() {
                    Some(p.description.clone())
                } else if !p.name.is_empty() {
                    Some(p.name.clone())
                } else {
                    None
                };
                let copy = skill_copy(
                    &format!("prompt-{}", p.id),
                    format!("/{}", p.slash),
                    d,
                    None,
                    cx,
                );
                let toggle = self.bundle_tog(List::Prompts, &p.id, &format!("/{}", p.slash), cx);
                list.push((
                    copy,
                    vec![
                        div()
                            .flex_none()
                            .mt(px(2.))
                            .child(toggle)
                            .into_any_element(),
                    ],
                ));
            }
            out = out.child(
                settings_group(
                    SharedString::from(format!("grp-prompt-{bundle}")),
                    bundle.clone(),
                )
                .child(skill_list(list, cx)),
            );
        }
        out.into_any_element()
    }

    /// The search pill over a bundle list.
    fn list_search(
        &mut self,
        key: &str,
        placeholder: &str,
        label: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let input = self.field(key, FieldOpts::default(), window, cx);
        div()
            .debug_selector(move || format!("inp:{label}"))
            .child(search_box(
                &input,
                &placeholder.to_string().into(),
                window,
                cx,
            ))
            .into_any_element()
    }

    pub(crate) fn grammars_pane(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut out = settings_groups().child(lead(
            "grammars-lead",
            vec![span(
                "Syntax highlighting. Off grammars are not used for those file types.",
            )],
            cx,
        ));
        if let Some(n) = self.state_note(
            "grammars",
            self.grammars.loading,
            &self.grammars.err,
            "Loading grammars…",
            window,
            cx,
        ) {
            return out.child(n).into_any_element();
        }
        if self.grammars.rows.is_empty() {
            return out
                .child(ctrl_text(
                    "grammars-msg",
                    vec![span("No grammars discovered.")],
                    window,
                    cx,
                ))
                .into_any_element();
        }
        out = out.child(self.list_search(
            "grammar-q",
            "Search grammars",
            "Search grammars",
            window,
            cx,
        ));
        let q = self.field_value("grammar-q", cx);
        let groups = grammar_groups(&self.grammars.rows, &q);
        if groups.is_empty() {
            return out
                .child(ctrl_text(
                    "grammars-msg",
                    vec![span("No grammars match.")],
                    window,
                    cx,
                ))
                .into_any_element();
        }
        for (bundle, items) in groups {
            let mut list = Vec::new();
            for g in items {
                let title = grammar_title(&g);
                let blurb = g.file_types.join(", ");
                list.push(
                    Row::new()
                        .child(copy(
                            &g.id,
                            title.clone(),
                            (!blurb.is_empty()).then(|| vec![span(blurb)]),
                            cx,
                        ))
                        .child(ctrl().child(self.bundle_tog(List::Grammars, &g.id, &title, cx))),
                );
            }
            out = out.child(
                settings_group(
                    SharedString::from(format!("grp-grammar-{bundle}")),
                    bundle.clone(),
                )
                .child(rows(IN_GROUP, list, window, cx)),
            );
        }
        out.into_any_element()
    }

    pub(crate) fn themes_pane(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut out = settings_groups().child(lead(
            "themes-lead",
            vec![span(
                "Editor colour themes. Off themes leave Settings → Appearance and the View menu.",
            )],
            cx,
        ));
        if let Some(n) = self.state_note(
            "themes",
            self.themes.loading,
            &self.themes.err,
            "Loading themes…",
            window,
            cx,
        ) {
            return out.child(n).into_any_element();
        }
        if self.themes.rows.is_empty() {
            return out
                .child(ctrl_text(
                    "themes-msg",
                    vec![span("No themes discovered.")],
                    window,
                    cx,
                ))
                .into_any_element();
        }
        out = out.child(self.list_search(
            "theme-q",
            "Search themes",
            "Search bundle themes",
            window,
            cx,
        ));
        let q = self.field_value("theme-q", cx);
        let groups = theme_groups(&self.themes.rows, &q);
        if groups.is_empty() {
            return out
                .child(ctrl_text(
                    "themes-msg",
                    vec![span("No themes match.")],
                    window,
                    cx,
                ))
                .into_any_element();
        }
        for (bundle, items) in groups {
            let mut list = Vec::new();
            for th in items {
                let kind = if th.ui_theme == "light" {
                    "Light"
                } else {
                    "Dark"
                };
                list.push(
                    Row::new()
                        .child(copy(&th.id, th.name.clone(), Some(vec![span(kind)]), cx))
                        .child(ctrl().child(self.bundle_tog(List::Themes, &th.id, &th.name, cx))),
                );
            }
            out = out.child(
                settings_group(
                    SharedString::from(format!("grp-theme-{bundle}")),
                    bundle.clone(),
                )
                .child(rows(IN_GROUP, list, window, cx)),
            );
        }
        out.into_any_element()
    }

    pub(crate) fn snippets_pane(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut out = settings_groups().child(lead(
            "snippets-lead",
            vec![span(
                "Tab triggers and snippet keybindings. Off snippets do not expand or appear in autocomplete.",
            )],
            cx,
        ));
        if let Some(n) = self.state_note(
            "snippets",
            self.snippets.loading,
            &self.snippets.err,
            "Loading snippets…",
            window,
            cx,
        ) {
            return out.child(n).into_any_element();
        }
        if self.snippets.rows.is_empty() {
            return out
                .child(ctrl_text(
                    "snippets-msg",
                    vec![span("No snippets discovered.")],
                    window,
                    cx,
                ))
                .into_any_element();
        }
        out = out.child(self.list_search(
            "snippet-q",
            "Search snippets",
            "Search snippets",
            window,
            cx,
        ));
        let q = self.field_value("snippet-q", cx);
        let groups = snippet_groups(&self.snippets.rows, &q);
        if groups.is_empty() {
            return out
                .child(ctrl_text(
                    "snippets-msg",
                    vec![span("No snippets match.")],
                    window,
                    cx,
                ))
                .into_any_element();
        }
        for (bundle, items) in groups {
            let mut list = Vec::new();
            for s in items {
                let blurb = snippet_blurb(&s);
                let mut control = ctrl().gap(px(10.));
                if let Some(k) = s.key_equivalent_display.as_ref().filter(|k| !k.is_empty()) {
                    control = control.child(bundle_kbd(k, cx));
                }
                control = control.child(self.bundle_tog(List::Snippets, &s.id, &s.label, cx));
                list.push(
                    Row::new()
                        .child(copy(
                            &s.id,
                            s.label.clone(),
                            (!blurb.is_empty()).then(|| vec![span(blurb)]),
                            cx,
                        ))
                        .child(control),
                );
            }
            out = out.child(
                settings_group(
                    SharedString::from(format!("grp-snippet-{bundle}")),
                    bundle.clone(),
                )
                .child(rows(IN_GROUP, list, window, cx)),
            );
        }
        out.into_any_element()
    }

    pub(crate) fn lsp_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let out = settings_groups().child(lead(
            "lsp-lead",
            vec![span(
                "Language servers for the Observatory editor. Managed servers download into Recur’s data dir on first use; others need the binary on PATH.",
            )],
            cx,
        ));
        if let Some(n) = self.state_note(
            "lsp",
            self.lsp.loading,
            &self.lsp.err,
            "Loading language servers…",
            window,
            cx,
        ) {
            return out.child(n).into_any_element();
        }
        if self.lsp.rows.is_empty() {
            return out
                .child(ctrl_text(
                    "lsp-msg",
                    vec![span("No language servers discovered.")],
                    window,
                    cx,
                ))
                .into_any_element();
        }
        let mut list = Vec::new();
        for ls in self.lsp.rows.clone() {
            let missing = !ls.available && ls.managed.is_empty();
            let mut d = ls.languages.clone().unwrap_or_default().join(", ");
            if !ls.managed.is_empty() {
                d.push_str(&format!(" · managed ({})", ls.managed));
            }
            if !ls.bundle_name.is_empty() {
                d.push_str(&format!(" · {}", ls.bundle_name));
            }
            let title_text = crate::style::title(ls.id.clone(), cx)
                .id(SharedString::from(format!("lsp-title-{}", ls.id)));
            let title_text = if missing {
                lsp_title_with_badge(title_text, &ls.id, window, cx).into_any_element()
            } else {
                title_text.into_any_element()
            };
            list.push(
                Row::new()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(title_text)
                            .child(crate::style::desc(
                                SharedString::from(format!("lsp-desc-{}", ls.id)),
                                vec![span(d)],
                                cx,
                            )),
                    )
                    .child(ctrl().child(self.bundle_tog(List::Lsp, &ls.id, &ls.id, cx))),
            );
        }
        out.child(
            settings_group("grp-lsp", "Language servers").child(rows(IN_GROUP, list, window, cx)),
        )
        .into_any_element()
    }

    // ---- loaders ----

    pub(crate) fn load_commands(&mut self, g: u64, cx: &mut Context<Self>) {
        self.cmds.loading = true;
        self.cmds.err.clear();
        let all = self
            .vcx
            .state
            .update(cx, |s, cx| s.list_all_commands_unfiltered(cx));
        let services = self.vcx.services.clone();
        cx.spawn(async move |this, cx| {
            let cmds = all.await;
            let skips = services
                .bundle
                .list_skipped_bundles()
                .await
                .unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                if this.fx.live("section", g) {
                    this.cmds.rows = cmds;
                    this.skips = skips;
                    this.cmds.loading = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(crate) fn load_prompts(&mut self, g: u64, cx: &mut Context<Self>) {
        self.prompts.loading = true;
        self.prompts.err.clear();
        let epoch = self.vcx.state.read(cx).bundles_epoch;
        let load =
            recur_view_chat::prompt_actions::PromptCatalog::load(&self.vcx.services, epoch, cx);
        cx.spawn(async move |this, cx| {
            let list = load.await;
            let _ = this.update(cx, |this, cx| {
                if this.fx.live("section", g) {
                    this.prompts.rows = (*list).clone();
                    this.prompts.loading = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(crate) fn load_grammars(&mut self, g: u64, cx: &mut Context<Self>) {
        self.grammars.loading = true;
        self.grammars.err.clear();
        let services = self.vcx.services.clone();
        cx.spawn(async move |this, cx| {
            let res = services.bundle.list_grammars().await;
            let _ = this.update(cx, |this, cx| {
                if this.fx.live("section", g) {
                    match res {
                        Ok(l) => this.grammars.rows = l,
                        Err(e) => {
                            this.grammars.rows.clear();
                            this.grammars.err = e.message.clone();
                        }
                    }
                    this.grammars.loading = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(crate) fn load_themes(&mut self, g: u64, cx: &mut Context<Self>) {
        self.themes.loading = true;
        self.themes.err.clear();
        let services = self.vcx.services.clone();
        cx.spawn(async move |this, cx| {
            let res = services.bundle.list_themes().await;
            let _ = this.update(cx, |this, cx| {
                if this.fx.live("section", g) {
                    match res {
                        Ok(l) => this.themes.rows = l,
                        Err(e) => {
                            this.themes.rows.clear();
                            this.themes.err = e.message.clone();
                        }
                    }
                    this.themes.loading = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(crate) fn load_snippets(&mut self, g: u64, cx: &mut Context<Self>) {
        self.snippets.loading = true;
        self.snippets.err.clear();
        let services = self.vcx.services.clone();
        cx.spawn(async move |this, cx| {
            let list: Vec<recur_editor::snippet::RegisteredSnippet> = services
                .bundle
                .list_snippets("", "")
                .await
                .map(|l| {
                    let mut out: Vec<_> = l
                        .iter()
                        .map(|s| {
                            recur_editor::snippet::RegisteredSnippet::from_info(
                                &recur_editor::snippet::SnippetInfo {
                                    id: s.id.clone(),
                                    name: s.name.clone(),
                                    tab_trigger: s.tab_trigger.clone(),
                                    scope: s.scope.clone(),
                                    file_glob: s.file_glob.clone(),
                                    body: s.body.clone(),
                                    order: s.order,
                                    bundle_name: s.bundle_name.clone(),
                                    key_equivalent: s.key_equivalent.clone(),
                                },
                            )
                        })
                        .collect();
                    recur_editor::snippet::sort_snippets(&mut out);
                    out
                })
                .unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                if this.fx.live("section", g) {
                    this.snippets.rows = list;
                    this.snippets.loading = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(crate) fn load_lsp(&mut self, g: u64, cx: &mut Context<Self>) {
        self.lsp.loading = true;
        self.lsp.err.clear();
        let services = self.vcx.services.clone();
        cx.spawn(async move |this, cx| {
            let res = services.lsp.list_language_servers().await;
            let _ = this.update(cx, |this, cx| {
                if this.fx.live("section", g) {
                    match res {
                        Ok(l) => this.lsp.rows = l,
                        Err(e) => {
                            this.lsp.rows.clear();
                            this.lsp.err = e.message.clone();
                        }
                    }
                    this.lsp.loading = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

/// `<Kbd class="bundle-cmd-kbd">`: Inter 10px on `--pane-2`, 1px `--line`, radius 4, padding
/// 1px 6px, line-height 16px, `--muted`.
fn bundle_kbd(label: &str, cx: &App) -> impl IntoElement {
    let t = tokens(cx);
    let spans: Vec<recur_ui::text::Span> = recur_ui::kbd::shortcut_parts(label)
        .into_iter()
        .map(|p| span(p.text))
        .collect();
    card(t.pane_2, t.line, 4.)
        .flex_none()
        .py(px(1.))
        .px(px(6.))
        .child(
            recur_ui::text::Text::from_spans(spans)
                .size(10.)
                .line_height(LineHeight::Px(16.))
                .color(t.muted)
                .nowrap(),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(id: &str, label: &str, category: &str, order: i64) -> RegisteredCommand {
        RegisteredCommand {
            id: id.into(),
            label: label.into(),
            category: category.into(),
            order,
            scope: String::new(),
            file_glob: String::new(),
            triggers: vec![],
            prompts: vec![],
            input: String::new(),
            fallback_input: String::new(),
            output: String::new(),
            interpreter: String::new(),
            bundle_name: String::new(),
            key_equivalent_raw: String::new(),
            key_equivalent: String::new(),
            key_equivalent_display: String::new(),
            language: String::new(),
            open_in_preview: false,
        }
    }

    #[test]
    fn command_groups_order_by_lowest_order_then_name() {
        let groups = command_groups(&[
            cmd("a", "Zed", "Text", 20),
            cmd("b", "Alpha", "Go", 30),
            cmd("c", "Beta", "Text", 5),
            cmd("d", "Gamma", "", 50),
        ]);
        let names: Vec<&str> = groups.iter().map(|g| g.0.as_str()).collect();
        assert_eq!(names, ["Text", "Go", "Commands"]);
        let text: Vec<&str> = groups[0].1.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(text, ["Beta", "Zed"]);
    }

    #[test]
    fn grammar_and_theme_groups() {
        let g = |id: &str, lang: &str, bundle: &str, ft: &[&str]| GrammarInfo {
            id: id.into(),
            language: lang.into(),
            bundle_name: bundle.into(),
            file_types: ft.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        let list = vec![
            g("recur.languages.go", "go", "Languages", &["go"]),
            g(
                "recur.languages.ts",
                "TypeScript",
                "Languages",
                &["ts", "tsx"],
            ),
            g("x.toml", "", "Extras", &["toml"]),
        ];
        let groups = grammar_groups(&list, "");
        assert_eq!(groups[0].0, "Extras");
        assert_eq!(grammar_title(&groups[0].1[0]), "x.toml");
        let ts = grammar_groups(&list, " TSX ");
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].1[0].id, "recur.languages.ts");

        let th = |id: &str, name: &str, order: i64| ThemeInfo {
            id: id.into(),
            name: name.into(),
            order,
            ..Default::default()
        };
        let themes = theme_groups(
            &[
                th("z", "Zebra", 0),
                th("b", "Beta", 20),
                th("a", "Alpha", 10),
                th("y", "Ash", 0),
            ],
            "",
        );
        let ids: Vec<&str> = themes[0].1.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(themes[0].0, "Themes");
        assert_eq!(ids, ["a", "b", "y", "z"]);
    }

    #[test]
    fn snippet_blurbs_and_prompt_groups() {
        let s = recur_editor::snippet::RegisteredSnippet {
            id: "recur.go.ife".into(),
            label: "If Err".into(),
            tab_trigger: "ife".into(),
            key_equivalent_display: Some("⌃⌥E".into()),
            ..Default::default()
        };
        assert_eq!(snippet_blurb(&s), "ife · ⌃⌥E");
        let p = |id: &str, slash: &str, bundle: &str| recur_services::api::PromptActionInfo {
            id: id.into(),
            slash: slash.into(),
            bundle_name: bundle.into(),
            ..Default::default()
        };
        let groups = prompt_groups(&[
            p("2", "review", "Chat"),
            p("1", "commit", "Chat"),
            p("3", "x", ""),
        ]);
        assert_eq!(groups[0].0, "Chat");
        assert_eq!(groups[0].1[0].slash, "commit");
        assert_eq!(groups[1].0, "Prompts");
    }
}

/// A language server title with `.set-lsp-miss`: an inline-block after a space and 8px, raised
/// 2px (`vertical-align: 2px`), its own 10px bold line at the inherited 1.45, padded 1px 6px.
fn lsp_title_with_badge(
    title: recur_ui::text::Text,
    id: &str,
    window: &Window,
    cx: &App,
) -> gpui_kit::Div {
    use recur_ui::fonts::Family;
    use recur_ui::text::strut_metrics;
    let t = tokens(cx);
    let measure = |spans: Vec<recur_ui::text::Span>| {
        recur_ui::text::Text::from_spans(spans)
            .size(13.)
            .weight(weight::SEMIBOLD)
            .leading(1.45)
            .nowrap()
            .measure(None, window)
            .0
    };
    let w = measure(vec![span(id.to_string()), span(" "), span("x")]) - measure(vec![span("x")]);
    let line = strut_metrics(&Family::Ui, 13., LineHeight::Scale(1.45), window);
    let badge_line = strut_metrics(&Family::Ui, 10., LineHeight::Scale(1.45), window);
    let top = line.baseline - 2. - (1. + badge_line.baseline);
    let badge = div()
        .flex_none()
        .py(px(1.))
        .px(px(6.))
        .rounded(px(4.))
        .bg(hsla(t.amber.fade(0.16)))
        .child(
            text("Not installed")
                .size(10.)
                .weight(weight::BOLD)
                .tracking(0.02)
                .leading(1.45)
                .color(t.amber)
                .nowrap(),
        );
    div().relative().child(title).child(
        div()
            .absolute()
            .top_0()
            .left_0()
            .flex()
            .flex_col()
            .child(recur_ui::exact::vgap(top.max(0.), window))
            .child(
                div()
                    .flex()
                    .child(recur_ui::exact::hgap(w + 8., window))
                    .child(badge),
            ),
    )
}
