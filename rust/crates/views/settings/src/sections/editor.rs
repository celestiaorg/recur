//! Code editor: the editor prefs, per-language indent, the terminal font, Observatory panel
//! sides and the file explorer's exclude globs.

use gpui_kit::{
    AnyElement, Context, Div, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use recur_editor::prefs::{
    EDITOR_FONT_SIZES, EDITOR_INDENT_LANGS, EDITOR_LINE_HEIGHTS, EDITOR_RULER_COLUMNS,
    EDITOR_TAB_SIZES, EditorIndent, EditorKeymap, EditorPrefs, EditorWhitespace,
    language_indent_default,
};
use recur_ui::IconName;
use recur_ui::text::span;
use recur_ui::widgets::{SelectOption, TriggerStyle, settings_group, settings_groups};
use recur_view_core::prefs::{CodePanelKey, PanelSide, TERMINAL_LINE_HEIGHTS};

use crate::field::{FieldEvent, FieldOpts};
use crate::mono_fonts::{font_stack, list_monospace_fonts};
use crate::rules::{int_text, number_input_value};
use crate::style::{
    InpStyle, Row, RowsCtx, copy, ctrl, inp, rm_btn, rows, seg, seg_btn, tbtn_label,
};
use crate::view::SettingsView;

const IN_GROUP: RowsCtx = RowsCtx {
    sides: true,
    in_group: true,
    more: false,
};

/// `EDITOR_INDENT_OPTIONS`: key, label, short label.
const INDENT_OPTIONS: [(EditorIndent, &str, &str); 3] = [
    (EditorIndent::Spaces2, "2 spaces", "2"),
    (EditorIndent::Spaces4, "4 spaces", "4"),
    (EditorIndent::Tabs, "Tabs", "Tabs"),
];

/// `EDITOR_LINE_HEIGHT_LABELS`.
fn line_height_label(n: f32) -> &'static str {
    match n {
        x if x == 1.4 => "Compact",
        x if x == 1.7 => "Relaxed",
        _ => "Default",
    }
}

/// `TERMINAL_LINE_HEIGHT_LABELS`.
fn terminal_line_height_label(n: f64) -> &'static str {
    match n {
        x if x == 1. => "Compact",
        x if x == 1.35 => "Relaxed",
        _ => "Default",
    }
}

/// One `.seg` choice: label, whether it is on, and what clicking does.
type Choice = (String, bool, Box<dyn Fn(&mut SettingsView, &mut Context<SettingsView>)>);

impl SettingsView {
    /// A labelled `.seg` row control (`role="group"` named `group`).
    fn seg_group(
        &self,
        group: impl Into<SharedString>,
        choices: Vec<Choice>,
        pad_x: f32,
        cx: &mut Context<Self>,
    ) -> Div {
        let group: SharedString = group.into();
        let mut s = seg(cx);
        for (i, (label, on, act)) in choices.into_iter().enumerate() {
            let l = label.clone();
            let g = group.clone();
            let act = std::rc::Rc::new(act);
            s = s.child(
                seg_btn(SharedString::from(format!("{group}-{i}")), label, on, false, pad_x, cx)
                    .debug_selector(move || format!("seg:{g}:{l}{}", if on { ":on" } else { "" }))
                    .on_click(cx.listener(move |this, _, _, cx| act(this, cx))),
            );
        }
        ctrl().child(s)
    }

