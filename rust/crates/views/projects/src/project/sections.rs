//! The project page's sections (`Project.svelte`'s `{#if section === …}` blocks) and the
//! `_settings.scss` rules only they use (`.proj-ident*`, `.copy-files*`, `.run-*`,
//! `.set-confirm*`).

use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Div, Hsla, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div, px, relative,
};
use recur_agent::mcp::{custom_mcp_servers, mcp_global_disabled};
use recur_code::view::skills::{
    SkillKind, SkillRow, effective_skill_enabled, project_skill_override_lists, skill_kind,
    skill_kind_override_count, skill_location, skill_override_hint,
};
use recur_domain::MCP_ID_LINEAR;
use recur_ui::fonts::weight;
use recur_ui::set::{
    InpStyle, Row, RowsCtx, code, copy, ctrl, ctrl_text, danger_btn, desc, inp, lead, mm_radio,
    note, radio_rows, radios_width, rm_btn, rows, seg, seg_btn, skill_copy, skill_list, tbtn,
    tbtn_icon, tbtn_label, tog,
};
use recur_ui::text::{Text, TextStyled, span, text};
use recur_ui::widgets::project_pic::{
    PROJECT_COLORS, ParsedPic, is_project_preset_color, parse_project_pic, project_pic_from_name,
};
use recur_ui::widgets::{PicBox, SelectOption, project_pic, settings_group, settings_groups};
use recur_ui::{IconName, Theme, Tokens, icon};

use super::{
    APP_URL, LETTER, LINEAR, NAME, ProjectView, REPO, Section, branch_options, copy_files_meta,
    run_name_key, style_area,
};
use crate::pic::{color_to_hex, parse_hex_color};
use recur_view_briefing::schedules::ScheduledPrompts;

const IN_GROUP: RowsCtx = RowsCtx {
    sides: false,
    in_group: true,
    more: false,
};

impl ProjectView {
    pub(crate) fn section_body(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.section {
            Section::General => self.general(window, cx),
            Section::Schedules => self.schedules_section(window, cx),
            Section::Run => self.run_section(window, cx),
            Section::Prs => self.prs_section(window, cx),
            Section::Skills => self.skills_section(window, cx),
            Section::Mcp => self.mcp_section(window, cx),
            Section::Danger => self.danger_section(window, cx),
        }
    }

    // ---- General ----

    fn general(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let vw = f32::from(window.viewport_size().width);
        let name = self.field(NAME, window, cx);
        let name_row = Row::new()
            .id("name")
            .child(copy("proj-name", "Name", None, cx))
            .child(
                ctrl().child(
                    div()
                        .w(px(240f32.min(0.7 * vw)))
                        .flex()
                        .debug_selector(|| "inp:Project name".into())
                        .child(inp(&name, &"my-app".into(), InpStyle::BASE, window, cx)),
                ),
            );
        let ident = self.identity(window, cx);
        let identity = settings_group("grp-identity", "Identity")
            .blurb("How this project appears in Recur.")
            .child(rows(IN_GROUP, vec![name_row, ident], window, cx));

        let repo = self.field(REPO, window, cx);
        let picking = self.picking;
        let path_pick = div()
            .flex()
            .items_stretch()
            .gap(px(8.))
            .w_full()
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .debug_selector(|| "inp:Repository path".into())
                    .child(inp(
                        &repo,
                        &"~/code/my-app".into(),
                        InpStyle::BASE,
                        window,
                        cx,
                    )),
            )
            .child(
                tbtn("proj-browse", picking, cx)
                    .h_auto()
                    .debug_selector(|| "proj-browse".into())
                    .aria_label("Browse for repository")
                    .when(!picking, |d| {
                        d.on_click(cx.listener(|v, _, window, cx| v.pick_repo(window, cx)))
                    })
                    .child(tbtn_icon(IconName::Folder))
                    .child(recur_ui::style::button_label(
                        if picking { "Opening…" } else { "Browse" },
                        12.,
                        weight::MEDIUM,
                    )),
            );
        let repository = settings_group("grp-repo", "Repository")
            .blurb("Local git repo. Workspaces run in isolated git worktrees off this checkout.")
            .child(rows(
                IN_GROUP,
                vec![Row::stack().child(path_pick)],
                window,
                cx,
            ));

