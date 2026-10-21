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
use crate::widgets::{PicBox, project_pic, ring};
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
];

pub(super) fn matches(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => pattern == name,
    }
}

/// A scene that renders a function of the app context.
struct Static(fn(&mut Window, &mut App) -> AnyElement);

impl Render for Static {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        (self.0)(window, cx)
    }
}

pub(super) fn build(name: &str, _: &mut Window, cx: &mut App) -> Option<AnyView> {
    let kind = name.split('/').next().unwrap_or_default();
    let variant = name.split_once('/').map_or("", |(_, v)| v);
    let f: fn(&mut Window, &mut App) -> AnyElement = match (kind, variant) {
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
        _ => return None,
    };
    Some(cx.new(|_| Static(f)).into())
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
        tbtn(id, kind, b, false, cx).child(button_label(label, b.font_size, weight::MEDIUM))
    };
    let tsz = TbtnSize::TITLEBAR;
    let titlebar = |id: &'static str, kind, label: &'static str, disabled, cx: &App| {
        tbtn(id, kind, tsz, disabled, cx).child(button_label(label, tsz.font_size, weight::MEDIUM))
    };
    let rb = |id: &'static str, kind, label: &'static str, disabled, cx: &App| {
        rbtn(id, kind, disabled, cx).child(button_label(label, 12.5, weight::MEDIUM_PLUS))
    };
    let go = |id: &'static str, disabled, cx: &App| {
        primary_button(id, disabled, cx).child(button_label("Create", 13., weight::SEMIBOLD))
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
                            tbtn("t7", Tbtn::Default, b, false, cx)
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
    let theme = Theme::of(cx);
    let (light, fg) = (theme.is_light(), theme.tokens.text);
    let cells = FILES
        .iter()
        .map(|f| file_icon(f, light, fg).into_any_element())
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
