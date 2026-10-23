//! `Welcome.svelte` (the `welcome` route) with `_welcome.scss`: the first-run checklist.
//! Connect an agent, add a project, start the first task, then the explore list. The flow is
//! derived from live state, so it resumes wherever the user left off; agent probes run only
//! while the page is shown (and again on each return to it).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Div, Entity, FocusHandle, Focusable, Hsla,
    InteractiveElement, IntoElement, ParentElement, Render, ScrollHandle, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Subscription, Task, Window, div, px,
};
use recur_domain::{PROVIDER_CLAUDE_CODE, PROVIDER_CODEX, PROVIDER_CURSOR, Workspace};
use recur_services::AppEvent;
use recur_services::api::{CodexStatus, CursorStatus};
use recur_ui::fonts::{Family, weight};
use recur_ui::style::{Tbtn, TbtnSize, button_label, is_focus_visible, outline, tbtn};
use recur_ui::text::{InlineBox, LineHeight, Text, TextStyled, span, text};
use recur_ui::widgets::{PicBox, ToastKind, project_pic};
use recur_ui::{Color, IconName, Theme, Tokens, icon, motion, provider_icon};
use recur_view_core::models::provider_enabled;
use recur_view_core::modes::{ChatMode, ws_has_run};
use recur_view_core::{NewTaskDraft, ViewContext, ViewName, call, toast};

use crate::wordmark::wordmark;

/// `EXAMPLES`: each mode's one-liner and sample prompt.
fn example(m: ChatMode) -> (&'static str, &'static str) {
    match m {
        ChatMode::Ask => (
            "Answers about your code. No edits.",
            "What are the riskiest parts of this codebase, and why?",
        ),
        ChatMode::Plan => (
            "An approach before any change.",
            "Plan how to add end-to-end tests for the main user flow.",
        ),
        ChatMode::Agent => (
            "Edits files and runs checks with you.",
            "Refactor the largest function in this repo into smaller pieces.",
        ),
        ChatMode::Review => (
            "Findings on a diff.",
            "Review the last commit for bugs and missing tests.",
        ),
        ChatMode::Loop => (
            "A whole process, hands-off: coding, review cycles, docs, or your own.",
            "Audit error handling across the codebase, fix what is unsafe, and re-review until clean.",
        ),
    }
}

/// `EXPLORE`: the rows of "Worth knowing about".
const EXPLORE: [(IconName, &str, &str); 4] = [
    (
        IconName::Inbox,
        "Inbox",
        "What needs you, across every workspace.",
    ),
    (
        IconName::GitPullRequest,
        "Pull requests",
        "Review, comment, and merge without leaving Recur.",
    ),
    (
        IconName::RefreshCw,
        "Loops",
        "Pick the default loop and how cautious it should be.",
    ),
    (
        IconName::Braces,
        "Bundles",
        "Commands, prompts, grammars, and themes you can extend.",
    ),
];

/// `taskStarted`: the user has sent something to an agent (a loop engaged, a finished chat
/// turn, or the first message renaming the workspace off its placeholder). The New-workspace
/// modal eagerly creates (and on cancel deletes) a record, so a bare record does not count.
pub fn task_started(w: &Workspace) -> bool {
    let goal = w.goal.trim();
    !w.archived
        && !w.is_main
        && w.host_pr_id.trim().is_empty()
        && (ws_has_run(w) || w.last_activity_at > 0 || (!goal.is_empty() && goal != "New Workspace"))
}

/// The probe results and the checklist inputs `step` reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Probe {
    pub claude_on: bool,
    pub codex_on: bool,
    pub cursor_on: bool,
    pub detected: String,
    pub detect_failed: bool,
    pub codex: Option<CodexStatus>,
    pub cursor: Option<CursorStatus>,
    pub claude_checked: bool,
    pub codex_checked: bool,
    pub cursor_checked: bool,
    pub cursor_key: bool,
}

impl Probe {
    pub fn claude_ready(&self) -> bool {
        self.claude_on && !self.detected.is_empty() && !self.detect_failed
    }
    pub fn cursor_signed_in(&self) -> bool {
        self.cursor_on && self.cursor_key
    }
    pub fn cursor_needs_sdk(&self) -> bool {
        self.cursor_signed_in()
            && self.cursor_checked
            && !self.cursor.as_ref().is_some_and(|c| c.bridge_installed)
    }
    pub fn cursor_ready(&self) -> bool {
        self.cursor_signed_in() && self.cursor.as_ref().is_some_and(|c| c.bridge_installed)
    }
    pub fn codex_ready(&self) -> bool {
        self.codex_on && self.codex.as_ref().is_some_and(|c| c.authenticated)
    }
    pub fn has_provider(&self) -> bool {
        self.claude_ready() || self.cursor_ready() || self.codex_ready()
    }
    /// Until the first probes settle, a started task is taken as proof an agent works.
    pub fn probing(&self) -> bool {
        (self.claude_on && !self.claude_checked)
            || (self.codex_on && !self.codex_checked)
            || (self.cursor_on && !self.cursor_checked)
    }
    /// `claudeDetail`: the detector's line without its `Claude Code ·` prefix
    /// (`/^Claude Code(\s*·\s*)?/`).
    pub fn claude_detail(&self) -> String {
        let d = match self.detected.strip_prefix("Claude Code") {
            Some(rest) => rest
                .trim_start()
                .strip_prefix('·')
                .map_or(rest, str::trim_start),
            None => self.detected.as_str(),
        };
        if d.is_empty() {
            "Signed in".into()
        } else {
            d.to_string()
        }
    }
    pub fn codex_detail(&self) -> String {
        let c = self.codex.as_ref();
        [c.map(|c| c.email.as_str()), c.map(|c| c.login_method.as_str())]
            .into_iter()
            .flatten()
            .find(|s| !s.is_empty())
            .unwrap_or("Signed in")
            .to_string()
    }
    pub fn cursor_detail(&self) -> String {
        let c = self.cursor.as_ref();
        [c.map(|c| c.email.as_str()), c.map(|c| c.account.as_str())]
            .into_iter()
            .flatten()
            .find(|s| !s.is_empty())
            .unwrap_or("Signed in")
            .to_string()
    }
}

