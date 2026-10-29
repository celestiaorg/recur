//! The Observatory's view: the toolbar (panel toggles, the file / review tab strip, the
//! surface toggles), the body grid (left dock, the main column, the side dock), the frosted
//! creating overlay, the maximized terminal, the tree / tab context menu, Quick Open and the
//! command prompts.

mod ctx;
mod explorer;
mod main;
mod panels;
mod scm;

use recur_ui::guard::Guarded;
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, Context, Div, InteractiveElement, IntoElement, MouseButton, ParentElement,
    Render, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};
use recur_ui::text::{TextStyled, text};
use recur_ui::tooltip::titled;
use recur_ui::widgets::{Axis, ChipIcon, Edge, resize_handle, tab_chip, tab_strip};
use recur_ui::{IconName, Theme, motion};
use recur_view_core::prefs::{CodePanelKey, PanelKey, PanelSide};
use recur_view_review::review_diff::ReviewTab;
pub(crate) use recur_view_review::style::checkerboard;
use recur_view_review::style::hsla;

pub(crate) use scm::ScmProbes;

use crate::model::{FocusZone, MainSurface, is_scratch_path};
use crate::obs::{Observatory, OpenRequest};
use crate::quick_open::QuickOpenMode;
use crate::style::{CHROME_H, NARROW_MAX_W, pane_badge, pane_toggle, side_head, side_title};

const WAITING: &str = "Waiting for the worktree…";

/// `@keyframes pulse` (opacity .5 → 1 → .5, ease-in-out, 1.2s): the current opacity, asking
/// for the next frame while it runs.
pub(crate) fn pulse_opacity(epoch: Instant, window: &mut Window, cx: &App) -> f32 {
    const PERIOD: Duration = Duration::from_millis(1200);
    let e = motion::elapsed(epoch, cx).as_secs_f32();
    let ph = (e % PERIOD.as_secs_f32()) / PERIOD.as_secs_f32();
    let half = if ph < 0.5 { ph * 2. } else { (1. - ph) * 2. };
    if !motion::is_frozen(cx) {
        window.request_animation_frame();
    }
    0.5 + 0.5 * motion::Easing::EASE_IN_OUT.apply(half)
}

fn panel_icon(k: CodePanelKey) -> IconName {
    match k {
        CodePanelKey::Maestro => IconName::Message,
        CodePanelKey::Plan => IconName::ListTodo,
        CodePanelKey::Findings => IconName::HatGlasses,
        CodePanelKey::Files => IconName::FolderSimple,
        CodePanelKey::Scm => IconName::GitPullRequest,
        CodePanelKey::Outline => IconName::List,
        CodePanelKey::Problems => IconName::CircleAlert,
        CodePanelKey::Search => IconName::Search,
        CodePanelKey::Refs => IconName::Refs,
        CodePanelKey::Artifacts => IconName::Grid,
    }
}

