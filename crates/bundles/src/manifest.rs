//! `bundle.toml` schema: `[bundle]`, `[[command]]`, `[[prompt]]`,
//! `[[search]]`, `[[expand]]`, `[[languageServer]]`, plus the per-kind
//! parsers that live in their own modules.

use std::sync::LazyLock;
use std::time::Duration;

use recur_domain::path;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::grammar::{
    Grammar, TerminalTheme, Theme, parse_grammars, parse_terminal_themes, parse_themes,
};
use crate::langdata::{LanguageData, parse_language_data};
use crate::loops::{LoopDef, Policy, Role, parse_loops, parse_policies, parse_roles};
use crate::setting::{Setting, parse_settings};
use crate::snippet::{Snippet, parse_snippets};
use crate::symbol::{Symbol, SymbolToml, parse_symbols};
use crate::{Error, err};

/// Default prompt action timeout.
pub const PROMPT_TIMEOUT: Duration = Duration::from_secs(5);
/// Ceiling for `timeoutMs` on prompts, searches and expanders.
pub const PROMPT_TIMEOUT_MAX_MS: i64 = 30000;
/// Prompt `display` is coerced to one line of at most this many characters.
pub const PROMPT_DISPLAY_MAX_LEN: usize = 120;
/// Default timeout for complete scripts and `[[search]]`.
pub const PROMPT_COMPLETE_TIMEOUT: Duration = Duration::from_secs(4);
/// At most this many complete hits are kept.
pub const PROMPT_COMPLETE_MAX_HITS: usize = 25;
/// Default timeout for `[[expand]]`.
pub const EXPAND_TIMEOUT: Duration = Duration::from_secs(15);
const PROMPT_ICON_MAX_BYTES: usize = 16 << 10;

/// Prompt modes (optional manifest field; omit to stay on the current chat).
pub const PROMPT_MODE_ASK: &str = "ask";
pub const PROMPT_MODE_PLAN: &str = "plan";
pub const PROMPT_MODE_AGENT: &str = "agent";
pub const PROMPT_MODE_REVIEW: &str = "review";
pub const PROMPT_MODE_LOOP: &str = "loop";

pub(crate) const PROMPT_SLASH_PATTERN: &str = "^[a-z][a-z0-9-]*$";
pub(crate) const PROMPT_ARG_NAME_PATTERN: &str = "^[a-z][a-z0-9_]{0,39}$";
pub(crate) static PROMPT_SLASH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(PROMPT_SLASH_PATTERN).expect("slash pattern"));
pub(crate) static PROMPT_ARG_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(PROMPT_ARG_NAME_PATTERN).expect("arg pattern"));
static SAFE_ID_CHARS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\p{L}\p{Nd}._-]*$").expect("id chars"));

/// Maximum `[[prompt.arg]]` entries per prompt.
pub const PROMPT_ARG_MAX: usize = 8;

/// Input kinds declare what text a command receives on stdin / as selected text.
pub const INPUT_NONE: &str = "none";
pub const INPUT_SELECTION: &str = "selection";
pub const INPUT_DOCUMENT: &str = "document";
pub const INPUT_LINE: &str = "line";
pub const INPUT_WORD: &str = "word";

/// Output routing tells the frontend how to apply a command's stdout.
pub const OUTPUT_DISCARD: &str = "discard";
pub const OUTPUT_REPLACE_SELECTION: &str = "replaceSelection";
pub const OUTPUT_REPLACE_DOCUMENT: &str = "replaceDocument";
pub const OUTPUT_REPLACE_INPUT: &str = "replaceInput";
pub const OUTPUT_INSERT_TEXT: &str = "insertText";
pub const OUTPUT_INSERT_SNIPPET: &str = "insertSnippet";
pub const OUTPUT_NEW_DOCUMENT: &str = "newDocument";
pub const OUTPUT_SHOW_AS_TOOLTIP: &str = "showAsTooltip";
pub const OUTPUT_COPY_TO_CLIPBOARD: &str = "copyToClipboard";
pub const OUTPUT_SHOW_AS_TOAST: &str = "showAsToast";

/// Interpreter kinds. `native` is rejected (use typescript).
pub const INTERPRETER_SHEBANG: &str = "shebang";
pub const INTERPRETER_NATIVE: &str = "native";
pub const INTERPRETER_TYPESCRIPT: &str = "typescript";

/// Command trigger names.
pub const TRIGGER_PALETTE: &str = "palette";
pub const TRIGGER_WILL_SAVE: &str = "willSave";

/// Rejects empty or path-like ids so cache filenames cannot escape the
/// cache directory (e.g. `id = "../../../../tmp/evil"`).
pub(crate) fn valid_bundle_id(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() || s.len() > 80 {
        return false;
    }
    if s.contains("..") || s.contains('/') || s.contains('\\') {
        return false;
    }
    if path::base(s) != s {
        return false;
    }
    SAFE_ID_CHARS.is_match(s)
}

