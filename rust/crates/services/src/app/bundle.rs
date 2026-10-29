//! `BundleService` (Go `bundle_service.go`): the bundle catalogs (cached on a
//! `bundle.toml` stamp), shebang commands, prompt actions and complete
//! scripts, palette search, goal expanders, and TypeScript compilation for
//! the macro runtime.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::{Duration, UNIX_EPOCH};

use async_trait::async_trait;
use parking_lot::Mutex;
use recur_bundles::{
    Catalog, Command, DiscoverRoots, ExpandAction, Grammar, LanguageData, PromptAction,
    PromptCompleteHit, PromptRunContext, RunContext, RunCtx, SearchAction, Snippet, Symbol,
    TerminalTheme, Theme,
};
use recur_domain::{
    PrSource, SETTING_LINEAR_API_KEY, SETTING_LINEAR_TEAM_KEY, effective_update_base_method,
};

use super::rt::rt;
use super::{AppServices, err};
use crate::api::{
    BundleService, BundleSettingInfo, BundleSkipInfo, CommandInfo, CommandPromptDto, GrammarInfo,
    LoopInfo, PaletteSearchDto, PaletteSearchGroupDto, PolicyInfo, PromptActionInfo, PromptArgInfo,
    PromptCompleteHitDto, PromptCompleteRequestDto, PromptCompleteResultDto, PromptContextDto,
    PromptResultDto, RoleInfo, RunContextDto, RunResultDto, SnippetInfo, SymbolInfo,
    TerminalPaletteInfo, TerminalThemeInfo, ThemeInfo, ThemePreviewInfo,
};
use crate::{Error, Result};

#[derive(Default)]
struct Cached {
    catalog: Catalog,
    stamp: String,
    roots: (String, String),
}

/// The discovered catalog, reused while no `bundle.toml` changed.
#[derive(Default)]
pub(crate) struct CatalogCache(Mutex<Option<Cached>>);

/// What a re-read keys on: the roots, every bundle directory's
/// `bundle.toml` mtime and size, and read errors.
fn catalog_stamp(user: &str, def: &str) -> String {
    let mut b = String::new();
    b.push_str(user);
    b.push('\0');
    b.push_str(def);
    for dir in [def, user] {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                b.push('\u{1}');
                b.push_str(&format!("{dir}: {e}"));
                continue;
            }
        };
        let mut names: Vec<_> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.file_name())
            .collect();
        names.sort();
        for name in names {
            let p = std::path::Path::new(dir).join(name).join("bundle.toml");
            match std::fs::metadata(&p) {
                Ok(m) => {
                    let ns = m
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map_or(0, |d| d.as_nanos());
                    let _ = write!(b, "{}:{ns}:{};", p.display(), m.len());
                }
                Err(_) => {
                    b.push('\u{2}');
                    b.push_str(&p.to_string_lossy());
                }
            }
        }
    }
    b
}

fn to_command_info(c: &Command) -> CommandInfo {
    CommandInfo {
        id: c.id.clone(),
        name: c.name.clone(),
        category: c.category.clone(),
        order: c.order,
        scope: c.scope.clone(),
        file_glob: c.file_glob.clone(),
        input: c.input.clone(),
        fallback_input: c.fallback_input.clone(),
        output: c.output.clone(),
        triggers: Some(c.triggers.clone()),
        prompts: c
            .prompts
            .iter()
            .map(|p| CommandPromptDto {
                name: p.name.clone(),
                label: p.label.clone(),
                r#type: p.kind.clone(),
                default: p.default.clone(),
                choices: p.choices.clone(),
            })
            .collect(),
        key_equivalent: c.key_equivalent.clone(),
        language: c.language.clone(),
        open_in_preview: c.open_in_preview,
        interpreter: c.interpreter.clone(),
        bundle_name: c.bundle_name.clone(),
        source_root: c.source_root.clone(),
    }
}

fn to_snippet_info(s: &Snippet) -> SnippetInfo {
    SnippetInfo {
        id: s.id.clone(),
        name: s.name.clone(),
        tab_trigger: s.tab_trigger.clone(),
        scope: s.scope.clone(),
        file_glob: s.file_glob.clone(),
        body: s.body.clone(),
        key_equivalent: s.key_equivalent.clone(),
        order: s.order,
        bundle_name: s.bundle_name.clone(),
        source_root: s.source_root.clone(),
    }
}

