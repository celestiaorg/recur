//! Run contexts and the `RECUR_*` environment handed to shebang scripts.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;

use recur_domain::path;

use crate::manifest::{
    INPUT_DOCUMENT, INPUT_LINE, INPUT_NONE, INPUT_SELECTION, INPUT_WORD, PROMPT_DISPLAY_MAX_LEN,
    PromptAction,
};
use crate::setting::setting_env_name;

/// Editor state passed into a command execution.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunContext {
    pub selected_text: String,
    pub current_line: String,
    pub line_number: i64,
    pub column_number: i64,
    pub current_word: String,
    /// Absolute or workspace-relative path shown to the user.
    pub file_path: String,
    pub file_name: String,
    pub directory: String,
    pub project_directory: String,
    pub scope: String,
    pub tab_size: i64,
    pub soft_tabs: bool,
    pub clipboard: String,
    /// Absolute path to the bundle directory.
    pub bundle_support: String,
    /// Text piped to stdin (resolved from input/fallbackInput).
    pub input: String,
    pub prompt_values: BTreeMap<String, String>,
    /// Process working directory: worktree / project root.
    pub work_dir: String,
    /// Name → value for this command's bundle `[[setting]]` entries.
    pub bundle_settings: BTreeMap<String, String>,
}

/// Workspace + editor state for a shebang `[[prompt]]`, `[[search]]` or
/// `[[expand]]` run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptRunContext {
    pub workspace_id: String,
    pub project_id: String,
    pub worktree: String,
    pub port: i64,

    pub selected_text: String,
    pub document: String,
    pub current_line: String,
    pub line_number: i64,
    pub column: i64,
    pub current_word: String,
    pub file_path: String,
    pub file_name: String,
    pub scope: String,
    pub clipboard: String,
    pub tab_size: i64,
    pub soft_tabs: bool,

    /// Workspace merge base (main, origin/main, …).
    pub base_branch: String,
    /// "merge" or "rebase".
    pub update_base_method: String,
    /// project.toml `conventional_commits`.
    pub conventional_commits: bool,
    /// Open (non-merged) PR for this workspace, if any. `/open-pr`
    /// fast-fails when set so a second invoke does not start an agent turn.
    pub pr_id: String,
    pub pr_number: i64,
    pub pr_source: String,
    /// True when `/open-draft-pr` invoked this shebang.
    pub pr_draft: bool,
    /// Trailing composer remainder after `/slash`. For complete scripts this
    /// is the active slot's value.
    pub args: String,
    /// The `[[prompt.arg]]` name being completed (empty on invoke).
    pub complete_arg: String,
    /// The ⌘K query (`RECUR_SEARCH_QUERY`).
    pub search_query: String,
    /// Named `[[prompt.arg]]` values.
    pub params: BTreeMap<String, String>,
    /// Name → value for this prompt's bundle `[[setting]]` entries.
    pub bundle_settings: BTreeMap<String, String>,
    /// The `[[prompt]]` slash that launched this run.
    pub prompt_slash: String,
    /// Operator's looking-at view: workspace, local_pr, github_pr.
    pub surface: String,
    pub pr_url: String,
    pub pr_title: String,
    pub pr_base: String,
    pub pr_branch: String,
    /// cwd is a PR chat host (project checkout, not the PR head).
    pub pr_host: bool,
    /// Checkpoint commits for a `/re-review` continuing an earlier review.
    pub review_since: String,
    pub review_now: String,

    pub bundle_support: String,
    /// Process working directory, usually the worktree.
    pub work_dir: String,
}

/// One environment entry (`KEY=VALUE`; values may hold arbitrary bytes).
pub type EnvVar = (OsString, OsString);

fn inherited(skip: impl Fn(&[u8]) -> bool) -> Vec<EnvVar> {
    std::env::vars_os()
        .filter(|(k, _)| !skip(k.as_bytes()))
        .collect()
}

fn push(env: &mut Vec<EnvVar>, key: &str, value: impl AsRef<[u8]>) {
    env.push((
        OsString::from(key),
        OsStr::from_bytes(value.as_ref()).to_os_string(),
    ));
}

