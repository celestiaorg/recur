//! `Project.svelte` (the `project` route): one project's settings in the Settings layout. A
//! local copy of the project is edited and autosaved 450ms after it settles (run settings go
//! to `.recur/project.toml`); skills and MCP overrides save at once.

mod sections;

use recur_ui::guard::Guarded;
use recur_view_core::{ObserveState as _, Topics};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::base::input::TextareaState;
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement,
    ParentElement, Render, ScrollHandle, SharedString, Styled, Subscription, Task, Window, div,
};
use recur_code::view::skills::{SkillKind, SkillRow, map_skill_rows, skill_kind_has_overrides};
use recur_domain::{BranchChoice, Project, RunCommand};
use recur_ui::IconName;
use recur_ui::field::{Field, FieldEvent, FieldOpts};
use recur_ui::set::{NavEntry, nav_panel, page_head, page_main};
use recur_ui::widgets::{IconPicked, IconPicker, Select, SelectEvent, ToastKind, ToastOptions};
use recur_view_core::run::project_runs;
use recur_view_core::{ViewContext, ViewName, call, toast};

use recur_view_core::mini::{MiniChange, MiniLang, MiniProps, MiniScriptEditor};
use recur_view_widgets::schedules::ScheduledPrompts;

/// A section of the project page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    General,
    Schedules,
    Run,
    Prs,
    Skills,
    Mcp,
    Danger,
}

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Schedules => "Schedules",
            Section::Run => "Run",
            Section::Prs => "Pull requests",
            Section::Skills => "Skills",
            Section::Mcp => "MCP",
            Section::Danger => "Danger zone",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Section::General => IconName::SlidersHorizontal,
            Section::Schedules => IconName::Clock,
            Section::Run => IconName::Play,
            Section::Prs => IconName::GitPullRequest,
            Section::Skills => IconName::Slash,
            Section::Mcp => IconName::Plug,
            Section::Danger => IconName::Trash,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Section::General => "general",
            Section::Schedules => "schedules",
            Section::Run => "run",
            Section::Prs => "prs",
            Section::Skills => "skills",
            Section::Mcp => "mcp",
            Section::Danger => "danger",
        }
    }

    pub fn from_key(k: &str) -> Option<Section> {
        [
            Section::General,
            Section::Schedules,
            Section::Run,
            Section::Prs,
            Section::Skills,
            Section::Mcp,
            Section::Danger,
        ]
        .into_iter()
        .find(|s| s.key() == k)
    }

    fn keywords(self) -> &'static str {
        match self {
            Section::General => {
                "name avatar pic color icon identity repository path git repo checkout worktree default branch group sidebar issues linear github"
            }
            Section::Run => {
                "setup teardown script app url env environment verify browser project.toml copy files glob dotenv workspace"
            }
            Section::Schedules => "schedule automation prompt daily weekdays recurring timer",
            Section::Prs => {
                "github merge squash rebase update base delete branch commit message title description"
            }
            Section::Skills => "skills slash recur user repository override",
            Section::Mcp => "mcp linear tools http stdio server override",
            Section::Danger => "delete remove wipe project",
        }
    }
}

/// `nav` filtered by `query` (`filteredNav`): groups in order, items matching the label or a
/// keyword; Danger only for a saved project.
pub fn filtered_nav(query: &str, is_new: bool) -> Vec<(&'static str, Vec<Section>)> {
    let mut groups: Vec<(&'static str, Vec<Section>)> = vec![(
        "Project",
        vec![
            Section::General,
            Section::Run,
            Section::Schedules,
            Section::Prs,
            Section::Skills,
            Section::Mcp,
        ],
    )];
    if !is_new {
        groups.push(("Danger", vec![Section::Danger]));
    }
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return groups;
    }
    groups
        .into_iter()
        .map(|(g, items)| {
            let items = items
                .into_iter()
                .filter(|s| s.title().to_lowercase().contains(&q) || s.keywords().contains(&q))
                .collect::<Vec<_>>();
            (g, items)
        })
        .filter(|(_, items)| !items.is_empty())
        .collect()
}

/// `branchOptions`: the branches, with a configured base surfaced even if its ref is gone.
pub fn branch_options(choices: &[BranchChoice], cur: &str) -> Vec<(String, String)> {
    let mut opts: Vec<(String, String)> = choices
        .iter()
        .map(|b| (b.name.clone(), b.group.clone()))
        .collect();
    let cur = cur.trim();
    if !cur.is_empty() && !opts.iter().any(|(v, _)| v == cur) {
        let remote = cur
            .find('/')
            .filter(|i| *i > 0)
            .map(|i| &cur[..i])
            .unwrap_or("");
        let group = if !remote.is_empty() && opts.iter().any(|(_, g)| g == remote) {
            remote.to_string()
        } else {
            "Local".to_string()
        };
        let at = if group == "Local" {
            opts.iter()
                .position(|(_, g)| g == "Local")
                .unwrap_or(opts.len())
        } else {
            opts.iter()
                .position(|(_, g)| *g == group)
                .or_else(|| opts.iter().position(|(_, g)| g == "Local"))
                .unwrap_or(opts.len())
        };
        opts.insert(at, (cur.to_string(), group));
    }
    if opts.is_empty() && !cur.is_empty() {
        return vec![(cur.to_string(), "Local".into())];
    }
    opts
}

