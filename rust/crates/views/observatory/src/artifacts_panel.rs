//! `ArtifactsPanel.svelte`: a workspace's artifacts (or a given list, for plan attachments)
//! with open, inline rename, reveal, and delete (armed for 3s, or confirmed from the context
//! menu). Refreshes when an artifact is created or presented.

use std::time::Duration;

use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, InteractiveElement, IntoElement,
    ParentElement, PromptButton, PromptLevel, Render, SharedString, StatefulInteractiveElement,
    Styled, Subscription, Task, Window, deferred, div, px,
};
use recur_engine::view::artifacts::{ArtifactRef, artifact_icon};
use recur_services::{AppEvent, Services};
use recur_ui::shadow::{css_box_shadow, with_shadow};
use recur_ui::style::anchored_at;
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::tooltip::titled;
use recur_ui::{IconName, Theme, hex, icon, rgba};
use recur_view_core::call;
use recur_view_review::style::hsla;

/// What the panel asks its host to do (`onOpen`, `onRenamed`, `onDeleted`).
#[derive(Clone, Debug, PartialEq)]
pub enum ArtifactsPanelEvent {
    Open(ArtifactRef),
    Renamed(ArtifactRef),
    Deleted(String),
}

struct Menu {
    x: f32,
    y: f32,
    row: ArtifactRef,
}

pub struct ArtifactsPanel {
    services: Services,
    workspace_id: String,
    /// `items`: a fixed list (plan attachments) instead of the workspace's artifacts.
    items: Option<Vec<ArtifactRef>>,
    empty: String,
    selected_id: String,
    listed: Vec<ArtifactRef>,
    loading: bool,
    err: String,
    menu: Option<Menu>,
    renaming: Option<(String, Entity<InputState>)>,
    armed_delete: String,
    arm_timer: Option<Task<()>>,
    hovered: Option<String>,
    load: Option<Task<()>>,
    _events: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<ArtifactsPanelEvent> for ArtifactsPanel {}

fn from_domain(a: &recur_domain::Artifact) -> ArtifactRef {
    ArtifactRef {
        id: a.id.clone(),
        title: a.title.clone(),
        kind: a.kind.to_string(),
        path: a.path.clone(),
    }
}

impl ArtifactsPanel {
    pub fn new(services: Services, workspace_id: &str, cx: &mut Context<Self>) -> Self {
        let mut p = ArtifactsPanel {
            services: services.clone(),
            workspace_id: String::new(),
            items: None,
            empty: "No artifacts yet. Agents create them with create_artifact.".into(),
            selected_id: String::new(),
            listed: Vec::new(),
            loading: false,
            err: String::new(),
            menu: None,
            renaming: None,
            armed_delete: String::new(),
            arm_timer: None,
            hovered: None,
            load: None,
            _events: None,
            _subs: Vec::new(),
        };
        let mut events = services.subscribe();
        p._events = Some(cx.spawn(async move |this, cx| {
            while let Some(ev) = events.recv().await {
                let ws = match &ev {
                    AppEvent::ArtifactCreated(e) | AppEvent::ArtifactPresented(e) => {
                        e.workspace_id.clone()
                    }
                    _ => continue,
                };
                let ok = this.update(cx, |p, cx| {
                    if p.items.is_none() && ws == p.workspace_id {
                        p.refresh(cx);
                    }
                });
                if ok.is_err() {
                    break;
                }
            }
        }));
        p.set_workspace(workspace_id, cx);
        p
    }

    /// A fixed list (`items`) with its empty text: plan attachments.
    pub fn with_items(mut self, items: Vec<ArtifactRef>, empty: &str) -> Self {
        self.items = Some(items);
        self.empty = empty.to_string();
        self
    }

    pub fn set_items(&mut self, items: Vec<ArtifactRef>, cx: &mut Context<Self>) {
        self.items = Some(items);
        cx.notify();
    }

    pub fn set_workspace(&mut self, workspace_id: &str, cx: &mut Context<Self>) {
        if self.workspace_id == workspace_id && self.load.is_some() {
            return;
        }
        self.workspace_id = workspace_id.to_string();
        self.refresh(cx);
    }

    pub fn set_selected(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.selected_id != id {
            self.selected_id = id.to_string();
            cx.notify();
        }
    }

