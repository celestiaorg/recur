//! The main column (`.obs-main`): the crumb bar with the view chip and editor actions, the
//! editor panes (code, preview, diff, image / PDF / video panes, the split), the branch review
//! host, the status bars, and the terminal dock below or beside them.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, Context, InteractiveElement, IntoElement, MouseButton, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};
use recur_editor::view_state::{EditorViewMode, is_binary_image_path};
use recur_ui::guard::Guarded;
use recur_ui::kbd::kbd;
use recur_ui::shadow::{css_box_shadow, with_shadow};
use recur_ui::style::cached_view;
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::tooltip::titled;
use recur_ui::{IconName, Theme, icon};
use recur_view_core::prefs::{CodePanelKey, PanelKey};
use recur_view_review::style::hsla;

use crate::model::{FocusZone, MainSurface, Pane, format_bytes};
use crate::obs::{Observatory, TermMaxPhase};
use crate::style::{
    CHROME_H, foot_sep, foot_text, icon_btn, link_text, linkish, obs_empty, seg, seg_text_btn,
    statusbar,
};

impl Observatory {
    /// The status bar's language for the focused pane's file.
    pub(crate) fn status_language(&self, cx: &App) -> String {
        let label = crate::textmate::editor_env_peek(cx)
            .map(|e| e.read(cx).language_label(self.active_path()))
            .unwrap_or_default();
        if label.is_empty() {
            "Plain Text".into()
        } else {
            label
        }
    }

