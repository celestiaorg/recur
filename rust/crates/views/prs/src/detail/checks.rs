//! `checksPanel`: the checks card (the conversation's compact one with its own scroller, and
//! the Checks tab's), with live durations and the Fix checks flow; and the Checks pane.

use recur_ui::guard::Guarded;
use std::sync::LazyLock;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use recur_domain::{Check, Pr, PrSource};
use recur_forge::Forge;
use recur_forge::github::GitHub;
use recur_forge::view::{
    RollupKind, check_group_collapsed, checks_box_byline, checks_heading, checks_rollup,
    group_checks, is_pending_check, is_skipped_check, live_check_detail,
};
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::tooltip::titled;
use recur_ui::{IconName, Theme, icon};
use recur_view_core::pr::{PrLifecycle, pr_lifecycle};
use recur_view_review::style::note_btn;

use super::PrDetail;
use super::render::{rollup_kind, rollup_look};
use crate::style::{hsla, spin_deg};

static GITHUB: LazyLock<GitHub> = LazyLock::new(GitHub::default);

/// `checkIcon`: skipped, passed, pending (spinning) or failed.
pub(crate) fn check_icon(c: &Check, t: &recur_ui::Tokens) -> (IconName, recur_ui::Color, bool) {
    if is_skipped_check(&c.status, &c.detail) {
        (IconName::CircleSlash, t.muted, false)
    } else if c.passed {
        (IconName::Check, t.emerald, false)
    } else if is_pending_check(&c.status) {
        (IconName::Loader, t.amber, true)
    } else {
        (IconName::X, t.coral, false)
    }
}

impl PrDetail {
    /// `canFixChecks`.
    fn can_fix_checks(pr: &Pr) -> bool {
        let life = pr_lifecycle(Some(pr));
        pr.source == PrSource::GitHub
            && !matches!(life, Some(PrLifecycle::Merged | PrLifecycle::Closed))
            && checks_rollup(
                pr.checks.as_deref().unwrap_or_default(),
                Some(pr.checks_ok),
                Some(pr.has_checks),
            ) == RollupKind::Fail
    }

    fn toggle_check_group(&mut self, key: &str, compact: bool, cx: &mut Context<Self>) {
        let next = !check_group_collapsed(key, &self.collapsed_check_groups, compact);
        self.collapsed_check_groups.insert(key.to_string(), next);
        cx.notify();
    }

