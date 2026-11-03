//! MCP: the Linear server switch and your own HTTP and stdio servers.

use recur_ui::guard::Guarded;
use std::collections::BTreeMap;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use recur_agent::mcp::{
    custom_mcp_servers, linear_mcp_on, remove_mcp_server, set_custom_mcp_enabled, slug_mcp_id,
    update_mcp_server, upsert_linear_enabled, valid_mcp_id,
};
use recur_domain::{McpServer, SETTING_LINEAR_API_KEY};
use recur_ui::text::{TextStyled, span};
use recur_ui::widgets::{settings_group, settings_groups};

use crate::field::FieldOpts;
use crate::nav::Section;
use crate::rules::{mcp_auth_header, split_args};
use crate::style::{
    InpStyle, Row, RowsCtx, copy, ctrl_text, inp, lead, rows, skill_copy, skill_list, tbtn_label,
    tog, tokens,
};
use crate::view::SettingsView;

const LINEAR_MCP_ID: &str = "linear";

impl SettingsView {
    fn mcp_input(
        &mut self,
        key: &str,
        label: &'static str,
        placeholder: &str,
        masked: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let input = self.field(
            key,
            FieldOpts {
                masked,
                align_right: false,
            },
            window,
            cx,
        );
        div()
            .flex()
            .debug_selector(move || format!("inp:{label}"))
            .child(inp(
                &input,
                &placeholder.to_string().into(),
                InpStyle::BASE,
                window,
                cx,
            ))
            .into_any_element()
    }

    pub(crate) fn mcp_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = tokens(cx);
        let s = self.settings(cx).cloned();
        let servers: Vec<McpServer> = s
            .as_ref()
            .map(|s| s.mcp_servers.clone())
            .unwrap_or_default();
        let linear_on = linear_mcp_on(&servers);
        let key_set = s
            .as_ref()
            .and_then(|s| s.bundle_values.get(SETTING_LINEAR_API_KEY))
            .is_some_and(|v| !v.trim().is_empty());
        let custom = custom_mcp_servers(&servers);
        let has = s.is_some();

        let mut blurb = vec![span(
            "Search and update issues from chat. Uses the API key in Settings → Integrations. Ask, Plan, and Review can look up issues; Agent can change them.",
        )];
        let link = linear_on && !key_set;
        if link {
            blurb.push(span(" "));
            blurb.push(span("Connect Linear…").color(t.amber));
        }
        let state = self.vcx.state.clone();
        let mut linear = settings_group("grp-mcp-linear", "Linear")
            .blurb_spans(blurb)
            .override_toggle(linear_on, !has, Some("Enable Linear".into()), {
                let this = cx.entity().downgrade();
                move |on, _, cx| {
                    let _ = this.update(cx, |this, cx| this.set_linear_mcp(on, cx));
                }
            });
        if link {
            let st = state.clone();
            linear = linear.blurb_link(2, move |_, cx| {
                st.update(cx, |s, cx| {
                    s.settings_section = Section::Integrations.id().to_string();
                    cx.notify();
                })
            });
        }

