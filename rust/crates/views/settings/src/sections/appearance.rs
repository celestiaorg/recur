//! Appearance: the app chrome, code editor and terminal themes, each with Sync with system.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, Bounds, Context, Div, Entity, InteractiveElement, IntoElement, ParentElement,
    Pixels, SharedString, StatefulInteractiveElement, Styled, Window, canvas, div, point, px,
    relative,
};
use recur_editor::theme::catalog::{EditorThemeCard, filter_for_list};
use recur_services::api::TerminalThemeInfo;
use recur_ui::fonts::{Family, weight};
use recur_ui::motion::{self, Easing};
use recur_ui::style::{card, text_field};
use recur_ui::text::{Align, LineHeight, TextStyled, span, text};
use recur_ui::theme::{Swatches, ThemeId};
use recur_ui::widgets::{settings_group, settings_groups};
use recur_ui::{Color, IconName, icon};
use recur_view_core::AppState;
use recur_view_core::prefs::TERMINAL_MATCH_EDITOR;
use recur_view_core::themes::is_light_theme;

use crate::field::FieldOpts;
use crate::style::{copy, css_color, ctrl, hsla, seg, seg_btn, swatch, thauto_label, tokens};
use crate::view::{SettingsView, ThemeKind};

/// What a theme card shows: background, three dots, and the `.tha` sample (text and accent).
struct CardLook {
    bg: Color,
    dots: [Color; 3],
    sample: (&'static str, Color, &'static str, Color),
    name: String,
}

/// Which pick a grid changes.
type PickTheme = Rc<dyn Fn(&EditorThemeCard, &mut App)>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pick {
    App(Option<bool>),
    Terminal(Option<bool>),
}

impl SettingsView {
    /// `themeModeRow`: the bare `.set-row` with Single theme / Sync with system.
    fn theme_mode_row(
        &mut self,
        id: &'static str,
        auto: bool,
        set: fn(&mut AppState, bool, &mut gpui_kit::Context<AppState>),
        cx: &mut Context<Self>,
    ) -> Div {
        let t = tokens(cx);
        let mut s = seg(cx);
        for (on, label, val) in [
            (!auto, "Single theme", false),
            (auto, "Sync with system", true),
        ] {
            let state = self.vcx.state.clone();
            s = s.child(
                seg_btn(
                    SharedString::from(format!("{id}-{val}")),
                    label,
                    on,
                    false,
                    11.,
                    cx,
                )
                .debug_selector(move || format!("seg:{id}:{label}{}", if on { ":on" } else { "" }))
                .on_click(move |_, _, cx| state.update(cx, |s, cx| set(s, val, cx))),
            );
        }
        div()
            .flex()
            .items_center()
            .gap(px(16.))
            .py(px(16.))
            .border_b(px(1.))
            .border_color(hsla(t.line_soft))
            .child(copy(id, "Theme mode", None, cx))
            .child(ctrl().child(s))
    }

