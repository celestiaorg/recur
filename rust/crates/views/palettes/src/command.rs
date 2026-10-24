//! `CommandPalette.svelte` (⌘K, Navigate → Go to…): actions, routes, workspaces, PRs and
//! projects, plus the bundle `[[search]]` groups (`SearchPalette`, such as Linear issues)
//! once the query has two characters.

use recur_ui::guard::Guarded;
use std::time::Duration;

use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, Focusable, InteractiveElement, IntoElement,
    KeystrokeEvent, ParentElement, Render, ScrollHandle, SharedString, StatefulInteractiveElement,
    Styled, Subscription, Task, Window, div, px,
};
use recur_domain::{Pr, Project, Workspace};
use recur_services::api::PaletteSearchDto;
use recur_ui::style::{
    palette_dot, palette_empty, palette_group, palette_key, palette_pic, palette_row, palette_sub,
    palette_tile, style_text_input,
};
use recur_ui::widgets::{PicBox, ToastKind, project_pic};
use recur_ui::{Color, IconName, Theme, svg_markup};
use recur_view_core::modes::ws_main_blocked;
use recur_view_core::pr::pr_icon_name;
use recur_view_core::run::workspace_dev_cmd;
use recur_view_core::{
    AppState, NewTaskDraft, ScheduleEditorRequest, ViewName, bridge_error_message,
};

use crate::chrome::{self, Kind, js_len, plain, step};

/// A row's leading `.pic`.
#[derive(Clone, Debug, PartialEq)]
pub enum Pic {
    /// No dot, picture or icon: the empty tile.
    None,
    Dot(Color),
    Project {
        pic: String,
        color: String,
        name: String,
    },
    /// A lucide icon, or a pack's own SVG markup when `svg` is set.
    Icon(IconName, String),
}

/// What a row does when run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    NewWorkspace,
    NewProject,
    NewSchedule,
    NewBlankWorkspace,
    OpenPath,
    OpenIn,
    CopyPath,
    ChangeLanguage,
    RunCommand,
    Shortcuts,
    Run,
    StopRun,
    Go(ViewName),
    OpenWorkspace(String),
    OpenPr(String),
    OpenProject(String),
    /// A `[[search]]` hit: `settings:<section>` actions open Settings, the rest expand
    /// `insert` into a New workspace draft.
    SearchHit {
        insert: String,
        action: String,
    },
}

/// `Cmd`.
#[derive(Clone, Debug, PartialEq)]
pub struct Cmd {
    pub g: String,
    pub label: String,
    pub psub: String,
    pub kb: String,
    pub pic: Pic,
    /// Searchable terms beyond label, psub and group.
    pub keys: Vec<String>,
    pub action: Action,
}

impl Cmd {
    fn new(g: &str, pic: Pic, label: &str, kb: &str, action: Action) -> Cmd {
        Cmd {
            g: g.into(),
            label: label.into(),
            psub: String::new(),
            kb: kb.into(),
            pic,
            keys: Vec::new(),
            action,
        }
    }

    fn icon(g: &str, name: IconName, label: &str, kb: &str, action: Action) -> Cmd {
        Cmd::new(g, Pic::Icon(name, String::new()), label, kb, action)
    }
}

/// What `all` reads from the stores.
pub struct Inputs<'a> {
    pub view: ViewName,
    pub selected_workspace_id: &'a str,
    pub workspaces: &'a [Workspace],
    pub prs: &'a [Pr],
    pub projects: &'a [Project],
    /// `isAnyRunRunning` for the selected workspace.
    pub any_run_running: bool,
    /// The focused editor has a file (`currentEditorFocus().path`).
    pub editor_file_open: bool,
    pub needs: Color,
    pub active: Color,
    pub ready: Color,
}