        let mut yours = settings_group("grp-mcp-yours", "Your servers");
        if !custom.is_empty() {
            let mut list = Vec::new();
            for sv in &custom {
                let name = if sv.name.is_empty() {
                    sv.id.clone()
                } else {
                    sv.name.clone()
                };
                let d = if sv.transport == "stdio" {
                    if sv.command.is_empty() {
                        "stdio".to_string()
                    } else {
                        sv.command.clone()
                    }
                } else if sv.url.is_empty() {
                    "http".to_string()
                } else {
                    sv.url.clone()
                };
                let copy = skill_copy(&format!("mcp-{}", sv.id), name.clone(), Some(d), None, cx);
                let (edit, rm) = (sv.clone(), sv.id.clone());
                let id = sv.id.clone();
                let on = sv.enabled;
                let mut tg = tog(
                    SharedString::from(format!("tog-mcp-{}", sv.id)),
                    format!("{} {name}", if on { "Disable" } else { "Enable" }),
                    on,
                    !has,
                    cx,
                )
                .mt(px(2.));
                if has {
                    tg = tg.on_click(cx.guarded_listener(move |this, _, _, cx| {
                        this.set_mcp_enabled(&id, !on, cx)
                    }));
                }
                list.push((
                    copy,
                    vec![
                        tbtn_label(
                            SharedString::from(format!("mcp-edit-{}", sv.id)),
                            "Edit",
                            false,
                            cx,
                        )
                        .debug_selector({
                            let n = name.clone();
                            move || format!("btn:Edit {n}")
                        })
                        .on_click(
                            cx.guarded_listener(move |this, _, _, cx| {
                                this.start_mcp_edit(&edit, cx)
                            }),
                        )
                        .into_any_element(),
                        tbtn_label(
                            SharedString::from(format!("mcp-rm-{}", sv.id)),
                            "Remove",
                            false,
                            cx,
                        )
                        .debug_selector({
                            let n = name.clone();
                            move || format!("btn:Remove {n}")
                        })
                        .on_click(
                            cx.guarded_listener(move |this, _, _, cx| this.delete_mcp(&rm, cx)),
                        )
                        .into_any_element(),
                        tg.into_any_element(),
                    ],
                ));
            }
            yours = yours.child(skill_list(list, cx));
        } else if self.mcp.kind.is_empty() {
            yours = yours.child(ctrl_text("mcp-none", vec![span("None yet.")], window, cx));
        }
        if self.mcp.kind.is_empty() {
            yours =
                yours.child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .mt(px(12.))
                        .child(
                            tbtn_label("mcp-add-http", "Add HTTP", false, cx)
                                .debug_selector(|| "btn:Add HTTP".into())
                                .on_click(cx.guarded_listener(|this, _, _, cx| {
                                    this.open_mcp_form("http", cx)
                                })),
                        )
                        .child(
                            tbtn_label("mcp-add-stdio", "Add stdio", false, cx)
                                .debug_selector(|| "btn:Add stdio".into())
                                .on_click(cx.guarded_listener(|this, _, _, cx| {
                                    this.open_mcp_form("stdio", cx)
                                })),
                        ),
                );
        } else {
            let stdio = self.mcp.kind == "stdio";
            let editing_auth = !self.mcp.edit_id.is_empty()
                && custom
                    .iter()
                    .find(|c| c.id == self.mcp.edit_id)
                    .is_some_and(|c| c.headers.contains_key("Authorization"));
            let mut form = vec![
                Row::stack()
                    .child(copy("mcp-name", "Name", None, cx))
                    .child(self.mcp_input(
                        "mcp-name",
                        "Name",
                        if stdio { "filesystem" } else { "acme" },
                        false,
                        window,
                        cx,
                    )),
            ];
            if stdio {
                form.push(
                    Row::stack()
                        .child(copy("mcp-command", "Command", None, cx))
                        .child(self.mcp_input("mcp-command", "Command", "npx", false, window, cx)),
                );
                form.push(
                    Row::stack()
                        .child(copy("mcp-args", "Args", None, cx))
                        .child(self.mcp_input(
                            "mcp-args",
                            "Args",
                            "-y @example/mcp",
                            false,
                            window,
                            cx,
                        )),
                );
            } else {
                form.push(Row::stack().child(copy("mcp-url", "URL", None, cx)).child(
                    self.mcp_input(
                        "mcp-url",
                        "URL",
                        "https://example.com/mcp",
                        false,
                        window,
                        cx,
                    ),
                ));
                form.push(
                    Row::stack()
                        .child(copy(
                            "mcp-auth",
                            "Authorization",
                            Some(vec![span(if editing_auth {
                                "Leave blank to keep the current token."
                            } else {
                                "Optional. A Bearer token if the server needs one."
                            })]),
                            cx,
                        ))
                        .child(self.mcp_input(
                            "mcp-auth",
                            "Authorization",
                            "Bearer …",
                            true,
                            window,
                            cx,
                        )),
                );
            }
            let err = self.mcp.err.clone();
            let editing = !self.mcp.edit_id.is_empty();
            yours = yours.child(
                div()
                    .mt(px(12.))
                    .flex()
                    .flex_col()
                    .child(rows(
                        RowsCtx {
                            sides: false,
                            in_group: true,
                            more: true,
                        },
                        form,
                        window,
                        cx,
                    ))
                    .when(!err.is_empty(), |d| {
                        d.child(ctrl_text("mcp-err", vec![span(err)], window, cx))
                    })
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .mt(px(4.))
                            .child(
                                tbtn_label(
                                    "mcp-submit",
                                    if editing { "Save" } else { "Add" },
                                    false,
                                    cx,
                                )
                                .debug_selector(move || {
                                    format!("btn:{}", if editing { "Save" } else { "Add" })
                                })
                                .on_click(
                                    cx.guarded_listener(|this, _, _, cx| this.submit_mcp(cx)),
                                ),
                            )
                            .child(
                                tbtn_label("mcp-cancel", "Cancel", false, cx)
                                    .debug_selector(|| "btn:Cancel".into())
                                    .on_click(cx.guarded_listener(|this, _, _, cx| {
                                        this.reset_mcp_form();
                                        cx.notify();
                                    })),
                            ),
                    ),
            );
        }

        settings_groups()
            .child(lead(
                "mcp-lead",
                vec![span(
                    "Linear and other MCP tools chats can use. Each project can override these on its project page.",
                )],
                cx,
            ))
            .child(linear)
            .child(yours)
            .into_any_element()
    }

    /// `resetMCPForm`.
    fn reset_mcp_form(&mut self) {
        self.mcp = Default::default();
        for k in ["mcp-name", "mcp-url", "mcp-auth", "mcp-command", "mcp-args"] {
            self.field_rewrite(k, String::new());
        }
    }

    fn open_mcp_form(&mut self, kind: &str, cx: &mut Context<Self>) {
        self.reset_mcp_form();
        self.mcp.kind = kind.to_string();
        cx.notify();
    }

    fn set_linear_mcp(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.settings(cx).is_none() {
            return;
        }
        self.patch(
            false,
            move |s| s.mcp_servers = upsert_linear_enabled(&s.mcp_servers, on),
            cx,
        )
        .detach();
    }

    fn set_mcp_enabled(&mut self, id: &str, on: bool, cx: &mut Context<Self>) {
        let id = id.to_string();
        self.patch(
            false,
            move |s| s.mcp_servers = set_custom_mcp_enabled(&s.mcp_servers, &id, on),
            cx,
        )
        .detach();
    }

    fn delete_mcp(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.settings(cx).is_none() {
            return;
        }
        if self.mcp.edit_id == id {
            self.reset_mcp_form();
        }
        let id = id.to_string();
        self.patch(
            false,
            move |s| s.mcp_servers = remove_mcp_server(&s.mcp_servers, &id),
            cx,
        )
        .detach();
        cx.notify();
    }

    /// `startMCPEdit`: the form over the server's values (the token is never shown).
    fn start_mcp_edit(&mut self, s: &McpServer, cx: &mut Context<Self>) {
        self.mcp.edit_id = s.id.clone();
        self.mcp.kind = if s.transport == "stdio" {
            "stdio"
        } else {
            "http"
        }
        .into();
        self.mcp.err.clear();
        let name = if s.name.is_empty() {
            s.id.clone()
        } else {
            s.name.clone()
        };
        self.field_rewrite("mcp-name", name);
        self.field_rewrite("mcp-url", s.url.clone());
        self.field_rewrite("mcp-auth", String::new());
        self.field_rewrite("mcp-command", s.command.clone());
        self.field_rewrite("mcp-args", s.args.join(" "));
        cx.notify();
    }

    /// `submitMCPAdd`: validate, then add a server or save the one being edited.
    fn submit_mcp(&mut self, cx: &mut Context<Self>) {
        let Some(settings) = self.settings(cx).cloned() else {
            return;
        };
        let name = self.field_value("mcp-name", cx).trim().to_string();
        let url = self.field_value("mcp-url", cx).trim().to_string();
        let auth = self.field_value("mcp-auth", cx).trim().to_string();
        let command = self.field_value("mcp-command", cx).trim().to_string();
        let args = split_args(&self.field_value("mcp-args", cx));
        let stdio = self.mcp.kind == "stdio";
        if !self.mcp.edit_id.is_empty() {
            let Some(cur) = settings
                .mcp_servers
                .iter()
                .find(|s| s.id == self.mcp.edit_id)
                .cloned()
            else {
                self.mcp.err = "That server is gone.".into();
                cx.notify();
                return;
            };
            let mut patch = McpServer {
                name: if name.is_empty() {
                    cur.id.clone()
                } else {
                    name
                },
                transport: if self.mcp.kind.is_empty() {
                    if cur.transport.is_empty() {
                        "http".into()
                    } else {
                        cur.transport.clone()
                    }
                } else {
                    self.mcp.kind.clone()
                },
                ..Default::default()
            };
            if stdio {
                if command.is_empty() {
                    self.mcp.err = "Command is required.".into();
                    cx.notify();
                    return;
                }
                patch.command = command;
                patch.args = args;
                patch.url = String::new();
                patch.headers = BTreeMap::new();
            } else {
                if url.is_empty() {
                    self.mcp.err = "URL is required.".into();
                    cx.notify();
                    return;
                }
                patch.url = url;
                patch.command = String::new();
                patch.args = Vec::new();
                // simplify: an empty Authorization keeps the stored header (rotate by typing a
                // new token).
                patch.headers = if auth.is_empty() {
                    cur.headers.clone()
                } else {
                    BTreeMap::from([("Authorization".to_string(), mcp_auth_header(&auth))])
                };
            }
            self.mcp.err.clear();
            let id = self.mcp.edit_id.clone();
            self.reset_mcp_form();
            self.patch(
                false,
                move |s| {
                    s.mcp_servers = update_mcp_server(&s.mcp_servers, &id, |m| {
                        m.name = patch.name.clone();
                        m.transport = patch.transport.clone();
                        m.command = patch.command.clone();
                        m.args = patch.args.clone();
                        m.url = patch.url.clone();
                        m.headers = patch.headers.clone();
                    })
                },
                cx,
            )
            .detach();
            cx.notify();
            return;
        }
        let id = slug_mcp_id(&name);
        if !valid_mcp_id(&id) {
            self.mcp.err =
                "Name must start with a letter (a–z) and use letters, digits, or hyphens.".into();
            cx.notify();
            return;
        }
        if id == LINEAR_MCP_ID || settings.mcp_servers.iter().any(|s| s.id == id) {
            self.mcp.err = "That id is already in use.".into();
            cx.notify();
            return;
        }
        let mut row = McpServer {
            name: if name.is_empty() { id.clone() } else { name },
            id,
            enabled: true,
            transport: if self.mcp.kind.is_empty() {
                "http".into()
            } else {
                self.mcp.kind.clone()
            },
            ..Default::default()
        };
        if stdio {
            if command.is_empty() {
                self.mcp.err = "Command is required.".into();
                cx.notify();
                return;
            }
            row.command = command;
            row.args = args;
        } else {
            if url.is_empty() {
                self.mcp.err = "URL is required.".into();
                cx.notify();
                return;
            }
            row.url = url;
            if !auth.is_empty() {
                row.headers =
                    BTreeMap::from([("Authorization".to_string(), mcp_auth_header(&auth))]);
            }
        }
        self.reset_mcp_form();
        self.patch(false, move |s| s.mcp_servers.push(row), cx)
            .detach();
        cx.notify();
    }
}