/// The `[bundle]` table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BundleMeta {
    pub name: String,
    pub id: String,
    pub version: String,
    pub author: String,
}

/// A user-input field collected before a command runs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prompt {
    pub name: String,
    pub label: String,
    /// text | choice | password
    #[serde(rename = "type")]
    pub kind: String,
    pub default: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
}

/// One `[[command]]` entry after load (body resolved if `script =` was set).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Command {
    pub id: String,
    pub name: String,
    pub category: String,
    pub order: i64,
    pub scope: String,
    #[serde(rename = "fileGlob")]
    pub file_glob: String,
    pub input: String,
    #[serde(rename = "fallbackInput")]
    pub fallback_input: String,
    pub output: String,
    pub triggers: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub prompts: Vec<Prompt>,
    #[serde(rename = "keyEquivalent")]
    pub key_equivalent: String,
    /// Scratch-tab file extension for newDocument output (e.g. "diff", "json").
    pub language: String,
    /// Opens a newDocument scratch on the Preview chip when the language
    /// supports it (html, markdown, svg, …). Ignored for other outputs.
    #[serde(rename = "openInPreview")]
    pub open_in_preview: bool,
    pub interpreter: String,
    #[serde(skip_serializing)]
    pub body: String,
    #[serde(skip_serializing)]
    pub script: String,

    #[serde(skip_deserializing, rename = "bundleName")]
    pub bundle_name: String,
    #[serde(skip_deserializing, rename = "bundleID")]
    pub bundle_id: String,
    /// "user" | "default"
    #[serde(skip_deserializing, rename = "sourceRoot")]
    pub source_root: String,
    #[serde(skip_deserializing, rename = "bundleDir")]
    pub bundle_dir: String,
    /// Absolute path when `script` was a pointer; empty for an inline body.
    #[serde(skip)]
    pub script_path: String,
}

/// One `[[prompt]]` entry: a chat prompt action (not a command [`Prompt`]
/// input field). `body` is resolved when `script =` was set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PromptAction {
    pub id: String,
    pub name: String,
    pub description: String,
    pub slash: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mode: String,
    #[serde(rename = "loop", skip_serializing_if = "String::is_empty")]
    pub loop_id: String,
    #[serde(rename = "timeoutMs", skip_serializing_if = "is_zero")]
    pub timeout_ms: i64,
    #[serde(
        rename(deserialize = "fresh_session", serialize = "freshSession"),
        skip_serializing_if = "is_false"
    )]
    pub fresh_session: bool,
    /// Shared chat-tab slot. Empty: nearest idle tab in `mode` (or create).
    /// Set: nearest idle tab already in this group (or create).
    #[serde(
        rename(deserialize = "tab_group", serialize = "tabGroup"),
        skip_serializing_if = "String::is_empty"
    )]
    pub tab_group: String,
    pub interpreter: String,
    /// Picking from the `/` menu inserts a command plus arg slots instead of
    /// invoking. Implied when `[[prompt.arg]]` is set.
    #[serde(rename = "takesArgs", skip_serializing_if = "is_false")]
    pub takes_args: bool,
    /// Empty-args hint (also the first arg's placeholder when
    /// `[[prompt.arg]]` omits one).
    #[serde(rename = "argPlaceholder", skip_serializing_if = "String::is_empty")]
    pub arg_placeholder: String,
    /// Named trailing parameters (`[[prompt.arg]]`).
    #[serde(
        rename(deserialize = "arg", serialize = "args"),
        skip_serializing_if = "Vec::is_empty"
    )]
    pub args: Vec<PromptArgSpec>,
    /// Optional pack-relative script suggesting trailing-arg hits. `.ts` /
    /// `.mts` / `.js` / `.mjs` run in the Worker, anything else is shebang.
    #[serde(skip_serializing)]
    pub complete: String,
    /// Pack-relative `.svg` for complete (and ⌘K) rows.
    #[serde(skip_serializing)]
    pub icon: String,
    #[serde(skip_serializing)]
    pub body: String,
    #[serde(skip_serializing)]
    pub script: String,

    #[serde(skip_deserializing, rename = "bundleName")]
    pub bundle_name: String,
    #[serde(skip_deserializing, rename = "bundleID")]
    pub bundle_id: String,
    #[serde(skip_deserializing, rename = "sourceRoot")]
    pub source_root: String,
    #[serde(skip_deserializing, rename = "bundleDir")]
    pub bundle_dir: String,
    #[serde(skip)]
    pub script_path: String,
    #[serde(skip)]
    pub complete_body: String,
    #[serde(skip)]
    pub complete_path: String,
    #[serde(
        skip_deserializing,
        rename = "completeInterpreter",
        skip_serializing_if = "String::is_empty"
    )]
    pub complete_interpreter: String,
    /// Themed markup from `icon` (`fill=#…` → `currentColor`).
    #[serde(
        skip_deserializing,
        rename = "iconSvg",
        skip_serializing_if = "String::is_empty"
    )]
    pub icon_svg: String,
}