/// `step`: 1 connect, 2 add a project, 3 first task, 4 done.
pub fn step(probe: &Probe, skip_agent: bool, has_git_project: bool, has_workspace: bool) -> u8 {
    if !(probe.has_provider() || skip_agent || (probe.probing() && has_workspace)) {
        1
    } else if !has_git_project {
        2
    } else if !has_workspace {
        3
    } else {
        4
    }
}

pub struct WelcomeView {
    vcx: ViewContext,
    probe: Probe,
    skip_agent: bool,
    installing: bool,
    install_pct: i64,
    install_msg: String,
    /// The page was shown with settings loaded (the probe effect's trigger).
    armed: bool,
    shown_at: Option<Instant>,
    explore_at: Option<Instant>,
    scroll: ScrollHandle,
    focus: FocusHandle,
    buttons: HashMap<String, FocusHandle>,
    probes: Vec<Task<()>>,
    _subs: Vec<Subscription>,
    _events: Task<()>,
}

impl Focusable for WelcomeView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// `.ws-chip`.
fn chip(muted: bool, t: &Tokens) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .h(px(22.))
        .pl(px(if muted { 8. } else { 6. }))
        .pr(px(if muted { 8. } else { 9. }))
        .rounded_full()
        .border(px(1.))
        .border_color(Hsla::from(t.line))
        .bg(t.pane)
        .text_color(if muted { t.muted } else { t.text })
}

fn chip_label(label: impl Into<SharedString>, color: Color) -> Text {
    text(label)
        .size(12.)
        .line_height(LineHeight::Scale(1.))
        .color(color)
        .nowrap()
}

/// `.ws-chip-ico`: a 13px muted glyph box.
fn chip_ico(el: impl IntoElement, t: &Tokens) -> Div {
    div()
        .flex_none()
        .size(px(13.))
        .flex()
        .items_center()
        .justify_center()
        .text_color(t.muted)
        .child(el)
}

impl WelcomeView {
    pub fn new(vcx: &ViewContext, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let subs = vec![cx.observe(&vcx.state, |this, _, cx| this.sync(cx))];
        // `cursor.install` progress while InstallCursorBridge runs.
        let mut events = vcx.services.subscribe();
        let ev = cx.spawn(async move |this, cx| {
            while let Some(ev) = events.recv().await {
                if let AppEvent::CursorInstall(p) = ev
                    && this
                        .update(cx, |v, cx| {
                            v.install_msg = p.message.clone();
                            v.install_pct = p.percent;
                            cx.notify();
                        })
                        .is_err()
                {
                    break;
                }
            }
        });
        let mut v = WelcomeView {
            vcx: vcx.clone(),
            probe: Probe::default(),
            skip_agent: false,
            installing: false,
            install_pct: -1,
            install_msg: String::new(),
            armed: false,
            shown_at: None,
            explore_at: None,
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            buttons: HashMap::new(),
            probes: Vec::new(),
            _subs: subs,
            _events: ev,
        };
        v.sync(cx);
        v
    }

