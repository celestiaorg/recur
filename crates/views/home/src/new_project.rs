//! The quick-create project modal (`newProjectOpen`). Picks a repository
//! (typed or through the folder dialog), names it after the folder until the user renames
//! it, suggests and lists its branches, and adds it; reopens New workspace afterwards when it
//! was opened from there.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, ParentElement, PathPromptOptions, Render,
    StatefulInteractiveElement, Styled, Subscription, Task, Window, div, px,
};
use recur_domain::{BranchChoice, Project};
use recur_ui::field::{Field, FieldEvent, FieldOpts};
use recur_ui::fonts::weight;
use recur_ui::guard::Guarded;
use recur_ui::set::{InpStyle, inp};
use recur_ui::shadow::with_shadow;
use recur_ui::style::{
    MODAL_SHADOW, Tbtn, TbtnSize, button_label, modal_panel, modal_scrim, nt_esc_hint, outline,
    primary_button, tbtn,
};
use recur_ui::text::{TextStyled, text};
use recur_ui::widgets::project_pic::project_pic_from_name;
use recur_ui::widgets::{Select, SelectEvent, SelectOption, ToastKind};
use recur_ui::{IconName, Theme, icon};
use recur_view_core::{ObserveState as _, Topics};
use recur_view_core::{ViewContext, call, toast};

use crate::dialog::{cycle, window_keys};

/// A Tab stop of the dialog, in DOM order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    Path,
    Browse,
    Name,
    Branch,
    Go,
}

/// The last path segment (`path.replace(/[/\\]+$/, '').split(/[/\\]/).pop()`).
pub fn base_name(path: &str) -> String {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_string()
}

pub struct NewProjectModal {
    vcx: ViewContext,
    focus: FocusHandle,
    open: bool,
    path: Field,
    name: Field,
    base_branch: String,
    suggested_for: String,
    choices: Vec<BranchChoice>,
    picking: bool,
    saving: bool,
    auto_name: String,
    branch: Entity<Select>,
    browse_focus: FocusHandle,
    go_focus: FocusHandle,
    /// The control Tab last moved to (`.kb-focus`), cleared by a pointer press.
    kb: Option<Stop>,
    hover_x: bool,
    suggest_key: Option<String>,
    suggest: Option<Task<()>>,
    list_key: Option<(String, String)>,
    list: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl Focusable for NewProjectModal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl NewProjectModal {
    pub fn new(vcx: &ViewContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let branch = cx.new(|cx| {
            Select::new(Vec::new(), window, cx)
                .label("Default branch")
                .searchable()
                .trigger_icon(IconName::GitBranch)
        });
        let subs = vec![
            window_keys(cx, Self::on_key),
            cx.observe_state_in(&vcx.state, Topics::ALL, window, |this, _, window, cx| {
                this.sync(window, cx)
            }),
            cx.guarded_subscribe(&branch, |this, _, ev: &SelectEvent, cx| {
                if let SelectEvent::Change(v) = ev {
                    this.base_branch = v.to_string();
                    cx.notify();
                }
            }),
        ];
        let path = Field::new(
            "path".into(),
            FieldOpts::default(),
            Self::field_event,
            window,
            cx,
        );
        let name = Field::new(
            "name".into(),
            FieldOpts::default(),
            Self::field_event,
            window,
            cx,
        );
        NewProjectModal {
            vcx: vcx.clone(),
            focus: cx.focus_handle(),
            open: false,
            path,
            name,
            base_branch: String::new(),
            suggested_for: String::new(),
            choices: Vec::new(),
            picking: false,
            saving: false,
            auto_name: String::new(),
            branch,
            browse_focus: cx.focus_handle().tab_stop(true),
            go_focus: cx.focus_handle().tab_stop(true),
            kb: None,
            hover_x: false,
            suggest_key: None,
            suggest: None,
            list_key: None,
            list: None,
            _subs: subs,
        }
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.vcx.state.read(cx).overlays.new_project;
        if open && !self.open {
            self.reset(window, cx);
            // `setTimeout(() => pathEl.focus(), 0)`.
            let f = gpui_kit::Focusable::focus_handle(self.path.input.read(cx), cx);
            window.focus(&f, cx);
        }
        if !open && self.open {
            self.suggest = None;
            self.list = None;
            // Focus inside the closed dialog falls back to the document (`body`).
            if self
                .stops(cx)
                .iter()
                .any(|s| self.handle(*s, cx).is_focused(window))
            {
                window.blur(cx);
            }
        }
        self.open = open;
        cx.notify();
    }

    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.path.set("", window, cx);
        self.name.set("", window, cx);
        self.base_branch.clear();
        self.suggested_for.clear();
        self.choices.clear();
        self.picking = false;
        self.saving = false;
        self.auto_name.clear();
        self.kb = None;
        self.suggest_key = None;
        self.suggest = None;
        self.list_key = None;
        self.list = None;
    }

