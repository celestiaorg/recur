//! Scene builders, one per `scenes.json` name prefix. Each mirrors the Svelte scene of the
//! same name in `rust/tools/ui-parity/web/scenes/`.

use crate::file_icons::file_icon;
use crate::fonts::{Family, weight};
use crate::icon::{icon, lucide_icon, provider_icon};
use crate::icon_name::IconName;
use crate::kbd::kbd;
use crate::style::{
    self, Rbtn, Tbtn, TbtnSize, body, button_label, esc_key, footer_key, palette_footer,
    palette_input_row, palette_key, primary_button, rbtn, tbtn,
};
use crate::text::{LineHeight, Text, TextStyled, span, text};
use crate::theme::Theme;
use crate::tooltip::{TitleTooltip, titled};
use crate::widgets::{
    ChipIcon, PicBox, ToastAsk, ToastKind, ToastOptions, ToastStore, Toasts, armed_button,
    blurb_code, decode_text, project_pic, push_toast, ring, settings_group, settings_groups,
    tab_chip, tab_strip, tab_strip_action, toast_action,
};
use gpui_kit::{
    AnyElement, AnyView, App, AppContext, Context, Div, IntoElement, ParentElement, Render, Styled,
    Window, div, px,
};
use serde::Deserialize;

/// Scene name patterns; a trailing `*` matches any suffix.
pub(super) const NAMES: &[&str] = &[
    "tokens/*",
    "text/calibration",
    "kbd/*",
    "buttons/*",
    "toggles/*",
    "icons/app",
    "icons/lucide",
    "icons/provider",
    "icons/file",
    "icons/file-light",
    "ring/*",
    "project-pic/*",
    "settings-group/*",
    "armed-button/*",
    "toasts/*",
    "tooltip/*",
    "tab-strip/*",
];

pub(super) fn matches(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => pattern == name,
    }
}

type Content = Box<dyn Fn(&mut Window, &mut App) -> AnyElement>;

/// A scene's root: its content, then the overlays every Recur window renders last (title
/// tooltips, and the toast stack where the scene shows it).
struct Stage {
    content: Content,
    overlays: Vec<AnyView>,
}

impl Render for Stage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        body(cx)
            .relative()
            .size_full()
            .child((self.content)(window, cx))
            .children(self.overlays.iter().cloned())
    }
}

pub(super) fn build(name: &str, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let kind = name.split('/').next().unwrap_or_default();
    let variant = name.split_once('/').map_or("", |(_, v)| v);
    let mut overlays: Vec<AnyView> = vec![TitleTooltip::new(cx).into()];
    let content: Content = match (kind, variant) {
        ("toasts", _) => {
            seed_toasts(window, cx);
            overlays.push(cx.new(Toasts::new).into());
            Box::new(|_, cx| body(cx).size_full().into_any_element())
        }
        _ => Box::new(static_content(kind, variant)?),
    };
    Some(cx.new(|_| Stage { content, overlays }).into())
}

fn static_content(kind: &str, variant: &str) -> Option<fn(&mut Window, &mut App) -> AnyElement> {
    Some(match (kind, variant) {
        ("tokens", _) => tokens,
        ("text", "calibration") => calibration,
        ("kbd", _) => kbd_scene,
        ("buttons", _) => buttons,
        ("toggles", _) => toggles,
        ("icons", "app") => icons_app,
        ("icons", "lucide") => icons_lucide,
        ("icons", "provider") => icons_provider,
        ("icons", "file") | ("icons", "file-light") => icons_file,
        ("ring", _) => rings,
        ("project-pic", _) => project_pics,
        ("settings-group", _) => settings,
        ("armed-button", _) => armed,
        ("tooltip", _) => tooltips,
        ("tab-strip", "overflow") => tabs_overflow,
        ("tab-strip", _) => tabs,
        _ => return None,
    })
}

fn row() -> Div {
    div().flex().items_center()
}