    /// The `$effect` on `$view === 'welcome' && settingsLoaded`, and the `.on` entrance.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let (on, loaded) = {
            let s = self.vcx.state.read(cx);
            (s.view == ViewName::Welcome, s.settings.is_some())
        };
        if on && self.shown_at.is_none() {
            self.shown_at = Some(Instant::now());
        } else if !on {
            self.shown_at = None;
            self.explore_at = None;
        }
        let want = on && loaded;
        if want && !self.armed {
            self.check_agents(cx);
        }
        self.armed = want;
        cx.notify();
    }

    fn toggles(&mut self, cx: &App) {
        let s = self.vcx.state.read(cx);
        let disabled = s
            .settings
            .as_ref()
            .map(|s| s.disabled_providers.clone())
            .unwrap_or_default();
        self.probe.claude_on = provider_enabled(&disabled, PROVIDER_CLAUDE_CODE);
        self.probe.codex_on = provider_enabled(&disabled, PROVIDER_CODEX);
        self.probe.cursor_on = provider_enabled(&disabled, PROVIDER_CURSOR);
        self.probe.cursor_key = s
            .settings
            .as_ref()
            .is_some_and(|s| !s.cursor_api_key.is_empty());
    }

    /// `checkAgents`: probe every provider that is on.
    pub fn check_agents(&mut self, cx: &mut Context<Self>) {
        self.toggles(cx);
        let services = self.vcx.services.clone();
        if self.probe.claude_on {
            let s = services.clone();
            self.probes.push(cx.spawn(async move |this, cx| {
                let r = s.settings.detect_agent().await;
                let _ = this.update(cx, |v, cx| {
                    match r {
                        Ok(d) => {
                            v.probe.detect_failed = d.is_empty();
                            v.probe.detected = d;
                        }
                        Err(_) => v.probe.detect_failed = true,
                    }
                    v.probe.claude_checked = true;
                    cx.notify();
                });
            }));
        }
        if self.probe.codex_on {
            let s = services.clone();
            self.probes.push(cx.spawn(async move |this, cx| {
                let r = s.settings.get_codex_status().await;
                let _ = this.update(cx, |v, cx| {
                    v.probe.codex = r.ok();
                    v.probe.codex_checked = true;
                    cx.notify();
                });
            }));
        }
        if self.probe.cursor_on {
            let s = services;
            self.probes.push(cx.spawn(async move |this, cx| {
                let r = s.settings.get_cursor_status().await;
                let _ = this.update(cx, |v, cx| {
                    v.probe.cursor = r.ok();
                    v.probe.cursor_checked = true;
                    cx.notify();
                });
            }));
        }
        self.probes.retain(|t| !t.is_ready());
    }

    fn install_cursor_sdk(&mut self, cx: &mut Context<Self>) {
        self.installing = true;
        self.install_msg = "Starting…".into();
        self.install_pct = -1;
        cx.notify();
        let services = self.vcx.services.clone();
        cx.spawn(async move |this, cx| {
            let st = call(
                cx,
                services.settings.install_cursor_bridge(),
                "Could not install the Cursor SDK",
            )
            .await;
            let _ = this.update(cx, |v, cx| {
                if let Some(st) = st {
                    let installed = st.bridge_installed;
                    v.probe.cursor = Some(st);
                    if installed {
                        toast(ToastKind::Success, "Cursor SDK installed", cx);
                    }
                }
                v.installing = false;
                v.install_msg.clear();
                v.install_pct = -1;
                cx.notify();
            });
        })
        .detach();
    }

    /// The step the page shows now.
    pub fn current_step(&self, cx: &App) -> u8 {
        let s = self.vcx.state.read(cx);
        let has_git = s.projects.iter().any(|p| p.is_git);
        let live = s.workspaces.iter().filter(|w| task_started(w)).count();
        step(&self.probe, self.skip_agent, has_git, live > 0)
    }

    fn focus_of(&mut self, key: &str, cx: &mut Context<Self>) -> FocusHandle {
        self.buttons
            .entry(key.to_string())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone()
    }

    /// A `<button>` in the page: a tab stop with `.welcome button:focus-visible`'s ring
    /// (2px `--amber`, offset 2, radius 4).
    fn button(&mut self, key: &str, window: &Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let focus = self.focus_of(key, cx);
        let ring = is_focus_visible(&focus, window);
        let amber = Theme::of(cx).tokens.amber;
        let sel = key.to_string();
        div()
            .id(SharedString::from(format!("welcome-{key}")))
            .debug_selector(move || sel.clone())
            .relative()
            .track_focus(&focus)
            .cursor_pointer()
            .when(ring, |d| d.child(outline(2., 2., 4., 0., amber)))
    }

    /// `.lnk` in `.wprov` / `.ws-links` / `.ws-skip` / `.ws-head-link`: mono 12.5px `--muted`,
    /// underlined 3px below the baseline, `--text` on hover.
    fn lnk(
        &mut self,
        key: &str,
        label: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
        run: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        let t = Theme::of(cx).tokens;
        let hovered = window.use_keyed_state(
            SharedString::from(format!("welcome-hover-{key}")),
            cx,
            |_, _| false,
        );
        let on = *hovered.read(cx);
        let h = hovered.clone();
        self.button(key, window, cx)
            .flex_none()
            .on_hover(move |over, _, cx| h.update(cx, |v, cx| {
                *v = *over;
                cx.notify();
            }))
            .on_click(cx.listener(move |this, _, window, cx| run(this, window, cx)))
            .child(
                text(label.to_string())
                    .mono()
                    .size(12.5)
                    .line_height(LineHeight::Normal)
                    .color(if on { t.text } else { t.muted })
                    .underline(true)
                    .underline_offset(3.)
                    .nowrap(),
            )
    }

    fn go_settings(&self, section: &'static str, cx: &mut App) {
        self.vcx
            .state
            .update(cx, |s, cx| s.open_settings(Some(section), cx));
    }

    /// `stepHead`: the numbered disc, the title and, once past the step, its chips.
    fn step_head(&self, n: u8, step: u8, title: &str, chips: Option<Div>, cx: &App) -> Div {
        let t = Theme::of(cx).tokens;
        let done = step > n;
        let current = step == n;
        let (fg, border, bg) = if done {
            (
                t.emerald,
                t.emerald.mix(0.4, t.line),
                t.emerald.mix(0.1, t.bg),
            )
        } else if current {
            (t.amber, t.amber, t.bg)
        } else {
            (t.faint, t.line_bright, t.bg)
        };
        let num = recur_ui::style::card(bg, border, 12.)
            .flex_none()
            .size(px(24.))
            .flex()
            .items_center()
            .justify_center()
            .debug_selector(move || {
                format!(
                    "ws-num:{n}{}",
                    if done {
                        ":done"
                    } else if current {
                        ":current"
                    } else {
                        ""
                    }
                )
            })
            .child(if done {
                icon(IconName::Check).size(px(13.)).color(fg).into_any_element()
            } else {
                text(n.to_string())
                    .mono()
                    .size(12.)
                    .line_height(LineHeight::Normal)
                    .color(fg)
                    .into_any_element()
            });
        let mut head = div()
            .relative()
            .flex()
            .items_center()
            .gap(px(12.))
            .min_h(px(24.))
            // `.ws-num { position: absolute; left: 0 }` against the `li`, vertically at its
            // static (centered) position in the head.
            .child(
                div()
                    .absolute()
                    .left(px(-40.))
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .child(num),
            )
            .child(
                text(title.to_string())
                    .size(15.)
                    .weight(if current { weight::SEMIBOLD } else { weight::MEDIUM })
                    .color(if current { t.text } else { t.muted })
                    .nowrap(),
            );
        if done && let Some(chips) = chips {
            head = head.child(
                chips
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(6.))
                    .ml(px(2.)),
            );
        }
        head
    }

    fn provider_chips(&self, cx: &App) -> Div {
        let t = Theme::of(cx).tokens;
        let mut d = div();
        let mut i = 0;
        for (ready, slug, name) in [
            (self.probe.claude_ready(), "claude", "Claude Code"),
            (self.probe.cursor_ready(), "cursor", "Cursor"),
            (self.probe.codex_ready(), "codex", "Codex"),
        ] {
            if !ready {
                continue;
            }
            let sel = format!("ws-chip:1:{i}={name}");
            i += 1;
            d = d.child(
                chip(false, &t)
                    .debug_selector(move || sel.clone())
                    .child(chip_ico(provider_icon(slug).size(px(13.)), &t))
                    .child(chip_label(name, t.text)),
            );
        }
        d
    }

    fn project_chips(&self, cx: &App) -> Div {
        let t = Theme::of(cx).tokens;
        let s = self.vcx.state.read(cx);
        let git: Vec<_> = s.projects.iter().filter(|p| p.is_git).collect();
        let mut d = div();
        for (i, p) in git.iter().take(3).enumerate() {
            let sel = format!("ws-chip:2:{i}={}", p.name);
            d = d.child(
                chip(false, &t)
                    .debug_selector(move || sel.clone())
                    .child(project_pic(
                        p.pic.clone(),
                        p.color.clone(),
                        p.name.clone(),
                        PicBox {
                            size: 14.,
                            radius: 4.,
                            font_size: 9.,
                        },
                    ))
                    .child(chip_label(p.name.clone(), t.text)),
            );
        }
        if git.len() > 3 {
            let label = format!("+{}", git.len() - 3);
            let sel = format!("ws-chip:2:3={label}");
            d = d.child(
                chip(true, &t)
                    .debug_selector(move || sel.clone())
                    .child(chip_label(label, t.muted)),
            );
        }
        d
    }

    fn workspace_chip(&self, cx: &App) -> Div {
        let t = Theme::of(cx).tokens;
        let n = self
            .vcx
            .state
            .read(cx)
            .workspaces
            .iter()
            .filter(|w| task_started(w))
            .count();
        let label = format!("{n} {}", if n == 1 { "workspace" } else { "workspaces" });
        let sel = format!("ws-chip:3:0={label}");
        div().child(
            chip(false, &t)
                .debug_selector(move || sel.clone())
                .child(chip_ico(icon(IconName::GitBranch).size(px(12.)), &t))
                .child(chip_label(label, t.text)),
        )
    }

    /// `.ws-note`: muted 13px at 1.55, at most 52ch wide.
    fn note(label: &str, cx: &App) -> Div {
        let t = Theme::of(cx).tokens;
        let ch = recur_ui::font_metrics::zero_advance(&Family::Ui, 13.);
        div().mb(px(16.)).max_w(px(52. * ch)).child(
            text(label.to_string())
                .id(SharedString::from(format!("ws-note-{}", label.len())))
                .size(13.)
                .leading(1.55)
                .color(t.muted),
        )
    }

    /// One `.wprov li`: name, note and action in a `104px | 1fr | auto` grid (one column
    /// with the note below at 720px and narrower).
    fn provider_row(
        &mut self,
        slug: &'static str,
        name: &str,
        (on, warn): (bool, bool),
        note: AnyElement,
        action: Option<AnyElement>,
        first: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = Theme::of(cx).tokens;
        let narrow = f32::from(window.viewport_size().width) <= 720.;
        let name_el = text(name.to_string())
            .size(13.)
            .weight(weight::SEMIBOLD)
            .color(if on || warn { t.text } else { t.muted });
        let action = div().flex_none().children(action);
        let sel = format!(
            "wprov:{slug}{}{}",
            if on { ":on" } else { "" },
            if warn { ":warn" } else { "" }
        );
        let row = div()
            .debug_selector(move || sel.clone())
            .py(px(11.))
            .px(px(14.))
            .bg(if warn { t.amber.mix(0.06, t.pane) } else { t.pane })
            .when(!first, |d| {
                d.border_t(px(1.)).border_color(Hsla::from(t.line_soft))
            });
        if narrow {
            row.flex().flex_col().gap(px(14.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(14.))
                        .child(div().flex_1().min_w_0().child(name_el))
                        .child(action),
                )
                .child(div().min_w_0().child(note))
        } else {
            row.flex()
                .items_center()
                .gap(px(14.))
                .child(div().flex_none().w(px(104.)).child(name_el))
                .child(div().flex_1().min_w_0().child(note))
                .child(action)
        }
    }

    fn plain_note(id: &str, spans: Vec<recur_ui::Span>, cx: &App) -> AnyElement {
        let t = Theme::of(cx).tokens;
        Text::from_spans(spans)
            .id(SharedString::from(format!("wprov-note-{id}")))
            .size(12.5)
            .leading(1.5)
            .color(t.muted)
            .into_any_element()
    }

    /// `.wprov-note code`.
    fn code(c: &str, t: &Tokens) -> recur_ui::Span {
        span(c.to_string())
            .mono()
            .size(11.5)
            .color(t.text)
            .boxed(InlineBox {
                pad_top: 1.,
                pad_bottom: 1.,
                pad_left: 5.,
                pad_right: 5.,
                background: Some(t.pane_2.into()),
                radius: 4.,
                ..Default::default()
            })
    }

    fn on_note(detail: String, cx: &App) -> AnyElement {
        let t = Theme::of(cx).tokens;
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .min_w_0()
            .child(icon(IconName::Check).size(px(13.)).color(t.emerald))
            .child(
                text(detail)
                    .size(12.5)
                    .leading(1.5)
                    .color(t.text),
            )
            .into_any_element()
    }

    fn providers(&mut self, step: u8, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let t = Theme::of(cx).tokens;
        self.toggles(cx);
        let p = self.probe.clone();
        let codex_chatgpt = self
            .vcx
            .state
            .read(cx)
            .settings
            .as_ref()
            .is_some_and(|s| s.codex_chat_gpt_auth);
        let mut rows: Vec<Div> = Vec::new();

        // Claude Code.
        let (note, action) = if p.claude_ready() {
            (Self::on_note(p.claude_detail(), cx), None)
        } else if !p.claude_on {
            (
                Self::plain_note("claude", vec![span("Turned off.")], cx),
                Some(self.lnk("wprov:claude:Turn on", "Turn on", window, cx, |v, _, cx| {
                    v.go_settings("agent", cx)
                })),
            )
        } else if !p.claude_checked {
            (Self::plain_note("claude", vec![span("Checking…")], cx), None)
        } else {
            (
                Self::plain_note(
                    "claude",
                    vec![
                        span("Not signed in. Run "),
                        Self::code("claude", &t),
                        span(" in a terminal."),
                    ],
                    cx,
                ),
                Some(self.lnk("wprov:claude:Check again", "Check again", window, cx, |v, _, cx| {
                    v.check_agents(cx)
                })),
            )
        };
        rows.push(self.provider_row(
            "claude",
            "Claude Code",
            (p.claude_ready(), false),
            note,
            action.map(IntoElement::into_any_element),
            true,
            window,
            cx,
        ));

        // Codex.
        let (note, action) = if p.codex_ready() {
            (Self::on_note(p.codex_detail(), cx), None)
        } else if !p.codex_on {
            (
                Self::plain_note("codex", vec![span("Turned off.")], cx),
                Some(self.lnk("wprov:codex:Turn on", "Turn on", window, cx, |v, _, cx| {
                    v.go_settings("agent", cx)
                })),
            )
        } else if !p.codex_checked {
            (Self::plain_note("codex", vec![span("Checking…")], cx), None)
        } else if codex_chatgpt {
            (
                Self::plain_note(
                    "codex",
                    vec![
                        span("Not signed in. Run "),
                        Self::code("codex login", &t),
                        span(" in a terminal."),
                    ],
                    cx,
                ),
                Some(self.lnk("wprov:codex:Check again", "Check again", window, cx, |v, _, cx| {
                    v.check_agents(cx)
                })),
            )
        } else {
            (
                Self::plain_note("codex", vec![span("Sign in with ChatGPT or an API key.")], cx),
                Some(self.lnk("wprov:codex:Connect", "Connect", window, cx, |v, _, cx| {
                    v.go_settings("agent", cx)
                })),
            )
        };
        rows.push(self.provider_row(
            "codex",
            "Codex",
            (p.codex_ready(), false),
            note,
            action.map(IntoElement::into_any_element),
            false,
            window,
            cx,
        ));

        // Cursor.
        let warn = p.cursor_needs_sdk();
        let (note, action): (AnyElement, Option<AnyElement>) = if p.cursor_ready() {
            (Self::on_note(p.cursor_detail(), cx), None)
        } else if warn {
            let msg = if self.installing {
                let m = if self.install_msg.is_empty() {
                    "Installing…".to_string()
                } else {
                    self.install_msg.clone()
                };
                if self.install_pct >= 0 {
                    format!("{m} · {}%", self.install_pct)
                } else {
                    m
                }
            } else {
                "Signed in, but the Cursor SDK is not installed. Recur needs it to run Cursor agents (about 50 MB).".into()
            };
            let installing = self.installing;
            let btn = self
                .button("wprov:cursor:Install SDK", window, cx)
                .child(
                    tbtn(
                        "wprov-install",
                        Tbtn::Default,
                        TbtnSize {
                            height: 28.,
                            pad_x: 12.,
                            font_size: 12.5,
                        },
                        None,
                        cx,
                    )
                    .when(installing, |d| d.cursor_default())
                    .child(button_label(
                        if installing { "Installing…" } else { "Install SDK" },
                        12.5,
                        weight::MEDIUM,
                    )),
                )
                .when(!installing, |d| {
                    d.on_click(cx.listener(|v, _, _, cx| v.install_cursor_sdk(cx)))
                });
            (
                div()
                    .flex()
                    .items_start()
                    .gap(px(6.))
                    .min_w_0()
                    .child(
                        div()
                            .flex_none()
                            .mt(px(2.))
                            .child(icon(IconName::TriangleAlert).size(px(13.)).color(t.amber)),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            text(msg)
                                .id("wprov-cursor-warn")
                                .size(12.5)
                                .leading(1.5)
                                .color(t.text),
                        ),
                    )
                    .into_any_element(),
                Some(btn.into_any_element()),
            )
        } else if !p.cursor_on {
            (
                Self::plain_note("cursor", vec![span("Turned off.")], cx),
                Some(
                    self.lnk("wprov:cursor:Turn on", "Turn on", window, cx, |v, _, cx| {
                        v.go_settings("agent", cx)
                    })
                    .into_any_element(),
                ),
            )
        } else if p.cursor_signed_in() && !p.cursor_checked {
            (Self::plain_note("cursor", vec![span("Checking…")], cx), None)
        } else {
            (
                Self::plain_note("cursor", vec![span("Sign in with your Cursor account.")], cx),
                Some(
                    self.lnk("wprov:cursor:Connect", "Connect", window, cx, |v, _, cx| {
                        v.go_settings("agent", cx)
                    })
                    .into_any_element(),
                ),
            )
        };
        rows.push(self.provider_row(
            "cursor",
            "Cursor",
            (p.cursor_ready(), warn),
            note,
            action,
            false,
            window,
            cx,
        ));

        let border = if step == 1 { t.line } else { t.line_soft };
        let list = list_box(border)
            .flex()
            .flex_col()
            .children(round_ends(rows));
        let mut links = div()
            .flex()
            .gap(px(18.))
            .mt(px(12.))
            .child(self.lnk(
                "ws-links:Provider settings",
                "Provider settings",
                window,
                cx,
                |v, _, cx| v.go_settings("agent", cx),
            ));
        if step == 1 {
            links = links.child(self.lnk(
                "ws-links:Continue without an agent",
                "Continue without an agent",
                window,
                cx,
                |v, _, cx| {
                    v.skip_agent = true;
                    cx.notify();
                },
            ));
        }
        div().child(list).child(links)
    }

    /// `.wmode`: one mode row of step 3.
    fn mode_row(&mut self, m: ChatMode, first: bool, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let t = Theme::of(cx).tokens;
        let accent = m.tint(&t);
        let (line, prompt) = example(m);
        let key = format!("wmode:{}", m.label());
        let hovered = window.use_keyed_state(
            SharedString::from(format!("welcome-hover-{key}")),
            cx,
            |_, _| false,
        );
        let on = *hovered.read(cx);
        let h = hovered.clone();
        let name = Text::from_spans(vec![
            span(m.label()).weight(weight::SEMIBOLD),
            span(line)
                .weight(weight::REGULAR)
                .color(t.muted)
                .boxed(InlineBox {
                    margin_left: 8.,
                    ..Default::default()
                }),
        ])
        .id(SharedString::from(format!("{key}-name")))
        .size(13.5)
        .line_height(LineHeight::Normal)
        .color(t.text);
        let prompt = text(format!("“{prompt}”"))
            .id(SharedString::from(format!("{key}-prompt")))
            .size(13.)
            .line_height(LineHeight::Normal)
            .color(t.text)
            .opacity(0.85);
        let row = self
            .button(&key, window, cx)
            .flex()
            .items_center()
            .gap(px(14.))
            .w_full()
            .py(px(13.))
            .px(px(14.))
            .on_hover(move |over, _, cx| h.update(cx, |v, cx| {
                *v = *over;
                cx.notify();
            }))
            .on_click(cx.listener(move |v, _, _, cx| v.start_with(m, cx)))
            .child(
                div()
                    .flex_none()
                    .size(px(28.))
                    .rounded(px(8.))
                    .bg(accent.mix(0.14, recur_ui::color::TRANSPARENT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(m.icon()).size(px(15.)).color(accent)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .min_w_0()
                    .flex_1()
                    .child(name)
                    .child(prompt),
            )
            .child(
                icon(IconName::ChevronRight)
                    .size(px(14.))
                    .color(if on { t.text } else { t.faint }),
            );
        div()
            .bg(if on { t.pane_2 } else { t.pane })
            .when(!first, |d| {
                d.border_t(px(1.)).border_color(Hsla::from(t.line_soft))
            })
            .child(row)
    }

    fn start_with(&mut self, m: ChatMode, cx: &mut Context<Self>) {
        let first = self
            .vcx
            .state
            .read(cx)
            .projects
            .iter()
            .find(|p| p.is_git)
            .map(|p| p.id.clone())
            .unwrap_or_default();
        self.vcx.state.update(cx, |s, cx| {
            s.open_new_task(
                &first,
                Some(NewTaskDraft {
                    mode: m.key().into(),
                    ..Default::default()
                }),
                cx,
            )
        });
    }

    fn explore_row(
        &mut self,
        (ico, name, line): (IconName, &'static str, &'static str),
        first: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = Theme::of(cx).tokens;
        let key = format!("wexplore:{name}");
        let hovered = window.use_keyed_state(
            SharedString::from(format!("welcome-hover-{key}")),
            cx,
            |_, _| false,
        );
        let on = *hovered.read(cx);
        let h = hovered.clone();
        let row = self
            .button(&key, window, cx)
            .flex()
            .items_center()
            .gap(px(14.))
            .w_full()
            .py(px(13.))
            .px(px(14.))
            .on_hover(move |over, _, cx| h.update(cx, |v, cx| {
                *v = *over;
                cx.notify();
            }))
            .on_click(cx.listener(move |v, _, _, cx| {
                v.vcx.state.update(cx, |s, cx| match name {
                    "Inbox" => s.go(ViewName::Briefing, None, cx),
                    "Pull requests" => s.go(ViewName::Pr, None, cx),
                    "Loops" => s.open_settings(Some("loops"), cx),
                    _ => s.open_settings(Some("bundle-commands"), cx),
                })
            }))
            .child(icon(ico).size(px(16.)).color(t.muted))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .flex_1()
                    .min_w_0()
                    .child(
                        text(name)
                            .size(13.5)
                            .weight(weight::SEMIBOLD)
                            .line_height(LineHeight::Normal)
                            .color(t.text),
                    )
                    .child(
                        text(line)
                            .id(SharedString::from(format!("{key}-line")))
                            .size(12.5)
                            .line_height(LineHeight::Normal)
                            .color(t.muted),
                    ),
            )
            .child(
                icon(IconName::ChevronRight)
                    .size(px(14.))
                    .color(if on { t.text } else { t.faint }),
            );
        div()
            .bg(if on { t.pane_2 } else { t.pane })
            .when(!first, |d| {
                d.border_t(px(1.)).border_color(Hsla::from(t.line_soft))
            })
            .child(row)
    }

    /// `.tbtn.amber` with a `⌘N` keycap, 32px tall (`.ws-body .tbtn`, `.wexplore-foot .tbtn`).
    fn amber_button(
        &mut self,
        key: &str,
        label: &str,
        kbd: bool,
        window: &Window,
        cx: &mut Context<Self>,
        run: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        let mut b = tbtn(
            SharedString::from(format!("{key}-btn")),
            Tbtn::Amber,
            TbtnSize {
                height: 32.,
                pad_x: 14.,
                font_size: 13.,
            },
            None,
            cx,
        )
        .when(kbd, |d| d.gap(px(10.)))
        .child(button_label(label.to_string(), 13., weight::MEDIUM));
        if kbd {
            b = b.child(recur_ui::kbd::kbd("⌘N").size(13.).color(Theme::of(cx).tokens.on_amber));
        }
        self.button(key, window, cx)
            .flex_none()
            .on_click(cx.listener(move |v, _, _, cx| run(v, cx)))
            .child(b)
    }

    /// `#v-welcome.on .welcome > *`: `win .45s cubic-bezier(.2, .8, .2, 1) both` from
    /// `opacity: 0; translateY(6px)`.
    fn entrance(el: Div, at: Option<Instant>, window: &mut Window, cx: &App) -> Div {
        let p = at.map_or(1., |at| {
            motion::eased(at, Duration::from_millis(450), motion::Easing::SETTLE, cx)
        });
        if p >= 1. {
            return el;
        }
        if !motion::is_frozen(cx) {
            window.request_animation_frame();
        }
        el.relative().top(px(6. * (1. - p))).opacity(p)
    }
}

