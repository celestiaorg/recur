//! Panel heads and bodies (`panelHeadExtras`, `panelBody`): the Files scope chips, the SCM
//! list mode and actions, the Findings status chips and Fix button, Outline, Problems, Search
//! and References, and the agent / plan slots.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use recur_ui::file_icons::file_icon;
use recur_ui::guard::Guarded;
use recur_ui::text::{LineHeight, TextStyled, span, text};
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

/// The Search panel's file header and hit rows, and how far past the viewport they are built.
const SEARCH_HEAD_H: f32 = 24.;
const SEARCH_HIT_H: f32 = 22.;
const SEARCH_OVERSCAN: f32 = 240.;

/// A hit's line as (before, match, after) around its first match, the head cut to keep a match
/// far along the line in view.
fn split_hit<'a>(preview: &'a str, re: &regex::Regex) -> Option<(String, &'a str, &'a str)> {
    let m = re.find(preview).filter(|m| !m.is_empty())?;
    let cut = if m.start() > 32 {
        (0..=m.start() - 24)
            .rev()
            .find(|&i| preview.is_char_boundary(i))
            .unwrap_or(0)
    } else {
        0
    };
    let head = &preview[cut..m.start()];
    let head = if cut > 0 {
        format!("…{head}")
    } else {
        head.to_string()
    };
    Some((head, m.as_str(), &preview[m.end()..]))
}