impl Observatory {
    pub(crate) fn open_quick_open(
        &mut self,
        mode: QuickOpenMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (p, w, syms) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
            self.outline.clone(),
        );
        self.quick_open.update(cx, |q, cx| {
            q.set_props(&p, &w, &syms, cx);
            q.show(mode, window, cx);
        });
    }

    fn narrow(window: &Window) -> bool {
        f32::from(window.viewport_size().width) <= NARROW_MAX_W
    }

    /// One `.obs-pane-toggle` with its badge in a toolbar panel group.
    fn panel_toggle(
        &mut self,
        k: CodePanelKey,
        side: PanelSide,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let blocked = side == PanelSide::Right && self.props.main_blocked;
        let on = match side {
            PanelSide::Left => self.left_gutter_open() && self.left_kind == k,
            PanelSide::Right => self.show_side && self.right_kind == k,
        };
        let n = self.pane_badge_count(k, cx);
        let title = if blocked {
            WAITING.to_string()
        } else {
            self.pane_badge_title(k, cx)
        };
        let b = pane_toggle(
            SharedString::from(format!("panel-{}", k.key())),
            panel_icon(k),
            on,
            blocked.then_some(t.pane),
            cx,
        )
        .debug_selector(move || format!("panel-toggle:{}", k.key()))
        .when(n > 0, |d| d.child(pane_badge(n, cx)))
        .when(!blocked, |d| {
            d.on_click(
                cx.guarded_listener(move |o, _, window, cx| o.select_code_panel(k, window, cx)),
            )
        });
        titled(SharedString::from(format!("panel-t-{}", k.key())), title, b).into_any_element()
    }

    fn panel_icons(&mut self, side: PanelSide, cx: &mut Context<Self>) -> Div {
        let t = Theme::of(cx).tokens;
        let keys = self.panels_on(side, cx);
        let mut g = div().flex().items_center().gap(px(1.));
        g = match side {
            PanelSide::Left => g.ml(px(2.)).pl(px(4.)).border_l_1(),
            PanelSide::Right => g.mr(px(2.)).pr(px(4.)).border_r_1(),
        }
        .border_color(hsla(t.line));
        for k in keys {
            g = g.child(self.panel_toggle(k, side, cx));
        }
        g
    }

    /// `.obs-toolbar`.
    fn render_toolbar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let narrow = Self::narrow(window);
        let has_left = self.has_left(cx);
        let has_right = self.has_right(cx);
        let left_open = self.left_gutter_open();
        let no_left = !has_left || !left_open;
        let no_side = !has_right || !self.show_side;
        let blocked = self.props.main_blocked;
        let review = self.main_surface(cx) == MainSurface::Review;
        let lit = self.focus_zone == FocusZone::Editor;

        // The root tracks the Observatory's focus, which a click would take; the chrome
        // outside the focusable regions is not focusable in the page.
        let mut bar = div()
            .debug_selector(|| "obs-toolbar".into())
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .flex()
            .items_stretch()
            .flex_none()
            .h(px(CHROME_H))
            .border_b_1()
            .border_color(hsla(t.line))
            .bg(hsla(t.pane));

        let dock_w = self.maestro_dock_width(cx);
        let left_col = |d: Div| {
            if has_left && !no_left && !narrow {
                d.flex_none().w(px(dock_w))
            } else {
                d.flex_none()
            }
        };
        let group = || {
            div()
                .flex()
                .items_center()
                .gap(px(2.))
                .px(px(8.))
                .h_full()
                .min_w_0()
        };
        if has_left {
            let tog = titled(
                "left-gutter-t",
                if left_open {
                    "Hide left panel (⌥⌘B)"
                } else {
                    "Show left panel (⌥⌘B)"
                },
                pane_toggle("left-gutter", IconName::PanelLeft, left_open, None, cx)
                    .debug_selector(|| "obs-left-toggle".into())
                    .on_click(cx.guarded_listener(move |o, _, _, cx| {
                        o.set_left_gutter_open(!left_open, cx)
                    })),
            );
            let icons = self.panel_icons(PanelSide::Left, cx);
            bar = bar.child(left_col(
                group()
                    .when(!narrow, |d| {
                        d.border_r_1().border_color(hsla(if no_left {
                            recur_ui::color::TRANSPARENT
                        } else {
                            t.line
                        }))
                    })
                    .child(tog)
                    .child(icons),
            ));
        } else if !self.props.loop_scoped {
            bar = bar.child(left_col(
                group()
                    .when(!narrow, |d| d.border_r_1().border_color(hsla(t.line)))
                    .child(
                        text("Code observatory")
                            .mono()
                            .size(11.)
                            .tracking(0.04)
                            .uppercase()
                            .color(t.muted)
                            .nowrap(),
                    ),
            ));
        }

        let active_key = if review {
            match self.review_strip_tab(cx) {
                ReviewTab::Changes => "changes".to_string(),
                ReviewTab::Commits => "commits".to_string(),
            }
        } else {
            self.selected_path.clone()
        };
        let mut strip = tab_strip(
            "obs-file-tabs",
            if review { "Review" } else { "Open files" },
        )
        .grow(true)
        .active_key(SharedString::from(active_key))
        .reveal_nonce(self.file_tab_reveal_nonce)
        .scroll_handle(self.file_tab_scroll.clone());
        if let Some((left, id)) = self.file_tab_scroll_restore {
            strip = strip.restore_scroll(left, id);
        }
        if review {
            let b = self.review_bound.clone();
            let count = b.file_count as u32;
            let tab = self.review_strip_tab(cx);
            strip = strip.tab(
                "changes",
                tab_chip("review-changes", "Changes")
                    .title(if count > 0 {
                        format!("Changes ({count})")
                    } else {
                        "Changes".into()
                    })
                    .icon(ChipIcon::App(IconName::FileDiff, None))
                    .badge(count)
                    .active(tab == ReviewTab::Changes)
                    .lit(lit)
                    .on_click(cx.guarded_listener(move |o, _, _, cx| {
                        if !o.props.main_blocked {
                            o.set_review_tab(ReviewTab::Changes, cx);
                        }
                    })),
            );
            if self.review_has_commits(cx) {
                let n = b.commit_count as u32;
                let dirty = self.working_dirty_count();
                let title = match (n > 0, dirty > 0) {
                    (true, true) => format!("Commits ({n}) · {dirty} uncommitted files"),
                    (true, false) => format!("Commits ({n})"),
                    (false, true) => format!("Commits · {dirty} uncommitted files"),
                    (false, false) => "Commits".into(),
                };
                strip = strip.tab(
                    "commits",
                    tab_chip("review-commits", "Commits")
                        .title(title)
                        .icon(ChipIcon::App(IconName::GitCommit, None))
                        .badge(n)
                        .dirty(dirty > 0)
                        .active(tab == ReviewTab::Commits)
                        .lit(lit)
                        .on_click(cx.guarded_listener(move |o, _, _, cx| {
                            if !o.props.main_blocked {
                                o.set_review_tab(ReviewTab::Commits, cx);
                            }
                        })),
                );
            }
        } else {
            let mut seen = std::collections::HashSet::new();
            let files: Vec<String> = self
                .open_files
                .iter()
                .filter(|p| seen.insert((*p).clone()))
                .cloned()
                .collect();
            let n = self.open_files.len();
            strip = strip
                .can_start({
                    let me = self.weak.clone();
                    move |cx| {
                        me.upgrade()
                            .is_some_and(|o| !o.read(cx).props.main_blocked && n > 1)
                    }
                })
                .on_reorder({
                    let me = self.weak.clone();
                    move |order, _, _, cx| {
                        if let Some(o) = me.upgrade() {
                            o.update(cx, |o, cx| {
                                o.reorder_open_files(
                                    order.iter().map(|s| s.to_string()).collect(),
                                    cx,
                                )
                            });
                        }
                    }
                });
            for p in files {
                let scratch = is_scratch_path(&p);
                let name = if scratch {
                    self.scratch_tab_label(&p)
                } else {
                    p.rsplit('/')
                        .next()
                        .filter(|s| !s.is_empty())
                        .unwrap_or(&p)
                        .to_string()
                };
                let (title, icon_name) = if scratch {
                    let d = self.scratch_docs.get(&p);
                    let lang = d
                        .map(|d| d.lang.clone())
                        .filter(|l| !l.is_empty())
                        .unwrap_or_else(|| "txt".into());
                    let t = d
                        .map(|d| d.title.clone())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| name.clone());
                    (format!("{t} ({p})"), format!("file.{lang}"))
                } else {
                    (p.clone(), name.clone())
                };
                let (p1, p2, p3) = (p.clone(), p.clone(), p.clone());
                let ctx_name = name.clone();
                let mut chip = tab_chip(SharedString::from(format!("file-tab:{p}")), name)
                    .title(title)
                    .icon(ChipIcon::File(icon_name.into()))
                    .active(self.selected_path == p)
                    .lit(lit)
                    .dirty(self.tab_is_dirty(&p))
                    .reorderable(!blocked && n > 1)
                    .on_click(cx.guarded_listener(move |o, _, window, cx| {
                        if !o.props.main_blocked {
                            o.open_path(OpenRequest::new(p1.clone()), window, cx)
                                .detach();
                        }
                    }));
                if !blocked {
                    chip =
                        chip.on_close(cx.guarded_listener(move |o, _, window, cx| {
                            o.close_mru(&p2, window, cx)
                        }))
                        .on_context_menu(cx.guarded_listener(
                            move |o, e: &gpui_kit::MouseDownEvent, window, cx| {
                                o.open_ctx(
                                    f32::from(e.position.x),
                                    f32::from(e.position.y),
                                    &p3,
                                    false,
                                    &ctx_name,
                                    true,
                                    window,
                                    cx,
                                );
                            },
                        ));
                }
                strip = strip.tab(SharedString::from(p), chip);
            }
        }
        if !self.props.workspace_id.is_empty() {
            let dis = blocked.then_some(t.pane);
            let editor_on = !review && !self.editor_hidden(cx);
            strip = strip.trailing(titled(
                "editor-toggle-t",
                if blocked {
                    WAITING
                } else if !editor_on {
                    "Show editor (⌥⌘E)"
                } else {
                    "Hide editor (⌥⌘E)"
                },
                pane_toggle("editor-toggle", IconName::Code, editor_on, dis, cx)
                    .debug_selector(|| "obs-editor-toggle".into())
                    .when(!blocked, |d| {
                        d.on_click(cx.guarded_listener(|o, _, _, cx| o.toggle_editor_surface(cx)))
                    }),
            ));
            if self.is_git(cx) {
                strip = strip.trailing(titled(
                    "review-toggle-t",
                    if blocked {
                        WAITING
                    } else {
                        "Review changes (⌥⌘R)"
                    },
                    pane_toggle("review-toggle", IconName::FileDiff, review, dis, cx)
                        .debug_selector(|| "obs-review-toggle".into())
                        .when(!blocked, |d| {
                            d.on_click(cx.guarded_listener(|o, _, _, cx| o.toggle_review(cx)))
                        }),
                ));
            }
            let term_open = self.term_open(cx);
            let ws = self.props.workspace_id.clone();
            strip = strip.trailing(titled(
                "term-toggle-t",
                if blocked {
                    WAITING
                } else if term_open {
                    "Hide terminal (⌃`)"
                } else {
                    "Show terminal (⌃`)"
                },
                pane_toggle("term-toggle", IconName::Terminal, term_open, dis, cx)
                    .debug_selector(|| "obs-term-toggle".into())
                    .when(!blocked, |d| {
                        d.on_click(cx.guarded_listener(move |o, _, _, cx| {
                            o.state
                                .update(cx, |s, cx| s.set_terminal_dock_open(&ws, !term_open, cx));
                        }))
                    }),
            ));
            let beside = self.term_beside(cx);
            let beside_dis = blocked || self.term_overlaying();
            strip = strip.trailing(titled(
                "term-beside-t",
                if blocked {
                    WAITING
                } else if beside {
                    "Terminal below editor"
                } else {
                    "Terminal beside editor"
                },
                pane_toggle(
                    "term-beside",
                    IconName::SquareSplitHorizontal,
                    beside,
                    beside_dis.then_some(t.pane),
                    cx,
                )
                .debug_selector(|| "obs-term-beside-toggle".into())
                .when(!beside_dis, |d| {
                    d.on_click(cx.guarded_listener(|o, _, _, cx| o.toggle_term_beside(cx)))
                }),
            ));
        }
        bar = bar.child(
            div()
                .id("obs-tb-center")
                .debug_selector(|| "obs-tb-center".into())
                .track_focus(&self.tabs_focus)
                .flex_1()
                .min_w_0()
                .flex()
                .items_stretch()
                .child(strip),
        );

        if has_right {
            let show = self.show_side;
            let icons = self.panel_icons(PanelSide::Right, cx);
            let tog = titled(
                "side-toggle-t",
                if blocked {
                    WAITING
                } else if show {
                    "Hide right panel (⌥⌘⇧B)"
                } else {
                    "Show right panel (⌥⌘⇧B)"
                },
                pane_toggle(
                    "side-toggle",
                    IconName::PanelRight,
                    show,
                    blocked.then_some(t.pane),
                    cx,
                )
                .debug_selector(|| "obs-side-toggle".into())
                .when(!blocked, |d| {
                    d.on_click(cx.guarded_listener(move |o, _, _, cx| o.set_show_side(!show, cx)))
                }),
            );
            let mut right = group()
                .justify_end()
                .when(!narrow, |d| {
                    d.border_l_1().border_color(hsla(if no_side {
                        recur_ui::color::TRANSPARENT
                    } else {
                        t.line
                    }))
                })
                .child(icons)
                .child(tog);
            right = if !no_side && !narrow {
                right.flex_none().w(px(self.obs_side_width(cx)))
            } else {
                right.flex_none()
            };
            bar = bar.child(right);
        } else if !narrow {
            bar = bar.child(div().flex_none().w(px(self.obs_side_width(cx))));
        }
        bar.into_any_element()
    }

    /// `.obs-side-head` for a dock showing `kind`.
    fn render_side_head(
        &mut self,
        kind: CodePanelKey,
        left: bool,
        portal: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = self.panel_label(kind).0;
        let agent_head = left && kind == CodePanelKey::Maestro && self.slots.left_head.is_some();
        let plan_head = kind == CodePanelKey::Plan && self.slots.plan_head.is_some();
        let tabbed = (agent_head && self.props.left_title == "Agent") || plan_head;
        let mut head = side_head(tabbed, cx);
        if portal {
            let on = self.state.read(cx).pr_agent_dock_open;
            head = head.child(titled(
                "prd-chat-toggle-t",
                "Hide left panel (⌥⌘B)",
                pane_toggle("prd-chat-toggle", IconName::PanelLeft, on, None, cx)
                    .track_focus(&self.pr_toggle_focus)
                    .debug_selector(|| "button:Hide left panel".into())
                    .on_mouse_down(
                        gpui_kit::MouseButton::Left,
                        cx.guarded_listener(|o, _, window, cx| {
                            window.focus(&o.pr_toggle_focus, cx)
                        }),
                    )
                    .on_click(
                        cx.guarded_listener(|o, _, _, cx| o.set_pr_agent_dock_open(false, cx)),
                    ),
            ));
        }
        let slot = if agent_head {
            self.slots.left_head.clone()
        } else if plan_head {
            self.slots.plan_head.clone()
        } else {
            None
        };
        if let Some(v) = slot {
            let layer = self.head_layer(v, cx);
            head = head.child(div().flex_1().min_w_0().flex().items_stretch().child(
                recur_ui::style::cached_view(
                    &layer,
                    gpui_kit::StyleRefinement::default().flex_1().min_w_0(),
                    cx,
                ),
            ));
        } else {
            head = head.child(side_title(label, cx));
            head = head.children(self.render_head_extras(kind, cx));
        }
        head.into_any_element()
    }

    /// `.obs-left`: the left dock (`maestroDock` / `prChatDock` wide).
    fn render_left_aside(
        &mut self,
        kind: CodePanelKey,
        portal: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let narrow = Self::narrow(window) && !portal;
        let panel = if portal {
            PanelKey::PrChatDock
        } else {
            PanelKey::MaestroDock
        };
        let width = self.state.read(cx).panels.size(panel);
        let head = self.render_side_head(kind, true, portal, cx);
        let body = self.render_panel_body(kind, false, window, cx);
        let state = self.state.clone();
        let handle = resize_handle(
            if portal {
                "pr-chat-dock-resize"
            } else {
                "maestro-dock-resize"
            },
            Axis::X,
            Edge::End,
            width,
            &self.resize_focus[0],
        )
        .on_live({
            let state = state.clone();
            move |w, _, cx| state.update(cx, |s, cx| s.set_panel_live(panel, w, cx))
        })
        .on_commit({
            let state = state.clone();
            move |_, cx| state.update(cx, |s, _| s.persist_panels())
        })
        .on_step(move |w, _, cx| state.update(cx, |s, cx| s.set_panel(panel, w, cx)));
        let mut aside = div()
            .id("obs-left")
            .debug_selector(|| "obs-left".into())
            .track_focus(&self.left_focus)
            .relative()
            .flex()
            .flex_col()
            .min_h_0()
            .min_w_0()
            .bg(hsla(t.bg))
            .on_mouse_down(
                MouseButton::Left,
                cx.guarded_listener(|o, _, window, cx| {
                    if !o.left_focus.contains_focused(window, cx) {
                        o.left_focus.focus(window, cx);
                    }
                }),
            )
            .child(head)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .bg(hsla(t.bg))
                    .child(body),
            );
        aside = if narrow {
            // `max-height: 200px`. The chat's transcript is a virtualized list that adds no
            // content height here, but in WebKit its messages always overflow the cap.
            let aside = if kind == CodePanelKey::Maestro {
                aside.flex_none().h(px(200.))
            } else {
                aside.max_h(px(200.))
            };
            aside.border_b_1().border_color(hsla(t.line))
        } else {
            aside
                .flex_none()
                .w(px(width))
                .h_full()
                .border_r_1()
                .border_color(hsla(t.line))
                .when(!self.term_overlaying(), |d| d.child(handle))
        };
        aside.into_any_element()
    }

    /// The agent dock rendered into PR detail (`attachAgentPortal`) while this workspace is the
    /// PR's agent workspace and the PR dock is open.
    pub fn render_pr_agent_dock(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.portal_agent_to_pr(cx) || !self.state.read(cx).pr_agent_dock_open {
            return None;
        }
        Some(self.render_left_aside(CodePanelKey::Maestro, true, window, cx))
    }

    fn head_layer(
        &mut self,
        view: gpui_kit::AnyView,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Entity<recur_ui::pane::Layer> {
        let id = view.entity_id();
        if let Some((_, l)) = self.head_layers.iter().find(|(e, _)| *e == id) {
            return l.clone();
        }
        let layer = recur_ui::pane::Layer::new(view, cx);
        self.head_layers.push((id, layer.clone()));
        layer
    }

    /// The review's open line notes' text boxes, drawn here because the review host is cached
    /// (see `recur_view_review::note_portal`).
    fn review_note_portals(&self, cx: &App) -> Vec<AnyElement> {
        self.review_surface
            .as_ref()
            .map(|r| r.read(cx).files_view().read(cx).stack().pane.portals())
            .unwrap_or_default()
    }

    fn left_pane_body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match self.left_meta(cx) {
            Some(kind) => self.render_left_aside(kind, false, window, cx),
            None => div().into_any_element(),
        }
    }

    fn side_pane_body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match self.right_meta(cx) {
            Some(kind) => self.render_side_aside(kind, window, cx),
            None => div().into_any_element(),
        }
    }

    /// `.obs-side`: the right dock (`obsSide` wide).
    fn render_side_aside(
        &mut self,
        kind: CodePanelKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let narrow = Self::narrow(window);
        let width = self.obs_side_width(cx);
        let head = self.render_side_head(kind, false, false, cx);
        let body = self.render_panel_body(kind, true, window, cx);
        let mut aside = div()
            .id("obs-side")
            .debug_selector(|| "obs-side".into())
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .relative()
            .flex()
            .flex_col()
            .min_h_0()
            .min_w_0()
            .bg(hsla(t.pane))
            .child(head)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .bg(hsla(t.bg))
                    .child(body),
            );
        if narrow {
            aside = aside
                .max_h(px(200.))
                .border_b_1()
                .border_color(hsla(t.line));
        } else {
            aside = aside
                .flex_none()
                .w(px(width))
                .h_full()
                .border_l_1()
                .border_color(hsla(t.line));
            if !self.props.main_blocked && !self.term_overlaying() {
                let state = self.state.clone();
                aside = aside.child(
                    resize_handle(
                        "obs-side-resize",
                        Axis::X,
                        Edge::Start,
                        width,
                        &self.resize_focus[1],
                    )
                    .on_live({
                        let state = state.clone();
                        move |w, _, cx| {
                            state.update(cx, |s, cx| s.set_panel_live(PanelKey::ObsSide, w, cx))
                        }
                    })
                    .on_commit({
                        let state = state.clone();
                        move |_, cx| state.update(cx, |s, _| s.persist_panels())
                    })
                    .on_step(move |w, _, cx| {
                        state.update(cx, |s, cx| s.set_panel(PanelKey::ObsSide, w, cx))
                    }),
                );
            }
        }
        if self.props.main_blocked || self.term_overlaying() {
            aside = aside.occlude();
        }
        aside.into_any_element()
    }

    /// `.obs-body`.
    fn render_body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let narrow = Self::narrow(window);
        let portal = self.portal_agent_to_pr(cx);
        let mut body = div()
            .debug_selector(|| "obs-body".into())
            .flex()
            .flex_1()
            .min_h_0()
            .min_w_0();
        body = if narrow {
            body.flex_col()
        } else {
            body.flex_row()
        };
        if !portal
            && self.has_left(cx)
            && self.left_gutter_open()
            && let Some(kind) = self.left_meta(cx)
        {
            // Cached like the right dock (a key in a line note or the chat beside it would
            // redraw the findings and their markdown), unless it holds a text field itself.
            if !matches!(kind, CodePanelKey::Maestro | CodePanelKey::Scm) && !narrow {
                let pane = self
                    .left_pane
                    .get_or_insert_with(|| recur_ui::pane::Pane::of(cx, Self::left_pane_body))
                    .clone();
                let width = self.state.read(cx).panels.size(PanelKey::MaestroDock);
                body = body.child(recur_ui::style::cached_view(
                    &pane,
                    gpui_kit::StyleRefinement::default()
                        .flex_none()
                        .w(px(width))
                        .h_full(),
                    cx,
                ));
            } else {
                body = body.child(self.render_left_aside(kind, false, window, cx));
            }
        }
        let main = self.render_main(window, cx);
        let mut work = div()
            .debug_selector(|| "obs-work".into())
            .relative()
            .flex()
            .flex_1()
            .min_w_0()
            .min_h_0();
        work = if narrow {
            work.flex_col()
        } else {
            work.flex_row()
        };
        work = if self.editor_fullscreen {
            // `.obs-main.fullscreen`: the editor pane over the whole window (`z-index: 150`).
            let vp = window.viewport_size();
            work.child(
                gpui_kit::deferred(
                    gpui_kit::anchored()
                        .position_mode(gpui_kit::AnchoredPositionMode::Window)
                        .position(gpui_kit::point(px(0.), px(0.)))
                        .child(div().flex().w(vp.width).h(vp.height).occlude().child(main)),
                )
                .with_priority(150),
            )
        } else {
            work.child(div().flex().flex_1().min_w_0().min_h_0().child(main))
        };
        if self.has_right(cx)
            && self.show_side
            && let Some(kind) = self.right_meta(cx)
            && !(portal && kind == CodePanelKey::Maestro)
        {
            // Its own cached view unless it holds the chat or the commit message (whose keys
            // would dirty it, redrawing all of it) or stacks under the main area at its content's
            // height. Source control caches its changes list inside instead.
            if !matches!(kind, CodePanelKey::Maestro | CodePanelKey::Scm) && !narrow {
                let pane = self
                    .side_pane
                    .get_or_insert_with(|| recur_ui::pane::Pane::of(cx, Self::side_pane_body))
                    .clone();
                let width = self.obs_side_width(cx);
                work = work.child(recur_ui::style::cached_view(
                    &pane,
                    gpui_kit::StyleRefinement::default()
                        .flex_none()
                        .w(px(width))
                        .h_full(),
                    cx,
                ));
            } else {
                work = work.child(self.render_side_aside(kind, window, cx));
            }
        }
        if self.props.main_blocked
            && let Some(v) = self.slots.main_overlay.clone()
        {
            // `z-index: 10`: over the side panels' resize handles (8).
            work = work.child(
                gpui_kit::deferred(
                    div()
                        .debug_selector(|| "obs-work-overlay".into())
                        .absolute()
                        .inset_0()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .min_h_0()
                        .occlude()
                        .child(recur_ui::style::backdrop_blur(3., t.bg.fade(0.68)))
                        .child(v),
                )
                .with_priority(10),
            );
        }
        body.child(work).into_any_element()
    }
}