fn to_symbol_info(s: &Symbol) -> SymbolInfo {
    SymbolInfo {
        id: s.id.clone(),
        scope: s.scope.clone(),
        transform: s.transform.clone(),
        kind: s.kind.clone(),
        show: s.show,
        bundle_name: s.bundle_name.clone(),
        source_root: s.source_root.clone(),
    }
}

fn to_grammar_info(g: &Grammar, lang: &[LanguageData]) -> GrammarInfo {
    let mut info = GrammarInfo {
        scope_name: g.scope_name.clone(),
        id: g.id.clone(),
        path: g.path.clone(),
        language: g.language.clone(),
        file_types: g.file_types.clone(),
        first_line_match: g.first_line_match.clone(),
        inject_to: g.inject_to.clone(),
        embedded_languages: g.embedded_languages.clone(),
        bundle_name: g.bundle_name.clone(),
        source_root: g.source_root.clone(),
        ..Default::default()
    };
    if let Some(d) = recur_bundles::resolve_language_data(lang, &g.scope_name) {
        info.line_comment.clone_from(&d.line_comment);
        info.block_comment_start.clone_from(&d.block_comment_start);
        info.block_comment_end.clone_from(&d.block_comment_end);
        info.increase_indent_pattern
            .clone_from(&d.increase_indent_pattern);
        info.decrease_indent_pattern
            .clone_from(&d.decrease_indent_pattern);
        info.indent_next_line_pattern
            .clone_from(&d.indent_next_line_pattern);
        info.zero_indent_pattern.clone_from(&d.zero_indent_pattern);
        info.disable_indent_corrections = d.disable_indent_corrections;
        info.close_brackets.clone_from(&d.close_brackets);
    }
    info
}

fn to_theme_info(t: &Theme) -> ThemeInfo {
    ThemeInfo {
        id: t.id.clone(),
        name: t.name.clone(),
        path: t.path.clone(),
        ui_theme: t.ui_theme.clone(),
        order: t.order,
        preview: ThemePreviewInfo {
            bg: t.preview.bg.clone(),
            fg: t.preview.fg.clone(),
            r#fn: t.preview.func.clone(),
            name: t.preview.name.clone(),
            string: t.preview.string.clone(),
        },
        bundle_name: t.bundle_name.clone(),
        source_root: t.source_root.clone(),
    }
}

fn to_terminal_theme_info(t: &TerminalTheme) -> TerminalThemeInfo {
    let p = &t.palette;
    TerminalThemeInfo {
        id: t.id.clone(),
        name: t.name.clone(),
        path: t.path.clone(),
        ui_theme: t.ui_theme.clone(),
        order: t.order,
        palette: TerminalPaletteInfo {
            background: p.background.clone(),
            foreground: p.foreground.clone(),
            cursor: p.cursor.clone(),
            cursor_accent: p.cursor_accent.clone(),
            selection_background: p.selection_background.clone(),
            selection_foreground: p.selection_foreground.clone(),
            selection_inactive_background: p.selection_inactive_background.clone(),
            black: p.black.clone(),
            red: p.red.clone(),
            green: p.green.clone(),
            yellow: p.yellow.clone(),
            blue: p.blue.clone(),
            magenta: p.magenta.clone(),
            cyan: p.cyan.clone(),
            white: p.white.clone(),
            bright_black: p.bright_black.clone(),
            bright_red: p.bright_red.clone(),
            bright_green: p.bright_green.clone(),
            bright_yellow: p.bright_yellow.clone(),
            bright_blue: p.bright_blue.clone(),
            bright_magenta: p.bright_magenta.clone(),
            bright_cyan: p.bright_cyan.clone(),
            bright_white: p.bright_white.clone(),
        },
        bundle_name: t.bundle_name.clone(),
        source_root: t.source_root.clone(),
    }
}