    /// `.obs-crumbbar`: the path crumbs (or the two pane names in a split) and the actions.
    fn render_crumbbar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.selected_path.is_empty() && (!self.split_open || self.split_path.is_empty()) {
            return None;
        }
        if self.main_surface(cx) != MainSurface::Editor {
            return None;
        }
        let t = Theme::of(cx).tokens;
        let split = self.show_split_layout();
        let mut bar = div()
            .debug_selector(|| "obs-crumbbar".into())
            .relative()
            .flex_none()
            .h(px(CHROME_H))
            .border_b_1()
            .border_color(hsla(t.line))
            .bg(hsla(t.pane))
            .text_color(hsla(t.muted))
            .min_w_0();
        if split {
            let pct = self.split_pct as f32 / 100.;
            let pane_file = |pane: Pane, last: bool, o: &Observatory, cx: &mut Context<Self>| {
                let (path, dirty) = match pane {
                    Pane::Main => (o.selected_path.clone(), o.dirty()),
                    Pane::Split => (o.split_path.clone(), o.split_dirty()),
                };
                let on = o.focused_pane == pane;
                let lit = on && o.focus_zone == FocusZone::Editor;
                let name = if path.is_empty() {
                    "Empty".to_string()
                } else {
                    path.rsplit('/').next().unwrap_or(&path).to_string()
                };
                let title = if path.is_empty() {
                    "Empty".to_string()
                } else {
                    path.clone()
                };
                let mut b = div()
                    .id(SharedString::from(format!("pane-file-{}", pane.key())))
                    .debug_selector(move || format!("pane-file:{}", pane.key()))
                    .relative()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .pl(px(12.))
                    .pr(px(if last { 200. } else { 12. }))
                    .cursor_pointer()
                    .text_color(hsla(if on { t.text } else { t.faint }))
                    .when(!on, |d| {
                        d.hover(move |s| s.text_color(hsla(t.muted)).bg(hsla(t.pane_2)))
                    })
                    .when(!last, |d| d.border_r_1().border_color(hsla(t.line)))
                    .child(
                        div()
                            .min_w_0()
                            .child(text(name).mono().size(11.).truncate()),
                    )
                    .on_click(
                        cx.guarded_listener(move |o, _, window, cx| o.focus_pane(pane, window, cx)),
                    );
                if dirty {
                    b = b.child(
                        div()
                            .size(px(6.))
                            .rounded(px(3.))
                            .flex_none()
                            .bg(hsla(t.amber)),
                    );
                }
                if on {
                    b = b.child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .h(px(2.))
                            .bg(hsla(if lit { t.amber } else { t.line })),
                    );
                }
                titled(
                    SharedString::from(format!("pane-file-t-{}", pane.key())),
                    title,
                    b,
                )
            };
            let main = pane_file(Pane::Main, false, self, cx);
            let side = pane_file(Pane::Split, true, self, cx);
            bar = bar
                .flex()
                .items_stretch()
                .child(
                    div()
                        .flex_none()
                        .w(gpui_kit::relative(pct))
                        .flex()
                        .child(main),
                )
                .child(div().flex_1().min_w_0().flex().child(side));
            let actions = self.render_crumb_actions(window, cx);
            bar = bar.child(
                div()
                    .absolute()
                    .right(px(8.))
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .child(actions),
            );
            return Some(bar.into_any_element());
        }
        let active = self.active_path().to_string();
        let parts: Vec<String> = active
            .split('/')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        let mut row = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .pl(px(12.))
            .pr(px(8.))
            .h_full();
        let n = parts.len();
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                row = row.child(text("/").mono().size(11.).color(t.faint).nowrap());
            }
            let is_file = i + 1 == n;
            let prefix = parts[..=i].join("/");
            let crumb = div()
                .id(SharedString::from(format!("crumb-{i}")))
                .debug_selector(move || format!("crumb:{i}"))
                // `overflow: hidden`: no automatic minimum, so a narrow bar shrinks the path.
                .min_w_0()
                .overflow_hidden()
                .max_w(px(140.))
                .px(px(4.))
                .py(px(2.))
                .rounded(px(4.))
                .text_color(hsla(if is_file { t.text } else { t.muted }))
                .when(!is_file, |d| {
                    d.cursor_pointer()
                        .hover(move |s| s.text_color(hsla(t.text)).bg(hsla(t.pane_2)))
                })
                .child(text(part.clone()).mono().size(11.).truncate())
                .when(!is_file, |d| {
                    let p = prefix.clone();
                    d.on_click(cx.guarded_listener(move |o, _, window, cx| {
                        o.reveal_in_tree(&format!("{p}/_"), cx).detach();
                        o.select_code_panel(CodePanelKey::Files, window, cx);
                    }))
                });
            row = row.child(titled(
                SharedString::from(format!("crumb-t-{i}")),
                prefix,
                crumb,
            ));
        }
        row = row.child(div().flex_1());
        let actions = self.render_crumb_actions(window, cx);
        row = row.child(div().ml_auto().pl(px(6.)).flex_none().child(actions));
        Some(bar.child(row).into_any_element())
    }

    /// `.obs-crumb-actions`: the view chip and the find, split, reload, review, full screen
    /// buttons.
    fn render_crumb_actions(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let base = self.review_base(cx);
        let chip_title = if base.is_empty() {
            "Cycle view (⌘⇧V) · Diff vs HEAD".to_string()
        } else {
            format!("Cycle view (⌘⇧V) · Diff vs {base} (merge-base)")
        };
        let diff_title = if base.is_empty() {
            "Diff vs HEAD".to_string()
        } else {
            format!("Diff vs {base} (merge-base)")
        };
        let modes = self.available_view_modes();
        let mut actions = div().flex().items_center().gap(px(2.)).flex_none();
        let chip_btn = |id: &'static str,
                        label: &'static str,
                        on: bool,
                        disabled: bool,
                        mode: EditorViewMode,
                        cx: &mut Context<Self>| {
            seg_text_btn(id, on, disabled.then_some(t.pane), cx)
                .debug_selector(move || format!("view-mode:{label}"))
                .h_full()
                .px(px(10.))
                .child(
                    text(label)
                        .size(11.)
                        .weight(500.)
                        .line_height(LineHeight::Normal)
                        .nowrap(),
                )
                .when(!disabled, |b| {
                    b.on_click(cx.guarded_listener(move |o, _, window, cx| {
                        o.set_view_mode(mode, window, cx).detach();
                    }))
                })
        };
        let preview_kind = self.preview_kind();
        if is_binary_image_path(&self.selected_path) && modes.contains(&EditorViewMode::Diff) {
            let mut s = seg(cx).h(px(24.)).items_stretch().mr(px(6.));
            if modes.contains(&EditorViewMode::Preview) {
                s = s.child(chip_btn(
                    "vm-preview",
                    "Preview",
                    self.view_mode != EditorViewMode::Diff,
                    false,
                    EditorViewMode::Preview,
                    cx,
                ));
            }
            s = s.child(titled(
                "vm-diff-t",
                diff_title.clone(),
                chip_btn(
                    "vm-diff",
                    "Diff",
                    self.view_mode == EditorViewMode::Diff,
                    false,
                    EditorViewMode::Diff,
                    cx,
                ),
            ));
            actions = actions.child(titled(
                "vm-seg-t",
                chip_title.clone(),
                div().id("vm-seg").child(s),
            ));
        } else if self.can_show_diff() || preview_kind.is_some() {
            let deleted = self.selected_git_status() == "d";
            let mut s = seg(cx)
                .h(px(24.))
                .items_stretch()
                .mr(px(6.))
                .child(chip_btn(
                    "vm-code",
                    "Code",
                    self.view_mode == EditorViewMode::Code,
                    self.file.is_none() || deleted,
                    EditorViewMode::Code,
                    cx,
                ));
            if self.can_show_diff() {
                s = s.child(titled(
                    "vm-diff-t",
                    diff_title,
                    chip_btn(
                        "vm-diff",
                        "Diff",
                        self.view_mode == EditorViewMode::Diff,
                        false,
                        EditorViewMode::Diff,
                        cx,
                    ),
                ));
            }
            if preview_kind.is_some() {
                s = s.child(chip_btn(
                    "vm-preview",
                    "Preview",
                    self.view_mode == EditorViewMode::Preview,
                    self.file.is_none() || deleted,
                    EditorViewMode::Preview,
                    cx,
                ));
            }
            actions = actions.child(titled("vm-seg-t", chip_title, div().id("vm-seg").child(s)));
        }
        if self.is_git(cx) && self.view_mode == EditorViewMode::Diff {
            let base = if base.is_empty() {
                "HEAD".to_string()
            } else {
                base
            };
            actions = actions.child(titled(
                "all-diffs-t",
                format!("All diffs — review changes since {base} (⌥⌘R)"),
                icon_btn("all-diffs", IconName::Rows3, false, None, cx).on_click(
                    cx.guarded_listener(|o, _, _, cx| {
                        o.set_review_tab(recur_view_review::review_diff::ReviewTab::Changes, cx);
                        o.toggle_review(cx);
                    }),
                ),
            ));
        }
        let can_find = self.can_toggle_find();
        actions = actions
            .child(titled(
                "find-t",
                "Find (⌘F)",
                icon_btn(
                    "find",
                    IconName::Search,
                    self.find_panel_open,
                    (!can_find).then_some(t.pane),
                    cx,
                )
                .debug_selector(|| "crumb:find".into())
                .when(can_find, |b| {
                    b.on_click(
                        cx.guarded_listener(|o, _, window, cx| o.toggle_find_panel(window, cx)),
                    )
                }),
            ))
            .child(titled(
                "split-t",
                "Split editor (⌘\\) — click a pane to focus; opens go there",
                {
                    let on = self.split_open;
                    let b = icon_btn("split", IconName::Columns, false, None, cx)
                        .debug_selector(|| "crumb:split".into());
                    let b = if on { b.text_color(hsla(t.text)) } else { b };
                    b.on_click(cx.guarded_listener(|o, _, window, cx| o.toggle_split(window, cx)))
                },
            ))
            .child(titled(
                "reload-t",
                "Reload from disk (F5)",
                icon_btn("reload", IconName::Refresh, false, None, cx)
                    .debug_selector(|| "crumb:reload".into())
                    .on_click(
                        cx.guarded_listener(|o, _, window, cx| o.reload_from_disk(window, cx)),
                    ),
            ));
        if preview_kind.is_some()
            || !self.selected_artifact_id().is_empty()
            || self.show_image_pane()
            || self.show_pdf_pane()
            || self.show_video_pane()
        {
            actions = actions.child(titled(
                "fullscreen-t",
                "Full screen",
                icon_btn("fullscreen", IconName::Maximize, false, None, cx)
                    .debug_selector(|| "crumb:fullscreen".into())
                    .on_click(cx.guarded_listener(|o, _, window, cx| {
                        if o.preview_kind().is_some() {
                            o.view_mode = EditorViewMode::Preview;
                            o.sync_editors(window, cx);
                        }
                        o.set_editor_fullscreen(true, window, cx);
                    })),
            ));
        }
        actions.into_any_element()
    }

    /// `.obs-image-meta`: path, mime and size (and the zoom button).
    fn image_meta(&self, zoom: Option<AnyElement>, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let path = self
            .file
            .as_ref()
            .map_or_else(|| self.selected_path.clone(), |f| f.path.clone());
        let mut d = div()
            .flex_none()
            .flex()
            .flex_wrap()
            // `align-items: baseline` on same-size text: the texts sit at the top, the zoom
            // button centers itself.
            .items_start()
            .justify_center()
            .gap_x(px(14.))
            .gap_y(px(8.))
            .child(text(path).mono().size(11.).color(t.text).nowrap());
        if let Some(f) = &self.file {
            d = d.child(
                text(format!("{} · {}", f.mime, format_bytes(f.size)))
                    .mono()
                    .size(11.)
                    .color(t.faint)
                    .nowrap(),
            );
        }
        d.children(zoom.map(|z| div().self_center().child(z)))
            .into_any_element()
    }

    /// The raster at its `object-fit: contain` size. `compare` puts it in a figure cell that
    /// it fills; alone it is capped by the pane's content box (`max-width/height: 100%`).
    fn raster(
        &self,
        base: bool,
        compare: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let r = if base {
            &self.image.base
        } else {
            &self.image.current
        };
        let Some(d) = r.image.clone() else {
            return div().into_any_element();
        };
        let (src, name) = if base {
            (
                self.image_base_src(),
                format!("{} (previous)", self.selected_path),
            )
        } else {
            (
                self.image_src(),
                self.file
                    .as_ref()
                    .map(|f| f.path.clone())
                    .unwrap_or_default(),
            )
        };
        let cell = if base {
            &self.bounds.image_base
        } else {
            &self.bounds.image_cur
        };
        let avail = cell.get().size;
        let pad = if compare { 0. } else { 48. };
        let ((w, h), _) = recur_content::lightbox::fit(
            (d.width, d.height),
            (f32::from(avail.width) - pad, f32::from(avail.height) - pad),
        );
        let pic = div()
            .id(SharedString::from(format!(
                "obs-img-{}",
                if base { "base" } else { "cur" }
            )))
            .debug_selector(move || format!("obs-image:{}", if base { "base" } else { "current" }))
            .cursor_pointer()
            .w(px(w))
            .h(px(h))
            .child({
                let scale = window.scale_factor();
                let image = r
                    .at((w * scale).round() as u32, (h * scale).round() as u32)
                    .unwrap_or_else(|| d.image.clone());
                gpui_kit::canvas(
                    |_, _, _| {},
                    move |b, _, window, _| {
                        let _ =
                            window.paint_image(b, b, Default::default(), image.clone(), 0, false);
                    },
                )
                .size_full()
            })
            .on_click(cx.guarded_listener(move |o, _, window, cx| {
                o.zoom_image(src.clone(), name.clone(), base, window, cx)
            }));
        if !compare {
            return with_shadow(pic, &[css_box_shadow(0., 0., 0., 1., t.line)], 0.)
                .flex_none()
                .into_any_element();
        }
        div()
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(crate::geometry::probe(cell))
            .child(with_shadow(
                pic,
                &[css_box_shadow(0., 0., 0., 1., t.line)],
                0.,
            ))
            .into_any_element()
    }

    /// `.obs-image`: the checkerboard pane with the raster (or the base / current pair).
    fn render_image_pane(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let compare = self.image_compare();
        let deleted = self.selected_git_status() == "d";
        let mut pane = crate::render::checkerboard(cx)
            .debug_selector(|| "obs-image".into())
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(24.))
            .overflow_hidden();
        pane = if compare {
            pane.items_stretch()
        } else {
            pane.items_center().justify_center()
        };
        if compare {
            let figure = |label: &'static str, el: AnyElement| {
                div()
                    .min_w_0()
                    .min_h_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(el),
                    )
                    .child(text(label).size(11.).color(t.muted).nowrap())
            };
            let mut pair = div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .w_full()
                .flex()
                .gap(px(28.));
            pair = pair.child(figure("Previous", self.raster(true, true, window, cx)));
            if self.file.is_some() && !deleted {
                pair = pair.child(figure("Current", self.raster(false, true, window, cx)));
            }
            pane = pane.child(pair);
        } else if self.file.is_some() && !deleted {
            pane = pane
                .child(crate::geometry::probe(&self.bounds.image_cur))
                .child(self.raster(false, false, window, cx));
        } else {
            pane = pane.child(obs_empty("This image was deleted.", true, cx));
        }
        let zoom = (self.file.is_some() && !deleted).then(|| {
            let (src, name) = (
                self.image_src(),
                self.file
                    .as_ref()
                    .map(|f| f.path.clone())
                    .unwrap_or_default(),
            );
            titled(
                "img-zoom-t",
                "Zoom",
                icon_btn("img-zoom", IconName::ZoomIn, false, None, cx).on_click(
                    cx.guarded_listener(move |o, _, window, cx| {
                        o.zoom_image(src.clone(), name.clone(), false, window, cx)
                    }),
                ),
            )
            .into_any_element()
        });
        pane.child(self.image_meta(zoom, cx)).into_any_element()
    }

    /// The main editor pane's contents (`.obs-editor-pane` main).
    fn render_main_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let mut pane = div()
            .id("obs-pane-main")
            .debug_selector(|| "obs-editor-pane:main".into())
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .when(self.split_open, |d| {
                d.capture_any_mouse_down(cx.guarded_listener(
                    |o, e: &gpui_kit::MouseDownEvent, window, cx| {
                        if e.button == MouseButton::Left && o.focused_pane != Pane::Main {
                            o.focus_pane(Pane::Main, window, cx)
                        }
                    },
                ))
            });
        if self.show_image_pane() {
            return pane
                .child(self.render_image_pane(window, cx))
                .into_any_element();
        }
        if self.show_pdf_pane() && self.file.is_some() {
            let mut pdf = div()
                .debug_selector(|| "obs-pdf".into())
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .bg(hsla(t.bg));
            if let Some(p) = &self.pdf_preview {
                pdf = pdf.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .child(cached_view(p, fill(), cx)),
                );
            }
            pdf = pdf.child(
                div()
                    .px(px(12.))
                    .py(px(8.))
                    .child(self.image_meta(None, cx)),
            );
            return pane.child(pdf).into_any_element();
        }
        if self.show_video_pane() && self.file.is_some() {
            let zoom = titled(
                "video-zoom-t",
                "Zoom",
                icon_btn("video-zoom", IconName::ZoomIn, false, None, cx)
                    .debug_selector(|| "video-zoom".into())
                    .on_click(cx.guarded_listener(|o, _, _, cx| o.zoom_video(cx))),
            )
            .into_any_element();
            let video = crate::render::checkerboard(cx)
                .debug_selector(|| "obs-video".into())
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(12.))
                .p(px(24.))
                .child(
                    div()
                        .border_1()
                        .border_color(hsla(t.line))
                        .bg(hsla(recur_ui::hex(0x000000)))
                        .w(px(320.))
                        .h(px(180.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(IconName::Play).size(px(28.)).color(hsla(t.muted))),
                )
                .child(self.image_meta(Some(zoom), cx));
            return pane.child(video).into_any_element();
        }
        let show_preview = self.show_preview();
        let show_diff = self.show_diff();
        if !self.editor_path.is_empty()
            && let Some(ed) = self.main_editor.clone()
        {
            let off = show_preview || show_diff || self.view_mode == EditorViewMode::Diff;
            pane = pane.child(surface(cached_view(&ed, fill(), cx), off));
        }
        if let Some((_, fp)) = &self.file_preview {
            pane = pane.child(surface(cached_view(fp, fill(), cx), !show_preview));
        }
        if show_diff {
            if let Some((_, m)) = &self.merge_editor {
                pane = pane.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .child(cached_view(m, fill(), cx)),
                );
            }
        } else if self.editor_path.is_empty() && !show_preview {
            // Its own cached view, filling the rest of the pane: a key typed in the chat redraws
            // the Observatory but must not lay this out again.
            let empty = self
                .empty_pane
                .get_or_insert_with(|| recur_ui::pane::Pane::of(cx, Self::main_empty))
                .clone();
            pane = pane.child(cached_view(
                &empty,
                gpui_kit::StyleRefinement::default()
                    .flex_1()
                    .min_h_0()
                    .w_full(),
                cx,
            ));
        }
        if (self.show_file_loading || self.diff_pending()) && !self.editor_path.is_empty() {
            let name = self
                .selected_path
                .rsplit('/')
                .next()
                .unwrap_or(&self.selected_path)
                .to_string();
            pane = pane.child(
                div()
                    .debug_selector(|| "obs-load-overlay".into())
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(hsla(t.bg.fade(0.55)))
                    .child(
                        text(format!("Loading {name}…"))
                            .mono()
                            .size(12.)
                            .color(t.muted),
                    ),
            );
        }
        let _ = window;
        pane.into_any_element()
    }

    /// `.obs-editors`: the main pane, and the split pane with its drag handle.
    fn render_editors(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.main_surface(cx) != MainSurface::Editor {
            return None;
        }
        let t = Theme::of(cx).tokens;
        let split = self.show_split_layout();
        let main = self.render_main_pane(window, cx);
        let mut row = div()
            .debug_selector(|| "obs-editors".into())
            .relative()
            .flex()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .child(crate::geometry::probe(&self.bounds.editors));
        if !split {
            return Some(row.child(main).into_any_element());
        }
        let pct = self.split_pct as f32 / 100.;
        let ring = |focused: bool| {
            focused.then(|| {
                div()
                    .absolute()
                    .inset_0()
                    .border_1()
                    .border_color(hsla(t.amber.fade(0.55)))
            })
        };
        let main_focused = self.focused_pane == Pane::Main;
        row = row.child(
            div()
                .relative()
                .flex_none()
                .w(gpui_kit::relative(pct))
                .flex()
                .min_w_0()
                .child(main)
                .children(ring(main_focused)),
        );
        let mut side = div()
            .id("obs-pane-split")
            .debug_selector(|| "obs-editor-pane:split".into())
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .border_l_1()
            .border_color(hsla(t.line))
            .capture_any_mouse_down(cx.guarded_listener(
                |o, e: &gpui_kit::MouseDownEvent, window, cx| {
                    if e.button == MouseButton::Left && o.focused_pane != Pane::Split {
                        o.focus_pane(Pane::Split, window, cx)
                    }
                },
            ));
        if !self.split_path.is_empty()
            && let Some(ed) = self.split_editor.clone()
        {
            side = side.child(surface(cached_view(&ed, fill(), cx), false));
        } else {
            side = side.child(obs_empty(
                "Open a file — it will land in this pane while focused.",
                true,
                cx,
            ));
        }
        row = row.child(side.children(ring(!main_focused)));
        row = row.child(self.split_handle(cx));
        Some(row.into_any_element())
    }

    /// `.vsplit.obs-esplit`: drag the split (20–80%), arrows step 2 (5 with Shift).
    fn split_handle(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let dragging = self.split_resizing;
        let pct = self.split_pct as f32 / 100.;
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .left(gpui_kit::relative(pct))
            .child(
                div()
                    .id("obs-esplit")
                    .debug_selector(|| "obs-esplit".into())
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(-3.))
                    .w(px(6.))
                    .cursor_col_resize()
                    .when(dragging, |d| d.bg(hsla(t.amber.fade(0.55))))
                    .hover(move |s| s.bg(hsla(t.amber.fade(0.55))))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.guarded_listener(|o, _, _, cx| {
                            o.split_resizing = true;
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .on_drag((), |_, _, _, cx| {
                        gpui_kit::AppContext::new(cx, |_| gpui_kit::Empty)
                    })
                    .on_drag_move(cx.guarded_listener(
                        |o, ev: &gpui_kit::DragMoveEvent<()>, _, cx| {
                            let b = o.bounds.editors.get();
                            let w = f32::from(b.size.width);
                            if w <= 0. {
                                return;
                            }
                            let x = f32::from(ev.event.position.x - b.origin.x);
                            o.set_split_pct((x / w * 100.) as f64, false, cx);
                        },
                    ))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.guarded_listener(|o, _, _, cx| {
                            if o.split_resizing {
                                o.split_resizing = false;
                                let p = o.split_pct as f64;
                                o.set_split_pct(p, true, cx);
                            }
                        }),
                    )
                    .on_key_down(cx.guarded_listener(|o, e: &gpui_kit::KeyDownEvent, _, cx| {
                        let step = if e.keystroke.modifiers.shift { 5. } else { 2. };
                        let p = o.split_pct as f64;
                        match e.keystroke.key.as_str() {
                            "left" => o.set_split_pct(p - step, true, cx),
                            "right" => o.set_split_pct(p + step, true, cx),
                            _ => return,
                        }
                        cx.stop_propagation();
                    })),
            )
            .into_any_element()
    }

    /// `.obs-review-host`.
    fn render_review_host(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.is_git(cx) {
            self.review_surface = None;
            return None;
        }
        self.sync_review_surface(window, cx);
        let r = self.review_surface.clone()?;
        // `.obs-review-host.off` is `display: none`: the surface keeps its state but leaves the
        // layout (laid out hidden, a large diff costs seconds a frame).
        if self.main_surface(cx) == MainSurface::Editor {
            return None;
        }
        Some(
            div()
                .id("obs-review-host")
                .debug_selector(|| "obs-review-host".into())
                .track_focus(&self.review_focus)
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .overflow_hidden()
                // Cached, so a redraw passing through the Observatory (a key typed in the chat)
                // does not render it. Its own scrolling dirties it and rebuilds what is inside;
                // the card stack and the file list are memoized for that.
                .child(cached_view(&r, fill(), cx))
                .into_any_element(),
        )
    }

    /// The editor's status bar (`Ln, Col · language · Bundles · git · problems · symbol`).
    fn render_editor_status(&mut self, corner: (bool, bool), cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        // Nowrap text keeps each item's exact width; once the bar overflows (the spacer is
        // gone) the items wrap like the web bar's, until it is wider than where that began.
        let (bar_w, sp_w) = (
            f32::from(self.bounds.status.get().size.width),
            f32::from(self.bounds.status_sp.get().size.width),
        );
        match self.status_wrap {
            None if bar_w > 0. && sp_w < 0.25 => {
                self.status_wrap = Some(bar_w);
                cx.notify();
            }
            Some(w0) if bar_w > w0 + 0.5 => {
                self.status_wrap = None;
                cx.notify();
            }
            _ => {}
        }
        let wrap = self.status_wrap.is_some();
        let w = move |t: recur_ui::text::Text| {
            if wrap {
                t.white_space(recur_ui::text::WhiteSpace::Normal)
            } else {
                t
            }
        };
        let active = self.active_path().to_string();
        let lang = self.status_language(cx);
        let st = self.git_status.get(&active).cloned().unwrap_or_default();
        let mut bar = statusbar(cx)
            .debug_selector(|| "obs-statusbar".into())
            .relative()
            .child(crate::geometry::probe(&self.bounds.status))
            .when(corner.0, |d| d.pl(px(crate::style::WIN_CORNER_PAD)))
            .when(corner.1, |d| d.pr(px(crate::style::WIN_CORNER_PAD)))
            .child(w(foot_text(format!(
                "Ln {}, Col {}",
                self.cursor_line, self.cursor_col
            ))))
            .child(foot_sep(cx))
            .child(
                linkish("status-lang", None, cx)
                    .debug_selector(|| "status:lang".into())
                    .child(w(link_text(lang)))
                    .on_click(cx.guarded_listener(|o, _, window, cx| {
                        o.quick_open.update(cx, |q, cx| q.close(cx));
                        o.state.update(cx, |s, cx| s.open_syntax_mode_picker(cx));
                        let _ = window;
                    })),
            )
            .child(foot_sep(cx))
            .child(titled(
                "status-bundles-t",
                "Commands and snippets for this file",
                linkish("status-bundles", None, cx)
                    .child(w(link_text("Bundles")))
                    .on_click(cx.guarded_listener(|o, _, _, cx| {
                        o.state.update(cx, |s, cx| s.open_commands_palette(cx))
                    })),
            ))
            .child(kbd("⌘⇧P").size(10.5));
        if !st.is_empty() {
            let base = self.review_base(cx);
            let c = match st.as_str() {
                "m" => t.amber,
                "a" | "u" => t.emerald,
                "d" => t.coral,
                _ => t.faint,
            };
            bar = bar
                .child(foot_sep(cx))
                .child(w(foot_text(st.to_uppercase())).weight(600.).color(c))
                .child(
                    div().ml(px(8.)).child(
                        text(format!(
                            "vs {}",
                            if base.is_empty() { "HEAD" } else { &base }
                        ))
                        .mono()
                        .size(11.)
                        .line_height(LineHeight::Scale(1.))
                        .color(t.muted)
                        .white_space(if wrap {
                            recur_ui::text::WhiteSpace::Normal
                        } else {
                            recur_ui::text::WhiteSpace::NoWrap
                        }),
                    ),
                );
        }
        if self.active_dirty() {
            bar = bar.child(foot_sep(cx)).child(w(foot_text("Modified")));
        }
        if self.split_open {
            bar = bar.child(foot_sep(cx)).child(w(foot_text(format!(
                "{} focused",
                if self.focused_pane == Pane::Split {
                    "Split"
                } else {
                    "Main"
                }
            ))));
        }
        let problems = crate::lsp::problems::group_by_path(&self.problems(cx));
        if let Some(pf) = problems.get(&active).cloned() {
            let warn = pf.errors == 0 && pf.warnings > 0;
            let c = if warn { t.amber } else { t.coral };
            let mut parts: Vec<AnyElement> = Vec::new();
            if pf.errors > 0 {
                parts.push(
                    w(link_text(format!(
                        "{} error{}",
                        pf.errors,
                        if pf.errors == 1 { "" } else { "s" }
                    )))
                    .into_any_element(),
                );
            }
            if pf.errors > 0 && pf.warnings > 0 {
                parts.push(foot_sep(cx));
            }
            if pf.warnings > 0 {
                parts.push(
                    w(link_text(format!(
                        "{} warning{}",
                        pf.warnings,
                        if pf.warnings == 1 { "" } else { "s" }
                    )))
                    .into_any_element(),
                );
            }
            if pf.errors == 0 && pf.warnings == 0 {
                parts.push(
                    w(link_text(format!(
                        "{} issue{}",
                        pf.count,
                        if pf.count == 1 { "" } else { "s" }
                    )))
                    .into_any_element(),
                );
            }
            bar = bar.child(foot_sep(cx)).child(titled(
                "status-problems-t",
                "Open Problems",
                linkish("status-problems", Some(c), cx)
                    .gap(px(6.))
                    .children(parts)
                    .on_click(cx.guarded_listener(|o, _, window, cx| {
                        o.select_code_panel(CodePanelKey::Problems, window, cx)
                    })),
            ));
        }
        bar = bar.child(
            div()
                .relative()
                .flex_1()
                .child(crate::geometry::probe(&self.bounds.status_sp)),
        );
        let line = self.cursor_line as i64;
        let sym = {
            let syms: Vec<recur_textmate::OutlineSymbol> = self
                .outline
                .iter()
                .map(|s| recur_textmate::OutlineSymbol {
                    name: s.name.clone(),
                    kind: s.kind.clone(),
                    line: s.line.max(0) as usize,
                    end: None,
                })
                .collect();
            recur_textmate::symbol_at_line(&syms, line.max(0) as usize)
                .map(|s| s.name.trim().to_string())
        };
        if let Some(name) = sym {
            bar = bar
                .child(foot_sep(cx))
                .child(titled(
                    "status-sym-t",
                    format!("{name} — Go to Symbol"),
                    linkish("status-sym", None, cx)
                        .min_w_0()
                        .max_w(px(28. * 6.3))
                        .child(w(foot_text(name)).truncate())
                        .on_click(cx.guarded_listener(|o, _, window, cx| {
                            o.open_quick_open(crate::quick_open::QuickOpenMode::Symbol, window, cx)
                        })),
                ))
                .child(foot_sep(cx));
        }
        bar = bar
            .child(
                linkish("status-gosym", None, cx)
                    .child(w(link_text("Go to Symbol")))
                    .on_click(cx.guarded_listener(|o, _, window, cx| {
                        o.open_quick_open(crate::quick_open::QuickOpenMode::Symbol, window, cx)
                    })),
            )
            .child(kbd("⌘⇧O").size(10.5));
        bar.into_any_element()
    }

    /// The review status bar (files, totals, scope, findings).
    fn render_review_status(&mut self, corner: (bool, bool), cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let b = self.review_bound.clone();
        let base = self.review_base(cx);
        let scope = if !b.commit_scope_hash.is_empty() {
            "this commit".to_string()
        } else if b.working_open {
            "vs HEAD".to_string()
        } else {
            format!("vs {}", if base.is_empty() { "HEAD" } else { &base })
        };
        let mut bar = statusbar(cx)
            .debug_selector(|| "obs-review-status".into())
            .when(corner.0, |d| d.pl(px(crate::style::WIN_CORNER_PAD)))
            .when(corner.1, |d| d.pr(px(crate::style::WIN_CORNER_PAD)))
            .child(foot_text(format!(
                "{} file{}",
                b.file_count,
                if b.file_count == 1 { "" } else { "s" }
            )))
            .child(foot_sep(cx))
            .child(
                div()
                    .flex()
                    .gap(px(6.3))
                    .child(
                        foot_text(format!("+{}", b.add))
                            .weight(600.)
                            .color(t.emerald),
                    )
                    .child(foot_text(format!("−{}", b.del)).weight(600.).color(t.coral)),
            )
            .child(foot_sep(cx))
            .child(
                div().ml(px(8.)).child(
                    text(scope)
                        .mono()
                        .size(11.)
                        .line_height(LineHeight::Scale(1.))
                        .color(t.muted)
                        .nowrap(),
                ),
            );
        let n = self.open_findings_count();
        if n > 0 {
            bar = bar.child(foot_sep(cx)).child(
                linkish("review-findings", Some(t.amber), cx)
                    .child(
                        link_text(format!("{n} finding{}", if n == 1 { "" } else { "s" }))
                            .weight(600.),
                    )
                    .on_click(cx.guarded_listener(|o, _, window, cx| {
                        o.select_code_panel(CodePanelKey::Findings, window, cx)
                    })),
            );
        }
        bar.into_any_element()
    }

    /// `.obs-main`, with the dock below, beside or overlaying it.
    pub(crate) fn render_main(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let hidden = self.editor_tucked(cx) && self.main_surface(cx) == MainSurface::Editor;
        let beside = self.term_beside_layout(cx);
        let term_open = self.term_open(cx);
        let overlaying = self.term_overlaying();
        self.ensure_dock(window, cx);
        let no_dock = !term_open;
        let rail_collapsed = self.state.read(cx).rail_collapsed;
        let no_left = !self.has_left(cx) || !self.left_gutter_open();
        let no_side = !self.has_right(cx) || !self.show_side;
        let corner = (rail_collapsed && no_left && no_dock, no_side && no_dock);
        let crumb = (!hidden)
            .then(|| self.render_crumbbar(window, cx))
            .flatten();
        let editors = (!hidden).then(|| self.render_editors(window, cx)).flatten();
        let review = (!hidden)
            .then(|| self.render_review_host(window, cx))
            .flatten();
        let status = if hidden {
            None
        } else if !self.active_path().is_empty() && self.main_surface(cx) == MainSurface::Editor {
            Some(self.render_editor_status(corner, cx))
        } else if self.main_surface(cx) == MainSurface::Review {
            Some(self.render_review_status(corner, cx))
        } else {
            None
        };
        let fs = self.editor_fullscreen;
        let mut col = div().flex().flex_col().min_w_0().min_h_0().flex_1();
        col = col
            .children(crumb.filter(|_| !fs))
            .children(editors)
            .children(review)
            .children(status.filter(|_| !fs));
        let dock_el = self.dock.as_ref().map(|(_, d)| d.clone());
        let mut main = div()
            .id("obs-main")
            .debug_selector(|| "obs-main".into())
            .flex_1()
            .track_focus(&self.main_focus)
            .relative()
            .flex()
            .min_w_0()
            .min_h_0()
            .bg(hsla(t.bg))
            .child(crate::geometry::probe(&self.bounds.main))
            .on_key_down(
                cx.guarded_listener(|o, e: &gpui_kit::KeyDownEvent, window, cx| {
                    if o.editor_fullscreen && e.keystroke.key == "escape" {
                        o.set_editor_fullscreen(false, window, cx);
                        cx.stop_propagation();
                    }
                }),
            )
            .when(fs, |d| {
                d.on_mouse_move(cx.guarded_listener(|o, _, _, cx| o.reveal_fs_exit(cx)))
            });
        if beside {
            let pct = self.term_beside_pct(cx) as f32 / 100.;
            main = main.flex_row().child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .w(gpui_kit::relative(pct))
                    .min_w_0()
                    .min_h_0()
                    .child(col),
            );
            if let Some(d) = dock_el.filter(|_| !overlaying) {
                main = main.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .relative()
                        .child(crate::geometry::probe(&self.bounds.dock))
                        .child(d),
                );
            } else if overlaying && !self.overlay_keep_editor_hidden {
                main = main.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(crate::geometry::probe(&self.bounds.slot)),
                );
            }
            main = main.child(self.beside_handle(pct, cx));
        } else {
            main = main.flex_col();
            if !hidden {
                main = main.child(col);
            }
            if term_open {
                if overlaying && !self.overlay_keep_editor_hidden {
                    main = main.child(
                        div()
                            .flex_none()
                            .h(px(self.term_slot_h))
                            .relative()
                            .child(crate::geometry::probe(&self.bounds.slot)),
                    );
                } else if !overlaying && let Some(d) = dock_el {
                    let h = self.state.read(cx).panels.size(PanelKey::TerminalDock);
                    let holder = div()
                        .relative()
                        .min_h_0()
                        .child(crate::geometry::probe(&self.bounds.dock))
                        .child(d);
                    main = main.child(if hidden {
                        holder.flex_1()
                    } else {
                        holder.flex_none().h(px(h))
                    });
                }
            }
        }
        if fs {
            let show = self.fs_exit_visible || self.fs_exit_hovered;
            main = main.child(
                div()
                    .id("obs-fs-exit")
                    .absolute()
                    .top_0()
                    .right_0()
                    .p(px(12.))
                    .on_hover(cx.guarded_listener(|o, on: &bool, _, cx| {
                        o.fs_exit_hovered = *on;
                        cx.notify();
                    }))
                    .child(titled(
                        "fs-exit-t",
                        "Exit full screen (Esc)",
                        icon_btn("fs-exit", IconName::Minimize, false, None, cx)
                            .debug_selector(|| "fs-exit".into())
                            .bg(hsla(t.pane_2))
                            .border_1()
                            .border_color(hsla(t.line))
                            .when(!show, |d| d.opacity(0.))
                            .on_click(cx.guarded_listener(|o, _, window, cx| {
                                o.set_editor_fullscreen(false, window, cx)
                            })),
                    )),
            );
        }
        if let Some(lb) = &self.media_zoom {
            main = main.child(lb.clone());
        }
        main.into_any_element()
    }

    /// `.vsplit.obs-tsplit`: drag the editor / terminal split when they sit side by side.
    fn beside_handle(&mut self, pct: f32, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let dragging = self.term_beside_resizing;
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .left(gpui_kit::relative(pct))
            .child(
                div()
                    .id("obs-tsplit")
                    .debug_selector(|| "obs-tsplit".into())
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(-3.))
                    .w(px(6.))
                    .cursor_col_resize()
                    .when(dragging, |d| d.bg(hsla(t.amber.fade(0.55))))
                    .hover(move |s| s.bg(hsla(t.amber.fade(0.55))))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.guarded_listener(|o, _, _, cx| {
                            o.term_beside_resizing = true;
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .on_drag((), |_, _, _, cx| {
                        gpui_kit::AppContext::new(cx, |_| gpui_kit::Empty)
                    })
                    .on_drag_move(cx.guarded_listener(
                        |o, ev: &gpui_kit::DragMoveEvent<()>, _, cx| {
                            let b = o.bounds.main.get();
                            let w = f32::from(b.size.width);
                            if w <= 0. {
                                return;
                            }
                            let x = f32::from(ev.event.position.x - b.origin.x);
                            let id = o.props.workspace_id.clone();
                            o.state.update(cx, |s, cx| {
                                s.set_terminal_beside_pct(&id, (x / w * 100.) as f64, false, cx)
                            });
                        },
                    ))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.guarded_listener(|o, _, _, cx| {
                            if o.term_beside_resizing {
                                o.term_beside_resizing = false;
                                let id = o.props.workspace_id.clone();
                                let p = o.term_beside_pct(cx) as f64;
                                o.state.update(cx, |s, cx| {
                                    s.set_terminal_beside_pct(&id, p, true, cx)
                                });
                            }
                        }),
                    )
                    .on_key_down(cx.guarded_listener(|o, e: &gpui_kit::KeyDownEvent, _, cx| {
                        let step = if e.keystroke.modifiers.shift { 5. } else { 2. };
                        let p = o.term_beside_pct(cx) as f64;
                        let id = o.props.workspace_id.clone();
                        let next = match e.keystroke.key.as_str() {
                            "left" => p - step,
                            "right" => p + step,
                            _ => return,
                        };
                        cx.stop_propagation();
                        o.state
                            .update(cx, |s, cx| s.set_terminal_beside_pct(&id, next, true, cx));
                    })),
            )
            .into_any_element()
    }

    /// The maximized dock (`.obs.term-max .term-dock`): fixed over the Observatory at the
    /// animated rectangle, with the scrim under it.
    pub(crate) fn render_term_overlay(&mut self, _cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.term_overlaying() {
            return None;
        }
        let (_, d) = self.dock.clone()?;
        let r = self.overlay_rect;
        let _ = TermMaxPhase::Idle;
        Some(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .child(
                    div()
                        .absolute()
                        .top(px(r.top))
                        .left(px(r.left))
                        .w(px(r.width))
                        .h(px(r.height))
                        .child(crate::geometry::probe(&self.bounds.dock))
                        .child(d),
                )
                .into_any_element(),
        )
    }
}