/// One `[[search]]` entry: ⌘K rows. Shebang only (secrets).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchAction {
    pub id: String,
    pub name: String,
    #[serde(rename = "timeoutMs", skip_serializing_if = "is_zero")]
    pub timeout_ms: i64,
    pub interpreter: String,
    #[serde(skip_serializing)]
    pub script: String,
    #[serde(skip_serializing)]
    pub icon: String,
    #[serde(skip_serializing)]
    pub body: String,

    #[serde(skip_deserializing, rename = "bundleName")]
    pub bundle_name: String,
    #[serde(skip_deserializing, rename = "bundleID")]
    pub bundle_id: String,
    #[serde(skip_deserializing, rename = "sourceRoot")]
    pub source_root: String,
    #[serde(skip_deserializing, rename = "bundleDir")]
    pub bundle_dir: String,
    #[serde(skip)]
    pub script_path: String,
    #[serde(
        skip_deserializing,
        rename = "iconSvg",
        skip_serializing_if = "String::is_empty"
    )]
    pub icon_svg: String,
}

impl SearchAction {
    pub(crate) fn timeout(&self) -> Duration {
        if self.timeout_ms > 0 {
            Duration::from_millis(self.timeout_ms as u64)
        } else {
            PROMPT_COMPLETE_TIMEOUT
        }
    }
}

/// One `[[expand]]` entry: New Task / ⌘K pick → work prompt. Shebang only.
/// An empty `match` means the host never auto-runs this expander.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ExpandAction {
    pub id: String,
    pub name: String,
    #[serde(rename = "match", skip_serializing_if = "String::is_empty")]
    pub match_pattern: String,
    #[serde(rename = "timeoutMs", skip_serializing_if = "is_zero")]
    pub timeout_ms: i64,
    pub interpreter: String,
    #[serde(skip_serializing)]
    pub script: String,
    #[serde(skip_serializing)]
    pub body: String,

    #[serde(skip_deserializing, rename = "bundleName")]
    pub bundle_name: String,
    #[serde(skip_deserializing, rename = "bundleID")]
    pub bundle_id: String,
    #[serde(skip_deserializing, rename = "sourceRoot")]
    pub source_root: String,
    #[serde(skip_deserializing, rename = "bundleDir")]
    pub bundle_dir: String,
    #[serde(skip)]
    pub script_path: String,
    #[serde(skip)]
    pub match_re: Option<Regex>,
}

impl ExpandAction {
    /// True when `match` is set and the trimmed text hits it.
    pub fn matches(&self, text: &str) -> bool {
        self.match_re
            .as_ref()
            .is_some_and(|re| re.is_match(text.trim()))
    }

    pub(crate) fn timeout(&self) -> Duration {
        if self.timeout_ms > 0 {
            Duration::from_millis(self.timeout_ms as u64)
        } else {
            EXPAND_TIMEOUT
        }
    }
}

/// One named `[[prompt.arg]]` slot (Discord-style composer params).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PromptArgSpec {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub placeholder: String,
    /// Empty slot + Enter invokes (scripts treat empty as unset). Default
    /// false keeps a slot required.
    #[serde(skip_serializing_if = "is_false")]
    pub optional: bool,
}

/// One `[[languageServer]]` entry: a long-lived LSP process declaration.
/// Binaries resolve via PATH and/or a managed download keyed by `managed`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LanguageServer {
    pub id: String,
    pub languages: Vec<String>,
    #[serde(rename = "fileGlob", skip_serializing_if = "Vec::is_empty")]
    pub file_glob: Vec<String>,
    pub command: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Pin registry key, e.g. "gopls".
    #[serde(skip_serializing_if = "String::is_empty")]
    pub managed: String,
    /// `KEY=VAL` extras.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<String>,

    #[serde(skip_deserializing, rename = "bundleName")]
    pub bundle_name: String,
    #[serde(skip_deserializing, rename = "bundleID")]
    pub bundle_id: String,
    #[serde(skip_deserializing, rename = "sourceRoot")]
    pub source_root: String,
    #[serde(skip_deserializing, rename = "bundleDir")]
    pub bundle_dir: String,
}

/// Everything one `bundle.toml` declares, validated and resolved.
#[derive(Debug, Clone, Default)]
pub struct ParsedBundle {
    pub meta: BundleMeta,
    pub commands: Vec<Command>,
    pub prompts: Vec<PromptAction>,
    pub grammars: Vec<Grammar>,
    pub themes: Vec<Theme>,
    pub terminal_themes: Vec<TerminalTheme>,
    pub language_data: Vec<LanguageData>,
    pub snippets: Vec<Snippet>,
    pub symbols: Vec<Symbol>,
    pub language_servers: Vec<LanguageServer>,
    pub settings: Vec<Setting>,
    pub searches: Vec<SearchAction>,
    pub expands: Vec<ExpandAction>,
    pub roles: Vec<Role>,
    pub loops: Vec<LoopDef>,
    pub policies: Vec<Policy>,
}