fn to_prompt_action_info(p: &PromptAction) -> PromptActionInfo {
    PromptActionInfo {
        id: p.id.clone(),
        name: p.name.clone(),
        description: p.description.clone(),
        slash: p.slash.clone(),
        mode: p.mode.clone(),
        r#loop: p.loop_id.clone(),
        timeout_ms: p.timeout_ms,
        fresh_session: p.fresh_session,
        tab_group: p.tab_group.clone(),
        interpreter: p.interpreter.clone(),
        takes_args: p.takes_args,
        has_complete: !p.complete_body.trim().is_empty(),
        arg_placeholder: p.arg_placeholder.clone(),
        complete_interpreter: p.complete_interpreter.clone(),
        icon_svg: p.icon_svg.clone(),
        args: p
            .args
            .iter()
            .map(|a| PromptArgInfo {
                name: a.name.clone(),
                placeholder: a.placeholder.clone(),
                optional: a.optional,
            })
            .collect(),
        bundle_name: p.bundle_name.clone(),
        source_root: p.source_root.clone(),
    }
}

fn to_hit(h: PromptCompleteHit) -> PromptCompleteHitDto {
    PromptCompleteHitDto {
        insert: h.insert,
        label: h.label,
        psub: h.psub,
        detail: h.detail,
        badge: h.badge,
        action: h.action,
    }
}

fn to_run_context(rc: &RunContextDto, cmd: &Command) -> RunContext {
    RunContext {
        selected_text: rc.selected_text.clone(),
        current_line: rc.current_line.clone(),
        line_number: rc.line_number,
        column_number: rc.column_number,
        current_word: rc.current_word.clone(),
        file_path: rc.file_path.clone(),
        file_name: rc.file_name.clone(),
        directory: rc.directory.clone(),
        project_directory: rc.project_directory.clone(),
        scope: rc.scope.clone(),
        tab_size: rc.tab_size,
        soft_tabs: rc.soft_tabs,
        clipboard: rc.clipboard.clone(),
        bundle_support: cmd.bundle_dir.clone(),
        input: rc.input.clone(),
        prompt_values: rc.prompt_values.clone(),
        work_dir: rc.work_dir.clone(),
        bundle_settings: BTreeMap::new(),
    }
}

fn to_prompt_run_context(rc: &PromptContextDto, prompt: &PromptAction) -> PromptRunContext {
    let work_dir = if rc.work_dir.is_empty() {
        rc.worktree.clone()
    } else {
        rc.work_dir.clone()
    };
    let params = if rc.params.is_empty() {
        recur_bundles::params_from_args(prompt, &rc.args)
    } else {
        rc.params.clone()
    };
    PromptRunContext {
        workspace_id: rc.workspace_id.clone(),
        project_id: rc.project_id.clone(),
        worktree: rc.worktree.clone(),
        port: rc.port,
        selected_text: rc.selected_text.clone(),
        document: rc.document.clone(),
        current_line: rc.current_line.clone(),
        line_number: rc.line_number,
        column: rc.column,
        current_word: rc.current_word.clone(),
        file_path: rc.filepath.clone(),
        file_name: rc.filename.clone(),
        scope: rc.scope.clone(),
        clipboard: rc.clipboard.clone(),
        tab_size: rc.tab_size,
        soft_tabs: rc.soft_tabs,
        prompt_slash: prompt.slash.clone(),
        args: rc.args.clone(),
        params,
        pr_draft: prompt.slash == "open-draft-pr",
        surface: rc.surface.clone(),
        bundle_support: prompt.bundle_dir.clone(),
        work_dir,
        ..Default::default()
    }
}

/// The Linear bundle settings fall back to the process environment.
fn bundle_setting_process_fallback(id: &str) -> String {
    let env = match id {
        SETTING_LINEAR_API_KEY => "LINEAR_API_KEY",
        SETTING_LINEAR_TEAM_KEY => "LINEAR_TEAM_KEY",
        _ => return String::new(),
    };
    std::env::var(env).unwrap_or_default().trim().to_owned()
}

impl AppServices {
    fn roots(&self) -> &DiscoverRoots {
        &self.config.bundles
    }