/// `all`: every command, in declaration order.
pub fn all_commands(i: &Inputs) -> Vec<Cmd> {
    let selected = i
        .workspaces
        .iter()
        .find(|w| w.id == i.selected_workspace_id);
    let on_detail = i.view == ViewName::Detail
        && !i.selected_workspace_id.is_empty()
        && !ws_main_blocked(selected);
    let can_dev = on_detail
        && !workspace_dev_cmd(i.selected_workspace_id, i.workspaces, i.projects).is_empty();
    let a = "Actions";
    let mut out = vec![
        Cmd::icon(
            a,
            IconName::Plus,
            "New workspace",
            "⌘N",
            Action::NewWorkspace,
        ),
        Cmd::icon(
            a,
            IconName::HousePlus,
            "New project",
            "",
            Action::NewProject,
        ),
        Cmd::icon(
            a,
            IconName::ClockPlus,
            "New schedule",
            "",
            Action::NewSchedule,
        ),
        Cmd::icon(
            a,
            IconName::Plus,
            "New blank workspace",
            "⇧⌘N",
            Action::NewBlankWorkspace,
        ),
        Cmd::icon(
            a,
            IconName::Folder,
            "Open file or folder",
            "⌥⌘O",
            Action::OpenPath,
        ),
    ];
    if on_detail {
        out.push(Cmd::icon(
            a,
            IconName::QueuePopOut,
            "Open in…",
            "⌘O",
            Action::OpenIn,
        ));
        out.push(Cmd::icon(
            a,
            IconName::Copy,
            "Copy path",
            "",
            Action::CopyPath,
        ));
    }
    if i.editor_file_open {
        out.push(Cmd::icon(
            a,
            IconName::Code,
            "Change language mode",
            "",
            Action::ChangeLanguage,
        ));
    }
    out.push(Cmd::icon(
        a,
        IconName::Command,
        "Run command / snippets…",
        "⌘⇧P",
        Action::RunCommand,
    ));
    out.push(Cmd::icon(
        a,
        IconName::Help,
        "Keyboard shortcuts",
        "⌘⇧/",
        Action::Shortcuts,
    ));
    if can_dev {
        out.push(Cmd::icon(a, IconName::Play, "Run…", "⌘R", Action::Run));
    }
    if can_dev && i.any_run_running {
        out.push(Cmd::icon(
            a,
            IconName::SquareFill,
            "Stop run…",
            "⌘⇧R",
            Action::StopRun,
        ));
    }
    let g = "Go to";
    for (name, label, view) in [
        (IconName::Inbox, "Inbox", ViewName::Briefing),
        (IconName::CircleDot, "Issues", ViewName::Issues),
        (IconName::GitPullRequest, "Pull requests", ViewName::Pr),
        (IconName::Clock, "Schedules", ViewName::Schedules),
        (IconName::Settings, "Settings · policy", ViewName::Settings),
        (IconName::Help, "Get started", ViewName::Welcome),
    ] {
        out.push(Cmd::icon(g, name, label, "", Action::Go(view)));
    }
    for l in i
        .workspaces
        .iter()
        .filter(|l| !l.archived && l.host_pr_id.trim().is_empty())
    {
        let dot = match l.group.as_str() {
            "needs" => Pic::Dot(i.needs),
            "active" => Pic::Dot(i.active),
            "ready" => Pic::Dot(i.ready),
            _ => Pic::None,
        };
        out.push(Cmd {
            g: "Loops".into(),
            label: l.goal.clone(),
            psub: l.id.clone(),
            kb: String::new(),
            pic: dot,
            keys: vec![
                l.goal.clone(),
                l.id.clone(),
                l.short.clone(),
                l.project_id.clone(),
                l.branch.clone(),
            ],
            action: Action::OpenWorkspace(l.id.clone()),
        });
    }
    for p in i.prs {
        out.push(Cmd {
            g: "Pull requests".into(),
            label: p.title.clone(),
            psub: format!("#{}", p.number),
            kb: String::new(),
            pic: Pic::Icon(pr_icon_name(Some(p)), String::new()),
            keys: vec![
                p.title.clone(),
                format!("#{}", p.number),
                p.number.to_string(),
                p.workspace_id.clone(),
                p.branch.clone(),
            ],
            action: Action::OpenPr(p.id.clone()),
        });
    }
    for p in i.projects {
        // `{:else if c.pic}`: a project without a picture leaves the tile empty.
        let pic = if p.pic.is_empty() {
            Pic::None
        } else {
            Pic::Project {
                pic: p.pic.clone(),
                color: p.color.clone(),
                name: p.name.clone(),
            }
        };
        out.push(Cmd {
            g: "Projects".into(),
            label: p.name.clone(),
            psub: String::new(),
            kb: String::new(),
            pic,
            keys: vec![p.name.clone(), p.repo_path.clone()],
            action: Action::OpenProject(p.id.clone()),
        });
    }
    out
}