/// On-disk shape of `bundle.toml`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct FileManifest {
    bundle: BundleMeta,
    #[serde(rename = "command")]
    commands: Vec<Command>,
    prompt: Vec<PromptAction>,
    grammar: Vec<Grammar>,
    theme: Vec<Theme>,
    #[serde(rename = "terminalTheme")]
    terminal_theme: Vec<TerminalTheme>,
    #[serde(rename = "languageData")]
    language_data: Vec<LanguageData>,
    snippet: Vec<Snippet>,
    symbol: Vec<SymbolToml>,
    #[serde(rename = "languageServer")]
    language_servers: Vec<LanguageServer>,
    setting: Vec<Setting>,
    search: Vec<SearchAction>,
    expand: Vec<ExpandAction>,
    role: Vec<Role>,
    #[serde(rename = "loop")]
    loops: Vec<LoopDef>,
    policy: Vec<Policy>,
}

/// Reads and validates a `bundle.toml`, resolving script pointers relative
/// to `bundle_dir`. Returns the bundle meta, commands and `[[prompt]]` actions.
pub fn parse_manifest(
    bundle_dir: &str,
    data: &[u8],
) -> Result<(BundleMeta, Vec<Command>, Vec<PromptAction>), Error> {
    let parsed = parse_manifest_full(bundle_dir, data)?;
    Ok((parsed.meta, parsed.commands, parsed.prompts))
}

/// [`parse_manifest`] plus every other contribution kind. Bundles with no
/// commands (grammar-only, theme-only, setting-only, …) are valid.
pub fn parse_manifest_full(bundle_dir: &str, data: &[u8]) -> Result<ParsedBundle, Error> {
    let text =
        std::str::from_utf8(data).map_err(|_| err!("parse bundle.toml: toml: invalid UTF-8"))?;
    let mut fm: FileManifest =
        crate::tomlx::from_str(text).map_err(|e| err!("parse bundle.toml: {e}"))?;
    fm.bundle.id = fm.bundle.id.trim().to_string();
    if fm.bundle.id.is_empty() {
        return Err(err!("bundle.toml: [bundle].id is required"));
    }
    if !valid_bundle_id(&fm.bundle.id) {
        return Err(err!("bundle.toml: [bundle].id is not a safe id"));
    }
    if fm.bundle.name.trim().is_empty() {
        fm.bundle.name = path::base(bundle_dir);
    }
    let meta = fm.bundle;
    let mut commands = Vec::with_capacity(fm.commands.len());
    for (i, c) in fm.commands.into_iter().enumerate() {
        commands.push(parse_command(bundle_dir, &meta, i, c)?);
    }
    let prompts = parse_prompt_actions(bundle_dir, &meta, fm.prompt)?;
    let grammars = parse_grammars(bundle_dir, &meta, fm.grammar)?;
    let themes = parse_themes(bundle_dir, &meta, fm.theme)?;
    let terminal_themes = parse_terminal_themes(bundle_dir, &meta, fm.terminal_theme)?;
    let language_data = parse_language_data(bundle_dir, &meta, fm.language_data)?;
    let snippets = parse_snippets(bundle_dir, &meta, fm.snippet)?;
    let symbols = parse_symbols(bundle_dir, &meta, fm.symbol)?;
    let language_servers = parse_language_servers(bundle_dir, &meta, fm.language_servers)?;
    let settings = parse_settings(bundle_dir, &meta, fm.setting)?;
    let searches = parse_search_actions(bundle_dir, &meta, fm.search)?;
    let expands = parse_expand_actions(bundle_dir, &meta, fm.expand)?;
    let roles = parse_roles(bundle_dir, &meta, fm.role)?;
    let policies = parse_policies(bundle_dir, &meta, fm.policy)?;
    let loops = parse_loops(bundle_dir, &meta, fm.loops)?;
    Ok(ParsedBundle {
        meta,
        commands,
        prompts,
        grammars,
        themes,
        terminal_themes,
        language_data,
        snippets,
        symbols,
        language_servers,
        settings,
        searches,
        expands,
        roles,
        loops,
        policies,
    })
}