    /// `.thcard`: preview, name and check; `.on` rings it amber.
    fn theme_card(
        &mut self,
        id: SharedString,
        look: CardLook,
        on: bool,
        pick: impl Fn(&mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = tokens(cx);
        let hover = motion::hover(
            SharedString::from(format!("{id}-hover")),
            Duration::from_millis(120),
            Easing::EASE,
            window,
            cx,
        );
        let border = if on {
            t.amber
        } else {
            t.line_bright.mix(hover.k(), t.line)
        };
        let (s1, c1, s2, c2) = look.sample;
        let sel = id.clone();
        let mut dots = div().flex().gap(px(5.));
        for d in look.dots {
            dots = dots.child(div().size(px(11.)).rounded_full().bg(hsla(d)));
        }
        let prev = div()
            .h(px(60.))
            .py(px(10.))
            .px(px(11.))
            .flex()
            .flex_col()
            .justify_between()
            .rounded_t(px(8.))
            .bg(hsla(look.bg))
            .child(dots)
            .child(
                recur_ui::text::Text::from_spans(vec![span(s1).color(c1), span(s2).color(c2)])
                    .mono()
                    .size(13.)
                    .weight(weight::SEMIBOLD)
                    .tracking(-0.02)
                    .line_height(LineHeight::Normal)
                    .align(Align::Center)
                    .nowrap(),
            );
        let meta = div()
            .py(px(9.))
            .px(px(11.))
            .border_t(px(1.))
            .border_color(hsla(t.line))
            .flex()
            .items_center()
            .gap(px(7.))
            .child(
                text(look.name)
                    .size(12.5)
                    .weight(weight::SEMIBOLD)
                    .line_height(LineHeight::Normal)
                    .color(if on { t.amber } else { t.text })
                    .nowrap(),
            )
            .child(
                div()
                    .ml_auto()
                    .flex_none()
                    .opacity(if on { 1. } else { 0. })
                    .child(
                        icon(IconName::Check)
                            .stroke_width(2.6)
                            .size(px(15.))
                            .color(t.amber),
                    ),
            );
        let mut c = card(t.pane, border, 9.)
            .id(id)
            .debug_selector(move || format!("thcard:{sel}{}", if on { ":on" } else { "" }))
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .cursor_pointer()
            .on_hover(hover.listener())
            .on_click(move |_, window, cx| pick(window, cx))
            .child(prev)
            .child(meta);
        if on {
            // The amber border and `box-shadow: 0 0 0 1px` read as one 2px ring; drawn as one
            // shape, two anti-aliased rings would leave a seam between them.
            c = c.child(
                div()
                    .absolute()
                    .top(px(-2.))
                    .left(px(-2.))
                    .right(px(-2.))
                    .bottom(px(-2.))
                    .border(px(2.))
                    .border_color(hsla(t.amber))
                    .rounded(px(10.)),
            );
        }
        c.into_any_element()
    }

    /// `.thgrid`: three equal columns, 11px gaps, `padding: 8px 0 6px` in a group body.
    fn grid(cards: Vec<AnyElement>) -> Div {
        let mut g = div().flex().flex_col().gap(px(11.)).pt(px(8.)).pb(px(6.));
        let mut it = cards.into_iter().peekable();
        while it.peek().is_some() {
            let mut row = div().flex().gap(px(11.));
            for _ in 0..3 {
                // Grid tracks share the row exactly; a bordered card's flex basis would not.
                row = row.child(recur_ui::style::flex_share(
                    1.,
                    match it.next() {
                        Some(c) => c,
                        None => div().flex_1().min_w_0().into_any_element(),
                    },
                ));
            }
            g = g.child(row);
        }
        g
    }

    fn app_look(id: ThemeId, sample_app: bool) -> CardLook {
        let s: Swatches = id.swatches();
        let sample = if sample_app {
            ("recur", s.text, "_", s.accent)
        } else {
            ("ls", s.text, "~", s.ok)
        };
        CardLook {
            bg: s.bg,
            dots: [s.accent, s.ok, s.warn],
            sample,
            name: id.name().into(),
        }
    }

    /// `themeGrid`.
    fn app_grid(
        &mut self,
        cur: &str,
        mode: Option<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut cards = Vec::new();
        for id in ThemeId::ALL {
            if let Some(light) = mode
                && is_light_theme(id.key()) != light
            {
                continue;
            }
            let state = self.vcx.state.clone();
            let key = id.key();
            let grid_id = match mode {
                None => "app",
                Some(true) => "app-light",
                Some(false) => "app-dark",
            };
            let pick = Pick::App(mode);
            cards.push(self.theme_card(
                SharedString::from(format!("{grid_id}-{key}")),
                Self::app_look(id, true),
                cur == key,
                move |_, cx| {
                    state.update(cx, |s, cx| match pick {
                        Pick::App(None) => s.set_theme(key, cx),
                        Pick::App(Some(light)) => s.set_system_theme(!light, key, cx),
                        Pick::Terminal(_) => {}
                    })
                },
                window,
                cx,
            ));
        }
        Self::grid(cards)
    }

    /// `terminalGrid`: Match app (the chrome palette `app`), then the bundle terminal themes.
    fn terminal_grid(
        &mut self,
        cur: &str,
        app: ThemeId,
        mode: Option<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let grid_id = match mode {
            None => "term",
            Some(true) => "term-light",
            Some(false) => "term-dark",
        };
        let pick = Pick::Terminal(mode);
        let apply = move |state: &Entity<AppState>, id: String, cx: &mut App| {
            state.update(cx, |s, cx| match pick {
                Pick::Terminal(None) => s.set_terminal_theme(&id, cx),
                Pick::Terminal(Some(light)) => s.set_system_terminal_theme(!light, &id, cx),
                Pick::App(_) => {}
            })
        };
        let mut cards = Vec::new();
        let mut look = Self::app_look(app, false);
        look.name = "Match app".into();
        let state = self.vcx.state.clone();
        cards.push(self.theme_card(
            SharedString::from(format!("{grid_id}-match")),
            look,
            cur == TERMINAL_MATCH_EDITOR,
            move |_, cx| apply(&state, TERMINAL_MATCH_EDITOR.into(), cx),
            window,
            cx,
        ));
        let catalog: Vec<TerminalThemeInfo> = self
            .vcx
            .state
            .read(cx)
            .terminal_theme_catalog
            .iter()
            .filter(|t| mode.is_none_or(|light| (t.ui_theme == "light") == light))
            .cloned()
            .collect();
        for th in catalog {
            let p = &th.palette;
            let fg = css_color(&p.foreground, tokens(cx).text);
            let or_fg = |s: &str| if s.is_empty() { fg } else { css_color(s, fg) };
            let green = or_fg(&p.green);
            let look = CardLook {
                bg: css_color(&p.background, tokens(cx).bg),
                dots: [or_fg(&p.red), green, or_fg(&p.blue)],
                sample: ("ls", fg, "~", green),
                name: th.name.clone(),
            };
            let state = self.vcx.state.clone();
            let id = th.id.clone();
            cards.push(self.theme_card(
                SharedString::from(format!("{grid_id}-{}", th.id)),
                look,
                cur == th.id,
                move |_, cx| apply(&state, id.clone(), cx),
                window,
                cx,
            ));
        }
        Self::grid(cards)
    }

    /// `editorThemeRows`: the list with its Recur / All themes labels, or the empty note.
    fn editor_rows(
        &mut self,
        list_id: &'static str,
        list: Vec<EditorThemeCard>,
        current: &str,
        pick: PickTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = tokens(cx);
        if list.is_empty() {
            return div()
                .py(px(18.))
                .px(px(4.))
                .child(
                    text("No themes match.")
                        .size(12.5)
                        .leading(1.45)
                        .color(t.muted),
                )
                .into_any_element();
        }
        let vh = f32::from(window.viewport_size().height);
        let max = 420f32.min(recur_ui::exact::lu(0.55 * vh));
        let reveal = self.edth_scroll_pending;
        let mut rows: Vec<AnyElement> = Vec::new();
        for (i, th) in list.iter().enumerate() {
            let prev_pinned = i > 0 && list[i - 1].pinned;
            let label = if th.pinned && i == 0 {
                Some("Recur")
            } else if !th.pinned && (i == 0 || prev_pinned) {
                Some("All themes")
            } else {
                None
            };
            if let Some(l) = label {
                rows.push(
                    div()
                        .when(i == 0, |d| d.rounded_t(px(8.)))
                        .pt(px(8.))
                        .px(px(12.))
                        .pb(px(4.))
                        .bg(hsla(t.bg))
                        .child(
                            text(l)
                                .mono()
                                .size(10.)
                                .leading(1.45)
                                .tracking(0.06)
                                .uppercase()
                                .color(t.muted),
                        )
                        .into_any_element(),
                );
            }
            let on = current == th.key;
            let bg_mix = if on { 0.14 } else { 0.08 };
            let hover_bg = t.amber.fade(0.08);
            let dark = th.dark;
            let name = th.name.clone();
            let card = th.clone();
            let pick = pick.clone();
            let mut row = div()
                .id(SharedString::from(format!("{list_id}-{}", th.key)))
                .debug_selector(move || format!("edth:{name}{}", if on { ":on" } else { "" }))
                .flex()
                .items_center()
                .gap(px(10.))
                .w_full()
                .py(px(8.))
                .px(px(12.))
                .when(label.is_none(), |d| {
                    d.border_t(px(1.)).border_color(hsla(t.line_soft))
                })
                .cursor_pointer()
                .when(on, |d| d.bg(hsla(t.amber.fade(bg_mix))))
                .when(!on, |d| d.hover(move |s| s.bg(hsla(hover_bg))))
                .on_click(move |_, _, cx| pick(&card, cx))
                .child(swatch(
                    css_color(&th.bg, recur_ui::hex(0x1e1e1e)),
                    [
                        css_color(&th.b, t.text),
                        css_color(&th.a, t.text),
                        css_color(&th.c, t.text),
                    ],
                    cx,
                ))
                .child(
                    div().flex_1().min_w_0().child(
                        text(th.name.clone())
                            .size(12.5)
                            .weight(if on { weight::SEMIBOLD } else { weight::MEDIUM })
                            .line_height(LineHeight::Normal)
                            .color(if on { t.amber } else { t.text })
                            .truncate(),
                    ),
                )
                .child(
                    div().flex_none().child(
                        text(if dark { "dark" } else { "light" })
                            .mono()
                            .size(10.)
                            .line_height(LineHeight::Normal)
                            .tracking(0.05)
                            .uppercase()
                            .color(t.muted),
                    ),
                )
                .child(
                    div().flex_none().opacity(if on { 1. } else { 0. }).child(
                        icon(IconName::Check)
                            .stroke_width(2.6)
                            .size(px(14.))
                            .color(t.amber),
                    ),
                );
            if on && reveal {
                row = row.relative().child(self.reveal_probe(list_id));
            }
            rows.push(row.into_any_element());
        }
        card(t.pane, t.line, 9.)
            .flex()
            .flex_col()
            .max_h(px(max))
            .overflow_hidden()
            .child(
                recur_ui::widgets::scroll_area(SharedString::from(format!("{list_id}-list")))
                    .track(&self.edth_scroll(list_id))
                    .child(
                        div()
                            .debug_selector(move || format!("edth-list:{list_id}"))
                            .rounded(px(8.))
                            .flex()
                            .flex_col()
                            .children(rows),
                    ),
            )
            .into_any_element()
    }

    fn edth_scroll(&mut self, list_id: &'static str) -> gpui_kit::ScrollHandle {
        self.edth_scrolls.entry(list_id).or_default().clone()
    }

    /// `scrollSelectedEditorThemeIntoView`: once laid out, scroll the list and then the page
    /// the least that shows the selected row (`scrollIntoView({ block: 'nearest' })`).
    fn reveal_probe(&mut self, list_id: &'static str) -> impl IntoElement {
        let list = self.edth_scroll(list_id);
        let page = self.main_scroll.clone();
        let done = Rc::new(Cell::new(false));
        canvas(
            move |bounds: Bounds<Pixels>, window, _| {
                if done.get() {
                    return;
                }
                done.set(true);
                let near = |port: Bounds<Pixels>, row: Bounds<Pixels>| -> f32 {
                    if row.bottom() > port.bottom() && row.size.height <= port.size.height {
                        f32::from(row.bottom() - port.bottom())
                    } else if row.top() < port.top() {
                        f32::from(row.top() - port.top())
                    } else {
                        0.
                    }
                };
                let mut row = bounds;
                for h in [&list, &page] {
                    let port = h.bounds();
                    let dy = near(port, row);
                    if dy != 0. {
                        let o = h.offset();
                        h.set_offset(point(o.x, o.y - px(dy)));
                        row.origin.y -= px(dy);
                        window.refresh();
                    }
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .w(relative(1.))
        .h(relative(1.))
    }

    pub(crate) fn appearance_pane(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = tokens(cx);
        let s = self.vcx.state.read(cx);
        let app_auto = s.appearance.sync.auto;
        let app_cur = ThemeId::from_key(&s.appearance.theme);
        let (app_light, app_dark) = (
            s.appearance.sync.light.clone(),
            s.appearance.sync.dark.clone(),
        );
        let ed_auto = s.editor_sync.auto;
        let ed_cur = s.editor_theme.clone();
        let (ed_light, ed_dark) = (s.editor_sync.light.clone(), s.editor_sync.dark.clone());
        let term_auto = s.terminal_sync.auto;
        let term_cur = s.terminal_prefs.theme.clone();
        let (term_light, term_dark) = (s.terminal_sync.light.clone(), s.terminal_sync.dark.clone());
        let cards = s.editor_themes_for_picker();

        // App theme.
        let mut app_body = div().flex().flex_col().child(self.theme_mode_row(
            "App theme mode",
            app_auto,
            AppState::set_theme_auto,
            cx,
        ));
        if app_auto {
            for (light, label, cur) in [
                (true, "Light mode", app_light),
                (false, "Dark mode", app_dark),
            ] {
                app_body = app_body.child(thauto_label(label, cx)).child(self.app_grid(
                    &cur,
                    Some(light),
                    window,
                    cx,
                ));
            }
        } else {
            app_body = app_body.child(self.app_grid(app_cur.key(), None, window, cx));
        }

        // Code editor theme.
        let query_input = self.field("edth-search", FieldOpts::default(), window, cx);
        let query = self.field_value("edth-search", cx);
        let kind = match self.theme_kind {
            ThemeKind::All => None,
            ThemeKind::Dark => Some(false),
            ThemeKind::Light => Some(true),
        };
        let selected = cards.iter().find(|c| c.key == ed_cur).cloned();
        let mut ed_body = div().flex().flex_col().child(self.theme_mode_row(
            "Code editor theme mode",
            ed_auto,
            AppState::set_editor_theme_auto,
            cx,
        ));
        let has_current = !ed_auto && selected.is_some();
        if let Some(sel) = selected.filter(|_| !ed_auto) {
            ed_body = ed_body.child(self.editor_current(&sel, window, cx));
        }
        let focused = self
            .fields
            .get("edth-search")
            .is_some_and(|f| f.is_focused(window, cx));
        let search = card(
            t.bg,
            if focused { t.line_bright } else { t.line },
            if focused { 4. } else { 7. },
        )
        .debug_selector(|| "inp:Search editor themes".into())
        .flex_1()
        .min_w(px(160.))
        .h(px(30.))
        .px(px(10.))
        .flex()
        .items_center()
        .child(text_field(
            &query_input,
            &"Search themes…".into(),
            recur_ui::style::UA_PLACEHOLDER,
            Family::Ui,
            12.5,
            window,
            cx,
        ));
        let mut toolbar = div()
            .mt(px(if has_current { 10. } else { 8. }))
            .mb(px(10.))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(10.))
            .child(search);
        if !ed_auto {
            let mut sg = seg(cx);
            for (k, label) in [
                (ThemeKind::All, "All"),
                (ThemeKind::Dark, "Dark"),
                (ThemeKind::Light, "Light"),
            ] {
                let on = self.theme_kind == k;
                sg = sg.child(
                    seg_btn(
                        SharedString::from(format!("edkind-{label}")),
                        label,
                        on,
                        false,
                        11.,
                        cx,
                    )
                    .debug_selector(move || {
                        format!("seg:Filter by kind:{label}{}", if on { ":on" } else { "" })
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.theme_kind = k;
                        cx.notify();
                    })),
                );
            }
            toolbar = toolbar.child(sg);
        }
        ed_body = ed_body.child(toolbar);
        let state = self.vcx.state.clone();
        if ed_auto {
            for (light, label, cur) in [
                (true, "Light mode", ed_light),
                (false, "Dark mode", ed_dark),
            ] {
                let st = state.clone();
                let list = filter_for_list(&cards, &query, Some(!light));
                let pick: PickTheme = Rc::new(move |c, cx| {
                    st.update(cx, |s, cx| s.set_system_editor_theme(!light, &c.key, cx))
                });
                let rows = self.editor_rows(
                    if light { "edth-light" } else { "edth-dark" },
                    list,
                    &cur,
                    pick,
                    window,
                    cx,
                );
                ed_body = ed_body.child(thauto_label(label, cx)).child(rows);
            }
        } else {
            let list = filter_for_list(&cards, &query, kind.map(|light| !light));
            let st = state.clone();
            let pick: PickTheme = Rc::new(move |c, cx| {
                st.update(cx, |s, cx| s.set_editor_theme(&c.key, cx));
                AppState::toast_editor_theme(&c.name, cx);
            });
            let rows = self.editor_rows("edth", list, &ed_cur, pick, window, cx);
            ed_body = ed_body.child(rows);
        }
        if !cards.is_empty() {
            self.edth_scroll_pending = false;
        }

        // Terminal theme.
        let app_for = |key: &str| ThemeId::from_key(key);
        let mut term_body = div().flex().flex_col().child(self.theme_mode_row(
            "Terminal theme mode",
            term_auto,
            AppState::set_terminal_theme_auto,
            cx,
        ));
        if term_auto {
            for (light, label, cur, app_key) in [
                (
                    true,
                    "Light mode",
                    term_light,
                    app_light_key(&self.vcx.state, true, cx),
                ),
                (
                    false,
                    "Dark mode",
                    term_dark,
                    app_light_key(&self.vcx.state, false, cx),
                ),
            ] {
                let app = if app_auto { app_for(&app_key) } else { app_cur };
                term_body = term_body
                    .child(thauto_label(label, cx))
                    .child(self.terminal_grid(&cur, app, Some(light), window, cx));
            }
        } else {
            term_body = term_body.child(self.terminal_grid(&term_cur, app_cur, None, window, cx));
        }

        settings_groups()
            .child(
                settings_group("grp-app-theme", "App theme")
                    .blurb("Chrome, sidebars, and panels.")
                    .child(app_body),
            )
            .child(
                settings_group("grp-editor-theme", "Code editor theme")
                    .blurb("How code looks in the editor.")
                    .child(ed_body),
            )
            .child(
                settings_group("grp-term-theme", "Terminal theme")
                    .blurb("ANSI colours in the dock. Match app follows the chrome palette.")
                    .child(term_body),
            )
            .into_any_element()
    }

    /// `.edth-current`: the selected theme; clicking it clears the filters and reveals it.
    fn editor_current(
        &mut self,
        sel: &EditorThemeCard,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = tokens(cx);
        let border = t.amber.mix(0.45, t.line);
        let bg = t.amber.mix(0.12, t.pane);
        let _ = window;
        let name = sel.name.clone();
        let c = card(bg, border, 9.)
            .id("edth-current")
            .debug_selector(move || format!("edth-current:{name}"))
            .mt(px(8.))
            .flex()
            .items_center()
            .gap(px(10.))
            .w_full()
            .py(px(9.))
            .px(px(12.))
            .cursor_pointer()
            .hover(move |s| s.border_color(hsla(t.amber)))
            .on_click(cx.listener(|this, _, _, cx| {
                this.theme_kind = ThemeKind::All;
                this.field_rewrite("edth-search", String::new());
                this.edth_scroll_pending = true;
                cx.notify();
            }))
            .child(swatch(
                css_color(&sel.bg, recur_ui::hex(0x1e1e1e)),
                [
                    css_color(&sel.b, t.text),
                    css_color(&sel.a, t.text),
                    css_color(&sel.c, t.text),
                ],
                cx,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        text("Selected")
                            .mono()
                            .size(9.5)
                            .line_height(LineHeight::Normal)
                            .tracking(0.06)
                            .uppercase()
                            .color(t.muted),
                    )
                    .child(
                        text(sel.name.clone())
                            .size(13.)
                            .weight(weight::SEMIBOLD)
                            .line_height(LineHeight::Normal)
                            .color(t.amber)
                            .truncate(),
                    ),
            )
            .child(
                div().flex_none().child(
                    text(if sel.dark { "dark" } else { "light" })
                        .mono()
                        .size(10.)
                        .line_height(LineHeight::Normal)
                        .tracking(0.05)
                        .uppercase()
                        .color(t.muted),
                ),
            )
            .child(
                div().flex_none().child(
                    icon(IconName::Check)
                        .stroke_width(2.6)
                        .size(px(14.))
                        .color(t.amber),
                ),
            );
        recur_ui::tooltip::titled("edth-current-tip", "Show in list", c).into_any_element()
    }
}

/// `appThemeFor(mode)`'s key: the light or dark pick of the app sync.
fn app_light_key(state: &Entity<AppState>, light: bool, cx: &App) -> String {
    let s = &state.read(cx).appearance.sync;
    if light {
        s.light.clone()
    } else {
        s.dark.clone()
    }
}