fn hit_preview(
    preview: &str,
    re: Option<&regex::Regex>,
    t: recur_ui::Tokens,
) -> recur_ui::text::Text {
    let Some((head, hit, tail)) = re.and_then(|re| split_hit(preview, re)) else {
        return text(preview.to_string());
    };
    recur_ui::text::Text::from_spans(vec![
        span(head),
        span(hit.to_string())
            .color(t.text)
            .boxed(recur_ui::text::InlineBox {
                background: Some(hsla(t.amber_d)),
                radius: 2.,
                ..Default::default()
            }),
        span(tail.to_string()),
    ])
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
            CodePanelKey::Maestro => self
                .slots
                .left_panel
                .clone()
                .map_or_else(|| div().into_any_element(), |v| v.into_any_element()),
            CodePanelKey::Plan => self
                .slots
                .plan_panel
                .clone()
                .map_or_else(|| div().into_any_element(), |v| v.into_any_element()),
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
            let sev_icon = recur_view_widgets::findings_ui::finding_sev_icon(match p.severity {
                ProblemSeverity::Error => "high",
                ProblemSeverity::Warning => "medium",
                ProblemSeverity::Info => "info",
                ProblemSeverity::Hint => "low",
            });
            let name = p.path.rsplit('/').next().unwrap_or(&p.path).to_string();
            let loc = if p.line > 0 {
                format!("{name}:{}", p.line)
            } else if name.is_empty() {
                "-".into()
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
        let writable = self.tree_writable();
        let q = self.inputs.query(cx);
        let field_bg = if Theme::of(cx).is_light() {
            t.pane
        } else {
            t.bg
        };
        let inline_btn = |id: &'static str, name: IconName, on: bool, disabled: bool, cx: &App| {
            let b = if disabled {
                recur_view_review::style::icon_btn_disabled(id, name, 20., 13., field_bg, cx)
            } else if on {
                div()
                    .id(id)
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(20.))
                    .rounded(px(5.))
                    .cursor_pointer()
                    .bg(hsla(t.amber_d))
                    .text_color(hsla(t.amber))
                    .child(icon(name).size(px(13.)))
            } else {
                recur_view_review::style::icon_btn(id, name, 20., 13., None, cx)
            };
            b.debug_selector(move || format!("search-opt:{id}"))
        };
        let query_row = recur_ui::style::find_field(
            &self.inputs.query,
            crate::inputs::SEARCH_PLACEHOLDER,
            window,
            cx,
        )
        .pr(px(3.))
        .gap(px(2.))
        .child(titled(
            "search-case-t",
            "Match case",
            inline_btn(
                "search-case",
                IconName::CaseSensitive,
                self.search_case,
                false,
                cx,
            )
            .on_click(cx.guarded_listener(|o, _, window, cx| {
                o.search_case = !o.search_case;
                o.run_search(true, window, cx);
                cx.notify();
            })),
        ))
        .child(titled(
            "search-regex-t",
            "Regular expression",
            inline_btn(
                "search-regex",
                IconName::Regex,
                self.search_regex,
                false,
                cx,
            )
            .on_click(cx.guarded_listener(|o, _, window, cx| {
                o.search_regex = !o.search_regex;
                o.run_search(true, window, cx);
                cx.notify();
            })),
        ));
        let replace_row = writable.then(|| {
            let disabled = q.trim().is_empty();
            recur_ui::style::find_field(
                &self.inputs.replace,
                crate::inputs::REPLACE_PLACEHOLDER,
                window,
                cx,
            )
            .pr(px(3.))
            .child(titled(
                "search-replace-t",
                "Replace all",
                inline_btn("search-replace", IconName::Replace, false, disabled, cx).when(
                    !disabled,
                    |b| {
                        b.on_click(
                            cx.guarded_listener(|o, _, window, cx| o.run_replace(window, cx)),
                        )
                    },
                ),
            ))
        });
        let boxed = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(10.))
            .bg(hsla(t.bg))
            .border_b_1()
            .border_color(hsla(t.line_soft))
            .child(query_row)
            .children(replace_row)
            .child(recur_ui::style::find_field(
                &self.inputs.glob,
                crate::inputs::GLOB_PLACEHOLDER,
                window,
                cx,
            ));
        let mut list = div().flex().flex_col();
        let mut summary = None;
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
        } else {
            let files = self.hits.chunk_by(|a, b| a.path == b.path).count();
            let n = self.hits.len();
            summary = Some(
                div().px(px(10.)).pt(px(8.)).pb(px(4.)).child(
                    text(format!(
                        "{n}{} result{} in {files} file{}",
                        if self.search_truncated { "+" } else { "" },
                        if n == 1 { "" } else { "s" },
                        if files == 1 { "" } else { "s" },
                    ))
                    .size(11.)
                    .color(t.faint)
                    .nowrap(),
                ),
            );
        }
        // Rows have fixed heights, so only those near the viewport are built.
        let view_h = f32::from(self.search_scroll.bounds().size.height);
        let scroll = (-f32::from(self.search_scroll.offset().y)).max(0.);
        let (lo, hi) = (
            scroll - SEARCH_OVERSCAN,
            scroll + view_h.max(600.) + SEARCH_OVERSCAN,
        );
        let (mut y, mut gap, mut i) = (0., 0., 0);
        for group in self.hits.chunk_by(|a, b| a.path == b.path) {
            if y + SEARCH_HEAD_H < lo || y > hi {
                gap += SEARCH_HEAD_H;
            } else {
                if gap > 0. {
                    list = list.child(div().flex_none().h(px(gap)));
                    gap = 0.;
                }
                let (dir, name) = match group[0].path.rsplit_once('/') {
                    Some((d, n)) => (d.to_string(), n.to_string()),
                    None => (String::new(), group[0].path.clone()),
                };
                list = list.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(6.))
                        .w_full()
                        .h(px(SEARCH_HEAD_H))
                        .px(px(10.))
                        .pt(px(4.))
                        .child(
                            div()
                                .size(px(14.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(file_icon(SharedString::from(name.clone()))),
                        )
                        .child(text(name).size(12.).color(t.text).nowrap())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(text(dir).size(11.).color(t.faint).truncate()),
                        )
                        .child(
                            text(group.len().to_string())
                                .mono()
                                .size(10.5)
                                .color(t.faint)
                                .nowrap(),
                        ),
                );
            }
            y += SEARCH_HEAD_H;
            for h in group {
                let idx = i;
                i += 1;
                if y + SEARCH_HIT_H < lo || y > hi {
                    gap += SEARCH_HIT_H;
                    y += SEARCH_HIT_H;
                    continue;
                }
                y += SEARCH_HIT_H;
                if gap > 0. {
                    list = list.child(div().flex_none().h(px(gap)));
                    gap = 0.;
                }
                let (path, line) = (h.path.clone(), h.line);
                list = list.child(
                    div()
                        .id(SharedString::from(format!("hit-{idx}")))
                        .debug_selector(move || format!("obs-hit:{idx}"))
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(8.))
                        .w_full()
                        .h(px(SEARCH_HIT_H))
                        .pl(px(30.))
                        .pr(px(10.))
                        .cursor_pointer()
                        .hover(move |s| s.bg(hsla(t.pane_2)))
                        .child(
                            div().flex_none().min_w(px(24.)).flex().justify_end().child(
                                text(h.line.to_string())
                                    .mono()
                                    .size(10.5)
                                    .color(t.faint)
                                    .nowrap(),
                            ),
                        )
                        .child(
                            div().flex_1().min_w_0().child(
                                hit_preview(&h.preview, self.search_match.as_ref(), t)
                                    .mono()
                                    .size(11.)
                                    .color(t.muted)
                                    .truncate(),
                            ),
                        )
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
        }
        if gap > 0. {
            list = list.child(div().flex_none().h(px(gap)));
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
            .flex_1()
            .min_h_0()
            .child(boxed)
            .children(summary)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        recur_ui::widgets::scroll_area("obs-search-list")
                            .track(&self.search_scroll)
                            .child(list),
                    )
                    .children(recur_ui::style::edge_fades(&self.search_scroll, t.bg, 24.)),
            )
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

#[cfg(test)]
mod tests {
    use super::split_hit;

    #[test]
    fn split_hit_marks_the_first_match_and_cuts_a_long_head() {
        let re = regex::Regex::new("(?i)println").unwrap();
        assert_eq!(
            split_hit("fmt.Println(x)", &re),
            Some(("fmt.".into(), "Println", "(x)"))
        );
        let long = format!("{}Println()", "é".repeat(40));
        let (head, hit, tail) = split_hit(&long, &re).unwrap();
        assert!(head.starts_with('…') && head.chars().count() <= 26);
        assert_eq!((hit, tail), ("Println", "()"));
        assert_eq!(split_hit("nothing here", &re), None);
    }
}