/// The `[[search]]` groups as rows (`#` or the pack's icon), and their names in order.
pub fn search_rows(res: &PaletteSearchDto) -> (Vec<Cmd>, Vec<String>) {
    let mut rows = Vec::new();
    let mut names = Vec::new();
    for g in res.groups.iter().flatten() {
        let hits = g.hits.as_deref().unwrap_or_default();
        if hits.is_empty() {
            continue;
        }
        names.push(g.name.clone());
        for h in hits {
            let insert = h.insert.trim().to_string();
            let action = h.action.trim().to_string();
            let label = if h.label.is_empty() {
                insert.clone()
            } else {
                h.label.clone()
            };
            let psub = match h.psub.trim() {
                "" => insert.clone(),
                p => p.to_string(),
            };
            rows.push(Cmd {
                g: g.name.clone(),
                label,
                psub,
                kb: String::new(),
                pic: Pic::Icon(IconName::Hash, g.icon_svg.clone()),
                keys: Vec::new(),
                action: Action::SearchHit { insert, action },
            });
        }
    }
    (rows, names)
}

/// `palOrder`.
pub fn pal_order(search_groups: &[String]) -> Vec<String> {
    let mut o = vec!["Actions".to_string(), "Go to".to_string()];
    o.extend(search_groups.iter().cloned());
    o.extend(["Loops", "Pull requests", "Projects"].map(String::from));
    o
}

/// `flat`: the commands matching `q` (label, psub, group or keys) and the search hits, in
/// `palOrder` so the flat index is the rendered order.
pub fn flat(all: &[Cmd], hits: &[Cmd], search_groups: &[String], q: &str) -> Vec<Cmd> {
    let query = q.trim().to_lowercase();
    let has = |s: &str| s.to_lowercase().contains(&query);
    let matched: Vec<&Cmd> = all
        .iter()
        .filter(|c| {
            query.is_empty()
                || has(&c.label)
                || has(&c.psub)
                || has(&c.g)
                || c.keys.iter().any(|k| has(k))
        })
        .collect();
    pal_order(search_groups)
        .iter()
        .flat_map(|g| {
            let pick: Vec<Cmd> = if search_groups.contains(g) {
                hits.iter().filter(|c| c.g == *g).cloned().collect()
            } else {
                matched
                    .iter()
                    .filter(|c| c.g == *g)
                    .map(|c| (*c).clone())
                    .collect()
            };
            pick
        })
        .collect()
}

pub struct CommandPalette {
    state: Entity<AppState>,
    input: Entity<InputState>,
    sel: usize,
    hits: Vec<Cmd>,
    hit_groups: Vec<String>,
    search_gen: u64,
    search: Option<Task<()>>,
    scroll: ScrollHandle,
    /// The selection and list last revealed (`scrollIntoView` after `sel` or `flat` change).
    revealed: Option<(usize, Vec<String>)>,
    was_open: bool,
    _subs: Vec<Subscription>,
}

