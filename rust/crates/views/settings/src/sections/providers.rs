//! Providers: permissions, the Claude Code and Cursor accounts, the OpenAI group (ChatGPT,
//! the Codex runtime and the OpenAI API endpoint), and OpenAI-compatible endpoints.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, Context, Div, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use recur_code::view::skills::toggle_disabled_name;
use recur_domain::CompatibleProvider;
use recur_ui::guard::Guarded;
use recur_ui::style::progress;
use recur_ui::text::{Span, TextStyled, span, text};
use recur_ui::widgets::{SettingsGroup, ToastKind, settings_group, settings_groups};
use recur_ui::{IconName, icon};
use recur_view_core::models::provider_enabled;
use recur_view_core::{bridge_error_message, call, toast};

use crate::field::{FieldEvent, FieldOpts};
use crate::rules::{
    PROVIDER_CHATGPT, PROVIDER_CLAUDE, PROVIDER_CODEX, PROVIDER_CURSOR, can_disable_provider,
};
use crate::style::{
    Head, InpStyle, Row, RowsCtx, code, copy, ctrl, desc, desc_text, field_msg, inp, rows,
    status_head, status_table, tbtn, tbtn_icon, tbtn_label, tog, tokens,
};
use crate::view::SettingsView;

const SIDES: RowsCtx = RowsCtx {
    sides: true,
    in_group: true,
    more: false,
};

/// `.acct-status`: a column with 10px gaps.
fn acct() -> Div {
    div().flex().flex_col().gap(px(10.)).w_full().min_w_0()
}

impl SettingsView {
    pub(crate) fn providers_pane(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = self.settings(cx).cloned();
        let claude_on = self.provider_on(PROVIDER_CLAUDE, cx);
        let cursor_on = self.provider_on(PROVIDER_CURSOR, cx);
        let codex_on = self.provider_on(PROVIDER_CODEX, cx);
        let mut out = settings_groups();
        if let Some(s) = &s {
            let bypass = s.bypass_permissions;
            out = out.child(settings_group("grp-perms", "Permissions").child(rows(
                SIDES,
                vec![Row::new()
                    .child(copy(
                        "bypass",
                        "Bypass tool permissions",
                        Some(vec![span("Skip permission prompts in Claude Code and Codex")]),
                        cx,
                    ))
                    .child(ctrl().child(
                        tog("tog-bypass", "toggle bypass permissions", bypass, false, cx).on_click(
                            cx.guarded_listener(move |this, _, _, cx| {
                                this.patch(false, |s| s.bypass_permissions = !bypass, cx)
                                    .detach()
                            }),
                        ),
                    ))],
                window,
                cx,
            )));
        }
        let claude = self.claude_group(claude_on, window, cx);
        out = out.child(claude);
        if s.is_some() {
            out = out.child(self.cursor_group(cursor_on, window, cx));
            let compat = s
                .as_ref()
                .map(|s| s.compatible_providers.clone())
                .unwrap_or_default();
            let (api, others): (Vec<_>, Vec<_>) =
                compat.into_iter().partition(|p| p.id == "openai");
            out = out.child(self.openai_group(codex_on, api, window, cx));
            for p in others {
                let name = if p.name.is_empty() {
                    p.id.clone()
                } else {
                    p.name.clone()
                };
                let label = format!("Enable {name}");
                out = out.child(self.compat_group(&p, name, label, window, cx));
            }
            out = out.child(
                settings_group("grp-compat-add", "OpenAI-compatible").child(rows(
                    SIDES,
                    vec![Row::stack()
                        .child(copy(
                            "compat-add",
                            "Add an HTTP endpoint",
                            Some(vec![
                                span("Recur owns the coding loop and calls "),
                                code("POST /v1/responses", cx),
                                span(". Add OpenAI, OpenRouter, Groq, or any compatible server. Multiple providers are allowed."),
                            ]),
                            cx,
                        ))
                        .child(ctrl().child(
                            tbtn_label("compat-add", "Add provider", false, cx)
                                .debug_selector(|| "btn:Add provider".into())
                                .on_click(cx.guarded_listener(|this, _, window, cx| this.open_compat_modal(window, cx))),
                        ))],
                    window,
                    cx,
                )),
            );
        }
        out.into_any_element()
    }

