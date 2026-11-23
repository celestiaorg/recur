//! The browser view of the center column: one page per workspace, kept across shells in
//! [`Browsers`] and dropped with the workspace, under an address bar. It opens on the
//! project's app URL at the workspace's port; when nothing answers there it offers to start
//! the project's dev run and keeps trying until the page loads. A capture (⌘⇧C or the bar's
//! toggle) becomes an image draft carrying its page context, emitted for the chat composer.

use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, Global,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div, px,
};
use recur_content::BrowserEvent;
use recur_content::browser::{
    AVAILABLE, BrowserCapture, BrowserPage, BrowserState, LoadError, capture_label,
    format_capture_context, normalize_url, parse_capture_source, resolve_capture_source,
};
use recur_engine::view::chat::{ChatAttachmentDraft, new_attachment_id};
use recur_ui::guard::Guarded as _;
use recur_ui::isolated::FieldView;
use recur_ui::text::{TextStyled as _, text};
use recur_ui::tooltip::titled;
use recur_ui::widgets::ToastKind;
use recur_ui::{IconName, Theme};
use recur_view_core::{AppState, toast};
use recur_view_review::style::hsla;

use crate::style::{icon_btn, obs_empty};

/// Each workspace's browser, created the first time it shows.
#[derive(Default)]
pub struct Browsers(HashMap<String, Entity<BrowserSurface>>);

impl Global for Browsers {}

/// The workspace's browser, created on first use.
pub(crate) fn browser_for(
    state: &Entity<AppState>,
    workspace_id: &str,
    window: &mut Window,
    cx: &mut App,
) -> Entity<BrowserSurface> {
    if let Some(b) = cx.default_global::<Browsers>().0.get(workspace_id) {
        return b.clone();
    }
    let b = cx.new(|cx| BrowserSurface::new(state.clone(), workspace_id, window, cx));
    cx.global_mut::<Browsers>()
        .0
        .insert(workspace_id.to_string(), b.clone());
    b
}

/// The workspace's browser if it has been shown.
pub(crate) fn existing_browser(workspace_id: &str, cx: &App) -> Option<Entity<BrowserSurface>> {
    cx.try_global::<Browsers>()?.0.get(workspace_id).cloned()
}

/// The workspace is gone: so is its browser, with its history.
pub(crate) fn drop_browser(workspace_id: &str, cx: &mut App) {
    if existing_browser(workspace_id, cx).is_some() {
        cx.global_mut::<Browsers>().0.remove(workspace_id);
    }
}

/// How long a started dev server gets to answer.
const WAIT_TRIES: u32 = 60;
const WAIT_EVERY: Duration = Duration::from_millis(1500);