/// Maps a [`RunContext`] to the `RECUR_*` environment plus the inherited
/// environment with any stale `RECUR_*` stripped.
pub fn environ(rc: &RunContext) -> Vec<EnvVar> {
    let mut env = inherited(|k| k.starts_with(b"RECUR_"));
    let file_name = if rc.file_name.is_empty() && !rc.file_path.is_empty() {
        path::base(&rc.file_path)
    } else {
        rc.file_name.clone()
    };
    let dir = if rc.directory.is_empty() && !rc.file_path.is_empty() {
        path::dir(&rc.file_path)
    } else {
        rc.directory.clone()
    };
    for (k, v) in [
        ("RECUR_SELECTED_TEXT", rc.selected_text.as_str()),
        ("RECUR_CURRENT_LINE", &rc.current_line),
        ("RECUR_LINE_NUMBER", &rc.line_number.to_string()),
        ("RECUR_COLUMN_NUMBER", &rc.column_number.to_string()),
        ("RECUR_CURRENT_WORD", &rc.current_word),
        ("RECUR_FILEPATH", &rc.file_path),
        ("RECUR_FILENAME", &file_name),
        ("RECUR_DIRECTORY", &dir),
        ("RECUR_PROJECT_DIRECTORY", &rc.project_directory),
        ("RECUR_SCOPE", &rc.scope),
        ("RECUR_TAB_SIZE", &rc.tab_size.to_string()),
        ("RECUR_SOFT_TABS", if rc.soft_tabs { "1" } else { "0" }),
        ("RECUR_CLIPBOARD", &rc.clipboard),
        ("RECUR_BUNDLE_SUPPORT", &rc.bundle_support),
    ] {
        push(&mut env, k, v);
    }
    append_bundle_setting_env(&mut env, &rc.bundle_settings);
    for (name, val) in &rc.prompt_values {
        push(
            &mut env,
            &format!("RECUR_INPUT_{}", sanitize_input_name(name)),
            val,
        );
    }
    env
}

/// Large prompt blobs are capped so argv + env stays under the OS
/// `ARG_MAX` (~1MB on macOS). TypeScript prompts see the full document via
/// the Worker JSON context instead.
const PROMPT_ENV_BLOB_MAX: usize = 64 * 1024;

/// Byte-exact truncation: may cut inside a UTF-8 sequence.
fn truncate_prompt_env_blob(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    if b.len() <= PROMPT_ENV_BLOB_MAX {
        return b.to_vec();
    }
    let mut out = b[..PROMPT_ENV_BLOB_MAX].to_vec();
    out.extend_from_slice("\n…(truncated; over PromptEnviron cap)…".as_bytes());
    out
}

/// Splits the trailing remainder into named `[[prompt.arg]]` values. One
/// arg keeps spaces; several split on whitespace (double quotes group, `\`
/// escapes) with the remainder on the last.
pub fn params_from_args(prompt: &PromptAction, args: &str) -> BTreeMap<String, String> {
    let specs = &prompt.args;
    let mut out = BTreeMap::new();
    if specs.is_empty() {
        return out;
    }
    let raw = args.trim();
    if specs.len() == 1 {
        out.insert(specs[0].name.clone(), raw.to_string());
        return out;
    }
    let parts = split_prompt_arg_tokens(raw);
    for (i, spec) in specs.iter().enumerate() {
        let val = if i == specs.len() - 1 {
            parts
                .get(i..)
                .map(|rest| rest.join(" "))
                .unwrap_or_default()
        } else {
            parts.get(i).cloned().unwrap_or_default()
        };
        out.insert(spec.name.clone(), val);
    }
    out
}

fn split_prompt_arg_tokens(s: &str) -> Vec<String> {
    let s = s.trim().as_bytes();
    let is_space = |b: u8| matches!(b, b' ' | b'\t' | b'\n' | b'\r');
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        while i < s.len() && is_space(s[i]) {
            i += 1;
        }
        if i >= s.len() {
            break;
        }
        if s[i] == b'"' {
            i += 1;
            let mut b = Vec::new();
            while i < s.len() {
                if s[i] == b'\\' && i + 1 < s.len() {
                    b.push(s[i + 1]);
                    i += 2;
                    continue;
                }
                if s[i] == b'"' {
                    i += 1;
                    break;
                }
                b.push(s[i]);
                i += 1;
            }
            out.push(String::from_utf8_lossy(&b).into_owned());
            continue;
        }
        let j = (i..s.len()).find(|&j| is_space(s[j])).unwrap_or(s.len());
        out.push(String::from_utf8_lossy(&s[i..j]).into_owned());
        i = j;
    }
    out
}