fn parse_command(
    bundle_dir: &str,
    meta: &BundleMeta,
    i: usize,
    mut c: Command,
) -> Result<Command, Error> {
    c.id = c.id.trim().to_string();
    if c.id.is_empty() {
        return Err(err!("command[{i}]: id is required"));
    }
    if !valid_bundle_id(&c.id) {
        return Err(err!("command[{i}]: id is not a safe id"));
    }
    let id = format!("{:?}", c.id);
    if c.name.trim().is_empty() {
        return Err(err!("command {id}: name is required"));
    }
    if c.category.is_empty() {
        c.category = meta.name.clone();
    }
    if c.input.is_empty() {
        c.input = INPUT_SELECTION.to_string();
    }
    if !valid_input_kind(&c.input) {
        return Err(err!(
            "command {id}: unknown input {:?} (use none|selection|document|line|word)",
            c.input
        ));
    }
    if !c.fallback_input.is_empty() && !valid_input_kind(&c.fallback_input) {
        return Err(err!(
            "command {id}: unknown fallbackInput {:?} (use none|selection|document|line|word)",
            c.fallback_input
        ));
    }
    if c.output.is_empty() {
        c.output = OUTPUT_REPLACE_SELECTION.to_string();
    }
    validate_output(&c.output).map_err(|e| err!("command {id}: {e}"))?;
    if c.interpreter.is_empty() {
        c.interpreter = INTERPRETER_SHEBANG.to_string();
    }
    match c.interpreter.as_str() {
        INTERPRETER_NATIVE | "js" => {
            return Err(err!(
                "command {id}: interpreter {:?} is not valid (use typescript or shebang)",
                c.interpreter
            ));
        }
        INTERPRETER_SHEBANG | INTERPRETER_TYPESCRIPT => {}
        other => return Err(err!("command {id}: unknown interpreter {:?}", other)),
    }
    if c.triggers.is_empty() {
        c.triggers = vec![TRIGGER_PALETTE.to_string()];
    }
    for tr in &c.triggers {
        if !valid_trigger(tr) {
            return Err(err!(
                "command {id}: unknown trigger {:?} (use palette|willSave)",
                tr
            ));
        }
    }
    let body = c.body.trim();
    let script = c.script.trim().to_string();
    if !body.is_empty() && !script.is_empty() {
        return Err(err!("command {id}: set body or script, not both"));
    }
    if !script.is_empty() {
        let p = path::join(&[bundle_dir, &script]);
        let b = std::fs::read_to_string(&p)
            .map_err(|e| err!("command {id}: read script {:?}: {e}", script))?;
        c.body = b;
        c.script_path = p;
        c.script = script;
    }
    if c.body.trim().is_empty() {
        return Err(err!("command {id}: body or script is required"));
    }
    c.bundle_name = meta.name.clone();
    c.bundle_id = meta.id.clone();
    c.bundle_dir = bundle_dir.to_string();
    Ok(c)
}

fn parse_language_servers(
    bundle_dir: &str,
    meta: &BundleMeta,
    raw: Vec<LanguageServer>,
) -> Result<Vec<LanguageServer>, Error> {
    let mut out = Vec::with_capacity(raw.len());
    for (i, mut s) in raw.into_iter().enumerate() {
        let id = s.id.trim().to_string();
        if id.is_empty() {
            return Err(err!("languageServer[{i}]: id is required"));
        }
        if !valid_bundle_id(&id) {
            return Err(err!("languageServer[{i}]: id {:?} is not a safe id", id));
        }
        if s.command.trim().is_empty() {
            return Err(err!("languageServer {:?}: command is required", id));
        }
        let langs: Vec<String> = s
            .languages
            .iter()
            .map(|l| l.trim().to_lowercase())
            .filter(|l| !l.is_empty())
            .collect();
        if langs.is_empty() {
            return Err(err!(
                "languageServer {:?}: languages must list at least one language id",
                id
            ));
        }
        s.id = id;
        s.languages = langs;
        s.managed = s.managed.trim().to_string();
        s.bundle_name = meta.name.clone();
        s.bundle_id = meta.id.clone();
        s.bundle_dir = bundle_dir.to_string();
        out.push(s);
    }
    Ok(out)
}