    /// The catalog, re-read when any `bundle.toml` (or the roots) changed
    /// since the last read (Go `discoverCatalog`).
    pub(crate) fn discover_catalog(&self) -> Result<Catalog> {
        let DiscoverRoots {
            user_dir,
            default_dir,
        } = self.roots().clone();
        let stamp = catalog_stamp(&user_dir, &default_dir);
        let mut cache = self.catalog.0.lock();
        if let Some(c) = cache.as_ref()
            && c.stamp == stamp
            && c.roots == (user_dir.clone(), default_dir.clone())
        {
            return Ok(c.catalog.clone());
        }
        let cat = recur_bundles::discover_catalog(self.roots()).map_err(err)?;
        *cache = Some(Cached {
            catalog: cat.clone(),
            stamp,
            roots: (user_dir, default_dir),
        });
        Ok(cat)
    }

    async fn catalog(&self) -> Result<Catalog> {
        let me = self.me();
        self.off_thread(move || me.discover_catalog()).await
    }

    fn cache_dir(&self) -> Result<String> {
        std::fs::create_dir_all(&self.config.bundles_cache_dir).map_err(err)?;
        Ok(self.config.bundles_cache_dir.to_string_lossy().into_owned())
    }

    /// The bundle's `[[setting]]` values by name, Settings first, then the
    /// process environment for the Linear keys (Go `fillBundleSettingEnv`).
    fn bundle_setting_env(&self, cat: &Catalog, bundle_id: &str) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        if bundle_id.trim().is_empty() {
            return out;
        }
        let set = self.store().settings().migrate_bundle_values();
        for st in recur_bundles::settings_for_bundle(&cat.settings, bundle_id) {
            let mut val = set.bundle_value(&st.id);
            if val.is_empty() {
                val = bundle_setting_process_fallback(&st.id);
            }
            if !val.is_empty() {
                out.insert(st.name.clone(), val);
            }
        }
        out
    }

    /// Stamps the workspace merge base, update method, looking-at surface and
    /// open PR so shebang prompts (`/update`, `/review`, `/open-pr`) need not
    /// guess (Go `fillPromptGitEnv`).
    pub(crate) fn fill_prompt_git_env(&self, rc: &mut PromptRunContext, workspace_id: &str) {
        if workspace_id.is_empty() {
            return;
        }
        let Some(ws) = self.store().workspace(workspace_id) else {
            return;
        };
        let proj = self.store().project(&ws.project_id);
        let mut base = ws.base_branch.trim().to_owned();
        if base.is_empty()
            && let Some(p) = &proj
        {
            base = p.base_branch.trim().to_owned();
        }
        rc.base_branch = base;
        if let Some(p) = &proj {
            rc.update_base_method = effective_update_base_method(p, &self.store().settings())
                .as_str()
                .to_owned();
        }
        rc.surface = match rc.surface.trim().to_lowercase().as_str() {
            "local_pr" | "local-pr" | "local" => "local_pr",
            "github_pr" | "github-pr" | "gh" | "github" => "github_pr",
            "workspace" => "workspace",
            _ => "",
        }
        .into();
        let pr = if ws.is_pr_chat_host() {
            rc.pr_host = true;
            rc.surface = "github_pr".into();
            self.store().pr(ws.host_pr_id.trim())
        } else {
            self.store()
                .pr_by_workspace(workspace_id)
                .filter(|p| p.state != "Merged" && p.group != "merged")
        };
        if let Some(pr) = pr {
            rc.pr_source = pr.source.as_str().to_owned();
            rc.pr_url = pr.url.clone();
            rc.pr_title = pr.title.clone();
            rc.pr_base = pr.base.trim().to_owned();
            rc.pr_branch = pr.branch.trim().to_owned();
            // Shebang fast-fail is GitHub only: a Local fallback (gh create
            // failed) leaves /open-pr runnable so the agent can retry.
            if pr.source == PrSource::GitHub {
                rc.pr_id = pr.id.clone();
            }
            rc.pr_number = pr.number;
        }
        if rc.surface.is_empty() {
            rc.surface = "workspace".into();
        }
    }

    async fn run_prompt_action_at(
        &self,
        id: &str,
        rc: &PromptContextDto,
    ) -> Result<PromptResultDto> {
        let cat = self.catalog().await?;
        let prompt = recur_bundles::find_prompt_by_id(&cat.prompts, id)
            .ok_or_else(|| Error::new(format!("prompt {:?} not found", id)))?
            .clone();
        if recur_bundles::is_script_interpreter(&prompt.interpreter) {
            return Err(Error::new(format!(
                "prompt {:?} is {} — use CompiledPromptModule",
                id, prompt.interpreter
            )));
        }
        let mut timeout = recur_bundles::PROMPT_TIMEOUT;
        if prompt.timeout_ms > 0 {
            timeout = Duration::from_millis(prompt.timeout_ms as u64);
        }
        if rc.timeout_ms > 0 {
            timeout = Duration::from_millis(rc.timeout_ms as u64);
        }
        let ctx = RunCtx::background().with_timeout(timeout);
        let mut run_ctx = to_prompt_run_context(rc, &prompt);
        self.fill_prompt_git_env(&mut run_ctx, &rc.workspace_id);
        run_ctx.bundle_settings = self.bundle_setting_env(&cat, &prompt.bundle_id);
        let mut out = PromptResultDto::default();
        if !rc.chat_code.is_empty() && run_ctx.surface == "workspace" {
            let (since, now) = self
                .engine
                .review_checkpoint(&rc.workspace_id, &rc.chat_code, &prompt.slash)
                .await;
            if !since.is_empty() {
                out.fresh_session = Some(false);
            }
            run_ctx.review_since = since;
            run_ctx.review_now = now;
        }
        let res = recur_bundles::run_prompt_shebang(&ctx, &prompt, run_ctx)
            .await
            .map_err(err)?;
        out.prompt = res.prompt;
        out.display = res.display;
        out.issue = res.issue;
        Ok(out)
    }

    async fn run_prompt_complete_at(
        &self,
        id: &str,
        req: &PromptCompleteRequestDto,
    ) -> Result<PromptCompleteResultDto> {
        let cat = self.catalog().await?;
        let prompt = recur_bundles::find_prompt_by_id(&cat.prompts, id)
            .ok_or_else(|| Error::new(format!("prompt {:?} not found", id)))?
            .clone();
        if prompt.complete_body.trim().is_empty() {
            return Ok(PromptCompleteResultDto {
                hits: Some(Vec::new()),
                message: String::new(),
            });
        }
        if recur_bundles::is_script_interpreter(&prompt.complete_interpreter) {
            return Err(Error::new(format!(
                "prompt {:?} complete is {} — use CompiledCompleteModule",
                id, prompt.complete_interpreter
            )));
        }
        let ctx = RunCtx::background().with_timeout(recur_bundles::PROMPT_COMPLETE_TIMEOUT);
        let mut arg = req.arg.trim().to_owned();
        let params = if req.params.is_empty() {
            recur_bundles::params_from_args(&prompt, &req.args)
        } else {
            req.params.clone()
        };
        if arg.is_empty()
            && let Some(first) = prompt.args.first()
        {
            arg.clone_from(&first.name);
        }
        let mut rc = PromptRunContext {
            args: req.args.clone(),
            complete_arg: arg,
            params,
            prompt_slash: prompt.slash.clone(),
            bundle_support: prompt.bundle_dir.clone(),
            work_dir: prompt.bundle_dir.clone(),
            workspace_id: req.workspace_id.clone(),
            ..Default::default()
        };
        if !req.workspace_id.is_empty()
            && let Some(ws) = self.store().workspace(&req.workspace_id)
        {
            rc.project_id.clone_from(&ws.project_id);
            rc.worktree.clone_from(&ws.worktree_path);
            rc.port = ws.port;
            if !ws.worktree_path.trim().is_empty() {
                rc.work_dir.clone_from(&ws.worktree_path);
            }
        }
        self.fill_prompt_git_env(&mut rc, &req.workspace_id);
        rc.bundle_settings = self.bundle_setting_env(&cat, &prompt.bundle_id);
        let res = recur_bundles::run_prompt_complete_shebang(&ctx, &prompt, rc)
            .await
            .map_err(err)?;
        Ok(PromptCompleteResultDto {
            hits: Some(res.hits.into_iter().map(to_hit).collect()),
            message: res.message,
        })
    }

    async fn search_palette_at(&self, query: &str) -> Result<PaletteSearchDto> {
        let q = query.trim().to_owned();
        let empty = PaletteSearchDto {
            groups: Some(Vec::new()),
        };
        if q.chars().count() < 2 {
            return Ok(empty);
        }
        let cat = self.catalog().await?;
        if cat.searches.is_empty() {
            return Ok(empty);
        }
        let make_rc = |se: &SearchAction| PromptRunContext {
            search_query: q.clone(),
            args: q.clone(),
            complete_arg: "issue".into(),
            params: BTreeMap::from([("issue".to_owned(), q.clone())]),
            bundle_support: se.bundle_dir.clone(),
            work_dir: se.bundle_dir.clone(),
            bundle_settings: self.bundle_setting_env(&cat, &se.bundle_id),
            ..Default::default()
        };
        let groups = recur_bundles::collect_search_groups(
            &RunCtx::background(),
            &cat.searches,
            Some(&make_rc),
        )
        .await;
        Ok(PaletteSearchDto {
            groups: Some(
                groups
                    .into_iter()
                    .map(|g| PaletteSearchGroupDto {
                        name: g.name,
                        icon_svg: g.icon_svg,
                        hits: Some(g.hits.into_iter().map(to_hit).collect()),
                    })
                    .collect(),
            ),
        })
    }

    async fn expand_goal_at(&self, text: &str) -> Result<PromptResultDto> {
        let trimmed = text.trim().to_owned();
        if trimmed.is_empty() {
            return Ok(PromptResultDto::default());
        }
        let cat = self.catalog().await?;
        let make_rc = |ex: &ExpandAction| PromptRunContext {
            args: trimmed.clone(),
            params: BTreeMap::from([("issue".to_owned(), trimmed.clone())]),
            bundle_support: ex.bundle_dir.clone(),
            work_dir: ex.bundle_dir.clone(),
            bundle_settings: self.bundle_setting_env(&cat, &ex.bundle_id),
            ..Default::default()
        };
        let res = recur_bundles::expand_matching(&trimmed, &cat.expands, Some(&make_rc))
            .await
            .map_err(err)?;
        Ok(PromptResultDto {
            prompt: res.prompt,
            display: res.display,
            fresh_session: None,
            issue: res.issue,
        })
    }

    async fn run_command_at(&self, id: &str, rc: &RunContextDto) -> Result<RunResultDto> {
        let cat = self.catalog().await?;
        let cmd = recur_bundles::find_by_id(&cat.commands, id)
            .ok_or_else(|| Error::new(format!("command {:?} not found", id)))?
            .clone();
        if recur_bundles::is_script_interpreter(&cmd.interpreter) {
            return Err(Error::new(format!(
                "command {:?} is {} — use CompiledModule",
                id, cmd.interpreter
            )));
        }
        let timeout = if rc.timeout_ms > 0 {
            Duration::from_millis(rc.timeout_ms as u64)
        } else {
            recur_bundles::DEFAULT_TIMEOUT
        };
        let ctx = RunCtx::background().with_timeout(timeout);
        let mut run_ctx = to_run_context(rc, &cmd);
        run_ctx.bundle_settings = self.bundle_setting_env(&cat, &cmd.bundle_id);
        match recur_bundles::run(&ctx, &cmd, run_ctx).await {
            Ok(res) => Ok(RunResultDto {
                stdout: res.stdout,
                stderr: res.stderr,
                exit_code: res.exit_code,
                output: res.output,
            }),
            // Go returned the partial output beside the error; the bridge
            // rejected the call, so only the error reaches the UI.
            Err(e) => Err(Error::new(e.message)),
        }
    }

    async fn compiled<F>(&self, f: F) -> Result<String>
    where
        F: FnOnce(&Catalog, &str) -> Result<String> + Send + 'static,
    {
        let me = self.me();
        self.off_thread(move || {
            let cat = me.discover_catalog()?;
            let cache = me.cache_dir()?;
            f(&cat, &cache)
        })
        .await
    }
}

