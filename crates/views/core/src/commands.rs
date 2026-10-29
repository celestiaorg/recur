//! The Recur Bundle command registry: discovered bundle commands
//! (TypeScript macros and shebang scripts), scope and file-glob matching, input resolution and
//! running a command against an editor context.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::{App, AppContext, Context, Entity, Task};
use recur_editor::keyequiv::{effective_key_equivalent, format_key_equivalent_display};
use recur_services::api::{CommandInfo, RunContextDto};

use crate::state::{AppState, bridge_error_message};

/// Where a command's output goes (`OutputRouting`).
pub const OUTPUT_ROUTINGS: [&str; 10] = [
    "discard",
    "replaceSelection",
    "replaceDocument",
    "replaceInput",
    "insertText",
    "insertSnippet",
    "newDocument",
    "showAsTooltip",
    "copyToClipboard",
    "showAsToast",
];

/// `validateOutputRouting`: `None` when valid, else the error text.
pub fn validate_output_routing(s: &str) -> Option<String> {
    if OUTPUT_ROUTINGS.contains(&s) {
        return None;
    }
    if s == "showAsHTML" {
        return Some(
            "output showAsHTML was removed — use newDocument with language + openInPreview".into(),
        );
    }
    Some(format!(
        "unknown output {}",
        serde_json::to_string(s).unwrap_or_default()
    ))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandPrompt {
    pub name: String,
    pub label: String,
    pub r#type: String,
    pub default: String,
    pub choices: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisteredCommand {
    pub id: String,
    pub label: String,
    pub category: String,
    pub order: i64,
    pub scope: String,
    pub file_glob: String,
    pub triggers: Vec<String>,
    pub prompts: Vec<CommandPrompt>,
    pub input: String,
    pub fallback_input: String,
    pub output: String,
    pub interpreter: String,
    /// Bundle display name from the manifest (Settings subtitle).
    pub bundle_name: String,
    /// The authoring string from bundle.toml (may be empty, invalid or reserved).
    pub key_equivalent_raw: String,
    /// The canonical chord when valid and not app-reserved; else empty.
    pub key_equivalent: String,
    /// Glyph form for UI (⇧⌘U).
    pub key_equivalent_display: String,
    /// Scratch-tab language/extension for `newDocument`.
    pub language: String,
    pub open_in_preview: bool,
}

/// What a command runs against (`EditorContext`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditorContext {
    pub selected_text: String,
    pub document: String,
    pub current_line: String,
    pub line_number: i64,
    pub column: i64,
    pub current_word: String,
    pub scope: String,
    pub tab_size: i64,
    pub soft_tabs: bool,
    pub path: String,
    pub directory: String,
    pub project_directory: String,
    pub work_dir: String,
    pub clipboard: String,
    /// Shebang / macro wall-clock limit (ms); 0 is the backend/runner default.
    pub timeout_ms: i64,
}

/// The input a TypeScript macro sees (`MacroContext`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MacroContext {
    pub input: String,
    pub selected_text: String,
    pub document: String,
    pub current_line: String,
    pub line_number: i64,
    pub column: i64,
    pub current_word: String,
    pub filepath: String,
    pub filename: String,
    pub scope: String,
    pub clipboard: String,
    pub tab_size: i64,
    pub soft_tabs: bool,
    pub prompts: BTreeMap<String, String>,
}

/// A macro's return (`MacroResult`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MacroResult {
    pub text: String,
    pub output: String,
    pub language: String,
    pub open_in_preview: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunCommandResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i64,
    pub output: String,
    pub input_mode: String,
    /// The effective language for `newDocument` (macro override or command default).
    pub language: String,
    pub open_in_preview: bool,
}

/// A boxed local future.
pub type BoxFuture<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T>>>;

/// Runs a compiled TypeScript macro module (`runMacroModule`, the macro worker).
pub type MacroRunner =
    Rc<dyn Fn(String, MacroContext, Duration) -> BoxFuture<Result<MacroResult, String>>>;