/// `copyFilesMeta`.
pub fn copy_files_meta(p: &CopyPreview, patterns: &str, repo: &str, is_new: bool) -> String {
    if p.loading {
        return "Scanning…".into();
    }
    if !p.error.is_empty() {
        return p.error.clone();
    }
    if patterns.trim().is_empty() {
        return "Nothing will be copied".into();
    }
    if is_new {
        return "Preview available after the project is created".into();
    }
    let root = if p.root.is_empty() {
        repo.trim().to_string()
    } else {
        p.root.clone()
    };
    if p.count == 0 {
        return if root.is_empty() {
            "No matching files".into()
        } else {
            format!("No matching files in {root}")
        };
    }
    let noun = if p.count == 1 { "file" } else { "files" };
    if root.is_empty() {
        format!("{} {noun} will be copied", p.count)
    } else {
        format!("{} {noun} will be copied from {root}", p.count)
    }
}

/// The Files to copy scan (`copyPreview`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CopyPreview {
    pub root: String,
    pub files: Vec<String>,
    pub count: i64,
    pub truncated: bool,
    pub error: String,
    pub loading: bool,
}

/// One named run command row.
pub(crate) struct RunRow {
    pub id: u64,
    pub editor: Entity<MiniScriptEditor>,
    pub cmd: String,
}

pub struct ProjectView {
    pub(crate) vcx: ViewContext,
    focus: FocusHandle,
    nav_scroll: ScrollHandle,
    main_scroll: ScrollHandle,
    pub(crate) fields: HashMap<String, Field>,
    pub(crate) copy_files: Entity<TextareaState>,
    pub(crate) form: Project,
    last_form_id: Option<String>,
    pub(crate) saving: bool,
    pub(crate) deleting: bool,
    pub(crate) confirming_delete: bool,
    pub(crate) picking: bool,
    pub(crate) branch_choices: Vec<BranchChoice>,
    branch_key: Option<(String, String)>,
    branch_task: Option<Task<()>>,
    pub(crate) skills: Vec<SkillRow>,
    pub(crate) skills_loading: bool,
    pub(crate) skills_err: String,
    pub(crate) saving_skills: bool,
    skills_loaded_key: String,
    skills_task: Option<(String, Task<()>)>,
    pub(crate) override_recur: bool,
    pub(crate) override_user: bool,
    pub(crate) run_rows: Vec<RunRow>,
    run_seq: u64,
    pub(crate) setup: Option<Entity<MiniScriptEditor>>,
    pub(crate) teardown: Option<Entity<MiniScriptEditor>>,
    pub(crate) env: Option<Entity<MiniScriptEditor>>,
    persist_snap: String,
    persist_pending: bool,
    persist_timer: Option<Task<()>>,
    pub(crate) section: Section,
    pub(crate) copy_preview: CopyPreview,
    copy_key: Option<(String, String, u64, String)>,
    copy_task: Option<Task<()>>,
    pub(crate) copy_tick: u64,
    pub(crate) branch_sel: Entity<Select>,
    pub(crate) group_sel: Entity<Select>,
    pub(crate) icon_picker: Entity<IconPicker>,
    pub(crate) schedules: Option<(String, Entity<ScheduledPrompts>)>,
    _subs: Vec<Subscription>,
}

impl Focusable for ProjectView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Field keys.
pub(crate) const NAV: &str = "nav-search";
pub(crate) const NAME: &str = "name";
pub(crate) const LETTER: &str = "letter";
pub(crate) const REPO: &str = "repo";
pub(crate) const LINEAR: &str = "linear";
pub(crate) const APP_URL: &str = "app-url";

/// Undoes view state when an override save fails.
pub(crate) type OnFail = Rc<dyn Fn(&mut ProjectView)>;

pub(crate) fn run_name_key(id: u64) -> String {
    format!("run-name-{id}")
}