pub struct BrowserSurface {
    state: Entity<AppState>,
    workspace_id: String,
    page: Option<Entity<BrowserPage>>,
    nav: BrowserState,
    address: Entity<InputState>,
    field: Entity<FieldView>,
    editing: bool,
    setup_error: Option<String>,
    /// Started the dev run: retrying until the page loads.
    waiting: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<ChatAttachmentDraft> for BrowserSurface {}

impl BrowserSurface {
    fn new(
        state: Entity<AppState>,
        workspace_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let address =
            recur_ui::style::text_input(&SharedString::from("Enter a URL (⌘L)"), window, cx);
        let start = start_url(&state, workspace_id, cx);
        address.update(cx, |s, cx| s.set_value(start.clone(), window, cx));
        let input = address.clone();
        let field = FieldView::new(
            move |window, cx| {
                recur_ui::style::style_text_input(&input, cx);
                recur_ui::style::text_field(
                    &input,
                    &SharedString::from("Enter a URL (⌘L)"),
                    recur_ui::style::UA_PLACEHOLDER,
                    recur_ui::fonts::Family::Mono,
                    12.,
                    window,
                    cx,
                )
                .into_any_element()
            },
            cx,
        );
        let subs = vec![cx.guarded_subscribe_in(
            &address,
            window,
            |b: &mut Self, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } => b.submit(window, cx),
                InputEvent::Focus => b.editing = true,
                InputEvent::Blur => {
                    b.editing = false;
                    b.show_url(window, cx);
                }
                _ => {}
            },
        )];
        let mut b = BrowserSurface {
            state: state.clone(),
            workspace_id: workspace_id.to_string(),
            page: None,
            nav: BrowserState::default(),
            address,
            field,
            editing: false,
            setup_error: None,
            waiting: None,
            _subs: subs,
        };
        if AVAILABLE {
            b.open_page(start, window, cx);
        }
        b
    }

    /// Creates the page once Recur's server ports are known, so it never loads before they
    /// are blocked.
    fn open_page(&mut self, start: String, window: &mut Window, cx: &mut Context<Self>) {
        let services = self.state.read(cx).services().clone();
        cx.spawn_in(window, async move |this, cx| {
            let ports = services.observatory.recur_server_ports().await;
            let _ = this.update_in(cx, |b, window, cx| {
                let ports = match ports {
                    Ok(p) => p,
                    Err(e) => {
                        b.setup_error = Some(format!("Could not open the browser: {}", e.message));
                        cx.notify();
                        return;
                    }
                };
                let page = cx.new(|cx| BrowserPage::new(&ports, cx));
                b._subs
                    .push(cx.guarded_subscribe_in(&page, window, Self::on_page_event));
                if !start.is_empty() {
                    page.update(cx, |p, cx| p.navigate(&start, cx));
                }
                b.page = Some(page);
                cx.notify();
            });
        })
        .detach();
    }

    fn on_page_event(
        &mut self,
        page: &Entity<BrowserPage>,
        ev: &BrowserEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            BrowserEvent::FocusAddress => self.focus_address(window, cx),
            BrowserEvent::Captured(c) => {
                page.read(cx).release_keyboard();
                let draft = self.capture_draft(c, cx);
                cx.emit(draft);
            }
            BrowserEvent::CaptureFailed => {
                toast(ToastKind::Error, "Could not capture the page", cx);
            }
            BrowserEvent::Changed => {
                let nav = page.read(cx).state().clone();
                if nav == self.nav {
                    cx.notify();
                    return;
                }
                let loaded = !nav.loading && nav.error.is_none() && !nav.url.is_empty();
                if loaded {
                    self.waiting = None;
                }
                self.nav = nav;
                self.show_url(window, cx);
                cx.notify();
            }
        }
    }

    /// The address bar follows the page, except while it is being edited.
    fn show_url(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing {
            return;
        }
        let url = match (&self.nav.error, &self.page) {
            (None, _) if !self.nav.url.is_empty() => self.nav.url.clone(),
            (_, Some(p)) => p.read(cx).requested().to_string(),
            _ => return,
        };
        if *self.address.read(cx).value() != url {
            self.address
                .update(cx, |s, cx| s.set_value(url, window, cx));
        }
    }

    /// The capture as an image draft whose context names its source in the worktree.
    fn capture_draft(&self, c: &BrowserCapture, cx: &App) -> ChatAttachmentDraft {
        let root = self
            .state
            .read(cx)
            .workspace(&self.workspace_id)
            .map(|w| w.worktree_path.clone())
            .unwrap_or_default();
        let source =
            parse_capture_source(&c.source, &root).and_then(|s| resolve_capture_source(s, &root));
        use base64::Engine as _;
        ChatAttachmentDraft {
            id: new_attachment_id(chrono::Utc::now().timestamp_millis()),
            name: capture_label(&c.pick, source.as_ref()),
            mime: "image/png".into(),
            data_url: format!(
                "data:image/png;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(&c.png)
            ),
            capture: format_capture_context(&c.pick, &c.url, source.as_ref()),
        }
    }

    /// ⌘⇧C: starts or stops the capture picker.
    pub fn toggle_capture(&mut self, cx: &mut Context<Self>) {
        self.with_page(cx, |p, cx| p.toggle_capture(cx));
    }

    pub fn picking(&self, cx: &App) -> bool {
        self.page.as_ref().is_some_and(|p| p.read(cx).picking())
    }

    /// ⌘L: the address bar, its text selected.
    pub fn focus_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = &self.page {
            p.read(cx).release_keyboard();
        }
        self.address.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = normalize_url(self.address.read(cx).value().as_ref());
        let Some(page) = self.page.clone().filter(|_| !url.is_empty()) else {
            return;
        };
        self.waiting = None;
        self.editing = false;
        self.address
            .update(cx, |s, cx| s.set_value(url.clone(), window, cx));
        window.blur(cx);
        page.update(cx, |p, cx| p.navigate(&url, cx));
    }

    fn with_page(
        &self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut BrowserPage, &mut Context<BrowserPage>),
    ) {
        if let Some(p) = &self.page {
            p.update(cx, f);
        }
    }

    /// Starts the project's dev run and retries the page until it answers.
    fn start_dev(&mut self, cx: &mut Context<Self>) {
        let ws = self.workspace_id.clone();
        let run = {
            let s = self.state.read(cx);
            recur_view_core::run::project_for_workspace(&ws, &s.workspaces, &s.projects)
                .map(recur_domain::verify_dev_run)
                .filter(|r| !r.cmd.trim().is_empty())
        };
        if !self
            .state
            .update(cx, |s, cx| s.request_run_dev(&ws, run, cx))
        {
            return;
        }
        self.wait_for_page(cx);
    }

    fn wait_for_page(&mut self, cx: &mut Context<Self>) {
        self.waiting = Some(cx.spawn(async move |this, cx| {
            for _ in 0..WAIT_TRIES {
                cx.background_executor().timer(WAIT_EVERY).await;
                let Ok(retry) = this.update(cx, |b, cx| {
                    let stuck = matches!(b.nav.error, Some(LoadError::Unreachable));
                    if stuck {
                        b.with_page(cx, |p, cx| p.retry(cx));
                    }
                    stuck || b.nav.loading
                }) else {
                    return;
                };
                if !retry {
                    break;
                }
            }
            let _ = this.update(cx, |b, cx| {
                b.waiting = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn has_dev_run(&self, cx: &App) -> bool {
        let s = self.state.read(cx);
        recur_view_core::run::project_for_workspace(&self.workspace_id, &s.workspaces, &s.projects)
            .is_some_and(|p| !recur_domain::verify_dev_cmd(p).trim().is_empty())
    }

    /// The URL the page was last asked to load.
    #[cfg(test)]
    pub(crate) fn requested(&self, cx: &App) -> String {
        self.page
            .as_ref()
            .map(|p| p.read(cx).requested().to_string())
            .unwrap_or_default()
    }

    /// The page's title, for the head over the view.
    pub fn title(&self) -> &str {
        &self.nav.title
    }

    fn render_bar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let has_page = self.page.is_some();
        let dis = |on: bool| (!on).then_some(t.bg);
        let nav = &self.nav;
        let back =
            titled(
                "browser-back-t",
                "Back",
                icon_btn(
                    "browser-back",
                    IconName::ChevronLeft,
                    false,
                    dis(nav.can_go_back),
                    cx,
                )
                .debug_selector(|| "browser-back".into())
                .when(nav.can_go_back, |d| {
                    d.on_click(cx.guarded_listener(|b: &mut Self, _, _, cx| {
                        b.with_page(cx, |p, cx| p.back(cx))
                    }))
                }),
            );
        let forward = titled(
            "browser-forward-t",
            "Forward",
            icon_btn(
                "browser-forward",
                IconName::ChevronRight,
                false,
                dis(nav.can_go_forward),
                cx,
            )
            .debug_selector(|| "browser-forward".into())
            .when(nav.can_go_forward, |d| {
                d.on_click(cx.guarded_listener(|b: &mut Self, _, _, cx| {
                    b.with_page(cx, |p, cx| p.forward(cx))
                }))
            }),
        );
        let loading = nav.loading;
        let reload = titled(
            "browser-reload-t",
            if loading { "Stop" } else { "Reload" },
            icon_btn(
                "browser-reload",
                if loading {
                    IconName::X
                } else {
                    IconName::RefreshCw
                },
                false,
                dis(has_page),
                cx,
            )
            .debug_selector(|| "browser-reload".into())
            .when(has_page, |d| {
                d.on_click(cx.guarded_listener(move |b: &mut Self, _, _, cx| {
                    b.with_page(cx, |p, cx| if loading { p.stop(cx) } else { p.reload(cx) })
                }))
            }),
        );
        let can_capture = has_page && !self.nav.url.is_empty() && self.nav.error.is_none();
        let picking = self.picking(cx);
        let capture = titled(
            "browser-capture-t",
            "Capture an element or drag an area into chat (⌘⇧C)",
            icon_btn(
                "browser-capture",
                IconName::SquareMousePointer,
                picking,
                dis(can_capture),
                cx,
            )
            .debug_selector(|| "browser-capture".into())
            .when(can_capture, |d| {
                d.on_click(cx.guarded_listener(|b: &mut Self, _, _, cx| b.toggle_capture(cx)))
            }),
        );
        let address = div()
            .id("browser-address")
            .debug_selector(|| "browser-address".into())
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .h(px(26.))
            .px(px(8.))
            .ml(px(4.))
            .rounded(px(6.))
            .border_1()
            .border_color(hsla(t.line))
            .bg(hsla(t.pane_2))
            .capture_key_down(cx.guarded_listener(
                |b: &mut Self, e: &gpui_kit::KeyDownEvent, window, cx| {
                    if e.keystroke.key == "escape" {
                        cx.stop_propagation();
                        b.editing = false;
                        b.show_url(window, cx);
                        window.blur(cx);
                    }
                },
            ))
            .child(FieldView::element(
                &self.field,
                gpui_kit::StyleRefinement::default().flex_1().h(px(16.)),
            ));
        div()
            .debug_selector(|| "browser-bar".into())
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.))
            .h(px(36.))
            .px(px(6.))
            .border_b_1()
            .border_color(hsla(t.line))
            .child(back)
            .child(forward)
            .child(reload)
            .child(capture)
            .child(address)
            .into_any_element()
    }

    fn render_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let empty = |msg: String, cx: &App| {
            obs_empty(msg, true, cx).debug_selector(|| "browser-empty".into())
        };
        if !AVAILABLE {
            return empty("The browser is not available on Linux yet.".into(), cx)
                .into_any_element();
        }
        if let Some(e) = &self.setup_error {
            return empty(e.clone(), cx).into_any_element();
        }
        let Some(page) = self.page.clone() else {
            return div().flex_1().into_any_element();
        };
        let requested = page.read(cx).requested().to_string();
        if requested.is_empty() {
            return empty("Enter a URL to open it here.".into(), cx).into_any_element();
        }
        let message = match &self.nav.error {
            _ if self.waiting.is_some() => format!("Waiting for {requested} to answer…"),
            Some(LoadError::Unreachable) => format!("Nothing is answering at {requested}."),
            Some(LoadError::Blocked) => {
                "Recur's own servers cannot be opened in the browser.".to_string()
            }
            Some(LoadError::Failed(m)) => m.clone(),
            None => {
                return div()
                    .debug_selector(|| "browser-page".into())
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(page)
                    .into_any_element();
            }
        };
        let unreachable = matches!(self.nav.error, Some(LoadError::Unreachable));
        let start = (unreachable && self.waiting.is_none() && self.has_dev_run(cx)).then(|| {
            recur_ui::style::primary_button("browser-start-dev", None, cx)
                .ml_0()
                .debug_selector(|| "browser-start-dev".into())
                .child(recur_ui::style::button_label("Start dev server", 13., 600.))
                .on_click(cx.guarded_listener(|b: &mut Self, _, _, cx| b.start_dev(cx)))
        });
        let retry = (self.waiting.is_none()).then(|| {
            recur_ui::style::rbtn("browser-retry", recur_ui::style::Rbtn::Default, None, cx)
                .debug_selector(|| "browser-retry".into())
                .child(recur_ui::style::button_label("Try again", 12.5, 500.))
                .on_click(cx.guarded_listener(|b: &mut Self, _, _, cx| {
                    b.with_page(cx, |p, cx| p.retry(cx))
                }))
        });
        div()
            .debug_selector(|| "browser-empty".into())
            .flex()
            .flex_col()
            .flex_1()
            .items_center()
            .gap(px(14.))
            .px(px(24.))
            .py(px(48.))
            .child(
                text(message)
                    .size(12.)
                    .color(t.faint)
                    .align(recur_ui::text::Align::Center),
            )
            .child(div().flex().gap(px(8.)).children(start).children(retry))
            .into_any_element()
    }

    fn render_view(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let bar = self.render_bar(cx);
        let body = self.render_body(cx);
        div()
            .id("obs-browser")
            .debug_selector(|| "obs-browser".into())
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .bg(hsla(t.bg))
            .child(bar)
            .child(body)
            .into_any_element()
    }
}

impl Render for BrowserSurface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

/// The project's app URL at the workspace's port.
fn start_url(state: &Entity<AppState>, workspace_id: &str, cx: &App) -> String {
    let s = state.read(cx);
    let port = s.workspace(workspace_id).map_or(0, |w| w.port);
    let app_url =
        recur_view_core::run::project_for_workspace(workspace_id, &s.workspaces, &s.projects)
            .map_or("", |p| p.app_url.as_str());
    recur_domain::browser_url(app_url, port)
}