/// `.obs-surface` (`.off` keeps it mounted but hidden and inert).
/// A pane's editor or preview as a cached view: it re-renders when it notifies, not on every
/// frame the Observatory draws (typing in the chat beside it redraws the Observatory).
fn fill() -> gpui_kit::StyleRefinement {
    gpui_kit::StyleRefinement::default().size_full()
}

fn surface(child: AnyElement, off: bool) -> AnyElement {
    let d = div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .overflow_hidden()
        .relative()
        .child(child);
    if off {
        div()
            .absolute()
            .inset_0()
            .invisible()
            .child(d)
            .into_any_element()
    } else {
        d.into_any_element()
    }
}

impl Observatory {
    /// The main pane with no file open: loading, a directory, or the prompt to pick a file.
    fn main_empty(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let name = self
            .selected_path
            .rsplit('/')
            .next()
            .unwrap_or(&self.selected_path)
            .to_string();
        if self.show_file_loading {
            obs_empty(format!("Loading {name}…"), true, cx)
                .text_color(hsla(t.muted))
                .into_any_element()
        } else if self.file_loading {
            obs_empty("", true, cx).into_any_element()
        } else if self.selected_is_dir {
            obs_empty(
                "This path is a directory (often a git submodule). Use Diff to see the recorded change, or browse it from the Files tree.",
                true,
                cx,
            )
            .into_any_element()
        } else if self.tree_writable() {
            obs_empty("Select a file to view/edit.", true, cx).into_any_element()
        } else {
            obs_empty("Select a file to view.", true, cx).into_any_element()
        }
    }
}