/// The active Observatory's "run this command against the focused editor".
pub type EditorCommandHandler = Rc<dyn Fn(String, &mut App) -> Task<Result<(), String>>>;

/// The focused editor's `(scope, path)`.
pub type EditorFocusProvider = Rc<dyn Fn(&App) -> (String, String)>;

/// The `scopeForLang` table for files no grammar claims.
pub fn scope_for_lang(id: &str) -> &'static str {
    match id {
        "go" => "source.go",
        "python" => "source.python",
        "javascript" => "source.js",
        "typescript" => "source.ts",
        "json" => "source.json",
        "yaml" => "source.yaml",
        "markdown" => "text.html.markdown",
        "rust" => "source.rust",
        "html" => "text.html.basic",
        "css" => "source.css",
        "shell" => "source.shell",
        "svelte" => "source.svelte",
        "vue" => "source.vue",
        "java" => "source.java",
        "c" => "source.c",
        "cpp" => "source.cpp",
        "ruby" => "source.ruby",
        "php" => "source.php",
        "sql" => "source.sql",
        "toml" => "source.toml",
        _ => "text.plain",
    }
}

/// `scopeForPath`: the TextMate scope for a path. `grammar_scope` is what the registered
/// grammars resolve (`resolveScopeForPath`); without one, .tsx/.jsx keep their own scopes.
pub fn scope_for_path(path: &str, grammar_scope: Option<&str>) -> String {
    if let Some(s) = grammar_scope.filter(|s| !s.is_empty()) {
        return s.to_string();
    }
    let lower = path.to_lowercase();
    if lower.ends_with(".tsx") {
        return "source.tsx".into();
    }
    if lower.ends_with(".jsx") {
        return "source.js.jsx".into();
    }
    scope_for_lang(recur_editor::prefs::language_id_for_path(path)).into()
}

/// One `{a,b,c}` brace group into alternatives.
fn expand_braces(pattern: &str) -> Vec<String> {
    let (Some(start), Some(end)) = (pattern.find('{'), pattern.rfind('}')) else {
        return vec![pattern.to_string()];
    };
    if end <= start {
        return vec![pattern.to_string()];
    }
    let (prefix, suffix) = (&pattern[..start], &pattern[end + 1..]);
    let out: Vec<String> = pattern[start + 1..end]
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| format!("{prefix}{p}{suffix}"))
        .collect();
    if out.is_empty() {
        vec![pattern.to_string()]
    } else {
        out
    }
}

/// The glob as a regex (`filepath.Match` style): `?` one non-slash char, `*` within a
/// segment, `**` across segments.
fn glob_regex(p: &str) -> Option<regex::Regex> {
    let s = regex::escape(p)
        .replace(r"\?", "[^/]")
        .replace(r"\*\*", "::DS::")
        .replace(r"\*", "[^/]*")
        .replace("::DS::", ".*");
    regex::Regex::new(&format!("^{s}$")).ok()
}

fn glob_match(pattern: &str, file_path: &str) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return true;
    }
    let alts = expand_braces(pattern);
    if alts.len() > 1 {
        return alts.iter().any(|p| glob_match(p, file_path));
    }
    let base = file_path.rsplit('/').next().unwrap_or(file_path);
    let test = |p: &str, s: &str| glob_regex(p).is_some_and(|r| r.is_match(s));
    if test(pattern, base) || test(pattern, file_path) {
        return true;
    }
    if let Some(suf) = pattern.strip_prefix("**/") {
        if test(suf, base) {
            return true;
        }
        let parts: Vec<&str> = file_path.split('/').collect();
        return (0..parts.len()).any(|i| test(suf, &parts[i..].join("/")));
    }
    false
}

