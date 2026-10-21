//! A port of the parts of chroma (`github.com/alecthomas/chroma/v2`) Recur
//! uses: its lexer registry and regex lexers, so diff lines tokenize exactly
//! as they did in Go. The lexer definitions are chroma's own, vendored under
//! `chroma/` (MIT, see `chroma/COPYING`).

mod custom;
mod glob;
mod lexer;
mod raku;
mod regexp2;
mod registry;
#[allow(dead_code)]
mod token;
mod xml;

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

pub(crate) use lexer::{Lexer, Token};
pub(crate) use regexp2::{is_letter, is_number, to_upper};
pub(crate) use token::TokenType;

static LEXER_CACHE: LazyLock<Mutex<HashMap<String, Option<Arc<Lexer>>>>> =
    LazyLock::new(Default::default);

/// The lexer chroma matches for `path`'s base name, cached by base name.
pub(crate) fn lexer_for(path: &str) -> Option<Arc<Lexer>> {
    let mut key = crate::gopath::base(path);
    if key.is_empty() {
        key = path.to_string();
    }
    if let Some(hit) = LEXER_CACHE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
    {
        return hit.clone();
    }
    let lexer = registry::registry().match_filename(&key);
    LEXER_CACHE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key, lexer.clone());
    lexer
}

/// `chroma.Coalesce(lexer).Tokenise(nil, text)`.
pub(crate) fn tokenise(lexer: &Lexer, text: &str) -> Result<Vec<Token>, String> {
    lexer
        .tokenise(None, text)
        .map(lexer::coalesce)
        .map_err(|e| e.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// chroma `TestCompileAllRegexes`: every registered lexer compiles its
    /// rules and tokenises empty input.
    #[test]
    fn every_lexer_compiles() {
        let mut failures = Vec::new();
        for lexer in &registry::registry().lexers {
            if let Err(e) = lexer.tokenise(None, "") {
                failures.push(format!("{}: {}", lexer.config().name, e.0));
            }
        }
        assert!(
            failures.is_empty(),
            "lexers failed to compile:\n{}",
            failures.join("\n")
        );
    }

    /// chroma's own lexer tests (`lexers/testdata`: `<name>[.<case>].actual`
    /// against `.expected`, and `<name>/*.actual`), coalesced and compared
    /// token for token. Needs a chroma checkout: set `CHROMA_TESTDATA` to its
    /// `lexers/testdata` directory.
    #[test]
    #[ignore = "needs CHROMA_TESTDATA pointing at chroma's lexers/testdata"]
    fn chroma_testdata_conformance() {
        let Some(dir) = std::env::var_os("CHROMA_TESTDATA") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        let mut cases: Vec<(String, std::path::PathBuf)> = Vec::new();
        let mut entries: Vec<_> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("{e}"))
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        entries.sort();
        for path in entries {
            let name = path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            if name == "analysis" {
                continue;
            }
            if path.is_dir() {
                let mut subs: Vec<_> = std::fs::read_dir(&path)
                    .unwrap_or_else(|e| panic!("{e}"))
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().is_some_and(|x| x == "actual"))
                    .collect();
                subs.sort();
                cases.extend(subs.into_iter().map(|p| (name.clone(), p)));
            } else if path.extension().is_some_and(|x| x == "actual") {
                let base = name.split('.').next().unwrap_or_default().to_string();
                cases.push((base, path));
            }
        }
        let mut failures = Vec::new();
        for (lexer_name, path) in &cases {
            let Some(lexer) = registry::get(lexer_name) else {
                failures.push(format!("{}: no lexer {lexer_name}", path.display()));
                continue;
            };
            let source = std::fs::read_to_string(path).unwrap_or_default();
            let expected =
                std::fs::read_to_string(path.with_extension("expected")).unwrap_or_default();
            match tokenise(&lexer, &source) {
                Ok(tokens) => {
                    let got = token_json(&tokens);
                    if got != expected {
                        if let Some(out) = std::env::var_os("CHROMA_TESTDATA_OUT") {
                            let file = path
                                .file_name()
                                .map(|f| f.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            let _ = std::fs::write(
                                std::path::Path::new(&out).join(file + ".got"),
                                &got,
                            );
                        }
                        failures.push(format!("{}: tokens differ", path.display()));
                    }
                }
                Err(e) => failures.push(format!("{}: {e}", path.display())),
            }
        }
        assert!(
            failures.is_empty(),
            "{} of {} conformance failures:\n{}",
            failures.len(),
            cases.len(),
            failures.join("\n")
        );
    }

    /// chroma's JSON formatter output for a token list.
    fn token_json(tokens: &[Token]) -> String {
        let mut out = String::from("[\n");
        for (i, t) in tokens.iter().enumerate() {
            if i > 0 {
                out.push_str(",\n");
            }
            out.push_str(&format!(
                "  {{\"type\":\"{}\",\"value\":{}}}",
                t.ty.name(),
                json_string(&t.value)
            ));
        }
        out.push_str("\n]\n");
        out
    }

    fn json_string(s: &str) -> String {
        let mut out = String::from("\"");
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\u{8}' => out.push_str("\\b"),
                '\u{c}' => out.push_str("\\f"),
                '<' => out.push_str("\\u003c"),
                '>' => out.push_str("\\u003e"),
                '&' => out.push_str("\\u0026"),
                '\u{2028}' => out.push_str("\\u2028"),
                '\u{2029}' => out.push_str("\\u2029"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }
}
