//! Panel heads and bodies (`panelHeadExtras`, `panelBody`): the Files scope chips, the SCM
//! list mode and actions, the Findings status chips and Fix button, Outline, Problems, Search
//! and References, and the agent / plan slots.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use recur_ui::guard::Guarded;
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::tooltip::titled;
use recur_ui::{IconName, Theme, icon};
use recur_view_core::prefs::CodePanelKey;
use recur_view_review::review_diff::FileListMode;
use recur_view_review::style::hsla;

use crate::lsp::problems::ProblemSeverity;
use crate::obs::{FindingsFilter, LIST_CAP, Observatory, OpenRequest};
use crate::style::{icon_btn, obs_empty, seg, seg_text_btn};

/// `moreRow`: "Show all N…" past `LIST_CAP`.
pub(crate) fn more_row(
    key: &'static str,
    total: usize,
    cx: &mut Context<Observatory>,
) -> AnyElement {
    let t = Theme::of(cx).tokens;
    div()
        .id(SharedString::from(format!("more-{key}")))
        .debug_selector(move || format!("more:{key}"))
        .flex()
        .flex_col()
        .w_full()
        .px(px(10.))
        .py(px(7.))
        .border_b_1()
        .border_color(hsla(t.line_soft))
        .cursor_pointer()
        .text_color(hsla(t.muted))
        .hover(move |s| s.bg(hsla(t.pane_2)))
        .child(
            text(format!("Show all {total}…"))
                .size(11.5)
                .line_height(LineHeight::Normal)
                .nowrap(),
        )
        .on_click(cx.guarded_listener(move |o, _, _, cx| {
            o.expanded_lists.insert(key.to_string());
            cx.notify();
        }))
        .into_any_element()
}

/// A `.obs-sym` / `.obs-hit` row shell.
fn hit_row(id: SharedString, cx: &App) -> gpui_kit::Stateful<gpui_kit::Div> {
    let t = Theme::of(cx).tokens;
    div()
        .id(id)
        .flex()
        .flex_col()
        .gap(px(2.))
        .w_full()
        .px(px(10.))
        .py(px(7.))
        .border_b_1()
        .border_color(hsla(t.line_soft))
        .cursor_pointer()
        .text_color(hsla(t.text))
        .hover(move |s| s.bg(hsla(t.pane_2)))
}