    fn claude_group(
        &mut self,
        on: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let st = self.claude_status.clone();
        let mut status = acct();
        if self.claude_busy && st.is_none() {
            status = status.child(status_head(Head::Muted, "Checking…", cx));
        } else if let Some(c) = st.as_ref().filter(|c| c.authenticated) {
            status = status.child(status_head(Head::Ok, "Connected", cx));
            let mut items = Vec::new();
            for (dt, dd, titled) in [
                ("Path", &c.path, true),
                ("Version", &c.version, false),
                ("Provider", &c.provider, false),
                ("Login method", &c.login_method, false),
                ("Organization", &c.organization, false),
                ("Email", &c.email, false),
            ] {
                if !dd.is_empty() {
                    items.push((dt, dd.clone(), titled));
                }
            }
            status = status.child(status_table("claude", items, cx)).child(desc(
                "claude-desc",
                vec![
                    span("To switch accounts, run "),
                    code("claude auth login", cx),
                    span(" in your terminal."),
                ],
                cx,
            ));
        } else {
            let err = st
                .as_ref()
                .map(|c| c.error.clone())
                .filter(|e| !e.is_empty())
                .unwrap_or_else(|| "Not connected".into());
            status = status.child(status_head(Head::Bad, err, cx));
            let (path, version) = st
                .as_ref()
                .map(|c| (c.path.clone(), c.version.clone()))
                .unwrap_or_default();
            if !path.is_empty() || !version.is_empty() {
                let mut items = Vec::new();
                if !path.is_empty() {
                    items.push(("Path", path.clone(), true));
                }
                if !version.is_empty() {
                    items.push(("Version", version.clone(), false));
                }
                status = status.child(status_table("claude", items, cx));
            }
            let spans: Vec<Span> = if !version.is_empty() || !path.is_empty() {
                vec![
                    span("Run "),
                    code("claude auth login", cx),
                    span(" in your terminal, then reopen Settings."),
                ]
            } else {
                vec![
                    span("Install Claude Code and run "),
                    code("claude auth login", cx),
                    span(", then reopen Settings."),
                ]
            };
            status = status.child(desc("claude-desc", spans, cx));
        }
        let this = cx.entity().downgrade();
        settings_group("grp-claude", "Claude Code")
            .override_toggle(
                on,
                false,
                Some("Enable Claude Code".into()),
                move |on, _, cx| {
                    let _ = this.update(cx, |this, cx| {
                        this.set_provider_enabled(PROVIDER_CLAUDE, on, cx)
                    });
                },
            )
            .child(rows(SIDES, vec![Row::stack().child(status)], window, cx))
            .into_any_element()
    }