    fn prefs_choice<T: PartialEq + Copy + 'static>(
        label: String,
        cur: T,
        v: T,
        set: fn(&mut EditorPrefs, T),
    ) -> Choice {
        (
            label,
            cur == v,
            Box::new(move |this, cx| this.set_editor_prefs(move |p| set(p, v), cx)),
        )
    }

    fn on_off(group: &'static str, cur: bool, set: fn(&mut EditorPrefs, bool)) -> (&'static str, Vec<Choice>) {
        (
            group,
            vec![
                Self::prefs_choice("On".into(), cur, true, set),
                Self::prefs_choice("Off".into(), cur, false, set),
            ],
        )
    }

    pub(crate) fn editor_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = self.vcx.state.read(cx).editor_prefs.clone();
        let tp = self.vcx.state.read(cx).terminal_prefs.clone();
        let scanning = if self.mono_loading { " · scanning…" } else { "" };

        // Font pickers.
        let mut opts: Vec<SelectOption> = self
            .mono_fonts
            .iter()
            .map(|f| SelectOption::new(f.family.clone(), f.label.clone()))
            .collect();
        let have_fonts = !opts.is_empty();
        if have_fonts && !p.font_face.is_empty() && !opts.iter().any(|o| o.value == p.font_face) {
            opts.insert(0, SelectOption::new(p.font_face.clone(), p.font_face.clone()));
        }
        let ed_opts: Vec<SelectOption> = opts
            .iter()
            .cloned()
            .map(|o| {
                let stack = font_stack(&o.value);
                o.font(stack, p.font_size as f32)
            })
            .collect();
        let term_face = if tp.font_face.is_empty() {
            p.font_face.clone()
        } else {
            tp.font_face.clone()
        };
        let term_size = if tp.font_size == 0 {
            p.font_size as f32
        } else {
            tp.font_size as f32
        };
        let mut term_opts = vec![SelectOption::new("", "Match editor")
            .font(recur_ui::fonts::Family::Ui, term_size)];
        term_opts.extend(opts.iter().cloned().map(|o| {
            let stack = font_stack(&o.value);
            o.font(stack, term_size)
        }));
        let backdrop = crate::style::tokens(cx).pane;
        let font_trigger = TriggerStyle {
            pad_y: Some(8.),
            pad_x: Some(12.),
            min_height: Some(38.),
            ..Default::default()
        };
        let ed_props = format!("{:?}", (&ed_opts, &p.font_face, p.font_size, have_fonts));
        if self.props_changed("font-select", ed_props) {
            let (face, size) = (p.font_face.clone(), p.font_size as f32);
            let tr = font_trigger.clone();
            self.font_select.update(cx, |s, cx| {
                s.set_options(ed_opts, cx);
                s.set_value(face.clone(), cx);
                s.set_value_font(Some((font_stack(&face), size)), cx);
                s.set_trigger_style(tr, cx);
                s.set_disabled((!have_fonts).then_some(backdrop), cx);
            });
        }
        let term_props = format!("{:?}", (&term_opts, &tp.font_face, &term_face, term_size, have_fonts));
        if self.props_changed("term-font-select", term_props) {
            let value = tp.font_face.clone();
            let tr = font_trigger.clone();
            self.term_font_select.update(cx, |s, cx| {
                s.set_options(term_opts, cx);
                s.set_value(value, cx);
                s.set_value_font(Some((font_stack(&term_face), term_size)), cx);
                s.set_trigger_style(tr, cx);
                s.set_disabled((!have_fonts).then_some(backdrop), cx);
            });
        }
        let vw = f32::from(window.viewport_size().width);
        let font_min = 280f32.min(0.7 * vw);
        let font_ctrl = |sel: gpui_kit::Entity<recur_ui::widgets::Select>| {
            ctrl().child(div().min_w(px(font_min)).child(sel))
        };

        // Editor rows, before and after the per-language group.
        let mut head: Vec<Row> = Vec::new();
        head.push(
            Row::new()
                .child(copy(
                    "font",
                    "Font",
                    Some(vec![span(format!("System monospace fonts{scanning}"))]),
                    cx,
                ))
                .child(font_ctrl(self.font_select.clone())),
        );
        let groups: Vec<(&str, Option<&str>, &'static str, Vec<Choice>)> = vec![
            (
                "Font size",
                None,
                "Editor font size",
                EDITOR_FONT_SIZES
                    .iter()
                    .map(|n| Self::prefs_choice(n.to_string(), p.font_size, *n, |p, v| p.font_size = v))
                    .collect(),
            ),
            (
                "Line height",
                None,
                "Line height",
                EDITOR_LINE_HEIGHTS
                    .iter()
                    .map(|n| {
                        Self::prefs_choice(line_height_label(*n).into(), p.line_height, *n, |p, v| {
                            p.line_height = v
                        })
                    })
                    .collect(),
            ),
            (
                "Wrap long lines",
                None,
                "Line wrapping",
                vec![
                    Self::prefs_choice("Wrap".into(), p.line_wrapping, true, |p, v| p.line_wrapping = v),
                    Self::prefs_choice("Scroll".into(), p.line_wrapping, false, |p, v| p.line_wrapping = v),
                ],
            ),
            (
                "Tab size",
                Some("Display width of tab characters."),
                "Tab size",
                EDITOR_TAB_SIZES
                    .iter()
                    .map(|n| Self::prefs_choice(n.to_string(), p.tab_size, *n, |p, v| p.tab_size = v))
                    .collect(),
            ),
            (
                "Default indent",
                Some("What Tab inserts when a language has no override."),
                "Default indent style",
                INDENT_OPTIONS
                    .iter()
                    .map(|(k, l, _)| Self::prefs_choice((*l).into(), p.indent, *k, |p, v| p.indent = v))
                    .collect(),
            ),
        ];
        for (title, d, group, choices) in groups {
            head.push(
                Row::new()
                    .child(copy(group, title, d.map(|d| vec![span(d)]), cx))
                    .child(self.seg_group(group, choices, 11., cx)),
            );
        }

        // Indent by language.
        let mut langs: Vec<Row> = Vec::new();
        for (id, label, hint) in EDITOR_INDENT_LANGS {
            let over = p.indent_by_lang.get(*id).copied();
            let conv = language_indent_default(id);
            let effective = p.resolve_indent(id);
            let d = match (over, conv) {
                (Some(o), _) => format!("Override · {}", o.label()),
                (None, Some(c)) => match hint {
                    Some(h) => format!("Auto · {} ({h})", c.label()),
                    None => format!("Auto · {}", c.label()),
                },
                (None, None) => format!("Auto · {} (global)", effective.label()),
            };
            let lang = id.to_string();
            let mut choices: Vec<Choice> = vec![(
                "Auto".into(),
                over.is_none(),
                Box::new({
                    let lang = lang.clone();
                    move |this, cx| {
                        let lang = lang.clone();
                        this.vcx
                            .state
                            .update(cx, |s, cx| s.set_editor_indent_for_lang(&lang, None, cx))
                    }
                }),
            )];
            for (k, _, short) in INDENT_OPTIONS {
                let lang = lang.clone();
                choices.push((
                    short.into(),
                    over == Some(k),
                    Box::new(move |this, cx| {
                        let lang = lang.clone();
                        this.vcx
                            .state
                            .update(cx, |s, cx| s.set_editor_indent_for_lang(&lang, Some(k), cx))
                    }),
                ));
            }
            let group = format!("{label} indent");
            langs.push(
                Row::new()
                    .child(copy(id, *label, Some(vec![span(d)]), cx))
                    .child(self.seg_group(group, choices, 8., cx)),
            );
        }

        // After the group.
        let mut tail: Vec<Row> = Vec::new();
        let ws = (
            "Render whitespace",
            Some("Show spaces and tabs as marks."),
            "Render whitespace",
            [
                (EditorWhitespace::Off, "Off"),
                (EditorWhitespace::Trailing, "Trailing"),
                (EditorWhitespace::All, "All"),
            ]
            .into_iter()
            .map(|(k, l)| Self::prefs_choice(l.into(), p.whitespace, k, |p, v| p.whitespace = v))
            .collect::<Vec<_>>(),
        );
        let mut more: Vec<(&str, Option<&str>, &'static str, Vec<Choice>)> = vec![ws];
        let toggles: [(&str, Option<&str>, &'static str, bool, fn(&mut EditorPrefs, bool)); 10] = [
            (
                "Special characters",
                Some("Highlight NBSP, zero-width, and other invisible chars."),
                "Special characters",
                p.special_chars,
                |p, v| p.special_chars = v,
            ),
            ("Line numbers", None, "Line numbers", p.line_numbers, |p, v| p.line_numbers = v),
            ("Highlight active line", None, "Highlight active line", p.active_line, |p, v| {
                p.active_line = v
            }),
            ("Fold gutter", None, "Fold gutter", p.fold_gutter, |p, v| p.fold_gutter = v),
            (
                "Diff gutter",
                Some("Add, modify, and delete hints versus the workspace base."),
                "Diff gutter",
                p.diff_gutter,
                |p, v| p.diff_gutter = v,
            ),
            ("Bracket matching", None, "Bracket matching", p.bracket_matching, |p, v| {
                p.bracket_matching = v
            }),
            (
                "Scroll past end",
                Some("Allow the last lines to sit mid-viewport."),
                "Scroll past end",
                p.scroll_past_end,
                |p, v| p.scroll_past_end = v,
            ),
            ("Cursor blink", None, "Cursor blink", p.cursor_blink, |p, v| p.cursor_blink = v),
            ("Thick caret", None, "Thick caret", p.thick_caret, |p, v| p.thick_caret = v),
            (
                "Rectangular selection",
                Some("Option/Alt-drag to select columns."),
                "Rectangular selection",
                p.rectangular_select,
                |p, v| p.rectangular_select = v,
            ),
        ];
        for (title, d, group, cur, set) in toggles {
            let (g, c) = Self::on_off(group, cur, set);
            more.push((title, d, g, c));
        }
        let (g, c) = Self::on_off("Drop cursor", p.drop_cursor, |p, v| p.drop_cursor = v);
        more.push(("Drop cursor", Some("Show a caret while dragging text."), g, c));
        more.push((
            "Print margin",
            Some("Vertical ruler at a column."),
            "Print margin",
            EDITOR_RULER_COLUMNS
                .iter()
                .map(|n| {
                    let label = if *n == 0 { "Off".to_string() } else { n.to_string() };
                    Self::prefs_choice(label, p.ruler_column, *n, |p, v| p.ruler_column = v)
                })
                .collect(),
        ));
        more.push((
            "Keymap",
            Some("Default, Vim, or Emacs bindings."),
            "Editor keymap",
            [
                (EditorKeymap::Default, "Default"),
                (EditorKeymap::Vim, "Vim"),
                (EditorKeymap::Emacs, "Emacs"),
            ]
            .into_iter()
            .map(|(k, l)| Self::prefs_choice(l.into(), p.keymap, k, |p, v| p.keymap = v))
            .collect(),
        ));
        for (title, d, group, choices) in more {
            tail.push(
                Row::new()
                    .child(copy(group, title, d.map(|d| vec![span(d)]), cx))
                    .child(self.seg_group(group, choices, 11., cx)),
            );
        }
        let max_open = self.bound_field(
            "max-open-files",
            &int_text(p.max_open_files as i64),
            FieldOpts {
                masked: false,
                align_right: true,
            },
            window,
            cx,
        );
        tail.push(
            Row::new()
                .child(copy(
                    "max-open",
                    "Max open files",
                    Some(vec![span("Tab strip limit. Use 0 for unlimited.")]),
                    cx,
                ))
                .child(ctrl().child(
                    div()
                        .debug_selector(|| "inp:Max open files".into())
                        .child(inp(&max_open, &SharedString::default(), InpStyle::NUM, window, cx)),
                )),
        );

        // Terminal.
        let mut term_sizes: Vec<Choice> = vec![(
            "Match".into(),
            tp.font_size == 0,
            Box::new(|this, cx| {
                this.vcx
                    .state
                    .update(cx, |s, cx| s.set_terminal_prefs(|p| p.font_size = 0, cx))
            }),
        )];
        for n in EDITOR_FONT_SIZES {
            term_sizes.push((
                n.to_string(),
                tp.font_size as u32 == n,
                Box::new(move |this, cx| {
                    this.vcx
                        .state
                        .update(cx, |s, cx| s.set_terminal_prefs(|p| p.font_size = n as u8, cx))
                }),
            ));
        }
        let term_lh: Vec<Choice> = TERMINAL_LINE_HEIGHTS
            .iter()
            .map(|n| {
                let n = *n;
                let c: Choice = (
                    terminal_line_height_label(n).into(),
                    tp.line_height == n,
                    Box::new(move |this, cx| {
                        this.vcx
                            .state
                            .update(cx, |s, cx| s.set_terminal_prefs(|p| p.line_height = n, cx))
                    }),
                );
                c
            })
            .collect();
        let term_rows = vec![
            Row::new()
                .child(copy(
                    "term-font",
                    "Font",
                    Some(vec![span(format!("System monospace fonts{scanning}"))]),
                    cx,
                ))
                .child(font_ctrl(self.term_font_select.clone())),
            Row::new()
                .child(copy("term-size", "Font size", None, cx))
                .child(self.seg_group("Terminal font size", term_sizes, 11., cx)),
            Row::new()
                .child(copy(
                    "term-lh",
                    "Line height",
                    Some(vec![span("Tighter than the editor fits more rows in the dock.")]),
                    cx,
                ))
                .child(self.seg_group("Terminal line height", term_lh, 11., cx)),
        ];

        // Code panels.
        let sides = self.vcx.state.read(cx).panels.sides.clone();
        let mut panel_rows: Vec<Row> = Vec::new();
        for key in CodePanelKey::ALL {
            let cur = sides.get(&key).copied().unwrap_or(key.default_side());
            let group = format!("{} panel side", key.label());
            let choices: Vec<Choice> = [(PanelSide::Left, "Left"), (PanelSide::Right, "Right")]
                .into_iter()
                .map(|(side, l)| {
                    let c: Choice = (
                        l.into(),
                        cur == side,
                        Box::new(move |this, cx| {
                            this.vcx
                                .state
                                .update(cx, |s, cx| s.set_code_panel_side(key, side, cx))
                        }),
                    );
                    c
                })
                .collect();
            panel_rows.push(
                Row::new()
                    .child(copy(key.key(), key.label(), None, cx))
                    .child(self.seg_group(group, choices, 11., cx)),
            );
        }

        // Files: exclude.
        let patterns = self.vcx.state.read(cx).explorer_exclude();
        let mut excl = div().flex().flex_col().gap(px(6.)).w_full();
        for (i, pat) in patterns.iter().enumerate() {
            let key = format!("exclude:{i}");
            let input = self.bound_field(&key, pat, FieldOpts::default(), window, cx);
            excl = excl.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .debug_selector(move || format!("inp:Exclude pattern {}", i + 1))
                            .child(inp(&input, &"**/pattern".into(), InpStyle::EXCLUDE, window, cx)),
                    )
                    .child(recur_ui::tooltip::titled(
                        SharedString::from(format!("exclude-rm-tip-{i}")),
                        "Remove pattern",
                        rm_btn(SharedString::from(format!("exclude-rm-{i}")), IconName::X, false, cx)
                            .debug_selector(move || format!("btn:Remove pattern {}", i + 1))
                            .on_click(cx.listener(move |this, _, _, cx| this.remove_exclude(i, cx))),
                    )),
            );
        }
        excl = excl.child(
            div()
                .flex()
                .gap(px(8.))
                .mt(px(4.))
                .child(
                    tbtn_label("exclude-add", "Add pattern", false, cx)
                        .debug_selector(|| "btn:Add pattern".into())
                        .on_click(cx.listener(|this, _, _, cx| {
                            let mut next = this.vcx.state.read(cx).explorer_exclude();
                            next.push(String::new());
                            this.vcx
                                .state
                                .update(cx, |s, cx| s.set_explorer_exclude(next, cx));
                        })),
                )
                .child(
                    tbtn_label("exclude-reset", "Reset defaults", false, cx)
                        .debug_selector(|| "btn:Reset defaults".into())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.vcx
                                .state
                                .update(cx, |s, cx| s.reset_explorer_exclude(cx))
                        })),
                ),
        );

        settings_groups()
            .child(
                settings_group("grp-editor", "Editor")
                    .blurb("Font, indent, keymap, and display for the observatory CodeMirror editor.")
                    .child(div().debug_selector(|| "editor-prefs".into()).child(rows(IN_GROUP, head, window, cx)))
                    .child(
                        settings_group("grp-indent", "Indent by language")
                            .blurb("Auto uses common conventions (Go tabs, TS 2 spaces, …). Override anytime.")
                            .nested()
                            .child(
                                div()
                                    .debug_selector(|| "indent-lang-rows".into())
                                    .child(rows(IN_GROUP, langs, window, cx)),
                            ),
                    )
                    .child(div().debug_selector(|| "editor-prefs-2".into()).child(rows(IN_GROUP, tail, window, cx))),
            )
            .child(
                settings_group("grp-terminal", "Terminal")
                    .blurb("Font for the terminal dock (⌃`). Defaults to the editor's.")
                    .child(rows(IN_GROUP, term_rows, window, cx)),
            )
            .child(
                settings_group("grp-panels", "Code panels")
                    .blurb("Which side of the editor each Code tool opens on.")
                    .child(rows(IN_GROUP, panel_rows, window, cx)),
            )
            .child(
                settings_group("grp-exclude", "Files: exclude")
                    .blurb("Glob patterns hidden from the Code file explorer. Git-ignored files and folders still appear (dimmed) unless matched here.")
                    .child(excl),
            )
            .into_any_element()
    }

    fn remove_exclude(&mut self, i: usize, cx: &mut Context<Self>) {
        let mut next = self.vcx.state.read(cx).explorer_exclude();
        if i < next.len() {
            next.remove(i);
        }
        self.vcx
            .state
            .update(cx, |s, cx| s.set_explorer_exclude(next, cx));
    }

    /// The Code editor pane's field events.
    pub(crate) fn editor_field(&mut self, key: &str, ev: FieldEvent, value: String, cx: &mut Context<Self>) {
        if key == "max-open-files" && ev == FieldEvent::Change {
            let n = number_input_value(&value);
            self.set_editor_prefs(move |p| p.max_open_files = n.max(0.).min(u32::MAX as f64) as u32, cx);
            return;
        }
        if let Some(i) = key.strip_prefix("exclude:").and_then(|i| i.parse::<usize>().ok())
            && ev == FieldEvent::Input
        {
            let mut next = self.vcx.state.read(cx).explorer_exclude();
            if let Some(slot) = next.get_mut(i) {
                *slot = value;
            }
            self.vcx
                .state
                .update(cx, |s, cx| s.set_explorer_exclude(next, cx));
        }
    }

    /// The Code editor effect: the monospace font list.
    pub(crate) fn load_fonts(&mut self, g: u64, cx: &mut Context<Self>) {
        self.mono_loading = true;
        let task = list_monospace_fonts(&self.vcx.services, cx);
        cx.spawn(async move |this, cx| {
            let fonts = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.fx.live("section", g) {
                    this.mono_fonts = fonts;
                    this.mono_loading = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }
}