    pub fn repo_path(&self, cx: &App) -> String {
        self.path.value(cx)
    }

    /// Types `path` into the Repository path field (drivers and specs).
    pub fn set_repo_path(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_repo_path(path, window, cx);
        self.effects(cx);
        cx.notify();
    }

    pub fn can_save(&self, cx: &App) -> bool {
        !self.path.value(cx).trim().is_empty() && !self.saving
    }

    fn field_event(
        &mut self,
        key: &str,
        ev: FieldEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ev != FieldEvent::Input {
            cx.notify();
            return;
        }
        if key == "path" {
            let v = self.path.value(cx);
            self.apply_repo_path(&v, window, cx);
        }
        self.effects(cx);
        cx.notify();
    }

    /// `applyRepoPath`: the name follows the folder until the user edits it.
    fn apply_repo_path(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.path.value(cx) != path {
            self.path.set(path, window, cx);
        }
        self.suggested_for.clear();
        let base = base_name(path);
        let name = self.name.value(cx);
        if name.trim().is_empty() || name == self.auto_name {
            self.name.set(&base, window, cx);
            self.auto_name = base;
        }
    }

    /// The modal's two `$effect`s: suggest a base for a new path, and list its branches
    /// (again once the suggestion lands, preferring it).
    fn effects(&mut self, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        let path = self.path.value(cx).trim().to_string();
        let services = self.vcx.services.clone();
        let want = (!path.is_empty() && path != self.suggested_for).then(|| path.clone());
        if want != self.suggest_key {
            self.suggest_key = want.clone();
            self.suggest = want.map(|path| {
                self.base_branch.clear();
                let s = services.clone();
                cx.spawn(async move |this, cx| {
                    let r = s.projects.suggest_base_branch(&path).await;
                    let _ = this.update(cx, |m, cx| {
                        match r {
                            Ok(b) if b.is_empty() => return,
                            Ok(b) => {
                                m.suggested_for = path;
                                if m.base_branch.is_empty() {
                                    m.base_branch = b;
                                }
                            }
                            Err(_) => {
                                m.suggested_for = path;
                                if m.base_branch.is_empty() {
                                    m.base_branch = "main".into();
                                }
                            }
                        }
                        m.effects(cx);
                        cx.notify();
                    });
                })
            });
        }
        let key = (path.clone(), self.suggested_for.clone());
        if self.list_key.as_ref() != Some(&key) {
            self.list_key = Some(key);
            if path.is_empty() {
                self.choices.clear();
                self.list = None;
            } else {
                let preferred = self.base_branch.trim().to_string();
                self.list = Some(cx.spawn(async move |this, cx| {
                    let r = services
                        .projects
                        .list_branches_for_path(&path, &preferred)
                        .await;
                    let _ = this.update(cx, |m, cx| {
                        m.choices = r.unwrap_or_default();
                        cx.notify();
                    });
                }));
            }
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.vcx.state.update(cx, |s, cx| s.close_new_project(cx));
    }

    /// `pickRepo`: the folder dialog, starting nowhere in particular.
    fn pick_repo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picking {
            return;
        }
        self.picking = true;
        cx.notify();
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Select".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let picked = rx.await.ok().and_then(Result::ok).flatten();
            let _ = this.update_in(cx, |m, window, cx| {
                m.picking = false;
                if let Some(p) = picked.and_then(|v| v.into_iter().next()) {
                    let p = p.to_string_lossy().to_string();
                    if !p.is_empty() {
                        m.apply_repo_path(&p, window, cx);
                        m.effects(cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// `create`.
    pub fn create(&mut self, cx: &mut Context<Self>) {
        if !self.can_save(cx) {
            return;
        }
        self.saving = true;
        cx.notify();
        let path = self.path.value(cx).trim().to_string();
        let name = self.name.value(cx).trim().to_string();
        let label = if name.is_empty() {
            base_name(&path)
        } else {
            name
        };
        let project = Project {
            name: label.clone(),
            repo_path: path,
            base_branch: self.base_branch.trim().to_string(),
            pic: project_pic_from_name(&label),
            ..Default::default()
        };
        let services = self.vcx.services.clone();
        let state = self.vcx.state.clone();
        cx.spawn(async move |this, cx| {
            let saved = call(
                cx,
                services.projects.add_project(&project),
                "Could not create project",
            )
            .await;
            if let Some(saved) = saved {
                let load = state.update(cx, |s, cx| s.load_initial(cx));
                let _ = load.await;
                state.update(cx, |s, cx| {
                    toast(ToastKind::Success, "Project created", cx);
                    let resume = s.take_new_project_resume_new_task();
                    s.close_new_project(cx);
                    if resume && !saved.id.is_empty() {
                        s.open_new_task(&saved.id, None, cx);
                    }
                });
            }
            let _ = this.update(cx, |m, cx| {
                m.saving = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn stops(&self, cx: &App) -> Vec<Stop> {
        let mut v = vec![Stop::Path, Stop::Browse, Stop::Name];
        if !self.choices.is_empty() {
            v.push(Stop::Branch);
        }
        if self.can_save(cx) {
            v.push(Stop::Go);
        }
        v
    }

    fn handle(&self, s: Stop, cx: &App) -> FocusHandle {
        match s {
            Stop::Path => gpui_kit::Focusable::focus_handle(self.path.input.read(cx), cx),
            Stop::Name => gpui_kit::Focusable::focus_handle(self.name.input.read(cx), cx),
            Stop::Browse => self.browse_focus.clone(),
            Stop::Branch => self.branch.focus_handle(cx),
            Stop::Go => self.go_focus.clone(),
        }
    }

    /// The window keydown listener (capture): Tab cycles, ⌘↵ creates, Escape closes unless a
    /// select menu is open.
    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        let m = &ev.keystroke.modifiers;
        let key = ev.keystroke.key.as_str();
        if key == "tab" && !m.platform && !m.control && !m.alt {
            cx.stop_propagation();
            let stops = self.stops(cx);
            let at = stops
                .iter()
                .position(|s| self.handle(*s, cx).is_focused(window));
            if let Some(i) = cycle(stops.len(), at, m.shift) {
                let s = stops[i];
                // `.kb-focus` rings only pickers and the submit button.
                self.kb = matches!(s, Stop::Branch | Stop::Go).then_some(s);
                let h = self.handle(s, cx);
                window.focus(&h, cx);
                cx.notify();
            }
            return;
        }
        if key == "enter" && (m.platform || m.control) {
            cx.stop_propagation();
            self.create(cx);
            return;
        }
        if key == "escape" {
            if self.branch.read(cx).is_open() {
                return;
            }
            cx.stop_propagation();
            self.close(cx);
        }
    }
}

impl NewProjectModal {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        if !self.open {
            return div().into_any_element();
        }
        let t = Theme::of(cx).tokens;
        self.path.flush(window, cx);
        self.name.flush(window, cx);
        recur_ui::style::style_text_input(&self.path.input, cx);
        recur_ui::style::style_text_input(&self.name.input, cx);
        let options: Vec<SelectOption> = self
            .choices
            .iter()
            .map(|b| SelectOption::new(b.name.clone(), b.name.clone()).group(b.group.clone()))
            .collect();
        let shown = if self.base_branch.is_empty() {
            options
                .first()
                .map(|o| o.value.to_string())
                .unwrap_or_default()
        } else {
            self.base_branch.clone()
        };
        let branch_ring =
            self.kb == Some(Stop::Branch) && self.branch.focus_handle(cx).is_focused(window);
        self.branch.update(cx, |s, cx| {
            if s.value() != &shown {
                s.set_value(shown.clone(), cx);
            }
            let mut ts = s.trigger().clone();
            ts.no_outline = true;
            if s.trigger() != &ts {
                s.set_trigger_style(ts, cx);
            }
            s.set_options(options.clone(), cx);
        });

        let lbl = |s: &str| {
            text(s.to_string())
                .size(13.)
                .leading(1.45)
                .color(t.text)
                .nowrap()
        };
        let picking = self.picking;
        let browse = tbtn("np-browse", Tbtn::Default, TbtnSize::BASE, None, cx)
            .h_auto()
            .track_focus(&self.browse_focus)
            .debug_selector(|| "np-browse".into())
            .aria_label("Browse for repository")
            .when(!picking, |d| {
                d.on_click(cx.guarded_listener(|m, _, window, cx| m.pick_repo(window, cx)))
            })
            .child(icon(IconName::Folder).size(px(13.)))
            .child(button_label(
                if picking { "Opening…" } else { "Browse" },
                12.,
                weight::MEDIUM,
            ));
        let path_row = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .min_w_0()
            .child(lbl("Repository"))
            .child(
                div()
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
                                &self.path.input,
                                &"~/code/my-app".into(),
                                InpStyle::BASE,
                                window,
                                cx,
                            )),
                    )
                    .child(browse),
            );
        let name_row = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .min_w_0()
            .child(lbl("Name"))
            .child(
                div()
                    .flex()
                    .debug_selector(|| "inp:Project name".into())
                    .child(inp(
                        &self.name.input,
                        &"my-app".into(),
                        InpStyle::BASE,
                        window,
                        cx,
                    )),
            );
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .pt(px(10.))
            .px(px(16.))
            .pb(px(4.))
            .child(path_row)
            .child(name_row);
        if !options.is_empty() {
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .min_w_0()
                    .child(lbl("Default branch"))
                    .child(
                        div()
                            .relative()
                            .child(self.branch.clone())
                            .when(branch_ring, |d| d.child(outline(2., 2., 6., 0., t.amber))),
                    ),
            );
        }

        let can_save = self.can_save(cx);
        let saving = self.saving;
        let mut go = primary_button("np-go", (!can_save).then_some(t.pane), cx)
            .debug_selector(move || format!("np-go:{}", if can_save { "on" } else { "off" }))
            .relative()
            .track_focus(&self.go_focus);
        go = if saving {
            go.child(button_label("Creating…", 13., weight::SEMIBOLD))
        } else {
            go.child(button_label("Create project", 13., weight::SEMIBOLD))
                .child(recur_ui::kbd::kbd("⌘↵").size(12.).opacity(0.55))
        };
        if can_save {
            go = go.on_click(cx.guarded_listener(|m, _, _, cx| m.create(cx)));
        }
        if self.kb == Some(Stop::Go) && self.go_focus.is_focused(window) {
            go = go.child(outline(2., 2., 8., 0., t.amber));
        }
        let x = div()
            .id("np-x")
            .debug_selector(|| "np-x".into())
            .ml_auto()
            .flex_none()
            .cursor_pointer()
            .on_hover(cx.guarded_listener(|m, h: &bool, _, cx| {
                m.hover_x = *h;
                cx.notify();
            }))
            .on_click(cx.guarded_listener(|m, _, _, cx| m.close(cx)))
            .child(nt_esc_hint(self.hover_x, window, cx));
        let head = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .pt(px(12.))
            .px(px(14.))
            .min_h(px(44.))
            .child(
                div().pt(px(2.)).px(px(2.)).child(
                    text("New project")
                        .size(14.)
                        .weight(weight::SEMIBOLD)
                        .leading(1.45)
                        .color(t.text),
                ),
            )
            .child(x);
        let foot = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .pt(px(10.))
            .px(px(14.))
            .pb(px(14.))
            .min_w_0()
            .child(go);
        let panel = modal_panel(cx)
            .w_full()
            .debug_selector(|| "np-modal".into())
            .child(head)
            .child(body)
            .child(foot);
        let width = 440f32.min(f32::from(window.viewport_size().width) - 32.);
        let panel = with_shadow(panel, &MODAL_SHADOW, 16.)
            .id("np-modal")
            .w(px(width))
            .occlude()
            // `clearKeyboardFocus(modal)` on pointer down.
            .on_mouse_down(
                MouseButton::Left,
                cx.guarded_listener(|m, _, _, cx| {
                    m.kb = None;
                    cx.stop_propagation();
                    cx.notify();
                }),
            );
        modal_scrim(window, panel)
            .id("v-newproject")
            .debug_selector(|| "v-newproject".into())
            .track_focus(&self.focus)
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.guarded_listener(|m, _, _, cx| m.close(cx)),
            )
            .into_any_element()
    }
}

impl Render for NewProjectModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

/// The window's New project modal, for specs.
pub struct CurrentNewProject(pub gpui_kit::WeakEntity<NewProjectModal>);

impl gpui_kit::Global for CurrentNewProject {}

impl NewProjectModal {
    pub fn current(cx: &App) -> Option<Entity<NewProjectModal>> {
        cx.try_global::<CurrentNewProject>()
            .and_then(|c| c.0.upgrade())
    }

    /// The Name field.
    pub fn name(&self, cx: &App) -> String {
        self.name.value(cx)
    }
}

/// The overlay (`OverlaySlot::NewProject`).
pub fn build(vcx: &ViewContext, window: &mut Window, cx: &mut App) -> Entity<NewProjectModal> {
    let e = cx.new(|cx| NewProjectModal::new(vcx, window, cx));
    cx.set_global(CurrentNewProject(e.downgrade()));
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_after_the_last_path_segment() {
        assert_eq!(base_name("/Users/me/code/my-app/"), "my-app");
        assert_eq!(base_name("C:\\code\\app"), "app");
        assert_eq!(base_name("app"), "app");
        assert_eq!(base_name(""), "");
    }
}