/// Maps a [`PromptRunContext`] to `RECUR_*` env (workspace vars plus the
/// editor vars of [`environ`]). Stale `RECUR_*` and `PORT` are stripped.
pub fn prompt_environ(rc: &PromptRunContext) -> Vec<EnvVar> {
    let mut env = inherited(|k| k.starts_with(b"RECUR_") || k == b"PORT");
    let file_name = if rc.file_name.is_empty() && !rc.file_path.is_empty() {
        path::base(&rc.file_path)
    } else {
        rc.file_name.clone()
    };
    let t = truncate_prompt_env_blob;
    let mut kv: Vec<(String, Vec<u8>)> = vec![
        ("RECUR_WORKSPACE_ID".into(), rc.workspace_id.clone().into()),
        ("RECUR_PROJECT_ID".into(), rc.project_id.clone().into()),
        ("RECUR_WORKTREE".into(), rc.worktree.clone().into()),
        ("RECUR_SELECTED_TEXT".into(), t(&rc.selected_text)),
        ("RECUR_DOCUMENT".into(), t(&rc.document)),
        ("RECUR_CURRENT_LINE".into(), t(&rc.current_line)),
        (
            "RECUR_LINE_NUMBER".into(),
            rc.line_number.to_string().into(),
        ),
        ("RECUR_COLUMN_NUMBER".into(), rc.column.to_string().into()),
        ("RECUR_CURRENT_WORD".into(), t(&rc.current_word)),
        ("RECUR_FILEPATH".into(), rc.file_path.clone().into()),
        ("RECUR_FILENAME".into(), file_name.into()),
        ("RECUR_SCOPE".into(), rc.scope.clone().into()),
        ("RECUR_TAB_SIZE".into(), rc.tab_size.to_string().into()),
        (
            "RECUR_SOFT_TABS".into(),
            if rc.soft_tabs { "1" } else { "0" }.into(),
        ),
        ("RECUR_CLIPBOARD".into(), t(&rc.clipboard)),
        (
            "RECUR_BUNDLE_SUPPORT".into(),
            rc.bundle_support.clone().into(),
        ),
        ("RECUR_BASE_BRANCH".into(), rc.base_branch.clone().into()),
        (
            "RECUR_UPDATE_BASE_METHOD".into(),
            rc.update_base_method.clone().into(),
        ),
        (
            "RECUR_CONVENTIONAL_COMMITS".into(),
            if rc.conventional_commits { "1" } else { "0" }.into(),
        ),
        ("RECUR_PR_ID".into(), rc.pr_id.clone().into()),
        ("RECUR_PR_NUMBER".into(), rc.pr_number.to_string().into()),
        ("RECUR_PR_SOURCE".into(), rc.pr_source.clone().into()),
        ("RECUR_PR_URL".into(), rc.pr_url.clone().into()),
        ("RECUR_PR_TITLE".into(), rc.pr_title.clone().into()),
        ("RECUR_PR_BASE".into(), rc.pr_base.clone().into()),
        ("RECUR_PR_BRANCH".into(), rc.pr_branch.clone().into()),
        (
            "RECUR_PR_DRAFT".into(),
            if rc.pr_draft { "1" } else { "0" }.into(),
        ),
        ("RECUR_PROMPT_SLASH".into(), rc.prompt_slash.clone().into()),
        ("RECUR_PROMPT_ARGS".into(), t(&rc.args)),
        (
            "RECUR_PROMPT_COMPLETE_ARG".into(),
            rc.complete_arg.trim().into(),
        ),
        ("RECUR_SEARCH_QUERY".into(), t(&rc.search_query)),
        ("RECUR_SURFACE".into(), rc.surface.clone().into()),
    ];
    if !rc.params.is_empty() {
        let mut obj = BTreeMap::new();
        for (name, val) in &rc.params {
            let val = val.trim();
            obj.insert(name.as_str(), val);
            kv.push((format!("RECUR_PROMPT_ARG_{}", name.to_uppercase()), t(val)));
        }
        if let Ok(json) = serde_json::to_string(&obj) {
            kv.push(("RECUR_PROMPT_PARAMS".into(), t(&json)));
        }
    }
    if rc.pr_host {
        kv.push(("RECUR_PR_HOST".into(), b"1".to_vec()));
    }
    if !rc.review_since.is_empty() && !rc.review_now.is_empty() {
        kv.push(("RECUR_REVIEW_SINCE".into(), rc.review_since.clone().into()));
        kv.push(("RECUR_REVIEW_NOW".into(), rc.review_now.clone().into()));
    }
    if rc.port > 0 {
        let p = rc.port.to_string();
        kv.push(("RECUR_PORT".into(), p.clone().into()));
        kv.push(("PORT".into(), p.into()));
    }
    for (k, v) in kv {
        push(&mut env, &k, v);
    }
    append_bundle_setting_env(&mut env, &rc.bundle_settings);
    env
}