fn tokens(_: &mut Window, cx: &mut App) -> AnyElement {
    let t = Theme::of(cx).tokens;
    body(cx)
        .size_full()
        .child(
            div()
                .p(px(8.))
                .w(px(8. * 2. + 7. * 36. + 6. * 4.))
                .flex()
                .flex_wrap()
                .gap(px(4.))
                .children(
                    t.named()
                        .into_iter()
                        .map(|(_, c)| div().size(px(36.)).bg(c)),
                ),
        )
        .into_any_element()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Sample {
    family: String,
    size: f32,
    weight: f32,
    line_height: serde_json::Value,
    #[serde(default)]
    tracking: f32,
    #[serde(default)]
    uppercase: bool,
}

#[derive(Deserialize)]
struct CalibrationSet {
    text: String,
    samples: Vec<Sample>,
}

fn calibration(_: &mut Window, cx: &mut App) -> AnyElement {
    let set: CalibrationSet =
        serde_json::from_str(super::CALIBRATION_JSON).unwrap_or(CalibrationSet {
            text: String::new(),
            samples: Vec::new(),
        });
    let lines = set.samples.into_iter().map(|s| {
        let family = match s.family.as_str() {
            "mono" => Family::Mono,
            "disp" => Family::Display,
            "system" => Family::System,
            _ => Family::Ui,
        };
        let lh = match s.line_height.as_f64() {
            Some(m) => LineHeight::Scale(m as f32),
            None => LineHeight::Normal,
        };
        let mut t = text(set.text.clone())
            .family(family)
            .size(s.size)
            .weight(s.weight)
            .tracking(s.tracking)
            .line_height(lh)
            .nowrap();
        if s.uppercase {
            t = t.uppercase();
        }
        t
    });
    body(cx)
        .size_full()
        .child(
            div()
                .flex()
                .flex_col()
                .items_start()
                .p(px(8.))
                .children(lines),
        )
        .into_any_element()
}

const KEYS: [&str; 6] = ["⌘K", "⌘⇧P", "⌥←", "Ctrl+K", "esc", "↵"];

fn kbd_scene(_: &mut Window, cx: &mut App) -> AnyElement {
    let t = Theme::of(cx).tokens;
    let pal_row = |i: usize, k: &'static str| {
        let sel = i == 1;
        let mut r = div()
            .flex()
            .items_center()
            .gap(px(11.))
            .py(px(8.))
            .px(px(10.))
            .rounded(px(7.));
        if sel {
            r = r.bg(t.pane_2);
        }
        r.child(palette_key(k, sel, cx))
    };
    body(cx)
        .size_full()
        .child(
            div()
                .p(px(12.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(
                    row()
                        .gap(px(10.))
                        .text_size(px(11.))
                        .text_color(t.faint)
                        .children(KEYS.map(kbd)),
                )
                .child(
                    row()
                        .gap(px(10.))
                        .children(KEYS.iter().enumerate().map(|(i, k)| pal_row(i, k))),
                )
                .child(
                    row().gap(px(10.)).child(
                        palette_footer(false, cx)
                            .child(Text::from_spans(vec![
                                footer_key("↑↓", cx),
                                span("navigate"),
                            ]))
                            .child(Text::from_spans(vec![footer_key("↵", cx), span("open")])),
                    ),
                )
                .child(
                    row()
                        .gap(px(10.))
                        .child(palette_input_row(false, cx).child(esc_key(false, cx))),
                ),
        )
        .into_any_element()
}

fn buttons(_: &mut Window, cx: &mut App) -> AnyElement {
    let b = TbtnSize::BASE;
    let tb = |id: &'static str, kind, label: &'static str, cx: &App| {
        tbtn(id, kind, b, None, cx).child(button_label(label, b.font_size, weight::MEDIUM))
    };
    let tsz = TbtnSize::TITLEBAR;
    let back = Theme::of(cx).tokens.bg;
    let titlebar = |id: &'static str, kind, label: &'static str, disabled: bool, cx: &App| {
        tbtn(id, kind, tsz, disabled.then_some(back), cx).child(button_label(
            label,
            tsz.font_size,
            weight::MEDIUM,
        ))
    };
    let rb = |id: &'static str, kind, label: &'static str, disabled: bool, cx: &App| {
        rbtn(id, kind, disabled.then_some(back), cx).child(button_label(
            label,
            12.5,
            weight::MEDIUM_PLUS,
        ))
    };
    let go = |id: &'static str, disabled: bool, cx: &App| {
        primary_button(id, disabled.then_some(back), cx).child(button_label(
            "Create",
            13.,
            weight::SEMIBOLD,
        ))
    };
    let link = |id: &'static str, on, label: &'static str, cx: &App| {
        style::link_button(id, on, cx)
            .child(icon(IconName::Plus).size(px(12.)))
            .child(text(label).size(12.).nowrap())
    };
    body(cx)
        .size_full()
        .child(
            div()
                .p(px(12.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(
                    row()
                        .gap(px(8.))
                        .child(tb("t1", Tbtn::Default, "Default", cx))
                        .child(tb("t2", Tbtn::Amber, "Amber", cx))
                        .child(tb("t3", Tbtn::Solid, "Solid", cx))
                        .child(tb("t4", Tbtn::On, "On", cx))
                        .child(tb("t5", Tbtn::Danger, "Remove", cx))
                        .child(tb("t6", Tbtn::Armed, "Confirm", cx))
                        .child(
                            tbtn("t7", Tbtn::Default, b, None, cx)
                                .child(icon(IconName::Plus).size(px(13.)))
                                .child(button_label(
                                    "Create workspace",
                                    b.font_size,
                                    weight::MEDIUM,
                                )),
                        ),
                )
                .child(
                    row().gap(px(8.)).child(
                        row()
                            .flex_none()
                            .justify_end()
                            .gap(px(6.))
                            .child(titlebar("r1", Tbtn::Default, "Titlebar", false, cx))
                            .child(titlebar("r2", Tbtn::Danger, "Remove", false, cx))
                            .child(titlebar("r3", Tbtn::Danger, "Disabled", true, cx)),
                    ),
                )
                .child(
                    row()
                        .gap(px(8.))
                        .child(rb("b1", Rbtn::Default, "Review", false, cx))
                        .child(rb("b2", Rbtn::Merge, "Merge", false, cx))
                        .child(rb("b3", Rbtn::Primary, "Primary", false, cx))
                        .child(rb("b4", Rbtn::Armed, "Armed", false, cx))
                        .child(rb("b5", Rbtn::Primary, "Disabled", true, cx)),
                )
                .child(
                    row()
                        .gap(px(8.))
                        .child(go("g1", false, cx))
                        .child(go("g2", true, cx))
                        .child(link("l1", false, "Attach", cx))
                        .child(link("l2", true, "Linked", cx))
                        .child(style::lnk("open log", cx)),
                )
                .child(
                    row()
                        .gap(px(8.))
                        .child(
                            style::titlebar_icon_button("i1", false, cx)
                                .child(icon(IconName::Plus).size(px(14.))),
                        )
                        .child(
                            style::titlebar_icon_button("i2", true, cx)
                                .child(icon(IconName::Plus).size(px(14.))),
                        )
                        .child(
                            style::rail_toggle("i3", false, cx)
                                .child(icon(IconName::Box).size(px(16.))),
                        )
                        .child(
                            style::rail_toggle("i4", true, cx)
                                .child(icon(IconName::Box).size(px(16.))),
                        ),
                ),
        )
        .into_any_element()
}

fn toggles(_: &mut Window, cx: &mut App) -> AnyElement {
    body(cx)
        .size_full()
        .child(
            row()
                .p(px(12.))
                .gap(px(12.))
                .child(style::toggle("a", false, false, cx))
                .child(style::toggle("b", true, false, cx))
                .child(
                    div()
                        .w(px(60.))
                        .h(px(30.))
                        .flex()
                        .child(style::panel(cx).flex_1()),
                )
                .child(div().w(px(80.)).child(style::progress(0.4, cx))),
        )
        .into_any_element()
}

fn icon_grid(cols: usize, cells: Vec<AnyElement>, cx: &App) -> AnyElement {
    let t = Theme::of(cx).tokens;
    body(cx)
        .size_full()
        .child(
            div()
                .p(px(8.))
                .w(px(16. + cols as f32 * 20. + (cols - 1) as f32 * 4.))
                .flex()
                .flex_wrap()
                .gap(px(4.))
                .text_color(t.text)
                .children(cells.into_iter().map(|c| {
                    div()
                        .size(px(20.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(c)
                })),
        )
        .into_any_element()
}

fn icons_app(_: &mut Window, cx: &mut App) -> AnyElement {
    let cells = IconName::ALL
        .iter()
        .map(|n| icon(*n).size(px(16.)).into_any_element())
        .collect();
    icon_grid(21, cells, cx)
}

fn icons_lucide(_: &mut Window, cx: &mut App) -> AnyElement {
    let cells = crate::assets::lucide_suggested()
        .map(|e| lucide_icon(&e.name).size(px(16.)).into_any_element())
        .collect();
    icon_grid(16, cells, cx)
}

fn icons_provider(_: &mut Window, cx: &mut App) -> AnyElement {
    let cells = ["cursor", "claude", "codex", "ollama", "compatible"]
        .iter()
        .map(|p| provider_icon(p).size(px(14.)).into_any_element())
        .collect();
    icon_grid(16, cells, cx)
}

const FILES: [&str; 20] = [
    "main.rs",
    "app.ts",
    "index.js",
    "package.json",
    ".env",
    "README.md",
    "Cargo.toml",
    "go.mod",
    "style.scss",
    "index.html",
    "logo.png",
    "Dockerfile",
    "Makefile",
    "tool.py",
    "App.svelte",
    "data.json",
    "notes.txt",
    "unknown.zzz",
    "deploy.yml",
    "schema.sql",
];

fn icons_file(_: &mut Window, cx: &mut App) -> AnyElement {
    let cells = FILES
        .iter()
        .map(|f| file_icon(*f).into_any_element())
        .collect();
    icon_grid(16, cells, cx)
}

fn rings(_: &mut Window, cx: &mut App) -> AnyElement {
    let t = Theme::of(cx).tokens;
    body(cx)
        .size_full()
        .child(
            row()
                .p(px(12.))
                .gap(px(14.))
                .child(ring(30., 0.4, 2))
                .child(ring(42., 0.8, 5).color(t.emerald))
                .child(ring(60., 0., 0))
                .child(ring(100., 0.6, 3)),
        )
        .into_any_element()
}

fn project_pics(_: &mut Window, cx: &mut App) -> AnyElement {
    let pics = [
        ("", "var(--cyan)", "recur"),
        ("Ab", "var(--amber)", ""),
        ("lucide:rocket", "var(--indigo)", ""),
        ("lucide:box", "#1e2a3a", ""),
        ("", "#f2c94c", "zeta"),
        ("", "", "émile"),
    ];
    body(cx)
        .size_full()
        .child(
            row()
                .p(px(12.))
                .gap(px(10.))
                .children(pics.map(|(p, c, n)| project_pic(p, c, n, PicBox::NT))),
        )
        .into_any_element()
}

fn settings(_: &mut Window, cx: &mut App) -> AnyElement {
    let muted = Theme::of(cx).tokens.muted;
    let body_text = move |s: &'static str| text(s).size(12.).color(muted);
    body(cx)
        .size_full()
        .child(
            div().p(px(12.)).w(px(420.)).child(
                settings_groups()
                    .child(
                        settings_group("a", "Appearance")
                            .blurb("Theme and density for every window.")
                            .child(body_text("Body content")),
                    )
                    .child(
                        settings_group("b", "Agent defaults")
                            .blurb_spans(vec![
                                span("Applied to new chats. Set "),
                                blurb_code("model"),
                                span(" per project."),
                            ])
                            .override_toggle(true, false, None, |_, _, _| {})
                            .child(body_text("Overridden here")),
                    )
                    .child(
                        settings_group("c", "Project override")
                            .blurb("Uses the global settings.")
                            .override_toggle(false, false, None, |_, _, _| {})
                            .child(body_text("Hidden")),
                    )
                    .child(settings_group("d", "Locked").override_toggle(
                        false,
                        true,
                        None,
                        |_, _, _| {},
                    ))
                    .child(
                        settings_group("e", "Editor").child(
                            settings_group("e1", "Indent by language")
                                .blurb("Auto uses common conventions.")
                                .nested()
                                .child(body_text("Go · tabs")),
                        ),
                    ),
            ),
        )
        .into_any_element()
}

fn armed(_: &mut Window, cx: &mut App) -> AnyElement {
    let t = Theme::of(cx).tokens;
    let label = |id: &'static str, s: &'static str, animate: bool| {
        decode_text(id, s)
            .animate(animate)
            .style(move |x| x.size(12.).color(t.muted))
    };
    body(cx)
        .size_full()
        .child(
            div()
                .p(px(12.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(
                    row()
                        .gap(px(8.))
                        .child(armed_button("a", "Remove", "Confirm remove", |_, _| {}))
                        .child(armed_button("b", "Delete", "Really delete?", |_, _| {}))
                        .child(
                            armed_button("c", "Discard", "Discard", |_, _| {}).disabled(Some(t.bg)),
                        ),
                )
                .child(
                    row().child(
                        row().flex_none().justify_end().gap(px(6.)).child(
                            armed_button("d", "Archive", "Confirm", |_, _| {})
                                .size(TbtnSize::TITLEBAR)
                                .icon(icon(IconName::Trash).size(px(13.))),
                        ),
                    ),
                )
                .child(
                    row()
                        .gap(px(8.))
                        .child(label("l1", "feature/parity-scenes", true))
                        .child(label("l2", "Reviewing changes", false)),
                ),
        )
        .into_any_element()
}

/// Sticky toasts of every kind, and an ask toast whose 30s TTL bar the scene freezes halfway.
fn seed_toasts(window: &mut Window, cx: &mut App) {
    let store = ToastStore::global(cx);
    let old: Vec<u64> = store.read(cx).toasts().iter().map(|t| t.id).collect();
    for id in old {
        ToastStore::dismiss(&store, id, false, window, cx);
    }
    let sticky = |action| ToastOptions {
        action,
        ttl: Some(std::time::Duration::ZERO),
        ..Default::default()
    };
    push_toast(
        ToastKind::Success,
        "Saved workspace settings",
        sticky(None),
        cx,
    );
    push_toast(
        ToastKind::Error,
        "Push failed: the remote rejected the update",
        sticky(None),
        cx,
    );
    push_toast(
        ToastKind::Info,
        "Recur 0.9 is ready",
        sticky(Some(toast_action("Restart", |_, _| {}))),
        cx,
    );
    let ask = ToastAsk {
        project: "recur".into(),
        workspace: "honeyed-cannoli".into(),
        question: "Should the parity task also run on Linux?".into(),
    };
    store.update(cx, |s, cx| {
        s.push_ask(ask, toast_action("Answer", |_, _| {}), None, cx)
    });
}

fn tooltips(_: &mut Window, cx: &mut App) -> AnyElement {
    let b = TbtnSize::BASE;
    let button = |id: &'static str, label: &'static str, cx: &App| {
        tbtn(id, Tbtn::Default, b, None, cx).child(button_label(label, b.font_size, weight::MEDIUM))
    };
    body(cx)
        .size_full()
        .relative()
        .child(div().absolute().left(px(16.)).top(px(16.)).child(titled(
            "settings-tip",
            "Open settings (⌘,)",
            button("settings", "Settings", cx),
        )))
        .child(
            div()
                .absolute()
                .left(px(120.))
                .bottom(px(16.))
                .child(titled(
                    "check-tip",
                    "Run the parity check against the WebKit reference",
                    button("check", "Check", cx),
                )),
        )
        .into_any_element()
}

fn tabs(_: &mut Window, cx: &mut App) -> AnyElement {
    let action = |id: &'static str, on: bool, name: IconName, cx: &App| {
        tab_strip_action(id, on, false, cx).child(icon(name).size(px(15.)))
    };
    let strip = tab_strip("files", "Open files")
        .chrome(true)
        .active_key("changes")
        .tab(
            "main",
            tab_chip("main", "main.rs").icon(ChipIcon::File("main.rs".into())),
        )
        .tab(
            "changes",
            tab_chip("changes", "Changes")
                .icon(ChipIcon::App(IconName::GitBranch, None))
                .active(true)
                .lit(true)
                .badge(4),
        )
        .tab(
            "notes",
            tab_chip("notes", "notes.md")
                .icon(ChipIcon::File("notes.md".into()))
                .dirty(true)
                .on_close(|_, _, _| {}),
        )
        .tab(
            "chat",
            tab_chip("chat", "Chat")
                .icon(ChipIcon::App(IconName::MessageQuestion, None))
                .unseen(true),
        )
        .tab(
            "build",
            tab_chip("build", "Build")
                .icon(ChipIcon::App(IconName::Terminal, None))
                .busy(true),
        )
        .tab(
            "old",
            tab_chip("old", "old shell")
                .icon(ChipIcon::App(IconName::Terminal, None))
                .muted(true),
        )
        .trailing(
            div()
                .flex()
                .items_center()
                .gap(px(2.))
                .child(action("add", false, IconName::Plus, cx))
                .child(action("max", true, IconName::Maximize, cx)),
        );
    body(cx).size_full().child(strip).into_any_element()
}

const MANY: [&str; 8] = [
    "alpha.ts",
    "beta.rs",
    "gamma.go",
    "delta.py",
    "epsilon.md",
    "zeta.json",
    "eta.toml",
    "theta.css",
];

fn tabs_overflow(_: &mut Window, cx: &mut App) -> AnyElement {
    let strip = MANY.iter().fold(
        tab_strip("many", "Open files")
            .chrome(true)
            .active_key("theta.css"),
        |s, f| {
            s.tab(
                *f,
                tab_chip(*f, *f)
                    .icon(ChipIcon::File((*f).into()))
                    .active(*f == "theta.css")
                    .on_close(|_, _, _| {}),
            )
        },
    );
    body(cx)
        .size_full()
        .child(div().w(px(300.)).child(strip))
        .into_any_element()
}