fn parse_prompt_actions(
    bundle_dir: &str,
    meta: &BundleMeta,
    raw: Vec<PromptAction>,
) -> Result<Vec<PromptAction>, Error> {
    let mut out = Vec::with_capacity(raw.len());
    for (i, mut p) in raw.into_iter().enumerate() {
        p.id = p.id.trim().to_string();
        if p.id.is_empty() {
            return Err(err!("prompt[{i}]: id is required"));
        }
        if !valid_bundle_id(&p.id) {
            return Err(err!("prompt[{i}]: id is not a safe id"));
        }
        let id = format!("{:?}", p.id);
        if p.name.trim().is_empty() {
            return Err(err!("prompt {id}: name is required"));
        }
        let mut slash = p.slash.trim().to_string();
        if slash.is_empty() {
            slash = kebab_from_name(&p.name);
        }
        if !PROMPT_SLASH_RE.is_match(&slash) {
            return Err(err!(
                "prompt {id}: slash {:?} must match {PROMPT_SLASH_PATTERN}",
                slash
            ));
        }
        p.slash = slash;
        let mut mode = p.mode.trim().to_string();
        let loop_id = p.loop_id.trim().to_string();
        if !loop_id.is_empty() {
            if !valid_bundle_id(&loop_id) {
                return Err(err!("prompt {id}: loop {:?} is not a safe id", loop_id));
            }
            if mode.is_empty() {
                mode = PROMPT_MODE_LOOP.to_string();
            } else if mode != PROMPT_MODE_LOOP {
                return Err(err!(
                    "prompt {id}: loop requires mode {:?}",
                    PROMPT_MODE_LOOP
                ));
            }
        }
        if !mode.is_empty() && !valid_prompt_mode(&mode) {
            return Err(err!(
                "prompt {id}: unknown mode {:?} (use ask|plan|agent|review|loop, or omit)",
                mode
            ));
        }
        p.mode = mode;
        p.loop_id = loop_id;
        if p.fresh_session && p.mode == PROMPT_MODE_LOOP {
            return Err(err!(
                "prompt {id}: fresh_session is not valid with mode {:?}",
                PROMPT_MODE_LOOP
            ));
        }
        let tab_group = p.tab_group.trim().to_string();
        if !tab_group.is_empty() {
            if p.mode.is_empty() {
                return Err(err!("prompt {id}: tab_group requires mode"));
            }
            if !PROMPT_SLASH_RE.is_match(&tab_group) {
                return Err(err!(
                    "prompt {id}: tab_group {:?} must match {PROMPT_SLASH_PATTERN}",
                    tab_group
                ));
            }
        }
        p.tab_group = tab_group;
        if p.timeout_ms < 0 || p.timeout_ms > PROMPT_TIMEOUT_MAX_MS {
            return Err(err!(
                "prompt {id}: timeoutMs must be 0 or 1..{PROMPT_TIMEOUT_MAX_MS}"
            ));
        }
        if p.interpreter.is_empty() {
            p.interpreter = INTERPRETER_SHEBANG.to_string();
        }
        match p.interpreter.as_str() {
            INTERPRETER_NATIVE | "js" => {
                return Err(err!(
                    "prompt {id}: interpreter {:?} is not valid (use typescript or shebang)",
                    p.interpreter
                ));
            }
            INTERPRETER_SHEBANG | INTERPRETER_TYPESCRIPT => {}
            other => return Err(err!("prompt {id}: unknown interpreter {:?}", other)),
        }
        let body = p.body.trim().to_string();
        let script = p.script.trim().to_string();
        if !body.is_empty() && !script.is_empty() {
            return Err(err!("prompt {id}: set body or script, not both"));
        }
        if body.is_empty() && script.is_empty() {
            return Err(err!("prompt {id}: body or script is required"));
        }
        if !script.is_empty() {
            let path = path::join(&[bundle_dir, &script]);
            let b = std::fs::read_to_string(&path)
                .map_err(|e| err!("prompt {id}: read script {:?}: {e}", script))?;
            p.body = b;
            p.script_path = path;
            p.script = script;
        }
        if p.body.trim().is_empty() {
            return Err(err!("prompt {id}: body or script is required"));
        }
        normalize_prompt_args(&mut p)?;
        resolve_prompt_complete(&mut p, bundle_dir)?;
        let svg = resolve_pack_icon("prompt", &p.id, &p.icon, bundle_dir)?;
        p.icon = p.icon.trim().to_string();
        p.icon_svg = svg;
        p.arg_placeholder = p.arg_placeholder.trim().to_string();
        if p.arg_placeholder.chars().count() > 200 {
            return Err(err!("prompt {id}: argPlaceholder is too long"));
        }
        p.bundle_name = meta.name.clone();
        p.bundle_id = meta.id.clone();
        p.bundle_dir = bundle_dir.to_string();
        out.push(p);
    }
    Ok(out)
}

fn parse_search_actions(
    bundle_dir: &str,
    meta: &BundleMeta,
    raw: Vec<SearchAction>,
) -> Result<Vec<SearchAction>, Error> {
    let mut out = Vec::with_capacity(raw.len());
    for (i, mut s) in raw.into_iter().enumerate() {
        s.id = s.id.trim().to_string();
        if s.id.is_empty() {
            return Err(err!("search[{i}]: id is required"));
        }
        if !valid_bundle_id(&s.id) {
            return Err(err!("search[{i}]: id is not a safe id"));
        }
        let id = format!("{:?}", s.id);
        if s.name.trim().is_empty() {
            return Err(err!("search {id}: name is required"));
        }
        if s.timeout_ms < 0 || s.timeout_ms > PROMPT_TIMEOUT_MAX_MS {
            return Err(err!(
                "search {id}: timeoutMs must be 0 or 1..{PROMPT_TIMEOUT_MAX_MS}"
            ));
        }
        require_shebang_script(
            "search",
            &s.id.clone(),
            &mut s.interpreter,
            &mut s.body,
            &mut s.script,
            &mut s.script_path,
            bundle_dir,
        )?;
        let svg = resolve_pack_icon("search", &s.id, &s.icon, bundle_dir)?;
        s.icon = s.icon.trim().to_string();
        s.icon_svg = svg;
        s.bundle_name = meta.name.clone();
        s.bundle_id = meta.id.clone();
        s.bundle_dir = bundle_dir.to_string();
        out.push(s);
    }
    Ok(out)
}