impl CommandPalette {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx));
        let me = cx.entity().downgrade();
        let subs = vec![
            cx.observe_in(&state, window, |this, _, window, cx| {
                this.sync_open(window, cx)
            }),
            cx.subscribe(&input, |this, _, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.query_changed(cx);
                }
                this.clamp_sel(cx);
                cx.notify();
            }),
            cx.intercept_keystrokes(move |ev, window, cx| {
                let Some(me) = me.upgrade() else { return };
                if me.update(cx, |p, cx| p.on_key(ev, window, cx)) {
                    cx.stop_propagation();
                }
            }),
        ];
        CommandPalette {
            state,
            input,
            sel: 0,
            hits: Vec::new(),
            hit_groups: Vec::new(),
            search_gen: 0,
            search: None,
            scroll: ScrollHandle::new(),
            revealed: None,
            was_open: false,
            _subs: subs,
        }
    }

    fn open(&self, cx: &App) -> bool {
        self.state.read(cx).overlays.palette
    }

    fn close(&self, cx: &mut App) {
        self.state
            .update(cx, |s, cx| s.close_overlay(|o| o.palette = false, cx));
    }

    fn all(&self, cx: &App) -> Vec<Cmd> {
        let s = self.state.read(cx);
        let t = Theme::of(cx).tokens;
        let id = s.selected_workspace_id.as_str();
        all_commands(&Inputs {
            view: s.view,
            selected_workspace_id: id,
            workspaces: &s.workspaces,
            prs: &s.prs,
            projects: &s.projects,
            any_run_running: s.run.is_any_run_running(id),
            editor_file_open: !s.current_editor_focus(cx).1.is_empty(),
            needs: t.coral,
            active: t.amber,
            ready: t.emerald,
        })
    }

    fn flat(&self, cx: &App) -> Vec<Cmd> {
        flat(
            &self.all(cx),
            &self.hits,
            &self.hit_groups,
            &self.input.read(cx).value(),
        )
    }

    fn clamp_sel(&mut self, cx: &App) {
        let n = self.flat(cx).len();
        if self.sel >= n {
            self.sel = n.saturating_sub(1);
        }
    }

    fn sync_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.open(cx);
        if open && !self.was_open {
            self.input.update(cx, |s, cx| s.set_value("", window, cx));
            self.sel = 0;
            let focus = self.input.focus_handle(cx);
            window.defer(cx, move |window, cx| window.focus(&focus, cx));
        }
        if open != self.was_open {
            if !open && self.was_open {
                chrome::release_focus(&self.input, window, cx);
            }
            self.was_open = open;
            self.query_changed(cx);
        }
        self.clamp_sel(cx);
        cx.notify();
    }

    /// The search effect: closed or under two characters clears the hits; otherwise 200ms
    /// after the last edit `SearchPalette` answers unless a newer query superseded it.
    fn query_changed(&mut self, cx: &mut Context<Self>) {
        self.search_gen += 1;
        self.search = None;
        let query = self.input.read(cx).value().trim().to_string();
        if !self.open(cx) || js_len(&query) < 2 {
            self.hits.clear();
            self.hit_groups.clear();
            return;
        }
        let generation = self.search_gen;
        let services = self.state.read(cx).services().clone();
        self.search = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            let res = services.bundle.search_palette(&query).await;
            let _ = this.update(cx, |this, cx| {
                if generation != this.search_gen {
                    return;
                }
                (this.hits, this.hit_groups) = match res {
                    Ok(res) => search_rows(&res),
                    Err(_) => (Vec::new(), Vec::new()),
                };
                this.clamp_sel(cx);
                cx.notify();
            });
        }));
    }

    fn run(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.flat(cx).get(i).cloned() else {
            return;
        };
        // The field hides before the action runs, so what it opens sees no focused field.
        chrome::release_focus(&self.input, window, cx);
        self.close(cx);
        run_action(&self.state, c.action, cx);
    }

    fn on_key(&mut self, ev: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.open(cx) || !self.input.focus_handle(cx).is_focused(window) {
            return false;
        }
        let k = &ev.keystroke;
        if !plain(k) {
            return false;
        }
        match k.key.as_str() {
            "down" | "up" => {
                let n = self.flat(cx).len();
                self.sel = step(self.sel, n, k.key == "down");
                cx.notify();
                true
            }
            "enter" => {
                self.run(self.sel, window, cx);
                true
            }
            "escape" => {
                self.close(cx);
                true
            }
            _ => false,
        }
    }
}