impl Observatory {
    fn capped<'a, T>(&self, key: &str, list: &'a [T]) -> &'a [T] {
        if self.expanded_lists.contains(key) || list.len() <= LIST_CAP {
            list
        } else {
            &list[..LIST_CAP]
        }
    }

    fn more(&self, key: &'static str, total: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        (total > LIST_CAP && !self.expanded_lists.contains(key)).then(|| more_row(key, total, cx))
    }

    /// `panelHeadExtras(kind)`.
    pub(crate) fn render_head_extras(
        &mut self,
        kind: CodePanelKey,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let t = Theme::of(cx).tokens;
        match kind {
            CodePanelKey::Files => {
                if self.scope_paths().is_empty() {
                    return None;
                }
                let base = self.review_base(cx);
                let base = if base.is_empty() {
                    "HEAD".to_string()
                } else {
                    base
                };
                let count = self.review_paths(cx).len();
                let browse = self.browse_all;
                let changed = seg_text_btn("scope-changed", !browse, None, cx)
                    .debug_selector(|| "obs-scope:changed".into())
                    .gap(px(4.))
                    .h_full()
                    .px(px(8.))
                    .child(
                        text("Changed")
                            .size(11.)
                            .weight(500.)
                            .line_height(LineHeight::Normal)
                            .nowrap(),
                    )
                    .child(
                        div().ml(px(1.)).child(
                            text(count.to_string())
                                .mono()
                                .size(10.)
                                .line_height(LineHeight::Normal)
                                .color(if !browse { t.muted } else { t.faint })
                                .nowrap(),
                        ),
                    )
                    .on_click(
                        cx.guarded_listener(|o, _, window, cx| o.set_browse(false, window, cx)),
                    );
                let all = seg_text_btn("scope-all", browse, None, cx)
                    .debug_selector(|| "obs-scope:all".into())
                    .gap(px(4.))
                    .h_full()
                    .px(px(8.))
                    .child(
                        text("All files")
                            .size(11.)
                            .weight(500.)
                            .line_height(LineHeight::Normal)
                            .nowrap(),
                    )
                    .on_click(
                        cx.guarded_listener(|o, _, window, cx| o.set_browse(true, window, cx)),
                    );
                Some(
                    div()
                        .ml_auto()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            seg(cx)
                                .h(px(24.))
                                .items_stretch()
                                .child(titled(
                                    "scope-changed-t",
                                    format!("Show files changed since {base}"),
                                    changed,
                                ))
                                .child(titled(
                                    "scope-all-t",
                                    "Browse the full repository tree",
                                    all,
                                )),
                        )
                        .into_any_element(),
                )
            }
            CodePanelKey::Scm => {
                if !self.is_git(cx) || !self.tree_writable() {
                    return None;
                }
                let mode = self.scm_file_list_mode;
                let mut tools = div().ml_auto().flex().items_center().gap(px(6.)).child(
                    seg(cx)
                        .child(titled(
                            "scm-flat-t",
                            "Flat list",
                            recur_view_review::style::seg_btn(
                                "scm-flat",
                                IconName::List,
                                mode == FileListMode::Flat,
                                None,
                                cx,
                            )
                            .on_click(cx.guarded_listener(
                                |o, _, _, cx| o.set_scm_file_list_mode(FileListMode::Flat, cx),
                            )),
                        ))
                        .child(titled(
                            "scm-tree-t",
                            "Tree",
                            recur_view_review::style::seg_btn(
                                "scm-tree",
                                IconName::Workflow,
                                mode == FileListMode::Tree,
                                None,
                                cx,
                            )
                            .on_click(cx.guarded_listener(
                                |o, _, _, cx| o.set_scm_file_list_mode(FileListMode::Tree, cx),
                            )),
                        )),
                );
                if self.working_dirty_count() > 0 {
                    tools =
                        tools
                            .child(titled(
                                "scm-review-t",
                                "Review uncommitted changes",
                                icon_btn("scm-review", IconName::FileDiff, false, None, cx)
                                    .on_click(cx.guarded_listener(|o, _, window, cx| {
                                        o.open_working_review(window, cx)
                                    })),
                            ))
                            .child(titled(
                                "scm-discard-all-t",
                                "Discard all uncommitted changes",
                                icon_btn("scm-discard-all", IconName::Undo, false, None, cx)
                                    .on_click(cx.guarded_listener(|o, _, window, cx| {
                                        o.discard_all_working(window, cx)
                                    })),
                            ));
                }
                Some(tools.into_any_element())
            }
            CodePanelKey::Findings => {
                let counts = |f: FindingsFilter| -> usize {
                    match f {
                        FindingsFilter::Open => self.open_findings_count(),
                        FindingsFilter::All => self.findings.len(),
                        other => self
                            .findings
                            .iter()
                            .filter(|x| x.status == other.key())
                            .count(),
                    }
                };
                let mut chips = seg(cx).flex_wrap().min_h(px(24.)).max_w(px(340.));
                for f in FindingsFilter::ALL {
                    let on = self.findings_filter == f;
                    let n = counts(f);
                    let mut b =
                        seg_text_btn(SharedString::from(format!("ff-{}", f.key())), on, None, cx)
                            .debug_selector(move || format!("findings-filter:{}", f.key()))
                            .gap(px(4.))
                            .h(px(22.))
                            .px(px(7.))
                            .child(
                                text(f.label())
                                    .size(11.)
                                    .weight(500.)
                                    .line_height(LineHeight::Normal)
                                    .nowrap(),
                            );
                    if n > 0 {
                        b = b.child(
                            div().ml(px(1.)).child(
                                text(n.to_string())
                                    .mono()
                                    .size(10.)
                                    .line_height(LineHeight::Normal)
                                    .color(if on { t.muted } else { t.faint })
                                    .nowrap(),
                            ),
                        );
                    }
                    chips = chips.child(b.on_click(cx.guarded_listener(move |o, _, _, cx| {
                        o.findings_filter = f;
                        cx.notify();
                    })));
                }
                let mut tools = div()
                    .ml_auto()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(chips);
                let open = self.open_findings_count();
                if open > 0 && self.props.can_fix_findings {
                    tools = tools.child(titled(
                        "fix-findings-t",
                        format!("Fix open findings ({open})"),
                        icon_btn("fix-findings", IconName::Wrench, false, None, cx)
                            .debug_selector(|| "fix-findings".into())
                            .on_click(cx.guarded_listener(|o, _, window, cx| {
                                o.select_code_panel(CodePanelKey::Maestro, window, cx);
                                cx.emit(crate::obs::ObservatoryEvent::FixFindings);
                            })),
                    ));
                }
                Some(tools.into_any_element())
            }
            _ => None,
        }
    }

    /// `panelBody(kind)`.
    pub(crate) fn render_panel_body(
        &mut self,
        kind: CodePanelKey,
        in_side: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match kind {
            // The host's views are cached: the Observatory's own redraws (explorer, editor,
            // review) never re-render the chat or the plan, and theirs never re-render it.
            CodePanelKey::Maestro => self.slots.left_panel.clone().map_or_else(
                || div().into_any_element(),
                |v| v.cached(slot_style()).into_any_element(),
            ),
            CodePanelKey::Plan => self.slots.plan_panel.clone().map_or_else(
                || div().into_any_element(),
                |v| v.cached(slot_style()).into_any_element(),
            ),
            CodePanelKey::Findings => {
                let findings = self.findings.clone();
                let (filter, busy) = (self.findings_filter, self.finding_busy_id.clone());
                self.findings_panel
                    .update(cx, |p, cx| p.set_props(&findings, filter, &busy, cx));
                self.findings_panel.clone().into_any_element()
            }
            CodePanelKey::Artifacts => self.ensure_artifacts_panel(window, cx).into_any_element(),
            CodePanelKey::Files => self.render_files_panel(in_side, window, cx),
            CodePanelKey::Scm => self.render_scm_panel(window, cx),
            CodePanelKey::Outline => self.render_outline(cx),
            CodePanelKey::Problems => self.render_problems(cx),
            CodePanelKey::Search => self.render_search(window, cx),
            CodePanelKey::Refs => self.render_refs(cx),
        }
    }

    fn ensure_artifacts_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Entity<crate::artifacts_panel::ArtifactsPanel> {
        let ws = self.props.workspace_id.clone();
        let sel = self.selected_artifact_id();
        if let Some(p) = &self.artifacts_panel {
            p.update(cx, |p, cx| {
                p.set_workspace(&ws, cx);
                p.set_selected(&sel, cx);
            });
            return p.clone();
        }
        let services = self.services.clone();
        let p = gpui_kit::AppContext::new(cx, |cx| {
            let mut p = crate::artifacts_panel::ArtifactsPanel::new(services, &ws, cx);
            p.set_selected(&sel, cx);
            p
        });
        let sub = cx.guarded_subscribe_in(
            &p,
            window,
            |o, _, ev: &crate::artifacts_panel::ArtifactsPanelEvent, window, cx| {
                use crate::artifacts_panel::ArtifactsPanelEvent as E;
                match ev {
                    E::Open(a) => {
                        o.open_artifact(&a.id, &a.title, &a.kind, &a.path, window, cx)
                            .detach();
                    }
                    E::Renamed(a) => o.rename_artifact_tab(&a.id, &a.title, cx),
                    E::Deleted(id) => o.drop_artifact_tab(id, window, cx),
                }
            },
        );
        self.editor_subs.push(sub);
        self.artifacts_panel = Some(p.clone());
        p
    }

    fn render_outline(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let go = recur_ui::style::rbtn("outline-go", recur_ui::style::Rbtn::Default, None, cx)
            .w_full()
            .justify_center()
            .child(
                text("Go to Symbol ⌘⇧O")
                    .size(12.5)
                    .weight(550.)
                    .line_height(LineHeight::Normal)
                    .nowrap(),
            )
            .on_click(cx.guarded_listener(|o, _, window, cx| {
                o.open_quick_open(crate::quick_open::QuickOpenMode::Symbol, window, cx)
            }));
        let mut list = div().flex().flex_col();
        let syms = self.outline.clone();
        let shown = self.capped("outline", &syms).to_vec();
        if shown.is_empty() {
            list = list.child(obs_empty(
                if self.lsp_attach.is_some() {
                    "No symbols from language server"
                } else {
                    "No symbols"
                },
                false,
                cx,
            ));
        }
        for (i, s) in shown.into_iter().enumerate() {
            let (s1, s2) = (s.clone(), s.clone());
            list = list.child(titled(
                SharedString::from(format!("sym-t-{i}")),
                "Click: refs · Double-click: go to definition",
                hit_row(SharedString::from(format!("sym-{i}")), cx)
                    .debug_selector(move || format!("obs-sym:{i}"))
                    .child(
                        text(s.kind.clone())
                            .mono()
                            .size(9.)
                            .uppercase()
                            .color(t.cyan)
                            .nowrap(),
                    )
                    .child(text(s.name.clone()).mono().size(11.5).nowrap())
                    .child(
                        text(s.line.to_string())
                            .mono()
                            .size(10.)
                            .color(t.faint)
                            .nowrap(),
                    )
                    .on_click(cx.guarded_listener(
                        move |o, ev: &gpui_kit::ClickEvent, window, cx| {
                            if ev.click_count() > 1 {
                                let n = recur_textmate::symbol_lookup_name(&s2.name, &s2.kind);
                                o.goto_def(&n, window, cx);
                                return;
                            }
                            let line = s1.line.max(0) as usize;
                            o.scroll_to_line(line, 0, 0, 0, window, cx);
                            let n = recur_textmate::symbol_lookup_name(&s1.name, &s1.kind);
                            o.find_refs(&n, line, window, cx);
                        },
                    )),
            ));
        }
        let more = self.more("outline", syms.len(), cx);
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .pt(px(10.))
                    .px(px(10.))
                    .pb(px(6.))
                    .bg(hsla(t.bg))
                    .child(go),
            )
            .child(list)
            .children(more)
            .into_any_element()
    }

    fn render_problems(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let probs = self.problems(cx);
        let counts = crate::lsp::problems::count_problems(&probs);
        let mut head = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
            .flex_wrap()
            .px(px(12.))
            .py(px(8.))
            .min_h(px(36.))
            .border_b_1()
            .border_color(hsla(t.line_soft));
        if counts.total > 0 {
            let stat = |n: usize,
                        suffix: &str,
                        c: recur_ui::Color,
                        id: &'static str,
                        tip: &'static str| {
                titled(
                    SharedString::from(format!("{id}-t")),
                    tip,
                    div()
                        .id(id)
                        .px(px(7.))
                        .py(px(2.))
                        .rounded(px(5.))
                        .bg(hsla(c.fade(0.14)))
                        .child(
                            text(format!("{n} {suffix}"))
                                .mono()
                                .size(11.)
                                .weight(600.)
                                .tabular()
                                .line_height(LineHeight::Scale(1.3))
                                .color(c)
                                .nowrap(),
                        ),
                )
            };
            head = head
                .child(stat(counts.errors, "err", t.coral, "probs-err", "Errors"))
                .child(stat(
                    counts.warnings,
                    "warn",
                    t.amber,
                    "probs-warn",
                    "Warnings",
                ));
            if let Some(a) = &self.lsp_attach {
                head = head.child(titled(
                    "probs-server-t",
                    "Language server",
                    div()
                        .id("probs-server")
                        .ml_auto()
                        .max_w(gpui_kit::relative(0.4))
                        .child(
                            text(a.server_key.clone())
                                .mono()
                                .size(10.5)
                                .color(t.faint)
                                .truncate(),
                        ),
                ));
            }
        } else {
            head = head.child(
                text(if self.lsp_attach.is_some() {
                    "No problems"
                } else {
                    "Open a file with a language server on PATH"
                })
                .size(12.)
                .color(t.faint),
            );
        }
        let mut list = div().flex().flex_col();
        let shown = self.capped("problems", &probs).to_vec();
        if shown.is_empty() && counts.total == 0 && self.lsp_attach.is_some() {
            list = list.child(obs_empty("No diagnostics yet", false, cx));
        }
        let busy_any = !self.problem_fix_busy.is_empty();
        for (i, p) in shown.into_iter().enumerate() {
            let busy_key = format!("{}:{}:{}", p.uri, p.line, p.message);
            let fixing = self.problem_fix_busy == busy_key;
            let sev_color = match p.severity {
                ProblemSeverity::Error => t.coral,
                ProblemSeverity::Warning => t.amber,
                ProblemSeverity::Info => t.cyan,
                ProblemSeverity::Hint => t.muted,
            };
            let sev_icon = recur_view_chat::findings_ui::finding_sev_icon(match p.severity {
                ProblemSeverity::Error => "high",
                ProblemSeverity::Warning => "medium",
                ProblemSeverity::Info => "info",
                ProblemSeverity::Hint => "low",
            });
            let name = p.path.rsplit('/').next().unwrap_or(&p.path).to_string();
            let loc = if p.line > 0 {
                format!("{name}:{}", p.line)
            } else if name.is_empty() {
                "—".into()
            } else {
                name
            };
            let (open_p, fix_p, ai_p) = (p.clone(), p.clone(), p.clone());
            let mut meta = div()
                .flex()
                .items_center()
                .gap(px(8.))
                .min_w_0()
                .child(titled(
                    SharedString::from(format!("prob-loc-t-{i}")),
                    p.path.clone(),
                    div()
                        .id(SharedString::from(format!("prob-loc-{i}")))
                        .min_w_0()
                        .child(text(loc).mono().size(11.).color(t.cyan).truncate()),
                ));
            if let Some(src) = &p.source {
                meta = meta.child(
                    div().flex_none().child(
                        text(src.to_lowercase())
                            .mono()
                            .size(10.)
                            .tracking(0.02)
                            .color(t.faint)
                            .nowrap(),
                    ),
                );
            }
            let main = titled(
                SharedString::from(format!("prob-main-t-{i}")),
                p.message.clone(),
                div()
                    .id(SharedString::from(format!("prob-main-{i}")))
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .flex_1()
                    .min_w_0()
                    .pt(px(8.))
                    .pr(px(4.))
                    .pb(px(8.))
                    .pl(px(12.))
                    .cursor_pointer()
                    .child(
                        div().w(px(18.)).flex_none().child(
                            div()
                                .mt(px(1.))
                                .size(px(14.))
                                .child(icon(sev_icon).size(px(14.)).color(hsla(sev_color))),
                        ),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(
                                text(p.message.clone())
                                    .size(12.5)
                                    .line_height(LineHeight::Scale(1.35))
                                    .line_clamp(2),
                            )
                            .child(meta),
                    )
                    .on_click(cx.guarded_listener(move |o, _, window, cx| {
                        o.open_path(
                            OpenRequest::new(open_p.path.clone()).line(open_p.line.max(0) as usize),
                            window,
                            cx,
                        )
                        .detach()
                    })),
            );
            let fix_btn = |id: String| {
                div()
                    .id(SharedString::from(id))
                    .flex()
                    .flex_none()
                    .items_center()
                    .self_center()
                    .my(px(6.))
                    .mr(px(6.))
                    .h(px(24.))
                    .px(px(9.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(hsla(t.line))
                    .bg(hsla(t.pane))
                    .text_color(hsla(t.muted))
                    .cursor_pointer()
                    .hover(move |s| {
                        s.text_color(hsla(t.text))
                            .border_color(hsla(t.line_bright))
                            .bg(hsla(t.pane_2))
                    })
            };
            let fix = titled(
                SharedString::from(format!("prob-fix-t-{i}")),
                "Apply quick fix",
                fix_btn(format!("prob-fix-{i}"))
                    .debug_selector(move || format!("prob-fix:{i}"))
                    .when(busy_any, |d| d.opacity(0.5))
                    .child(
                        text(if fixing { "…" } else { "Fix" })
                            .size(11.)
                            .weight(600.)
                            .line_height(LineHeight::Scale(1.))
                            .nowrap(),
                    )
                    .when(!busy_any, |d| {
                        d.on_click(cx.guarded_listener(move |o, _, _, cx| {
                            o.apply_problem_fix(fix_p.clone(), cx)
                        }))
                    }),
            );
            let state = self.state.clone();
            let ai = titled(
                SharedString::from(format!("prob-ai-t-{i}")),
                "Ask the agent to fix this",
                fix_btn(format!("prob-ai-{i}"))
                    .child(icon(IconName::Sparkle).size(px(12.)))
                    .on_click(cx.guarded_listener(move |_, _, _, cx| {
                        crate::lsp::client::fix_problem_with_ai(&ai_p, &state, cx)
                    })),
            );
            let hovered = self.problem_hovered == Some(i);
            list = list.child(
                div()
                    .id(SharedString::from(format!("prob-{i}")))
                    .debug_selector(move || format!("obs-problem:{i}"))
                    .flex()
                    .items_start()
                    .gap(px(4.))
                    .pr(px(6.))
                    .border_b_1()
                    .border_color(hsla(t.line_soft))
                    .when(hovered, |d| d.bg(hsla(t.pane_2)))
                    .when(fixing, |d| d.opacity(0.7))
                    .on_hover(cx.guarded_listener(move |o, on: &bool, _, cx| {
                        o.problem_hovered = if *on {
                            Some(i)
                        } else if o.problem_hovered == Some(i) {
                            None
                        } else {
                            o.problem_hovered
                        };
                        cx.notify();
                    }))
                    .child(main)
                    .child(fix)
                    .child(ai),
            );
        }
        let more = self.more("problems", probs.len(), cx);
        div()
            .debug_selector(|| "obs-problems".into())
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(head)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(recur_ui::widgets::scroll_area("obs-problems-list").child(list)),
            )
            .children(more)
            .into_any_element()
    }

    fn render_search(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let field = |input: &gpui_kit::Entity<gpui_kit::base::input::InputState>,
                     ph: &'static str,
                     disabled: bool,
                     window: &Window,
                     cx: &App| {
            let focused = gpui_kit::Focusable::focus_handle(input.read(cx), cx).is_focused(window);
            recur_ui::style::card(t.bg, if focused { t.line_bright } else { t.line }, 6.)
                .w_full()
                .flex()
                .py(px(6.))
                .px(px(10.))
                .when(disabled, |d| d.opacity(0.5))
                .child(recur_ui::style::text_field(
                    input,
                    &SharedString::from(ph),
                    recur_ui::style::UA_PLACEHOLDER,
                    recur_ui::fonts::Family::Mono,
                    12.,
                    window,
                    cx,
                ))
        };
        let writable = self.tree_writable();
        let q = self.inputs.query(cx);
        let checkbox =
            |id: &'static str, label: &'static str, on: bool, _cx: &mut Context<Self>| {
                div()
                    .id(id)
                    .debug_selector(move || format!("search-opt:{label}"))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .cursor_pointer()
                    .child(crate::style::native_checkbox(on))
                    .child(text(label).mono().size(11.).color(t.muted).nowrap())
            };
        let opts = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .child(
                checkbox("search-regex", "Regex", self.search_regex, cx).on_click(
                    cx.guarded_listener(|o, _, _, cx| {
                        o.search_regex = !o.search_regex;
                        cx.notify();
                    }),
                ),
            )
            .child(
                checkbox("search-case", "Case", self.search_case, cx).on_click(
                    cx.guarded_listener(|o, _, _, cx| {
                        o.search_case = !o.search_case;
                        cx.notify();
                    }),
                ),
            )
            .child(
                recur_ui::style::rbtn("search-find", recur_ui::style::Rbtn::Default, None, cx)
                    .child(
                        text("Find")
                            .size(12.5)
                            .weight(550.)
                            .line_height(LineHeight::Normal)
                            .nowrap(),
                    )
                    .on_click(
                        cx.guarded_listener(|o, _, window, cx| o.run_search(true, window, cx)),
                    ),
            )
            .when(writable, |d| {
                let disabled = q.trim().is_empty();
                d.child(
                    recur_ui::style::rbtn(
                        "search-replace",
                        recur_ui::style::Rbtn::Default,
                        disabled.then_some(t.bg),
                        cx,
                    )
                    .child(
                        text("Replace all")
                            .size(12.5)
                            .weight(550.)
                            .line_height(LineHeight::Normal)
                            .nowrap(),
                    )
                    .when(!disabled, |b| {
                        b.on_click(
                            cx.guarded_listener(|o, _, window, cx| o.run_replace(window, cx)),
                        )
                    }),
                )
            });
        let boxed = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .pt(px(10.))
            .px(px(10.))
            .pb(px(6.))
            .bg(hsla(t.bg))
            .child(field(
                &self.inputs.query,
                crate::inputs::SEARCH_PLACEHOLDER,
                false,
                window,
                cx,
            ))
            .child(field(
                &self.inputs.replace,
                crate::inputs::REPLACE_PLACEHOLDER,
                !writable,
                window,
                cx,
            ))
            .child(field(
                &self.inputs.glob,
                crate::inputs::GLOB_PLACEHOLDER,
                false,
                window,
                cx,
            ))
            .child(opts);
        let mut list = div().flex().flex_col();
        if self.hits.is_empty() {
            list = list.child(obs_empty(
                if q.trim().is_empty() {
                    "Type a query and press Enter"
                } else {
                    "No matches"
                },
                false,
                cx,
            ));
        }
        for (i, h) in self.hits.clone().into_iter().enumerate() {
            let (path, line) = (h.path.clone(), h.line);
            list = list.child(
                hit_row(SharedString::from(format!("hit-{i}")), cx)
                    .debug_selector(move || format!("obs-hit:{i}"))
                    .child(
                        text(format!("{}:{}", h.path, h.line))
                            .mono()
                            .size(10.5)
                            .color(t.amber)
                            .nowrap(),
                    )
                    .child(text(h.preview.clone()).size(11.).color(t.muted).truncate())
                    .on_click(cx.guarded_listener(move |o, _, window, cx| {
                        o.open_path(
                            OpenRequest::new(path.clone()).line(line.max(0) as usize),
                            window,
                            cx,
                        )
                        .detach()
                    })),
            );
        }
        if self.search_skipped_large > 0 {
            let n = self.search_skipped_large;
            list = list.child(obs_empty(
                format!("{n} large file{} skipped", if n == 1 { "" } else { "s" }),
                false,
                cx,
            ));
        }
        if self.search_truncated {
            list = list.child(
                hit_row("hit-more".into(), cx)
                    .text_color(hsla(t.muted))
                    .child(
                        text("Load more…")
                            .size(11.5)
                            .line_height(LineHeight::Normal)
                            .nowrap(),
                    )
                    .on_click(
                        cx.guarded_listener(|o, _, window, cx| o.run_search(false, window, cx)),
                    ),
            );
        }
        div()
            .flex()
            .flex_col()
            .child(boxed)
            .child(list)
            .into_any_element()
    }

    fn render_refs(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let refs = self.refs.clone();
        let shown = self.capped("refs", &refs).to_vec();
        let mut list = div().flex().flex_col();
        if shown.is_empty() {
            list = list.child(obs_empty("Pick a symbol from Outline", false, cx));
        }
        for (i, r) in shown.into_iter().enumerate() {
            let (path, line) = (r.path.clone(), r.line);
            list = list.child(
                hit_row(SharedString::from(format!("ref-{i}")), cx)
                    .debug_selector(move || format!("obs-ref:{i}"))
                    .child(
                        text(format!(
                            "{} · {}:{}",
                            if r.def { "def" } else { "ref" },
                            r.path,
                            r.line
                        ))
                        .mono()
                        .size(10.5)
                        .color(t.amber)
                        .nowrap(),
                    )
                    .child(text(r.text.clone()).size(11.).color(t.muted).truncate())
                    .on_click(cx.guarded_listener(move |o, _, window, cx| {
                        o.open_path(
                            OpenRequest::new(path.clone()).line(line.max(0) as usize),
                            window,
                            cx,
                        )
                        .detach()
                    })),
            );
        }
        let more = self.more("refs", refs.len(), cx);
        div()
            .flex()
            .flex_col()
            .child(list)
            .children(more)
            .into_any_element()
    }
}

/// A slot view fills its panel body (a flex column).
fn slot_style() -> gpui_kit::StyleRefinement {
    use gpui_kit::Styled as _;
    gpui_kit::StyleRefinement::default().flex_1().min_h_0()
}