impl Render for WelcomeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx).tokens;
        let (on, loaded) = {
            let s = self.vcx.state.read(cx);
            (s.view == ViewName::Welcome, s.settings.is_some())
        };
        self.toggles(cx);
        let step = self.current_step(cx);
        if step == 4 && self.explore_at.is_none() && on {
            self.explore_at = Some(Instant::now());
        }
        let shown_at = self.shown_at;

        let h1 = text(if step == 4 {
            "You’re set up."
        } else {
            "Set up Recur"
        })
        .display()
        .size(28.)
        .weight(weight::SEMIBOLD)
        .tracking(-0.035)
        .leading(1.2)
        .color(t.text);
        let h1 = div()
            .mb(px(28.))
            .debug_selector(move || {
                format!("welcome-h1={}", if step == 4 { "You’re set up." } else { "Set up Recur" })
            })
            .child(h1);

        // Step 1.
        let mut s1 = self.step_li(1, step, cx).child(self.step_head(
            1,
            step,
            "Connect an agent",
            (step == 4).then(|| self.provider_chips(cx)),
            cx,
        ));
        if step == 4 {
            s1 = s1.child(
                div()
                    .absolute()
                    .right_0()
                    .top(px(3.))
                    .child(self.lnk("ws-head-link:Providers", "Providers", window, cx, |v, _, cx| {
                        v.go_settings("agent", cx)
                    })),
            );
        }
        if step < 4 {
            let body = if !loaded {
                Self::note("Checking Claude Code and Codex…", cx)
            } else {
                self.providers(step, window, cx)
            };
            s1 = s1.child(div().mt(px(12.)).child(body));
        }

        // Step 2.
        let mut s2 = self.step_li(2, step, cx).child(self.step_head(
            2,
            step,
            "Add a project",
            Some(self.project_chips(cx)),
            cx,
        ));
        if step == 2 {
            let btn = self.amber_button(
                "ws-body:Choose a repository",
                "Choose a repository",
                false,
                window,
                cx,
                |v, cx| v.vcx.state.update(cx, |s, cx| s.open_project("", cx)),
            );
            s2 = s2.child(
                div()
                    .mt(px(12.))
                    .child(Self::note(
                        "Pick a local Git repository. Recur creates a worktree per task, so your checkout stays untouched.",
                        cx,
                    ))
                    .child(div().flex().child(btn)),
            );
        }

        // Step 3.
        let mut s3 = self.step_li(3, step, cx).child(self.step_head(
            3,
            step,
            "Start your first task",
            Some(self.workspace_chip(cx)),
            cx,
        ));
        if step == 3 {
            let modes = ChatMode::options(false, false);
            let mut rows = Vec::new();
            for (i, m) in modes.into_iter().enumerate() {
                rows.push(self.mode_row(m, i == 0, window, cx));
            }
            let list = list_box(t.line)
                .flex()
                .flex_col()
                .children(round_ends(rows));
            let skip = self.ws_skip(window, cx);
            s3 = s3.child(
                div()
                    .mt(px(12.))
                    .child(Self::note(
                        "Each task gets its own workspace and a chat in one of five modes. Pick the mode to start in.",
                        cx,
                    ))
                    .child(list)
                    .child(div().flex().child(skip)),
            );
        }

        let steps = div()
            .flex()
            .flex_col()
            .child(s1.child(Self::rail(cx)))
            .child(s2.child(Self::rail(cx)))
            .child(s3);

        let mut col = div()
            .w_full()
            .max_w(px(620.))
            .mx_auto()
            .pt(px(56.))
            .px(px(32.))
            .pb(px(64.))
            .flex()
            .flex_col()
            .child(Self::entrance(
                div().mb(px(36.)).child(wordmark(cx)),
                shown_at,
                window,
                cx,
            ))
            .child(Self::entrance(h1, shown_at, window, cx))
            .child(Self::entrance(steps, shown_at, window, cx));

        if step == 4 {
            let mut rows = Vec::new();
            for (i, e) in EXPLORE.into_iter().enumerate() {
                rows.push(self.explore_row(e, i == 0, window, cx));
            }
            let list = list_box(t.line)
                .flex()
                .flex_col()
                .children(round_ends(rows));
            let foot = div().flex().items_center().flex_wrap().gap(px(10.)).mt(px(24.)).child(
                self.amber_button("wexplore-foot:New workspace", "New workspace", true, window, cx, |v, cx| {
                    v.vcx.state.update(cx, |s, cx| s.open_new_task("", None, cx))
                }),
            );
            let section = div()
                .child(
                    div().mb(px(12.)).child(
                        text("Worth knowing about")
                            .size(12.)
                            .weight(weight::MEDIUM)
                            .tracking(0.04)
                            .uppercase()
                            .color(t.faint),
                    ),
                )
                .child(list)
                .child(foot);
            let at = self.explore_at.max(shown_at);
            col = col.child(Self::entrance(section, at, window, cx));
        }

        div()
            .id("v-welcome")
            .debug_selector(move || format!("v-welcome{}", if on { ":on" } else { "" }))
            .key_context("Welcome")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(t.bg)
            .child(
                recur_ui::widgets::scroll_area("welcome-scroll")
                    .track(&self.scroll)
                    .child(col),
            )
    }
}