/// `c.run()`.
fn run_action(state: &Entity<AppState>, action: Action, cx: &mut App) {
    state.update(cx, |s, cx| match action {
        Action::NewWorkspace => s.open_new_task("", None, cx),
        Action::NewProject => s.open_new_project(false, cx),
        Action::NewSchedule => s.set_schedule_editor(Some(ScheduleEditorRequest::default()), cx),
        // The shell owns these (`createBlankWorkspace`, `pickAndOpenPath`,
        // `copySelectedWorkspacePath`).
        Action::NewBlankWorkspace => s.dispatch_menu_action("new-blank-workspace", cx),
        Action::OpenPath => s.dispatch_menu_action("open-path", cx),
        Action::CopyPath => s.dispatch_menu_action("copy-path", cx),
        Action::OpenIn => s.close_overlay(|o| o.open_in = true, cx),
        Action::ChangeLanguage => s.open_syntax_mode_picker(cx),
        Action::RunCommand => s.close_overlay(
            |o| {
                o.shortcuts = false;
                o.commands = true;
            },
            cx,
        ),
        Action::Shortcuts => s.close_overlay(|o| o.shortcuts = true, cx),
        Action::Run => {
            let id = s.selected_workspace_id.clone();
            s.open_run_menu(&id, cx);
        }
        Action::StopRun => {
            let id = s.selected_workspace_id.clone();
            s.open_stop_menu(&id, cx);
        }
        Action::Go(v) => s.go(v, None, cx),
        Action::OpenWorkspace(id) => s.open_workspace(&id, false, cx),
        Action::OpenPr(id) => s.open_pr(&id, cx),
        Action::OpenProject(id) => s.open_project(&id, cx),
        Action::SearchHit { insert, action } => pick_search_hit(insert, action, s, cx),
    });
}

/// `pickSearchHit`.
fn pick_search_hit(insert: String, action: String, s: &mut AppState, cx: &mut Context<AppState>) {
    if let Some(section) = action.trim().strip_prefix("settings:") {
        s.open_settings(Some(section), cx);
        return;
    }
    let r = insert.trim().to_string();
    if r.is_empty() {
        return;
    }
    let services = s.services().clone();
    cx.spawn(async move |this, cx| {
        let res = services.bundle.expand_goal(&r).await;
        let _ = this.update(cx, |s, cx| match res {
            Ok(res) => {
                let text = match res.prompt.trim() {
                    "" => r.clone(),
                    p => p.to_string(),
                };
                let draft = NewTaskDraft {
                    text,
                    display: res.display,
                    ..Default::default()
                };
                s.open_new_task("", Some(draft), cx);
            }
            Err(e) => {
                let msg = bridge_error_message(&e);
                let msg = if msg.is_empty() {
                    "Could not expand search hit".to_string()
                } else {
                    msg
                };
                recur_view_core::toast(ToastKind::Error, msg, cx);
            }
        });
    })
    .detach();
}