fn append_bundle_setting_env(env: &mut Vec<EnvVar>, values: &BTreeMap<String, String>) {
    let mut obj = BTreeMap::new();
    for (name, val) in values {
        let val = val.trim();
        if val.is_empty() {
            continue;
        }
        obj.insert(name.as_str(), val);
        push(env, &setting_env_name(name), truncate_prompt_env_blob(val));
    }
    if obj.is_empty() {
        return;
    }
    if let Ok(json) = serde_json::to_string(&obj) {
        push(
            env,
            "RECUR_BUNDLE_SETTINGS",
            truncate_prompt_env_blob(&json),
        );
    }
}

/// Looks up `key` in an environment list (last entry wins, like exec).
pub fn env_lookup<'a>(env: &'a [EnvVar], key: &str) -> Option<&'a OsStr> {
    env.iter()
        .rev()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_os_str())
}

/// Coerces `display` to a single line of at most [`PROMPT_DISPLAY_MAX_LEN`]
/// characters. Empty display falls back to `fallback_name`.
pub fn normalize_prompt_display(display: &str, fallback_name: &str) -> String {
    let mut d = display.trim();
    if d.is_empty() {
        d = fallback_name.trim();
    }
    let mut d = d.replace("\r\n", " ").replace(['\n', '\r'], " ");
    while d.contains("  ") {
        d = d.replace("  ", " ");
    }
    let d = d.trim();
    let runes: Vec<char> = d.chars().collect();
    if runes.len() <= PROMPT_DISPLAY_MAX_LEN {
        return d.to_string();
    }
    let mut out: String = runes[..PROMPT_DISPLAY_MAX_LEN - 1].iter().collect();
    out.push('…');
    out
}

/// Uppercases and replaces each non-alphanumeric rune with `_`.
fn sanitize_input_name(name: &str) -> String {
    let out: String = name
        .to_uppercase()
        .chars()
        .map(|r| {
            if r.is_ascii_uppercase() || r.is_ascii_digit() {
                r
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        "VALUE".to_string()
    } else {
        out
    }
}

/// Picks stdin text from the command's input/fallbackInput modes. A
/// UI-resolved `rc.input` wins.
pub fn resolve_input(cmd: &crate::Command, rc: &RunContext) -> String {
    if !rc.input.is_empty() {
        return rc.input.clone();
    }
    let pick = |mode: &str| -> String {
        match mode {
            INPUT_NONE | "" => String::new(),
            INPUT_SELECTION => rc.selected_text.clone(),
            // The UI sets `input` for full documents.
            INPUT_DOCUMENT => rc.selected_text.clone(),
            INPUT_LINE => rc.current_line.clone(),
            INPUT_WORD => rc.current_word.clone(),
            _ => rc.selected_text.clone(),
        }
    };
    let mut v = pick(&cmd.input);
    if v.is_empty() && !cmd.fallback_input.is_empty() {
        v = pick(&cmd.fallback_input);
    }
    v
}