impl Observatory {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        if let Some(path) = self.pending_rehydrate.take()
            && path == self.selected_path
        {
            self.hydrate_file(&path, 0, 0, 0, 0, window, cx).detach();
        }
        self.sync_editors(window, cx);
        let toolbar_pane = self
            .toolbar_pane
            .get_or_insert_with(|| recur_ui::pane::Pane::of(cx, Self::render_toolbar))
            .clone();
        let toolbar = recur_ui::style::cached_view(
            &toolbar_pane,
            gpui_kit::StyleRefinement::default()
                .flex_none()
                .w_full()
                .h(px(CHROME_H)),
            cx,
        );
        let body = self.render_body(window, cx);
        self.step_term_anim(window, cx);
        let overlay = self.render_term_overlay(cx);
        let ctx = self.render_ctx_menu(cx);
        let mut root = div()
            .id("obs")
            .debug_selector(|| "obs".into())
            .key_context("Observatory")
            .track_focus(&self.focus)
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .size_full()
            .min_h_0()
            .bg(hsla(t.bg))
            .child(crate::geometry::probe(&self.bounds.obs))
            .child(if self.term_overlaying() {
                div().occlude().child(toolbar).into_any_element()
            } else {
                toolbar
            })
            .child(body)
            .children(overlay)
            .children(ctx)
            .children(self.review_note_portals(cx))
            .children(self.layers.iter().map(|l| {
                gpui_kit::deferred(recur_ui::style::cached_view(
                    l,
                    gpui_kit::StyleRefinement::default().absolute().inset_0(),
                    cx,
                ))
                .with_priority(recur_ui::style::OVERLAY_PRIORITY)
            }));
        if self.term_overlaying() {
            root = root.overflow_hidden();
        }
        // The Svelte `$effect`s that mirror this frame's state outward, and `focusin` for a
        // move the zone watchers could not see (focus taken while a watcher was arming).
        cx.defer_in(window, |o, window, cx| {
            o.sync_side_effects(cx);
            let focused = window.focused(cx);
            if focused != o.last_focus {
                o.last_focus = focused;
                if o.focus.contains_focused(window, cx) {
                    o.on_obs_focus_in(window, cx);
                }
            }
        });
        root
    }
}

impl Render for Observatory {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}