impl CommandPalette {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        if !self.open(cx) {
            return div().into_any_element();
        }
        style_text_input(&self.input, cx);
        let t = Theme::of(cx).tokens;
        let flat = self.flat(cx);
        let placeholder: SharedString = "Search workspaces, PRs, or projects…".into();
        let head = chrome::head(
            Kind::Palette,
            IconName::Search,
            &self.input,
            &placeholder,
            "esc",
            window,
            cx,
        );
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut sel_child = None;
        if flat.is_empty() {
            rows.push(palette_empty("No matches", false, cx).into_any_element());
        } else {
            for g in pal_order(&self.hit_groups) {
                let items: Vec<(usize, &Cmd)> =
                    flat.iter().enumerate().filter(|(_, c)| c.g == g).collect();
                if items.is_empty() {
                    continue;
                }
                let sel = format!("pal-group:{g}");
                rows.push(
                    palette_group(g.clone(), false, cx)
                        .debug_selector(move || sel)
                        .into_any_element(),
                );
                for (idx, c) in items {
                    let on = idx == self.sel;
                    if on {
                        sel_child = Some(rows.len());
                    }
                    let id = SharedString::from(format!("pal-row:{idx}"));
                    let label = c.label.clone();
                    let tile = match &c.pic {
                        Pic::None => palette_tile(on, cx).into_any_element(),
                        Pic::Dot(color) => palette_tile(on, cx)
                            .child(palette_dot(*color))
                            .into_any_element(),
                        Pic::Project { pic, color, name } => palette_tile(on, cx)
                            .child(project_pic(
                                pic.clone(),
                                color.clone(),
                                name.clone(),
                                PicBox::PALETTE,
                            ))
                            .into_any_element(),
                        Pic::Icon(name, svg) if svg.trim().is_empty() => {
                            palette_pic(*name, on, cx).into_any_element()
                        }
                        Pic::Icon(_, svg) => palette_tile(on, cx)
                            .child(svg_markup(svg.clone()).size(px(14.)).color(if on {
                                t.amber
                            } else {
                                t.muted
                            }))
                            .into_any_element(),
                    };
                    let mut row = palette_row(id.clone(), on, false, cx)
                        .debug_selector(move || {
                            format!("pal-row:{label}{}", if on { ":sel" } else { "" })
                        })
                        .child(tile)
                        .child(chrome::label(
                            SharedString::from(format!("pal-pl:{idx}")),
                            c.label.clone(),
                            Kind::Palette,
                            cx,
                        ));
                    if !c.psub.is_empty() {
                        row = row.child(palette_sub(c.psub.clone(), cx));
                    }
                    if !c.kb.is_empty() {
                        row = row.child(palette_key(c.kb.clone(), on, cx));
                    }
                    rows.push(
                        row.on_mouse_move(cx.guarded_listener(move |p, _, _, cx| {
                            if p.sel != idx {
                                p.sel = idx;
                                cx.notify();
                            }
                        }))
                        .on_click(
                            cx.guarded_listener(move |p, _, window, cx| p.run(idx, window, cx)),
                        )
                        .into_any_element(),
                    );
                }
            }
        }
        let key = (self.sel, flat.iter().map(|c| c.label.clone()).collect());
        if self.revealed.as_ref() != Some(&key) {
            if let Some(ix) = sel_child {
                self.scroll.scroll_to_item(ix);
            }
            self.revealed = Some(key);
        }
        let list = chrome::list("pal-list", Kind::Palette, &self.scroll, rows);
        let foot = chrome::footer(
            Kind::Palette,
            &[("↑↓", "navigate"), ("↵", "open"), ("esc", "close")],
            cx,
        );
        let state = self.state.clone();
        chrome::frame(
            Kind::Palette,
            Kind::Palette.max_vh(),
            "palette",
            vec![
                head.into_any_element(),
                list.into_any_element(),
                foot.into_any_element(),
            ],
            move |_, cx| {
                state.update(cx, |s, cx| s.close_overlay(|o| o.palette = false, cx));
            },
            window,
            cx,
        )
    }
}