impl WelcomeView {
    /// `.ws`: `padding: 0 0 28px 40px`, `current` while it is the step.
    fn step_li(&self, n: u8, step: u8, _: &App) -> Div {
        let current = step == n;
        div()
            .relative()
            .pl(px(40.))
            .pb(px(28.))
            .debug_selector(move || format!("ws-step:{n}{}", if current { ":current" } else { "" }))
    }

    /// `.ws:not(:last-child)::before`: the 1px rail from under the disc to 4px above the next.
    fn rail(cx: &App) -> Div {
        div()
            .absolute()
            .left(px(11.))
            .top(px(28.))
            .bottom(px(4.))
            .w(px(1.))
            .bg(Theme::of(cx).tokens.line)
    }

    /// `.lnk.ws-skip`: "New workspace" with its keycap, underlined.
    fn ws_skip(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = Theme::of(cx).tokens;
        let key = "ws-skip:New workspace";
        let hovered = window.use_keyed_state(
            SharedString::from(format!("welcome-hover-{key}")),
            cx,
            |_, _| false,
        );
        let on = *hovered.read(cx);
        let h = hovered.clone();
        let color = if on { t.text } else { t.muted };
        self.button(key, window, cx)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .mt(px(16.))
            .on_hover(move |over, _, cx| h.update(cx, |v, cx| {
                *v = *over;
                cx.notify();
            }))
            .on_click(cx.listener(|v, _, _, cx| {
                let first = v
                    .vcx
                    .state
                    .read(cx)
                    .projects
                    .iter()
                    .find(|p| p.is_git)
                    .map(|p| p.id.clone())
                    .unwrap_or_default();
                v.vcx
                    .state
                    .update(cx, |s, cx| s.open_new_task(&first, None, cx))
            }))
            .child(
                text("New workspace")
                    .mono()
                    .size(12.5)
                    .line_height(LineHeight::Normal)
                    .color(color)
                    .underline(true)
                    .underline_offset(3.)
                    .nowrap(),
            )
            .child(recur_ui::kbd::kbd("⌘N").size(10.).color(color).underline(true).underline_offset(3.))
    }
}