fn scope_prefix_match(want: &str, have: &str) -> bool {
    if want.is_empty() {
        return true;
    }
    if have.is_empty() {
        return false;
    }
    want.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .any(|alt| have == alt || have.starts_with(&format!("{alt}.")))
}

/// `commandMatches`: unscoped commands are global; scopes OR, the file glob ANDs.
pub fn command_matches(scope_want: &str, file_glob: &str, scope: &str, path: &str) -> bool {
    if scope_want.is_empty() && file_glob.is_empty() {
        return true;
    }
    if !scope_want.is_empty() && !scope_prefix_match(scope_want, scope) {
        return false;
    }
    file_glob.is_empty() || glob_match(file_glob, path)
}

fn non_empty(s: &str, fallback: &str) -> String {
    if s.is_empty() {
        fallback.to_string()
    } else {
        s.to_string()
    }
}

/// `fromBundleInfo`.
pub fn from_bundle_info(c: &CommandInfo) -> RegisteredCommand {
    let raw = c.key_equivalent.clone();
    let key = effective_key_equivalent(Some(&raw));
    RegisteredCommand {
        id: c.id.clone(),
        label: c.name.clone(),
        category: non_empty(&c.category, "Commands"),
        order: c.order,
        scope: c.scope.clone(),
        file_glob: c.file_glob.clone(),
        triggers: c
            .triggers
            .clone()
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| vec!["palette".into()]),
        prompts: c
            .prompts
            .iter()
            .map(|p| CommandPrompt {
                name: p.name.clone(),
                label: non_empty(&p.label, &p.name),
                r#type: non_empty(&p.r#type, "text"),
                default: p.default.clone(),
                choices: p.choices.clone(),
            })
            .collect(),
        input: non_empty(&c.input, "selection"),
        fallback_input: c.fallback_input.clone(),
        output: non_empty(&c.output, "replaceSelection"),
        interpreter: non_empty(&c.interpreter, "shebang"),
        bundle_name: c.bundle_name.trim().to_string(),
        key_equivalent_display: if key.is_empty() {
            String::new()
        } else {
            format_key_equivalent_display(&key)
        },
        key_equivalent_raw: raw,
        key_equivalent: key,
        language: c.language.trim().to_string(),
        open_in_preview: c.open_in_preview,
    }
}

/// `sortCommands`: by order, then category, then label.
pub fn sort_commands(cmds: &mut [RegisteredCommand]) {
    use recur_editor::text::locale_compare;
    cmds.sort_by(|a, b| {
        a.order
            .cmp(&b.order)
            .then_with(|| locale_compare(&a.category, &b.category))
            .then_with(|| locale_compare(&a.label, &b.label))
    });
}

/// `categories`: each category once, ordered by its lowest command order then name.
pub fn categories(cmds: &[RegisteredCommand]) -> Vec<String> {
    let mut meta: Vec<(String, i64)> = Vec::new();
    for c in cmds {
        match meta.iter_mut().find(|(n, _)| *n == c.category) {
            Some((_, o)) if c.order < *o => *o = c.order,
            Some(_) => {}
            None => meta.push((c.category.clone(), c.order)),
        }
    }
    meta.sort_by(|a, b| match a.1.cmp(&b.1) {
        Ordering::Equal => recur_editor::text::locale_compare(&a.0, &b.0),
        o => o,
    });
    meta.into_iter().map(|(n, _)| n).collect()
}

/// `resolveInputText`: `(text, mode)` with the fallback input applied when the first is empty.
pub fn resolve_input_text(cmd: &RegisteredCommand, ctx: &EditorContext) -> (String, String) {
    let pick = |mode: &str| match mode {
        "none" => String::new(),
        "document" => ctx.document.clone(),
        "line" => ctx.current_line.clone(),
        "word" => ctx.current_word.clone(),
        _ => ctx.selected_text.clone(),
    };
    let mut mode = non_empty(&cmd.input, "selection");
    let mut v = pick(&mode);
    if v.is_empty() && !cmd.fallback_input.is_empty() {
        mode = cmd.fallback_input.clone();
        v = pick(&mode);
    }
    (v, mode)
}