    /// The checks card. `scroll` is the conversation's compact variant.
    pub(crate) fn checks_card(
        &mut self,
        pr: &Pr,
        scroll: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let checks = pr.checks.clone().unwrap_or_default();
        let g = group_checks(&checks);
        let roll = checks_rollup(&checks, Some(pr.checks_ok), Some(pr.has_checks));
        let (color, name) = rollup_look(roll, &t);
        let gh = pr.source == PrSource::GitHub;
        let can_fix = Self::can_fix_checks(pr);
        let lh = recur_ui::style::button_label("x", 11., 400.)
            .measure(None, window)
            .2
            .height;
        let deg = if roll == RollupKind::Pending {
            spin_deg(window, cx)
        } else {
            0.
        };
        let kind = rollup_kind(roll);
        let heading = checks_heading(&checks, gh.then(|| GITHUB.label()));
        let href = if gh {
            GITHUB.checks_url(&pr.url)
        } else {
            String::new()
        };
        let mut head = div()
            .debug_selector(|| "checks-h".into())
            .flex()
            .items_center()
            .gap(px(9.))
            .py(px(11.))
            .px(px(15.))
            .border_b_1()
            .border_color(hsla(t.line))
            .child(
                div()
                    .debug_selector(move || format!("checks-h-ico={kind}"))
                    .flex_none()
                    .text_color(hsla(color))
                    .child(icon(name).size(px(15.)).rotate(deg)),
            );
        let head_text = text(heading.clone()).size(12.5).weight(600.).nowrap();
        if href.is_empty() {
            head = head.child(
                div()
                    .debug_selector({
                        let h = heading.clone();
                        move || format!("checks-h-text={h}")
                    })
                    .child(head_text),
            );
        } else {
            let h2 = heading.clone();
            let link = href.clone();
            head = head.child(titled(
                "checks-h-link-t",
                "Open run on GitHub",
                div()
                    .id("checks-h-link")
                    .debug_selector(move || format!("checks-h-link={h2}={link}"))
                    .cursor_pointer()
                    .child(head_text)
                    .hover(|s| s.underline())
                    .on_click(move |_, _, cx| recur_content::links::open_external_url(&href, cx)),
            ));
        }
        let mut byline = checks_box_byline(&g);
        if byline.is_empty() {
            byline = if gh {
                "GitHub checks on this PR".into()
            } else {
                "required before merge".into()
            };
        }
        head = head.child(
            div().ml_auto().child(
                text(byline)
                    .mono()
                    .size(10.)
                    .weight(400.)
                    .color(t.faint)
                    .nowrap(),
            ),
        );
        if can_fix {
            let pid = pr.id.clone();
            let state = self.state().clone();
            head = head.child(titled(
                "checks-fix-t",
                "Ask an agent to fix failing checks",
                note_btn("checks-fix", "Fix checks", false, false, t.pane, lh, cx)
                    .debug_selector(|| "checks-fix:Fix checks".into())
                    .on_click(move |_, _, cx| PrDetail::request_fix_checks(&state, &pid, "", cx)),
            ));
        }
        let groups: [(&str, &str, &Vec<Check>); 4] = [
            ("fail", "failed", &g.fail),
            ("pending", "in progress", &g.pending),
            ("pass", "successful", &g.pass),
            ("skip", "skipped", &g.skip),
        ];
        // `.checkrow:last-child`: the last element in the body loses its rule.
        let mut parts: Vec<(bool, AnyElement)> = Vec::new();
        let now = PrDetail::check_now(cx);
        for (key, label, items) in groups {
            if items.is_empty() {
                continue;
            }
            let collapsed = check_group_collapsed(key, &self.collapsed_check_groups, scroll);
            let k = key.to_string();
            parts.push((
                false,
                div()
                    .id(SharedString::from(format!("checks-group-{key}-{scroll}")))
                    .debug_selector(move || {
                        format!(
                            "checks-group-h:{label}{}",
                            if collapsed { ":collapsed" } else { "" }
                        )
                    })
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .w_full()
                    .py(px(6.))
                    .px(px(15.))
                    .cursor_pointer()
                    .text_color(hsla(t.faint))
                    .hover(move |s| s.text_color(hsla(t.muted)))
                    .child(
                        icon(IconName::ChevronDown)
                            .size(px(12.))
                            .rotate(if collapsed { -90. } else { 0. }),
                    )
                    .child(
                        text(format!("{} {label}", items.len()))
                            .mono()
                            .size(9.5)
                            .line_height(LineHeight::Scale(1.2))
                            .tracking(0.08)
                            .uppercase()
                            .nowrap(),
                    )
                    .on_click(
                        cx.guarded_listener(move |d, _, _, cx| {
                            d.toggle_check_group(&k, scroll, cx)
                        }),
                    )
                    .into_any_element(),
            ));
            if collapsed {
                continue;
            }
            for (i, c) in items.iter().enumerate() {
                let (ic, ic_color, spin) = check_icon(c, &t);
                let deg = if spin { spin_deg(window, cx) } else { 0. };
                let run = GITHUB.check_run_url(&c.url, &pr.url, pr.number, &c.status);
                let detail = live_check_detail(c, now, &mut self.clocks.borrow_mut());
                let name_text = text(c.name.clone()).size(12.).weight(500.).nowrap();
                let name_el: AnyElement = if run.is_empty() {
                    div().child(name_text).into_any_element()
                } else {
                    let r2 = run.clone();
                    titled(
                        SharedString::from(format!("check-cn-{key}-{i}-{scroll}")),
                        "Open on GitHub",
                        div()
                            .id(SharedString::from(format!("check-cn-a-{key}-{i}-{scroll}")))
                            .debug_selector({
                                let n = c.name.clone();
                                move || format!("check-cn-a:{n}={r2}")
                            })
                            .cursor_pointer()
                            .hover(|s| s.underline())
                            .child(name_text)
                            .on_click(move |_, _, cx| {
                                recur_content::links::open_external_url(&run, cx)
                            }),
                    )
                    .into_any_element()
                };
                let mut row = div()
                    .debug_selector({
                        let n = c.name.clone();
                        let d = detail.clone();
                        move || format!("checkrow:{n}={d}{}", if spin { ":spin" } else { "" })
                    })
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .py(px(10.))
                    .px(px(15.))
                    .text_size(px(12.))
                    .child(
                        div()
                            .flex_none()
                            .text_color(hsla(ic_color))
                            .child(icon(ic).size(px(14.)).stroke_width(2.4).rotate(deg)),
                    )
                    .child(name_el)
                    .child(
                        div()
                            .ml_auto()
                            .child(text(detail).mono().size(10.).color(t.faint).nowrap()),
                    );
                if can_fix && key == "fail" {
                    let pid = pr.id.clone();
                    let name = c.name.clone();
                    let state = self.state().clone();
                    row = row.child(titled(
                        SharedString::from(format!("check-fix-t-{i}-{scroll}")),
                        format!("Ask an agent to fix {}", c.name),
                        note_btn(
                            SharedString::from(format!("check-fix-{i}-{scroll}")),
                            "Fix",
                            false,
                            false,
                            t.pane,
                            lh,
                            cx,
                        )
                        .debug_selector({
                            let n = c.name.clone();
                            move || format!("checks-fix:Fix:{n}")
                        })
                        .on_click(move |_, _, cx| {
                            PrDetail::request_fix_checks(&state, &pid, &name, cx)
                        }),
                    ));
                }
                parts.push((true, row.into_any_element()));
            }
        }
        let n = parts.len();
        let mut body = div()
            .debug_selector(|| "checks-box-body".into())
            .pt(px(6.))
            .pb(px(8.))
            .flex()
            .flex_col();
        for (i, (is_row, el)) in parts.into_iter().enumerate() {
            let rule = is_row && i + 1 < n;
            body = body.child(
                div()
                    .when(rule, |d| d.border_b_1().border_color(hsla(t.line_soft)))
                    .child(el),
            );
        }
        let body: AnyElement = if scroll {
            div()
                .max_h(px(280.))
                .flex()
                .child(
                    recur_ui::widgets::scroll_area("checks-box-scroll")
                        .track(&self.checks_box_scroll)
                        .child(body),
                )
                .into_any_element()
        } else {
            body.into_any_element()
        };
        recur_ui::style::card(t.pane, t.line, 9.)
            .debug_selector(move || format!("checks-card:{}", if scroll { "conv" } else { "tab" }))
            .mb(px(14.))
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(head)
            .child(body)
            .into_any_element()
    }

    /// The Checks pane.
    pub(crate) fn checks_pane(
        &mut self,
        pr: Option<&Pr>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let mut wrap = div()
            .w_full()
            .max_w(px(680.))
            .mx_auto()
            .p(px(18.))
            .min_w_0()
            .flex()
            .flex_col();
        if let Some(pr) = pr {
            if self.checks_pending(cx) {
                wrap = wrap.child(super::skeleton::checks_skel(window, cx));
            } else {
                wrap = wrap.child(self.checks_card(pr, false, window, cx));
                let hint = if pr.source == PrSource::GitHub {
                    "These are the GitHub status checks on this PR."
                } else {
                    "Verify-in-browser is unique to Recur — the QA agent actually drove the running app. Evidence is in the Conversation tab."
                };
                wrap = wrap.child(
                    div().px(px(2.)).child(
                        text(hint)
                            .mono()
                            .size(10.5)
                            .color(t.faint)
                            .id("checks-hint"),
                    ),
                );
            }
        }
        let scroll = self.checks_scroll.clone();
        self.pane("checks", &scroll, wrap, cx)
    }
}