impl Render for CommandPalette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use recur_services::api::{PaletteSearchGroupDto, PromptCompleteHitDto};

    fn ws(id: &str, group: &str, goal: &str) -> Workspace {
        serde_json::from_value(serde_json::json!({
            "id": id, "projectId": "storefront", "goal": goal, "short": "", "group": group,
            "branch": format!("recur/{id}"), "worktreePath": format!("/w/{id}"),
        }))
        .unwrap()
    }

    fn inputs<'a>(ws: &'a [Workspace], prs: &'a [Pr], projects: &'a [Project]) -> Inputs<'a> {
        Inputs {
            view: ViewName::Briefing,
            selected_workspace_id: "",
            workspaces: ws,
            prs,
            projects,
            any_run_running: false,
            editor_file_open: false,
            needs: recur_ui::hex(0),
            active: recur_ui::hex(0),
            ready: recur_ui::hex(0),
        }
    }

    fn labels(v: &[Cmd]) -> Vec<&str> {
        v.iter().map(|c| c.label.as_str()).collect()
    }

    #[test]
    fn actions_and_routes_come_first_and_detail_adds_open_in_and_copy_path() {
        let w = vec![ws("LOOP-1", "active", "Ship it")];
        let all = all_commands(&inputs(&w, &[], &[]));
        assert_eq!(
            labels(&all)[..7],
            [
                "New workspace",
                "New project",
                "New schedule",
                "New blank workspace",
                "Open file or folder",
                "Run command / snippets…",
                "Keyboard shortcuts"
            ]
        );
        let mut i = inputs(&w, &[], &[]);
        i.view = ViewName::Detail;
        i.selected_workspace_id = "LOOP-1";
        i.editor_file_open = true;
        let all = all_commands(&i);
        let l = labels(&all);
        assert!(l.contains(&"Open in…") && l.contains(&"Copy path"));
        assert!(l.contains(&"Change language mode"));
        // No run command on the project: no Run… or Stop run….
        assert!(!l.contains(&"Run…") && !l.contains(&"Stop run…"));
    }

    #[test]
    fn hides_archived_and_pr_host_workspaces_and_colors_known_groups() {
        let mut archived = ws("A", "active", "Old");
        archived.archived = true;
        let mut host = ws("H", "active", "Host");
        host.host_pr_id = "482".into();
        let w = vec![
            ws("N", "needs", "Needs"),
            archived,
            host,
            ws("X", "", "Odd"),
        ];
        let mut i = inputs(&w, &[], &[]);
        i.needs = recur_ui::hex(0xff0000);
        let loops: Vec<Cmd> = all_commands(&i)
            .into_iter()
            .filter(|c| c.g == "Loops")
            .collect();
        assert_eq!(labels(&loops), ["Needs", "Odd"]);
        assert_eq!(loops[0].pic, Pic::Dot(recur_ui::hex(0xff0000)));
        assert_eq!(loops[1].pic, Pic::None);
        assert_eq!(loops[0].psub, "N");
    }

    #[test]
    fn matches_a_loop_by_project_id_and_a_pr_by_number() {
        let w = vec![ws("LOOP-1", "active", "Ship it")];
        let pr: Pr = serde_json::from_value(serde_json::json!({
            "id": "482", "number": 482, "title": "Add OAuth", "branch": "b", "workspaceId": "",
        }))
        .unwrap();
        let prs = vec![pr];
        let all = all_commands(&inputs(&w, &prs, &[]));
        assert_eq!(labels(&flat(&all, &[], &[], "storefront")), ["Ship it"]);
        assert_eq!(labels(&flat(&all, &[], &[], "#482")), ["Add OAuth"]);
        let goto = flat(&all, &[], &[], "go to");
        assert!(goto.iter().all(|c| c.g == "Go to") && goto.len() == 6);
        assert!(flat(&all, &[], &[], "zzz").is_empty());
    }

    #[test]
    fn search_groups_sit_between_go_to_and_loops() {
        let res = PaletteSearchDto {
            groups: Some(vec![
                PaletteSearchGroupDto {
                    name: "Linear".into(),
                    icon_svg: "<svg/>".into(),
                    hits: Some(vec![PromptCompleteHitDto {
                        insert: " ENG-1 ".into(),
                        label: String::new(),
                        ..Default::default()
                    }]),
                },
                PaletteSearchGroupDto {
                    name: "Empty".into(),
                    icon_svg: String::new(),
                    hits: Some(Vec::new()),
                },
            ]),
        };
        let (hits, names) = search_rows(&res);
        assert_eq!(names, ["Linear"]);
        assert_eq!(hits[0].label, "ENG-1");
        assert_eq!(hits[0].psub, "ENG-1");
        assert_eq!(hits[0].pic, Pic::Icon(IconName::Hash, "<svg/>".into()));
        let w = vec![ws("LOOP-1", "active", "eng work")];
        let all = all_commands(&inputs(&w, &[], &[]));
        let f = flat(&all, &hits, &names, "eng");
        assert_eq!(labels(&f), ["ENG-1", "eng work"]);
        assert_eq!(
            pal_order(&names),
            [
                "Actions",
                "Go to",
                "Linear",
                "Loops",
                "Pull requests",
                "Projects"
            ]
        );
    }
}
