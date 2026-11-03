//! A filed plan as a compact card, the summary and step checklist with
//! file chips and dependencies. Step status is read live from the workspace's current plan
//! (matched by id and title) so the card ticks along as the board runs.

use gpui_kit::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use recur_domain::PlanStep;
use recur_ui::fonts::Family;
use recur_ui::guard::Guarded;
use recur_ui::shadow::{box_shadow, css_box_shadow};
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::{IconName, Theme, Tokens, icon};
use serde::Deserialize;

use crate::chat::AgentChat;
use crate::html_inline::{InlineStyle, inline_html};
use crate::style::{hsla, label};

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct PlanCardStep {
    pub id: String,
    pub title: String,
    pub files: Vec<String>,
    pub depends_on: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct PlanCardData {
    pub summary: String,
    pub steps: Vec<PlanCardStep>,
    pub source: String,
}

/// `JSON.parse(data)` with a `steps` array.
pub fn parse_plan_card(data: &str) -> Option<PlanCardData> {
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    v.get("steps")?.as_array()?;
    serde_json::from_value(v).ok()
}

/// The card matches the current plan revision: same length, every step by id and title.
pub fn is_current_plan(plan: &PlanCardData, current: &[PlanStep]) -> bool {
    current.len() == plan.steps.len()
        && plan
            .steps
            .iter()
            .all(|s| current.iter().any(|c| c.id == s.id && c.title == s.title))
}

/// `.sd`: the 6px amber running dot with its glow.
pub(crate) fn running_dot(t: &Tokens) -> AnyElement {
    div()
        .relative()
        .size(px(6.))
        .child(box_shadow(
            &[css_box_shadow(0., 0., 5., 0., t.amber)],
            3.,
            0.,
        ))
        .child(div().absolute().inset_0().rounded_full().bg(hsla(t.amber)))
        .into_any_element()
}

fn fallback_row(id: SharedString, fallback: &str, t: &Tokens) -> AnyElement {
    let st = InlineStyle {
        family: Family::Mono,
        ..InlineStyle::meta(11., t.faint, t)
    };
    div()
        .flex()
        .items_baseline()
        .gap(px(7.))
        .w_full()
        .min_w_0()
        .pt(px(3.))
        .pb(px(3.))
        .pl(px(2.))
        .pr(px(6.))
        .text_size(px(12.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .child(inline_html(id, fallback, st)),
        )
        .into_any_element()
}

impl AgentChat {
    pub(crate) fn plan_card(
        &mut self,
        fact_id: &str,
        data: &str,
        fallback: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let id = SharedString::from(format!("plan-{}-{fact_id}", self.props.reset_key));
        let Some(plan) = parse_plan_card(data) else {
            return fallback_row(id, fallback, &t);
        };
        let ws_id = self.props.workspace_id.clone();
        let state = self.state.read(cx);
        let ws = state.workspaces.iter().find(|w| w.id == ws_id).cloned();
        let current: Vec<PlanStep> = ws
            .as_ref()
            .and_then(|w| w.plan.as_ref())
            .map(|p| p.steps.clone())
            .unwrap_or_default();
        let is_current = is_current_plan(&plan, &current);
        let status = |sid: &str| -> String {
            if !is_current {
                return String::new();
            }
            current
                .iter()
                .find(|c| c.id == sid)
                .map(|c| c.status.clone())
                .unwrap_or_else(|| "pending".into())
        };
        let live_child = |sid: &str| -> Option<String> {
            if !is_current {
                return None;
            }
            let s = current.iter().find(|c| c.id == sid)?;
            if s.status != "doing" || s.workspace.is_empty() {
                return None;
            }
            state
                .workspaces
                .iter()
                .find(|w| w.id == s.workspace && !w.archived)
                .map(|w| w.id.clone())
        };
        let done = plan
            .steps
            .iter()
            .filter(|s| matches!(status(&s.id).as_str(), "done" | "skipped"))
            .count();
        let can_open = !ws_id.is_empty() && ws.as_ref().is_some_and(|w| w.plan.is_some());
        let n = plan.steps.len();
        let plural = if n == 1 { "" } else { "s" };
        let sum = if is_current {
            format!("{done} of {n} step{plural} done")
        } else {
            format!("{n} step{plural}")
        };
        let mut head = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(10.))
            .py(px(6.))
            .text_size(px(12.))
            .text_color(hsla(t.muted))
            .border_b_1()
            .border_color(hsla(t.line))
            .child(
                div()
                    .size(px(14.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(IconName::ListTodo).size(px(14.)).color(hsla(t.cyan))),
            )
            .child(label("Plan", Family::Ui, 12., 600.).color(t.text))
            .child(
                div()
                    .ml_auto()
                    .child(label(sum, Family::Ui, 11., 400.).color(t.muted)),
            );
        if can_open {
            let w = ws_id.clone();
            head = head.child(recur_ui::tooltip::titled(
                SharedString::from(format!("{id}-open-tip")),
                "Open the Plan panel",
                div()
                    .id(SharedString::from(format!("{id}-open")))
                    .px(px(4.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|s| s.bg(hsla(t.pane_2)))
                    .child(recur_ui::style::button_label("Open plan", 11., 400.).color(t.cyan))
                    .on_click(cx.guarded_listener(move |this, _, _, cx| {
                        this.open_plan_panel(&w, "details", cx)
                    })),
            ));
        }
        let mut card = recur_ui::style::card(t.pane, t.line, 8.)
            .overflow_hidden()
            .child(head);
        if !plan.summary.is_empty() {
            card = card.child(
                div().pt(px(8.)).px(px(10.)).pb(px(4.)).child(
                    text(plan.summary.clone())
                        .id(SharedString::from(format!("{id}-sum")))
                        .size(12.)
                        .line_height(LineHeight::Scale(1.45))
                        .color(t.text),
                ),
            );
        }
        let state_entity = self.state.clone();
        let mut steps = div().pt(px(4.)).pb(px(6.)).flex().flex_col();
        for s in &plan.steps {
            let st = status(&s.id);
            let done_like = st == "done" || st == "skipped";
            let title_text = {
                let mut tx = text(s.title.clone())
                    .id(SharedString::from(format!("{id}-st-{}", s.id)))
                    .break_anywhere()
                    .selectable(
                        crate::transcript::select_key(&format!("{id}-st-{}", s.id)),
                        "\n",
                    );
                tx = if done_like {
                    tx.color(t.muted).line_through(true)
                } else {
                    tx.color(t.text)
                };
                tx
            };
            let title_el: AnyElement = if can_open {
                let w = ws_id.clone();
                div()
                    .id(SharedString::from(format!("{id}-sl-{}", s.id)))
                    .cursor_pointer()
                    .child(recur_ui::tooltip::titled(
                        SharedString::from(format!("{id}-sl-tip-{}", s.id)),
                        "Open the Todos pane",
                        title_text,
                    ))
                    .on_click(cx.guarded_listener(move |this, _, _, cx| {
                        this.open_plan_panel(&w, "todos", cx)
                    }))
                    .into_any_element()
            } else {
                title_text.into_any_element()
            };
            let mut body = div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(title_el);
            if !s.files.is_empty() || !s.depends_on.is_empty() {
                let mut meta = div()
                    .flex()
                    .flex_wrap()
                    .gap(px(4.))
                    .text_size(px(10.5))
                    .text_color(hsla(t.faint));
                for f in &s.files {
                    let name = f.rsplit('/').next().unwrap_or(f).to_string();
                    meta = meta.child(recur_ui::tooltip::titled(
                        SharedString::from(format!("{id}-sf-{}-{f}", s.id)),
                        f.clone(),
                        div()
                            .bg(hsla(t.pane_2))
                            .rounded(px(3.))
                            .px(px(5.))
                            .py(px(1.))
                            .child(label(name, Family::Mono, 10.5, 400.).color(t.faint)),
                    ));
                }
                if !s.depends_on.is_empty() {
                    meta = meta.child(
                        div().px(px(2.)).py(px(1.)).child(
                            label(
                                format!("after {}", s.depends_on.join(", ")),
                                Family::Ui,
                                10.5,
                                400.,
                            )
                            .color(t.faint),
                        ),
                    );
                }
                body = body.child(meta);
            }
            let state_color = if st == "skipped" { t.faint } else { t.emerald };
            let mark: AnyElement = match st.as_str() {
                "done" => icon(IconName::CircleCheck)
                    .size(px(13.))
                    .color(hsla(state_color))
                    .into_any_element(),
                "skipped" => icon(IconName::CircleSlash)
                    .size(px(13.))
                    .color(hsla(state_color))
                    .into_any_element(),
                "doing" => match live_child(&s.id) {
                    Some(child) => {
                        let se = state_entity.clone();
                        recur_ui::tooltip::titled(
                            SharedString::from(format!("{id}-sc-tip-{}", s.id)),
                            "Open the child workspace working this step",
                            div()
                                .id(SharedString::from(format!("{id}-sc-{}", s.id)))
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .px(px(2.))
                                .cursor_pointer()
                                .text_color(hsla(state_color))
                                .child(running_dot(&t))
                                .child(icon(IconName::GitBranch).size(px(13.)))
                                .on_click(move |_, _, cx| {
                                    se.update(cx, |s, cx| s.open_workspace(&child, false, cx))
                                }),
                        )
                        .into_any_element()
                    }
                    None => running_dot(&t),
                },
                _ => div().into_any_element(),
            };
            steps = steps.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .px(px(10.))
                    .py(px(4.))
                    .text_size(px(12.))
                    .child(
                        div()
                            .flex_none()
                            .mt(px(1.))
                            .bg(hsla(t.pane_2))
                            .rounded(px(4.))
                            .px(px(5.))
                            .py(px(2.))
                            .child(label(s.id.clone(), Family::Mono, 10., 400.).color(t.faint)),
                    )
                    .child(div().flex_1().min_w_0().child(body))
                    .child(
                        div()
                            .flex_none()
                            .w(px(16.))
                            .h(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(mark),
                    ),
            );
        }
        card.child(steps).into_any_element()
    }

    pub(crate) fn open_plan_panel(
        &mut self,
        workspace_id: &str,
        tab: &str,
        cx: &mut Context<Self>,
    ) {
        let (ws, tab) = (workspace_id.to_string(), tab.to_string());
        self.state.update(cx, |s, cx| {
            s.observatory_jump = Some(recur_view_core::ObsJump {
                panel: "plan".into(),
                workspace_id: ws,
                plan_tab: tab,
                ..Default::default()
            });
            cx.notify();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_matches_the_current_plan() {
        let p = parse_plan_card(r#"{"summary":"s","steps":[{"id":"S1","title":"a","files":["x/y.go"],"dependsOn":["S0"]}]}"#).unwrap();
        assert_eq!(p.steps[0].depends_on, ["S0"]);
        let cur = vec![PlanStep {
            id: "S1".into(),
            title: "a".into(),
            status: "done".into(),
            ..Default::default()
        }];
        assert!(is_current_plan(&p, &cur));
        let other = vec![PlanStep {
            id: "S1".into(),
            title: "b".into(),
            ..Default::default()
        }];
        assert!(!is_current_plan(&p, &other));
        assert!(parse_plan_card("{\"summary\":1}").is_none());
        assert!(parse_plan_card("nope").is_none());
    }
}
