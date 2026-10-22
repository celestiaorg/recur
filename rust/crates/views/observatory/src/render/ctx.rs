//! `.obs-ctx`: the tree / file tab right-click menu.

use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, Context, InteractiveElement, IntoElement, MouseButton, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, anchored, deferred, div, point, px,
};
use recur_ui::kbd::kbd;
use recur_ui::shadow::{css_box_shadow, with_shadow};
use recur_ui::text::{TextStyled, text};
use recur_ui::{Theme, rgba};
use recur_view_review::style::hsla;

use crate::model::parent_path;
use crate::obs::{Observatory, OpenRequest};

type Act = Rc<dyn Fn(&mut Observatory, &mut Window, &mut Context<Observatory>)>;

enum Item {
    Btn(&'static str, String, &'static str, bool, Act),
    Sep,
}

fn btn(
    id: &'static str,
    label: impl Into<String>,
    kb: &'static str,
    disabled: bool,
    f: impl Fn(&mut Observatory, &mut Window, &mut Context<Observatory>) + 'static,
) -> Item {
    Item::Btn(id, label.into(), kb, disabled, Rc::new(f))
}

impl Observatory {
    fn ctx_items(&self, cx: &Context<Self>) -> Vec<Item> {
        let Some(m) = self.ctx_menu.clone() else {
            return Vec::new();
        };
        let into = if m.dir {
            m.path.clone()
        } else {
            parent_path(&m.path).to_string()
        };
        let mut v = Vec::new();
        let p = m.path.clone();
        let clip_label = self
            .file_clip
            .as_ref()
            .map(|c| format!("Paste “{}”", c.path.rsplit('/').next().unwrap_or(&c.path)));
        if m.tab {
            let (p1, p2) = (p.clone(), p.clone());
            v.push(btn("close", "Close", "", false, move |o, w, cx| {
                o.close_ctx(cx);
                o.close_mru(&p1, w, cx);
            }));
            v.push(btn(
                "close-others",
                "Close others",
                "",
                self.open_files.len() < 2,
                move |o, w, cx| {
                    o.close_ctx(cx);
                    let rest: Vec<String> =
                        o.open_files.iter().filter(|f| **f != p2).cloned().collect();
                    o.close_tabs(rest, w, cx);
                },
            ));
            v.push(Item::Sep);
            v.push(btn("close-all", "Close all", "", false, |o, w, cx| {
                o.close_ctx(cx);
                let all = o.open_files.clone();
                o.close_tabs(all, w, cx);
            }));
        } else if self.tree_writable() {
            let (i1, i2) = (into.clone(), into.clone());
            v.push(btn(
                "new-file",
                "New File",
                "⌥⌘N",
                false,
                move |o, w, cx| o.start_new(false, &i1, w, cx),
            ));
            v.push(btn(
                "new-folder",
                "New Folder",
                "⇧⌘N",
                false,
                move |o, w, cx| o.start_new(true, &i2, w, cx),
            ));
            v.push(Item::Sep);
            if !p.is_empty() {
                let dir = m.dir;
                let (a, b, c, d, e, f) = (
                    p.clone(),
                    p.clone(),
                    into.clone(),
                    p.clone(),
                    p.clone(),
                    p.clone(),
                );
                let (name1, name2) = (m.name.clone(), m.name.clone());
                v.push(btn("cut", "Cut", "⌘X", false, move |o, _, cx| {
                    o.clip_cut(&a, dir, cx)
                }));
                v.push(btn("copy", "Copy", "⌘C", false, move |o, _, cx| {
                    o.clip_copy(&b, dir, cx)
                }));
                v.push(btn(
                    "paste",
                    clip_label.clone().unwrap_or_else(|| "Paste".into()),
                    "⌘V",
                    self.file_clip.is_none(),
                    move |o, w, cx| o.clip_paste(&c, w, cx),
                ));
                v.push(btn(
                    "duplicate",
                    "Duplicate",
                    "⌘D",
                    false,
                    move |o, w, cx| o.duplicate_path(&d, dir, w, cx),
                ));
                v.push(Item::Sep);
                v.push(btn("rename", "Rename…", "↩", false, move |o, w, cx| {
                    o.start_rename(&e, &name1, dir, w, cx)
                }));
                v.push(btn("delete", "Delete", "⌘⌫", false, move |o, w, cx| {
                    o.delete_path(&f, &name2, w, cx)
                }));
                v.push(Item::Sep);
            } else if let Some(label) = clip_label {
                v.push(btn("paste", label, "⌘V", false, |o, w, cx| {
                    o.clip_paste("", w, cx)
                }));
                v.push(Item::Sep);
            }
        } else if !p.is_empty() {
            let (a, dir) = (p.clone(), m.dir);
            v.push(btn("copy", "Copy", "⌘C", false, move |o, _, cx| {
                o.clip_copy(&a, dir, cx)
            }));
            v.push(Item::Sep);
        }
        if !p.is_empty() && !m.tab {
            let (a, b, c) = (p.clone(), p.clone(), p.clone());
            v.push(btn(
                "copy-path",
                "Copy Path",
                "⌥⌘C",
                false,
                move |o, _, cx| o.copy_abs_path(&a, cx),
            ));
            v.push(btn(
                "copy-rel",
                "Copy Relative Path",
                "⇧⌥⌘C",
                false,
                move |o, _, cx| o.copy_rel_path(&b, cx),
            ));
            v.push(btn(
                "reveal",
                "Reveal in Finder",
                "⌥⌘R",
                false,
                move |o, _, cx| o.reveal_path(&c, cx),
            ));
            if !m.dir {
                let (a, b, c) = (p.clone(), p.clone(), p.clone());
                let diff =
                    self.filter_changed() || self.git_status.get(&p).is_some_and(|s| s == "d");
                v.push(btn("open", "Open", "⌘↓", false, move |o, w, cx| {
                    o.close_ctx(cx);
                    o.open_path(OpenRequest::new(a.clone()).prefer_diff(diff), w, cx)
                        .detach();
                }));
                v.push(btn(
                    "open-external",
                    "Open in Editor",
                    "",
                    false,
                    move |o, _, cx| o.open_external(&b, cx),
                ));
                v.push(btn(
                    "open-split",
                    "Open in Split",
                    "",
                    false,
                    move |o, w, cx| {
                        o.close_ctx(cx);
                        o.open_in_split(
                            &c,
                            0,
                            0,
                            crate::obs::ExplorerReveal::Reveal,
                            false,
                            0,
                            0,
                            w,
                            cx,
                        )
                        .detach();
                    },
                ));
                if self.tree_writable() && self.working_status.contains_key(&p) {
                    v.push(Item::Sep);
                    let (a, b) = (p.clone(), p.clone());
                    if self.staged_files.contains(&p) {
                        v.push(btn("unstage", "Unstage", "", false, move |o, _, cx| {
                            o.close_ctx(cx);
                            o.stage_paths(vec![a.clone()], false, cx);
                        }));
                    } else {
                        v.push(btn("stage", "Stage", "", false, move |o, _, cx| {
                            o.close_ctx(cx);
                            o.stage_paths(vec![a.clone()], true, cx);
                        }));
                    }
                    v.push(btn(
                        "discard",
                        "Discard Changes",
                        "",
                        false,
                        move |o, w, cx| {
                            o.close_ctx(cx);
                            o.discard_paths(vec![b.clone()], "", w, cx);
                        },
                    ));
                }
            }
        }
        let _ = cx;
        v
    }