#[async_trait]
impl BundleService for AppServices {
    async fn compiled_complete_module(&self, id: &str) -> Result<String> {
        let id = id.to_owned();
        self.compiled(move |cat, cache| {
            let prompt = recur_bundles::find_prompt_by_id(&cat.prompts, &id)
                .ok_or_else(|| Error::new(format!("prompt {:?} not found", id)))?;
            if prompt.complete_interpreter != recur_bundles::INTERPRETER_TYPESCRIPT {
                return Err(Error::new(format!(
                    "prompt {:?} complete is not typescript",
                    id
                )));
            }
            recur_bundles::compile_complete_macro(cache, prompt)
                .map(|r| r.js)
                .map_err(err)
        })
        .await
    }

    async fn compiled_module(&self, id: &str) -> Result<String> {
        let id = id.to_owned();
        self.compiled(move |cat, cache| {
            let cmd = recur_bundles::find_by_id(&cat.commands, &id)
                .ok_or_else(|| Error::new(format!("command {:?} not found", id)))?;
            if !recur_bundles::is_script_interpreter(&cmd.interpreter) {
                return Err(Error::new(format!("command {:?} is not typescript", id)));
            }
            recur_bundles::compile_macro(cache, cmd)
                .map(|r| r.js)
                .map_err(err)
        })
        .await
    }

