//! `tests/*.toml` cases next to a bundle (command XOR snippet).

use std::collections::BTreeMap;

use recur_domain::path;
use serde::Deserialize;

use crate::manifest::Command;
use crate::{Error, err};

/// One `[[test]]` entry. Text payloads are inline strings or `*File` paths
/// relative to the TOML file's directory (not both).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct TestCase {
    pub name: String,
    /// Command id (preferred) or name.
    pub command: String,
    /// Snippet id; mutually exclusive with `command`.
    pub snippet: String,
    pub context: TestContext,
    pub expect: TestExpect,
}

/// Editor state fed to the command (MacroContext / RunContext).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct TestContext {
    pub input: String,
    #[serde(rename = "inputFile")]
    pub input_file: String,
    #[serde(rename = "selectedText")]
    pub selected_text: String,
    #[serde(rename = "selectedTextFile")]
    pub selected_text_file: String,
    pub document: String,
    #[serde(rename = "documentFile")]
    pub document_file: String,
    #[serde(rename = "currentLine")]
    pub current_line: String,
    #[serde(rename = "currentLineFile")]
    pub current_line_file: String,
    #[serde(rename = "currentWord")]
    pub current_word: String,
    #[serde(rename = "currentWordFile")]
    pub current_word_file: String,
    #[serde(rename = "lineNumber")]
    pub line_number: i64,
    pub column: i64,
    #[serde(rename = "filepath")]
    pub file_path: String,
    #[serde(rename = "filename")]
    pub file_name: String,
    pub scope: String,
    pub clipboard: String,
    #[serde(rename = "clipboardFile")]
    pub clipboard_file: String,
    #[serde(rename = "tabSize")]
    pub tab_size: i64,
    #[serde(rename = "softTabs")]
    pub soft_tabs: bool,
    pub prompts: BTreeMap<String, String>,
}

/// The assertion block for a case.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct TestExpect {
    pub stdout: String,
    #[serde(rename = "stdoutFile")]
    pub stdout_file: String,
    pub stderr: String,
    #[serde(rename = "stderrFile")]
    pub stderr_file: String,
    /// Optional routing check.
    pub output: String,
    /// Default 0.
    #[serde(rename = "exitCode")]
    pub exit_code: Option<i64>,
    /// For non-deterministic commands (e.g. shuffle).
    #[serde(rename = "skipStdout")]
    pub skip_stdout: bool,
    /// Assert TypeScript MacroResult fields (optional).
    pub language: String,
    #[serde(rename = "openInPreview")]
    pub open_in_preview: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct TestFile {
    test: Vec<TestCase>,
}

/// A case plus the TOML file it came from (for reporting).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadedTest {
    pub file: String,
    pub case: TestCase,
}

/// Reads `tests/*.toml` under `bundle_dir`. A missing `tests/` is not an
/// error. `*File` payloads resolve relative to each TOML file.
pub fn load_tests(bundle_dir: &str) -> Result<Vec<LoadedTest>, Error> {
    let dir = path::join(&[bundle_dir, "tests"]);
    let entries = match crate::registry::read_dir_sorted(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(err!("read tests/: {}", format!("{dir}: {e}"))),
    };
    let mut out = Vec::new();
    for (name, is_dir) in entries {
        if is_dir || !name.ends_with(".toml") {
            continue;
        }
        let path = path::join(&[&dir, &name]);
        let data = std::fs::read(&path).map_err(|e| err!("read {path}: {e}"))?;
        let text =
            std::str::from_utf8(&data).map_err(|_| err!("parse {path}: toml: invalid UTF-8"))?;
        let tf: TestFile = crate::tomlx::from_str(text).map_err(|e| err!("parse {path}: {e}"))?;
        let base = path::dir(&path);
        for (i, mut tc) in tf.test.into_iter().enumerate() {
            let has_command = !tc.command.trim().is_empty();
            let has_snippet = !tc.snippet.trim().is_empty();
            if !has_command && !has_snippet {
                return Err(err!("{path}: test[{i}]: command or snippet is required"));
            }
            if has_command && has_snippet {
                return Err(err!("{path}: test[{i}]: set command or snippet, not both"));
            }
            if tc.name.trim().is_empty() {
                tc.name = format!("{name}#{}", i + 1);
            }
            resolve_test_files(&mut tc, &base)
                .map_err(|e| err!("{path}: test {:?}: {e}", tc.name))?;
            out.push(LoadedTest {
                file: path.clone(),
                case: tc,
            });
        }
    }
    Ok(out)
}

fn resolve_test_files(tc: &mut TestCase, base: &str) -> Result<(), String> {
    let c = &mut tc.context;
    for (inline, file, field) in [
        (&mut c.input, &mut c.input_file, "input"),
        (
            &mut c.selected_text,
            &mut c.selected_text_file,
            "selectedText",
        ),
        (&mut c.document, &mut c.document_file, "document"),
        (&mut c.current_line, &mut c.current_line_file, "currentLine"),
        (&mut c.current_word, &mut c.current_word_file, "currentWord"),
        (&mut c.clipboard, &mut c.clipboard_file, "clipboard"),
        (&mut tc.expect.stdout, &mut tc.expect.stdout_file, "stdout"),
        (&mut tc.expect.stderr, &mut tc.expect.stderr_file, "stderr"),
    ] {
        *inline = resolve_payload(inline, file, base, field)?;
        file.clear();
    }
    Ok(())
}

/// Inline text, or the contents of `rel_file` (relative to `base_dir`).
fn resolve_payload(
    inline: &str,
    rel_file: &str,
    base_dir: &str,
    field: &str,
) -> Result<String, String> {
    let rel_file = rel_file.trim();
    if rel_file.is_empty() {
        return Ok(inline.to_string());
    }
    if !inline.is_empty() {
        return Err(format!(
            "{field}: set inline value or {field}File, not both"
        ));
    }
    if path::is_abs(rel_file) {
        return Err(format!("{field}File must be relative (got {:?})", rel_file));
    }
    let path = path::clean(&path::join(&[base_dir, rel_file]));
    let base_clean = path::clean(base_dir);
    if path != base_clean && !path.starts_with(&format!("{base_clean}/")) {
        return Err(format!("{field}File {:?} escapes test directory", rel_file));
    }
    let data = std::fs::read(&path).map_err(|e| format!("{field}File {:?}: {e}", rel_file))?;
    Ok(String::from_utf8_lossy(&data).into_owned())
}

pub(crate) fn find_command<'a>(cmds: &'a [Command], id: &str) -> Option<&'a Command> {
    cmds.iter().find(|c| c.id == id || c.name == id)
}