fn parse_expand_actions(
    bundle_dir: &str,
    meta: &BundleMeta,
    raw: Vec<ExpandAction>,
) -> Result<Vec<ExpandAction>, Error> {
    let mut out = Vec::with_capacity(raw.len());
    for (i, mut e) in raw.into_iter().enumerate() {
        e.id = e.id.trim().to_string();
        if e.id.is_empty() {
            return Err(err!("expand[{i}]: id is required"));
        }
        if !valid_bundle_id(&e.id) {
            return Err(err!("expand[{i}]: id is not a safe id"));
        }
        let id = format!("{:?}", e.id);
        if e.name.trim().is_empty() {
            return Err(err!("expand {id}: name is required"));
        }
        if e.timeout_ms < 0 || e.timeout_ms > PROMPT_TIMEOUT_MAX_MS {
            return Err(err!(
                "expand {id}: timeoutMs must be 0 or 1..{PROMPT_TIMEOUT_MAX_MS}"
            ));
        }
        require_shebang_script(
            "expand",
            &e.id.clone(),
            &mut e.interpreter,
            &mut e.body,
            &mut e.script,
            &mut e.script_path,
            bundle_dir,
        )?;
        e.match_pattern = e.match_pattern.trim().to_string();
        if !e.match_pattern.is_empty() {
            let re = regex::Regex::new(&e.match_pattern)
                .map_err(|msg| err!("expand {id}: match: {msg}"))?;
            e.match_re = Some(re);
        }
        e.bundle_name = meta.name.clone();
        e.bundle_id = meta.id.clone();
        e.bundle_dir = bundle_dir.to_string();
        out.push(e);
    }
    Ok(out)
}

fn require_shebang_script(
    kind: &str,
    id: &str,
    interpreter: &mut String,
    body: &mut String,
    script: &mut String,
    script_path: &mut String,
    bundle_dir: &str,
) -> Result<(), Error> {
    let id = format!("{id:?}");
    let mut interp = interpreter.trim().to_string();
    if interp.is_empty() {
        interp = INTERPRETER_SHEBANG.to_string();
    }
    if interp != INTERPRETER_SHEBANG {
        return Err(err!("{kind} {id}: interpreter must be shebang"));
    }
    *interpreter = interp;
    let b = body.trim();
    let sc = script.trim().to_string();
    if !b.is_empty() && !sc.is_empty() {
        return Err(err!("{kind} {id}: set body or script, not both"));
    }
    if b.is_empty() && sc.is_empty() {
        return Err(err!("{kind} {id}: body or script is required"));
    }
    if !sc.is_empty() {
        let path = path::join(&[bundle_dir, &sc]);
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| err!("{kind} {id}: read script {:?}: {e}", sc))?;
        *body = raw;
        *script_path = path;
        *script = sc;
    }
    if body.trim().is_empty() {
        return Err(err!("{kind} {id}: body or script is required"));
    }
    Ok(())
}

/// Lowercases `name` and replaces non-alphanumeric runs with `-`.
pub(crate) fn kebab_from_name(name: &str) -> String {
    let mut b = String::new();
    let mut prev_hyphen = false;
    for r in name.trim().to_lowercase().chars() {
        if r.is_ascii_lowercase() || r.is_ascii_digit() {
            b.push(r);
            prev_hyphen = false;
            continue;
        }
        if !prev_hyphen && !b.is_empty() {
            b.push('-');
            prev_hyphen = true;
        }
    }
    b.trim_matches('-').to_string()
}

fn normalize_prompt_args(p: &mut PromptAction) -> Result<(), Error> {
    let id = format!("{:?}", p.id);
    let mut out: Vec<PromptArgSpec> = Vec::with_capacity(p.args.len());
    let mut seen = std::collections::HashSet::new();
    for (i, a) in p.args.iter().enumerate() {
        let name = a.name.trim().to_lowercase();
        if name.is_empty() {
            return Err(err!("prompt {id}: arg[{i}]: name is required"));
        }
        if !PROMPT_ARG_NAME_RE.is_match(&name) {
            return Err(err!(
                "prompt {id}: arg {:?} must match {PROMPT_ARG_NAME_PATTERN}",
                a.name
            ));
        }
        if !seen.insert(name.clone()) {
            return Err(err!("prompt {id}: duplicate arg {:?}", name));
        }
        let ph = a.placeholder.trim().to_string();
        if ph.chars().count() > 200 {
            return Err(err!("prompt {id}: arg {:?} placeholder is too long", name));
        }
        out.push(PromptArgSpec {
            name,
            placeholder: ph,
            optional: a.optional,
        });
    }
    if out.len() > PROMPT_ARG_MAX {
        return Err(err!("prompt {id}: at most {PROMPT_ARG_MAX} args"));
    }
    if !out.is_empty() {
        p.takes_args = true;
        if p.arg_placeholder.is_empty() {
            p.arg_placeholder = out[0].placeholder.clone();
        }
    } else if p.takes_args {
        out = vec![PromptArgSpec {
            name: "args".to_string(),
            placeholder: p.arg_placeholder.clone(),
            optional: false,
        }];
    }
    p.args = out;
    Ok(())
}