    fn rows(&self) -> &[ArtifactRef] {
        self.items.as_deref().unwrap_or(&self.listed)
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.items.is_some() {
            return;
        }
        if self.workspace_id.is_empty() {
            self.listed.clear();
            cx.notify();
            return;
        }
        self.loading = true;
        self.err.clear();
        let svc = self.services.clone();
        let ws = self.workspace_id.clone();
        self.load = Some(cx.spawn(async move |this, cx| {
            let list = call(
                cx,
                svc.artifacts.list_artifacts(&ws),
                "Could not list artifacts",
            )
            .await;
            let _ = this.update(cx, |p, cx| {
                if p.workspace_id != ws {
                    return;
                }
                p.listed = list.unwrap_or_default().iter().map(from_domain).collect();
                p.loading = false;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn open_menu(
        &mut self,
        x: f32,
        y: f32,
        row: ArtifactRef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.items.is_some() {
            return;
        }
        let pad = 8.;
        let vw = f32::from(window.viewport_size().width);
        let vh = f32::from(window.viewport_size().height);
        let x = x.min(vw - 220. - pad).max(pad);
        let y = y.min(vh - 120. - pad).max(pad);
        self.menu = Some(Menu { x, y, row });
        cx.notify();
    }

    fn start_rename(&mut self, row: &ArtifactRef, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        let title = if row.title.is_empty() {
            row.id.clone()
        } else {
            row.title.clone()
        };
        let input = cx.new(|cx| {
            let mut s = InputState::new(window, cx);
            s.set_value(title, window, cx);
            s
        });
        recur_ui::style::style_text_input(&input, cx);
        let r = row.clone();
        self._subs = vec![cx.subscribe_in(
            &input,
            window,
            move |p, inp, ev: &InputEvent, _, cx| match ev {
                InputEvent::PressEnter { .. } => {
                    let v = inp.read(cx).value().to_string();
                    p.commit_rename(&r, v, cx);
                }
                InputEvent::Blur if p.renaming.as_ref().is_some_and(|(id, _)| *id == r.id) => {
                    let v = inp.read(cx).value().to_string();
                    p.commit_rename(&r, v, cx);
                }
                _ => {}
            },
        )];
        input.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        self.renaming = Some((row.id.clone(), input));
        cx.notify();
    }

    fn commit_rename(&mut self, row: &ArtifactRef, title: String, cx: &mut Context<Self>) {
        self.renaming = None;
        let title = title.trim().to_string();
        let current = if row.title.is_empty() {
            &row.id
        } else {
            &row.title
        };
        if title.is_empty() || title == *current {
            cx.notify();
            return;
        }
        let svc = self.services.clone();
        let (ws, id) = (self.workspace_id.clone(), row.id.clone());
        cx.spawn(async move |this, cx| {
            let next = call(
                cx,
                svc.artifacts.rename_artifact(&ws, &id, &title),
                "Could not rename artifact",
            )
            .await;
            let Some(next) = next else { return };
            let _ = this.update(cx, |p, cx| {
                cx.emit(ArtifactsPanelEvent::Renamed(from_domain(&next)));
                p.refresh(cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn delete_from_menu(&mut self, row: ArtifactRef, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        let name = if row.title.is_empty() {
            row.id.clone()
        } else {
            row.title.clone()
        };
        let answer = window.prompt(
            PromptLevel::Info,
            "Delete",
            Some(&format!("Delete “{name}”? This cannot be undone.")),
            &[PromptButton::cancel("Cancel"), PromptButton::ok("Delete")],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await.ok() != Some(1) {
                return;
            }
            let _ = this.update(cx, |p, cx| p.remove(row, cx));
        })
        .detach();
        cx.notify();
    }

    fn disarm(&mut self) {
        self.arm_timer = None;
        self.armed_delete.clear();
    }

    fn on_delete_click(&mut self, row: ArtifactRef, cx: &mut Context<Self>) {
        if self.armed_delete != row.id {
            self.armed_delete = row.id.clone();
            self.arm_timer = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(3000))
                    .await;
                let _ = this.update(cx, |p, cx| {
                    p.disarm();
                    cx.notify();
                });
            }));
            cx.notify();
            return;
        }
        self.disarm();
        self.remove(row, cx);
    }

    fn remove(&mut self, row: ArtifactRef, cx: &mut Context<Self>) {
        let svc = self.services.clone();
        let ws = self.workspace_id.clone();
        cx.spawn(async move |this, cx| {
            if call(
                cx,
                svc.artifacts.delete_artifact(&ws, &row.id),
                "Could not delete artifact",
            )
            .await
            .is_none()
            {
                return;
            }
            let _ = this.update(cx, |p, cx| {
                cx.emit(ArtifactsPanelEvent::Deleted(row.id.clone()));
                p.refresh(cx);
            });
        })
        .detach();
    }

    fn reveal(&mut self, row: &ArtifactRef, cx: &mut Context<Self>) {
        self.menu = None;
        let svc = self.services.clone();
        let (ws, id) = (self.workspace_id.clone(), row.id.clone());
        cx.spawn(async move |_, cx| {
            call(
                cx,
                svc.artifacts.reveal_artifact(&ws, &id),
                "Could not reveal artifact",
            )
            .await;
        })
        .detach();
        cx.notify();
    }

    fn row(&mut self, a: &ArtifactRef, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let armed = self.armed_delete == a.id;
        let hovered = self.hovered.as_deref() == Some(a.id.as_str());
        let reveal_del = hovered || armed;
        let title = if a.title.is_empty() {
            a.id.clone()
        } else {
            a.title.clone()
        };
        let kind = div()
            .flex_none()
            .opacity(0.55)
            .when(reveal_del && self.items.is_none(), |d| d.invisible())
            .child(
                text(a.kind.to_uppercase())
                    .size(11.)
                    .line_height(LineHeight::Normal)
                    .nowrap(),
            );
        let icon_el = icon(IconName::from_key(artifact_icon(&a.kind)).unwrap_or(IconName::File))
            .size(px(14.))
            .color(hsla(t.muted));
        let renaming = self
            .renaming
            .as_ref()
            .filter(|(id, _)| *id == a.id)
            .map(|(_, i)| i.clone());
        let body =
            if let Some(input) = renaming {
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .w_full()
                    .p(px(8.))
                    .rounded(px(8.))
                    .bg(hsla(t.pane_2))
                    .child(div().w(px(14.)).flex_none().child(icon_el))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .bg(hsla(t.pane))
                            .border_1()
                            .border_color(hsla(t.line_bright))
                            .rounded(px(4.))
                            .px(px(4.))
                            .py(px(1.))
                            .child(recur_ui::style::text_field(
                                &input,
                                &SharedString::default(),
                                recur_ui::style::UA_PLACEHOLDER,
                                recur_ui::fonts::Family::Ui,
                                13.,
                                window,
                                cx,
                            )),
                    )
                    .child(kind)
                    .into_any_element()
            } else {
                let (open, menu_row) = (a.clone(), a.clone());
                div()
                    .id(SharedString::from(format!("art-{}", a.id)))
                    .debug_selector({
                        let id = a.id.clone();
                        move || format!("art-row:{id}")
                    })
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .w_full()
                    .p(px(8.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .when(a.id == self.selected_id || hovered, |d| {
                        d.bg(hsla(t.pane_2))
                    })
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(ArtifactsPanelEvent::Open(open.clone()))
                    }))
                    .on_mouse_down(
                        gpui_kit::MouseButton::Right,
                        cx.listener(move |p, e: &gpui_kit::MouseDownEvent, window, cx| {
                            p.open_menu(
                                f32::from(e.position.x),
                                f32::from(e.position.y),
                                menu_row.clone(),
                                window,
                                cx,
                            )
                        }),
                    )
                    .child(div().w(px(14.)).flex_none().child(icon_el))
                    .child(
                        div().flex_1().min_w_0().child(
                            text(title.clone())
                                .size(13.)
                                .line_height(LineHeight::Normal)
                                .truncate(),
                        ),
                    )
                    .child(kind)
                    .into_any_element()
            };
        let mut item = div()
            .id(SharedString::from(format!("art-item-{}", a.id)))
            .relative()
            .on_hover(cx.listener({
                let id = a.id.clone();
                move |p, on: &bool, _, cx| {
                    if *on {
                        p.hovered = Some(id.clone());
                    } else {
                        if p.hovered.as_deref() == Some(id.as_str()) {
                            p.hovered = None;
                        }
                        if p.armed_delete == id {
                            p.disarm();
                        }
                    }
                    cx.notify();
                }
            }))
            .child(body);
        if self.items.is_none() && self.renaming.as_ref().is_none_or(|(id, _)| *id != a.id) {
            let row = a.clone();
            let label = if armed {
                format!("Confirm delete {title}")
            } else {
                format!("Delete {title}")
            };
            let _ = label;
            let del = div()
                .id(SharedString::from(format!("art-del-{}", a.id)))
                .debug_selector({
                    let id = a.id.clone();
                    move || format!("art-del:{id}")
                })
                .absolute()
                .top_0()
                .bottom_0()
                .right(px(6.))
                .flex()
                .items_center()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(22.))
                        .rounded(px(4.))
                        .when(armed, |d| {
                            d.bg(hsla(t.coral)).text_color(hsla(hex(0x0a0c10)))
                        })
                        .when(!armed, |d| {
                            d.text_color(hsla(t.faint))
                                .hover(move |s| s.text_color(hsla(t.coral)))
                        })
                        .cursor_pointer()
                        .child(icon(IconName::Trash).size(px(12.))),
                )
                .when(!reveal_del, |d| d.invisible())
                .on_click(cx.listener(move |p, _, _, cx| p.on_delete_click(row.clone(), cx)));
            item = item.child(titled(
                SharedString::from(format!("art-del-t-{}", a.id)),
                if armed { "Confirm delete" } else { "Delete" },
                del,
            ));
        }
        item.into_any_element()
    }

    fn menu_el(&self, m: &Menu, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let btn = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(16.))
                .w_full()
                .px(px(10.))
                .py(px(7.))
                .rounded(px(5.))
                .cursor_pointer()
                .text_color(hsla(t.text))
                .hover(move |s| s.bg(hsla(t.pane_2)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(text(label).size(12.).nowrap()),
                )
        };
        let (r1, r2, r3) = (m.row.clone(), m.row.clone(), m.row.clone());
        let panel = div()
            .debug_selector(|| "art-ctx".into())
            .min_w(px(220.))
            .max_w(px(300.))
            .p(px(4.))
            .flex()
            .flex_col()
            .child(
                btn("art-ctx-rename", "Rename…")
                    .on_click(cx.listener(move |p, _, window, cx| p.start_rename(&r1, window, cx))),
            )
            .child(
                btn("art-ctx-reveal", "Reveal in Finder")
                    .on_click(cx.listener(move |p, _, _, cx| p.reveal(&r2, cx))),
            )
            .child(div().h(px(1.)).mx(px(6.)).my(px(4.)).bg(hsla(t.line)))
            .child(btn("art-ctx-delete", "Delete").on_click(
                cx.listener(move |p, _, window, cx| p.delete_from_menu(r3.clone(), window, cx)),
            ));
        let card = recur_ui::style::card(t.pane, t.line_bright, 8.).child(panel);
        deferred(anchored_at(
            m.x,
            m.y,
            with_shadow(
                card,
                &[css_box_shadow(0., 12., 32., 0., rgba(0, 0, 0, 0.45))],
                8.,
            )
            .id("art-ctx-shield")
            .occlude()
            .on_mouse_down_out(cx.listener(|p, _, _, cx| {
                p.menu = None;
                cx.notify();
            })),
        ))
        .with_priority(80)
        .into_any_element()
    }
}

impl Render for ArtifactsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx).tokens;
        let empty = |s: String| {
            div().px(px(12.)).py(px(16.)).opacity(0.65).child(
                text(s)
                    .size(12.)
                    .line_height(recur_ui::text::LineHeight::Scale(1.4)),
            )
        };
        let rows: Vec<ArtifactRef> = self.rows().to_vec();
        let mut root = div()
            .debug_selector(|| "art-panel".into())
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .min_h_0()
            .text_color(hsla(t.text))
            .on_key_down(cx.listener(|p, e: &gpui_kit::KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" {
                    if p.menu.is_some() {
                        p.menu = None;
                        cx.notify();
                    } else if p.renaming.is_some() {
                        p.renaming = None;
                        cx.notify();
                    }
                }
            }));
        if !self.err.is_empty() {
            root = root.child(empty(self.err.clone()));
        } else if self.loading && rows.is_empty() {
            root = root.child(empty("Loading…".into()));
        } else if rows.is_empty() {
            root = root.child(empty(self.empty.clone()));
        } else {
            let items: Vec<AnyElement> = rows.iter().map(|a| self.row(a, window, cx)).collect();
            root = root.child(
                recur_ui::widgets::scroll_area("art-list")
                    .child(div().p(px(6.)).flex().flex_col().children(items)),
            );
        }
        if let Some(m) = &self.menu {
            let el = self.menu_el(m, cx);
            root = root.child(el);
        }
        root
    }
}