    fn cursor_group(
        &mut self,
        on: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = tokens(cx);
        let s = self.settings(cx).cloned().unwrap_or_default();
        let usage = self.cursor_usage(cx).cloned();
        let signed_in = self.cursor_signed_in(cx);
        let connecting = usage.as_ref().is_some_and(|u| u.connecting);
        let status = self.cursor_status.clone();
        let sdk_missing = on && signed_in && status.as_ref().is_some_and(|s| !s.bridge_installed);
        let name = status.as_ref().map(|s| s.name.clone()).unwrap_or_default();
        let email = usage
            .as_ref()
            .map(|u| u.email.clone())
            .filter(|e| !e.is_empty())
            .or_else(|| status.as_ref().map(|s| s.email.clone()))
            .unwrap_or_default();
        let busy = self.cursor_usage_busy;

        let head = if connecting {
            status_head(Head::Muted, "Waiting for browser sign-in…", cx)
        } else if sdk_missing {
            status_head(Head::Warn, "Connected, SDK not installed", cx)
        } else if signed_in {
            status_head(Head::Ok, "Connected", cx)
        } else {
            status_head(Head::Bad, "Not connected", cx)
        };
        let mut buttons = ctrl();
        if connecting {
            buttons = buttons.child(
                tbtn_label("cursor-cancel", "Cancel", false, cx)
                    .debug_selector(|| "btn:Cancel".into())
                    .on_click(cx.guarded_listener(|this, _, _, cx| {
                        let services = this.vcx.services.clone();
                        cx.background_executor()
                            .spawn(async move {
                                let _ = services.settings.cancel_cursor_usage_connect().await;
                            })
                            .detach();
                    })),
            );
        } else if signed_in {
            buttons = buttons
                .gap(px(8.))
                .child(
                    tbtn_label(
                        "cursor-reconnect",
                        if busy { "Opening…" } else { "Reconnect" },
                        busy,
                        cx,
                    )
                    .debug_selector(|| "btn:Reconnect".into())
                    .when(!busy, |d| {
                        d.on_click(cx.guarded_listener(|this, _, _, cx| this.connect_cursor(cx)))
                    }),
                )
                .child(
                    tbtn_label("cursor-disconnect", "Disconnect", busy, cx)
                        .debug_selector(|| "btn:Disconnect".into())
                        .when(!busy, |d| {
                            d.on_click(
                                cx.guarded_listener(|this, _, _, cx| this.disconnect_cursor(cx)),
                            )
                        }),
                );
        } else {
            buttons = buttons.child(
                tbtn_label(
                    "cursor-connect",
                    if busy { "Opening…" } else { "Connect" },
                    busy,
                    cx,
                )
                .debug_selector(|| "btn:Connect".into())
                .when(!busy, |d| {
                    d.on_click(cx.guarded_listener(|this, _, _, cx| this.connect_cursor(cx)))
                }),
            );
        }
        let mut acct_col = acct().child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .w_full()
                .min_w_0()
                .child(head)
                .child(buttons),
        );
        let installing = self.installing;
        let (msg, pct) = (self.install_msg.clone(), self.install_pct);
        if sdk_missing {
            let note_text = if installing {
                let m = if msg.is_empty() {
                    "Installing…".to_string()
                } else {
                    msg.clone()
                };
                if pct >= 0 {
                    format!("{m} · {pct}%")
                } else {
                    m
                }
            } else {
                "The Cursor SDK is not installed, so Cursor agents cannot run yet. About 50 MB to download.".into()
            };
            // `.set-note.warn` outranks `.acct-sdk-note`'s amber wash, border and icon color.
            let bg = recur_ui::rgba(231, 111, 81, 0.06);
            let border = recur_ui::rgba(231, 111, 81, 0.28);
            acct_col = acct_col.child(
                recur_ui::style::card(bg, border, 8.)
                    .debug_selector(|| "acct-sdk-note".into())
                    .mt(px(12.))
                    .flex()
                    .gap(px(10.))
                    .items_center()
                    .py(px(12.))
                    .px(px(13.))
                    .text_color(crate::style::hsla(t.text))
                    .child(
                        div()
                            .flex_none()
                            .child(icon(IconName::TriangleAlert).size(px(15.)).color(t.coral)),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            text(note_text)
                                .id("sdk-note-text")
                                .size(12.)
                                .leading(1.45)
                                .color(t.text),
                        ),
                    )
                    .child(
                        tbtn_label(
                            "sdk-note-install",
                            if installing {
                                "Installing…"
                            } else {
                                "Install SDK"
                            },
                            installing,
                            cx,
                        )
                        .debug_selector(|| "btn:Install SDK".into())
                        .when(!installing, |d| {
                            d.on_click(
                                cx.guarded_listener(|this, _, _, cx| this.install_bridge(cx)),
                            )
                        }),
                    ),
            );
        }
        if signed_in && (!name.is_empty() || !email.is_empty()) {
            let mut items = Vec::new();
            if !name.is_empty() {
                items.push(("Name", name.clone(), false));
            }
            if !email.is_empty() {
                items.push(("Email", email.clone(), false));
            }
            acct_col = acct_col.child(status_table("cursor", items, cx));
        }

        // SDK row.
        let sdk_desc = if installing {
            let m = if msg.is_empty() {
                "Installing…".to_string()
            } else {
                msg.clone()
            };
            if pct >= 0 {
                format!("{m} — {pct}%")
            } else {
                m
            }
        } else if let Some(st) = status.as_ref().filter(|s| s.bridge_installed) {
            let mut d = "Installed".to_string();
            if !st.bridge_version.is_empty() {
                d.push_str(&format!(" · v{}", st.bridge_version));
            }
            if st.update_available && !st.pinned_version.is_empty() {
                // Svelte trims the space opening the `{#if}` block.
                d.push_str(&format!("— update available (v{})", st.pinned_version));
            }
            d
        } else {
            "Not installed — about 50 MB to download, 155 MB on disk".into()
        };
        let mut sdk_copy = copy("sdk", "SDK", Some(vec![span(sdk_desc)]), cx);
        if installing && pct >= 0 {
            sdk_copy = sdk_copy.child(
                progress(pct as f32 / 100., cx).debug_selector(move || format!("prog:{pct}")),
            );
        }
        let sdk_label = if installing {
            "Installing…"
        } else if status.as_ref().is_some_and(|s| s.update_available) {
            "Update"
        } else if status.as_ref().is_some_and(|s| s.bridge_installed) {
            "Reinstall"
        } else {
            "Install"
        };
        let (strict, commits, prs) = (
            s.cursor_strict_config,
            s.cursor_attribute_commits,
            s.cursor_attribute_prs,
        );
        let list = vec![
            Row::stack().child(acct_col),
            Row::new().child(sdk_copy).child(ctrl().child(
                tbtn_label("sdk-install", sdk_label, installing, cx)
                    .debug_selector(move || format!("btn:SDK {sdk_label}"))
                    .when(!installing, |d| d.on_click(cx.guarded_listener(|this, _, _, cx| this.install_bridge(cx)))),
            )),
            Row::new()
                .child(copy(
                    "strict",
                    "Ignore project Cursor config",
                    Some(vec![
                        span("Skip this repo’s "),
                        code(".cursor/rules", cx),
                        span(" and "),
                        code("AGENTS.md", cx),
                        span(", and your "),
                        code("~/.cursor", cx),
                        span(" settings, when running Cursor agents"),
                    ]),
                    cx,
                ))
                .child(ctrl().child(
                    tog("tog-strict", "toggle strict cursor config", strict, false, cx).on_click(
                        cx.guarded_listener(move |this, _, _, cx| {
                            this.patch(false, |s| s.cursor_strict_config = !strict, cx).detach()
                        }),
                    ),
                )),
            Row::new()
                .child(copy(
                    "attr-commits",
                    "Commit attribution",
                    Some(vec![
                        span("Let Cursor agents append a "),
                        code("Made with Cursor", cx),
                        span(" trailer to commits they create. Off by default."),
                    ]),
                    cx,
                ))
                .child(ctrl().child(
                    tog("tog-attr-commits", "toggle cursor commit attribution", commits, false, cx)
                        .on_click(cx.guarded_listener(move |this, _, _, cx| {
                            this.patch(false, |s| s.cursor_attribute_commits = !commits, cx)
                                .detach()
                        })),
                )),
            Row::new()
                .child(copy(
                    "attr-prs",
                    "PR attribution",
                    Some(vec![span(
                        "Let Cursor agents append attribution to pull requests they open. Off by default.",
                    )]),
                    cx,
                ))
                .child(ctrl().child(
                    tog("tog-attr-prs", "toggle cursor PR attribution", prs, false, cx).on_click(
                        cx.guarded_listener(move |this, _, _, cx| {
                            this.patch(false, |s| s.cursor_attribute_prs = !prs, cx).detach()
                        }),
                    ),
                )),
        ];
        let this = cx.entity().downgrade();
        settings_group("grp-cursor", "Cursor")
            .override_toggle(on, false, Some("Enable Cursor".into()), move |on, _, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.set_provider_enabled(PROVIDER_CURSOR, on, cx)
                });
            })
            .child(rows(SIDES, list, window, cx))
            .into_any_element()
    }

    /// One compatible endpoint's group and card.
    fn compat_group(
        &mut self,
        p: &CompatibleProvider,
        title: String,
        label: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> SettingsGroup {
        let on = provider_enabled(&self.disabled_providers(cx), &p.id);
        let card = self.compat_card(p, window, cx);
        let testing = self.compat_testing.get(&p.id).copied().unwrap_or(false);
        let msg = self.compat_test.get(&p.id).cloned().flatten();
        let props = format!("{:?}", (p, testing, &msg));
        if self.props_changed(&format!("compat:{}", p.id), props) {
            let pc = p.clone();
            card.update(cx, |c, cx| c.set(pc, testing, msg, cx));
        }
        let id = p.id.clone();
        let this = cx.entity().downgrade();
        settings_group(SharedString::from(format!("grp-compat-{}", p.id)), title)
            .override_toggle(on, false, Some(label.into()), move |on, _, cx| {
                let id = id.clone();
                let _ = this.update(cx, |this, cx| this.set_provider_enabled(&id, on, cx));
            })
            .child(card)
    }

    /// The OpenAI group: the ChatGPT account, the Codex runtime and the API endpoint.
    fn openai_group(
        &mut self,
        codex_on: bool,
        api: Vec<CompatibleProvider>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let chatgpt_on = self.provider_on(PROVIDER_CHATGPT, cx);
        let this = cx.entity().downgrade();
        let mut chatgpt = settings_group("grp-chatgpt", "ChatGPT account")
            .nested()
            .override_toggle(
                chatgpt_on,
                false,
                Some("Enable ChatGPT".into()),
                move |on, _, cx| {
                    let _ = this.update(cx, |this, cx| {
                        this.set_provider_enabled(PROVIDER_CHATGPT, on, cx)
                    });
                },
            );
        if chatgpt_on {
            chatgpt = chatgpt.child(self.chatgpt_card(window, cx));
        }
        let this = cx.entity().downgrade();
        let mut group = settings_group("grp-openai", "OpenAI")
            .blurb("Connect a ChatGPT account, use a Codex runtime, or configure an API endpoint.")
            .override_toggle(
                self.openai_on(cx),
                false,
                Some("Enable OpenAI".into()),
                move |on, _, cx| {
                    let _ = this.update(cx, |this, cx| this.set_openai_enabled(on, cx));
                },
            )
            .child(chatgpt)
            .child(self.codex_group(codex_on, window, cx));
        for p in api {
            group = group.child(
                self.compat_group(
                    &p,
                    "API endpoint".into(),
                    "Enable OpenAI API endpoint".into(),
                    window,
                    cx,
                )
                .nested(),
            );
        }
        group.into_any_element()
    }

    fn codex_group(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let s = self.settings(cx).cloned().unwrap_or_default();
        let st = self.codex_status.clone();
        let configured = self.codex_configured(cx);
        let path_table = |st: &Option<recur_services::api::CodexStatus>, cx: &gpui_kit::App| {
            st.as_ref()
                .filter(|c| !c.bin_path.is_empty())
                .map(|c| status_table("codex", vec![("Path", c.bin_path.clone(), true)], cx))
        };
        let signing_in = self.codex_login.signing_in();
        let mut status = acct();
        if self.codex_busy && st.is_none() {
            status = status.child(status_head(Head::Muted, "Checking…", cx));
        } else if let Some(c) = st.as_ref().filter(|c| !c.bin_installed) {
            let err = if c.error.is_empty() {
                "Codex binary not found".to_string()
            } else {
                c.error.clone()
            };
            status = status.child(status_head(Head::Bad, err, cx)).child(desc(
                "codex-desc",
                vec![span("Install Codex or set its native Binary path below.")],
                cx,
            ));
        } else if !configured {
            status = status
                .child(status_head(Head::Bad, "Not configured", cx))
                .children(path_table(&st, cx))
                .child(desc(
                    "codex-desc",
                    vec![span(
                        "Sign in with ChatGPT or add an API key below to connect Codex models.",
                    )],
                    cx,
                ));
        } else if let Some(c) = st.as_ref().filter(|c| c.authenticated) {
            let mut items = Vec::new();
            for (dt, dd, titled) in [
                ("Path", &c.bin_path, true),
                ("Login method", &c.login_method, false),
                ("Plan", &c.plan, false),
                ("Email", &c.email, false),
            ] {
                if !dd.is_empty() {
                    items.push((dt, dd.clone(), titled));
                }
            }
            status = status
                .child(status_head(Head::Ok, "Connected", cx))
                .child(status_table("codex", items, cx));
        } else {
            let err = st
                .as_ref()
                .map(|c| c.error.clone())
                .filter(|e| !e.is_empty())
                .unwrap_or_else(|| "Not connected".into());
            let installed = st.as_ref().is_some_and(|c| c.bin_installed);
            let spans = if !installed {
                vec![
                    span("Install with "),
                    code("brew install --cask codex", cx),
                    span(", or set a path / "),
                    code("$RECUR_CODEX", cx),
                    span("."),
                ]
            } else {
                vec![span(
                    "Sign in with ChatGPT below, reuse an existing CLI login, or set an API key.",
                )]
            };
            status = status
                .child(status_head(Head::Bad, err, cx))
                .children(path_table(&st, cx))
                .child(desc("codex-desc", spans, cx));
        }

        let chatgpt = s.codex_chat_gpt_auth;
        let login_row = self.codex_login_row(cx);
        let mut list = vec![
            Row::stack().child(status),
            login_row,
            Row::new()
                .child(copy(
                    "chatgpt",
                    "Use ChatGPT login",
                    Some(vec![
                        span("Use the account signed in above, or reuse an existing "),
                        code("codex login", cx),
                        span(" on this machine."),
                    ]),
                    cx,
                ))
                .child(
                    ctrl().child(
                        tog(
                            "tog-chatgpt",
                            "toggle Codex ChatGPT auth",
                            chatgpt,
                            signing_in,
                            cx,
                        )
                        .when(!signing_in, |d| {
                            d.on_click(cx.guarded_listener(move |this, _, _, cx| {
                                this.toggle_chatgpt(!chatgpt, cx)
                            }))
                        }),
                    ),
                ),
        ];
        if !chatgpt {
            let visible = self.codex_key_visible;
            let input = self.field("codex-key", FieldOpts::default(), window, cx);
            if let Some(f) = self.fields.get("codex-key") {
                f.set_masked(!visible, window, cx);
            }
            let draft = self.field_value("codex-key", cx);
            let testing = self.codex_key_testing;
            let dirty = self.codex_key_dirty;
            let mut key_row = Row::stack()
                .child(copy(
                    "codex-key",
                    "API key",
                    Some(vec![
                        span("Exported as "),
                        code("CODEX_API_KEY", cx),
                        span(". Stored locally in Recur’s data directory."),
                    ]),
                    cx,
                ))
                .child(
                    div()
                        .flex()
                        .items_stretch()
                        .gap(px(8.))
                        .w_full()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .debug_selector(|| "inp:Codex API key".into())
                                .child(inp(&input, &"sk-…".into(), InpStyle::BASE, window, cx)),
                        )
                        .child(recur_ui::tooltip::titled(
                            "codex-eye-tip",
                            if visible { "Hide" } else { "Show" },
                            tbtn("codex-eye", draft.is_empty(), cx)
                                .h_auto()
                                .debug_selector(move || {
                                    format!(
                                        "btn:{}",
                                        if visible {
                                            "Hide API key"
                                        } else {
                                            "Show API key"
                                        }
                                    )
                                })
                                .child(tbtn_icon(if visible {
                                    IconName::EyeOff
                                } else {
                                    IconName::Eye
                                }))
                                .when(!draft.is_empty(), |d| {
                                    d.on_click(cx.guarded_listener(|this, _, _, cx| {
                                        this.codex_key_visible = !this.codex_key_visible;
                                        cx.notify();
                                    }))
                                }),
                        ))
                        .child(
                            tbtn_label(
                                "codex-test",
                                "Test",
                                signing_in || testing || draft.is_empty(),
                                cx,
                            )
                            .h_auto()
                            .debug_selector(|| "btn:Test".into())
                            .when(
                                !(signing_in || testing || draft.is_empty()),
                                |d| {
                                    d.on_click(
                                        cx.guarded_listener(|this, _, _, cx| {
                                            this.test_codex_key(cx)
                                        }),
                                    )
                                },
                            ),
                        )
                        .child(
                            tbtn_label("codex-save", "Save", signing_in || !dirty || testing, cx)
                                .h_auto()
                                .debug_selector(|| "btn:Save".into())
                                .when(!signing_in && dirty && !testing, |d| {
                                    d.on_click(
                                        cx.guarded_listener(|this, _, _, cx| {
                                            this.save_codex_key(cx)
                                        }),
                                    )
                                }),
                        ),
                );
            if testing {
                key_row = key_row.child(field_msg("codex-msg", "Testing…", None, cx));
            } else if let Some((ok, msg)) = self.codex_key_test.clone() {
                key_row = key_row.child(field_msg("codex-msg", &msg, Some(ok), cx));
            }
            list.push(key_row);
        }
        let network = s.codex_network_access;
        list.push(
            Row::new()
                .child(copy(
                    "codex-net",
                    "Sandbox network access",
                    Some(vec![
                        span("Allow Codex Agent commands to use the network. Review/Ask/Plan already can (GitHub "),
                        code("gh", cx),
                        span(")."),
                    ]),
                    cx,
                ))
                .child(ctrl().child(
                    tog("tog-codex-net", "toggle Codex sandbox network access", network, false, cx)
                        .on_click(cx.guarded_listener(move |this, _, _, cx| {
                            this.patch(false, |s| s.codex_network_access = !network, cx).detach()
                        })),
                )),
        );
        let bin_desc = match st.as_ref().filter(|c| c.bin_installed) {
            Some(c) => desc_text(
                "codex-bin-desc",
                vec![span(if c.bin_path.is_empty() {
                    "Found".to_string()
                } else {
                    format!("Found · {}", c.bin_path)
                })],
                cx,
            ),
            None => desc_text(
                "codex-bin-desc",
                vec![
                    span("Install Codex from "),
                    span("official releases").color(tokens(cx).amber),
                    span(", or set a native binary path / "),
                    code("$RECUR_CODEX", cx),
                ],
                cx,
            )
            .on_click_span(1, |_, cx| {
                recur_content::links::open_external_url(
                    "https://github.com/openai/codex/releases",
                    cx,
                )
            }),
        };
        let bin_placeholder = st
            .as_ref()
            .map(|c| c.bin_path.clone())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| "/path/to/codex".into());
        let bin = self.bound_field(
            "codex-bin",
            &s.codex_bin_path,
            FieldOpts::default(),
            window,
            cx,
        );
        list.push(
            Row::stack()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(crate::style::title("Binary", cx))
                        .child(div().mt(px(3.)).child(bin_desc)),
                )
                .child(
                    div()
                        .flex()
                        .w_full()
                        .debug_selector(|| "inp:Codex binary path".into())
                        .child(inp(
                            &bin,
                            &bin_placeholder.into(),
                            InpStyle::BASE,
                            window,
                            cx,
                        )),
                ),
        );
        let this = cx.entity().downgrade();
        settings_group("grp-codex", "Codex runtime")
            .nested()
            .override_toggle(on, false, Some("Enable Codex".into()), move |on, _, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.set_provider_enabled(PROVIDER_CODEX, on, cx)
                });
            })
            .child(rows(SIDES, list, window, cx))
            .into_any_element()
    }

    // ---- actions ----

    /// `setProviderEnabled`: refuse to turn the last model provider off.
    pub(crate) fn set_provider_enabled(&mut self, slug: &str, on: bool, cx: &mut Context<Self>) {
        let Some(s) = self.settings(cx).cloned() else {
            return;
        };
        let codex_ready = self.codex_status.as_ref().is_some_and(|c| c.authenticated);
        if !on
            && !can_disable_provider(
                slug,
                &s.disabled_providers,
                self.cursor_signed_in(cx),
                codex_ready,
                &self.extra_ready(cx),
            )
        {
            toast(ToastKind::Error, "Keep at least one provider on", cx);
            return;
        }
        let slug = slug.to_string();
        let save = self.patch(
            true,
            move |s| s.disabled_providers = toggle_disabled_name(&s.disabled_providers, &slug, on),
            cx,
        );
        let state = self.vcx.state.clone();
        cx.spawn(async move |_, cx| {
            save.await;
            cx.update(|cx| state.update(cx, |s, cx| s.refresh_provider_usage(cx).detach()));
        })
        .detach();
    }

    /// The Claude probe: status, then a forced usage collect when the snapshot still shows an
    /// auth error after a good login.
    pub(crate) fn probe_claude(&mut self, g: u64, cx: &mut Context<Self>) {
        self.claude_busy = true;
        let state = self.vcx.state.clone();
        let painted = state.update(cx, |s, cx| s.prefetch_provider_usage(cx));
        let services = self.vcx.services.clone();
        cx.spawn(async move |this, cx| {
            let res = services.settings.get_claude_status().await;
            let authed = matches!(&res, Ok(s) if s.authenticated);
            let _ = this.update(cx, |this, cx| {
                if this.fx.live("claude", g) {
                    this.claude_status = res.ok();
                    cx.notify();
                }
            });
            painted.await;
            let live = this
                .update(cx, |this, _| this.fx.live("claude", g))
                .unwrap_or(false);
            if live && authed {
                cx.update(|cx| {
                    let s = state.read(cx);
                    let disabled = s
                        .settings
                        .as_ref()
                        .map(|s| s.disabled_providers.clone())
                        .unwrap_or_default();
                    if recur_view_core::provider_usage::snapshot_has_provider_error(
                        s.provider_usage.as_ref(),
                        &disabled,
                    ) {
                        state.update(cx, |s, cx| s.refresh_provider_usage(cx).detach());
                    }
                });
            }
            let _ = this.update(cx, |this, cx| {
                if this.fx.live("claude", g) {
                    this.claude_busy = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The Cursor probe; the backend drops an orphaned key, so settings reload only when the
    /// stored key changed.
    pub(crate) fn probe_cursor(&mut self, prev: String, g: u64, cx: &mut Context<Self>) {
        let services = self.vcx.services.clone();
        let state = self.vcx.state.clone();
        cx.spawn(async move |this, cx| {
            let res = services.settings.get_cursor_status().await;
            let ok = res.is_ok();
            let live = this
                .update(cx, |this, cx| {
                    let live = this.fx.live("cursor", g);
                    if live {
                        this.cursor_status = res.ok();
                        cx.notify();
                    }
                    live
                })
                .unwrap_or(false);
            if !live || !ok || prev.is_empty() {
                return;
            }
            if let Ok(next) = services.settings.get_settings().await {
                let live = this
                    .update(cx, |this, _| this.fx.live("cursor", g))
                    .unwrap_or(false);
                if live && next.cursor_api_key != prev {
                    cx.update(|cx| state.update(cx, |s, cx| s.set_settings(next, cx)));
                }
            }
        })
        .detach();
    }

    pub(crate) fn probe_codex(&mut self, g: u64, cx: &mut Context<Self>) {
        self.codex_busy = true;
        let services = self.vcx.services.clone();
        cx.spawn(async move |this, cx| {
            let res = services.settings.get_codex_status().await;
            let _ = this.update(cx, |this, cx| {
                if this.fx.live("codex", g) {
                    this.codex_status = res.ok();
                    this.codex_busy = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn connect_cursor(&mut self, cx: &mut Context<Self>) {
        self.cursor_usage_busy = true;
        cx.notify();
        let services = self.vcx.services.clone();
        cx.spawn(async move |this, cx| {
            match services.settings.connect_cursor_usage().await {
                Ok(r) if !r.error.is_empty() => {
                    cx.update(|cx| toast(ToastKind::Error, r.error, cx))
                }
                Ok(r) if !r.login_url.is_empty() => {
                    cx.update(|cx| recur_content::links::open_external_url(&r.login_url, cx))
                }
                Ok(_) => {}
                Err(e) => {
                    let m = bridge_error_message(&e);
                    let m = if m.is_empty() {
                        "Could not start Cursor sign-in".to_string()
                    } else {
                        m
                    };
                    cx.update(|cx| toast(ToastKind::Error, m, cx));
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.cursor_usage_busy = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn disconnect_cursor(&mut self, cx: &mut Context<Self>) {
        self.cursor_usage_busy = true;
        cx.notify();
        let services = self.vcx.services.clone();
        let state = self.vcx.state.clone();
        cx.spawn(async move |this, cx| {
            let res = async {
                services.settings.disconnect_cursor_usage().await?;
                let _ = this.update(cx, |this, cx| {
                    if let Some(s) = this.cursor_status.as_mut() {
                        s.authenticated = false;
                        s.name.clear();
                        s.email.clear();
                        s.account.clear();
                        s.error.clear();
                    }
                    cx.notify();
                });
                let next = services.settings.get_settings().await?;
                cx.update(|cx| {
                    state.update(cx, |s, cx| {
                        s.set_settings(next, cx);
                        s.prefetch_provider_usage(cx).detach();
                    })
                });
                Ok::<_, recur_services::Error>(())
            }
            .await;
            if let Err(e) = res {
                let m = bridge_error_message(&e);
                let m = if m.is_empty() {
                    "Could not disconnect Cursor".to_string()
                } else {
                    m
                };
                cx.update(|cx| toast(ToastKind::Error, m, cx));
            }
            let _ = this.update(cx, |this, cx| {
                this.cursor_usage_busy = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// `installBridge`: progress arrives as `cursor.install` events while it runs.
    pub fn install_bridge(&mut self, cx: &mut Context<Self>) {
        self.installing = true;
        self.install_msg = "Starting…".into();
        self.install_pct = -1;
        cx.notify();
        let services = self.vcx.services.clone();
        cx.spawn(async move |this, cx| {
            let res = call(
                cx,
                services.settings.install_cursor_bridge(),
                "Could not install the Cursor SDK",
            )
            .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(s) = res {
                    let installed = s.bridge_installed;
                    this.cursor_status = Some(s);
                    if installed {
                        toast(ToastKind::Success, "Cursor SDK installed", cx);
                    }
                }
                this.installing = false;
                this.install_msg.clear();
                this.install_pct = -1;
                cx.notify();
            });
        })
        .detach();
    }

    fn toggle_chatgpt(&mut self, next: bool, cx: &mut Context<Self>) {
        self.codex_busy = true;
        if next {
            self.codex_key_dirty = false;
            self.codex_key_test = None;
            self.field_rewrite("codex-key", String::new());
        }
        let save = self.patch(
            true,
            move |s| {
                s.codex_chat_gpt_auth = next;
                if next {
                    s.codex_api_key.clear();
                }
            },
            cx,
        );
        self.refresh_codex_after(save, cx);
    }

    fn save_codex_key(&mut self, cx: &mut Context<Self>) {
        let draft = self.field_value("codex-key", cx);
        self.codex_key_dirty = false;
        self.codex_key_test = None;
        self.codex_busy = true;
        let save = self.patch(true, move |s| s.codex_api_key = draft, cx);
        self.refresh_codex_after(save, cx);
    }

    /// After a Codex save: re-probe when it saved.
    fn refresh_codex_after(
        &mut self,
        save: gpui_kit::Task<Option<recur_domain::Settings>>,
        cx: &mut Context<Self>,
    ) {
        cx.notify();
        let services = self.vcx.services.clone();
        cx.spawn(async move |this, cx| {
            if save.await.is_some() {
                let res = services.settings.get_codex_status().await;
                let _ = this.update(cx, |this, _| match res {
                    Ok(s) => this.codex_status = Some(s),
                    Err(_) => this.codex_status = None,
                });
            }
            let _ = this.update(cx, |this, cx| {
                this.codex_busy = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn test_codex_key(&mut self, cx: &mut Context<Self>) {
        self.codex_key_testing = true;
        self.codex_key_test = None;
        cx.notify();
        let draft = self.field_value("codex-key", cx);
        let chatgpt = self.settings(cx).is_some_and(|s| s.codex_chat_gpt_auth);
        let services = self.vcx.services.clone();
        cx.spawn(async move |this, cx| {
            let res = services.settings.test_codex_key(&draft, chatgpt).await;
            let msg = match res {
                Ok(s) if s.authenticated => {
                    let who = if s.email.is_empty() {
                        s.login_method
                    } else {
                        s.email
                    };
                    (
                        true,
                        if who.is_empty() {
                            "Connected successfully".to_string()
                        } else {
                            format!("Connected successfully as {who}")
                        },
                    )
                }
                Ok(s) => (
                    false,
                    if s.error.is_empty() {
                        "Key was rejected".into()
                    } else {
                        s.error
                    },
                ),
                Err(e) => {
                    let m = bridge_error_message(&e);
                    (
                        false,
                        if m.is_empty() {
                            "Could not reach Codex".into()
                        } else {
                            m
                        },
                    )
                }
            };
            let _ = this.update(cx, |this, cx| {
                this.codex_key_test = Some(msg);
                this.codex_key_testing = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// Codex field events.
    pub(crate) fn codex_field(
        &mut self,
        key: &str,
        ev: FieldEvent,
        value: String,
        cx: &mut Context<Self>,
    ) {
        match (key, ev) {
            ("codex-key", FieldEvent::Input) => {
                self.codex_key_dirty = true;
                self.codex_key_test = None;
            }
            ("codex-bin", FieldEvent::Change) => {
                self.patch(true, move |s| s.codex_bin_path = value, cx)
                    .detach();
            }
            _ => {}
        }
    }

    // ---- compatible providers ----

    /// `updateCompatible`: a change that leaves no model id keeps the saved models (or saves
    /// nothing when there were none).
    pub(crate) fn update_compatible(
        &mut self,
        mut next: CompatibleProvider,
        cx: &mut Context<Self>,
    ) {
        let list = self
            .settings(cx)
            .map(|s| s.compatible_providers.clone())
            .unwrap_or_default();
        if !next.models.iter().any(|m| !m.id.trim().is_empty()) {
            match list.iter().find(|p| p.id == next.id) {
                Some(prev) if prev.models.iter().any(|m| !m.id.trim().is_empty()) => {
                    next.models = prev.models.clone();
                }
                _ => return,
            }
        }
        let list: Vec<CompatibleProvider> = list
            .into_iter()
            .map(|p| if p.id == next.id { next.clone() } else { p })
            .collect();
        self.patch(true, move |s| s.compatible_providers = list, cx)
            .detach();
    }

    pub(crate) fn remove_compatible(&mut self, id: &str, cx: &mut Context<Self>) {
        let id = id.to_string();
        let keep = id.clone();
        self.patch(
            true,
            move |s| s.compatible_providers.retain(|p| p.id != keep),
            cx,
        )
        .detach();
        self.compat_test.remove(&id);
        cx.notify();
    }

    pub(crate) fn test_compatible(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(p) = self
            .settings(cx)
            .and_then(|s| s.compatible_providers.iter().find(|p| p.id == id).cloned())
        else {
            return;
        };
        let id = id.to_string();
        self.compat_testing.insert(id.clone(), true);
        self.compat_test.insert(id.clone(), None);
        cx.notify();
        let services = self.vcx.services.clone();
        cx.spawn(async move |this, cx| {
            let res = services.settings.test_compatible_provider(&p).await;
            let msg = match res {
                Ok(r) if r.ok => (true, "Connected".to_string()),
                Ok(r) => (
                    false,
                    if r.error.is_empty() {
                        "Could not reach endpoint".into()
                    } else {
                        r.error
                    },
                ),
                Err(e) => {
                    let m = bridge_error_message(&e);
                    (
                        false,
                        if m.is_empty() {
                            "Could not reach endpoint".into()
                        } else {
                            m
                        },
                    )
                }
            };
            let _ = this.update(cx, |this, cx| {
                this.compat_test.insert(id.clone(), Some(msg));
                this.compat_testing.insert(id, false);
                cx.notify();
            });
        })
        .detach();
    }
}