fn resolve_prompt_complete(p: &mut PromptAction, bundle_dir: &str) -> Result<(), Error> {
    let rel = p.complete.trim().to_string();
    p.complete = rel.clone();
    p.complete_body.clear();
    p.complete_path.clear();
    p.complete_interpreter.clear();
    if rel.is_empty() {
        return Ok(());
    }
    let id = format!("{:?}", p.id);
    if !p.takes_args {
        return Err(err!("prompt {id}: complete requires takesArgs"));
    }
    let path = path::join(&[bundle_dir, &rel]);
    let b = std::fs::read_to_string(&path)
        .map_err(|e| err!("prompt {id}: read complete {:?}: {e}", rel))?;
    if b.trim().is_empty() {
        return Err(err!("prompt {id}: complete {:?} is empty", rel));
    }
    p.complete_body = b;
    p.complete_path = path;
    p.complete_interpreter = complete_interpreter_for(&rel).to_string();
    Ok(())
}

static PROMPT_ICON_FILL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"fill[\t\n\x0C\r ]*=[\t\n\x0C\r ]*["']#[0-9A-Fa-f]{3,8}["']"#).expect("fill re")
});
const PROMPT_ICON_BAD: [&str; 7] = [
    "<script",
    "javascript:",
    "onload=",
    "onerror=",
    "<foreignobject",
    "<iframe",
    "<embed",
];

pub(crate) fn resolve_pack_icon(
    kind: &str,
    id: &str,
    rel: &str,
    bundle_dir: &str,
) -> Result<String, Error> {
    let rel = rel.trim();
    if rel.is_empty() {
        return Ok(String::new());
    }
    let id = format!("{id:?}");
    if path::ext(rel).to_lowercase() != ".svg" {
        return Err(err!("{kind} {id}: icon must be a .svg"));
    }
    let path = path::join(&[bundle_dir, rel]);
    let b = std::fs::read(&path).map_err(|e| err!("{kind} {id}: read icon {:?}: {e}", rel))?;
    if b.len() > PROMPT_ICON_MAX_BYTES {
        return Err(err!(
            "{kind} {id}: icon is larger than {PROMPT_ICON_MAX_BYTES} bytes"
        ));
    }
    sanitize_prompt_icon_svg(&String::from_utf8_lossy(&b))
        .map_err(|e| err!("{kind} {id}: icon {:?}: {e}", rel))
}

fn sanitize_prompt_icon_svg(raw: &str) -> Result<String, &'static str> {
    let s = raw.trim();
    let low = s.to_lowercase();
    if !low.contains("<svg") {
        return Err("not an svg");
    }
    if PROMPT_ICON_BAD.iter().any(|bad| low.contains(bad)) {
        return Err("svg is not a static icon");
    }
    // simplify: hardcoded fills → currentColor so dark/light chrome can theme.
    Ok(PROMPT_ICON_FILL_RE
        .replace_all(s, r#"fill="currentColor""#)
        .into_owned())
}

fn complete_interpreter_for(rel: &str) -> &'static str {
    match path::ext(rel).to_lowercase().as_str() {
        ".ts" | ".mts" | ".js" | ".mjs" => INTERPRETER_TYPESCRIPT,
        _ => INTERPRETER_SHEBANG,
    }
}

pub(crate) fn valid_prompt_mode(s: &str) -> bool {
    matches!(
        s,
        PROMPT_MODE_ASK
            | PROMPT_MODE_PLAN
            | PROMPT_MODE_AGENT
            | PROMPT_MODE_REVIEW
            | PROMPT_MODE_LOOP
    )
}

fn valid_input_kind(s: &str) -> bool {
    matches!(
        s,
        INPUT_NONE | INPUT_SELECTION | INPUT_DOCUMENT | INPUT_LINE | INPUT_WORD
    )
}

fn valid_trigger(s: &str) -> bool {
    matches!(s, TRIGGER_PALETTE | TRIGGER_WILL_SAVE)
}

/// Reports whether `s` is a known output routing value. `showAsHTML` is
/// rejected with a migration hint.
pub fn validate_output(s: &str) -> Result<(), Error> {
    match s {
        OUTPUT_DISCARD
        | OUTPUT_REPLACE_SELECTION
        | OUTPUT_REPLACE_DOCUMENT
        | OUTPUT_REPLACE_INPUT
        | OUTPUT_INSERT_TEXT
        | OUTPUT_INSERT_SNIPPET
        | OUTPUT_NEW_DOCUMENT
        | OUTPUT_SHOW_AS_TOOLTIP
        | OUTPUT_COPY_TO_CLIPBOARD
        | OUTPUT_SHOW_AS_TOAST => Ok(()),
        "showAsHTML" => Err(err!(
            "output showAsHTML was removed — use newDocument with language + openInPreview"
        )),
        other => Err(err!("unknown output {:?}", other)),
    }
}

pub(crate) fn is_false(b: &bool) -> bool {
    !*b
}

pub(crate) fn is_zero(n: &i64) -> bool {
    *n == 0
}