        // Workspace: default branch, group, hide.
        let opts: Vec<SelectOption> = branch_options(&self.branch_choices, &self.form.base_branch)
            .into_iter()
            .map(|(v, g)| SelectOption::new(v.clone(), v).group(g))
            .collect();
        let shown = if self.form.base_branch.is_empty() {
            opts.first()
                .map(|o| o.value.to_string())
                .unwrap_or_default()
        } else {
            self.form.base_branch.clone()
        };
        self.branch_sel.update(cx, |s, cx| {
            s.set_options(opts, cx);
            if s.value() != &shown {
                s.set_value(shown, cx);
            }
        });
        let mut group_opts = vec![SelectOption::new("", "Ungrouped")];
        group_opts.extend(
            self.vcx
                .state
                .read(cx)
                .project_groups
                .iter()
                .map(|g| SelectOption::new(g.id.clone(), g.name.clone())),
        );
        let gid = self.form.group_id.clone();
        self.group_sel.update(cx, |s, cx| {
            s.set_options(group_opts, cx);
            if s.value() != &gid {
                s.set_value(gid, cx);
            }
        });
        let sel_w = 240f32.min(0.7 * vw);
        let mut ws_rows = vec![
            Row::new()
                .id("branch")
                .child(copy(
                    "proj-branch",
                    "Default branch",
                    Some(vec![span(
                        "Workspaces spin off this base. Prefer a remote tip (e.g. origin/main) when you want a fresh fetch at create.",
                    )]),
                    cx,
                ))
                .child(ctrl().child(div().min_w(px(sel_w)).child(self.branch_sel.clone()))),
            Row::new()
                .id("group")
                .child(copy(
                    "proj-group",
                    "Group",
                    Some(vec![span(
                        "Sidebar section this project appears under. Create groups from the Projects rail.",
                    )]),
                    cx,
                ))
                .child(ctrl().child(div().min_w(px(sel_w)).child(self.group_sel.clone()))),
        ];
        if let Some(sel) = self.selected(cx) {
            let hidden = sel.hidden;
            let id = sel.id.clone();
            ws_rows.push(
                Row::new()
                    .id("hide")
                    .child(copy(
                        "proj-hide",
                        "Hide from sidebar",
                        Some(vec![span(
                            "Leaves this project out of the Projects rail. The eye button in the rail header shows hidden projects.",
                        )]),
                        cx,
                    ))
                    .child(ctrl().child(
                        tog("proj-hide-tog", "toggle hide from sidebar", hidden, false, cx)
                            .on_click(cx.listener(move |v, _, _, cx| {
                                let id = id.clone();
                                v.vcx
                                    .state
                                    .update(cx, |s, cx| s.set_project_hidden(&id, !hidden, cx))
                                    .detach();
                            })),
                    )),
            );
        }
        let workspace = settings_group("grp-ws", "Workspace")
            .blurb("Where new worktrees branch from, and where this project sits in the sidebar.")
            .child(rows(IN_GROUP, ws_rows, window, cx));