    async fn compiled_prompt_module(&self, id: &str) -> Result<String> {
        let id = id.to_owned();
        self.compiled(move |cat, cache| {
            let prompt = recur_bundles::find_prompt_by_id(&cat.prompts, &id)
                .ok_or_else(|| Error::new(format!("prompt {:?} not found", id)))?;
            if !recur_bundles::is_script_interpreter(&prompt.interpreter) {
                return Err(Error::new(format!("prompt {:?} is not typescript", id)));
            }
            recur_bundles::compile_prompt_macro(cache, prompt)
                .map(|r| r.js)
                .map_err(err)
        })
        .await
    }

    async fn expand_goal(&self, text: &str) -> Result<PromptResultDto> {
        rt!(self, text => |s| s.expand_goal_at(&text).await)
    }

    async fn invalidate_catalog(&self) -> Result<()> {
        if let Some(c) = self.catalog.0.lock().as_mut() {
            c.stamp.clear();
        }
        Ok(())
    }

    async fn list_bundle_settings(&self) -> Result<Vec<BundleSettingInfo>> {
        let cat = self.catalog().await?;
        Ok(cat
            .settings
            .iter()
            .map(|st| BundleSettingInfo {
                id: st.id.clone(),
                name: st.name.clone(),
                label: st.label.clone(),
                description: st.description.clone(),
                secret: st.secret,
                placeholder: st.placeholder.clone(),
                url: st.url.clone(),
                url_label: st.url_label.clone(),
                bundle_id: st.bundle_id.clone(),
                bundle_name: st.bundle_name.clone(),
                source_root: st.source_root.clone(),
            })
            .collect())
    }