impl ProjectView {
    pub fn new(vcx: &ViewContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let copy_files =
            cx.new(|cx| TextareaState::new(window, cx).placeholder(COPY_FILES_PLACEHOLDER));
        let branch_sel = cx.new(|cx| {
            Select::new(Vec::new(), window, cx)
                .label("Default branch")
                .searchable()
                .trigger_icon(IconName::GitBranch)
        });
        let group_sel = cx.new(|cx| {
            Select::new(Vec::new(), window, cx)
                .label("Project group")
                .trigger_icon(IconName::Folder)
        });
        let icon_picker = cx.new(|cx| IconPicker::new("box", window, cx));
        let subs = vec![
            cx.observe_state_in(&vcx.state, Topics::ALL, window, |this, _, window, cx| {
                this.sync(window, cx)
            }),
            cx.guarded_subscribe_in(&copy_files, window, |this, area, ev, window, cx| {
                if matches!(ev, gpui_kit::base::input::InputEvent::Change) {
                    this.form.copy_files = area.read(cx).value().to_string();
                    this.changed(window, cx);
                }
            }),
            cx.guarded_subscribe_in(
                &branch_sel,
                window,
                |this, _, ev: &SelectEvent, window, cx| {
                    if let SelectEvent::Change(v) = ev {
                        this.form.base_branch = v.to_string();
                        this.changed(window, cx);
                    }
                },
            ),
            cx.guarded_subscribe_in(
                &group_sel,
                window,
                |this, _, ev: &SelectEvent, window, cx| {
                    if let SelectEvent::Change(v) = ev {
                        this.form.group_id = v.to_string();
                        this.changed(window, cx);
                    }
                },
            ),
            cx.guarded_subscribe_in(
                &icon_picker,
                window,
                |this, _, ev: &IconPicked, window, cx| {
                    this.form.pic = recur_ui::widgets::project_pic::format_lucide_pic(&ev.0);
                    this.changed(window, cx);
                },
            ),
        ];
        let mut v = ProjectView {
            vcx: vcx.clone(),
            focus: cx.focus_handle(),
            nav_scroll: ScrollHandle::new(),
            main_scroll: ScrollHandle::new(),
            fields: HashMap::new(),
            copy_files,
            form: Project::default(),
            last_form_id: None,
            saving: false,
            deleting: false,
            confirming_delete: false,
            picking: false,
            branch_choices: Vec::new(),
            branch_key: None,
            branch_task: None,
            skills: Vec::new(),
            skills_loading: false,
            skills_err: String::new(),
            saving_skills: false,
            skills_loaded_key: String::new(),
            skills_task: None,
            override_recur: false,
            override_user: false,
            run_rows: Vec::new(),
            run_seq: 0,
            setup: None,
            teardown: None,
            env: None,
            persist_snap: String::new(),
            persist_pending: false,
            persist_timer: None,
            section: Section::General,
            copy_preview: CopyPreview::default(),
            copy_key: None,
            copy_task: None,
            copy_tick: 0,
            branch_sel,
            group_sel,
            icon_picker,
            schedules: None,
            _subs: subs,
        };
        v.sync(window, cx);
        v
    }

    // ---- derived ----

    /// The project being edited (`selected`), from the live store.
    pub(crate) fn selected(&self, cx: &App) -> Option<Project> {
        let s = self.vcx.state.read(cx);
        s.project(&s.project_form_id).cloned()
    }

    pub(crate) fn is_new(&self, cx: &App) -> bool {
        self.selected(cx).is_none()
    }

    pub(crate) fn can_save(&self) -> bool {
        !self.form.repo_path.trim().is_empty()
    }

    pub(crate) fn field_value(&self, key: &str, cx: &App) -> String {
        self.fields
            .get(key)
            .map(|f| f.value(cx))
            .unwrap_or_default()
    }

    /// The field for `key`, created on first use.
    pub(crate) fn field(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<gpui_kit::base::input::InputState> {
        if !self.fields.contains_key(key) {
            let f = Field::new(
                key.to_string(),
                FieldOpts::default(),
                Self::field_event,
                window,
                cx,
            );
            self.fields.insert(key.to_string(), f);
        }
        let f = self.fields.get_mut(key).expect("just inserted");
        f.flush(window, cx);
        recur_ui::style::style_text_input(&f.input, cx);
        f.input.clone()
    }

    fn set_field(&mut self, key: &str, v: &str, window: &mut Window, cx: &mut Context<Self>) {
        let _ = self.field(key, window, cx);
        if let Some(f) = self.fields.get_mut(key) {
            f.set(v, window, cx);
        }
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
        let v = self.field_value(key, cx);
        match key {
            NAV => {
                // `section` follows the filter when it hides the current one.
                let ids: Vec<Section> = filtered_nav(&v, self.is_new(cx))
                    .into_iter()
                    .flat_map(|(_, items)| items)
                    .collect();
                if let Some(first) = ids.first()
                    && !ids.contains(&self.section)
                {
                    self.section = *first;
                }
                cx.notify();
                return;
            }
            NAME => self.set_name(&v),
            LETTER => self.form.pic = recur_ui::widgets::project_pic::project_pic_from_name(&v),
            REPO => self.form.repo_path = v,
            LINEAR => self.form.linear_projects = v,
            APP_URL => self.form.app_url = v,
            k if k.starts_with("run-name-") => {}
            _ => return,
        }
        self.changed(window, cx);
    }

    /// `setName`: a letter picture follows the name.
    fn set_name(&mut self, next: &str) {
        let prev = recur_ui::widgets::project_pic::project_pic_from_name(&self.form.name);
        self.form.name = next.to_string();
        if self.form.pic.is_empty() || self.form.pic == prev {
            self.form.pic = recur_ui::widgets::project_pic::project_pic_from_name(next);
        }
    }

    // ---- effects ----

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (view, fid) = {
            let s = self.vcx.state.read(cx);
            (s.view, s.project_form_id.clone())
        };
        if view == ViewName::Project && fid.is_empty() {
            self.vcx
                .state
                .update(cx, |s, cx| s.go(ViewName::Briefing, None, cx));
            return;
        }
        if self.last_form_id.as_deref() != Some(&fid) {
            self.last_form_id = Some(fid.clone());
            self.reseed(window, cx);
            if !fid.is_empty() {
                // Refresh isGit when focusing a project (`git init` outside Recur).
                self.vcx
                    .state
                    .update(cx, |s, cx| s.refresh_projects(cx))
                    .detach();
            }
        }
        self.skills_effect(cx);
        self.branches_effect(cx);
        self.copy_effect(cx);
        self.persist_effect(cx);
        cx.notify();
    }