        let linear = self.field(LINEAR, window, cx);
        let issues = settings_group("grp-issues", "Issues")
            .blurb("Which assigned issues belong to this project. GitHub issues match this repo's git remotes automatically.")
            .child(rows(
                IN_GROUP,
                vec![Row::new()
                    .id("linear")
                    .child(copy(
                        "proj-linear",
                        "Linear projects",
                        Some(vec![span(
                            "Comma-separated Linear project names or team keys. A project name match wins over a team key.",
                        )]),
                        cx,
                    ))
                    .child(ctrl().child(
                        div()
                            .flex()
                            .debug_selector(|| "inp:Linear projects".into())
                            .child(inp(
                                &linear,
                                &"Mobile App, ENG".into(),
                                // An `<input>` without a width takes WebKit's `size=20` width:
                                // 20 average advances of the 12px mono face plus its
                                // max-minus-average advance (170px), in 12px sides and a border.
                                InpStyle {
                                    width: Some(196.),
                                    ..InpStyle::BASE
                                },
                                window,
                                cx,
                            )),
                    ))],
                window,
                cx,
            ));
        let _ = t;
        settings_groups()
            .child(identity)
            .child(repository)
            .child(workspace)
            .child(issues)
            .into_any_element()
    }

    /// The Avatar row: the picture beside the kind segment, its slot, and the colors.
    fn identity(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Row {
        let t = Theme::of(cx).tokens;
        let pic = self.ident_pic();
        let color = self.ident_color();
        let parsed = parse_project_pic(&pic, "?");
        let kind = match parsed {
            ParsedPic::Letter(_) => "letter",
            ParsedPic::Icon(_) => "icon",
            ParsedPic::Image(_) => "image",
        };
        let label = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .items_start()
            .gap(px(10.))
            .child(recur_ui::set::title("Avatar", cx))
            .child(project_pic(
                pic.clone(),
                color.clone(),
                self.form.name.clone(),
                PicBox {
                    size: 36.,
                    radius: 8.,
                    font_size: 14.,
                },
            ));
        // `.set-ctrl`'s `justify-content: flex-end` packs the grid's one auto column at the
        // end at its max-content width, so the buttons keep their own widths.
        let mut kinds = seg(cx).w_full();
        for (k, l) in [("letter", "Letter"), ("icon", "Icon"), ("image", "Image")] {
            kinds = kinds.child(
                seg_btn(
                    SharedString::from(format!("ident-{k}")),
                    l,
                    kind == k,
                    false,
                    8.,
                    cx,
                )
                .flex_1()
                .flex_shrink_0()
                .justify_center()
                .debug_selector(move || format!("ident-kind:{l}"))
                .on_click(cx.listener(move |v, _, window, cx| v.set_avatar_kind(k, window, cx))),
            );
        }
        let slot: AnyElement = match &parsed {
            ParsedPic::Letter(_) => {
                let letter = self.field(LETTER, window, cx);
                letter.update(cx, |s, cx| {
                    s.set_text_align(gpui_kit::TextAlign::Center, cx)
                });
                div()
                    .flex()
                    .debug_selector(|| "inp:Avatar letter".into())
                    .child(
                        inp(
                            &letter,
                            &"".into(),
                            InpStyle {
                                pad_y: 0.,
                                pad_x: 0.,
                                width: Some(32.),
                                min_h: Some(30.),
                                ..InpStyle::BASE
                            },
                            window,
                            cx,
                        )
                        .h(px(30.))
                        .justify_center(),
                    )
                    .into_any_element()
            }
            ParsedPic::Icon(name) => {
                let name = name.clone();
                self.icon_picker.update(cx, |p, cx| p.set_value(name, cx));
                div()
                    .w_full()
                    .h(px(30.))
                    .child(self.icon_picker.clone())
                    .into_any_element()
            }
            ParsedPic::Image(_) => div()
                .flex()
                .items_center()
                .gap(px(6.))
                .w_full()
                .child(
                    tbtn("ident-browse", false, cx)
                        .h(px(30.))
                        .flex_1()
                        .justify_center()
                        .on_click(cx.listener(|v, _, window, cx| v.pick_image(window, cx)))
                        .child(tbtn_icon(IconName::Photo))
                        .child(recur_ui::style::button_label("Browse", 12., weight::MEDIUM)),
                )
                .child(
                    tbtn_label("ident-clear", "Clear", false, cx)
                        .h(px(30.))
                        .flex_1()
                        .justify_center()
                        .on_click(cx.listener(|v, _, window, cx| {
                            let p = project_pic_from_name(&v.form.name);
                            v.form.pic = if p.is_empty() { "P".into() } else { p };
                            v.changed(window, cx);
                        })),
                )
                .into_any_element(),
        };
        let mut colors = div().flex().items_center().gap(px(6.));
        for c in PROJECT_COLORS {
            let on = color == c;
            let fill = Theme::of(cx).resolve_css_color(c).unwrap_or(t.pane);
            colors = colors.child(
                swatch(SharedString::from(format!("swatch-{c}")), on, &t)
                    .bg(Hsla::from(fill))
                    .debug_selector(move || format!("swatch:{c}{}", if on { ":on" } else { "" }))
                    .on_click(cx.listener(move |v, _, window, cx| {
                        v.form.color = c.to_string();
                        v.changed(window, cx);
                    })),
            );
        }
        let custom_on = !is_project_preset_color(&color);
        let custom_fill = if custom_on {
            recur_ui::Color::parse(&color).unwrap_or(t.pane)
        } else {
            t.pane
        };
        let hex = {
            let h = color_to_hex(&color, &t);
            if h.is_empty() {
                "#4cc6d6".to_string()
            } else {
                h
            }
        };
        let mut custom = swatch("swatch-custom".into(), custom_on, &t)
            .debug_selector(|| "swatch:custom".into())
            .bg(Hsla::from(custom_fill))
            .flex()
            .items_center()
            .justify_center()
            .on_click(cx.listener(move |v, _, window, cx| v.open_color_panel(&hex, window, cx)));
        if custom_on {
            custom = custom.border_color(Hsla::transparent_black());
        } else {
            custom = custom
                .border(px(1.))
                .border_dashed()
                .border_color(Hsla::from(t.line_bright))
                .child(icon(IconName::Plus).size(px(10.)).color(t.muted));
        }
        colors = colors.child(custom);
        let track = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(div().h(px(28.)).flex().items_center().child(kinds))
            .child(div().h(px(30.)).flex().items_center().min_w_0().child(slot))
            .child(colors);
        let control = div()
            .flex_none()
            .w(px(204.))
            .flex()
            .justify_end()
            .child(track);
        Row::new()
            .id("avatar")
            .danger()
            .child(label)
            .child(ctrl().items_start().child(control))
    }

    /// `setAvatarKind`.
    fn set_avatar_kind(&mut self, kind: &str, window: &mut Window, cx: &mut Context<Self>) {
        let parsed = parse_project_pic(&self.ident_pic(), "?");
        match kind {
            "letter" => {
                let p = project_pic_from_name(&self.form.name);
                self.form.pic = if p.is_empty() { "P".into() } else { p };
                let l = self.form.pic.clone();
                if let Some(f) = self.fields.get_mut(LETTER) {
                    f.set(&l, window, cx);
                }
            }
            "icon" => {
                self.form.pic = match parsed {
                    ParsedPic::Icon(_) => self.ident_pic(),
                    _ => recur_ui::widgets::project_pic::DEFAULT_LUCIDE_PIC.into(),
                };
            }
            _ => {
                if matches!(parsed, ParsedPic::Image(_)) {
                    return;
                }
                self.pick_image(window, cx);
                return;
            }
        }
        self.changed(window, cx);
    }

    /// The hidden `<input type=file accept="image/…">`: pick an image and store it as the pic.
    fn pick_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Some(path) = rx
                .await
                .ok()
                .and_then(Result::ok)
                .flatten()
                .and_then(|v| v.into_iter().next())
            else {
                return;
            };
            let bytes = std::fs::read(&path).ok();
            let _ = this.update_in(cx, |v, window, cx| {
                let pic = bytes
                    .ok_or_else(|| "Could not read that image".to_string())
                    .and_then(|b| crate::pic::pic_from_image_bytes(&b));
                match pic {
                    Ok(p) => {
                        v.form.pic = p;
                        v.changed(window, cx);
                    }
                    Err(_) => recur_view_core::toast(
                        recur_ui::widgets::ToastKind::Error,
                        "Could not read that image",
                        cx,
                    ),
                }
            });
        })
        .detach();
    }

    /// `<input type=color>`: the system color panel, its picks applied as they change.
    fn open_color_panel(&mut self, hex: &str, window: &mut Window, cx: &mut Context<Self>) {
        let me = cx.entity().downgrade();
        crate::color_panel::open(
            hex,
            Rc::new(move |next: String, cx: &mut App| {
                let _ = me.update(cx, |v, cx| {
                    let h = parse_hex_color(&next);
                    if !h.is_empty() && v.form.color != h {
                        v.form.color = h;
                        v.touch(cx);
                    }
                });
            }),
            window,
            cx,
        );
    }

    // ---- Schedules ----

    fn schedules_section(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(sel) = self.selected(cx) else {
            return div().into_any_element();
        };
        // `{#key selected.id}`: a fresh list per project.
        if self.schedules.as_ref().is_none_or(|(id, _)| *id != sel.id) {
            let vcx = self.vcx.clone();
            let p = sel.clone();
            let list = cx.new(|cx| ScheduledPrompts::new(&vcx, Some(p), window, cx));
            self.schedules = Some((sel.id.clone(), list));
        }
        self.schedules
            .as_ref()
            .map(|(_, l)| l.clone().into_any_element())
            .unwrap_or_else(|| div().into_any_element())
    }

    // ---- Run ----

    fn run_section(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let mut out = div().flex().flex_col();
        let err = self
            .selected(cx)
            .map(|p| p.project_file_error)
            .unwrap_or_default();
        if !err.is_empty() {
            out = out.child(
                note(true, cx)
                    .mt(px(0.))
                    .mb(px(16.))
                    .child(icon(IconName::Info).size(px(15.)).color(t.coral).mt(px(1.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(recur_ui::set::title(
                                "project.toml could not be parsed — editing run settings will replace it",
                                cx,
                            ))
                            .child(desc("proj-file-err", vec![span(err)], cx)),
                    ),
            );
        }
        let is_new = self.is_new(cx);
        let meta = copy_files_meta(
            &self.copy_preview,
            &self.form.copy_files,
            &self.form.repo_path,
            is_new,
        );
        style_area(&self.copy_files, cx);
        let err_meta = !self.copy_preview.error.is_empty() && !self.copy_preview.loading;
        let loading = self.copy_preview.loading;
        let mut copy_files = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .w_full()
            .pt(px(10.))
            .pb(px(8.))
            .child(
                recur_ui::style::card(t.pane, t.line, 6.)
                    .debug_selector(|| "inp:Files to copy".into())
                    .w_full()
                    .min_h(px(88.))
                    .max_h(px(220.))
                    .py(px(9.))
                    .px(px(12.))
                    .font_family(recur_ui::fonts::MONO)
                    .text_size(px(12.))
                    .line_height(relative(1.45))
                    .child(gpui_kit::base::input::Textarea::new(&self.copy_files)),
            )
            .child(
                div()
                    .debug_selector({
                        let m = meta.clone();
                        move || format!("copy-files-meta={m}")
                    })
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .min_h(px(28.))
                    .child(div().flex_1().min_w_0().child(recur_ui::tooltip::titled(
                        "copy-meta-tip",
                        meta.clone(),
                        {
                            let tx = text(meta.clone())
                                .id("copy-meta")
                                .size(12.)
                                .leading(1.4)
                                .color(if err_meta { t.coral } else { t.muted });
                            if err_meta { tx } else { tx.truncate() }
                        },
                    )))
                    .child(
                        rm_btn("copy-refresh", IconName::Refresh, loading || is_new, cx)
                            .aria_label("Refresh matching files")
                            .when(!(loading || is_new), |d| {
                                d.on_click(cx.listener(|v, _, window, cx| {
                                    v.copy_tick += 1;
                                    v.changed(window, cx);
                                }))
                            }),
                    ),
            );
        if !self.copy_preview.files.is_empty() {
            let mut list = recur_ui::style::card(t.pane, t.line, 6.)
                .debug_selector(|| "copy-files-list".into())
                .max_h(px(180.))
                .py(px(6.))
                .flex()
                .flex_col();
            let mut items = Vec::new();
            for f in &self.copy_preview.files {
                let sel = format!("copy-file:{f}");
                items.push(
                    div()
                        .debug_selector(move || sel.clone())
                        .py(px(3.))
                        .px(px(10.))
                        .child(recur_ui::tooltip::titled(
                            SharedString::from(format!("cf-{f}")),
                            f.clone(),
                            text(f.clone()).mono().size(11.5).color(t.muted).truncate(),
                        ))
                        .into_any_element(),
                );
            }
            if self.copy_preview.truncated {
                let more = self.copy_preview.count - self.copy_preview.files.len() as i64;
                items.push(
                    div()
                        .py(px(3.))
                        .px(px(10.))
                        .child(
                            text(format!("and {more} more"))
                                .mono()
                                .size(11.5)
                                .color(t.faint)
                                .truncate(),
                        )
                        .into_any_element(),
                );
            }
            list = list.child(recur_ui::widgets::scroll_area("copy-files-list").children(items));
            copy_files = copy_files.child(list);
        }

        let mini = |e: &Option<gpui_kit::Entity<crate::mini::MiniScriptEditor>>| {
            e.clone()
                .map(|e| e.into_any_element())
                .unwrap_or_else(|| div().into_any_element())
        };
        let setup = mini(&self.setup);
        let teardown = mini(&self.teardown);
        let env = mini(&self.env);

        let mut run_list = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .w_full()
            .pt(px(10.))
            .pb(px(8.));
        let narrow = recur_ui::set::narrow(window);
        let ids: Vec<(u64, gpui_kit::Entity<crate::mini::MiniScriptEditor>)> = self
            .run_rows
            .iter()
            .map(|r| (r.id, r.editor.clone()))
            .collect();
        for (id, editor) in ids {
            let input = self.field(&run_name_key(id), window, cx);
            run_list = run_list.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .debug_selector(move || format!("inp:Command name:{id}"))
                            .child(inp(
                                &input,
                                &"dev".into(),
                                InpStyle {
                                    pad_y: 7.,
                                    pad_x: 10.,
                                    width: Some(if narrow { 120. } else { 148. }),
                                    ..InpStyle::BASE
                                },
                                window,
                                cx,
                            )),
                    )
                    .child(div().flex_1().min_w_0().child(editor))
                    .child(
                        rm_btn(
                            SharedString::from(format!("run-rm-{id}")),
                            IconName::Trash,
                            false,
                            cx,
                        )
                        .mt(px(3.))
                        .aria_label("Remove command")
                        .on_click(
                            cx.listener(move |v, _, window, cx| v.remove_run_row(id, window, cx)),
                        ),
                    ),
            );
        }
        run_list = run_list.child(
            div().flex().gap(px(8.)).mt(px(4.)).child(
                tbtn("run-add", false, cx)
                    .debug_selector(|| "run-add".into())
                    .on_click(cx.listener(|v, _, window, cx| {
                        v.add_run_row("", "", window, cx);
                        v.changed(window, cx);
                    }))
                    .child(tbtn_icon(IconName::Plus))
                    .child(recur_ui::style::button_label(
                        "Add command",
                        12.,
                        weight::MEDIUM,
                    )),
            ),
        );

        let app_url = self.field(APP_URL, window, cx);
        let app_url_input = div().flex().debug_selector(|| "inp:App URL".into()).child(
            inp(
                &app_url,
                &"http://localhost:$RECUR_PORT".into(),
                InpStyle::BASE,
                window,
                cx,
            )
            .text_color(Hsla::from(t.cyan)),
        );
        let env_desc = Text::from_spans(vec![
            span("Values may use "),
            code("$RECUR_PORT", cx),
            span(", "),
            code("$RECUR_WORKSPACE_ID", cx),
            span(", "),
            code("$RECUR_WORKTREE", cx),
            span(", "),
            code("$RECUR_PROJECT_ID", cx),
            span(", "),
            code("$RECUR_BRANCH", cx),
            span(", "),
            code("$HOME", cx),
            span(", and keys from gitignored "),
            code(".env", cx),
            span(" files (expansion only). "),
            code("project.toml", cx),
            span(" env ignores "),
            code("PATH", cx),
            span(", "),
            code("HOME", cx),
            span(", "),
            code("SHELL", cx),
            span(", "),
            code("LD_*", cx),
            span(", and "),
            code("DYLD_*", cx),
            span("."),
        ])
        .id("env-desc")
        .size(12.)
        .leading(1.4)
        .color(t.faint);

        let groups = settings_groups()
            .child(lead(
                "run-lead",
                vec![
                    span("Run settings are saved to "),
                    code(".recur/project.toml", cx),
                    span("."),
                ],
                cx,
            ))
            .child(
                settings_group("grp-copy", "Files to copy")
                    .blurb("Copied into each new workspace before setup, skipping files that already exist in the worktree. Leave empty to copy nothing. A pattern without a slash matches at any depth. Heavy dirs (node_modules, dist, build, …) are skipped unless the pattern names them.")
                    .child(copy_files),
            )
            .child(
                settings_group("grp-setup", "Setup script")
                    .blurb("Bash script, once per worktree create. Gets $RECUR_PORT, $RECUR_WORKSPACE_ID, $RECUR_WORKTREE, $RECUR_PROJECT_ID, $RECUR_BRANCH, and project env.")
                    .child(rows(IN_GROUP, vec![Row::stack().child(setup)], window, cx)),
            )
            .child(
                settings_group("grp-teardown", "Teardown script")
                    .blurb("Bash script when the worktree is removed (best-effort). Same env as setup.")
                    .child(rows(IN_GROUP, vec![Row::stack().child(teardown)], window, cx)),
            )
            .child(
                settings_group("grp-runs", "Run commands")
                    .blurb("Named scripts for the titlebar Run button. A command named dev is what verify-in-browser boots — without one a loop can't verify and won't open a PR. Pin the default from that dropdown — it stays on this machine.")
                    .child(run_list),
            )
            .child(
                settings_group("grp-url", "App URL")
                    .blurb("Where the running app is reachable. $RECUR_PORT is filled in for each workspace.")
                    .child(rows(IN_GROUP, vec![Row::stack().child(app_url_input)], window, cx)),
            )
            .child(
                settings_group("grp-env", "Environment")
                    .blurb("Dotenv script (KEY=value, # comments) injected into setup, teardown, runs, and terminals. Do not put secrets here. They are written into .recur/project.toml, which is meant to be committed. Keep API keys in gitignored .env files; copy them into worktrees and let the process load them. Recur does not auto-export those keys.")
                    .child(rows(
                        IN_GROUP,
                        vec![Row::stack().child(env).child(div().mt(px(3.)).mb(px(12.)).child(env_desc))],
                        window,
                        cx,
                    )),
            );
        out.child(groups).into_any_element()
    }

    // ---- Pull requests ----

    fn prs_section(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let settings = self.vcx.state.read(cx).settings.clone().unwrap_or_default();
        let short = |m: &str| match m {
            "squash" => "Squash",
            "rebase" => "Rebase",
            _ => "Merge",
        };
        let squash_short = |m: &str| match m {
            "title" => "PR title",
            "title-and-description" => "PR title and description",
            _ => "PR title and commits",
        };
        let merge_short = |m: &str| match m {
            "title-and-description" => "PR title and description",
            _ => "PR title",
        };
        let global_merge = short(settings.default_merge_method.as_str());
        let global_update = short(settings.default_update_base_method.as_str());
        let global_squash = squash_short(settings.default_squash_commit_message.as_str());
        let global_msg = merge_short(settings.default_merge_commit_message.as_str());

        let f = self.form.clone();
        let gh = f.pr_on_github;
        let del = f.delete_branch_on_remove;
        type Get = fn(&recur_domain::Project) -> String;
        type Set = fn(&mut recur_domain::Project, &str);
        /// Title, the global default's label, description, options, getter and setter.
        type Group = (
            &'static str,
            String,
            &'static str,
            Vec<(&'static str, &'static str)>,
            Get,
            Set,
        );
        let groups: [Group; 4] = [
            (
                "Merge method",
                format!("Use global default ({global_merge})"),
                "Override the global default for this project. Applies when merging PRs from any loop.",
                vec![
                    ("merge", "Create a merge commit"),
                    ("squash", "Squash and merge"),
                    ("rebase", "Rebase and merge"),
                ],
                |p| p.merge_method.as_str().to_string(),
                |p, v| p.merge_method = v.into(),
            ),
            (
                "Update with base",
                format!("Use global default ({global_update})"),
                "How a PR branch catches up when you click Update with base.",
                vec![
                    ("merge", "Merge base into branch"),
                    ("rebase", "Rebase onto base"),
                ],
                |p| p.update_base_method.as_str().to_string(),
                |p, v| p.update_base_method = v.into(),
            ),
            (
                "Squash commit message",
                format!("Use global default ({global_squash})"),
                "Presented when squash-merging a pull request.",
                vec![
                    ("title", "Pull request title"),
                    ("title-and-commits", "Pull request title and commit details"),
                    (
                        "title-and-description",
                        "Pull request title and description",
                    ),
                ],
                |p| p.squash_commit_message.as_str().to_string(),
                |p, v| p.squash_commit_message = v.into(),
            ),
            (
                "Merge commit message",
                format!("Use global default ({global_msg})"),
                "Presented when merging a pull request with a merge commit.",
                vec![
                    ("title", "Pull request title"),
                    (
                        "title-and-description",
                        "Pull request title and description",
                    ),
                ],
                |p| p.merge_commit_message.as_str().to_string(),
                |p, v| p.merge_commit_message = v.into(),
            ),
        ];
        let mut labels: Vec<String> = Vec::new();
        for g in &groups {
            labels.push(g.1.clone());
            labels.extend(g.3.iter().map(|o| o.1.to_string()));
        }
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let w = radios_width(&label_refs, window);
        let mut list: Vec<(AnyElement, AnyElement, bool)> = vec![
            (
                copy(
                    "pr-gh",
                    "Open PRs on GitHub",
                    Some(vec![span("Push the loop branch and open a real GitHub PR (needs the gh CLI signed in and an origin remote). Off = Local PRs reviewed in-app. Falls back to Local if GitHub isn't reachable.")]),
                    cx,
                )
                .into_any_element(),
                tog("pr-gh-tog", "toggle GitHub PRs", gh, false, cx)
                    .on_click(cx.listener(move |v, _, window, cx| {
                        v.form.pr_on_github = !gh;
                        v.changed(window, cx);
                    }))
                    .into_any_element(),
                false,
            ),
            (
                copy(
                    "pr-del",
                    "Delete branch when removing a workspace",
                    Some(vec![span("Deletes the workspace’s local git branch after the worktree is removed. Remote branches are left alone. Off by default.")]),
                    cx,
                )
                .into_any_element(),
                tog("pr-del-tog", "toggle delete branch on remove", del, false, cx)
                    .on_click(cx.listener(move |v, _, window, cx| {
                        v.form.delete_branch_on_remove = !del;
                        v.changed(window, cx);
                    }))
                    .into_any_element(),
                false,
            ),
        ];
        for (gi, (title, global, d, opts, get, set)) in groups.into_iter().enumerate() {
            let cur = get(&f);
            let mut radios = div().flex().flex_col().gap(px(2.)).w_full().mt(px(-6.));
            let mut all: Vec<(&str, String)> = vec![("", global)];
            all.extend(opts.into_iter().map(|(v, l)| (v, l.to_string())));
            for (val, label) in all {
                let on = cur == val;
                let l = label.clone();
                radios = radios.child(
                    mm_radio(SharedString::from(format!("pr-{gi}-{val}")), label, on, cx)
                        .debug_selector(move || format!("radio:{title}:{l}:{on}"))
                        .on_click(cx.listener(move |v, _, window, cx| {
                            set(&mut v.form, val);
                            v.changed(window, cx);
                        })),
                );
            }
            list.push((
                copy(&format!("pr-{gi}"), title, Some(vec![span(d)]), cx).into_any_element(),
                radios.into_any_element(),
                true,
            ));
        }
        radio_rows(list, w, window, cx).into_any_element()
    }

    // ---- Skills ----

    fn skills_section(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let global_off = self
            .vcx
            .state
            .read(cx)
            .settings
            .as_ref()
            .map(|s| s.disabled_skills.clone())
            .unwrap_or_default();
        let mut out = settings_groups().child(self.settings_link_lead(
            "skills-lead",
            "Repository skills for this project. Recur and personal skills follow ",
            "Settings → Skills",
            " unless you override them below.",
            "skills",
            cx,
        ));
        let msg = |s: &str, window: &Window, cx: &App| {
            ctrl_text("proj-skills-msg", vec![span(s.to_string())], window, cx)
                .max_w(relative(1.))
                .justify_start()
        };
        if self.skills_loading {
            return out
                .child(msg("Loading skills…", window, cx))
                .into_any_element();
        }
        if !self.skills_err.is_empty() {
            let e = self.skills_err.clone();
            return out.child(msg(&e, window, cx)).into_any_element();
        }
        let mut by: [Vec<SkillRow>; 3] = Default::default();
        for s in &self.skills {
            let i = match skill_kind(s) {
                SkillKind::Recur => 0,
                SkillKind::Project => 1,
                SkillKind::User => 2,
            };
            by[i].push(s.clone());
        }
        for list in by.iter_mut() {
            list.sort_by(|a, b| recur_editor::text::locale_compare(&a.name, &b.name));
        }
        let saving = self.saving_skills;
        let (pd, pe) = (
            self.form.disabled_skills.clone(),
            self.form.enabled_skills.clone(),
        );
        let row = |v: &mut Self, sk: &SkillRow, show_loc: bool, cx: &mut Context<Self>| {
            let on = effective_skill_enabled(&sk.name, &global_off, &pd, &pe);
            let hint = skill_override_hint(&sk.name, &global_off, &pd, &pe);
            let loc = if show_loc {
                skill_location(&sk.path, &sk.scope, "")
            } else {
                String::new()
            };
            let id = format!("pskill-{}{}", sk.name, sk.path);
            let mut lines = Vec::new();
            if !sk.description.is_empty() {
                lines.push(sk.description.clone());
            }
            if !hint.is_empty() {
                lines.push(hint.to_string());
            }
            let copy = skill_copy(
                &id,
                format!("/{}", sk.name),
                lines,
                (!loc.is_empty()).then(|| (loc, sk.path.clone())),
                cx,
            );
            let name = sk.name.clone();
            let mut tg = tog(
                SharedString::from(format!("tog-{id}")),
                format!("{} {}", if on { "Disable" } else { "Enable" }, sk.name),
                on,
                saving,
                cx,
            )
            .mt(px(2.));
            if !saving {
                let _ = v;
                tg = tg
                    .on_click(cx.listener(move |v, _, _, cx| v.set_project_skill(&name, !on, cx)));
            }
            (copy, vec![tg.into_any_element()])
        };
        if !by[1].is_empty() {
            let list: Vec<SkillRow> = by[1].clone();
            let rows_: Vec<_> = list.iter().map(|sk| row(self, sk, true, cx)).collect();
            out = out.child(
                settings_group("grp-pskills-project", "In this project")
                    .blurb("Skills in the repository.")
                    .child(skill_list(rows_, cx)),
            );
        } else {
            out = out.child(msg("No skills found in this repository.", window, cx));
        }
        for (i, kind, title, label) in [
            (0usize, SkillKind::Recur, "Recur", "Override Recur skills"),
            (
                2usize,
                SkillKind::User,
                "Your skills",
                "Override your skills",
            ),
        ] {
            if by[i].is_empty() {
                continue;
            }
            let over = if kind == SkillKind::Recur {
                self.override_recur
            } else {
                self.override_user
            };
            let count = skill_kind_override_count(&by[i], &pd, &pe);
            let blurb = if over {
                format!(
                    "Customize for this project. Turn off to use Settings defaults{}.",
                    if count > 0 {
                        format!(
                            " and clear {count} override{}",
                            if count == 1 { "" } else { "s" }
                        )
                    } else {
                        String::new()
                    }
                )
            } else {
                "Using Settings → Skills. Turn on to override for this project.".to_string()
            };
            let list: Vec<SkillRow> = by[i].clone();
            let rows_: Vec<_> = list
                .iter()
                .map(|sk| row(self, sk, kind == SkillKind::User, cx))
                .collect();
            let me = cx.entity().downgrade();
            out = out.child(
                settings_group(SharedString::from(format!("grp-pskills-{i}")), title)
                    .blurb(blurb)
                    .override_toggle(over, saving, Some(label.into()), move |on, _, cx| {
                        let _ = me.update(cx, |v, cx| v.set_kind_override(kind, on, cx));
                    })
                    .child(skill_list(rows_, cx)),
            );
        }
        let _ = t;
        out.into_any_element()
    }

    /// A `.set-lead` with a `Settings → …` link in it.
    fn settings_link_lead(
        &self,
        id: &'static str,
        before: &str,
        link: &str,
        after: &str,
        section: &'static str,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = Theme::of(cx).tokens;
        let state = self.vcx.state.clone();
        div()
            .pt(px(2.))
            .pb(px(4.))
            .debug_selector(move || format!("lead-link:{section}"))
            .child(
                Text::from_spans(vec![
                    span(before.to_string()),
                    // `.lnk` with `font: inherit` (the lead's 12px UI face) in `--amber`.
                    span(link.to_string()).color(t.amber),
                    span(after.to_string()),
                ])
                .id(id)
                .size(12.)
                .leading(1.4)
                .color(t.faint)
                .on_click_span(1, move |_, cx| {
                    state.update(cx, |s, cx| s.open_settings(Some(section), cx))
                }),
            )
    }

    fn set_project_skill(&mut self, name: &str, enabled: bool, cx: &mut Context<Self>) {
        if self.selected(cx).is_none() {
            return;
        }
        let global = self
            .vcx
            .state
            .read(cx)
            .settings
            .as_ref()
            .map(|s| s.disabled_skills.clone())
            .unwrap_or_default();
        let (d, e) = project_skill_override_lists(
            name,
            enabled,
            &global,
            &self.form.disabled_skills,
            &self.form.enabled_skills,
        );
        self.persist_overrides(false, d, e, None, cx);
    }

    /// `setKindOverride`: turning an override off restores Settings' defaults for that kind.
    fn set_kind_override(&mut self, kind: SkillKind, on: bool, cx: &mut Context<Self>) {
        match kind {
            SkillKind::Recur => self.override_recur = on,
            _ => self.override_user = on,
        }
        cx.notify();
        if on || self.selected(cx).is_none() {
            return;
        }
        let names: std::collections::HashSet<String> = self
            .skills
            .iter()
            .filter(|s| skill_kind(s) == kind)
            .map(|s| s.name.clone())
            .collect();
        if names.is_empty() {
            return;
        }
        let d: Vec<String> = self
            .form
            .disabled_skills
            .iter()
            .filter(|n| !names.contains(*n))
            .cloned()
            .collect();
        let e: Vec<String> = self
            .form
            .enabled_skills
            .iter()
            .filter(|n| !names.contains(*n))
            .cloned()
            .collect();
        if d.len() == self.form.disabled_skills.len() && e.len() == self.form.enabled_skills.len() {
            return;
        }
        // A failed save keeps the overrides, so the panel stays open.
        let reopen: super::OnFail = Rc::new(move |v: &mut ProjectView| match kind {
            SkillKind::Recur => v.override_recur = true,
            _ => v.override_user = true,
        });
        self.persist_overrides(false, d, e, Some(reopen), cx);
    }

    // ---- MCP ----

    fn mcp_section(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let servers = self
            .vcx
            .state
            .read(cx)
            .settings
            .as_ref()
            .map(|s| s.mcp_servers.clone())
            .unwrap_or_default();
        let global_off = mcp_global_disabled(&servers);
        let mut rows_list: Vec<(String, String, String)> = vec![(
            MCP_ID_LINEAR.to_string(),
            "Linear".into(),
            "Uses the API key in Settings → Integrations".into(),
        )];
        for s in custom_mcp_servers(&servers) {
            let id = s.id.trim().to_string();
            if id.is_empty() {
                continue;
            }
            let detail = if s.transport == "stdio" {
                if s.command.is_empty() {
                    "stdio".into()
                } else {
                    s.command.clone()
                }
            } else if s.url.is_empty() {
                "http".into()
            } else {
                s.url.clone()
            };
            rows_list.push((
                id.clone(),
                if s.name.is_empty() {
                    id
                } else {
                    s.name.clone()
                },
                detail,
            ));
        }
        let is_new = self.is_new(cx);
        let saving = self.saving_skills;
        let (pd, pe) = (
            self.form.disabled_mcp.clone(),
            self.form.enabled_mcp.clone(),
        );
        let mut list = Vec::new();
        for (id, name, detail) in rows_list {
            let on = effective_skill_enabled(&id, &global_off, &pd, &pe);
            let hint = skill_override_hint(&id, &global_off, &pd, &pe);
            let mut lines = vec![detail];
            if !hint.is_empty() {
                lines.push(hint.to_string());
            }
            let copy = skill_copy(&format!("pmcp-{id}"), name.clone(), lines, None, cx);
            let disabled = saving || is_new;
            let go = global_off.clone();
            let mut tg = tog(
                SharedString::from(format!("tog-pmcp-{id}")),
                format!("{} {name}", if on { "Disable" } else { "Enable" }),
                on,
                disabled,
                cx,
            )
            .mt(px(2.));
            if !disabled {
                tg = tg.on_click(cx.listener(move |v, _, _, cx| {
                    let (d, e) = project_skill_override_lists(
                        &id,
                        !on,
                        &go,
                        &v.form.disabled_mcp,
                        &v.form.enabled_mcp,
                    );
                    v.persist_overrides(true, d, e, None, cx);
                }));
            }
            list.push((copy, vec![tg.into_any_element()]));
        }
        settings_groups()
            .child(self.settings_link_lead(
                "mcp-lead",
                "MCP tools for this project follow ",
                "Settings → MCP",
                " unless you override them below.",
                "mcp",
                cx,
            ))
            .child(settings_group("grp-pmcp", "Servers").child(skill_list(list, cx)))
            .into_any_element()
    }

    // ---- Danger zone ----

    fn danger_section(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.is_new(cx) {
            return div().into_any_element();
        }
        let t = Theme::of(cx).tokens;
        let deleting = self.deleting;
        let control: AnyElement = if !self.confirming_delete {
            danger_btn("proj-delete", false, false, cx)
                .debug_selector(|| "proj-delete".into())
                .on_click(cx.listener(|v, _, _, cx| {
                    v.confirming_delete = true;
                    cx.notify();
                }))
                .child(tbtn_icon(IconName::Trash))
                .child(recur_ui::style::button_label(
                    "Delete this project",
                    12.,
                    weight::MEDIUM,
                ))
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .items_end()
                .gap(px(8.))
                .child(
                    text(format!(
                        "Delete “{}” and everything under it?",
                        self.form.name
                    ))
                    .size(12.)
                    .weight(weight::MEDIUM)
                    .color(t.coral),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(
                            danger_btn("proj-delete-go", true, deleting, cx)
                                .debug_selector(|| "proj-delete-go".into())
                                .when(!deleting, |d| {
                                    d.on_click(cx.listener(|v, _, _, cx| v.delete_project(cx)))
                                })
                                .child(recur_ui::style::button_label(
                                    if deleting {
                                        "Deleting…"
                                    } else {
                                        "Delete project"
                                    },
                                    12.,
                                    weight::MEDIUM,
                                )),
                        )
                        .child(
                            tbtn_label("proj-delete-cancel", "Cancel", deleting, cx).when(
                                !deleting,
                                |d| {
                                    d.on_click(cx.listener(|v, _, _, cx| {
                                        v.confirming_delete = false;
                                        cx.notify();
                                    }))
                                },
                            ),
                        ),
                )
                .into_any_element()
        };
        rows(
            RowsCtx {
                sides: false,
                in_group: false,
                more: false,
            },
            vec![Row::new()
                .danger()
                .id("danger")
                .child(copy(
                    "proj-danger",
                    "Delete this project",
                    Some(vec![span("Removes the project and all its workspaces, PRs and history. Your policy & settings are kept. This can't be undone.")]),
                    cx,
                ))
                .child(ctrl().child(control))],
            window,
            cx,
        )
        .into_any_element()
    }
}

/// `.proj-ident-swatch`: an 18px circle in a 2px transparent border, ringed amber when `on`.
fn swatch(id: SharedString, on: bool, t: &Tokens) -> gpui_kit::Stateful<Div> {
    div()
        .id(id)
        .relative()
        .flex_none()
        .size(px(18.))
        .rounded_full()
        .border(px(2.))
        .border_color(Hsla::transparent_black())
        .cursor_pointer()
        .when(on, |d| {
            d.child(recur_ui::shadow::box_shadow(
                &[recur_ui::shadow::css_box_shadow(0., 0., 0., 2., t.amber)],
                9.,
                2.,
            ))
        })
}
