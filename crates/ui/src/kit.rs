//! Restyle gpui-kit's component layer with Recur tokens, so any gpui-component primitive a
//! view uses (inputs, lists, scrollbars) starts from Recur's palette and fonts.

use crate::color::Color;
use crate::fonts::{MONO, UI};
use crate::theme::Theme;
use gpui_kit::component::{Theme as KitTheme, ThemeMode};
use gpui_kit::{App, Hsla, px};

fn h(c: Color) -> Hsla {
    c.into()
}

pub(crate) fn sync_component_theme(cx: &mut App) {
    if !cx.has_global::<KitTheme>() || !cx.has_global::<Theme>() {
        return;
    }
    let theme = Theme::of(cx).clone();
    let t = theme.tokens;
    KitTheme::update(cx, |k| {
        k.mode = if theme.is_light() {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        };
        k.font_family = UI.into();
        k.font_size = px(13.);
        k.mono_font_family = MONO.into();
        k.mono_font_size = px(13.);
        k.radius = px(6.);
        k.radius_lg = px(10.);
        k.background = h(t.bg);
        k.foreground = h(t.text);
        k.border = h(t.line);
        k.input = h(t.line);
        k.muted = h(t.pane_2);
        k.muted_foreground = h(t.muted);
        k.popover = h(t.pane_2);
        k.popover_foreground = h(t.text);
        k.accent = h(t.pane);
        k.accent_foreground = h(t.text);
        k.primary = h(t.amber);
        k.primary_hover = h(t.amber.brightness(1.05));
        k.primary_active = h(t.amber);
        k.primary_foreground = h(t.on_amber);
        k.secondary = h(t.pane);
        k.secondary_hover = h(t.pane_2);
        k.secondary_active = h(t.pane_2);
        k.secondary_foreground = h(t.text);
        k.danger = h(t.coral);
        k.danger_foreground = h(t.on_amber);
        k.success = h(t.emerald);
        k.warning = h(t.warn);
        k.info = h(t.cyan);
        k.ring = h(t.amber.fade(0.7));
        k.selection = h(t.amber.fade(0.3));
        k.caret = h(t.text);
        k.link = h(t.amber);
        k.list_hover = h(t.pane_2);
        k.list_active = h(t.pane_2);
        k.popover = h(t.pane_2);
        k.scrollbar = Hsla::transparent_black();
        k.scrollbar_thumb = h(t.scrollbar_thumb);
        k.scrollbar_thumb_hover = h(t.scrollbar_thumb_hover);
        k.skeleton = h(t.pane_2);
        k.title_bar = h(t.titlebar);
        k.title_bar_border = h(t.line);
        k.status_bar = h(t.statusbar);
        k.sidebar = h(t.rail);
        k.sidebar_border = h(t.line);
        k.tab_bar = h(t.pane);
        k.tab = h(t.pane);
        k.tab_active = h(t.pane);
        k.tab_foreground = h(t.muted);
        k.tab_active_foreground = h(t.text);
        k.overlay = h(crate::color::rgba(5, 7, 10, 0.55));
        k.window_border = h(t.line);
    });
}