/// `scratchLanguage`: a language/extension token safe for `scratch://` paths.
pub fn scratch_language(raw: &str) -> String {
    let s = raw.trim().to_lowercase();
    let s = s.strip_prefix('.').unwrap_or(&s);
    let ok = !s.is_empty()
        && s.len() <= 16
        && s.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if ok { s.to_string() } else { "txt".into() }
}

const CACHE_TTL: Duration = Duration::from_millis(2000);

/// Command discovery state (`bundleCache`) and the handlers views register.
#[derive(Default)]
pub struct CommandRegistry {
    cache: Option<(String, Vec<RegisteredCommand>, Instant)>,
    editor_handler: Option<EditorCommandHandler>,
    focus_provider: Option<EditorFocusProvider>,
    macro_runner: Option<MacroRunner>,
}

impl AppState {
    fn disabled_command_ids(&self) -> Vec<String> {
        self.settings
            .as_ref()
            .map(|s| {
                s.disabled_bundle_commands
                    .iter()
                    .filter(|x| !x.is_empty())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    fn filter_enabled(&self, cmds: Vec<RegisteredCommand>) -> Vec<RegisteredCommand> {
        let disabled = self.disabled_command_ids();
        cmds.into_iter()
            .filter(|c| !disabled.contains(&c.id))
            .collect()
    }

    /// `invalidateCommandCache` (after an install or Reload Bundles).
    pub fn invalidate_command_cache(&mut self) {
        self.commands.cache = None;
    }

    /// `listCommands`: enabled commands for a scope and path, sorted.
    pub fn list_commands(
        &mut self,
        scope: &str,
        path: &str,
        cx: &mut Context<Self>,
    ) -> Task<Vec<RegisteredCommand>> {
        let key = format!("{scope}\0{path}");
        if let Some((k, cmds, at)) = &self.commands.cache
            && *k == key
            && at.elapsed() < CACHE_TTL
        {
            let mut out = self.filter_enabled(cmds.clone());
            sort_commands(&mut out);
            return Task::ready(out);
        }
        let services = self.services.clone();
        let (scope, path) = (scope.to_string(), path.to_string());
        cx.spawn(async move |this, cx| {
            let cmds: Vec<RegisteredCommand> =
                match services.bundle.list_commands(&scope, &path).await {
                    Ok(list) => {
                        let cmds: Vec<RegisteredCommand> = list
                            .iter()
                            .map(from_bundle_info)
                            .filter(|c| command_matches(&c.scope, &c.file_glob, &scope, &path))
                            .collect();
                        let cached = cmds.clone();
                        let _ = this.update(cx, |s, _| {
                            s.commands.cache = Some((key, cached, Instant::now()))
                        });
                        cmds
                    }
                    Err(_) => Vec::new(),
                };
            this.update(cx, |s, _| {
                let mut out = s.filter_enabled(cmds);
                sort_commands(&mut out);
                out
            })
            .unwrap_or_default()
        })
    }

    /// `listAllCommandsUnfiltered`: every discovered command, disabled ones included (Settings
    /// keeps their toggles visible).
    pub fn list_all_commands_unfiltered(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Task<Vec<RegisteredCommand>> {
        let services = self.services.clone();
        cx.spawn(async move |_, _| {
            let mut out: Vec<RegisteredCommand> = services
                .bundle
                .list_commands("", "")
                .await
                .map(|l| l.iter().map(from_bundle_info).collect())
                .unwrap_or_default();
            sort_commands(&mut out);
            out
        })
    }

    /// `listAllCommands`: every enabled command (the shortcuts cheatsheet).
    pub fn list_all_commands(&mut self, cx: &mut Context<Self>) -> Task<Vec<RegisteredCommand>> {
        let all = self.list_all_commands_unfiltered(cx);
        cx.spawn(async move |this, cx| {
            let all = all.await;
            this.update(cx, |s, _| s.filter_enabled(all))
                .unwrap_or_default()
        })
    }

    /// `listBoundCommands`: palette commands with an effective keyEquivalent.
    pub fn list_bound_commands(&mut self, cx: &mut Context<Self>) -> Task<Vec<RegisteredCommand>> {
        let all = self.list_all_commands(cx);
        cx.background_spawn(async move {
            all.await
                .into_iter()
                .filter(|c| {
                    c.triggers.iter().any(|t| t == "palette")
                        && !c.key_equivalent_display.is_empty()
                })
                .collect()
        })
    }

    /// `commandsForTrigger`.
    pub fn commands_for_trigger(
        &mut self,
        trigger: &str,
        scope: &str,
        path: &str,
        cx: &mut Context<Self>,
    ) -> Task<Vec<RegisteredCommand>> {
        let list = self.list_commands(scope, path, cx);
        let trigger = trigger.to_string();
        cx.background_spawn(async move {
            list.await
                .into_iter()
                .filter(|c| c.triggers.contains(&trigger))
                .collect()
        })
    }

    /// `commandsForKeyEquivalent`: palette commands bound to a canonical chord for this file.
    pub fn commands_for_key_equivalent(
        &mut self,
        canonical: &str,
        scope: &str,
        path: &str,
        cx: &mut Context<Self>,
    ) -> Task<Vec<RegisteredCommand>> {
        if canonical.is_empty() {
            return Task::ready(Vec::new());
        }
        let list = self.list_commands(scope, path, cx);
        let canonical = canonical.to_string();
        cx.background_spawn(async move {
            list.await
                .into_iter()
                .filter(|c| {
                    c.triggers.iter().any(|t| t == "palette") && c.key_equivalent == canonical
                })
                .collect()
        })
    }

    /// The macro worker's runner. TypeScript commands need one.
    pub fn set_macro_runner(&mut self, runner: Option<MacroRunner>) {
        self.commands.macro_runner = runner;
    }

    /// `runCommand`. `prefs_tab_size` is the editor prefs' tab size, used when the context
    /// carries none.
    pub fn run_command(
        &mut self,
        id: &str,
        ctx: EditorContext,
        inputs: BTreeMap<String, String>,
        prefs_tab_size: i64,
        cx: &mut Context<Self>,
    ) -> Task<Result<RunCommandResult, String>> {
        let list = self.list_commands(&ctx.scope, &ctx.path, cx);
        let services = self.services.clone();
        let runner = self.commands.macro_runner.clone();
        let id = id.to_string();
        cx.spawn(async move |_, _| {
            let all = list.await;
            let Some(cmd) = all.into_iter().find(|c| c.id == id) else {
                return Err(format!("Unknown command {id}"));
            };
            let (input, mode) = resolve_input_text(&cmd, &ctx);
            let tab_size = if ctx.tab_size != 0 {
                ctx.tab_size
            } else {
                prefs_tab_size
            };
            let file_name = ctx.path.rsplit('/').next().unwrap_or(&ctx.path).to_string();

            if cmd.interpreter == "typescript" {
                let code = services
                    .bundle
                    .compiled_module(&id)
                    .await
                    .map_err(|e| bridge_error_message(&e))?;
                let Some(runner) = runner else {
                    return Err("TypeScript commands need the macro runner".into());
                };
                let mctx = MacroContext {
                    input,
                    selected_text: ctx.selected_text.clone(),
                    document: ctx.document.clone(),
                    current_line: ctx.current_line.clone(),
                    line_number: ctx.line_number,
                    column: ctx.column,
                    current_word: ctx.current_word.clone(),
                    filepath: ctx.path.clone(),
                    filename: file_name,
                    scope: ctx.scope.clone(),
                    clipboard: ctx.clipboard.clone(),
                    tab_size,
                    soft_tabs: ctx.soft_tabs,
                    prompts: inputs,
                };
                let timeout = Duration::from_millis(if ctx.timeout_ms > 0 {
                    ctx.timeout_ms as u64
                } else {
                    10_000
                });
                return Ok(match runner(code, mctx, timeout).await {
                    Ok(r) => {
                        let output = non_empty(&r.output, &cmd.output);
                        let language = non_empty(&r.language, &cmd.language);
                        let open_in_preview = r.open_in_preview.unwrap_or(cmd.open_in_preview);
                        match validate_output_routing(&output) {
                            Some(bad) => RunCommandResult {
                                stdout: r.text,
                                stderr: bad,
                                exit_code: 1,
                                output: cmd.output.clone(),
                                input_mode: mode,
                                language,
                                open_in_preview,
                            },
                            None => RunCommandResult {
                                stdout: r.text,
                                stderr: String::new(),
                                exit_code: 0,
                                output,
                                input_mode: mode,
                                language,
                                open_in_preview,
                            },
                        }
                    }
                    Err(message) => RunCommandResult {
                        stdout: String::new(),
                        stderr: message,
                        exit_code: 1,
                        output: cmd.output.clone(),
                        input_mode: mode,
                        language: cmd.language.clone(),
                        open_in_preview: cmd.open_in_preview,
                    },
                });
            }

            let rc = RunContextDto {
                selected_text: ctx.selected_text.clone(),
                current_line: ctx.current_line.clone(),
                line_number: ctx.line_number,
                column_number: ctx.column,
                current_word: ctx.current_word.clone(),
                file_path: ctx.path.clone(),
                file_name,
                directory: ctx.directory.clone(),
                project_directory: ctx.project_directory.clone(),
                scope: ctx.scope.clone(),
                tab_size,
                soft_tabs: ctx.soft_tabs,
                clipboard: ctx.clipboard.clone(),
                input,
                prompt_values: inputs,
                work_dir: if ctx.work_dir.is_empty() {
                    ctx.project_directory.clone()
                } else {
                    ctx.work_dir.clone()
                },
                timeout_ms: ctx.timeout_ms,
            };
            let res = services
                .bundle
                .run_command(&id, &rc)
                .await
                .map_err(|e| bridge_error_message(&e))?;
            let output = non_empty(&res.output, &cmd.output);
            let bad = validate_output_routing(&output);
            if let Some(bad) = bad.clone().filter(|_| res.exit_code == 0) {
                return Ok(RunCommandResult {
                    stdout: res.stdout,
                    stderr: bad,
                    exit_code: 1,
                    output: cmd.output.clone(),
                    input_mode: mode,
                    language: cmd.language.clone(),
                    open_in_preview: cmd.open_in_preview,
                });
            }
            Ok(RunCommandResult {
                stdout: res.stdout,
                stderr: res.stderr,
                exit_code: res.exit_code,
                output: if bad.is_some() {
                    cmd.output.clone()
                } else {
                    output
                },
                input_mode: mode,
                language: cmd.language.clone(),
                open_in_preview: cmd.open_in_preview,
            })
        })
    }

    /// `setEditorCommandHandler`: the active Observatory runs commands against its editor.
    pub fn set_editor_command_handler(&mut self, handler: Option<EditorCommandHandler>) {
        self.commands.editor_handler = handler;
    }

    /// `runEditorCommand`. The handler reads the state and updates its window, so it runs with
    /// neither borrowed.
    pub fn run_editor_command(
        this: &Entity<Self>,
        id: &str,
        cx: &mut App,
    ) -> Task<Result<(), String>> {
        let handler = this.read(cx).commands.editor_handler.clone();
        match handler {
            Some(h) => h(id.to_string(), cx),
            None => Task::ready(Err(
                "No editor is focused — open a file in the Observatory first".into(),
            )),
        }
    }

    pub fn set_editor_focus_provider(&mut self, provider: Option<EditorFocusProvider>) {
        self.commands.focus_provider = provider;
    }

    /// `currentEditorFocus`: the focused editor's `(scope, path)`.
    pub fn current_editor_focus(&self, cx: &App) -> (String, String) {
        self.commands
            .focus_provider
            .as_ref()
            .map(|f| f(cx))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unscoped_commands_are_global() {
        assert!(command_matches("", "", "source.go", "main.go"));
    }

    #[test]
    fn scopes_or_and_glob_ands() {
        let scope = "text.html, source.svelte, source.vue, source.tsx, source.js.jsx, source.astro";
        let glob = "*.{html,htm,svelte,vue,tsx,jsx,astro,mdx,hbs,handlebars}";
        let m = |s: &str, p: &str| command_matches(scope, glob, s, p);
        assert!(m("source.svelte", "App.svelte"));
        assert!(m("text.html.basic", "index.html"));
        assert!(m("source.tsx", "App.tsx"));
        assert!(m("source.js.jsx", "App.jsx"));
        assert!(m("source.astro", "Page.astro"));
        assert!(m("text.html.markdown", "page.mdx"));
        assert!(!m("source.go", "main.go"));
        assert!(!m("source.ts", "app.ts"));
        assert!(!m("source.js", "app.js"));
        assert!(!m("text.html.markdown", "readme.md"));
        assert!(!m("source.svelte", "App.go"));
    }

    #[test]
    fn globs() {
        assert!(glob_match("**/*.go", "a/b/main.go"));
        assert!(glob_match("src/*.ts", "src/app.ts"));
        assert!(!glob_match("src/*.ts", "src/a/app.ts"));
        assert!(glob_match("?.md", "a.md"));
        assert!(!glob_match("?.md", "ab.md"));
        assert!(glob_match("**/test/**", "x/test/y/z.rs"));
    }

    #[test]
    fn tsx_and_jsx_keep_their_scopes() {
        assert_eq!(scope_for_path("src/App.tsx", None), "source.tsx");
        assert_eq!(scope_for_path("src/App.jsx", None), "source.js.jsx");
        assert_eq!(scope_for_path("src/app.ts", None), "source.ts");
        assert_eq!(scope_for_path("src/app.js", None), "source.js");
        assert_eq!(scope_for_path("x.go", Some("source.go")), "source.go");
    }

    #[test]
    fn output_routing() {
        assert_eq!(validate_output_routing("insertText"), None);
        assert!(
            validate_output_routing("showAsHTML")
                .unwrap()
                .contains("was removed")
        );
        assert_eq!(
            validate_output_routing("nope").as_deref(),
            Some("unknown output \"nope\"")
        );
    }

    #[test]
    fn scratch_languages() {
        assert_eq!(scratch_language(".JSON"), "json");
        assert_eq!(scratch_language("c++"), "txt");
        assert_eq!(scratch_language(""), "txt");
        assert_eq!(scratch_language("x".repeat(17).as_str()), "txt");
    }

    #[test]
    fn input_fallback() {
        let cmd = RegisteredCommand {
            id: "c".into(),
            label: "c".into(),
            category: "Commands".into(),
            order: 0,
            scope: String::new(),
            file_glob: String::new(),
            triggers: vec!["palette".into()],
            prompts: vec![],
            input: "selection".into(),
            fallback_input: "document".into(),
            output: "replaceSelection".into(),
            interpreter: "shebang".into(),
            bundle_name: String::new(),
            key_equivalent_raw: String::new(),
            key_equivalent: String::new(),
            key_equivalent_display: String::new(),
            language: String::new(),
            open_in_preview: false,
        };
        let ctx = EditorContext {
            document: "doc".into(),
            ..EditorContext::default()
        };
        assert_eq!(
            resolve_input_text(&cmd, &ctx),
            ("doc".to_string(), "document".to_string())
        );
    }
}
