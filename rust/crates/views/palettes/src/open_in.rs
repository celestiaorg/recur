//! `OpenInPalette.svelte`: open the selected workspace in an installed app (⌘O, File → Open
//! In…, the titlebar button), or copy its path. 1–9 pick a row while the filter is empty;
//! ⌘↩ or the pin makes an app the default, listed first.

use recur_ui::guard::Guarded;
use std::sync::Arc;

use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, Focusable, Image, ImageFormat,
    InteractiveElement, IntoElement, KeystrokeEvent, MouseMoveEvent, ObjectFit, ParentElement,
    Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled, StyledImage,
    Subscription, Window, div, img, px,
};
use recur_ui::logic::hover_gate::HoverGate;
use recur_ui::style::{palette_empty, palette_row, style_text_input};
use recur_ui::{IconName, TextStyled, Theme, icon};
use recur_view_core::AppState;
use recur_view_observatory::open_in_pref::{
    open_in_default, order_open_in_targets, set_open_in_default,
};

use crate::chrome::{self, Kind, cmd_or_ctrl, plain, row_digit, step};

pub const COPY_PATH: &str = "copy-path";

/// One installed target (`apps`).
#[derive(Clone)]
pub struct Target {
    pub id: String,
    pub name: String,
    pub icon: Option<Arc<Image>>,
}

/// A row: an app, or Copy path.
#[derive(Clone)]
pub struct Row {
    pub id: String,
    pub label: String,
    pub icon: IconName,
    pub image: Option<Arc<Image>>,
}

/// `ICONS`: the lucide fallback per known app id.
pub fn icon_for(id: &str) -> IconName {
    match id {
        "finder" => IconName::Folder,
        "cursor" | "vscode" | "zed" | "xcode" | "sublime" => IconName::Code,
        "ghostty" | "iterm" | "terminal" | "warp" => IconName::Terminal,
        "kaleidoscope" => IconName::FileDiff,
        COPY_PATH => IconName::Copy,
        _ => IconName::Box,
    }
}

/// `all`: the apps with the default first, then Copy path.
pub fn rows(apps: &[Target], default_id: &str) -> Vec<Row> {
    let mut out: Vec<Row> = order_open_in_targets(apps, default_id, |a| &a.id)
        .into_iter()
        .map(|a| Row {
            icon: icon_for(&a.id),
            id: a.id,
            label: a.name,
            image: a.icon,
        })
        .collect();
    out.push(Row {
        id: COPY_PATH.into(),
        label: "Copy path".into(),
        icon: IconName::Copy,
        image: None,
    });
    out
}

/// `flat`: rows whose label or id contains the query.
pub fn filter(rows: &[Row], q: &str) -> Vec<Row> {
    let query = q.trim().to_lowercase();
    if query.is_empty() {
        return rows.to_vec();
    }
    rows.iter()
        .filter(|r| r.label.to_lowercase().contains(&query) || r.id.to_lowercase().contains(&query))
        .cloned()
        .collect()
}

/// An app's icon from its data URL (`OpenInTarget.icon`).
fn data_image(url: &str) -> Option<Arc<Image>> {
    let (meta, data) = url.strip_prefix("data:")?.split_once(',')?;
    let format = match meta.split(';').next()? {
        "image/png" => ImageFormat::Png,
        "image/jpeg" | "image/jpg" => ImageFormat::Jpeg,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::Webp,
        "image/svg+xml" => ImageFormat::Svg,
        "image/bmp" => ImageFormat::Bmp,
        _ => return None,
    };
    let bytes = if meta.ends_with(";base64") {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .decode(data.trim())
            .ok()?
    } else {
        data.as_bytes().to_vec()
    };
    Some(Arc::new(Image::from_bytes(format, bytes)))
}

pub struct OpenInPalette {
    state: Entity<AppState>,
    input: Entity<InputState>,
    sel: usize,
    apps: Vec<Target>,
    ready: bool,
    hover: HoverGate,
    pin_hover: Option<String>,
    scroll: ScrollHandle,
    revealed: Option<(usize, Vec<String>)>,
    was_open: bool,
    _subs: Vec<Subscription>,
}