    /// The menu at the pointer, over everything; a press outside closes it.
    pub(crate) fn render_ctx_menu(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let m = self.ctx_menu.clone()?;
        let t = Theme::of(cx).tokens;
        let mut menu = div()
            .id("obs-ctx")
            .debug_selector(|| "obs-ctx".into())
            .min_w(px(220.))
            .max_w(px(300.))
            .p(px(4.))
            .bg(hsla(t.pane))
            .border_1()
            .border_color(hsla(t.line_bright))
            .rounded(px(8.))
            .flex()
            .flex_col()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation());
        for item in self.ctx_items(cx) {
            menu = match item {
                Item::Sep => menu.child(div().h(px(1.)).mx(px(6.)).my(px(4.)).bg(hsla(t.line))),
                Item::Btn(id, label, kb, disabled, act) => menu.child(
                    div()
                        .id(SharedString::from(format!("ctx-{id}")))
                        .debug_selector(move || format!("ctx:{id}"))
                        .flex()
                        .items_center()
                        .gap(px(16.))
                        .w_full()
                        .px(px(10.))
                        .py(px(7.))
                        .rounded(px(5.))
                        .text_color(hsla(t.text))
                        .when(disabled, |d| d.opacity(0.4))
                        .when(!disabled, |d| {
                            d.cursor_pointer()
                                .hover(move |s| s.bg(hsla(t.pane_2)))
                                .on_click(cx.listener(move |o, _, w, cx| act(o, w, cx)))
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(text(label).size(12.).nowrap()),
                        )
                        .when(!kb.is_empty(), |d| {
                            d.child(kbd(kb).size(11.).color(t.faint))
                        }),
                ),
            };
        }
        let menu = with_shadow(
            menu,
            &[css_box_shadow(0., 12., 32., 0., rgba(0, 0, 0, 0.45))],
            8.,
        );
        Some(
            deferred(
                anchored().position(point(px(m.x), px(m.y))).child(
                    div()
                        .on_mouse_down_out(cx.listener(|o, _, _, cx| o.close_ctx(cx)))
                        .child(menu),
                ),
            )
            .with_priority(80)
            .into_any_element(),
        )
    }
}
