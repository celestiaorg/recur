//! General: Simplify, concurrency caps, agent turns, power, workspace branches and schedules.

use gpui_kit::{
    AnyElement, Context, InteractiveElement, IntoElement, MouseButton, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use recur_ui::guard::Guarded;
use recur_ui::text::span;
use recur_ui::widgets::{settings_group, settings_groups};

use crate::field::{FieldEvent, FieldOpts};
use crate::rules::{
    branch_prefix_example, int_text, js_round, number_input_value, parse_governor_cap,
    prefix_preset, saved_branch_prefix,
};
use crate::style::{InpStyle, Row, RowsCtx, code, copy, ctrl, inp, rows, seg, seg_btn, tog};
use crate::view::SettingsView;

const GROUP_ROWS: RowsCtx = RowsCtx {
    sides: true,
    in_group: true,
    more: false,
};

const NUM: FieldOpts = FieldOpts {
    masked: false,
    align_right: true,
};

impl SettingsView {
    /// A `.set-num` field showing `v`.
    fn num_field(
        &mut self,
        key: &str,
        label: &'static str,
        v: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let input = self.bound_field(key, &int_text(v), NUM, window, cx);
        ctrl()
            .child(
                div()
                    .debug_selector(move || format!("inp:{label}"))
                    .child(inp(
                        &input,
                        &SharedString::default(),
                        InpStyle::NUM,
                        window,
                        cx,
                    )),
            )
            .into_any_element()
    }

    pub(crate) fn general_pane(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(s) = self.settings(cx).cloned() else {
            return settings_groups().into_any_element();
        };
        let simplify = recur_engine::simplify::text::simplify_enabled(s.simplify);
        let simplify_row = Row::new()
            .child(copy(
                "simplify",
                "Default Simplify",
                Some(vec![span("On by default for new chats")]),
                cx,
            ))
            .child(
                ctrl().child(
                    tog(
                        "tog-simplify",
                        "toggle default simplify",
                        simplify,
                        false,
                        cx,
                    )
                    .on_click(cx.guarded_listener(move |this, _, _, cx| {
                        this.patch(false, |s| s.simplify = Some(!simplify), cx)
                            .detach();
                    })),
                ),
            );

        let loops = self.num_field("max-loops", "Max concurrent loops", s.max_loops, window, cx);
        let agents = self.num_field(
            "max-agents",
            "Max concurrent agents",
            s.max_agents,
            window,
            cx,
        );
        let timeout_v = if s.agent_turn_timeout_minutes == 0 {
            240
        } else {
            s.agent_turn_timeout_minutes
        };
        let timeout = self.num_field(
            "turn-timeout",
            "Agent turn time limit in minutes",
            timeout_v,
            window,
            cx,
        );
        let keep = self.num_field(
            "sched-keep",
            "Keep last N scheduled workspaces per schedule",
            s.scheduled_workspace_keep,
            window,
            cx,
        );

        let queue = s.queue_follow_ups;
        let sleep = s.prevent_sleep_while_agents_run;

        // Branch name prefix.
        let saved = saved_branch_prefix(Some(&s));
        let dirty = self.branch_prefix_dirty;
        let shown = if dirty {
            self.field_value("branch-prefix", cx)
        } else {
            saved.clone()
        };
        let branch_input =
            self.bound_field("branch-prefix", &shown, FieldOpts::default(), window, cx);
        let value_now = self.field_value("branch-prefix", cx);
        let example = branch_prefix_example(if dirty { &value_now } else { &saved });
        let login = self.github_login.clone();
        let preset = prefix_preset(&saved, dirty, &login);
        let mut presets = seg(cx).flex_none();
        for (kind, label) in [("recur", "Recur"), ("github", "GitHub"), ("none", "None")] {
            let on = preset == kind;
            let disabled = kind == "github" && login.is_empty();
            let mut b = seg_btn(
                SharedString::from(format!("prefix-{kind}")),
                label,
                on,
                disabled,
                11.,
                cx,
            )
            .debug_selector(move || {
                format!(
                    "seg:Prefix presets:{label}{}{}",
                    if on { ":on" } else { "" },
                    if disabled { ":off" } else { "" }
                )
            });
            if disabled {
                b = b.on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                });
            } else {
                b = b
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.guarded_listener(|this, _, _, _| this.skip_prefix_commit = true),
                    )
                    .on_click(
                        cx.guarded_listener(move |this, _, _, cx| {
                            this.pick_branch_prefix(kind, cx)
                        }),
                    );
            }
            let tip = if kind == "github" {
                Some(if login.is_empty() {
                    "Requires gh login".to_string()
                } else {
                    login.clone()
                })
            } else {
                None
            };
            presets = presets.child(match tip {
                Some(tip) => {
                    recur_ui::tooltip::titled("prefix-github-tip", tip, b).into_any_element()
                }
                None => b.into_any_element(),
            });
        }
        let shown_example = example.clone();
        let branch_row = Row::stack()
            .child(
                copy(
                    "branch",
                    "Branch name prefix",
                    Some(vec![
                        span("Prepended to new workspace branch names, e.g. "),
                        code(example, cx),
                        span(". Leave blank for none."),
                    ]),
                    cx,
                )
                .debug_selector(move || format!("branch-example={shown_example}")),
            )
            .child(
                div().flex().flex_col().gap(px(6.)).w_full().child(
                    div()
                        .flex()
                        .items_stretch()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .debug_selector(|| "inp:Branch name prefix".into())
                                .child(inp(
                                    &branch_input,
                                    &"No prefix".into(),
                                    InpStyle::BASE,
                                    window,
                                    cx,
                                )),
                        )
                        .child(presets),
                ),
            );

        settings_groups()
            .child(
                settings_group("grp-simplify", "Simplify")
                    .blurb("Keep agents from over-building. They reuse what’s already here and ship the smallest change that works.")
                    .child(rows(GROUP_ROWS, vec![simplify_row], window, cx)),
            )
            .child(
                settings_group("grp-concurrency", "Concurrency")
                    .blurb("How many loops and agent turns can run at once.")
                    .child(rows(
                        GROUP_ROWS,
                        vec![
                            Row::new()
                                .child(copy(
                                    "max-loops",
                                    "Max concurrent loops",
                                    Some(vec![span(
                                        "Further loops wait in queue until a slot frees. Default 4. 0 is unlimited.",
                                    )]),
                                    cx,
                                ))
                                .child(loops),
                            Row::new()
                                .child(copy(
                                    "max-agents",
                                    "Max concurrent agents",
                                    Some(vec![span(
                                        "Agent turns wait for a free slot. Default 8. 0 is unlimited.",
                                    )]),
                                    cx,
                                ))
                                .child(agents),
                        ],
                        window,
                        cx,
                    )),
            )
            .child(
                settings_group("grp-agents", "Agents")
                    .blurb("How interactive agents run.")
                    .child(rows(
                        GROUP_ROWS,
                        vec![
                            Row::new()
                                .child(copy(
                                    "turn-timeout",
                                    "Turn time limit",
                                    Some(vec![span(
                                        "Minutes before an agent chat turn is stopped. Default 240 (4 hours); max 10080 (7 days). Stop still cancels immediately.",
                                    )]),
                                    cx,
                                ))
                                .child(timeout),
                            Row::new()
                                .child(copy(
                                    "queue",
                                    "Queue follow-ups while an agent runs",
                                    Some(vec![span(if queue {
                                        "⏎ queues until the turn ends and ⌘⏎ steers."
                                    } else {
                                        "⏎ steers the running turn and ⌘⏎ queues."
                                    })]),
                                    cx,
                                ))
                                .child(ctrl().child(
                                    tog(
                                        "tog-queue",
                                        "toggle queue follow-ups while an agent runs",
                                        queue,
                                        false,
                                        cx,
                                    )
                                    .on_click(cx.guarded_listener(move |this, _, _, cx| {
                                        this.patch(false, |s| s.queue_follow_ups = !queue, cx)
                                            .detach()
                                    })),
                                )),
                        ],
                        window,
                        cx,
                    )),
            )
            .child(
                settings_group("grp-power", "Power")
                    .blurb("Keep the machine awake while agents work.")
                    .child(rows(
                        GROUP_ROWS,
                        vec![Row::new()
                            .child(copy(
                                "sleep",
                                "Prevent sleep while agents run",
                                Some(vec![span(
                                    "Uses macOS caffeinate so idle sleep cannot interrupt an agent turn",
                                )]),
                                cx,
                            ))
                            .child(ctrl().child(
                                tog(
                                    "tog-sleep",
                                    "toggle prevent sleep while agents run",
                                    sleep,
                                    false,
                                    cx,
                                )
                                .on_click(cx.guarded_listener(move |this, _, _, cx| {
                                    this.patch(false, |s| s.prevent_sleep_while_agents_run = !sleep, cx)
                                        .detach()
                                })),
                            ))],
                        window,
                        cx,
                    )),
            )
            .child(
                settings_group("grp-workspaces", "Workspaces")
                    .blurb("How new workspace branches are named, and how scheduled runs are kept.")
                    .child(rows(
                        GROUP_ROWS,
                        vec![
                            branch_row,
                            Row::new()
                                .child(copy(
                                    "keep",
                                    "Keep scheduled workspaces",
                                    Some(vec![span(
                                        "Keep the last N live workspaces per schedule. Older settled runs are removed. 0 keeps all.",
                                    )]),
                                    cx,
                                ))
                                .child(keep),
                        ],
                        window,
                        cx,
                    )),
            )
            .into_any_element()
    }

    /// The General pane's field events.
    pub(crate) fn general_field(
        &mut self,
        key: &str,
        ev: FieldEvent,
        value: String,
        cx: &mut Context<Self>,
    ) {
        match (key, ev) {
            ("branch-prefix", FieldEvent::Input) => {
                self.skip_prefix_commit = false;
                self.branch_prefix_dirty = true;
            }
            ("branch-prefix", FieldEvent::Focus) => self.skip_prefix_commit = false,
            ("branch-prefix", FieldEvent::Change) => self.commit_branch_prefix(&value, cx),
            ("max-loops", FieldEvent::Change) => {
                let n = parse_governor_cap(&value, 4);
                self.save_number(key, n, |s, n| s.max_loops = n, |s| s.max_loops, cx);
            }
            ("max-agents", FieldEvent::Change) => {
                let n = parse_governor_cap(&value, 8);
                self.save_number(key, n, |s, n| s.max_agents = n, |s| s.max_agents, cx);
            }
            ("turn-timeout", FieldEvent::Change) => {
                let n = js_round(number_input_value(&value)) as i64;
                self.save_number(
                    key,
                    n,
                    |s, n| s.agent_turn_timeout_minutes = n,
                    |s| match s.agent_turn_timeout_minutes {
                        0 => 240,
                        m => m,
                    },
                    cx,
                );
            }
            ("sched-keep", FieldEvent::Change) => {
                let n = js_round(number_input_value(&value)).max(0.) as i64;
                self.save_number(
                    key,
                    n,
                    |s, n| s.scheduled_workspace_keep = n,
                    |s| s.scheduled_workspace_keep,
                    cx,
                );
            }
            _ => {}
        }
    }

    /// Save a number field, then write the stored value back into the field (the backend may
    /// clamp to the value already stored, so the bound value would not change).
    fn save_number(
        &mut self,
        key: &str,
        n: i64,
        set: fn(&mut recur_domain::Settings, i64),
        shown: fn(&recur_domain::Settings) -> i64,
        cx: &mut Context<Self>,
    ) {
        let save = self.patch(false, move |s| set(s, n), cx);
        let key = key.to_string();
        cx.spawn(async move |this, cx| {
            let saved = save.await;
            let _ = this.update(cx, |this, cx| {
                let v = saved
                    .as_ref()
                    .or(this.settings(cx))
                    .map(shown)
                    .unwrap_or_default();
                this.field_rewrite(&key, int_text(v));
                cx.notify();
            });
        })
        .detach();
    }

    /// `commitBranchPrefix`.
    fn commit_branch_prefix(&mut self, raw: &str, cx: &mut Context<Self>) {
        self.branch_prefix_dirty = false;
        if self.skip_prefix_commit {
            self.skip_prefix_commit = false;
            return;
        }
        let v = raw.trim().to_string();
        if v == saved_branch_prefix(self.settings(cx)) {
            return;
        }
        self.patch(false, |s| s.branch_name_prefix = Some(v), cx)
            .detach();
    }

    /// `pickBranchPrefix`.
    fn pick_branch_prefix(&mut self, kind: &str, cx: &mut Context<Self>) {
        self.skip_prefix_commit = false;
        self.branch_prefix_dirty = false;
        let v = match kind {
            "github" => {
                if self.github_login.is_empty() {
                    return;
                }
                self.github_login.clone()
            }
            "none" => String::new(),
            _ => "recur".into(),
        };
        self.patch(false, |s| s.branch_name_prefix = Some(v), cx)
            .detach();
        cx.notify();
    }

    /// The General effect: `GitHubLogin` for the GitHub preset.
    pub(crate) fn load_github_login(&mut self, g: u64, cx: &mut Context<Self>) {
        let services = self.vcx.services.clone();
        cx.spawn(async move |this, cx| {
            let login = services.settings.github_login().await;
            let _ = this.update(cx, |this, cx| {
                if this.fx.live("section", g) {
                    this.github_login = login.map(|l| l.trim().to_string()).unwrap_or_default();
                    cx.notify();
                }
            });
        })
        .detach();
    }
}