    async fn list_commands(&self, scope: &str, path: &str) -> Result<Vec<CommandInfo>> {
        let cat = self.catalog().await?;
        let filter = !scope.is_empty() || !path.is_empty();
        Ok(cat
            .commands
            .iter()
            .filter(|c| !filter || recur_bundles::matches(c, scope, path))
            .map(to_command_info)
            .collect())
    }

    async fn list_grammars(&self) -> Result<Vec<GrammarInfo>> {
        let cat = self.catalog().await?;
        Ok(cat
            .grammars
            .iter()
            .map(|g| to_grammar_info(g, &cat.language_data))
            .collect())
    }

    async fn list_loops(&self) -> Result<Vec<LoopInfo>> {
        let cat = self.catalog().await?;
        Ok(cat
            .loops
            .iter()
            .map(|l| LoopInfo {
                id: l.id.clone(),
                name: l.name.clone(),
                hint: l.hint.clone(),
                entry: l.entry.clone(),
                bundle_name: l.bundle_name.clone(),
                source_root: l.source_root.clone(),
            })
            .collect())
    }

    async fn list_policies(&self) -> Result<Vec<PolicyInfo>> {
        let cat = self.catalog().await?;
        Ok(cat
            .policies
            .iter()
            .map(|p| PolicyInfo {
                id: p.id.clone(),
                name: p.name.clone(),
                hint: p.hint.clone(),
                max_rounds: p.max_rounds,
                on_max_rounds: p.on_max_rounds.clone(),
                plan_approval: p.plan_approval.clone(),
                extend_rounds: p.extend_rounds,
                bundle_name: p.bundle_name.clone(),
                source_root: p.source_root.clone(),
            })
            .collect())
    }

