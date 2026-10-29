//! The OpenAI group of Providers: direct ChatGPT sign-in, the Codex runtime's browser
//! sign-in, and the OpenAI enable switch.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use recur_code::view::skills::toggle_disabled_name;
use recur_services::api::ChatGPTStatus;
use recur_ui::guard::Guarded;
use recur_ui::text::span;
use recur_ui::widgets::{SelectOption, ToastKind};
use recur_view_core::models::provider_enabled;
use recur_view_core::{bridge_error_message, call, toast};

use crate::field::FieldOpts;
use crate::rules::{PROVIDER_CHATGPT, can_disable_provider};
use crate::style::{InpStyle, Row, code, copy, ctrl, desc, field_msg, inp, rows, tbtn_label};
use crate::view::SettingsView;

/// `ChatGPTProviderCard`'s state.
#[derive(Default)]
pub(crate) struct ChatGPTCard {
    pub status: Option<ChatGPTStatus>,
    pub error: String,
    pub loading: bool,
    pub starting: bool,
    pub login_id: String,
    pub auth_url: String,
    pub user_code: String,
    pub method: String,
    pub account: String,
    pub attempt: u64,
}

impl ChatGPTCard {
    fn signing_in(&self) -> bool {
        self.starting || !self.login_id.is_empty()
    }
}

/// The Codex runtime's browser sign-in.
#[derive(Default)]
pub(crate) struct CodexLogin {
    pub starting: bool,
    pub login_id: String,
    pub error: String,
    pub attempt: u64,
}

impl CodexLogin {
    pub(crate) fn signing_in(&self) -> bool {
        self.starting || !self.login_id.is_empty()
    }
}

fn msg_or(e: &recur_services::Error, fallback: &str) -> String {
    let m = bridge_error_message(e);
    if m.is_empty() { fallback.into() } else { m }
}

impl SettingsView {
    /// `openAIProviders`: ChatGPT, Codex and an `openai` API endpoint.
    pub(crate) fn openai_providers(&self, cx: &gpui_kit::App) -> Vec<String> {
        let mut out = vec![
            PROVIDER_CHATGPT.to_string(),
            crate::rules::PROVIDER_CODEX.to_string(),
        ];
        if self
            .settings(cx)
            .is_some_and(|s| s.compatible_providers.iter().any(|p| p.id == "openai"))
        {
            out.push("openai".into());
        }
        out
    }