/// `.wprov` / `.wmodes` / `.wexplore ul`: a 1px border, radius 10 and `overflow: hidden`, with
/// no background of its own (the rows paint theirs).
fn list_box(border: Color) -> Div {
    div()
        .border(px(1.))
        .border_color(Hsla::from(border))
        .rounded(px(10.))
        .overflow_hidden()
}

/// GPUI clips rectangles: the rows of a rounded `overflow: hidden` list round their own outer
/// corners to the card's inner radius (10 minus the 1px border).
fn round_ends(rows: Vec<Div>) -> Vec<Div> {
    let n = rows.len();
    rows.into_iter()
        .enumerate()
        .map(|(i, r)| {
            r.when(i == 0, |d| d.rounded_t(px(9.)))
                .when(i + 1 == n, |d| d.rounded_b(px(9.)))
        })
        .collect()
}

/// Build the entity (the `welcome` route).
pub fn build(vcx: &ViewContext, window: &mut Window, cx: &mut App) -> Entity<WelcomeView> {
    cx.new(|cx| WelcomeView::new(vcx, window, cx))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe() -> Probe {
        Probe {
            claude_on: true,
            codex_on: true,
            cursor_on: true,
            ..Default::default()
        }
    }

    #[test]
    fn steps_follow_live_state() {
        let mut p = probe();
        assert_eq!(step(&p, false, false, false), 1);
        assert_eq!(step(&p, true, false, false), 2);
        // Probes still running and a task exists: an agent evidently works.
        assert_eq!(step(&p, false, true, true), 4);
        p.claude_checked = true;
        p.codex_checked = true;
        p.cursor_checked = true;
        assert_eq!(step(&p, false, true, true), 1);
        p.detected = "Claude Code · dev@example.com".into();
        assert_eq!(step(&p, false, true, false), 3);
        assert_eq!(step(&p, false, true, true), 4);
    }

    #[test]
    fn claude_detail_drops_the_product_prefix() {
        let mut p = probe();
        p.detected = "Claude Code · dev@example.com (Acme)".into();
        assert_eq!(p.claude_detail(), "dev@example.com (Acme)");
        p.detected = "Claude Code".into();
        assert_eq!(p.claude_detail(), "Signed in");
        p.detected = "Claude Code 2.1".into();
        assert_eq!(p.claude_detail(), " 2.1");
        p.detected = "Other agent".into();
        assert_eq!(p.claude_detail(), "Other agent");
    }

    #[test]
    fn cursor_needs_the_sdk_before_it_counts() {
        let mut p = probe();
        p.cursor_key = true;
        assert!(!p.cursor_needs_sdk());
        p.cursor_checked = true;
        assert!(p.cursor_needs_sdk() && !p.cursor_ready());
        p.cursor = Some(CursorStatus {
            bridge_installed: true,
            ..Default::default()
        });
        assert!(p.cursor_ready() && !p.cursor_needs_sdk());
    }

    #[test]
    fn a_task_counts_once_something_was_sent() {
        let base = Workspace {
            id: "w".into(),
            goal: "New Workspace".into(),
            ..Default::default()
        };
        assert!(!task_started(&base));
        assert!(task_started(&Workspace {
            last_activity_at: 1,
            ..base.clone()
        }));
        assert!(task_started(&Workspace {
            goal: "Add a timeout".into(),
            ..base.clone()
        }));
        assert!(!task_started(&Workspace {
            goal: "Add a timeout".into(),
            is_main: true,
            ..base.clone()
        }));
        assert!(!task_started(&Workspace {
            goal: "Add a timeout".into(),
            host_pr_id: "pr".into(),
            ..base
        }));
    }
}