    /// The form reseeded for a newly targeted project.
    fn reseed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.selected(cx);
        self.form = selected.clone().unwrap_or_default();
        let seeded = project_runs(selected.as_ref());
        self.run_rows.clear();
        if seeded.is_empty() {
            self.add_run_row("dev", "", window, cx);
        } else {
            for r in seeded {
                self.add_run_row(&r.name, &r.cmd, window, cx);
            }
        }
        self.confirming_delete = false;
        self.persist_snap.clear();
        self.persist_pending = false;
        self.persist_timer = None;
        self.skills_loaded_key.clear();
        self.override_recur = false;
        self.override_user = false;
        self.section = Section::General;
        let f = self.form.clone();
        self.set_field(NAV, "", window, cx);
        self.set_field(NAME, &f.name, window, cx);
        self.set_field(REPO, &f.repo_path, window, cx);
        self.set_field(LINEAR, &f.linear_projects, window, cx);
        self.set_field(APP_URL, &f.app_url, window, cx);
        let letter = match recur_ui::widgets::project_pic::parse_project_pic(&self.ident_pic(), "?")
        {
            recur_ui::widgets::project_pic::ParsedPic::Letter(t) => t,
            _ => String::new(),
        };
        self.set_field(LETTER, &letter, window, cx);
        let cf = f.copy_files.clone();
        self.copy_files
            .update(cx, |a, cx| a.set_value(cf, window, cx));
        for (slot, value, lang, ph, min, max) in [
            (
                0,
                f.setup_cmd.clone(),
                MiniLang::Shell,
                "# runs once when the worktree is created\npnpm install",
                4,
                14,
            ),
            (
                1,
                f.teardown_cmd.clone(),
                MiniLang::Shell,
                "# runs once on remove\nrm -rf \"$HOME/Library/Application Support/Recur-$RECUR_WORKSPACE_ID\"",
                3,
                12,
            ),
            (
                2,
                f.env.clone(),
                MiniLang::Env,
                "# injected into setup, teardown, dev, and terminals\nVITE_PORT=$RECUR_PORT\nRECUR_DATA_DIR=$HOME/Library/Application Support/Recur-$RECUR_WORKSPACE_ID",
                4,
                14,
            ),
        ] {
            let existing = match slot {
                0 => self.setup.clone(),
                1 => self.teardown.clone(),
                _ => self.env.clone(),
            };
            match existing {
                Some(e) => e.update(cx, |e, cx| e.set_value(&value, window, cx)),
                None => {
                    let state = self.vcx.state.clone();
                    let e = cx.new(|cx| {
                        MiniScriptEditor::new(
                            &state,
                            &value,
                            MiniProps {
                                lang,
                                placeholder: ph.into(),
                                min_lines: min,
                                max_lines: max,
                                ..Default::default()
                            },
                            window,
                            cx,
                        )
                    });
                    let sub = cx.guarded_subscribe_in(
                        &e,
                        window,
                        move |this, _, ev: &MiniChange, window, cx| {
                            match slot {
                                0 => this.form.setup_cmd = ev.0.clone(),
                                1 => this.form.teardown_cmd = ev.0.clone(),
                                _ => this.form.env = ev.0.clone(),
                            }
                            this.changed(window, cx);
                        },
                    );
                    self._subs.push(sub);
                    match slot {
                        0 => self.setup = Some(e),
                        1 => self.teardown = Some(e),
                        _ => self.env = Some(e),
                    }
                }
            }
        }
    }

    pub(crate) fn add_run_row(
        &mut self,
        name: &str,
        cmd: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_seq += 1;
        let id = self.run_seq;
        let state = self.vcx.state.clone();
        let editor = cx.new(|cx| {
            MiniScriptEditor::new(
                &state,
                cmd,
                MiniProps {
                    lang: MiniLang::Shell,
                    placeholder: "pnpm dev --port $RECUR_PORT".into(),
                    min_lines: 1,
                    max_lines: 10,
                    ..Default::default()
                },
                window,
                cx,
            )
        });
        let sub = cx.guarded_subscribe_in(
            &editor,
            window,
            move |this, _, ev: &MiniChange, window, cx| {
                if let Some(r) = this.run_rows.iter_mut().find(|r| r.id == id) {
                    r.cmd = ev.0.clone();
                }
                this.changed(window, cx);
            },
        );
        self._subs.push(sub);
        self.set_field(&run_name_key(id), name, window, cx);
        self.run_rows.push(RunRow {
            id,
            editor,
            cmd: cmd.to_string(),
        });
    }

    pub(crate) fn remove_run_row(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.run_rows.retain(|r| r.id != id);
        self.fields.remove(&run_name_key(id));
        if self.run_rows.is_empty() {
            self.add_run_row("dev", "", window, cx);
        }
        self.changed(window, cx);
    }

    /// The rows as `{name, cmd}`.
    pub(crate) fn runs(&self, cx: &App) -> Vec<RunCommand> {
        self.run_rows
            .iter()
            .map(|r| RunCommand {
                name: self.field_value(&run_name_key(r.id), cx),
                cmd: r.cmd.clone(),
            })
            .collect()
    }

    /// Something in the form changed: re-run the autosave and preview effects.
    pub(crate) fn changed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.touch(cx);
    }

    pub(crate) fn touch(&mut self, cx: &mut Context<Self>) {
        self.copy_effect(cx);
        self.persist_effect(cx);
        cx.notify();
    }

    fn skills_effect(&mut self, cx: &mut Context<Self>) {
        let id = self.vcx.state.read(cx).project_form_id.clone();
        let repo = self
            .selected(cx)
            .map(|p| p.repo_path.trim().to_string())
            .unwrap_or_default();
        let key = format!("{id}\0{repo}");
        if id.is_empty() {
            self.skills_loaded_key.clear();
            self.skills.clear();
            self.skills_err.clear();
            self.skills_loading = false;
            self.skills_task = None;
            return;
        }
        if key == self.skills_loaded_key
            || self.skills_task.as_ref().is_some_and(|(k, _)| *k == key)
        {
            return;
        }
        self.skills_loading = true;
        self.skills_err.clear();
        let services = self.vcx.services.clone();
        let k = key.clone();
        let task = cx.spawn(async move |this, cx| {
            let r = services.code.list_all_skills(&id, "").await;
            let _ = this.update(cx, |v, cx| {
                match r {
                    Ok(list) => {
                        let rows = map_skill_rows(&list);
                        v.override_recur = skill_kind_has_overrides(
                            &rows,
                            SkillKind::Recur,
                            &v.form.disabled_skills,
                            &v.form.enabled_skills,
                        );
                        v.override_user = skill_kind_has_overrides(
                            &rows,
                            SkillKind::User,
                            &v.form.disabled_skills,
                            &v.form.enabled_skills,
                        );
                        v.skills = rows;
                    }
                    Err(e) => {
                        v.skills.clear();
                        v.skills_err = e.message.clone();
                    }
                }
                v.skills_loaded_key = k;
                v.skills_loading = false;
                v.skills_task = None;
                cx.notify();
            });
        });
        self.skills_task = Some((key, task));
    }

    fn branches_effect(&mut self, cx: &mut Context<Self>) {
        let id = self.vcx.state.read(cx).project_form_id.clone();
        let repo = self.selected(cx).map(|p| p.repo_path).unwrap_or_default();
        let key = (id.clone(), repo);
        if self.branch_key.as_ref() == Some(&key) {
            return;
        }
        self.branch_key = Some(key);
        if id.is_empty() {
            self.branch_choices.clear();
            self.branch_task = None;
            return;
        }
        let services = self.vcx.services.clone();
        self.branch_task = Some(cx.spawn(async move |this, cx| {
            let r = services.projects.list_branches(&id).await;
            let _ = this.update(cx, |v, cx| {
                v.branch_choices = r.unwrap_or_default();
                cx.notify();
            });
        }));
    }

    /// The Files to copy preview, 280ms after the patterns settle, while Run shows.
    fn copy_effect(&mut self, cx: &mut Context<Self>) {
        if self.section != Section::Run {
            self.copy_key = None;
            self.copy_task = None;
            return;
        }
        let id = self.vcx.state.read(cx).project_form_id.clone();
        let repo = self
            .selected(cx)
            .map(|p| p.repo_path.trim().to_string())
            .unwrap_or_default();
        let patterns = self.form.copy_files.clone();
        let key = (id.clone(), patterns.clone(), self.copy_tick, repo);
        if self.copy_key.as_ref() == Some(&key) || id.is_empty() {
            return;
        }
        self.copy_key = Some(key);
        let services = self.vcx.services.clone();
        self.copy_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(280))
                .await;
            if patterns.trim().is_empty() {
                let _ = this.update(cx, |v, cx| {
                    v.copy_preview = CopyPreview {
                        root: v.form.repo_path.trim().to_string(),
                        ..Default::default()
                    };
                    cx.notify();
                });
                return;
            }
            let _ = this.update(cx, |v, cx| {
                v.copy_preview.loading = true;
                cx.notify();
            });
            let r = services.projects.preview_copy_files(&id, &patterns).await;
            let _ = this.update(cx, |v, cx| {
                v.copy_preview = match r {
                    Ok(got) => CopyPreview {
                        root: got.root,
                        files: got.files.unwrap_or_default(),
                        count: got.count,
                        truncated: got.truncated,
                        error: got.error,
                        loading: false,
                    },
                    Err(e) => CopyPreview {
                        error: e.message.clone(),
                        ..Default::default()
                    },
                };
                cx.notify();
            });
        }));
    }

    // ---- persistence ----

    /// `persistSnapshot`: what autosave compares.
    pub(crate) fn snapshot(&self, cx: &App) -> String {
        let f = &self.form;
        let runs: Vec<serde_json::Value> = self
            .runs(cx)
            .into_iter()
            .map(|r| serde_json::json!({ "name": r.name, "cmd": r.cmd }))
            .collect();
        serde_json::json!({
            "repoPath": f.repo_path, "name": f.name, "pic": f.pic, "color": f.color,
            "baseBranch": f.base_branch, "groupId": f.group_id, "setupCmd": f.setup_cmd,
            "teardownCmd": f.teardown_cmd, "appURL": f.app_url, "env": f.env,
            "copyFiles": f.copy_files, "prOnGithub": f.pr_on_github,
            "deleteBranchOnRemove": f.delete_branch_on_remove,
            "linearProjects": f.linear_projects, "mergeMethod": f.merge_method.as_str(),
            "updateBaseMethod": f.update_base_method.as_str(),
            "squashCommitMessage": f.squash_commit_message.as_str(),
            "mergeCommitMessage": f.merge_commit_message.as_str(),
            "runs": runs,
        })
        .to_string()
    }

    /// The autosave effect: 450ms after the snapshot settles, for a saved project.
    fn persist_effect(&mut self, cx: &mut Context<Self>) {
        if self.is_new(cx) {
            return;
        }
        let snap = self.snapshot(cx);
        if self.persist_snap.is_empty() {
            self.persist_snap = snap;
            return;
        }
        if snap == self.persist_snap || !self.can_save() {
            self.persist_timer = None;
            return;
        }
        self.persist_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(450))
                .await;
            let _ = this.update(cx, |v, cx| v.persist(cx));
        }));
    }

    /// `runConfigDirty`: the run settings differ from the last saved project.
    fn run_config_dirty(&self, runs: &[RunCommand], cx: &App) -> bool {
        let sel = self.selected(cx).unwrap_or_default();
        let f = &self.form;
        if f.repo_path.trim() != sel.repo_path.trim()
            || f.setup_cmd != sel.setup_cmd
            || f.teardown_cmd != sel.teardown_cmd
            || f.app_url != sel.app_url
            || f.env != sel.env
            || f.copy_files != sel.copy_files
        {
            return true;
        }
        let prev = project_runs(Some(&sel));
        let next = project_runs(Some(&Project {
            run_commands: runs.to_vec(),
            ..Default::default()
        }));
        prev.len() != next.len()
            || prev
                .iter()
                .zip(&next)
                .any(|(a, b)| a.name != b.name || a.cmd != b.cmd)
    }

    /// `persist`: save until the form stops changing under the request.
    pub(crate) fn persist(&mut self, cx: &mut Context<Self>) {
        if !self.can_save() {
            return;
        }
        if self.saving {
            self.persist_pending = true;
            return;
        }
        let snap = self.snapshot(cx);
        if snap == self.persist_snap {
            return;
        }
        self.saving = true;
        self.persist_pending = false;
        let runs: Vec<RunCommand> = self
            .runs(cx)
            .into_iter()
            .map(|r| RunCommand {
                name: r.name.trim().to_string(),
                cmd: r.cmd.trim().to_string(),
            })
            .filter(|r| !r.cmd.is_empty())
            .collect();
        let selected = self.selected(cx).unwrap_or_default();
        let mut payload = self.form.clone();
        payload.id = if selected.id.is_empty() {
            payload.id
        } else {
            selected.id.clone()
        };
        payload.hidden = selected.hidden;
        payload.write_project_file = self.run_config_dirty(&runs, cx);
        payload.run_commands = runs;
        let services = self.vcx.services.clone();
        let state = self.vcx.state.clone();
        cx.spawn(async move |this, cx| {
            let saved = call(
                cx,
                services.projects.update_project_run_config(&payload),
                "Save failed",
            )
            .await;
            let _ = this.update(cx, |v, cx| {
                v.saving = false;
                let Some(saved) = saved else {
                    cx.notify();
                    return;
                };
                v.persist_snap = snap;
                let id = saved.id.clone();
                state.update(cx, |s, cx| {
                    s.upsert_project(saved, cx);
                    if !id.is_empty() {
                        s.set_project_form_id(&id, cx);
                    }
                });
                v.last_form_id = Some(id);
                if v.persist_pending || v.snapshot(cx) != v.persist_snap {
                    v.persist(cx);
                    return;
                }
                recur_ui::widgets::push_toast(
                    ToastKind::Success,
                    "Project saved",
                    ToastOptions {
                        replace: Some("project-saved".into()),
                        ..Default::default()
                    },
                    cx,
                );
                cx.notify();
            });
        })
        .detach();
    }

    /// `persistSkillLists` / `persistMCPLists`: save only the override lists over the last
    /// saved project, rolling back on failure.
    pub(crate) fn persist_overrides(
        &mut self,
        mcp: bool,
        disabled: Vec<String>,
        enabled: Vec<String>,
        on_fail: Option<OnFail>,
        cx: &mut Context<Self>,
    ) {
        let Some(sel) = self.selected(cx) else { return };
        if self.saving_skills {
            return;
        }
        let prev = if mcp {
            (
                self.form.disabled_mcp.clone(),
                self.form.enabled_mcp.clone(),
            )
        } else {
            (
                self.form.disabled_skills.clone(),
                self.form.enabled_skills.clone(),
            )
        };
        let mut payload = sel;
        if mcp {
            self.form.disabled_mcp = disabled.clone();
            self.form.enabled_mcp = enabled.clone();
            payload.disabled_mcp = disabled;
            payload.enabled_mcp = enabled;
        } else {
            self.form.disabled_skills = disabled.clone();
            self.form.enabled_skills = enabled.clone();
            payload.disabled_skills = disabled;
            payload.enabled_skills = enabled;
        }
        self.saving_skills = true;
        cx.notify();
        let services = self.vcx.services.clone();
        let state = self.vcx.state.clone();
        let fail = if mcp {
            "Could not save MCP"
        } else {
            "Could not save skills"
        };
        cx.spawn(async move |this, cx| {
            let saved = call(
                cx,
                services.projects.update_project_run_config(&payload),
                fail,
            )
            .await;
            let _ = this.update(cx, |v, cx| {
                v.saving_skills = false;
                match saved {
                    Some(saved) => {
                        if mcp {
                            v.form.disabled_mcp = saved.disabled_mcp.clone();
                            v.form.enabled_mcp = saved.enabled_mcp.clone();
                        } else {
                            v.form.disabled_skills = saved.disabled_skills.clone();
                            v.form.enabled_skills = saved.enabled_skills.clone();
                        }
                        let id = saved.id.clone();
                        state.update(cx, |s, cx| {
                            s.update_project(
                                &id,
                                |p| {
                                    p.disabled_skills = saved.disabled_skills.clone();
                                    p.enabled_skills = saved.enabled_skills.clone();
                                    p.disabled_mcp = saved.disabled_mcp.clone();
                                    p.enabled_mcp = saved.enabled_mcp.clone();
                                },
                                cx,
                            )
                        });
                    }
                    None => {
                        if mcp {
                            (v.form.disabled_mcp, v.form.enabled_mcp) = prev;
                        } else {
                            (v.form.disabled_skills, v.form.enabled_skills) = prev;
                        }
                        if let Some(f) = &on_fail {
                            f(v);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// `pickRepo`.
    pub(crate) fn pick_repo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picking {
            return;
        }
        self.picking = true;
        cx.notify();
        let rx = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Select".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let picked = rx.await.ok().and_then(Result::ok).flatten();
            let _ = this.update_in(cx, |v, window, cx| {
                v.picking = false;
                if let Some(p) = picked.and_then(|x| x.into_iter().next()) {
                    let p = p.to_string_lossy().to_string();
                    if !p.is_empty() {
                        v.form.repo_path = p.clone();
                        v.set_field(REPO, &p, window, cx);
                        v.changed(window, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// `deleteProject`.
    pub(crate) fn delete_project(&mut self, cx: &mut Context<Self>) {
        let Some(sel) = self.selected(cx) else { return };
        self.deleting = true;
        cx.notify();
        let services = self.vcx.services.clone();
        let state = self.vcx.state.clone();
        cx.spawn(async move |this, cx| {
            let ok = call(
                cx,
                services.projects.delete_project(&sel.id),
                "Delete failed",
            )
            .await
            .is_some();
            if ok {
                let load = state.update(cx, |s, cx| s.load_initial(cx));
                let _ = load.await;
                state.update(cx, |s, cx| {
                    toast(ToastKind::Success, "Project deleted", cx);
                    let next = if s.projects.is_empty() {
                        ViewName::Welcome
                    } else {
                        ViewName::Briefing
                    };
                    s.go(next, None, cx);
                });
                let _ = this.update(cx, |v, _| v.confirming_delete = false);
            }
            let _ = this.update(cx, |v, cx| {
                v.deleting = false;
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn set_section(&mut self, s: Section, cx: &mut Context<Self>) {
        self.section = s;
        self.copy_effect(cx);
        cx.notify();
    }

    pub(crate) fn ident_pic(&self) -> String {
        if self.form.pic.is_empty() {
            let p = recur_ui::widgets::project_pic::project_pic_from_name(&self.form.name);
            if p.is_empty() { "?".into() } else { p }
        } else {
            self.form.pic.clone()
        }
    }

    pub(crate) fn ident_color(&self) -> String {
        if self.form.color.is_empty() {
            recur_ui::widgets::project_pic::PROJECT_COLORS[0].to_string()
        } else {
            self.form.color.clone()
        }
    }
}

/// The Files to copy placeholder, which the section draws as recur-ui text.
pub(crate) const COPY_FILES_PLACEHOLDER: &str = "# .env*\n# **/.env.local";

pub(crate) fn style_area(area: &Entity<TextareaState>, cx: &mut App) {
    let t = recur_ui::Theme::of(cx).tokens;
    recur_ui::style::style_textarea(area, t.faint.with_alpha(0.001), cx);
}

impl ProjectView {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = recur_ui::Theme::of(cx).tokens;
        let on = self.vcx.state.read(cx).view == ViewName::Project;
        let search = self.field(NAV, window, cx);
        let query = self.field_value(NAV, cx);
        let is_new = self.is_new(cx);
        let me = cx.entity().downgrade();
        let groups = filtered_nav(&query, is_new)
            .into_iter()
            .map(|(g, items)| {
                let items = items
                    .into_iter()
                    .map(|s| {
                        let me = me.clone();
                        NavEntry {
                            key: s.key().into(),
                            label: s.title().into(),
                            icon: s.icon(),
                            on: self.section == s,
                            on_click: Rc::new(move |_, cx| {
                                let _ = me.update(cx, |v, cx| v.set_section(s, cx));
                            }),
                        }
                    })
                    .collect();
                (SharedString::from(g), items)
            })
            .collect();
        let nav = nav_panel(&search, groups, &self.nav_scroll, window, cx);
        let title = if self.form.name.is_empty() {
            "Project".to_string()
        } else {
            self.form.name.clone()
        };
        let head = page_head(self.section.title(), Some(&title), window, cx);
        let body = self.section_body(window, cx);
        div()
            .id("v-project")
            .debug_selector(move || format!("v-project{}", if on { ":on" } else { "" }))
            .key_context("Project")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .bg(t.bg)
            .child(nav)
            .child(page_main(head, body, &self.main_scroll, window))
    }
}

impl Render for ProjectView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

/// Builds the entity (the `project` route).
pub fn build(vcx: &ViewContext, window: &mut Window, cx: &mut App) -> Entity<ProjectView> {
    cx.new(|cx| ProjectView::new(vcx, window, cx))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(name: &str, group: &str) -> BranchChoice {
        BranchChoice {
            name: name.into(),
            group: group.into(),
        }
    }

    #[test]
    fn a_configured_base_shows_even_when_its_ref_is_gone() {
        let list = vec![b("origin/main", "origin"), b("main", "Local")];
        let opts = branch_options(&list, "origin/release");
        assert_eq!(
            opts[0],
            ("origin/release".to_string(), "origin".to_string())
        );
        let opts = branch_options(&list, "feature");
        assert_eq!(opts[1], ("feature".to_string(), "Local".to_string()));
        assert_eq!(
            branch_options(&[], "main"),
            vec![("main".to_string(), "Local".to_string())]
        );
        assert_eq!(branch_options(&list, "main").len(), 2);
    }

    #[test]
    fn nav_filters_by_label_or_keyword() {
        assert_eq!(filtered_nav("", true).len(), 1);
        assert_eq!(filtered_nav("", false).len(), 2);
        let hits = filtered_nav("dotenv", false);
        assert_eq!(hits, vec![("Project", vec![Section::Run])]);
        assert!(filtered_nav("zzz", false).is_empty());
    }

    #[test]
    fn copy_meta_reports_matches() {
        let p = CopyPreview {
            count: 3,
            root: "/r".into(),
            ..Default::default()
        };
        assert_eq!(
            copy_files_meta(&p, ".env*", "", false),
            "3 files will be copied from /r"
        );
        assert_eq!(copy_files_meta(&p, "", "", false), "Nothing will be copied");
        assert_eq!(
            copy_files_meta(&p, "x", "", true),
            "Preview available after the project is created"
        );
        let none = CopyPreview::default();
        assert_eq!(
            copy_files_meta(&none, "x", "/repo", false),
            "No matching files in /repo"
        );
    }
}