    /// `setOpenAIEnabled`: every OpenAI runtime at once, refusing to turn the last provider off.
    pub(crate) fn set_openai_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        let Some(s) = self.settings(cx).cloned() else {
            return;
        };
        let codex_ready = self.codex_status.as_ref().is_some_and(|c| c.authenticated);
        let extra = self.extra_ready(cx);
        let cursor = self.cursor_signed_in(cx);
        let mut disabled = s.disabled_providers.clone();
        for slug in self.openai_providers(cx) {
            if !on && !can_disable_provider(&slug, &disabled, cursor, codex_ready, &extra) {
                toast(ToastKind::Error, "Keep at least one provider on", cx);
                return;
            }
            disabled = toggle_disabled_name(&disabled, &slug, on);
        }
        let save = self.patch(true, move |s| s.disabled_providers = disabled, cx);
        let state = self.vcx.state.clone();
        cx.spawn(async move |_, cx| {
            save.await;
            cx.update(|cx| state.update(cx, |s, cx| s.refresh_provider_usage(cx).detach()));
        })
        .detach();
    }

    // ---- ChatGPT account ----

    /// The card mounted (load the account) or unmounted (cancel a sign-in).
    pub(crate) fn chatgpt_mounted(&mut self, on: bool, g: u64, cx: &mut Context<Self>) {
        if !on {
            self.chatgpt_cancel(cx);
            return;
        }
        self.chatgpt.loading = true;
        let services = self.vcx.services.clone();
        cx.spawn(async move |this, cx| {
            let res = services.settings.get_chat_gpt_status().await;
            let _ = this.update(cx, |this, cx| {
                if !this.fx.live("chatgpt", g) {
                    return;
                }
                match res {
                    Ok(next) => this.chatgpt_set_status(next),
                    Err(e) => this.chatgpt.error = msg_or(&e, "Could not read ChatGPT account"),
                }
                this.chatgpt.loading = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn chatgpt_set_status(&mut self, next: ChatGPTStatus) {
        self.chatgpt.account = if next.status.account_id.is_empty() {
            next.status
                .accounts
                .first()
                .map(|a| a.id.clone())
                .unwrap_or_default()
        } else {
            next.status.account_id.clone()
        };
        self.chatgpt.status = Some(next);
    }

    pub(crate) fn chatgpt_cancel(&mut self, cx: &mut Context<Self>) {
        let c = &mut self.chatgpt;
        c.attempt += 1;
        c.starting = false;
        c.auth_url.clear();
        c.user_code.clear();
        let id = std::mem::take(&mut c.login_id);
        cx.notify();
        if id.is_empty() {
            return;
        }
        let services = self.vcx.services.clone();
        cx.spawn(async move |_, cx| {
            call(
                cx,
                services.settings.cancel_chat_gpt_login(&id),
                "Could not cancel ChatGPT sign-in",
            )
            .await;
        })
        .detach();
    }

    /// Saved settings, then the status, unless a newer attempt started.
    async fn chatgpt_apply(
        this: &gpui_kit::WeakEntity<Self>,
        services: &recur_services::Services,
        next: ChatGPTStatus,
        current: u64,
        cx: &mut gpui_kit::AsyncApp,
    ) -> Result<(), recur_services::Error> {
        let saved = services.settings.get_settings().await?;
        let _ = this.update(cx, |this, cx| {
            if this.chatgpt.attempt != current {
                return;
            }
            this.vcx.state.update(cx, |s, cx| {
                s.set_settings(saved, cx);
                s.invalidate_model_catalog(cx);
            });
            this.chatgpt_set_status(next);
        });
        Ok(())
    }

    fn chatgpt_sign_in(&mut self, cx: &mut Context<Self>) {
        let c = &mut self.chatgpt;
        c.attempt += 1;
        let current = c.attempt;
        c.starting = true;
        c.error.clear();
        let (account, method) = (c.account.clone(), c.method.clone());
        cx.notify();
        let services = self.vcx.services.clone();
        cx.spawn(async move |this, cx| {
            let mut id = String::new();
            let res: Result<(), recur_services::Error> = async {
                let login = services
                    .settings
                    .start_chat_gpt_login(&account, &method)
                    .await?;
                id.clone_from(&login.login_id);
                let live = this
                    .update(cx, |this, cx| {
                        let c = &mut this.chatgpt;
                        if c.attempt != current {
                            return false;
                        }
                        c.login_id.clone_from(&login.login_id);
                        c.auth_url.clone_from(&login.auth_url);
                        c.user_code.clone_from(&login.user_code);
                        c.starting = false;
                        if login.method == "browser" {
                            this.chatgpt_open_page(cx);
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !live {
                    let _ = services
                        .settings
                        .cancel_chat_gpt_login(&login.login_id)
                        .await;
                    id.clear();
                    return Ok(());
                }
                let next = services
                    .settings
                    .wait_chat_gpt_login(&login.login_id)
                    .await?;
                Self::chatgpt_apply(&this, &services, next, current, cx).await?;
                let _ = this.update(cx, |this, cx| {
                    if this.chatgpt.attempt == current {
                        toast(ToastKind::Success, "ChatGPT connected", cx);
                    }
                });
                Ok(())
            }
            .await;
            if let Err(e) = res {
                let current_now = this
                    .update(cx, |this, cx| {
                        let live = this.chatgpt.attempt == current;
                        if live {
                            this.chatgpt.error = msg_or(&e, "Could not connect ChatGPT");
                            cx.notify();
                        }
                        live
                    })
                    .unwrap_or(false);
                // OAuth may have completed while model discovery failed. Keep that account
                // visible so Refresh models or reauthorization can recover.
                if current_now && let Ok(next) = services.settings.get_chat_gpt_status().await {
                    let _ = this.update(cx, |this, _| {
                        if this.chatgpt.attempt == current {
                            this.chatgpt_set_status(next);
                        }
                    });
                }
                if !id.is_empty() {
                    let _ = services.settings.cancel_chat_gpt_login(&id).await;
                }
            }
            let _ = this.update(cx, |this, cx| {
                let c = &mut this.chatgpt;
                if c.attempt == current {
                    c.starting = false;
                    c.login_id.clear();
                    c.auth_url.clear();
                    c.user_code.clear();
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn chatgpt_open_page(&mut self, cx: &mut Context<Self>) {
        self.chatgpt.error.clear();
        recur_content::links::open_external_url(&self.chatgpt.auth_url, cx);
    }

    fn chatgpt_update_account(&mut self, disconnect: bool, cx: &mut Context<Self>) {
        let c = &mut self.chatgpt;
        c.attempt += 1;
        let current = c.attempt;
        c.loading = true;
        c.error.clear();
        cx.notify();
        let services = self.vcx.services.clone();
        cx.spawn(async move |this, cx| {
            let res = async {
                let next = if disconnect {
                    services.settings.disconnect_chat_gpt().await?
                } else {
                    services.settings.refresh_chat_gpt_models().await?
                };
                Self::chatgpt_apply(&this, &services, next, current, cx).await
            }
            .await;
            let _ = this.update(cx, |this, cx| {
                if this.chatgpt.attempt == current {
                    if let Err(e) = res {
                        this.chatgpt.error = msg_or(&e, "Could not update ChatGPT account");
                    }
                    this.chatgpt.loading = false;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The `ChatGPTProviderCard` rows.
    pub(crate) fn chatgpt_card(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let signing_in = self.chatgpt.signing_in();
        let c = &self.chatgpt;
        let loading = c.loading;
        let status = c.status.clone().unwrap_or_default();
        let authed = c.status.as_ref().is_some_and(|s| s.status.authenticated);
        let accounts: Vec<SelectOption> = status
            .status
            .accounts
            .iter()
            .map(|a| {
                let email = if a.email.is_empty() {
                    "ChatGPT account"
                } else {
                    &a.email
                };
                SelectOption::new(a.id.clone(), format!("{email} ({})", a.id))
            })
            .chain([SelectOption::new("", "Another ChatGPT account")])
            .collect();
        let (account, method) = (c.account.clone(), c.method.clone());
        let props = format!(
            "{:?}",
            (&accounts, &account, &method, loading || signing_in)
        );
        if self.props_changed("chatgpt-selects", props) {
            let backdrop = (loading || signing_in).then_some(crate::style::tokens(cx).pane);
            self.chatgpt_account.update(cx, |s, cx| {
                s.set_options(accounts, cx);
                s.set_value(account, cx);
                s.set_disabled(backdrop, cx);
            });
            self.chatgpt_method.update(cx, |s, cx| {
                s.set_value(method, cx);
                s.set_disabled(backdrop, cx);
            });
        }
        let c = &self.chatgpt;
        let desc_text = if c.starting {
            "Starting sign-in…".to_string()
        } else if !c.login_id.is_empty() {
            if c.user_code.is_empty() {
                "Complete sign-in in your browser.".into()
            } else {
                "Open the sign-in link in any browser and enter the one-time code. Recur will connect automatically.".into()
            }
        } else if loading {
            "Checking ChatGPT account…".into()
        } else if authed {
            let email = if status.status.email.is_empty() {
                "Signed in"
            } else {
                &status.status.email
            };
            let n = status.model_count;
            format!(
                "{email} · {n} {} available",
                if n == 1 { "model" } else { "models" }
            )
        } else {
            "Use your ChatGPT plan directly. Recur runs the coding tools. No Codex installation or API key is needed.".into()
        };
        let err = if c.error.is_empty() {
            status.error.clone()
        } else {
            c.error.clone()
        };
        let sel = format!("chatgpt-status={desc_text}");
        let mut account_copy = copy(
            "chatgpt-account",
            if authed {
                "Connected"
            } else {
                "ChatGPT account"
            },
            Some(vec![span(desc_text)]),
            cx,
        )
        .debug_selector(move || sel.clone());
        if !err.is_empty() {
            let sel = format!("chatgpt-err={err}");
            account_copy = account_copy.child(
                field_msg("chatgpt-err", &err, Some(false), cx).debug_selector(move || sel.clone()),
            );
        }
        let buttons = if signing_in {
            ctrl()
                .gap(px(8.))
                .when(!c.auth_url.is_empty(), |d| {
                    d.child(
                        tbtn_label("chatgpt-open", "Open sign-in page", false, cx)
                            .debug_selector(|| "btn:Open sign-in page".into())
                            .on_click(cx.guarded_listener(|this, _, _, cx| {
                                this.chatgpt_open_page(cx);
                                cx.notify();
                            })),
                    )
                })
                .child(
                    tbtn_label("chatgpt-cancel", "Cancel ChatGPT sign-in", false, cx)
                        .debug_selector(|| "btn:Cancel ChatGPT sign-in".into())
                        .on_click(cx.guarded_listener(|this, _, _, cx| this.chatgpt_cancel(cx))),
                )
        } else {
            ctrl().child(
                tbtn_label("chatgpt-continue", "Continue with ChatGPT", loading, cx)
                    .debug_selector(|| "btn:Continue with ChatGPT".into())
                    .when(!loading, |d| {
                        d.on_click(cx.guarded_listener(|this, _, _, cx| this.chatgpt_sign_in(cx)))
                    }),
            )
        };
        let mut list = vec![
            Row::new()
                .child(copy(
                    "chatgpt-method",
                    "Sign-in method",
                    Some(vec![span(
                        "Use a browser callback, or a headless device code that works from any browser.",
                    )]),
                    cx,
                ))
                .child(ctrl().child(self.chatgpt_method.clone())),
            Row::new().child(account_copy).child(buttons),
        ];
        let (login_id, auth_url, user_code) =
            (c.login_id.clone(), c.auth_url.clone(), c.user_code.clone());
        let have_accounts = !status.status.accounts.is_empty();
        if !login_id.is_empty() && !auth_url.is_empty() {
            let link =
                self.bound_field("chatgpt-link", &auth_url, FieldOpts::default(), window, cx);
            let mut row = Row::stack()
                .child(copy("chatgpt-link", "Sign-in link", None, cx))
                .child(
                    div()
                        .flex()
                        .w_full()
                        .debug_selector(|| "inp:ChatGPT sign-in link".into())
                        .child(inp(&link, &"".into(), InpStyle::BASE, window, cx)),
                );
            if !user_code.is_empty() {
                let sel = format!("chatgpt-code={user_code}");
                row = row.child(
                    desc(
                        "chatgpt-code",
                        vec![
                            span("One-time code: "),
                            code(user_code, cx),
                            span(". Expires in 15 minutes."),
                        ],
                        cx,
                    )
                    .debug_selector(move || sel.clone()),
                );
            }
            list.push(row);
        }
        if have_accounts {
            list.push(
                Row::new()
                    .child(copy(
                        "chatgpt-accounts",
                        "Sign-in account",
                        Some(vec![span(
                            "Sign in to a saved account or connect another ChatGPT account.",
                        )]),
                        cx,
                    ))
                    .child(ctrl().child(self.chatgpt_account.clone())),
            );
        }
        if authed {
            let off = loading || signing_in;
            list.push(
                Row::new()
                    .child(copy(
                        "chatgpt-access",
                        "Account access",
                        Some(vec![span(
                            "Credentials are stored locally in Recur’s data directory.",
                        )]),
                        cx,
                    ))
                    .child(
                        ctrl()
                            .gap(px(8.))
                            .child(
                                tbtn_label("chatgpt-refresh", "Refresh ChatGPT models", off, cx)
                                    .debug_selector(|| "btn:Refresh ChatGPT models".into())
                                    .when(!off, |d| {
                                        d.on_click(cx.guarded_listener(|this, _, _, cx| {
                                            this.chatgpt_update_account(false, cx)
                                        }))
                                    }),
                            )
                            .child(
                                tbtn_label("chatgpt-signout", "Sign out of ChatGPT", off, cx)
                                    .debug_selector(|| "btn:Sign out of ChatGPT".into())
                                    .when(!off, |d| {
                                        d.on_click(cx.guarded_listener(|this, _, _, cx| {
                                            this.chatgpt_update_account(true, cx)
                                        }))
                                    }),
                            ),
                    ),
            );
        }
        rows(
            crate::style::RowsCtx {
                sides: true,
                in_group: true,
                more: false,
            },
            list,
            window,
            cx,
        )
        .into_any_element()
    }

    // ---- Codex browser sign-in ----

    pub(crate) fn sign_in_codex(&mut self, cx: &mut Context<Self>) {
        let l = &mut self.codex_login;
        l.attempt += 1;
        let current = l.attempt;
        l.starting = true;
        l.error.clear();
        cx.notify();
        let services = self.vcx.services.clone();
        let state = self.vcx.state.clone();
        cx.spawn(async move |this, cx| {
            let mut id = String::new();
            let res: Result<(), recur_services::Error> = async {
                let login = services.settings.start_codex_login().await?;
                id.clone_from(&login.login_id);
                let live = this
                    .update(cx, |this, cx| {
                        if this.codex_login.attempt != current {
                            return false;
                        }
                        this.codex_login.login_id.clone_from(&login.login_id);
                        this.codex_login.starting = false;
                        recur_content::links::open_external_url(&login.auth_url, cx);
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !live {
                    let _ = services.settings.cancel_codex_login(&login.login_id).await;
                    id.clear();
                    return Ok(());
                }
                let status = services.settings.wait_codex_login(&login.login_id).await?;
                if !this
                    .update(cx, |this, _| this.codex_login.attempt == current)
                    .unwrap_or(false)
                {
                    return Ok(());
                }
                let next = services.settings.get_settings().await?;
                let _ = this.update(cx, |this, cx| {
                    if this.codex_login.attempt != current {
                        return;
                    }
                    this.codex_status = Some(status);
                    this.codex_key_dirty = false;
                    this.codex_key_test = None;
                    this.field_rewrite("codex-key", String::new());
                    state.update(cx, |s, cx| {
                        s.set_settings(next, cx);
                        s.invalidate_model_catalog(cx);
                        s.refresh_provider_usage(cx).detach();
                    });
                    toast(ToastKind::Success, "Signed in to Codex", cx);
                });
                Ok(())
            }
            .await;
            if let Err(e) = res {
                let _ = this.update(cx, |this, cx| {
                    if this.codex_login.attempt == current {
                        this.codex_login.error = msg_or(&e, "Could not sign in to Codex");
                        cx.notify();
                    }
                });
                if !id.is_empty() {
                    let _ = services.settings.cancel_codex_login(&id).await;
                }
            }
            let _ = this.update(cx, |this, cx| {
                if this.codex_login.attempt == current {
                    this.codex_login.starting = false;
                    this.codex_login.login_id.clear();
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn cancel_codex_login(&mut self, cx: &mut Context<Self>) {
        let l = &mut self.codex_login;
        l.attempt += 1;
        l.starting = false;
        let id = std::mem::take(&mut l.login_id);
        cx.notify();
        if id.is_empty() {
            return;
        }
        let services = self.vcx.services.clone();
        cx.spawn(async move |_, cx| {
            call(
                cx,
                services.settings.cancel_codex_login(&id),
                "Could not cancel Codex sign-in",
            )
            .await;
        })
        .detach();
    }

    /// The Codex runtime's "ChatGPT account" row.
    pub(crate) fn codex_login_row(&mut self, cx: &mut Context<Self>) -> Row {
        let l = &self.codex_login;
        let signing_in = l.signing_in();
        let text = if l.starting {
            "Starting sign-in…"
        } else if !l.login_id.is_empty() {
            "Complete sign-in in your browser. This can take up to five minutes."
        } else {
            "Sign in through your browser to use your ChatGPT plan. Credentials are saved in Recur’s data directory."
        };
        let mut c = copy("codex-login", "ChatGPT account", Some(vec![span(text)]), cx);
        if !l.error.is_empty() {
            let sel = format!("codex-login-err={}", l.error);
            c = c.child(
                field_msg("codex-login-err", &l.error, Some(false), cx)
                    .debug_selector(move || sel.clone()),
            );
        }
        let button = if signing_in {
            tbtn_label("codex-login-cancel", "Cancel sign-in", false, cx)
                .debug_selector(|| "btn:Cancel sign-in".into())
                .on_click(cx.guarded_listener(|this, _, _, cx| this.cancel_codex_login(cx)))
        } else {
            let switch = self.codex_status.as_ref().is_some_and(|s| s.authenticated)
                && self.settings(cx).is_some_and(|s| s.codex_chat_gpt_auth);
            let label = if switch {
                "Switch ChatGPT account"
            } else {
                "Sign in with ChatGPT"
            };
            let testing = self.codex_key_testing;
            tbtn_label("codex-login", label, testing, cx)
                .debug_selector(move || format!("btn:{label}"))
                .when(!testing, |d| {
                    d.on_click(cx.guarded_listener(|this, _, _, cx| this.sign_in_codex(cx)))
                })
        };
        Row::new().child(c).child(ctrl().child(button))
    }

    /// `extraReady` gains ChatGPT once it is connected with models.
    pub(crate) fn chatgpt_ready(&self) -> bool {
        self.chatgpt
            .status
            .as_ref()
            .is_some_and(|s| s.status.authenticated && s.model_count > 0)
    }

    pub(crate) fn openai_on(&self, cx: &gpui_kit::App) -> bool {
        let disabled = self.disabled_providers(cx);
        self.openai_providers(cx)
            .iter()
            .any(|p| provider_enabled(&disabled, p))
    }
}