    async fn list_prompt_actions(&self) -> Result<Vec<PromptActionInfo>> {
        let cat = self.catalog().await?;
        Ok(cat.prompts.iter().map(to_prompt_action_info).collect())
    }

    async fn list_roles(&self) -> Result<Vec<RoleInfo>> {
        let cat = self.catalog().await?;
        Ok(cat
            .roles
            .iter()
            .map(|r| RoleInfo {
                id: r.id.clone(),
                name: r.name.clone(),
                hint: r.hint.clone(),
                code: r.code.clone(),
                mode: r.mode.clone(),
                tools: r.tools.clone(),
                model_slot: r.model_slot.clone(),
                bundle_name: r.bundle_name.clone(),
                source_root: r.source_root.clone(),
            })
            .collect())
    }

    async fn list_skipped_bundles(&self) -> Result<Vec<BundleSkipInfo>> {
        let cat = self.catalog().await?;
        Ok(cat
            .skips
            .iter()
            .map(|sk| BundleSkipInfo {
                dir: sk.dir.clone(),
                name: sk.name.clone(),
                source: sk.source.clone(),
                reason: sk.reason.clone(),
            })
            .collect())
    }

    async fn list_snippets(&self, scope: &str, path: &str) -> Result<Vec<SnippetInfo>> {
        let cat = self.catalog().await?;
        let filter = !scope.is_empty() || !path.is_empty();
        Ok(cat
            .snippets
            .iter()
            .filter(|sn| !filter || recur_bundles::snippet_matches(sn, scope, path))
            .map(to_snippet_info)
            .collect())
    }

    async fn list_symbols(&self) -> Result<Vec<SymbolInfo>> {
        let cat = self.catalog().await?;
        Ok(cat.symbols.iter().map(to_symbol_info).collect())
    }

    async fn list_terminal_themes(&self) -> Result<Vec<TerminalThemeInfo>> {
        let cat = self.catalog().await?;
        Ok(cat
            .terminal_themes
            .iter()
            .map(to_terminal_theme_info)
            .collect())
    }

    async fn list_themes(&self) -> Result<Vec<ThemeInfo>> {
        let cat = self.catalog().await?;
        Ok(cat.themes.iter().map(to_theme_info).collect())
    }

    async fn read_grammar(&self, scope_name: &str) -> Result<String> {
        let cat = self.catalog().await?;
        let g = recur_bundles::find_grammar_by_scope(&cat.grammars, scope_name)
            .ok_or_else(|| Error::new(format!("grammar {:?} not found", scope_name)))?;
        let path = g.abs_path.clone();
        let scope = scope_name.to_owned();
        self.off_thread(move || {
            std::fs::read_to_string(&path)
                .map_err(|e| Error::new(format!("read grammar {:?}: {e}", scope)))
        })
        .await
    }

    async fn read_theme(&self, id: &str) -> Result<String> {
        let cat = self.catalog().await?;
        let t = recur_bundles::find_theme_by_id(&cat.themes, id)
            .ok_or_else(|| Error::new(format!("theme {:?} not found", id)))?;
        let path = t.abs_path.clone();
        let id = id.to_owned();
        self.off_thread(move || {
            std::fs::read_to_string(&path)
                .map_err(|e| Error::new(format!("read theme {:?}: {e}", id)))
        })
        .await
    }

    async fn run_command(&self, id: &str, rc: &RunContextDto) -> Result<RunResultDto> {
        rt!(self, id, rc => |s| s.run_command_at(&id, &rc).await)
    }

    async fn run_prompt_action(&self, id: &str, rc: &PromptContextDto) -> Result<PromptResultDto> {
        rt!(self, id, rc => |s| s.run_prompt_action_at(&id, &rc).await)
    }

    async fn run_prompt_complete(
        &self,
        id: &str,
        req: &PromptCompleteRequestDto,
    ) -> Result<PromptCompleteResultDto> {
        rt!(self, id, req => |s| s.run_prompt_complete_at(&id, &req).await)
    }

    async fn search_palette(&self, query: &str) -> Result<PaletteSearchDto> {
        rt!(self, query => |s| s.search_palette_at(&query).await)
    }
}