impl OpenInPalette {
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
        OpenInPalette {
            state,
            input,
            sel: 0,
            apps: Vec::new(),
            ready: false,
            hover: HoverGate::default(),
            pin_hover: None,
            scroll: ScrollHandle::new(),
            revealed: None,
            was_open: false,
            _subs: subs,
        }
    }

    fn open(&self, cx: &App) -> bool {
        self.state.read(cx).overlays.open_in
    }

    fn default_id(&self, cx: &App) -> String {
        open_in_default(self.state.read(cx).storage())
    }

    fn flat(&self, cx: &App) -> Vec<Row> {
        filter(
            &rows(&self.apps, &self.default_id(cx)),
            &self.input.read(cx).value(),
        )
    }

    fn clamp_sel(&mut self, cx: &App) {
        let n = self.flat(cx).len();
        if self.sel >= n {
            self.sel = n.saturating_sub(1);
        }
    }

    fn sync_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.open(cx);
        if open && !self.was_open {
            self.input.update(cx, |s, cx| s.set_value("", window, cx));
            self.sel = 0;
            self.ready = false;
            self.hover.reset();
            let focus = self.input.focus_handle(cx);
            window.defer(cx, move |window, cx| window.focus(&focus, cx));
            let services = self.state.read(cx).services().clone();
            cx.spawn(async move |this, cx| {
                let list = services.workspaces.list_open_in_targets().await;
                let _ = this.update(cx, |this, cx| {
                    this.apps = match list {
                        Ok(list) => list
                            .into_iter()
                            .map(|t| Target {
                                icon: data_image(&t.icon),
                                id: t.id,
                                name: t.name,
                            })
                            .collect(),
                        Err(_) => vec![Target {
                            id: "finder".into(),
                            name: "Finder".into(),
                            icon: None,
                        }],
                    };
                    this.ready = true;
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
            .update(cx, |s, cx| s.close_overlay(|o| o.open_in = false, cx));
    }

    fn make_default(&mut self, id: &str, cx: &mut Context<Self>) {
        if id.is_empty() || id == COPY_PATH {
            return;
        }
        self.state.update(cx, |s, cx| {
            set_open_in_default(s.storage_mut(), id);
            cx.notify();
        });
        self.sel = 0;
        cx.notify();
    }

    fn run(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(r) = self.flat(cx).get(i).cloned() else {
            return;
        };
        self.close(cx);
        if r.id == COPY_PATH {
            self.state
                .update(cx, |s, cx| s.dispatch_menu_action("copy-path", cx));
            return;
        }
        let (services, ws) = {
            let s = self.state.read(cx);
            (s.services().clone(), s.selected_workspace_id.clone())
        };
        if ws.is_empty() {
            return;
        }
        cx.spawn(async move |_, cx| {
            recur_view_core::call(
                cx,
                services.workspaces.open_workspace_in(&ws, &r.id),
                "Open failed",
            )
            .await;
        })
        .detach();
    }

    fn on_key(&mut self, ev: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.open(cx) || !self.input.focus_handle(cx).is_focused(window) {
            return false;
        }
        let k = &ev.keystroke;
        let n = self.flat(cx).len();
        match k.key.as_str() {
            "down" | "up" => {
                if !plain(k) {
                    return false;
                }
                self.sel = step(self.sel, n, k.key == "down");
                cx.notify();
                true
            }
            "enter" if cmd_or_ctrl(k) => {
                if let Some(r) = self.flat(cx).get(self.sel) {
                    let id = r.id.clone();
                    self.make_default(&id, cx);
                }
                true
            }
            "enter" => {
                self.run(self.sel, cx);
                true
            }
            "escape" => {
                self.close(cx);
                true
            }
            _ if self.input.read(cx).value().trim().is_empty() => match row_digit(k) {
                Some(d) if d - 1 < n => {
                    self.run(d - 1, cx);
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }
}

impl OpenInPalette {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        if !self.open(cx) {
            return div().into_any_element();
        }
        style_text_input(&self.input, cx);
        let t = Theme::of(cx).tokens;
        let flat = self.flat(cx);
        let default_id = self.default_id(cx);
        let filtered = !self.input.read(cx).value().trim().is_empty();
        let placeholder: SharedString = "Open workspace in…".into();
        let head = chrome::head(
            Kind::OpenIn,
            IconName::QueuePopOut,
            &self.input,
            &placeholder,
            "⌘O",
            window,
            cx,
        );
        let mut rows: Vec<AnyElement> = Vec::new();
        if !self.ready {
            rows.push(palette_empty("Looking for apps…", true, cx).into_any_element());
        } else if flat.is_empty() {
            let msg = if filtered {
                "No matching apps"
            } else {
                "No apps found"
            };
            rows.push(palette_empty(msg, true, cx).into_any_element());
        } else {
            for (idx, r) in flat.iter().enumerate() {
                let on = idx == self.sel;
                let is_default = r.id == default_id;
                let id = r.id.clone();
                let os = r.image.is_some();
                let tile = chrome::app_tile(on, os, cx).child(match &r.image {
                    Some(image) => img(image.clone())
                        .size(px(22.))
                        .object_fit(ObjectFit::Contain)
                        .into_any_element(),
                    None => icon(r.icon)
                        .size(px(13.))
                        .color(if on { t.amber } else { t.muted })
                        .into_any_element(),
                });
                let sel_id = id.clone();
                let mut row = palette_row(SharedString::from(format!("oi-row:{id}")), on, true, cx)
                    .gap(px(10.))
                    .debug_selector(move || {
                        format!("oi-row:{sel_id}{}", if on { ":sel" } else { "" })
                    })
                    .child(tile)
                    .child(chrome::label(
                        SharedString::from(format!("oi-pl:{id}")),
                        r.label.clone(),
                        Kind::OpenIn,
                        cx,
                    ));
                if is_default {
                    row = row.child(stretch_sub("default", cx));
                }
                let has_pin = r.id != COPY_PATH;
                if has_pin {
                    let pin_id = id.clone();
                    let hovered = self.pin_hover.as_deref() == Some(id.as_str());
                    let hover_id = id.clone();
                    row = row.child(
                        chrome::pin(
                            SharedString::from(format!("oi-pin:{id}")),
                            is_default,
                            on,
                            hovered,
                            cx,
                        )
                        .debug_selector({
                            let id = id.clone();
                            move || format!("oi-pin:{id}")
                        })
                        .on_hover(cx.guarded_listener(move |p, h: &bool, _, cx| {
                            p.pin_hover = h.then(|| hover_id.clone());
                            cx.notify();
                        }))
                        .on_click(cx.guarded_listener(
                            move |p, _, _, cx| {
                                cx.stop_propagation();
                                p.make_default(&pin_id, cx);
                            },
                        )),
                    );
                }
                if !filtered && idx < 9 {
                    row = row.child(chrome::digit_key(idx + 1, on, has_pin, cx));
                }
                rows.push(
                    row.on_mouse_move(cx.guarded_listener(move |p, ev: &MouseMoveEvent, _, cx| {
                        if p.hover
                            .allows(f32::from(ev.position.x), f32::from(ev.position.y))
                            && p.sel != idx
                        {
                            p.sel = idx;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.guarded_listener(move |p, _, _, cx| p.run(idx, cx)))
                    .into_any_element(),
                );
            }
        }
        let key = (self.sel, flat.iter().map(|r| r.id.clone()).collect());
        if self.revealed.as_ref() != Some(&key) {
            if self.ready && !flat.is_empty() {
                self.scroll.scroll_to_item(self.sel);
            }
            self.revealed = Some(key);
        }
        let list = chrome::list("oi-list", Kind::OpenIn, &self.scroll, rows);
        let foot = chrome::footer(
            Kind::OpenIn,
            &[
                ("↑↓", "navigate"),
                ("1–9", "open"),
                ("↵", "open"),
                ("⌘↩", "default"),
                ("esc", "close"),
            ],
            cx,
        );
        let state = self.state.clone();
        chrome::frame(
            Kind::OpenIn,
            Kind::OpenIn.max_vh(),
            "open-in-palette",
            vec![
                head.into_any_element(),
                list.into_any_element(),
                foot.into_any_element(),
            ],
            move |_, cx| state.update(cx, |s, cx| s.close_overlay(|o| o.open_in = false, cx)),
            window,
            cx,
        )
    }
}

impl Render for OpenInPalette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

/// `.open-in-palette .pal-row .pl + .psub`: the context after the label takes the free width
/// and ellipsizes.
pub fn stretch_sub(label: impl Into<SharedString>, cx: &App) -> gpui_kit::Div {
    let t = Theme::of(cx).tokens;
    div().flex_1().min_w_0().ml(px(8.)).overflow_hidden().child(
        recur_ui::text(label)
            .mono()
            .size(10.)
            .color(t.faint)
            .nowrap()
            .truncate(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, name: &str) -> Target {
        Target {
            id: id.into(),
            name: name.into(),
            icon: None,
        }
    }

    #[test]
    fn copy_path_trails_and_the_default_leads() {
        let apps = [app("finder", "Finder"), app("cursor", "Cursor")];
        let ids = |r: &[Row]| r.iter().map(|r| r.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&rows(&apps, "")), ["finder", "cursor", "copy-path"]);
        assert_eq!(
            ids(&rows(&apps, "cursor")),
            ["cursor", "finder", "copy-path"]
        );
        assert_eq!(rows(&apps, "")[2].label, "Copy path");
    }

    #[test]
    fn filters_by_label_or_id() {
        let apps = [app("vscode", "Visual Studio Code"), app("finder", "Finder")];
        let r = rows(&apps, "");
        let ids = |q: &str| filter(&r, q).into_iter().map(|r| r.id).collect::<Vec<_>>();
        assert_eq!(ids("VSC"), ["vscode"]);
        assert_eq!(ids("studio"), ["vscode"]);
        assert_eq!(ids("copy"), ["copy-path"]);
        assert_eq!(ids("  ").len(), 3);
    }

    #[test]
    fn known_apps_get_their_icon() {
        assert_eq!(icon_for("ghostty"), IconName::Terminal);
        assert_eq!(icon_for("kaleidoscope"), IconName::FileDiff);
        assert_eq!(icon_for("finder"), IconName::Folder);
        assert_eq!(icon_for("something"), IconName::Box);
    }

    #[test]
    fn decodes_png_data_urls() {
        assert!(data_image("data:image/png;base64,iVBORw0KGgo=").is_some());
        assert!(data_image("").is_none());
        assert!(data_image("https://x/y.png").is_none());
    }
}
